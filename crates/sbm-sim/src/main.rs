//! `sbm-sim --session DIR [--ui PATH | --no-ui]`
//!
//! Serves the session over a private Unix socket, starts `sbm-ui` (built with
//! `--features native-sim`) pointed at it, and exits when the shell asks for a
//! power action or its window is closed. The outcome is left in `DIR/power`.

use std::fs;
use std::io::ErrorKind;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::Duration;

use sbm_runtime::ipc::{ClientRequest, ClientResponse, read_message, write_message};
use sbm_sim::{POWER_PATH, PowerOutcome, Session};
use sbm_types::SbmErrorCode;

const USAGE: &str = "usage: sbm-sim --session DIR [--ui PATH | --no-ui]";

fn main() {
    if let Err(err) = run() {
        eprintln!("sbm-sim: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut session_dir = None;
    let mut ui = None;
    let mut no_ui = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--session" => session_dir = args.next().map(PathBuf::from),
            "--ui" => ui = args.next().map(PathBuf::from),
            "--no-ui" => no_ui = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
        }
    }
    let dir = session_dir.ok_or(USAGE)?;
    let session = Session::open(&dir)?;
    let _ = fs::remove_file(dir.join(POWER_PATH));

    let socket = socket_path(&dir);
    let _ = fs::remove_file(&socket);
    let listener =
        UnixListener::bind(&socket).map_err(|err| format!("bind {}: {err}", socket.display()))?;
    listener
        .set_nonblocking(true)
        .map_err(|err| err.to_string())?;
    eprintln!(
        "sbm-sim: SIMULATED Core for scenario {} on {}",
        session.scenario(),
        socket.display()
    );

    let mut child = if no_ui {
        None
    } else {
        Some(spawn_ui(ui, &socket)?)
    };
    let outcome = serve(&session, &listener, child.as_mut());
    if let Some(mut child) = child {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = fs::remove_file(&socket);
    let outcome = outcome?;
    fs::write(dir.join(POWER_PATH), outcome.as_str()).map_err(|err| err.to_string())?;
    eprintln!("sbm-sim: leaving Core ({})", outcome.as_str());
    Ok(())
}

fn serve(
    session: &Session,
    listener: &UnixListener,
    mut child: Option<&mut Child>,
) -> Result<PowerOutcome, String> {
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(8)));
                let (response, outcome) = match read_message::<ClientRequest>(&mut stream) {
                    Ok(request) => session.handle(request),
                    Err(err) => (
                        ClientResponse::Error {
                            message: err,
                            code: Some(SbmErrorCode::InvalidSchema),
                        },
                        None,
                    ),
                };
                let _ = write_message(&mut stream, &response);
                if let Some(outcome) = outcome {
                    // Let the shell show the acknowledgement before it closes.
                    thread::sleep(Duration::from_millis(400));
                    return Ok(outcome);
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                if let Some(child) = child.as_deref_mut()
                    && let Ok(Some(_)) = child.try_wait()
                {
                    // Closing the Core window counts as a reboot to the menu.
                    return Ok(PowerOutcome::Reboot);
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(err) => return Err(format!("accept: {err}")),
        }
    }
}

fn spawn_ui(ui: Option<PathBuf>, socket: &Path) -> Result<Child, String> {
    let ui = match ui {
        Some(path) => path,
        None => std::env::current_exe()
            .map_err(|err| err.to_string())?
            .with_file_name("sbm-ui"),
    };
    Command::new(&ui)
        .env("SBM_SIM_CORED_SOCKET", socket)
        .spawn()
        .map_err(|err| {
            format!(
                "start {}: {err}. Build it with `cargo build -p sbm-ui --features native-sim`.",
                ui.display()
            )
        })
}

/// Unix socket paths are limited to about 104 bytes on macOS.
fn socket_path(dir: &Path) -> PathBuf {
    let path = dir.join("cored.sock");
    if path.as_os_str().len() < 100 {
        path
    } else {
        std::env::temp_dir().join(format!("sbm-sim-{}.sock", std::process::id()))
    }
}
