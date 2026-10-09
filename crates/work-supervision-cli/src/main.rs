//! `ws` — the command line of Work Supervision v0.
//!
//! ```text
//! ws [--root <dir>] <command>        (or WS_ROOT; no default root)
//!
//! ws init --root <dir>
//! ws status
//! ws mission new --repository <name> --title <t> --brief <file> [--criterion <c>]…
//!                [--max-duration <seconds>] [--max-output <bytes>]
//! ws mission list | show <id> | ready <id>
//! ws run <id>
//! ws send <id> <text>                 (a newline is appended)
//! ws wait <id> <state>… [--timeout-ms <n>]
//! ws result submit <id> --evidence <file> --summary <s>
//! ws decide <id> accept|reject|abandon|cancel --reason <r>
//! ws note <id> <text>
//! ws worktree gc
//! ws journal verify | head
//! ws doctor
//! ws rebuild [--check]                (daemon stopped)
//! ```
//!
//! Results are JSON on standard output. A refusal prints `ws: <code>` on
//! standard error and exits 1; a usage error exits 2. `ws journal verify`
//! exits like `ws-journal-verify`: 0 valid, 1 invalid, 2 unreadable, 3 torn tail.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};
use work_supervision_daemon::{Anchor, Client, Config, check_anchor, init_root};
use work_supervision_journal_verifier::{Outcome, verify};
use work_supervision_store::{BlobStore, Layout, Store};

enum Exit {
    Usage,
    Refused(String),
    Code(u8),
}

fn main() -> ExitCode {
    match run(std::env::args_os().skip(1).collect()) {
        Ok(value) => {
            match serde_json::to_string_pretty(&value) {
                Ok(text) => println!("{text}"),
                Err(_) => return ExitCode::from(1),
            }
            ExitCode::SUCCESS
        }
        Err(Exit::Usage) => {
            eprintln!("ws: usage error (see the crate documentation for the commands)");
            ExitCode::from(2)
        }
        Err(Exit::Refused(code)) => {
            eprintln!("ws: {code}");
            ExitCode::from(1)
        }
        Err(Exit::Code(code)) => ExitCode::from(code),
    }
}

fn refused(code: &str) -> Exit {
    Exit::Refused(code.to_owned())
}

fn text(argument: &OsString) -> Result<String, Exit> {
    argument.to_str().map(str::to_owned).ok_or(Exit::Usage)
}

fn run(arguments: Vec<OsString>) -> Result<Value, Exit> {
    let mut arguments = arguments.into_iter().peekable();
    let mut root: Option<PathBuf> = None;
    if arguments.peek().is_some_and(|first| first == "--root") {
        arguments.next();
        root = Some(PathBuf::from(arguments.next().ok_or(Exit::Usage)?));
    }
    let rest: Vec<OsString> = arguments.collect();
    let words: Vec<String> = rest.iter().map(text).collect::<Result<_, _>>()?;
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    if let ["init", "--root", directory] = words.as_slice() {
        init_root(Path::new(directory)).map_err(|failure| refused(failure.code()))?;
        return Ok(json!({ "root": directory }));
    }
    let root = root
        .or_else(|| std::env::var_os("WS_ROOT").map(PathBuf::from))
        .filter(|root| root.is_absolute())
        .ok_or(Exit::Usage)?;
    let layout = Layout::new(&root);
    match words.as_slice() {
        ["journal", "verify"] => journal(&layout, false),
        ["journal", "head"] => journal(&layout, true),
        ["rebuild"] => rebuild(&layout, false),
        ["rebuild", "--check"] => rebuild(&layout, true),
        ["mission", "new", options @ ..] => mission_new(&layout, options),
        ["result", "submit", id, options @ ..] => {
            let options = Options::parse(options, &["--evidence", "--summary"], &[])?;
            let bytes =
                fs::read(options.one("--evidence")?).map_err(|_| refused("evidence.unreadable"))?;
            let digest = BlobStore::open(&layout.evidence())
                .and_then(|store| store.put(&bytes))
                .map_err(|failure| refused(failure.code()))?;
            request(
                &layout,
                &json!({ "op": "result.submit", "mission": id, "evidence_digest": digest.to_hex(), "summary": options.one("--summary")? }),
            )
        }
        _ => request(&layout, &simple_request(&words)?),
    }
}

fn simple_request(words: &[&str]) -> Result<Value, Exit> {
    Ok(match words {
        ["status"] => json!({ "op": "status" }),
        ["mission", "list"] => json!({ "op": "mission.list" }),
        ["mission", "show", id] => json!({ "op": "mission.show", "mission": id }),
        ["mission", "ready", id] => json!({ "op": "mission.ready", "mission": id }),
        ["run", id] => json!({ "op": "run", "mission": id }),
        ["send", id, input] => json!({ "op": "send", "mission": id, "text": format!("{input}\n") }),
        ["note", id, note] => json!({ "op": "note", "mission": id, "text": note }),
        ["decide", id, decision, "--reason", reason] => {
            json!({ "op": "decide", "mission": id, "decision": decision, "reason": reason })
        }
        ["wait", id, states @ ..] => {
            let mut timeout = 30_000_u64;
            let mut wanted = Vec::new();
            let mut iterator = states.iter();
            while let Some(word) = iterator.next() {
                if *word == "--timeout-ms" {
                    timeout = iterator
                        .next()
                        .and_then(|value| value.parse().ok())
                        .ok_or(Exit::Usage)?;
                } else {
                    wanted.push(*word);
                }
            }
            if wanted.is_empty() {
                return Err(Exit::Usage);
            }
            json!({ "op": "wait", "mission": id, "states": wanted, "timeout_ms": timeout })
        }
        ["worktree", "gc"] => json!({ "op": "worktree.gc" }),
        ["doctor"] => json!({ "op": "doctor" }),
        _ => return Err(Exit::Usage),
    })
}

fn socket(layout: &Layout) -> PathBuf {
    layout.root().join("run").join("wsd.sock")
}

fn request(layout: &Layout, request: &Value) -> Result<Value, Exit> {
    let mut client = Client::connect(&socket(layout)).map_err(|error| refused(error.code()))?;
    client
        .request(request)
        .map_err(|error| refused(error.code()))
}

struct Options<'a> {
    values: Vec<(&'a str, &'a str)>,
}

impl<'a> Options<'a> {
    /// `--name value` pairs; `single` names appear at most once, `repeated` any number of times.
    fn parse(words: &[&'a str], single: &[&str], repeated: &[&str]) -> Result<Self, Exit> {
        let mut values = Vec::new();
        let mut iterator = words.iter();
        while let Some(name) = iterator.next() {
            let value = iterator.next().ok_or(Exit::Usage)?;
            let known = single.contains(name) || repeated.contains(name);
            let duplicate = single.contains(name) && values.iter().any(|(seen, _)| seen == name);
            if !known || duplicate {
                return Err(Exit::Usage);
            }
            values.push((*name, *value));
        }
        Ok(Self { values })
    }

    fn one(&self, name: &str) -> Result<&'a str, Exit> {
        self.optional(name).ok_or(Exit::Usage)
    }

    fn optional(&self, name: &str) -> Option<&'a str> {
        self.values
            .iter()
            .find(|(seen, _)| *seen == name)
            .map(|(_, value)| *value)
    }

    fn all(&self, name: &str) -> Vec<&'a str> {
        self.values
            .iter()
            .filter(|(seen, _)| *seen == name)
            .map(|(_, value)| *value)
            .collect()
    }
}

fn mission_new(layout: &Layout, words: &[&str]) -> Result<Value, Exit> {
    let options = Options::parse(
        words,
        &[
            "--repository",
            "--title",
            "--brief",
            "--max-duration",
            "--max-output",
        ],
        &["--criterion"],
    )?;
    let brief =
        fs::read_to_string(options.one("--brief")?).map_err(|_| refused("brief.unreadable"))?;
    let number = |name: &str| -> Result<Value, Exit> {
        options
            .optional(name)
            .map(|value| {
                value
                    .parse::<u64>()
                    .map(Value::from)
                    .map_err(|_| Exit::Usage)
            })
            .transpose()
            .map(|value| value.unwrap_or(Value::Null))
    };
    request(
        layout,
        &json!({
            "op": "mission.new",
            "repository": options.one("--repository")?,
            "title": options.one("--title")?,
            "brief": brief,
            "criteria": options.all("--criterion"),
            "max_duration_seconds": number("--max-duration")?,
            "max_output_bytes": number("--max-output")?,
        }),
    )
}

fn journal(layout: &Layout, head_only: bool) -> Result<Value, Exit> {
    let Ok(file) = fs::File::open(layout.journal()) else {
        eprintln!("ws: journal unreadable");
        return Err(Exit::Code(2));
    };
    match verify(file) {
        Ok(Outcome::Valid { entries, head }) => {
            let head = head.map(|head| json!({ "seq": head.seq, "digest": head.digest }));
            if head_only {
                return Ok(head.unwrap_or(Value::Null));
            }
            let anchor = anchor_status(layout)?;
            Ok(json!({ "valid": true, "entries": entries, "head": head, "anchor": anchor }))
        }
        Ok(Outcome::Invalid { line, code, .. }) => {
            eprintln!("ws: journal invalid: {} at line {line}", code.code());
            Err(Exit::Code(1))
        }
        Ok(Outcome::TornTail { line, verified }) => {
            eprintln!("ws: journal torn tail at line {line} after {verified} verified entries");
            Err(Exit::Code(3))
        }
        Err(_) => {
            eprintln!("ws: journal unreadable");
            Err(Exit::Code(2))
        }
    }
}

/// Checks the anchor named by the configuration, if any (`journal.anchor_*` exits 1).
fn anchor_status(layout: &Layout) -> Result<Value, Exit> {
    let Ok(config) = Config::load(&layout.config()) else {
        return Ok(json!({ "status": "configuration-unreadable" }));
    };
    let Some(path) = config.anchor() else {
        return Ok(json!({ "status": "not-configured" }));
    };
    let anchor = Anchor::read(path).map_err(|failure| anchored(failure.code()))?;
    let Some(anchor) = anchor else {
        return Err(anchored("journal.anchor_missing"));
    };
    check_anchor(&layout.journal(), &anchor).map_err(|failure| anchored(failure.code()))?;
    Ok(json!({ "status": "matched", "seq": anchor.seq() }))
}

fn anchored(code: &str) -> Exit {
    eprintln!("ws: {code}");
    Exit::Code(1)
}

fn rebuild(layout: &Layout, check: bool) -> Result<Value, Exit> {
    if Client::connect(&socket(layout)).is_ok() {
        return Err(refused("daemon.running"));
    }
    let blobs = layout
        .blob_store()
        .map_err(|failure| refused(failure.code()))?;
    let target = layout.root().join("state.sqlite.rebuilt");
    remove_database(&target)?;
    let rebuilt = Store::rebuild(&layout.journal(), &blobs, &target)
        .map_err(|failure| refused(failure.code()))?;
    let fresh = rebuilt.dump().map_err(|failure| refused(failure.code()))?;
    drop(rebuilt);
    let live = if layout.state().exists() {
        Some(
            Store::open_read_only(&layout.state())
                .and_then(|store| store.dump())
                .map_err(|failure| refused(failure.code()))?,
        )
    } else {
        None
    };
    let equal = live.as_ref() == Some(&fresh);
    if check {
        remove_database(&target)?;
    } else {
        let previous = layout.root().join("state.sqlite.previous");
        remove_database(&previous)?;
        if layout.state().exists() {
            fs::rename(layout.state(), &previous).map_err(|_| refused("rebuild.io"))?;
            for suffix in ["-wal", "-shm"] {
                let _ = fs::remove_file(with_suffix(&layout.state(), suffix));
            }
        }
        fs::rename(&target, layout.state()).map_err(|_| refused("rebuild.io"))?;
    }
    Ok(json!({ "equal": equal, "had_projection": live.is_some(), "replaced": !check }))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn remove_database(path: &Path) -> Result<(), Exit> {
    for candidate in [
        path.to_owned(),
        with_suffix(path, "-wal"),
        with_suffix(path, "-shm"),
    ] {
        match fs::remove_file(&candidate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(refused("rebuild.io")),
        }
    }
    Ok(())
}
