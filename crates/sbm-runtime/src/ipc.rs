//! Local length-prefixed JSON socket between the shell and `sbm-cored`.
//!
//! The client cannot pass a shell command. Launch and power requests are enums.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use sbm_types::{SbmErrorCode, StorageOperationPlan};
use serde::{Deserialize, Serialize};

use crate::discover::{DiscoveredDisk, InstalledOs};
use crate::hostinfo::{MemoryInfo, NicStatus};
use crate::plan::PlanDecision;

pub const SOCKET_PATH: &str = "/run/sbm/cored.sock";
const MAX_MESSAGE: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareInfo {
    pub vendor: String,
    pub product: String,
    pub cpu_model: String,
    pub arch: String,
    pub cores: usize,
    pub gpus: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemSnapshot {
    pub hostname: String,
    pub kernel: String,
    pub arch: String,
    pub os_pretty: String,
    pub sbm_version: String,
    pub hardware: HardwareInfo,
    pub memory: MemoryInfo,
    pub toram_mode: String,
    pub backing_protected: bool,
    pub backing_disk: Option<String>,
    pub uptime_secs: u64,
    pub disks: Vec<DiscoveredDisk>,
    pub installed_os: Vec<InstalledOs>,
    pub nics: Vec<NicStatus>,
    pub tools: Vec<ToolReport>,
    pub handoff: String,
    pub handoff_verified: bool,
    pub renderer: String,
    pub remote_enabled: bool,
    pub remote_detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolReport {
    pub name: String,
    pub path: String,
    pub present: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchTool {
    Gparted,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerAction {
    Reboot,
    Shutdown,
    BootManager,
    BootTarget { target_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ClientRequest {
    Snapshot,
    UiReady,
    Launch { tool: LaunchTool },
    Power { action: PowerAction },
    RemoteStatus,
    RemoteEnable,
    RemoteDisable,
    ToolProbe,
    ValidatePlan { plan: Box<StorageOperationPlan> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ClientResponse {
    Snapshot {
        snapshot: Box<SystemSnapshot>,
    },
    Tools {
        tools: Vec<ToolReport>,
    },
    Remote {
        enabled: bool,
        detail: String,
        password: Option<String>,
    },
    Plan {
        decision: PlanDecision,
    },
    Ack {
        message: String,
    },
    Error {
        message: String,
        code: Option<SbmErrorCode>,
    },
}

pub fn transact(request: &ClientRequest) -> Result<ClientResponse, String> {
    let mut stream = UnixStream::connect(SOCKET_PATH)
        .map_err(|err| format!("sbm-cored is not reachable at {SOCKET_PATH}: {err}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(8)))
        .map_err(|err| err.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(8)))
        .map_err(|err| err.to_string())?;
    write_message(&mut stream, request)?;
    read_message(&mut stream)
}

pub fn write_message<T: Serialize>(stream: &mut impl Write, value: &T) -> Result<(), String> {
    let body = serde_json::to_vec(value).map_err(|err| err.to_string())?;
    if body.len() > MAX_MESSAGE {
        return Err("message exceeds 1 MiB".to_string());
    }
    let len = u32::try_from(body.len()).map_err(|err| err.to_string())?;
    stream
        .write_all(&len.to_be_bytes())
        .map_err(|err| err.to_string())?;
    stream.write_all(&body).map_err(|err| err.to_string())?;
    stream.flush().map_err(|err| err.to_string())
}

pub fn read_message<T: for<'de> Deserialize<'de>>(stream: &mut impl Read) -> Result<T, String> {
    let mut len_buf = [0_u8; 4];
    stream
        .read_exact(&mut len_buf)
        .map_err(|err| err.to_string())?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_MESSAGE {
        return Err("message exceeds 1 MiB".to_string());
    }
    let mut body = vec![0_u8; len];
    stream
        .read_exact(&mut body)
        .map_err(|err| err.to_string())?;
    serde_json::from_slice(&body).map_err(|err| err.to_string())
}

pub fn tool_catalog() -> Vec<(&'static str, &'static [&'static str])> {
    vec![
        (
            "gparted",
            &["/usr/libexec/gpartedbin", "/usr/sbin/gpartedbin"],
        ),
        ("parted", &["/usr/sbin/parted", "/usr/bin/parted"]),
        ("e2fsprogs", &["/usr/sbin/mkfs.ext4", "/usr/bin/mkfs.ext4"]),
        ("dosfstools", &["/usr/sbin/mkfs.fat", "/usr/sbin/mkfs.vfat"]),
        (
            "ntfs-3g",
            &[
                "/usr/bin/ntfsresize",
                "/usr/sbin/ntfsresize",
                "/usr/bin/ntfs-3g",
            ],
        ),
        (
            "exfatprogs",
            &["/usr/sbin/mkfs.exfat", "/usr/bin/mkfs.exfat"],
        ),
        ("cryptsetup", &["/usr/sbin/cryptsetup"]),
        ("btrfs-progs", &["/usr/sbin/btrfs", "/usr/bin/btrfs"]),
        (
            "partclone",
            &["/usr/sbin/partclone.ext4", "/usr/bin/partclone"],
        ),
        ("testdisk", &["/usr/bin/testdisk"]),
        ("photorec", &["/usr/bin/photorec"]),
        ("ddrescue", &["/usr/bin/ddrescue"]),
        ("smartctl", &["/usr/sbin/smartctl"]),
        ("nvme", &["/usr/sbin/nvme", "/usr/bin/nvme"]),
        ("efibootmgr", &["/usr/sbin/efibootmgr"]),
        ("rsync", &["/usr/bin/rsync"]),
        ("ssh", &["/usr/bin/ssh"]),
        ("wimlib-imagex", &["/usr/bin/wimlib-imagex"]),
        ("foot", &["/usr/bin/foot"]),
        ("labwc", &["/usr/bin/labwc"]),
        ("wayvnc", &["/usr/bin/wayvnc"]),
    ]
}

pub fn empty_snapshot() -> SystemSnapshot {
    SystemSnapshot {
        hostname: String::new(),
        kernel: String::new(),
        arch: String::new(),
        os_pretty: String::new(),
        sbm_version: crate::SBM_VERSION.to_string(),
        hardware: HardwareInfo {
            vendor: String::new(),
            product: String::new(),
            cpu_model: String::new(),
            arch: String::new(),
            cores: 1,
            gpus: Vec::new(),
        },
        memory: MemoryInfo {
            total_kib: 0,
            available_kib: 0,
        },
        toram_mode: "unknown".to_string(),
        backing_protected: true,
        backing_disk: None,
        uptime_secs: 0,
        disks: Vec::new(),
        installed_os: Vec::new(),
        nics: Vec::new(),
        tools: Vec::new(),
        handoff: "no EFI handoff file".to_string(),
        handoff_verified: false,
        renderer: String::new(),
        remote_enabled: false,
        remote_detail: "remote access is disabled".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn request_roundtrip_uses_length_prefix() {
        let request = ClientRequest::Launch {
            tool: LaunchTool::Gparted,
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &request).unwrap();
        let decoded: ClientRequest = read_message(&mut Cursor::new(buffer)).unwrap();
        assert_eq!(decoded, request);
    }
}
