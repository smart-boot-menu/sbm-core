//! EFI handoff file from Spec 03.
//!
//! `\EFI\sbm\handoff\current.json` is parsed with `sbm-protocol`. When the
//! kernel command line carries `sbm.handoff_sha256`, the bytes must match or
//! the payload is rejected. Without that parameter the payload is still shown,
//! marked unverified, so a lab image can demonstrate the file before
//! `sbm-uefi` is passing the hash.

use sbm_protocol::{EfiHandoffPayload, RecoveryLaunchMode};

use crate::sha256_hex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffLoad {
    pub payload: Option<EfiHandoffPayload>,
    pub verified: bool,
    pub detail: String,
}

pub fn launch_mode_label(mode: &RecoveryLaunchMode) -> &'static str {
    match mode {
        RecoveryLaunchMode::Interactive => "interactive",
        RecoveryLaunchMode::RemoteAssist => "remote-assist",
        RecoveryLaunchMode::DiagnosticOnly => "diagnostic-only",
        RecoveryLaunchMode::PlannedOperation { .. } => "planned-operation",
    }
}

pub fn load_handoff(bytes: Option<&[u8]>, cmdline: &str) -> HandoffLoad {
    let Some(bytes) = bytes else {
        return HandoffLoad {
            payload: None,
            verified: false,
            detail: "no EFI handoff file".to_string(),
        };
    };
    let expected = cmdline_value(cmdline, "sbm.handoff_sha256");
    if let Some(expected) = expected.as_deref() {
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
            let verified = expected.is_some();
            let detail = if verified {
                format!(
                    "{} attempt {} (hash verified)",
                    launch_mode_label(&payload.launch_mode),
                    payload.attempt_id
                )
            } else {
                format!(
                    "{} attempt {} (unverified: no sbm.handoff_sha256 on the kernel command line)",
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
}
