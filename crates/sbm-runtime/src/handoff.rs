//! EFI handoff file and boot ESP selection from Spec 03.
//!
//! `\EFI\sbm\handoff\current.json` is parsed with `sbm-protocol`. Handoff
//! metadata comes from volatile EFI variables (`LoaderDevicePartUUID`,
//! `SbmHandoffVersion`, `SbmBootAttemptId`, `SbmHandoffSha256`) or legacy
//! kernel command-line parameters (`sbm.esp_partuuid`, `sbm.handoff_sha256`).
//! When a digest is supplied, the exact bytes must match or the payload is
//! rejected. Without a verified handoff, Core records a local diagnostic and
//! does not emit an integrated boot-success report.

use sbm_protocol::{EfiHandoffPayload, RecoveryLaunchMode};
use sbm_types::{CpuArch, SCHEMA_VERSION_V1};

use crate::sha256_hex;

pub const ESP_PARTTYPE_GUID: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffLoad {
    pub payload: Option<EfiHandoffPayload>,
    pub verified: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HandoffContext {
    pub expected_sha256: Option<String>,
    pub expected_attempt_id: Option<String>,
    pub expected_version: Option<String>,
    pub expected_arch: Option<CpuArch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EspCandidate {
    pub path: String,
    pub partuuid: Option<String>,
    pub parttype: Option<String>,
    pub fstype: Option<String>,
    pub parent_disk: Option<String>,
}

pub fn launch_mode_label(mode: &RecoveryLaunchMode) -> &'static str {
    match mode {
        RecoveryLaunchMode::Interactive => "interactive",
        RecoveryLaunchMode::RemoteAssist => "remote-assist",
        RecoveryLaunchMode::DiagnosticOnly => "diagnostic-only",
        RecoveryLaunchMode::PlannedOperation { .. } => "planned-operation",
    }
}

pub fn native_cpu_arch() -> Option<CpuArch> {
    match std::env::consts::ARCH {
        "x86_64" => Some(CpuArch::X86_64),
        "aarch64" => Some(CpuArch::Aarch64),
        "x86" => Some(CpuArch::Ia32),
        _ => None,
    }
}

/// Decode a Linux efivarfs file (`/sys/firmware/efi/efivars/<Name>-<Guid>`),
/// stripping the 4-byte UEFI variable attribute header and NUL-terminated
/// UTF-16LE payload.
pub fn decode_efivar_utf16(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 6 {
        return None;
    }
    let payload = &bytes[4..];
    let mut units = Vec::with_capacity(payload.len() / 2);
    for chunk in payload.as_chunks::<2>().0 {
        let code = u16::from_le_bytes(*chunk);
        if code == 0 {
            break;
        }
        units.push(code);
    }
    let decoded = String::from_utf16(&units).ok()?;
    let trimmed = decoded.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn load_handoff(bytes: Option<&[u8]>, cmdline: &str) -> HandoffLoad {
    let ctx = HandoffContext {
        expected_sha256: cmdline_value(cmdline, "sbm.handoff_sha256"),
        ..HandoffContext::default()
    };
    verify_handoff(bytes, &ctx)
}

pub fn verify_handoff(bytes: Option<&[u8]>, ctx: &HandoffContext) -> HandoffLoad {
    let Some(bytes) = bytes else {
        return HandoffLoad {
            payload: None,
            verified: false,
            detail: "no EFI handoff file".to_string(),
        };
    };
    if let Some(expected_ver) = ctx.expected_version.as_deref()
        && expected_ver.trim() != SCHEMA_VERSION_V1
    {
        return HandoffLoad {
            payload: None,
            verified: false,
            detail: format!("unsupported handoff version variable ({expected_ver})"),
        };
    }
    if let Some(expected) = ctx.expected_sha256.as_deref() {
        let actual = sha256_hex(bytes);
        if !actual.eq_ignore_ascii_case(expected.trim()) {
            return HandoffLoad {
                payload: None,
                verified: false,
                detail: format!("handoff SHA-256 mismatch (file {actual})"),
            };
        }
    }
    match serde_json::from_slice::<EfiHandoffPayload>(bytes) {
        Ok(payload) => {
            if payload.schema_version != SCHEMA_VERSION_V1 {
                return HandoffLoad {
                    payload: None,
                    verified: false,
                    detail: format!(
                        "handoff schema version {} is unsupported",
                        payload.schema_version
                    ),
                };
            }
            if payload.attempt_id.trim().is_empty() {
                return HandoffLoad {
                    payload: None,
                    verified: false,
                    detail: "handoff attempt_id is empty".to_string(),
                };
            }
            if let Some(expected_attempt) = ctx.expected_attempt_id.as_deref()
                && payload.attempt_id != expected_attempt.trim()
            {
                return HandoffLoad {
                    payload: None,
                    verified: false,
                    detail: format!(
                        "handoff attempt_id {} does not match EFI variable {}",
                        payload.attempt_id, expected_attempt
                    ),
                };
            }
            if let Some(expected_arch) = ctx.expected_arch
                && payload.platform.arch != expected_arch
            {
                return HandoffLoad {
                    payload: None,
                    verified: false,
                    detail: format!(
                        "handoff platform arch {:?} does not match native {:?}",
                        payload.platform.arch, expected_arch
                    ),
                };
            }
            let verified = ctx.expected_sha256.is_some();
            let detail = if verified {
                format!(
                    "{} attempt {} (hash verified)",
                    launch_mode_label(&payload.launch_mode),
                    payload.attempt_id
                )
            } else {
                format!(
                    "{} attempt {} (unverified: no handoff digest provided)",
                    launch_mode_label(&payload.launch_mode),
                    payload.attempt_id
                )
            };
            HandoffLoad {
                payload: Some(payload),
                verified,
                detail,
            }
        }
        Err(err) => HandoffLoad {
            payload: None,
            verified: false,
            detail: format!("handoff JSON rejected: {err}"),
        },
    }
}

pub fn select_boot_esp(
    candidates: &[EspCandidate],
    expected_partuuid: Option<&str>,
    backing_disk: Option<&str>,
) -> Result<EspCandidate, String> {
    let esps: Vec<&EspCandidate> = candidates
        .iter()
        .filter(|cand| {
            let is_esp = cand
                .parttype
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case(ESP_PARTTYPE_GUID));
            let is_vfat = cand
                .fstype
                .as_deref()
                .is_some_and(|fs| fs.eq_ignore_ascii_case("vfat"));
            is_esp && is_vfat
        })
        .collect();

    if let Some(expected) = expected_partuuid.map(str::trim).filter(|s| !s.is_empty()) {
        let matched: Vec<&EspCandidate> = esps
            .into_iter()
            .filter(|cand| {
                cand.partuuid
                    .as_deref()
                    .is_some_and(|u| u.eq_ignore_ascii_case(expected))
            })
            .collect();
        if matched.is_empty() {
            return Err(format!(
                "boot ESP PARTUUID {expected} not found among VFAT GPT ESP partitions"
            ));
        }
        if matched.len() > 1 {
            return Err(format!(
                "multiple partitions matched boot ESP PARTUUID {expected}"
            ));
        }
        let chosen = matched[0];
        if let (Some(backing), Some(parent)) = (backing_disk, chosen.parent_disk.as_deref())
            && parent != backing
        {
            return Err(format!(
                "boot ESP {} sits on {parent}, which does not match verity backing disk {backing}",
                chosen.path
            ));
        }
        return Ok(chosen.clone());
    }

    if let Some(backing) = backing_disk {
        let on_backing: Vec<&EspCandidate> = esps
            .iter()
            .copied()
            .filter(|cand| cand.parent_disk.as_deref() == Some(backing))
            .collect();
        if on_backing.len() == 1 {
            return Ok(on_backing[0].clone());
        }
        if on_backing.len() > 1 {
            return Err(format!(
                "multiple ESP partitions found on backing disk {backing}"
            ));
        }
        return Err(format!("no VFAT GPT ESP found on backing disk {backing}"));
    }

    if esps.len() == 1 {
        return Ok(esps[0].clone());
    }
    if esps.is_empty() {
        Err("no VFAT GPT ESP partition found".to_string())
    } else {
        Err(format!(
            "ambiguous ESP selection ({} ESPs found without boot PARTUUID or backing disk)",
            esps.len()
        ))
    }
}

pub fn cmdline_value(cmdline: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    cmdline
        .split_whitespace()
        .find_map(|token| token.strip_prefix(&prefix).map(|value| value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sha256_hex;
    use std::path::Path;

    fn golden_bytes() -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../sbm-common/test-vectors/efi-handoff.v1.json");
        std::fs::read(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
    }

    #[test]
    fn golden_handoff_verifies_when_the_cmdline_hash_matches() {
        let bytes = golden_bytes();
        let hash = sha256_hex(&bytes);
        let loaded = load_handoff(
            Some(&bytes),
            &format!("console=tty0 sbm.handoff_sha256={hash}"),
        );
        assert!(loaded.verified);
        let payload = loaded.payload.expect("payload");
        assert_eq!(payload.schema_version, "1.0");
        assert_eq!(payload.discovered_targets.len(), 1);
        assert!(loaded.detail.contains("interactive"));
    }

    #[test]
    fn mismatched_hash_drops_the_payload() {
        let bytes = golden_bytes();
        let loaded = load_handoff(Some(&bytes), "sbm.handoff_sha256=deadbeef");
        assert!(loaded.payload.is_none());
        assert!(!loaded.verified);
        assert!(loaded.detail.contains("mismatch"));
    }

    #[test]
    fn missing_hash_keeps_the_payload_unverified() {
        let bytes = golden_bytes();
        let loaded = load_handoff(Some(&bytes), "console=ttyS0");
        assert!(!loaded.verified);
        assert!(loaded.payload.is_some());
        assert!(loaded.detail.contains("unverified"));
    }

    #[test]
    fn mismatched_attempt_id_or_arch_is_rejected() {
        let bytes = golden_bytes();
        let hash = sha256_hex(&bytes);
        let ctx = HandoffContext {
            expected_sha256: Some(hash.clone()),
            expected_attempt_id: Some("wrong-attempt".to_string()),
            expected_version: Some("1.0".to_string()),
            expected_arch: None,
        };
        let loaded = verify_handoff(Some(&bytes), &ctx);
        assert!(!loaded.verified);
        assert!(loaded.payload.is_none());

        let wrong_arch = HandoffContext {
            expected_sha256: Some(hash),
            expected_attempt_id: None,
            expected_version: Some("1.0".to_string()),
            expected_arch: Some(CpuArch::Ia32),
        };
        let loaded = verify_handoff(Some(&bytes), &wrong_arch);
        assert!(!loaded.verified);
        assert!(loaded.payload.is_none());
    }

    #[test]
    fn decodes_efivar_utf16_payload() {
        let mut raw = vec![0x06, 0x00, 0x00, 0x00];
        for ch in "d611d160-e840-477b-b81a-b308ed280636\0".encode_utf16() {
            raw.extend_from_slice(&ch.to_le_bytes());
        }
        assert_eq!(
            decode_efivar_utf16(&raw).as_deref(),
            Some("d611d160-e840-477b-b81a-b308ed280636")
        );
    }

    #[test]
    fn select_boot_esp_skips_decoy_and_enforces_backing_disk() {
        let decoy = EspCandidate {
            path: "/dev/vda1".to_string(),
            partuuid: Some("11111111-1111-1111-1111-111111111111".to_string()),
            parttype: Some(ESP_PARTTYPE_GUID.to_string()),
            fstype: Some("vfat".to_string()),
            parent_disk: Some("/dev/vda".to_string()),
        };
        let real = EspCandidate {
            path: "/dev/vdb1".to_string(),
            partuuid: Some("22222222-2222-2222-2222-222222222222".to_string()),
            parttype: Some(ESP_PARTTYPE_GUID.to_string()),
            fstype: Some("vfat".to_string()),
            parent_disk: Some("/dev/vdb".to_string()),
        };
        let list = [decoy.clone(), real.clone()];

        // Decoy enumerated first; expected PARTUUID selects real boot ESP.
        let picked = select_boot_esp(
            &list,
            Some("22222222-2222-2222-2222-222222222222"),
            Some("/dev/vdb"),
        )
        .unwrap();
        assert_eq!(picked.path, "/dev/vdb1");

        // Parent disk mismatch is rejected even if PARTUUID matched decoy.
        let err = select_boot_esp(
            &list,
            Some("11111111-1111-1111-1111-111111111111"),
            Some("/dev/vdb"),
        )
        .unwrap_err();
        assert!(err.contains("does not match verity backing disk"));

        // Unknown PARTUUID never falls back to decoy.
        assert!(
            select_boot_esp(&list, Some("33333333-3333-3333-3333-333333333333"), None).is_err()
        );

        // Ambiguous without PARTUUID or backing disk is rejected.
        assert!(select_boot_esp(&list, None, None).is_err());
    }
}
