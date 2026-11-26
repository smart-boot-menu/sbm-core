//! Privileged operations for SBM Core.
//!
//! `sbm-ui` sends a fixed request enum. This process is the only one that
//! execs recovery tools, and it never invokes a shell.

#![forbid(unsafe_code)]

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::{PermissionsExt, chown};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sbm_protocol::{BootCommand, BootSuccessReport};
use sbm_runtime::discover::{backing_disk, infer_installed_os, parse_lsblk};
use sbm_runtime::handoff::load_handoff;
use sbm_runtime::hostinfo::{
    cpu_count, cpu_model, gpu_label, parse_ip_addr, parse_meminfo, parse_uptime,
    read_os_release_value, renderer_hint,
};
use sbm_runtime::ipc::{
    ClientRequest, ClientResponse, HardwareInfo, LaunchTool, PowerAction, SOCKET_PATH,
    SystemSnapshot, ToolReport, read_message, tool_catalog, write_message,
};
use sbm_runtime::plan::validate_plan;
use sbm_runtime::{HandoffLoad, SBM_VERSION, SLOT_A_TARGET_ID};
use sbm_types::{BootTargetId, ComponentKind, SCHEMA_VERSION_V1, SbmErrorCode};

const ESP_TYPE: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";

enum AfterAction {
    None,
    Reboot,
    PowerOff,
}

struct Handled {
    response: ClientResponse,
    after: AfterAction,
}

pub fn serve() -> Result<(), String> {
    let _ = protect_backing_device();
    kmsg(&format!("SBM_TORAM_MODE={}", toram_mode()));
    let dir = Path::new("/run/sbm");
    let _ = fs::create_dir_all(dir);
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o750));
    let socket_path = Path::new(SOCKET_PATH);
    let _ = fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path).map_err(|err| err.to_string())?;
    let _ = fs::set_permissions(socket_path, fs::Permissions::from_mode(0o660));
    if let Some(gid) = group_gid("sbm") {
        let _ = chown(socket_path, Some(0), Some(gid));
    }
    let boot = snapshot();
    kmsg(&format!(
        "SBM_BOOT_METRICS mode={} mem_total_kib={} mem_available_kib={} kernel={}",
        boot.toram_mode, boot.memory.total_kib, boot.memory.available_kib, boot.kernel
    ));
    eprintln!("sbm-cored listening on {SOCKET_PATH}");
    for connection in listener.incoming() {
        let mut stream = match connection {
            Ok(stream) => stream,
            Err(err) => {
                eprintln!("accept: {err}");
                continue;
            }
        };
        if !peer_allowed(&stream) {
            let _ = write_message(
                &mut stream,
                &ClientResponse::Error {
                    message: "local socket peer is not root or the sbm user".to_string(),
                    code: Some(SbmErrorCode::UnauthorizedScope),
                },
            );
            continue;
        }
        let request = match read_message::<ClientRequest>(&mut stream) {
            Ok(request) => request,
            Err(err) => {
                let _ = write_message(
                    &mut stream,
                    &ClientResponse::Error {
                        message: err,
                        code: Some(SbmErrorCode::InvalidSchema),
                    },
                );
                continue;
            }
        };
        let handled = handle(request);
        let _ = write_message(&mut stream, &handled.response);
        let _ = stream.flush();
        drop(stream);
        match handled.after {
            AfterAction::None => {}
            AfterAction::Reboot => delayed_systemctl(&["reboot"]),
            AfterAction::PowerOff => delayed_systemctl(&["poweroff"]),
        }
    }
    Ok(())
}

pub fn protect_backing_device() -> bool {
    if toram_mode() != "backing-protected" {
        return false;
    }
    let Some(source) = read_line("/run/sbm/root-source") else {
        return false;
    };
    let Some(disk) = resolve_disk(&source) else {
        return false;
    };
    let _ = fs::write("/run/sbm/backing-disk", format!("{disk}\n"));
    let mounts = mount_table();
    let name = disk.trim_start_matches("/dev/");
    let sys = PathBuf::from("/sys/class/block").join(name);
    let Ok(entries) = fs::read_dir(&sys) else {
        return false;
    };
    for entry in entries.flatten() {
        let part_name = entry.file_name().to_string_lossy().to_string();
        if !part_name.starts_with(name) || !entry.path().join("partition").exists() {
            continue;
        }
        let dev = format!("/dev/{part_name}");
        if mounts
            .iter()
            .any(|(device, point)| device == &dev && point == "/efi")
        {
            continue;
        }
        set_ro(&dev, true);
    }
    true
}

fn handle(request: ClientRequest) -> Handled {
    match request {
        ClientRequest::Snapshot => Handled {
            response: ClientResponse::Snapshot {
                snapshot: Box::new(snapshot()),
            },
            after: AfterAction::None,
        },
        ClientRequest::UiReady => {
            kmsg("SBM_UI_READY");
            let _ = write_boot_success();
            ack("desktop reported ready")
        }
        ClientRequest::Launch { tool } => match tool {
            LaunchTool::Gparted => match launch_gparted() {
                Ok(message) => ack(message),
                Err(message) => error(message, None),
            },
            LaunchTool::Terminal => match launch_terminal() {
                Ok(message) => ack(message),
                Err(message) => error(message, None),
            },
        },
        ClientRequest::ToolProbe => Handled {
            response: ClientResponse::Tools {
                tools: probe_tools(),
            },
            after: AfterAction::None,
        },
        ClientRequest::RemoteStatus => remote_response(None),
        ClientRequest::RemoteEnable => match enable_remote() {
            Ok(password) => remote_response(Some(password)),
            Err(message) => error(message, None),
        },
        ClientRequest::RemoteDisable => {
            let _ = Command::new(systemctl_bin())
                .args(["stop", "sbm-wayvnc.service"])
                .status();
            let _ = fs::remove_file("/var/lib/sbm/remote.authorized");
            remote_response(None)
        }
        ClientRequest::ValidatePlan { plan } => {
            let snap = snapshot();
            let backing = snap.backing_disk.as_deref().and_then(|path| {
                backing_disk(&snap.disks, path).map(|disk| disk.identity.stable_id.clone())
            });
            let decision = validate_plan(&plan, snap.backing_protected, backing.as_deref());
            Handled {
                response: ClientResponse::Plan { decision },
                after: AfterAction::None,
            }
        }
        ClientRequest::Power { action } => power(action),
    }
}

fn snapshot() -> SystemSnapshot {
    let mut snap = sbm_runtime::ipc::empty_snapshot();
    snap.hostname = read_trimmed("/proc/sys/kernel/hostname").unwrap_or_else(|| "sbm-core".into());
    snap.kernel = read_trimmed("/proc/sys/kernel/osrelease").unwrap_or_else(|| "unknown".into());
    snap.arch = std::env::consts::ARCH.to_string();
    let os_release = fs::read_to_string("/etc/os-release").unwrap_or_default();
    snap.os_pretty =
        read_os_release_value(&os_release, "PRETTY_NAME").unwrap_or_else(|| "SBM Core".to_string());
    let sbm_release = fs::read_to_string("/usr/lib/sbm/release").unwrap_or_default();
    snap.sbm_version = read_os_release_value(&sbm_release, "SBM_VERSION")
        .unwrap_or_else(|| SBM_VERSION.to_string());
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    snap.memory = parse_meminfo(&meminfo);
    snap.uptime_secs = parse_uptime(&fs::read_to_string("/proc/uptime").unwrap_or_default());
    snap.hardware = HardwareInfo {
        vendor: read_trimmed("/sys/class/dmi/id/sys_vendor").unwrap_or_else(|| "Unknown".into()),
        product: read_trimmed("/sys/class/dmi/id/product_name").unwrap_or_else(|| "Unknown".into()),
        cpu_model: cpu_model(&cpuinfo),
        arch: snap.arch.clone(),
        cores: cpu_count(&cpuinfo),
        gpus: gpu_names(),
    };
    snap.toram_mode = toram_mode();
    snap.backing_protected = snap.toram_mode == "backing-protected";
    snap.backing_disk =
        read_line("/run/sbm/backing-disk").or_else(|| read_line("/run/sbm/root-source"));
    if let Some(json) = command_stdout(
        "/usr/bin/lsblk",
        &[
            "--json",
            "-b",
            "-o",
            "NAME,PATH,TYPE,SIZE,MODEL,SERIAL,WWN,PTUUID,FSTYPE,LABEL,PARTLABEL,PARTTYPE,PARTUUID,MOUNTPOINT,RO,RM,TRAN,START,LOG-SEC,FSAVAIL,HOTPLUG",
        ],
    ) {
        match parse_lsblk(&json, read_line("/run/sbm/root-source").as_deref()) {
            Ok(mut disks) => {
                annotate_health(&mut disks);
                snap.installed_os = infer_installed_os(&disks);
                snap.disks = disks;
            }
            Err(err) => eprintln!("lsblk parse: {err}"),
        }
    }
    if let Some(json) = command_stdout("/usr/sbin/ip", &["-j", "addr"])
        .or_else(|| command_stdout("/usr/bin/ip", &["-j", "addr"]))
        && let Ok(mut nics) = parse_ip_addr(&json)
    {
        for nic in &mut nics {
            nic.wireless = Path::new(&format!("/sys/class/net/{}/phy80211", nic.name)).exists();
        }
        snap.nics = nics;
    }
    snap.tools = tool_presence();
    let handoff = read_handoff();
    snap.handoff = handoff.detail;
    snap.handoff_verified = handoff.verified;
    let forced = fs::read_to_string("/efi/EFI/sbm/state/renderer")
        .ok()
        .map(|value| value.trim().to_string());
    snap.renderer = renderer_hint(forced.as_deref(), Path::new("/dev/dri/renderD128").exists());
    snap.remote_enabled = remote_enabled();
    snap.remote_detail = if snap.remote_enabled {
        "wayvnc is enabled on 127.0.0.1:5900 with authentication".to_string()
    } else {
        "remote access is disabled until an operator enables it".to_string()
    };
    snap
}

fn power(action: PowerAction) -> Handled {
    match action {
        PowerAction::Reboot => Handled {
            response: ClientResponse::Ack {
                message: "rebooting".to_string(),
            },
            after: AfterAction::Reboot,
        },
        PowerAction::Shutdown => Handled {
            response: ClientResponse::Ack {
                message: "shutting down".to_string(),
            },
            after: AfterAction::PowerOff,
        },
        PowerAction::BootManager => {
            if let Err(err) = write_boot_command(&BootCommand::Reboot) {
                return error(err, Some(SbmErrorCode::StorageIoError));
            }
            Handled {
                response: ClientResponse::Ack {
                    message: "requested return to the boot manager".to_string(),
                },
                after: AfterAction::Reboot,
            }
        }
        PowerAction::BootTarget { target_id } => {
            let id = BootTargetId(target_id);
            if !id.is_valid() {
                return error(
                    "target id is not a stable SBM identifier".to_string(),
                    Some(SbmErrorCode::InvalidSchema),
                );
            }
            if !known_target(id.as_str()) {
                return error(
                    "that boot target was not discovered".to_string(),
                    Some(SbmErrorCode::TargetNotFound),
                );
            }
            let command = BootCommand::BootTarget {
                target_id: id,
                one_shot: true,
                extra_cmdline_preset: None,
            };
            if let Err(err) = write_boot_command(&command) {
                return error(err, Some(SbmErrorCode::StorageIoError));
            }
            Handled {
                response: ClientResponse::Ack {
                    message: "requested the next boot target".to_string(),
                },
                after: AfterAction::Reboot,
            }
        }
    }
}

fn known_target(id: &str) -> bool {
    if id == SLOT_A_TARGET_ID {
        return true;
    }
    let snap = snapshot();
    if snap.installed_os.iter().any(|os| os.id == id) {
        return true;
    }
    read_handoff()
        .payload
        .into_iter()
        .flat_map(|payload| payload.discovered_targets)
        .any(|target| target.id.as_str() == id)
}

fn launch_gparted() -> Result<String, String> {
    let path = first_existing(&[
        "/usr/libexec/gpartedbin",
        "/usr/sbin/gpartedbin",
        "/usr/bin/gpartedbin",
    ])
    .ok_or_else(|| "gpartedbin is not installed".to_string())?;
    let _ = fs::create_dir_all("/run/user/0");
    let _ = fs::set_permissions("/run/user/0", fs::Permissions::from_mode(0o700));
    let mut command = if let Some(dbus) = first_existing(&["/usr/bin/dbus-run-session"]) {
        let mut wrapped = Command::new(dbus);
        wrapped.arg("--").arg(&path);
        wrapped
    } else {
        Command::new(&path)
    };
    let log = File::create("/run/sbm/gparted.log").or_else(|_| File::open("/dev/null"));
    command
        .env("GDK_BACKEND", "x11")
        .env("DISPLAY", ":0")
        .env("XDG_RUNTIME_DIR", "/run/user/0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            log.unwrap_or_else(|_| File::open("/dev/null").expect("devnull")),
        ));
    if let Some(env) = labwc_environ() {
        for key in ["DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY"] {
            if let Some(value) = env.get(key) {
                command.env(key, value);
            }
        }
    }
    let protected = toram_mode() == "backing-protected";
    let disk = read_line("/run/sbm/backing-disk")
        .or_else(|| read_line("/run/sbm/root-source").and_then(|source| resolve_disk(&source)));
    if protected && let Some(disk) = disk.clone() {
        set_ro(&disk, true);
    }
    let mut child = command.spawn().map_err(|err| err.to_string())?;
    thread::spawn(move || {
        let _ = child.wait();
        if protected && let Some(disk) = disk {
            set_ro(&disk, false);
        }
    });
    Ok(if protected {
        "GParted started through XWayland. The boot disk is held read-only because this system is not RAM-resident.".to_string()
    } else {
        "GParted started through XWayland. The root image is RAM-resident, so the previous boot disk is not the running root.".to_string()
    })
}

fn launch_terminal() -> Result<String, String> {
    let path =
        first_existing(&["/usr/bin/foot"]).ok_or_else(|| "foot is not installed".to_string())?;
    let wayland = wayland_display();
    let mut command = Command::new(path);
    command
        .uid(1000)
        .gid(1000)
        .env("HOME", "/var/lib/sbm")
        .env("USER", "sbm")
        .env("LOGNAME", "sbm")
        .env("XDG_RUNTIME_DIR", "/run/user/1000")
        .env("WAYLAND_DISPLAY", wayland)
        .env("XDG_SESSION_TYPE", "wayland")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = command.spawn().map_err(|err| err.to_string())?;
    Ok("terminal started".to_string())
}

fn probe_tools() -> Vec<ToolReport> {
    tool_catalog()
        .into_iter()
        .map(|(name, paths)| {
            let found = first_existing(paths);
            let detail = match (found.as_deref(), probe_args(name)) {
                (Some(path), Some(args)) => run_timed(path, args, 3),
                (Some(_), None) => "present (not executed; interactive tool)".to_string(),
                (None, _) => "missing".to_string(),
            };
            ToolReport {
                name: name.to_string(),
                path: found.clone().unwrap_or_else(|| paths[0].to_string()),
                present: found.is_some(),
                detail,
            }
        })
        .collect()
}

fn tool_presence() -> Vec<ToolReport> {
    tool_catalog()
        .into_iter()
        .map(|(name, paths)| {
            let found = first_existing(paths);
            ToolReport {
                name: name.to_string(),
                path: found.clone().unwrap_or_else(|| paths[0].to_string()),
                present: found.is_some(),
                detail: if found.is_some() {
                    "installed".to_string()
                } else {
                    "missing".to_string()
                },
            }
        })
        .collect()
}

fn probe_args(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "parted" | "cryptsetup" | "btrfs-progs" | "ddrescue" | "smartctl" | "efibootmgr"
        | "rsync" | "wimlib-imagex" | "foot" | "labwc" | "wayvnc" => Some(&["--version"]),
        "e2fsprogs" => Some(&["-V"]),
        "ssh" => Some(&["-V"]),
        "nvme" => Some(&["version"]),
        _ => None,
    }
}

fn enable_remote() -> Result<String, String> {
    let _ = fs::create_dir_all("/var/lib/sbm");
    let password = random_password();
    let config = format!(
        "address=127.0.0.1\nport=5900\nenable_auth=true\nusername=sbm\npassword={password}\n"
    );
    let path = Path::new("/var/lib/sbm/wayvnc.cfg");
    fs::write(path, &config).map_err(|err| err.to_string())?;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o640));
    if let Some(gid) = group_gid("sbm") {
        let _ = chown(path, Some(0), Some(gid));
    }
    fs::write("/var/lib/sbm/remote.authorized", "localhost\n").map_err(|err| err.to_string())?;
    let status = Command::new(systemctl_bin())
        .args(["restart", "sbm-wayvnc.service"])
        .status()
        .map_err(|err| err.to_string())?;
    if !status.success() {
        let _ = fs::remove_file("/var/lib/sbm/remote.authorized");
        return Err("wayvnc did not start; remote access remains disabled".to_string());
    }
    Ok(password)
}

fn remote_response(password: Option<String>) -> Handled {
    let enabled = remote_enabled();
    Handled {
        response: ClientResponse::Remote {
            enabled,
            detail: if enabled {
                "authenticated wayvnc on 127.0.0.1:5900".to_string()
            } else {
                "disabled".to_string()
            },
            password,
        },
        after: AfterAction::None,
    }
}

fn remote_enabled() -> bool {
    Path::new("/var/lib/sbm/remote.authorized").exists()
}

fn read_handoff() -> HandoffLoad {
    let cmdline = fs::read_to_string("/proc/cmdline").unwrap_or_default();
    for path in [
        "/efi/EFI/sbm/handoff/current.json",
        "/boot/efi/EFI/sbm/handoff/current.json",
        "/boot/EFI/sbm/handoff/current.json",
    ] {
        if let Ok(bytes) = fs::read(path) {
            return load_handoff(Some(&bytes), &cmdline);
        }
    }
    load_handoff(None, &cmdline)
}

fn write_boot_success() -> Result<(), String> {
    let handoff = read_handoff();
    let attempt_id = handoff
        .payload
        .as_ref()
        .map(|payload| payload.attempt_id.clone())
        .unwrap_or_else(|| "local".to_string());
    let report = BootSuccessReport {
        schema_version: SCHEMA_VERSION_V1.to_string(),
        attempt_id,
        target_id: BootTargetId(SLOT_A_TARGET_ID.to_string()),
        confirmed_by: ComponentKind::SbmCoreUki,
        confirmed_at_unix: unix_now(),
    };
    let bytes = serde_json::to_vec_pretty(&report).map_err(|err| err.to_string())?;
    write_esp("EFI/sbm/state/boot-success.json", &bytes)
}

fn write_boot_command(command: &BootCommand) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(command).map_err(|err| err.to_string())?;
    write_esp("EFI/sbm/handoff/requested-command.json", &bytes)
}

fn write_esp(relative: &str, bytes: &[u8]) -> Result<(), String> {
    ensure_efi_mounted();
    let path = Path::new("/efi").join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|err| err.to_string())?;
    fs::rename(&tmp, &path).map_err(|err| err.to_string())?;
    Ok(())
}

fn ensure_efi_mounted() {
    if mount_table().iter().any(|(_, point)| point == "/efi") {
        return;
    }
    let Some(json) = command_stdout(
        "/usr/bin/lsblk",
        &["--json", "-b", "-o", "NAME,PATH,TYPE,PARTTYPE,FSTYPE"],
    ) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
        return;
    };
    let Some(devices) = value.get("blockdevices").and_then(|item| item.as_array()) else {
        return;
    };
    for disk in devices {
        let Some(children) = disk.get("children").and_then(|item| item.as_array()) else {
            continue;
        };
        for part in children {
            let parttype = part
                .get("parttype")
                .and_then(|item| item.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if parttype != ESP_TYPE {
                continue;
            }
            let Some(path) = part.get("path").and_then(|item| item.as_str()) else {
                continue;
            };
            let _ = fs::create_dir_all("/efi");
            let _ = Command::new("mount")
                .args(["-o", "rw", path, "/efi"])
                .status();
            return;
        }
    }
}

fn annotate_health(disks: &mut [sbm_runtime::DiscoveredDisk]) {
    let Some(smartctl) = first_existing(&["/usr/sbin/smartctl", "/usr/bin/smartctl"]) else {
        return;
    };
    for disk in disks {
        let output = run_timed(&smartctl, &["-H", "-n", "standby", &disk.device_path], 3);
        disk.health = output
            .lines()
            .next()
            .unwrap_or("unsupported")
            .trim()
            .to_string();
        if disk.health.is_empty() {
            disk.health = "unsupported".to_string();
        }
    }
}

fn gpu_names() -> Vec<String> {
    let root = Path::new("/sys/class/drm");
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let uevent = fs::read_to_string(entry.path().join("device/uevent")).unwrap_or_default();
        let driver = uevent.lines().find_map(|line| line.strip_prefix("DRIVER="));
        let vendor = fs::read_to_string(entry.path().join("device/vendor"))
            .ok()
            .and_then(|text| u16::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok());
        names.push(gpu_label(driver.unwrap_or("drm"), vendor));
    }
    names
}

fn resolve_disk(source: &str) -> Option<String> {
    let mut current = source.trim().to_string();
    if current.is_empty() || current == "missing" {
        return None;
    }
    for _ in 0..8 {
        let name = current.trim_start_matches("/dev/").to_string();
        let sys = PathBuf::from("/sys/class/block").join(&name);
        if !sys.exists() {
            return None;
        }
        let slaves = sys.join("slaves");
        if let Ok(mut entries) = fs::read_dir(&slaves)
            && let Some(Ok(entry)) = entries.next()
        {
            current = format!("/dev/{}", entry.file_name().to_string_lossy());
            continue;
        }
        if sys.join("partition").exists() {
            let real = fs::canonicalize(&sys).ok()?;
            let parent = real.parent()?.file_name()?.to_string_lossy().to_string();
            return Some(format!("/dev/{parent}"));
        }
        return Some(if current.starts_with("/dev/") {
            current
        } else {
            format!("/dev/{current}")
        });
    }
    None
}

fn set_ro(dev: &str, ro: bool) {
    if !dev.starts_with("/dev/") {
        return;
    }
    let flag = if ro { "--setro" } else { "--setrw" };
    let bin = first_existing(&["/usr/sbin/blockdev", "/usr/bin/blockdev"])
        .unwrap_or_else(|| "blockdev".into());
    let _ = Command::new(bin).args([flag, dev]).status();
}

fn mount_table() -> Vec<(String, String)> {
    let text = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let mut rows = Vec::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let left_fields: Vec<&str> = left.split_whitespace().collect();
        let right_fields: Vec<&str> = right.split_whitespace().collect();
        if left_fields.len() >= 5 && right_fields.len() >= 2 {
            rows.push((right_fields[1].to_string(), left_fields[4].to_string()));
        }
    }
    rows
}

fn labwc_environ() -> Option<std::collections::HashMap<String, String>> {
    let pid = find_comm("labwc")?;
    let bytes = fs::read(format!("/proc/{pid}/environ")).ok()?;
    let mut map = std::collections::HashMap::new();
    for item in bytes.split(|byte| *byte == 0) {
        let Ok(text) = std::str::from_utf8(item) else {
            continue;
        };
        if let Some((key, value)) = text.split_once('=') {
            map.insert(key.to_string(), value.to_string());
        }
    }
    Some(map)
}

fn find_comm(name: &str) -> Option<i32> {
    for entry in fs::read_dir("/proc").ok()?.flatten() {
        let pid: i32 = entry.file_name().to_str()?.parse().ok()?;
        let comm = fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        if comm.trim() == name {
            return Some(pid);
        }
    }
    None
}

fn wayland_display() -> String {
    let dir = Path::new("/run/user/1000");
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("wayland-") && !name.contains('.') {
                return name;
            }
        }
    }
    "wayland-0".to_string()
}

fn run_timed(path: &str, args: &[&str], seconds: u64) -> String {
    let mut child = match Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => return format!("spawn error: {err}"),
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut output = String::new();
                if let Some(mut stdout) = child.stdout.take() {
                    let _ = stdout.read_to_string(&mut output);
                }
                if let Some(mut stderr) = child.stderr.take() {
                    let _ = stderr.read_to_string(&mut output);
                }
                let output = output.split_whitespace().collect::<Vec<_>>().join(" ");
                if output.is_empty() {
                    return format!("exit {status}");
                }
                return trim_probe(&output);
            }
            Ok(None) if started.elapsed() > Duration::from_secs(seconds) => {
                let _ = child.kill();
                let _ = child.wait();
                return "timed out".to_string();
            }
            Ok(None) => thread::sleep(Duration::from_millis(30)),
            Err(err) => return format!("wait error: {err}"),
        }
    }
}

fn trim_probe(text: &str) -> String {
    let mut trimmed = text.trim().to_string();
    if trimmed.len() > 240 {
        trimmed.truncate(240);
    }
    trimmed
}

fn command_stdout(path: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(path).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn first_existing(paths: &[&str]) -> Option<String> {
    paths
        .iter()
        .map(|path| (*path).to_string())
        .find(|path| Path::new(path).exists())
}

fn random_password() -> String {
    let mut bytes = [0_u8; 16];
    if let Ok(mut file) = File::open("/dev/urandom") {
        let _ = file.read_exact(&mut bytes);
    } else {
        bytes = [
            0x53, 0x42, 0x4d, 0x2d, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa,
            0xbb, 0xcc,
        ];
    }
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    bytes
        .iter()
        .map(|byte| ALPHABET[(*byte as usize) % ALPHABET.len()] as char)
        .collect()
}

fn delayed_systemctl(args: &[&str]) {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(400));
        let _ = Command::new(systemctl_bin()).args(&args).status();
    });
}

fn systemctl_bin() -> String {
    first_existing(&["/usr/bin/systemctl", "/usr/sbin/systemctl"])
        .unwrap_or_else(|| "systemctl".into())
}

fn toram_mode() -> String {
    read_line("/run/sbm/mode").unwrap_or_else(|| "backing-protected".to_string())
}

fn read_line(path: &str) -> Option<String> {
    read_trimmed(path).filter(|value| !value.is_empty())
}

fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
}

fn group_gid(name: &str) -> Option<u32> {
    let text = fs::read_to_string("/etc/group").ok()?;
    for line in text.lines() {
        let mut parts = line.split(':');
        if parts.next() == Some(name) {
            let _password = parts.next();
            return parts.next().and_then(|gid| gid.parse().ok());
        }
    }
    None
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn kmsg(message: &str) {
    let _ = fs::write("/dev/kmsg", format!("<6>sbm-core: {message}\n"));
}

fn ack(message: impl Into<String>) -> Handled {
    Handled {
        response: ClientResponse::Ack {
            message: message.into(),
        },
        after: AfterAction::None,
    }
}

fn error(message: impl Into<String>, code: Option<SbmErrorCode>) -> Handled {
    Handled {
        response: ClientResponse::Error {
            message: message.into(),
            code,
        },
        after: AfterAction::None,
    }
}

#[cfg(target_os = "linux")]
fn peer_allowed(stream: &UnixStream) -> bool {
    // `UnixStream::peer_cred` is still unstable on Rust 1.100 (feature
    // `peer_credentials_unix_socket`). rustix reads `SO_PEERCRED` instead.
    use std::os::fd::AsFd;
    match rustix::net::sockopt::get_socket_peercred(stream.as_fd()) {
        Ok(cred) => cred.uid.as_raw() == 0 || cred.uid.as_raw() == 1000,
        Err(_) => false,
    }
}

#[cfg(not(target_os = "linux"))]
fn peer_allowed(_stream: &UnixStream) -> bool {
    true
}
