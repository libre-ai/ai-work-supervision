//! `ws-fake-agent <scenario>` — the scenario-driven fake agent of Work Supervision v0.
//!
//! It is the only executor before the C0 confinement qualification. It reads a
//! scenario (in Work Supervision, the mission brief) and executes one step per
//! line, in its working directory (the mission worktree):
//!
//! | Step | Effect |
//! | --- | --- |
//! | `print <text>` | writes the text and a newline |
//! | `flood <n>` | writes exactly `n` bytes `abc…z` repeated, without newline |
//! | `read-line` | reads one line of input and writes `received <hex of its bytes>` |
//! | `sleep <ms>` | sleeps |
//! | `block` | sleeps forever (a stuck agent) |
//! | `write-file <relative path> <text>` | writes the text and a newline, creating directories; the path may not leave the working directory |
//! | `commit <message>` | `git add -A` then `git commit` with a fixed fake identity, hooks disabled |
//! | `spawn-stubborn-child` | starts a child shell in its process group that ignores `SIGTERM` and `SIGHUP` |
//! | `env` | writes `env <NAME>` for every environment variable, sorted |
//! | `print-cwd` | writes `cwd <absolute working directory>` |
//! | `crash` | aborts (`SIGABRT`) |
//! | `exit <code>` | exits with the code |
//!
//! Blank lines and lines starting with `#` are skipped. An unknown step, a
//! malformed argument or a refused path exits with code 2. Reaching the end of
//! the scenario exits with code 0.

use std::io::{self, BufRead as _, Write as _};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let (Some(scenario), None) = (arguments.next(), arguments.next()) else {
        eprintln!("usage: ws-fake-agent <scenario>");
        return ExitCode::from(2);
    };
    let Ok(text) = std::fs::read_to_string(&scenario) else {
        eprintln!("ws-fake-agent: scenario unreadable");
        return ExitCode::from(2);
    };
    for (index, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match step(line) {
            Ok(Flow::Continue) => {}
            Ok(Flow::Exit(code)) => return ExitCode::from(code),
            Err(reason) => {
                eprintln!("ws-fake-agent: step {} refused: {reason}", index + 1);
                return ExitCode::from(2);
            }
        }
    }
    ExitCode::SUCCESS
}

enum Flow {
    Continue,
    Exit(u8),
}

fn step(line: &str) -> Result<Flow, &'static str> {
    let (name, argument) = line.split_once(' ').unwrap_or((line, ""));
    let mut out = io::stdout().lock();
    match name {
        "print" => writeln!(out, "{argument}").map_err(|_| "write")?,
        "flood" => {
            let mut remaining: u64 = argument.parse().map_err(|_| "flood size")?;
            let pattern: Vec<u8> = (0..65_520_u32)
                .map(|index| b'a' + u8::try_from(index % 26).unwrap_or(0))
                .collect();
            while remaining > 0 {
                let take = usize::try_from(remaining.min(65_520)).map_err(|_| "flood size")?;
                let chunk = pattern.get(..take).ok_or("flood size")?;
                out.write_all(chunk).map_err(|_| "write")?;
                remaining -= u64::try_from(take).map_err(|_| "flood size")?;
            }
        }
        "read-line" => {
            out.flush().map_err(|_| "write")?;
            let mut input = String::new();
            io::stdin()
                .lock()
                .read_line(&mut input)
                .map_err(|_| "read")?;
            let line = input.trim_end_matches(['\n', '\r']);
            let hex: String = line.bytes().map(|byte| format!("{byte:02x}")).collect();
            writeln!(out, "received {hex}").map_err(|_| "write")?;
        }
        "sleep" => {
            let milliseconds: u64 = argument.parse().map_err(|_| "sleep duration")?;
            out.flush().map_err(|_| "write")?;
            std::thread::sleep(Duration::from_millis(milliseconds));
        }
        "block" => {
            out.flush().map_err(|_| "write")?;
            loop {
                std::thread::sleep(Duration::from_secs(3_600));
            }
        }
        "write-file" => {
            let (path, content) = argument.split_once(' ').ok_or("write-file arguments")?;
            let path = confined(path)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|_| "create directory")?;
            }
            std::fs::write(&path, format!("{content}\n")).map_err(|_| "write file")?;
        }
        "commit" => {
            if argument.is_empty() {
                return Err("commit message");
            }
            git(&["add", "-A"])?;
            git(&["commit", "-q", "-m", argument])?;
        }
        "spawn-stubborn-child" => {
            Command::new("/bin/sh")
                .args(["-c", "trap '' TERM HUP; while :; do sleep 1; done"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| "spawn child")?;
        }
        "env" => {
            let mut names: Vec<String> = std::env::vars_os()
                .map(|(name, _)| name.to_string_lossy().into_owned())
                .collect();
            names.sort();
            for name in names {
                writeln!(out, "env {name}").map_err(|_| "write")?;
            }
        }
        "print-cwd" => {
            let cwd = std::env::current_dir().map_err(|_| "cwd")?;
            writeln!(out, "cwd {}", cwd.display()).map_err(|_| "write")?;
        }
        "crash" => {
            out.flush().map_err(|_| "write")?;
            std::process::abort();
        }
        "exit" => {
            out.flush().map_err(|_| "write")?;
            return Ok(Flow::Exit(argument.parse().map_err(|_| "exit code")?));
        }
        _ => return Err("unknown step"),
    }
    out.flush().map_err(|_| "write")?;
    Ok(Flow::Continue)
}

/// A relative path that stays inside the working directory.
fn confined(path: &str) -> Result<PathBuf, &'static str> {
    let candidate = Path::new(path);
    let inside = !path.is_empty()
        && candidate
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if inside {
        Ok(candidate.to_owned())
    } else {
        Err("path leaves the working directory")
    }
}

fn git(arguments: &[&str]) -> Result<(), &'static str> {
    let status = Command::new("git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Work Supervision fake agent",
            "-c",
            "user.email=fake-agent@work-supervision.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(arguments)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::null())
        .status()
        .map_err(|_| "git")?;
    if status.success() {
        Ok(())
    } else {
        Err("git failed")
    }
}
