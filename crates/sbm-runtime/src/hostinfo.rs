//! Parsers for `/proc`, `ip -j`, and DRM driver names.
//!
//! The daemon reads the live files. These functions only interpret text so the
//! tests do not need Linux.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total_kib: u64,
    pub available_kib: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NicStatus {
    pub name: String,
    pub mac: Option<String>,
    pub state: String,
    pub addresses: Vec<String>,
    pub wireless: bool,
}

pub fn parse_meminfo(text: &str) -> MemoryInfo {
    let mut total = 0;
    let mut available = 0;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("0").parse().unwrap_or(0);
        match key {
            "MemTotal:" => total = value,
            "MemAvailable:" => available = value,
            _ => {}
        }
    }
    MemoryInfo {
        total_kib: total,
        available_kib: available,
    }
}

pub fn parse_uptime(text: &str) -> u64 {
    text.split_whitespace()
        .next()
        .and_then(|value| value.parse::<f64>().ok())
        .map(|value| value as u64)
        .unwrap_or(0)
}

pub fn cpu_model(cpuinfo: &str) -> String {
    for line in cpuinfo.lines() {
        if let Some(value) = line.strip_prefix("model name") {
            let value = value.trim().trim_start_matches(':').trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    let mut implementer = None;
    let mut part = None;
    for line in cpuinfo.lines() {
        if let Some(value) = line.split_once(':') {
            let key = value.0.trim();
            let data = value.1.trim();
            if key == "CPU implementer" {
                implementer = Some(data.to_string());
            } else if key == "CPU part" {
                part = Some(data.to_string());
            }
        }
    }
    match (implementer, part) {
        (Some(implementer), Some(part)) => format!("ARM implementer {implementer} part {part}"),
        _ => "Unknown CPU".to_string(),
    }
}

pub fn cpu_count(cpuinfo: &str) -> usize {
    let count = cpuinfo
        .lines()
        .filter(|line| line.starts_with("processor"))
        .count();
    count.max(1)
}

pub fn parse_ip_addr(json: &str) -> Result<Vec<NicStatus>, String> {
    let value: Value = serde_json::from_str(json).map_err(|err| err.to_string())?;
    let devices = value
        .as_array()
        .ok_or_else(|| "ip json is not an array".to_string())?;
    let mut nics = Vec::new();
    for device in devices {
        let name = device
            .get("ifname")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if name.is_empty() || name == "lo" {
            continue;
        }
        let mut addresses = Vec::new();
        if let Some(info) = device.get("addr_info").and_then(Value::as_array) {
            for addr in info {
                if let Some(local) = addr.get("local").and_then(Value::as_str) {
                    addresses.push(local.to_string());
                }
            }
        }
        nics.push(NicStatus {
            name,
            mac: device
                .get("address")
                .and_then(Value::as_str)
                .map(str::to_string),
            state: device
                .get("operstate")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            addresses,
            wireless: false,
        });
    }
    Ok(nics)
}

pub fn gpu_label(driver: &str, vendor_id: Option<u16>) -> String {
    let vendor = match vendor_id {
        Some(0x8086) => "Intel",
        Some(0x1002) | Some(0x1022) => "AMD",
        Some(0x10de) => "NVIDIA",
        Some(0x1af4) => "virtio",
        Some(0x1234) | Some(0x1b36) => "QEMU",
        Some(other) => return format!("{driver} (vendor {other:04x})"),
        None => return driver.to_string(),
    };
    format!("{vendor} ({driver})")
}

pub fn renderer_hint(forced: Option<&str>, render_node: bool) -> String {
    match forced.map(str::trim) {
        Some("software") | Some("tiny-skia") => "software (tiny-skia, pixman/llvmpipe)".to_string(),
        Some("gpu") | Some("wgpu") => "GPU requested (wgpu)".to_string(),
        _ if render_node => {
            "auto: wgpu when the DRM node initializes, tiny-skia if it does not".to_string()
        }
        _ => "software: no DRM render node, tiny-skia".to_string(),
    }
}

pub fn read_os_release_value(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    text.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix(&prefix)
            .map(|value| value.trim_matches('"').trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_memory_uptime_and_cpu() {
        let mem = parse_meminfo("MemTotal: 2048000 kB\nMemAvailable: 1500000 kB\n");
        assert_eq!(mem.total_kib, 2_048_000);
        assert_eq!(mem.available_kib, 1_500_000);
        assert_eq!(parse_uptime("12.9 40.0\n"), 12);
        assert_eq!(
            cpu_model("processor : 0\nmodel name : QEMU Virtual\n"),
            "QEMU Virtual"
        );
        assert_eq!(
            cpu_model("processor : 0\nCPU implementer : 0x61\nCPU part : 0xd0c\n"),
            "ARM implementer 0x61 part 0xd0c"
        );
        assert_eq!(cpu_count("processor : 0\nprocessor : 1\n"), 2);
    }

    #[test]
    fn parses_ip_and_gpu_labels() {
        let json = r#"[{"ifname":"lo","operstate":"UNKNOWN"},{"ifname":"enp0s1","operstate":"UP","address":"52:54:00:12:34:56","addr_info":[{"family":"inet","local":"10.0.2.15"}]}]"#;
        let nics = parse_ip_addr(json).unwrap();
        assert_eq!(nics.len(), 1);
        assert_eq!(nics[0].addresses, vec!["10.0.2.15".to_string()]);
        assert_eq!(gpu_label("virtio_gpu", Some(0x1af4)), "virtio (virtio_gpu)");
        assert_eq!(gpu_label("i915", Some(0x8086)), "Intel (i915)");
        assert_eq!(gpu_label("amdgpu", Some(0x1002)), "AMD (amdgpu)");
        assert_eq!(gpu_label("nvidia", Some(0x10de)), "NVIDIA (nvidia)");
        assert!(renderer_hint(Some("software"), true).contains("tiny-skia"));
        assert!(renderer_hint(None, false).contains("no DRM"));
    }
}
