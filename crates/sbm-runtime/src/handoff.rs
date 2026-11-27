//! EFI handoff file and boot ESP selection from Spec 03.
//!
//! Integrated boot requires all five Spec 03 EFI variables. Legacy
//! command-line handoffs are available for standalone diagnostics.

use std::io::Read;
use std::path::Path;

use sbm_protocol::{
    BootCommand, EfiHandoffPayload, LocalBootRequest, RecoveryLaunchMode, format_uuid_v4,
    is_uuid_v4,
};
use sbm_types::{
    BootTarget, BootTargetId, BootTargetKind, ComponentKind, CpuArch, SCHEMA_VERSION_V1,
    SbmErrorCode,
};

use crate::sha256_hex;

pub const ESP_PARTTYPE_GUID: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffLoad {
    pub payload: Option<EfiHandoffPayload>,
    pub verified: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct HandoffContext {
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

const SYSTEMD_VENDOR_GUID: &str = "4a67b082-0a4c-41cf-b6c7-440b29bb8c4f";
const SBM_VENDOR_GUID: &str = "d8f4a21c-7b6e-4c91-9e32-8a5c1f0e3b40";
pub const BOOT_ESP_VARIABLE: &str = "LoaderDevicePartUUID-4a67b082-0a4c-41cf-b6c7-440b29bb8c4f";

pub fn read_integrated(
    read_var: impl Fn(&str) -> Option<Vec<u8>>,
    expected_arch: Option<CpuArch>,
    read_file: impl FnOnce(&str) -> Result<Vec<u8>, String>,
) -> HandoffLoad {
    fn read(read_var: &impl Fn(&str) -> Option<Vec<u8>>, name: &str) -> Result<String, String> {
        read_var(name)
            .and_then(|bytes| decode_efivar_utf16(&bytes))
            .ok_or_else(|| format!("missing or invalid EFI variable {name}"))
    }

    let load = || -> Result<HandoffLoad, String> {
        let partuuid = read(&read_var, BOOT_ESP_VARIABLE)?;
        if !is_partuuid(&partuuid) {
            return Err("invalid boot ESP PARTUUID".into());
        }
        let selection = read(
            &read_var,
            &format!("LoaderEntrySelected-{SYSTEMD_VENDOR_GUID}"),
        )?;
        if selection != crate::SLOT_A_TARGET_ID {
            return Err("EFI selection does not name the Slot A recovery UKI".into());
        }
        let version = read(&read_var, &format!("SbmHandoffVersion-{SBM_VENDOR_GUID}"))?;
        let attempt = read(&read_var, &format!("SbmBootAttemptId-{SBM_VENDOR_GUID}"))?;
        let digest = read(&read_var, &format!("SbmHandoffSha256-{SBM_VENDOR_GUID}"))?;
        if version != SCHEMA_VERSION_V1 || !is_uuid_v4(&attempt) {
            return Err("unsupported handoff version or invalid boot attempt ID".into());
        }
        if !lower_hex(&digest, 64) {
            return Err("invalid handoff SHA-256 variable".into());
        }
        let arch = expected_arch.ok_or("unknown native architecture")?;
        let bytes = read_file(&partuuid)?;
        let mut result = verify_handoff(
            Some(&bytes),
            &HandoffContext {
                expected_sha256: Some(digest),
                expected_attempt_id: Some(attempt),
                expected_version: Some(version),
                expected_arch: Some(arch),
            },
        );
        if result.verified {
            result.detail = format!("{} [integrated EFI handoff]", result.detail);
        }
        Ok(result)
    };
    load().unwrap_or_else(|detail| HandoffLoad {
        payload: None,
        verified: false,
        detail,
    })
}

fn lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_partuuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

impl HandoffLoad {
    pub fn boot_targets(&self) -> Vec<BootTarget> {
        let Some(payload) = self.payload.as_ref().filter(|_| self.verified) else {
            return Vec::new();
        };
        let targets = &payload.discovered_targets;
        targets
            .iter()
            .filter(|target| {
                target.id.is_valid()
                    && !target.hidden
                    && target.verified_signature != Some(false)
                    && target.kind != BootTargetKind::SbmCore
                    && !target.id.as_str().starts_with("os:")
                    && !target.id.as_str().starts_with("sbm-core:")
                    && targets.iter().filter(|other| other.id == target.id).count() == 1
            })
            .cloned()
            .collect()
    }

    pub fn boot_request(
        &self,
        target_id: BootTargetId,
        now: i64,
    ) -> Result<LocalBootRequest, (String, SbmErrorCode)> {
        if !target_id.is_valid() {
            return Err((
                "target id is not a stable SBM boot identifier".into(),
                SbmErrorCode::InvalidSchema,
            ));
        }
        if !self
            .boot_targets()
            .iter()
            .any(|target| target.id == target_id)
        {
            return Err((
                "that target is unavailable in the integrated handoff".into(),
                SbmErrorCode::TargetNotFound,
            ));
        }
        let payload = self
            .payload
            .as_ref()
            .ok_or_else(|| ("no integrated handoff".into(), SbmErrorCode::TargetNotFound))?;
        let mut seed = [0; 16];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut seed))
            .map_err(|err| {
                (
                    format!("could not generate a boot request ID: {err}"),
                    SbmErrorCode::StorageIoError,
                )
            })?;
        Ok(LocalBootRequest {
            schema_version: SCHEMA_VERSION_V1.into(),
            request_id: format_uuid_v4(seed),
            attempt_id: payload.attempt_id.clone(),
            requested_by: ComponentKind::SbmCoreUki,
            requested_at_unix: now,
            command: BootCommand::BootTarget {
                target_id,
                one_shot: true,
                extra_cmdline_preset: None,
            },
        })
    }
}

pub fn write_esp_record(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let parent = path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    std::fs::create_dir_all(parent)?;
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    std::fs::File::open(parent)?.sync_all()
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
    if bytes.len() < 6 || bytes[..4] != [6, 0, 0, 0] || !bytes.len().is_multiple_of(2) {
        return None;
    }
    let (terminated, end) = bytes[4..].split_at(bytes.len() - 6);
    if end != [0, 0] {
        return None;
    }
    let units: Vec<u16> = terminated
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    if units.is_empty() || units.contains(&0) {
        return None;
    }
    String::from_utf16(&units).ok()
}

pub fn load_handoff(bytes: Option<&[u8]>, cmdline: &str) -> HandoffLoad {
    let ctx = HandoffContext {
        expected_sha256: cmdline_value(cmdline, "sbm.handoff_sha256"),
        ..HandoffContext::default()
    };
    let mut load = verify_handoff(bytes, &ctx);
    if load.verified {
        load.detail.push_str(" [legacy standalone diagnostic]");
        load.verified = false;
    }
    load
}

fn verify_handoff(bytes: Option<&[u8]>, ctx: &HandoffContext) -> HandoffLoad {
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

pub fn select_integrated_esp(
    candidates: &[EspCandidate],
    partuuid: &str,
    backing_disk: Option<&str>,
) -> Result<EspCandidate, String> {
    if !is_partuuid(partuuid) {
        return Err("invalid boot ESP PARTUUID".into());
    }
    let backing = backing_disk.ok_or("verity backing disk is unknown")?;
    let chosen = select_boot_esp(candidates, Some(partuuid), Some(backing))?;
    if chosen.parent_disk.as_deref() != Some(backing) {
        return Err("boot ESP parent is unknown".into());
    }
    Ok(chosen)
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
    fn golden_legacy_handoff_is_diagnostic_only() {
        let bytes = golden_bytes();
        let hash = sha256_hex(&bytes);
        let loaded = load_handoff(
            Some(&bytes),
            &format!("console=tty0 sbm.handoff_sha256={hash}"),
        );
        assert!(!loaded.verified);
        let payload = loaded.payload.expect("payload");
        assert_eq!(payload.schema_version, "1.0");
        assert_eq!(payload.discovered_targets.len(), 4);
        assert!(loaded.detail.contains("interactive"));
    }

    #[test]
    fn integrated_handoff_preserves_multiple_boot_targets() {
        let bytes = golden_bytes();
        let payload: EfiHandoffPayload = serde_json::from_slice(&bytes).unwrap();
        let vars = [
            (
                BOOT_ESP_VARIABLE.into(),
                "00000000-0000-4000-8000-0000000000e5".into(),
            ),
            (
                format!("LoaderEntrySelected-{SYSTEMD_VENDOR_GUID}"),
                crate::SLOT_A_TARGET_ID.into(),
            ),
            (
                format!("SbmHandoffVersion-{SBM_VENDOR_GUID}"),
                SCHEMA_VERSION_V1.into(),
            ),
            (
                format!("SbmBootAttemptId-{SBM_VENDOR_GUID}"),
                payload.attempt_id.clone(),
            ),
            (
                format!("SbmHandoffSha256-{SBM_VENDOR_GUID}"),
                sha256_hex(&bytes),
            ),
        ];
        let load = |omitted| {
            read_integrated(
                |name| {
                    vars.iter()
                        .enumerate()
                        .find(|(n, (var, _))| Some(*n) != omitted && var == name)
                        .map(|(_, (_, value))| {
                            let mut bytes = vec![6, 0, 0, 0];
                            bytes
                                .extend(value.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
                            bytes
                        })
                },
                Some(CpuArch::Aarch64),
                |_| Ok(bytes.clone()),
            )
        };
        let integrated = load(None);
        assert!(integrated.verified);
        let targets = integrated.boot_targets();
        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0].partition_guid, targets[1].partition_guid);
        assert_ne!(targets[0].id, targets[1].id);
        for target in targets {
            let request = integrated.boot_request(target.id, 5).unwrap();
            assert_eq!(request.attempt_id, payload.attempt_id);
            assert!(is_uuid_v4(&request.request_id));
        }
        for (n, (variable, _)) in vars.iter().enumerate() {
            let incomplete = load(Some(n));
            assert!(!incomplete.verified, "{variable}");
            assert!(incomplete.boot_targets().is_empty());
        }
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
