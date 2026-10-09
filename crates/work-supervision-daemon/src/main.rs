//! `wsd --root <dir>` (or `WS_ROOT`): the Work Supervision v0 daemon.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let root = match (arguments.next(), arguments.next(), arguments.next()) {
        (Some(flag), Some(root), None) if flag == "--root" => Some(PathBuf::from(root)),
        (None, None, None) => std::env::var_os("WS_ROOT").map(PathBuf::from),
        _ => None,
    };
    let Some(root) = root.filter(|root| root.is_absolute()) else {
        eprintln!("usage: wsd --root <absolute directory>  (or WS_ROOT)");
        return ExitCode::from(2);
    };
    match work_supervision_daemon::serve(&root) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("wsd: {failure}");
            ExitCode::from(1)
        }
    }
}
