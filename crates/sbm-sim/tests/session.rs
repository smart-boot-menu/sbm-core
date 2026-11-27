//! Contract tests against sessions written by `sbm-preview --materialize`
//! (regenerate with `cargo run -p xtask -- core fixtures` in sbm-uefi).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use sbm_protocol::{BootCommand, BootSuccessReport};
use sbm_runtime::ipc::{ClientRequest, ClientResponse, LaunchTool, PowerAction};
use sbm_sim::{COMMAND_PATH, POWER_PATH, PowerOutcome, SUCCESS_PATH, Session};
use sbm_types::{OsFamily, PartitionProtectionClass, SbmErrorCode};

static NEXT: AtomicU32 = AtomicU32::new(0);

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn session(name: &str) -> (Session, PathBuf) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let dir = std::env::temp_dir().join(format!(
        "sbm-sim-test-{}-{}-{name}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&source, &dir);
    (Session::open(&dir).unwrap(), dir)
}

#[test]
fn refuses_directories_without_the_simulated_marker() {
    let dir = std::env::temp_dir().join(format!("sbm-sim-test-{}-bare", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    assert!(Session::open(&dir).is_err());
}

#[test]
fn snapshot_uses_production_parsers_and_labels_simulation() {
    let (session, _dir) = session("dual-boot");
    let snap = session.snapshot();
    assert!(snap.kernel.contains("SIMULATED"));
    assert!(snap.os_pretty.contains("SIMULATED"));
    assert!(snap.toram_mode.contains("SIMULATED"));
    assert_eq!(snap.arch, "x86_64");
    assert_eq!(snap.disks.len(), 1);
    assert_eq!(snap.backing_disk.as_deref(), Some("/dev/nvme0n1"));
    let families: Vec<OsFamily> = snap.installed_os.iter().map(|os| os.os_family).collect();
    assert!(families.contains(&OsFamily::Windows));
    assert!(families.contains(&OsFamily::Linux));
    assert!(snap.tools.iter().all(|tool| !tool.present));
    assert!(snap.memory.total_kib > 0);
}

#[test]
fn handoff_from_the_uefi_preview_verifies() {
    let (session, _dir) = session("dual-boot");
    let handoff = session.handoff();
    assert!(handoff.verified, "{}", handoff.detail);
    let payload = handoff.payload.unwrap();
    assert!(
        payload
            .discovered_targets
            .iter()
            .any(|target| target.display_name == "Windows")
    );
}

#[test]
fn bitlocker_volume_is_protected() {
    let (session, _dir) = session("bitlocker");
    let snap = session.snapshot();
    let part = snap
        .disks
        .iter()
        .flat_map(|disk| &disk.partitions)
        .find(|part| part.encrypted)
        .expect("BitLocker partition");
    assert_eq!(
        part.protection,
        PartitionProtectionClass::ProtectedEncrypted
    );
    assert!(
        snap.installed_os
            .iter()
            .any(|os| os.display_name.contains("BitLocker"))
    );
}

#[test]
fn ui_ready_confirms_the_selected_target() {
    let (session, dir) = session("dual-boot");
    let (response, outcome) = session.handle(ClientRequest::UiReady);
    assert!(outcome.is_none());
    let ClientResponse::Ack { message } = response else {
        panic!("expected ack");
    };
    assert!(message.contains("SIMULATED"));
    let report: BootSuccessReport =
        serde_json::from_slice(&fs::read(dir.join(SUCCESS_PATH)).unwrap()).unwrap();
    assert_eq!(report.target_id.as_str(), "sbm-core:slot-a");
    assert_eq!(
        report.attempt_id,
        session.handoff().payload.unwrap().attempt_id
    );
}

#[test]
fn launches_are_never_executed() {
    let (session, dir) = session("dual-boot");
    for tool in [LaunchTool::Gparted, LaunchTool::Terminal] {
        let (response, outcome) = session.handle(ClientRequest::Launch { tool });
        assert!(outcome.is_none());
        let ClientResponse::Ack { message } = response else {
            panic!("expected ack");
        };
        assert!(message.starts_with("SIMULATED"));
    }
    assert!(!dir.join(POWER_PATH).exists());
}

#[test]
fn boot_target_writes_a_one_shot_command_and_reboots() {
    let (session, dir) = session("dual-boot");
    let windows = session
        .handoff()
        .payload
        .unwrap()
        .discovered_targets
        .into_iter()
        .find(|target| target.display_name == "Windows")
        .unwrap();
    let (response, outcome) = session.handle(ClientRequest::Power {
        action: PowerAction::BootTarget {
            target_id: windows.id.as_str().to_string(),
        },
    });
    assert!(matches!(response, ClientResponse::Ack { .. }));
    assert_eq!(outcome, Some(PowerOutcome::Reboot));
    let command: BootCommand =
        serde_json::from_slice(&fs::read(dir.join(COMMAND_PATH)).unwrap()).unwrap();
    assert!(matches!(
        command,
        BootCommand::BootTarget { ref target_id, one_shot: true, .. } if *target_id == windows.id
    ));
    assert_eq!(fs::read_to_string(dir.join(POWER_PATH)).unwrap(), "reboot");
}

#[test]
fn unknown_boot_target_is_rejected_like_production() {
    let (session, dir) = session("dual-boot");
    let (response, outcome) = session.handle(ClientRequest::Power {
        action: PowerAction::BootTarget {
            target_id: "efi:00000000-0000-0000-0000-000000000000:000000000000".into(),
        },
    });
    assert!(outcome.is_none());
    assert!(matches!(
        response,
        ClientResponse::Error {
            code: Some(SbmErrorCode::TargetNotFound),
            ..
        }
    ));
    assert!(!dir.join(COMMAND_PATH).exists());
}

#[test]
fn shutdown_is_recorded() {
    let (session, dir) = session("dual-boot");
    let (_, outcome) = session.handle(ClientRequest::Power {
        action: PowerAction::Shutdown,
    });
    assert_eq!(outcome, Some(PowerOutcome::Shutdown));
    assert_eq!(
        fs::read_to_string(dir.join(POWER_PATH)).unwrap(),
        "shutdown"
    );
}
