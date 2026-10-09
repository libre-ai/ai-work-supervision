//! `ws-cockpit --root <dir> [--port <n>]` (or `WS_ROOT`): the read-and-decide web cockpit.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let (root, port) = match words.as_slice() {
        ["--root", root] => (Some(PathBuf::from(root)), Some(0)),
        ["--root", root, "--port", port] => (Some(PathBuf::from(root)), port.parse().ok()),
        [] => (std::env::var_os("WS_ROOT").map(PathBuf::from), Some(0)),
        _ => (None, None),
    };
    let (Some(root), Some(port)) = (root.filter(|root| root.is_absolute()), port) else {
        eprintln!("usage: ws-cockpit --root <absolute directory> [--port <n>]");
        return ExitCode::from(2);
    };
    match work_supervision_cockpit::Cockpit::bind(&root, port) {
        Ok(cockpit) => {
            println!(
                "ws-cockpit: http://{}/login (token in run/cockpit.token of the root)",
                cockpit.local_address()
            );
            cockpit.serve();
            ExitCode::SUCCESS
        }
        Err(code) => {
            eprintln!("ws-cockpit: {code}");
            ExitCode::from(1)
        }
    }
}
