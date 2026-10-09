use std::fs;
use std::io::{BufReader, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

use serde_json::Value;
use work_supervision_store::Layout;

use crate::core::{self, Core, Shared};
use crate::protocol::{failure, read_line, success};
use crate::{Config, Failure, authorize_peer, peer_uid};

/// Runs the daemon of the root `root` until it is killed.
///
/// Startup order: configuration (the C0 guard refuses a real executor before
/// anything else), verification and recovery of the root, then the socket
/// `run/wsd.sock` (mode 0600 inside a 0700 directory). Recovery counts are
/// written to standard error; no content ever is.
///
/// # Errors
///
/// The configuration, recovery or socket failure that stopped the startup.
pub fn serve(root: &Path) -> Result<(), Failure> {
    let layout = Layout::new(root);
    let config = Config::load(&layout.config())?;
    config.fake_agent()?;
    if let Some(anchor) = config.anchor() {
        let root = fs::canonicalize(root).map_err(|_| Failure::new("root.io"))?;
        let parent = anchor
            .parent()
            .and_then(|parent| fs::canonicalize(parent).ok())
            .ok_or(Failure::new("config.anchor_invalid"))?;
        if parent.starts_with(&root) {
            return Err(Failure::new("config.anchor_inside_root"));
        }
    }
    let (shared, recovered) = Core::open(layout.clone(), config)?;
    eprintln!("wsd: recovered {}", core::report_json(&recovered));
    let socket = core::socket_path(&layout);
    let directory = socket.parent().ok_or(Failure::new("socket.io"))?;
    fs::create_dir_all(directory).map_err(|_| Failure::new("socket.io"))?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .map_err(|_| Failure::new("socket.io"))?;
    // The journal lock is held: a socket left here belongs to a dead daemon.
    match fs::remove_file(&socket) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(Failure::new("socket.io")),
    }
    let listener = UnixListener::bind(&socket).map_err(|_| Failure::new("socket.io"))?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .map_err(|_| Failure::new("socket.io"))?;
    let own = rustix::process::geteuid().as_raw();
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let shared = std::sync::Arc::clone(&shared);
        std::thread::spawn(move || connection(&shared, stream, own));
    }
    Ok(())
}

fn connection(shared: &Shared, stream: UnixStream, own: u32) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let admitted = peer_uid(&stream).and_then(|peer| authorize_peer(peer, own));
    if let Err(refusal) = admitted {
        let _ = respond(&mut writer, &failure(refusal.code()));
        return;
    }
    let mut reader = BufReader::new(stream);
    loop {
        let line = match read_line(&mut reader) {
            Ok(Some(line)) => line,
            Ok(None) => return,
            Err(_) => {
                let _ = respond(&mut writer, &failure("request.too_long"));
                return;
            }
        };
        let response = match serde_json::from_slice::<Value>(&line) {
            Ok(request) => {
                let op = request
                    .get("op")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let outcome = core::handle(shared, &request);
                core::anchor(shared);
                match outcome {
                    Ok(data) => success(&op, data),
                    Err(refusal) => failure(refusal.code()),
                }
            }
            Err(_) => failure("request.malformed"),
        };
        if respond(&mut writer, &response).is_err() {
            return;
        }
    }
}

fn respond(writer: &mut UnixStream, response: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(response).map_err(std::io::Error::other)?;
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()
}
