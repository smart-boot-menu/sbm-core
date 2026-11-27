//! Simulated `sbm-cored` for native development on macOS and Linux desktops.
//!
//! A session directory (written by `sbm-preview` in `sbm-uefi`) describes one
//! simulated computer in the formats `sbm-cored` reads on real hardware:
//! `lsblk.json`, `ip-addr.json`, `meminfo`, `cmdline`, and the EFI handoff on
//! `esp/`. Requests from `sbm-ui` are answered with the production parsers,
//! handoff verification and plan validation. Nothing here touches a physical
//! disk, an EFI variable, or a host service; launches and power actions are
//! recorded as files and acknowledged as simulated.

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sbm_protocol::BootSuccessReport;
use sbm_runtime::discover::{backing_disk, infer_installed_os, parse_lsblk};
use sbm_runtime::handoff::{
    EspCandidate, load_handoff, read_integrated, select_integrated_esp, write_esp_record,
};
use sbm_runtime::hostinfo::{parse_ip_addr, parse_meminfo};
use sbm_runtime::ipc::{
    ClientRequest, ClientResponse, HardwareInfo, LaunchTool, PowerAction, SystemSnapshot,
    ToolReport, empty_snapshot, tool_catalog,
};
use sbm_runtime::plan::validate_plan;
use sbm_runtime::{HandoffLoad, SLOT_A_TARGET_ID, TORAM_AUTO_MEMTOTAL_KIB};
use sbm_types::{BootTargetId, ComponentKind, CpuArch, SCHEMA_VERSION_V1, SbmErrorCode};
use serde::Deserialize;

pub const HANDOFF_PATH: &str = "esp/EFI/sbm/handoff/current.json";
pub const COMMAND_PATH: &str = "esp/EFI/sbm/handoff/requested-command.json";
pub const SUCCESS_PATH: &str = "esp/EFI/sbm/state/boot-success.json";
pub const POWER_PATH: &str = "power";

/// What the simulated computer does after `sbm-ui` asked for a power action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerOutcome {
    Reboot,
    Shutdown,
}

impl PowerOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reboot => "reboot",
            Self::Shutdown => "shutdown",
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct Hardware {
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    vendor: String,
    #[serde(default)]
    product: String,
    #[serde(default)]
    cpu_model: String,
    #[serde(default)]
    cores: usize,
    #[serde(default)]
    gpus: Vec<String>,
    #[serde(default)]
    arch: String,
    #[serde(default)]
    wireless: Vec<String>,
}

pub struct Session {
    dir: PathBuf,
}

impl Session {
    pub fn open(dir: &Path) -> Result<Self, String> {
        if !dir.join("SIMULATED").is_file() {
            return Err(format!(
                "{} is not an SBM simulation session (no SIMULATED marker). Create one with `sbm-preview --materialize DIR`.",
                dir.display()
            ));
        }
        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn read(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.dir.join(name)).ok()
    }

    fn line(&self, name: &str) -> Option<String> {
        self.read(name)
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    }

    fn hardware(&self) -> Hardware {
        self.read("hardware.json")
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn scenario(&self) -> String {
        self.read("scenario")
            .and_then(|text| text.lines().next().map(str::to_string))
            .unwrap_or_else(|| "unknown".into())
    }

    pub fn handoff(&self) -> HandoffLoad {
        let arch = match self.hardware().arch.as_str() {
            "x86_64" => Some(CpuArch::X86_64),
            "aarch64" => Some(CpuArch::Aarch64),
            _ => None,
        };
        let mut load = read_integrated(
            |name| fs::read(self.dir.join("efivars").join(name)).ok(),
            arch,
            |partuuid| {
                let source = self
                    .line("root-source")
                    .ok_or("simulated backing device is missing")?;
                let lsblk = self
                    .read("lsblk.json")
                    .ok_or("simulated disk inventory is missing")?;
                let disks = parse_lsblk(&lsblk, Some(&source)).map_err(|err| err.to_string())?;
                let backing = backing_disk(&disks, &source);
                let candidates = disks
                    .iter()
                    .flat_map(|disk| {
                        disk.partitions.iter().map(|part| EspCandidate {
                            path: part.device_path.clone(),
                            partuuid: part.partuuid.clone(),
                            parttype: part.parttype.clone(),
                            fstype: part.fstype.clone(),
                            parent_disk: Some(disk.device_path.clone()),
                        })
                    })
                    .collect::<Vec<_>>();
                select_integrated_esp(
                    &candidates,
                    partuuid,
                    backing.map(|disk| disk.device_path.as_str()),
                )?;
                fs::read(self.dir.join(HANDOFF_PATH)).map_err(|err| err.to_string())
            },
        );
        if !load.verified
            && !self.dir.join("efivars").exists()
            && let Some(cmdline) = self.read("cmdline")
        {
            let bytes = fs::read(self.dir.join(HANDOFF_PATH)).ok();
            load = load_handoff(bytes.as_deref(), &cmdline);
        }
        load.detail = format!("{} [SIMULATED]", load.detail);
        load
    }

    pub fn snapshot(&self) -> SystemSnapshot {
        let hw = self.hardware();
        let mut snap = empty_snapshot();
        snap.hostname = if hw.hostname.is_empty() {
            "sbm-sim".into()
        } else {
            hw.hostname.clone()
        };
        snap.kernel = format!(
            "SIMULATED (native preview on {}, scenario {})",
            std::env::consts::OS,
            self.scenario()
        );
        snap.arch = if hw.arch.is_empty() {
            std::env::consts::ARCH.into()
        } else {
            hw.arch.clone()
        };
        snap.os_pretty = "SBM Core (SIMULATED)".into();
        snap.memory = parse_meminfo(&self.read("meminfo").unwrap_or_default());
        snap.hardware = HardwareInfo {
            vendor: hw.vendor,
            product: hw.product,
            cpu_model: hw.cpu_model,
            arch: snap.arch.clone(),
            cores: hw.cores.max(1),
            gpus: hw.gpus,
        };
        snap.backing_protected = snap.memory.total_kib < TORAM_AUTO_MEMTOTAL_KIB;
        snap.toram_mode = if snap.backing_protected {
            "backing-protected (SIMULATED)".into()
        } else {
            "ram-resident (SIMULATED)".into()
        };
        let root_source = self.line("root-source");
        snap.backing_disk = root_source.clone();
        match parse_lsblk(
            &self.read("lsblk.json").unwrap_or_default(),
            root_source.as_deref(),
        ) {
            Ok(mut disks) => {
                for disk in &mut disks {
                    disk.health = "SIMULATED: not probed".into();
                }
                if let Some(source) = root_source.as_deref() {
                    snap.backing_disk =
                        backing_disk(&disks, source).map(|disk| disk.device_path.clone());
                }
                snap.installed_os = infer_installed_os(&disks);
                snap.disks = disks;
            }
            Err(err) => eprintln!("sbm-sim: lsblk.json: {err}"),
        }
        if let Some(json) = self.read("ip-addr.json")
            && let Ok(mut nics) = parse_ip_addr(&json)
        {
            for nic in &mut nics {
                nic.wireless = hw.wireless.contains(&nic.name);
            }
            snap.nics = nics;
        }
        snap.tools = simulated_tools();
        let handoff = self.handoff();
        snap.boot_targets = handoff.boot_targets();
        snap.handoff = handoff.detail;
        snap.handoff_verified = handoff.verified;
        snap.renderer = "SIMULATED: native desktop window".into();
        snap.remote_enabled = false;
        snap.remote_detail = "SIMULATED: remote access is not available in native preview".into();
        snap
    }

    /// Answer one request. `Some(outcome)` means the simulated computer
    /// should leave Core after the response is sent.
    pub fn handle(&self, request: ClientRequest) -> (ClientResponse, Option<PowerOutcome>) {
        match request {
            ClientRequest::Snapshot => (
                ClientResponse::Snapshot {
                    snapshot: Box::new(self.snapshot()),
                },
                None,
            ),
            ClientRequest::UiReady => (ack(self.ui_ready()), None),
            ClientRequest::Launch { tool } => {
                let name = match tool {
                    LaunchTool::Gparted => "GParted",
                    LaunchTool::Terminal => "the terminal",
                };
                (
                    ack(format!(
                        "SIMULATED: {name} was not started. Native preview never runs disk tools."
                    )),
                    None,
                )
            }
            ClientRequest::ToolProbe => (
                ClientResponse::Tools {
                    tools: simulated_tools(),
                },
                None,
            ),
            ClientRequest::RemoteStatus
            | ClientRequest::RemoteEnable
            | ClientRequest::RemoteDisable => (
                ClientResponse::Remote {
                    enabled: false,
                    detail: "SIMULATED: remote access is not available in native preview".into(),
                    password: None,
                },
                None,
            ),
            ClientRequest::ValidatePlan { plan } => {
                let snap = self.snapshot();
                let backing = snap.backing_disk.as_deref().and_then(|path| {
                    backing_disk(&snap.disks, path).map(|disk| disk.identity.stable_id.clone())
                });
                let decision = validate_plan(&plan, snap.backing_protected, backing.as_deref());
                (ClientResponse::Plan { decision }, None)
            }
            ClientRequest::Power { action } => self.power(action),
        }
    }

    fn ui_ready(&self) -> String {
        let handoff = self.handoff();
        let Some(payload) = handoff.payload.as_ref().filter(|_| handoff.verified) else {
            return "SIMULATED: desktop ready (handoff unverified; no confirmation written)".into();
        };
        let report = BootSuccessReport {
            schema_version: SCHEMA_VERSION_V1.to_string(),
            attempt_id: payload.attempt_id.clone(),
            target_id: BootTargetId(SLOT_A_TARGET_ID.into()),
            confirmed_by: ComponentKind::SbmCoreUki,
            confirmed_at_unix: unix_now(),
        };
        match serde_json::to_vec_pretty(&report)
            .map_err(|err| err.to_string())
            .and_then(|bytes| {
                write_esp_record(&self.dir.join(SUCCESS_PATH), &bytes)
                    .map_err(|err| err.to_string())
            }) {
            Ok(()) => format!(
                "SIMULATED: desktop ready (confirmed {})",
                payload.attempt_id
            ),
            Err(err) => format!("SIMULATED: desktop ready (report error: {err})"),
        }
    }

    fn power(&self, action: PowerAction) -> (ClientResponse, Option<PowerOutcome>) {
        let (message, outcome) = match action {
            PowerAction::Reboot => ("SIMULATED: rebooting", PowerOutcome::Reboot),
            PowerAction::Shutdown => ("SIMULATED: shutting down", PowerOutcome::Shutdown),
            PowerAction::BootManager => (
                "SIMULATED: returning to the boot manager",
                PowerOutcome::Reboot,
            ),
            PowerAction::BootTarget { target_id } => {
                let command = match self
                    .handoff()
                    .boot_request(BootTargetId(target_id), unix_now())
                {
                    Ok(command) => command,
                    Err((message, code)) => return (error(message, code), None),
                };
                if let Err(err) = serde_json::to_vec(&command)
                    .map_err(|err| err.to_string())
                    .and_then(|bytes| {
                        write_esp_record(&self.dir.join(COMMAND_PATH), &bytes)
                            .map_err(|err| err.to_string())
                    })
                {
                    return (error(err, SbmErrorCode::StorageIoError), None);
                }
                (
                    "SIMULATED: requested the next boot target",
                    PowerOutcome::Reboot,
                )
            }
        };
        if let Err(err) = self.write(POWER_PATH, outcome.as_str().as_bytes()) {
            return (error(err, SbmErrorCode::StorageIoError), None);
        }
        (ack(message), Some(outcome))
    }

    fn write(&self, relative: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, bytes).map_err(|err| format!("write {}: {err}", tmp.display()))?;
        fs::rename(&tmp, &path).map_err(|err| err.to_string())
    }
}

fn simulated_tools() -> Vec<ToolReport> {
    tool_catalog()
        .into_iter()
        .map(|(name, paths)| ToolReport {
            name: name.to_string(),
            path: paths[0].to_string(),
            present: false,
            detail: "SIMULATED: not probed".to_string(),
        })
        .collect()
}

fn ack(message: impl Into<String>) -> ClientResponse {
    ClientResponse::Ack {
        message: message.into(),
    }
}

fn error(message: impl Into<String>, code: SbmErrorCode) -> ClientResponse {
    ClientResponse::Error {
        message: message.into(),
        code: Some(code),
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
