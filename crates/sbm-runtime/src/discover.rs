//! Parse `lsblk --json` into stable disk identities.
//!
//! Device node names are kept for display and for matching the booted root.
//! Plans address disks by [`sbm_types::DiskIdentity`], never by `/dev/sdX`.

use crate::sha256_hex;
use sbm_types::{DiskBus, DiskIdentity, OsFamily, PartitionProtectionClass};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const ESP_TYPE: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
const APPLE_APFS: &str = "7c3457ef-0000-11aa-aa11-00306543ecac";
const APPLE_HFS: &str = "48465300-0000-11aa-aa11-00306543ecac";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredPartition {
    pub name: String,
    pub device_path: String,
    pub partuuid: Option<String>,
    pub parttype: Option<String>,
    pub partlabel: Option<String>,
    pub fstype: Option<String>,
    pub label: Option<String>,
    pub mountpoint: Option<String>,
    pub size_bytes: u64,
    pub start_bytes: u64,
    pub fs_avail_bytes: Option<u64>,
    pub protection: PartitionProtectionClass,
    pub encrypted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredDisk {
    pub identity: DiskIdentity,
    pub device_path: String,
    pub partitions: Vec<DiscoveredPartition>,
    pub partition_table_sha256: String,
    pub health: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledOs {
    pub id: String,
    pub display_name: String,
    pub os_family: OsFamily,
    pub evidence: String,
    pub partition_guid: Option<String>,
    pub protection: PartitionProtectionClass,
}

pub fn parse_lsblk(
    json: &str,
    backing_source: Option<&str>,
) -> Result<Vec<DiscoveredDisk>, String> {
    let value: Value = serde_json::from_str(json).map_err(|err| err.to_string())?;
    let devices = value
        .get("blockdevices")
        .and_then(Value::as_array)
        .ok_or_else(|| "lsblk json has no blockdevices array".to_string())?;
    let mut disks = Vec::new();
    for device in devices {
        if kind(device) != "disk" {
            continue;
        }
        disks.push(parse_disk(device, backing_source)?);
    }
    Ok(disks)
}

pub fn infer_installed_os(disks: &[DiscoveredDisk]) -> Vec<InstalledOs> {
    let mut found = Vec::new();
    for disk in disks {
        for part in &disk.partitions {
            if part.protection == PartitionProtectionClass::ProtectedSbmRecovery {
                continue;
            }
            if let Some(os) = infer_one(part) {
                found.push(os);
            }
        }
    }
    found
}

pub fn backing_disk<'a>(
    disks: &'a [DiscoveredDisk],
    root_source: &str,
) -> Option<&'a DiscoveredDisk> {
    let source = root_source.trim();
    if source.is_empty() {
        return None;
    }
    disks.iter().find(|disk| {
        disk.device_path == source
            || disk
                .partitions
                .iter()
                .any(|part| part.device_path == source)
    })
}

fn infer_one(part: &DiscoveredPartition) -> Option<InstalledOs> {
    let fs = part.fstype.as_deref().unwrap_or("").to_ascii_lowercase();
    let guid = part.partuuid.clone();
    let g = guid.as_deref()?;
    let id = format!("os:{g}");
    let (family, name, evidence) =
        if part.protection == PartitionProtectionClass::ProtectedAppleSystem || fs == "apfs" {
            (
                OsFamily::MacOs,
                "macOS (APFS, protected)",
                "APFS superblock or Apple GPT type",
            )
        } else if part.encrypted && (fs.contains("bitlk") || fs.contains("bitlocker")) {
            (
                OsFamily::Windows,
                "Windows (BitLocker, protected)",
                "BitLocker superblock",
            )
        } else if fs == "ntfs" {
            (OsFamily::Windows, "Windows (NTFS)", "NTFS superblock")
        } else if matches!(
            fs.as_str(),
            "ext4" | "ext3" | "ext2" | "xfs" | "btrfs" | "f2fs" | "bcachefs"
        ) {
            (OsFamily::Linux, "Linux", "Linux filesystem superblock")
        } else if part.protection == PartitionProtectionClass::ProtectedEncrypted {
            (
                OsFamily::Other,
                "Encrypted volume (protected)",
                "LUKS or BitLocker superblock",
            )
        } else {
            return None;
        };
    Some(InstalledOs {
        id,
        display_name: format!("{name} ({})", part.name),
        os_family: family,
        evidence: format!("{evidence}; {}", part.device_path),
        partition_guid: guid,
        protection: part.protection,
    })
}

fn parse_disk(device: &Value, backing_source: Option<&str>) -> Result<DiscoveredDisk, String> {
    let name = text(device, "name").unwrap_or_else(|| "disk".to_string());
    let device_path = text(device, "path").unwrap_or_else(|| format!("/dev/{name}"));
    let model = text(device, "model")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Unknown".to_string());
    let serial = text(device, "serial").filter(|value| !value.is_empty());
    let serial_hash = serial.as_deref().map(|value| sha256_hex(value.as_bytes()));
    let wwn = text(device, "wwn").filter(|value| !value.is_empty());
    let gpt = text(device, "ptuuid")
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());
    let sector = number(device, "log-sec").max(512);
    let tran = text(device, "tran");
    let identity = DiskIdentity {
        stable_id: stable_id(
            wwn.as_deref(),
            gpt.as_deref(),
            &model,
            serial_hash.as_deref(),
        ),
        gpt_disk_guid: gpt,
        wwn_or_eui64: wwn,
        model,
        serial_hash,
        size_bytes: number(device, "size"),
        logical_sector_size: u32::try_from(sector).unwrap_or(512),
        bus: bus_of(&name, tran.as_deref()),
        removable: flag(device, "rm") || flag(device, "hotplug"),
        read_only: flag(device, "ro"),
    };
    let mut partitions = Vec::new();
    if let Some(children) = device.get("children").and_then(Value::as_array) {
        for child in children {
            if kind(child) != "part" {
                continue;
            }
            partitions.push(parse_part(child, sector, backing_source));
        }
    }
    let partition_table_sha256 = table_hash(&partitions);
    Ok(DiscoveredDisk {
        identity,
        device_path,
        partitions,
        partition_table_sha256,
        health: "not queried".to_string(),
    })
}

fn parse_part(
    device: &Value,
    disk_sector: u64,
    backing_source: Option<&str>,
) -> DiscoveredPartition {
    let name = text(device, "name").unwrap_or_else(|| "part".to_string());
    let device_path = text(device, "path").unwrap_or_else(|| format!("/dev/{name}"));
    let sector = number(device, "log-sec").max(disk_sector).max(512);
    let start_sectors = number(device, "start");
    let fstype = text(device, "fstype").filter(|value| !value.is_empty());
    let parttype = text(device, "parttype")
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());
    let partlabel = text(device, "partlabel").filter(|value| !value.is_empty());
    let mountpoint = text(device, "mountpoint").filter(|value| !value.is_empty());
    let is_root = backing_source.is_some_and(|source| source == device_path);
    let (protection, encrypted) = classify(
        fstype.as_deref(),
        parttype.as_deref(),
        partlabel.as_deref(),
        is_root,
    );
    let fs_avail_bytes = if mountpoint.is_some() {
        device.get("fsavail").and_then(Value::as_u64).or_else(|| {
            device
                .get("fsavail")
                .and_then(Value::as_str)
                .and_then(|s| s.parse().ok())
        })
    } else {
        None
    };
    DiscoveredPartition {
        name,
        device_path,
        partuuid: text(device, "partuuid")
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase()),
        parttype,
        partlabel,
        fstype,
        label: text(device, "label").filter(|value| !value.is_empty()),
        mountpoint,
        size_bytes: number(device, "size"),
        start_bytes: start_sectors.saturating_mul(sector),
        fs_avail_bytes,
        protection,
        encrypted,
    }
}

fn classify(
    fstype: Option<&str>,
    parttype: Option<&str>,
    partlabel: Option<&str>,
    is_backing_root: bool,
) -> (PartitionProtectionClass, bool) {
    let fs = fstype.unwrap_or("").to_ascii_lowercase();
    let pt = parttype.unwrap_or("").to_ascii_lowercase();
    let label = partlabel.unwrap_or("").to_ascii_lowercase();
    let encrypted = fs.contains("crypto_luks") || fs.contains("bitlk") || fs.contains("bitlocker");
    if is_backing_root {
        return (PartitionProtectionClass::ProtectedSbmRecovery, encrypted);
    }
    if pt == APPLE_APFS
        || pt == APPLE_HFS
        || fs == "apfs"
        || label.contains("iscpreboot")
        || label.contains("recoveryos")
    {
        return (PartitionProtectionClass::ProtectedAppleSystem, encrypted);
    }
    if encrypted {
        return (PartitionProtectionClass::ProtectedEncrypted, true);
    }
    if pt == ESP_TYPE {
        return (PartitionProtectionClass::ProtectedEsp, false);
    }
    (PartitionProtectionClass::Normal, false)
}

fn table_hash(parts: &[DiscoveredPartition]) -> String {
    let mut rows: Vec<String> = parts
        .iter()
        .map(|part| {
            format!(
                "{}:{}:{}:{}",
                part.partuuid.as_deref().unwrap_or("-"),
                part.start_bytes,
                part.size_bytes,
                part.parttype.as_deref().unwrap_or("-")
            )
        })
        .collect();
    rows.sort();
    sha256_hex(rows.join("\n").as_bytes())
}

fn stable_id(
    wwn: Option<&str>,
    gpt: Option<&str>,
    model: &str,
    serial_hash: Option<&str>,
) -> String {
    if let Some(wwn) = wwn {
        return format!("wwn:{wwn}");
    }
    if let Some(gpt) = gpt {
        return format!("gpt:{gpt}");
    }
    let model = model.split_whitespace().collect::<Vec<_>>().join("_");
    format!("model-serial:{model}:{}", serial_hash.unwrap_or("none"))
}

fn bus_of(name: &str, tran: Option<&str>) -> DiskBus {
    match tran.unwrap_or("").to_ascii_lowercase().as_str() {
        "nvme" => DiskBus::Nvme,
        "sata" | "ata" => DiskBus::Sata,
        "usb" => DiskBus::Usb,
        "virtio" => DiskBus::Virtio,
        "scsi" | "sas" => DiskBus::Scsi,
        "mmc" => DiskBus::Mmc,
        _ if name.starts_with("nvme") => DiskBus::Nvme,
        _ if name.starts_with("vd") => DiskBus::Virtio,
        _ if name.starts_with("mmc") => DiskBus::Mmc,
        _ if name.starts_with("sd") => DiskBus::Scsi,
        _ => DiskBus::Unknown,
    }
}

fn kind(device: &Value) -> &str {
    device.get("type").and_then(Value::as_str).unwrap_or("")
}

fn text(device: &Value, key: &str) -> Option<String> {
    match device.get(key) {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Number(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn number(device: &Value, key: &str) -> u64 {
    match device.get(key) {
        Some(Value::Number(value)) => value.as_u64().unwrap_or(0),
        Some(Value::String(value)) => value.parse().unwrap_or(0),
        _ => 0,
    }
}

fn flag(device: &Value, key: &str) -> bool {
    match device.get(key) {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_u64().unwrap_or(0) != 0,
        Some(Value::String(value)) => value == "1" || value.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> &'static str {
        r#"{
          "blockdevices": [
            {
              "name": "vda",
              "path": "/dev/vda",
              "type": "disk",
              "size": 2147483648,
              "model": "QEMU HARDDISK",
              "serial": "QM00001",
              "wwn": null,
              "ptuuid": "11111111-2222-3333-4444-555555555555",
              "log-sec": 512,
              "ro": false,
              "rm": false,
              "tran": "virtio",
              "children": [
                {
                  "name": "vda1",
                  "path": "/dev/vda1",
                  "type": "part",
                  "size": 536870912,
                  "start": 2048,
                  "log-sec": 512,
                  "fstype": "vfat",
                  "parttype": "c12a7328-f81f-11d2-ba4b-00a0c93ec93b",
                  "partuuid": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                  "partlabel": "ESP",
                  "mountpoint": "/efi",
                  "fsavail": 1000000
                },
                {
                  "name": "vda2",
                  "path": "/dev/vda2",
                  "type": "part",
                  "size": 1073741824,
                  "start": 1050624,
                  "fstype": "erofs",
                  "parttype": "4f68bce3-e8cd-4db1-96e7-fbcaf984b709",
                  "partuuid": "bbbbbbbb-cccc-dddd-eeee-ffffffffffff",
                  "mountpoint": "/"
                }
              ]
            },
            {
              "name": "vdb",
              "path": "/dev/vdb",
              "type": "disk",
              "size": 1073741824,
              "model": "QEMU HARDDISK",
              "ptuuid": "99999999-8888-7777-6666-555555555555",
              "tran": "virtio",
              "children": [
                {
                  "name": "vdb1",
                  "path": "/dev/vdb1",
                  "type": "part",
                  "size": 1073741824,
                  "fstype": "crypto_LUKS",
                  "partuuid": "cccccccc-dddd-eeee-ffff-000000000001",
                  "parttype": "0fc63daf-8483-4772-8e79-3d69d8477de4"
                },
                {
                  "name": "vdb2",
                  "path": "/dev/vdb2",
                  "type": "part",
                  "size": 4096,
                  "fstype": "apfs",
                  "partuuid": "dddddddd-eeee-ffff-0000-111111111111",
                  "parttype": "7c3457ef-0000-11aa-aa11-00306543ecac"
                },
                {
                  "name": "vdb3",
                  "path": "/dev/vdb3",
                  "type": "part",
                  "size": 4096,
                  "fstype": "ntfs",
                  "partuuid": "eeeeeeee-ffff-0000-1111-222222222222",
                  "parttype": "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7"
                }
              ]
            }
          ]
        }"#
    }

    #[test]
    fn parses_virtio_disks_and_classifies_partitions() {
        let disks = parse_lsblk(sample(), Some("/dev/vda2")).unwrap();
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].identity.bus, DiskBus::Virtio);
        assert_eq!(
            disks[0].identity.stable_id,
            "gpt:11111111-2222-3333-4444-555555555555"
        );
        assert!(disks[0].identity.serial_hash.as_ref().unwrap().len() == 64);
        assert_eq!(
            disks[0].partitions[0].protection,
            PartitionProtectionClass::ProtectedEsp
        );
        assert_eq!(disks[0].partitions[0].fs_avail_bytes, Some(1_000_000));
        assert_eq!(disks[0].partitions[0].start_bytes, 2048 * 512);
        assert_eq!(
            disks[0].partitions[1].protection,
            PartitionProtectionClass::ProtectedSbmRecovery
        );
        assert!(disks[1].partitions[0].encrypted);
        assert_eq!(
            disks[1].partitions[0].protection,
            PartitionProtectionClass::ProtectedEncrypted
        );
        assert_eq!(
            disks[1].partitions[1].protection,
            PartitionProtectionClass::ProtectedAppleSystem
        );
        let os = infer_installed_os(&disks);
        assert!(os.iter().any(|item| item.os_family == OsFamily::MacOs));
        assert!(os.iter().any(|item| item.os_family == OsFamily::Windows));
        assert!(!os.iter().any(|item| item.evidence.contains("erofs")));
        assert!(backing_disk(&disks, "/dev/vda2").unwrap().device_path == "/dev/vda");
    }

    #[test]
    fn wwn_outranks_gpt_in_the_stable_id() {
        let json = r#"{"blockdevices":[{"name":"nvme0n1","path":"/dev/nvme0n1","type":"disk","size":1,"model":"NVMe","wwn":"eui.ABCDEF","ptuuid":"abc","tran":"nvme","log-sec":4096}]}"#;
        let disks = parse_lsblk(json, None).unwrap();
        assert_eq!(disks[0].identity.stable_id, "wwn:eui.ABCDEF");
        assert_eq!(disks[0].identity.bus, DiskBus::Nvme);
        assert_eq!(disks[0].identity.logical_sector_size, 4096);
    }
}
