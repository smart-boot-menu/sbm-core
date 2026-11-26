use std::env;
use std::process::exit;

use sbm_runtime::ipc::{ClientRequest, LaunchTool, PowerAction};
use sbm_runtime::transact;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let request = match args.first().map(String::as_str) {
        None | Some("snapshot") => ClientRequest::Snapshot,
        Some("tools") => ClientRequest::ToolProbe,
        Some("gparted") => ClientRequest::Launch {
            tool: LaunchTool::Gparted,
        },
        Some("terminal") => ClientRequest::Launch {
            tool: LaunchTool::Terminal,
        },
        Some("remote-status") => ClientRequest::RemoteStatus,
        Some("remote-enable") => ClientRequest::RemoteEnable,
        Some("remote-disable") => ClientRequest::RemoteDisable,
        Some("reboot") => ClientRequest::Power {
            action: PowerAction::Reboot,
        },
        Some("shutdown") => ClientRequest::Power {
            action: PowerAction::Shutdown,
        },
        Some("boot-manager") => ClientRequest::Power {
            action: PowerAction::BootManager,
        },
        Some("boot-target") => {
            let Some(target_id) = args.get(1) else {
                eprintln!("usage: sbm-ctl boot-target <id>");
                exit(2);
            };
            ClientRequest::Power {
                action: PowerAction::BootTarget {
                    target_id: target_id.clone(),
                },
            }
        }
        Some("help") | Some("--help") => {
            eprintln!(
                "usage: sbm-ctl [snapshot|tools|gparted|terminal|remote-status|remote-enable|remote-disable|reboot|shutdown|boot-manager|boot-target <id>]"
            );
            return;
        }
        Some(other) => {
            eprintln!("unknown request {other}");
            exit(2);
        }
    };
    match transact(&request) {
        Ok(response) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&response).unwrap_or_else(|err| err.to_string())
            );
        }
        Err(err) => {
            eprintln!("{err}");
            exit(1);
        }
    }
}
