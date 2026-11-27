//! Privileged operations for SBM Core.
//!
//! `sbm-ui` sends a fixed request enum. This process is the only one that
//! execs recovery tools, and it never invokes a shell.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::{PermissionsExt, chown};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sbm_protocol::BootSuccessReport;
use sbm_runtime::discover::{backing_disk, infer_installed_os, parse_lsblk};
use sbm_runtime::handoff::{
    BOOT_ESP_VARIABLE, EspCandidate, cmdline_value, decode_efivar_utf16, load_handoff,
    native_cpu_arch, read_integrated, select_integrated_esp, write_esp_record,
};
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

const BOOT_EFI_MOUNT: &str = "/run/sbm/boot-efi";

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
    let sbm_gid = group_gid("sbm");
    if let Some(gid) = sbm_gid {
        let _ = chown(dir, Some(0), Some(gid));
    }
    let socket_path = Path::new(SOCKET_PATH);
    let _ = fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path).map_err(|err| err.to_string())?;
    let _ = fs::set_permissions(socket_path, fs::Permissions::from_mode(0o660));
    if let Some(gid) = sbm_gid {
        let _ = chown(socket_path, Some(0), Some(gid));
    }
    let boot = snapshot();
    kmsg(&format!(
        "SBM_BOOT_METRICS mode={} mem_total_kib={} mem_available_kib={} kernel={} backing={} handoff_verified={}",
        boot.toram_mode,
        boot.memory.total_kib,
        boot.memory.available_kib,
        boot.kernel,
        boot.backing_disk.as_deref().unwrap_or("none"),
        boot.handoff_verified,
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
    let Some(source) = read_line("/run/sbm/root-source") else {
        return false;
    };
    let Some(disk) = resolve_disk(&source) else {
        return false;
    };
    let _ = fs::write("/run/sbm/backing-disk", format!("{disk}\n"));
    if toram_mode() != "backing-protected" {
        return false;
    }
    // Wait briefly for udev/lsblk to expose the boot ESP metadata so we never
    // accidentally mark the boot ESP read-only before systemd mounts /boot.
    let mut esp_dev = None;
    for _ in 0..30 {
        if let Ok(cand) = resolve_boot_esp_candidate() {
            esp_dev = Some(cand.path);
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let Some(esp_path) = esp_dev else {
        kmsg(&format!(
            "SBM_PROTECT status=deferred backing={disk} reason=esp-unresolved"
        ));
        return false;
    };
    set_ro(&esp_path, false);
    let mounts = mount_table();
    let name = disk.trim_start_matches("/dev/");
    let sys = PathBuf::from("/sys/class/block").join(name);
    let Ok(entries) = fs::read_dir(&sys) else {
        return false;
    };
    let mut protected = 0_u32;
    for entry in entries.flatten() {
        let part_name = entry.file_name().to_string_lossy().to_string();
        if !part_name.starts_with(name) || !entry.path().join("partition").exists() {
            continue;
        }
        let dev = format!("/dev/{part_name}");
        if same_block_device(&dev, &esp_path) {
            continue;
        }
        if mounts.iter().any(|(device, point, _)| {
            same_block_device(device, &dev)
                && (point == "/efi" || point == BOOT_EFI_MOUNT || point == "/boot")
        }) {
            continue;
        }
        set_ro(&dev, true);
        protected += 1;
    }
    kmsg(&format!(
        "SBM_PROTECT status=backing-protected backing={disk} esp={esp_path} ro_partitions={protected}"
    ));
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
            let (handoff, mount) = read_handoff();
            let root_source = read_line("/run/sbm/root-source").unwrap_or_else(|| "unknown".into());
            let esp_path = resolve_boot_esp_candidate()
                .map(|c| c.path)
                .unwrap_or_else(|_| "none".into());
            kmsg(&format!(
                "SBM_UI_READY handoff_verified={} esp={} root_source={}",
                handoff.verified, esp_path, root_source
            ));
            match write_boot_success(&handoff, mount.as_deref()) {
                Ok(Some((attempt_id, target_id))) => {
                    kmsg(&format!(
                        "SBM_BOOT_SUCCESS_WRITTEN attempt_id={attempt_id} target_id={target_id}"
                    ));
                    ack(format!("desktop ready (confirmed {attempt_id})"))
                }
                Ok(None) => {
                    kmsg(&format!(
                        "SBM_BOOT_SUCCESS_DECLINED reason={}",
                        handoff.detail
                    ));
                    ack("desktop ready (handoff unverified; no confirmation written)")
                }
                Err(err) => {
                    kmsg(&format!("SBM_BOOT_SUCCESS_DECLINED reason={err}"));
                    ack(format!("desktop ready (report error: {err})"))
                }
            }
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
    snap.backing_disk = current_backing_disk();
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
    let (handoff, _) = read_handoff();
    snap.boot_targets = handoff.boot_targets();
    snap.handoff = handoff.detail;
    snap.handoff_verified = handoff.verified;
    snap.renderer = active_renderer();
    snap.remote_enabled = remote_enabled();
    snap.remote_detail = if snap.remote_enabled {
        "wayvnc is enabled on 127.0.0.1:5900 with authentication".to_string()
    } else {
        "remote access is disabled until an operator enables it".to_string()
    };
    snap
}

fn active_renderer() -> String {
    if let Some(live) = read_line("/run/user/1000/sbm-renderer") {
        return live;
    }
    let cmdline = fs::read_to_string("/proc/cmdline").unwrap_or_default();
    let forced = cmdline_value(&cmdline, "sbm.renderer").or_else(|| {
        resolve_boot_esp_candidate()
            .and_then(|esp| boot_efi_mount(&esp))
            .ok()
            .and_then(|root| read_line(&root.join("EFI/sbm/state/renderer").to_string_lossy()))
    });
    renderer_hint(forced.as_deref(), Path::new("/dev/dri/renderD128").exists())
}

fn power(action: PowerAction) -> Handled {
    match action {
        PowerAction::Reboot => {
            kmsg("SBM_POWER_ACTION action=reboot");
            Handled {
                response: ClientResponse::Ack {
                    message: "rebooting".to_string(),
                },
                after: AfterAction::Reboot,
            }
        }
        PowerAction::Shutdown => {
            kmsg("SBM_POWER_ACTION action=shutdown");
            Handled {
                response: ClientResponse::Ack {
                    message: "shutting down".to_string(),
                },
                after: AfterAction::PowerOff,
            }
        }
        PowerAction::BootManager => {
            kmsg("SBM_POWER_ACTION action=boot-manager");
            Handled {
                response: ClientResponse::Ack {
                    message: "returning to the boot manager".to_string(),
                },
                after: AfterAction::Reboot,
            }
        }
        PowerAction::BootTarget { target_id } => {
            let (handoff, mount) = read_handoff();
            let command = match handoff.boot_request(BootTargetId(target_id), unix_now()) {
                Ok(command) => command,
                Err((message, code)) => return error(message, Some(code)),
            };
            let Some(root) = mount else {
                return error(
                    "boot ESP is unavailable",
                    Some(SbmErrorCode::StorageIoError),
                );
            };
            let bytes = match serde_json::to_vec(&command) {
                Ok(bytes) => bytes,
                Err(err) => return error(err.to_string(), Some(SbmErrorCode::InvalidSchema)),
            };
            if let Err(err) =
                write_esp_record(&root.join("EFI/sbm/handoff/requested-command.json"), &bytes)
            {
                return error(err.to_string(), Some(SbmErrorCode::StorageIoError));
            }
            kmsg("SBM_POWER_ACTION action=boot-target");
            Handled {
                response: ClientResponse::Ack {
                    message: "requested the next boot target".to_string(),
                },
                after: AfterAction::Reboot,
            }
        }
    }
}

fn authorize_xwayland_root(display: &str, wayland: &str, xauthority: Option<&str>) {
    let Some(xhost) = first_existing(&["/usr/bin/xhost", "/usr/sbin/xhost"]) else {
        return;
    };
    let mut cmd = Command::new(xhost);
    cmd.arg("+si:localuser:root")
        .uid(1000)
        .gid(1000)
        .env("HOME", "/var/lib/sbm")
        .env("USER", "sbm")
        .env("LOGNAME", "sbm")
        .env("XDG_RUNTIME_DIR", "/run/user/1000")
        .env("DISPLAY", display)
        .env("WAYLAND_DISPLAY", wayland)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(auth) = xauthority {
        cmd.env("XAUTHORITY", auth);
    }
    let Ok(mut child) = cmd.spawn() else {
        return;
    };
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(4) {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(_) => return,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
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

    let env = labwc_environ();
    let display = env
        .as_ref()
        .and_then(|map| map.get("DISPLAY").cloned())
        .unwrap_or_else(|| ":0".to_string());
    let wayland = env
        .as_ref()
        .and_then(|map| map.get("WAYLAND_DISPLAY").cloned())
        .unwrap_or_else(wayland_display);
    let xauthority = env.as_ref().and_then(|map| map.get("XAUTHORITY").cloned());

    // labwc runs Xwayland lazily (-terminate 10). Re-grant root access as the
    // sbm user immediately before launching gpartedbin so a fresh Xwayland
    // instance always authorizes the connection.
    authorize_xwayland_root(&display, &wayland, xauthority.as_deref());

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
        .env("DISPLAY", &display)
        .env("WAYLAND_DISPLAY", &wayland)
        .env("XDG_RUNTIME_DIR", "/run/user/0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            log.unwrap_or_else(|_| File::open("/dev/null").expect("devnull")),
        ));
    if let Some(auth) = &xauthority {
        command.env("XAUTHORITY", auth);
    }
    let protected = toram_mode() == "backing-protected";
    let lease = if protected {
        let disk = current_backing_disk().ok_or_else(|| {
            "backing-protected mode could not resolve the recovery backing disk; refusing to start GParted".to_string()
        })?;
        Some(GpartedDiskLease::acquire(disk)?)
    } else {
        None
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            drop(lease);
            return Err(err.to_string());
        }
    };
    kmsg(&format!("SBM_GPARTED_LAUNCHED pid={}", child.id()));
    thread::spawn(move || {
        let _ = child.wait();
        drop(lease);
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
    let password = random_password()?;
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

fn read_efivar(name: &str) -> Option<Vec<u8>> {
    fs::read(format!("/sys/firmware/efi/efivars/{name}")).ok()
}

fn current_backing_disk() -> Option<String> {
    read_line("/run/sbm/backing-disk")
        .or_else(|| read_line("/run/sbm/root-source").and_then(|source| resolve_disk(&source)))
}

fn resolve_boot_esp_candidate() -> Result<EspCandidate, String> {
    let expected = read_efivar(BOOT_ESP_VARIABLE)
        .and_then(|bytes| decode_efivar_utf16(&bytes))
        .ok_or("boot ESP EFI variable is missing")?;
    resolve_esp(&expected)
}

fn resolve_esp(expected: &str) -> Result<EspCandidate, String> {
    let backing = current_backing_disk();

    let json = command_stdout(
        "/usr/bin/lsblk",
        &[
            "--json",
            "-b",
            "-o",
            "NAME,PATH,TYPE,PARTTYPE,PARTUUID,FSTYPE,PKNAME",
        ],
    )
    .ok_or_else(|| "lsblk unavailable for ESP discovery".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|err| format!("lsblk json parse failed: {err}"))?;
    let devices = value
        .get("blockdevices")
        .and_then(|item| item.as_array())
        .ok_or_else(|| "lsblk output missing blockdevices".to_string())?;

    let mut candidates = Vec::new();
    for disk in devices {
        let disk_path = disk
            .get("path")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let Some(children) = disk.get("children").and_then(|item| item.as_array()) else {
            continue;
        };
        for part in children {
            let Some(path) = part.get("path").and_then(|item| item.as_str()) else {
                continue;
            };
            let parent = part
                .get("pkname")
                .and_then(|v| v.as_str())
                .map(|pk| format!("/dev/{pk}"))
                .or_else(|| disk_path.clone());
            candidates.push(EspCandidate {
                path: path.to_string(),
                partuuid: part
                    .get("partuuid")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                parttype: part
                    .get("parttype")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                fstype: part
                    .get("fstype")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                parent_disk: parent,
            });
        }
    }
    select_integrated_esp(&candidates, expected, backing.as_deref())
}

fn boot_efi_mount(esp: &EspCandidate) -> Result<PathBuf, String> {
    set_ro(&esp.path, false);
    let mounts = mount_table();
    for point in ["/efi", BOOT_EFI_MOUNT, "/boot"] {
        if let Some((_, _, opts)) = verified_esp_mount(&mounts, point, &esp.path) {
            if opts.split(',').any(|o| o == "ro") {
                let status = Command::new("mount")
                    .args(["-o", "remount,rw", point])
                    .status()
                    .map_err(|err| err.to_string())?;
                if !status.success() {
                    return Err(format!("failed to remount the boot ESP at {point}"));
                }
            }
            return Ok(PathBuf::from(point));
        }
    }
    let efi_taken = mounts.iter().any(|(_, point, _)| point == "/efi");
    let target = if efi_taken {
        PathBuf::from(BOOT_EFI_MOUNT)
    } else {
        PathBuf::from("/efi")
    };
    if mounts
        .iter()
        .any(|(_, point, _)| Path::new(point) == target)
    {
        return Err("boot ESP mount point is occupied".into());
    }
    fs::create_dir_all(&target).map_err(|err| err.to_string())?;
    let status = Command::new("mount")
        .args(["-t", "vfat", "-o", "rw", &esp.path])
        .arg(&target)
        .status()
        .map_err(|err| err.to_string())?;
    if !status.success() {
        return Err(format!(
            "failed to mount verified boot ESP {} at {}",
            esp.path,
            target.display()
        ));
    }
    Ok(target)
}

fn verified_esp_mount<'a>(
    mounts: &'a [(String, String, String)],
    mountpoint: &str,
    esp_device: &str,
) -> Option<&'a (String, String, String)> {
    let mut matching = mounts.iter().filter(|(_, point, _)| point == mountpoint);
    let entry = matching.next()?;
    (matching.next().is_none() && same_block_device(&entry.0, esp_device)).then_some(entry)
}

fn same_block_device(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

fn read_handoff() -> (HandoffLoad, Option<PathBuf>) {
    let mut mount = None;
    let mut load = read_integrated(read_efivar, native_cpu_arch(), |partuuid| {
        let esp = resolve_esp(partuuid)?;
        let root = boot_efi_mount(&esp)?;
        let bytes = fs::read(root.join("EFI/sbm/handoff/current.json"))
            .map_err(|err| format!("could not read the EFI handoff: {err}"))?;
        mount = Some(root);
        Ok(bytes)
    });
    if !load.verified && read_efivar(BOOT_ESP_VARIABLE).is_none() {
        let cmdline = fs::read_to_string("/proc/cmdline").unwrap_or_default();
        if let Some(uuid) = cmdline_value(&cmdline, "sbm.esp_partuuid")
            && let Ok(esp) = resolve_esp(&uuid)
        {
            // Legacy diagnostics may read an existing, identity-checked mount.
            let mounts = mount_table();
            if let Some((_, point, _)) = ["/efi", "/boot", BOOT_EFI_MOUNT]
                .into_iter()
                .find_map(|point| verified_esp_mount(&mounts, point, &esp.path))
            {
                let bytes = fs::read(Path::new(&point).join("EFI/sbm/handoff/current.json")).ok();
                load = load_handoff(bytes.as_deref(), &cmdline);
            }
        }
    }
    (load, mount)
}

fn write_boot_success(
    handoff: &HandoffLoad,
    root: Option<&Path>,
) -> Result<Option<(String, String)>, String> {
    let (Some(payload), Some(root)) = (handoff.payload.as_ref().filter(|_| handoff.verified), root)
    else {
        return Ok(None);
    };
    let target_id = SLOT_A_TARGET_ID.to_string();
    let report = BootSuccessReport {
        schema_version: SCHEMA_VERSION_V1.into(),
        attempt_id: payload.attempt_id.clone(),
        target_id: BootTargetId(target_id.clone()),
        confirmed_by: ComponentKind::SbmCoreUki,
        confirmed_at_unix: unix_now(),
    };
    let bytes = serde_json::to_vec(&report).map_err(|err| err.to_string())?;
    write_esp_record(&root.join("EFI/sbm/state/boot-success.json"), &bytes)
        .map_err(|err| err.to_string())?;
    Ok(Some((payload.attempt_id.clone(), target_id)))
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
    let trimmed = source.trim();
    if trimmed.is_empty() || trimmed == "missing" {
        return None;
    }
    let mut current = fs::canonicalize(trimmed)
        .ok()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| trimmed.to_string());
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

#[derive(Default)]
struct BackingRoTracker {
    counts: HashMap<String, usize>,
}

impl BackingRoTracker {
    fn acquire_with<F>(&mut self, disk: &str, mut apply_ro: F) -> Result<(), String>
    where
        F: FnMut(&str, bool) -> Result<(), String>,
    {
        let count = self.counts.entry(disk.to_string()).or_insert(0);
        if *count == 0 {
            apply_ro(disk, true)?;
        }
        *count += 1;
        Ok(())
    }

    fn release_with<F>(&mut self, disk: &str, mut on_last_release: F)
    where
        F: FnMut(&str),
    {
        if let Some(count) = self.counts.get_mut(disk) {
            if *count > 1 {
                *count -= 1;
                return;
            }
            self.counts.remove(disk);
            on_last_release(disk);
        }
    }

    #[cfg(test)]
    fn active_count(&self, disk: &str) -> usize {
        self.counts.get(disk).copied().unwrap_or(0)
    }
}

static GPARTED_RO_TRACKER: OnceLock<Mutex<BackingRoTracker>> = OnceLock::new();

fn gparted_ro_tracker() -> &'static Mutex<BackingRoTracker> {
    GPARTED_RO_TRACKER.get_or_init(|| Mutex::new(BackingRoTracker::default()))
}

struct GpartedDiskLease {
    disk: String,
}

impl GpartedDiskLease {
    fn acquire(disk: String) -> Result<Self, String> {
        let mut guard = gparted_ro_tracker()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.acquire_with(&disk, set_ro_checked)?;
        Ok(Self { disk })
    }
}

impl Drop for GpartedDiskLease {
    fn drop(&mut self) {
        let mut guard = gparted_ro_tracker()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.release_with(&self.disk, |disk| {
            set_ro(disk, false);
            // Re-assert non-ESP partition read-only protection immediately after
            // clearing the whole-disk lock so backing root/verity partitions
            // stay protected under all failure and exit paths.
            let _ = protect_backing_device();
        });
    }
}

fn set_ro_checked(dev: &str, ro: bool) -> Result<(), String> {
    if !dev.starts_with("/dev/") {
        return Err(format!("invalid block device path {dev}"));
    }
    let flag = if ro { "--setro" } else { "--setrw" };
    let bin = first_existing(&["/usr/sbin/blockdev", "/usr/bin/blockdev"])
        .unwrap_or_else(|| "blockdev".into());
    let status = Command::new(bin)
        .args([flag, dev])
        .status()
        .map_err(|err| format!("blockdev {flag} {dev} failed: {err}"))?;
    if !status.success() {
        return Err(format!("blockdev {flag} {dev} exited with {status}"));
    }
    Ok(())
}

fn set_ro(dev: &str, ro: bool) {
    let _ = set_ro_checked(dev, ro);
}

fn mount_table() -> Vec<(String, String, String)> {
    let text = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let mut rows = Vec::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let left_fields: Vec<&str> = left.split_whitespace().collect();
        let right_fields: Vec<&str> = right.split_whitespace().collect();
        if left_fields.len() >= 6 && right_fields.len() >= 2 {
            rows.push((
                right_fields[1].to_string(),
                left_fields[4].to_string(),
                left_fields[5].to_string(),
            ));
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
        let Some(fname) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Ok(pid) = fname.parse::<i32>() else {
            continue;
        };
        let Ok(comm) = fs::read_to_string(format!("/proc/{pid}/comm")) else {
            continue;
        };
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

fn random_password_from_reader<R: Read>(reader: &mut R) -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    reader
        .read_exact(&mut bytes)
        .map_err(|err| format!("failed to read entropy for remote access password: {err}"))?;
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    Ok(bytes
        .iter()
        .map(|byte| ALPHABET[(*byte as usize) % ALPHABET.len()] as char)
        .collect())
}

fn random_password() -> Result<String, String> {
    let mut file = File::open("/dev/urandom")
        .map_err(|err| format!("failed to open /dev/urandom for remote access password: {err}"))?;
    random_password_from_reader(&mut file)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gparted_ro_tracker_refcounts_concurrent_instances() {
        let mut tracker = BackingRoTracker::default();
        let mut events: Vec<(String, bool)> = Vec::new();

        tracker
            .acquire_with("/dev/sda", |disk, ro| {
                events.push((disk.to_string(), ro));
                Ok(())
            })
            .unwrap();
        assert_eq!(tracker.active_count("/dev/sda"), 1);

        // Second concurrent instance reuses the active RO hold without re-issuing blockdev.
        tracker
            .acquire_with("/dev/sda", |disk, ro| {
                events.push((disk.to_string(), ro));
                Ok(())
            })
            .unwrap();
        assert_eq!(tracker.active_count("/dev/sda"), 2);
        assert_eq!(events, vec![("/dev/sda".to_string(), true)]);

        // First instance exits: whole-disk RO must remain engaged.
        tracker.release_with("/dev/sda", |disk| {
            events.push((disk.to_string(), false));
        });
        assert_eq!(tracker.active_count("/dev/sda"), 1);
        assert_eq!(events, vec![("/dev/sda".to_string(), true)]);

        // Last instance exits: whole-disk RO clears once.
        tracker.release_with("/dev/sda", |disk| {
            events.push((disk.to_string(), false));
        });
        assert_eq!(tracker.active_count("/dev/sda"), 0);
        assert_eq!(
            events,
            vec![
                ("/dev/sda".to_string(), true),
                ("/dev/sda".to_string(), false)
            ]
        );
    }

    #[test]
    fn gparted_ro_tracker_rolls_back_failed_spawn_without_dropping_other_holders() {
        let mut tracker = BackingRoTracker::default();
        let mut releases = 0usize;

        // Single instance acquire + spawn failure -> immediate rollback clears RO.
        tracker
            .acquire_with("/dev/sda", |_disk, _ro| Ok(()))
            .unwrap();
        tracker.release_with("/dev/sda", |_| releases += 1);
        assert_eq!(tracker.active_count("/dev/sda"), 0);
        assert_eq!(releases, 1);

        // First instance active, second acquire + spawn failure -> stays RO.
        tracker
            .acquire_with("/dev/sda", |_disk, _ro| Ok(()))
            .unwrap();
        tracker
            .acquire_with("/dev/sda", |_disk, _ro| Ok(()))
            .unwrap();
        tracker.release_with("/dev/sda", |_| releases += 1);
        assert_eq!(tracker.active_count("/dev/sda"), 1);
        assert_eq!(releases, 1);

        // First instance exits -> now releases.
        tracker.release_with("/dev/sda", |_| releases += 1);
        assert_eq!(tracker.active_count("/dev/sda"), 0);
        assert_eq!(releases, 2);
    }

    #[test]
    fn gparted_ro_tracker_fails_closed_when_setro_fails() {
        let mut tracker = BackingRoTracker::default();
        let err = tracker
            .acquire_with("/dev/sda", |_disk, _ro| Err("blockdev failed".into()))
            .unwrap_err();
        assert!(err.contains("blockdev failed"));
        assert_eq!(tracker.active_count("/dev/sda"), 0);
    }

    #[test]
    fn random_password_fails_closed_on_short_entropy() {
        let mut empty: &[u8] = &[];
        assert!(random_password_from_reader(&mut empty).is_err());

        let mut short: &[u8] = &[1, 2, 3];
        assert!(random_password_from_reader(&mut short).is_err());

        let mut ok: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let pw = random_password_from_reader(&mut ok).unwrap();
        assert_eq!(pw.len(), 16);
    }

    #[test]
    fn production_mkosi_extra_contains_no_smoke_service_or_script() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let extra = manifest_dir.join("../../mkosi.extra");
        let preset = fs::read_to_string(extra.join("usr/lib/systemd/system-preset/00-sbm.preset"))
            .expect("read 00-sbm.preset");
        assert!(
            !preset.contains("sbm-smoke"),
            "production 00-sbm.preset must not reference sbm-smoke"
        );
        assert!(
            !extra
                .join("usr/lib/systemd/system/sbm-smoke.service")
                .exists(),
            "production mkosi.extra must not ship sbm-smoke.service"
        );
        assert!(
            !extra.join("usr/lib/sbm/sbm-smoke.sh").exists(),
            "production mkosi.extra must not ship sbm-smoke.sh"
        );
    }
}
