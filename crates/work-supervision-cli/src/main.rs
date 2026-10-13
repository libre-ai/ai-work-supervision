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
//! ws attach <id>                    (terminal interface)
//! ws send <id> <text>                 (a newline is appended)
//! ws wait <id> <state>… [--timeout-ms <n>]
//! ws result submit <id> --evidence <file> --summary <s>
//! ws decide <id> accept|reject|abandon|cancel --reason <r>
//! ws note <id> <text>
//! ws worktree gc
//! ws journal verify | head
//! ws doctor
//! ws rebuild [--check]                (daemon stopped)
//!
//! Coordination (docs/work-supervision/coordination-v0.md):
//! ws mission depend <id> --on <id>
//! ws mission scope <id> <path>…
//! ws mission check <id> <criterion> -- <program> [<argument>…]
//! ws check <id>                       (runs the declared checks)
//! ws report <id> [--markdown]
//! ws idea add <text> [--session <id>]
//! ws idea qualify <id> [--repository <r>] [--mission <id>] [--context <c>] [--session <id>]
//! ws idea promote <id> --title <t> [--repository <r>] [--brief <file>] [--criterion <c>]…
//! ws idea dismiss <id> --reason <r>
//! ws idea list
//! ws request open --question <q> --option <reversibility>:<label>:<consequence>…
//!                 [--mission <id>] [--recommended <n>] [--session <id>]
//! ws request answer <id> <choice> --reason <r>
//! ws request withdraw <id> --reason <r> [--session <id>]
//! ws request list [--open]
//! ws session register --harness <h> --label <l> [--repository <r>] [--mission <id>]
//! ws session report <id> <state> [--note <n>]
//! ws session end <id> <outcome> [--summary <s>]
//! ws session list
//! ws hook <harness> [<payload>]       (payload on standard input, or as the last argument)
//!
//! Phases (docs/work-supervision/phases-v0.md):
//! ws workflow <id> <phase>…
//! ws artifact submit <id> <phase> <file> [--session <id>]
//! ws artifact approve <artifact> --digest <sha256> [--reason <r>]
//! ws artifact return <artifact> --reason <r>
//! ws artifact list <id>
//! ws artifact show <artifact>
//! ```
//!
//! Results are JSON on standard output. A refusal prints `ws: <code>` on
//! standard error and exits 1; a usage error exits 2. `ws journal verify`
//! exits like `ws-journal-verify`: 0 valid, 1 invalid, 2 unreadable, 3 torn tail.
//! `ws hook` writes nothing on standard output and always exits 0: a harness
//! must never be stopped, nor given context, by its supervision.

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

/// Whether the command is `ws [--root <dir>] hook …`, recognised before any
/// validation: a hook must exit 0 whatever is wrong (Claude Code reads exit
/// code 2 as "block the prompt or the stop").
fn is_hook(arguments: &[OsString]) -> bool {
    match arguments {
        [first, ..] if first == "hook" => true,
        [flag, _, third, ..] if flag == "--root" => third == "hook",
        _ => false,
    }
}

fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    if is_hook(&arguments) {
        match run(arguments) {
            Ok(_) | Err(Exit::Code(_)) => {}
            Err(Exit::Usage) => eprintln!("ws: hook usage"),
            Err(Exit::Refused(code)) => eprintln!("ws: hook {code}"),
        }
        return ExitCode::SUCCESS;
    }
    match run(arguments) {
        Ok(Value::Null) => ExitCode::SUCCESS,
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
        ["attach", id] => {
            work_supervision_cli::tui::run(&root, Some((*id).to_owned())).map_err(Exit::Refused)?;
            Ok(Value::Null)
        }
        ["journal", "verify"] => journal(&layout, false),
        ["journal", "head"] => journal(&layout, true),
        ["rebuild"] => rebuild(&layout, false),
        ["rebuild", "--check"] => rebuild(&layout, true),
        ["mission", "new", options @ ..] => mission_new(&layout, options),
        ["hook", harness, rest @ ..] => {
            hook(&layout, harness, rest);
            Ok(Value::Null)
        }
        ["report", id] => request(&layout, &json!({ "op": "mission.report", "mission": id })),
        ["report", id, "--markdown"] => {
            let report = request(&layout, &json!({ "op": "mission.report", "mission": id }))?;
            print!("{}", work_supervision_cli::report::render(&report));
            Ok(Value::Null)
        }
        ["mission", "scope", id, paths @ ..] if !paths.is_empty() => request(
            &layout,
            &json!({ "op": "mission.scope", "mission": id, "paths": paths }),
        ),
        ["mission", "check", id, criterion, "--", argv @ ..] if !argv.is_empty() => {
            let criterion: u64 = criterion.parse().map_err(|_| Exit::Usage)?;
            request(
                &layout,
                &json!({ "op": "mission.check", "mission": id, "criterion": criterion, "argv": argv }),
            )
        }
        ["idea", "add", idea, options @ ..] => {
            let options = Options::parse(options, &["--session"], &[])?;
            request(
                &layout,
                &with_actor(json!({ "op": "idea.capture", "text": idea }), &options),
            )
        }
        ["idea", "qualify", id, options @ ..] => {
            let options = Options::parse(
                options,
                &["--repository", "--mission", "--context", "--session"],
                &[],
            )?;
            request(
                &layout,
                &with_actor(
                    json!({
                        "op": "idea.qualify", "idea": id,
                        "repository": options.optional("--repository"),
                        "mission": options.optional("--mission"),
                        "context": options.optional("--context"),
                    }),
                    &options,
                ),
            )
        }
        ["idea", "promote", id, options @ ..] => {
            let options = Options::parse(
                options,
                &["--title", "--repository", "--brief"],
                &["--criterion"],
            )?;
            let brief = options
                .optional("--brief")
                .map(|path| fs::read_to_string(path).map_err(|_| refused("brief.unreadable")))
                .transpose()?;
            request(
                &layout,
                &json!({
                    "op": "idea.promote", "idea": id, "title": options.one("--title")?,
                    "repository": options.optional("--repository"), "brief": brief,
                    "criteria": options.all("--criterion"),
                }),
            )
        }
        ["workflow", id, phases @ ..] if !phases.is_empty() => request(
            &layout,
            &json!({ "op": "mission.workflow", "mission": id, "phases": phases }),
        ),
        ["artifact", "submit", id, phase, file, options @ ..] => {
            let options = Options::parse(options, &["--session"], &[])?;
            let content = fs::read_to_string(file).map_err(|_| refused("artifact.unreadable"))?;
            request(
                &layout,
                &with_actor(
                    json!({ "op": "artifact.submit", "mission": id, "phase": phase, "content": content }),
                    &options,
                ),
            )
        }
        ["artifact", "approve", artifact, options @ ..] => {
            let options = Options::parse(options, &["--digest", "--reason"], &[])?;
            request(
                &layout,
                &json!({
                    "op": "artifact.approve", "artifact": artifact,
                    "digest": options.one("--digest")?, "reason": options.optional("--reason"),
                }),
            )
        }
        ["request", "open", options @ ..] => request_open(&layout, options),
        ["request", "withdraw", id, options @ ..] => {
            let options = Options::parse(options, &["--reason", "--session"], &[])?;
            request(
                &layout,
                &with_actor(
                    json!({ "op": "request.withdraw", "request": id, "reason": options.one("--reason")? }),
                    &options,
                ),
            )
        }
        ["session", "register", options @ ..] => {
            let options = Options::parse(
                options,
                &["--harness", "--label", "--repository", "--mission"],
                &[],
            )?;
            request(
                &layout,
                &json!({
                    "op": "session.register", "harness": options.one("--harness")?,
                    "label": options.one("--label")?,
                    "repository": options.optional("--repository"),
                    "mission": options.optional("--mission"),
                }),
            )
        }
        ["session", "report", id, state, options @ ..] => {
            let options = Options::parse(options, &["--note"], &[])?;
            request(
                &layout,
                &json!({
                    "op": "session.report", "actor": format!("session:{id}"), "session": id,
                    "state": state, "note": options.optional("--note"),
                }),
            )
        }
        ["session", "end", id, outcome, options @ ..] => {
            let options = Options::parse(options, &["--summary"], &[])?;
            request(
                &layout,
                &json!({
                    "op": "session.end", "actor": format!("session:{id}"), "session": id,
                    "outcome": outcome, "summary": options.optional("--summary"),
                }),
            )
        }
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
        ["mission", "depend", id, "--on", on] => {
            json!({ "op": "mission.depend", "mission": id, "on": on })
        }
        ["check", id] => json!({ "op": "check.run", "mission": id }),
        ["idea", "dismiss", id, "--reason", reason] => {
            json!({ "op": "idea.dismiss", "idea": id, "reason": reason })
        }
        ["idea", "list"] => json!({ "op": "idea.list" }),
        ["request", "answer", id, choice, "--reason", reason] => {
            let choice: u64 = choice.parse().map_err(|_| Exit::Usage)?;
            json!({ "op": "request.answer", "request": id, "choice": choice, "reason": reason })
        }
        ["request", "list"] => json!({ "op": "request.list" }),
        ["request", "list", "--open"] => json!({ "op": "request.list", "open_only": true }),
        ["session", "list"] => json!({ "op": "session.list" }),
        ["artifact", "return", artifact, "--reason", reason] => {
            json!({ "op": "artifact.return", "artifact": artifact, "reason": reason })
        }
        ["artifact", "list", id] => json!({ "op": "artifact.list", "mission": id }),
        ["artifact", "show", artifact] => json!({ "op": "artifact.show", "artifact": artifact }),
        _ => return Err(Exit::Usage),
    })
}

/// Adds `actor: session:<id>` when `--session` is given.
fn with_actor(mut request: Value, options: &Options<'_>) -> Value {
    if let (Some(session), Value::Object(map)) = (options.optional("--session"), &mut request) {
        map.insert("actor".to_owned(), json!(format!("session:{session}")));
    }
    request
}

fn request_open(layout: &Layout, words: &[&str]) -> Result<Value, Exit> {
    let options = Options::parse(
        words,
        &["--question", "--mission", "--recommended", "--session"],
        &["--option"],
    )?;
    let choices = options
        .all("--option")
        .into_iter()
        .map(|option| {
            let mut parts = option.splitn(3, ':');
            match (parts.next(), parts.next(), parts.next()) {
                (Some(reversibility), Some(label), Some(consequence)) => Ok(json!({
                    "reversibility": reversibility, "label": label, "consequence": consequence,
                })),
                _ => Err(Exit::Usage),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let recommended = options
        .optional("--recommended")
        .map(|value| value.parse::<u64>().map_err(|_| Exit::Usage))
        .transpose()?;
    request(
        layout,
        &with_actor(
            json!({
                "op": "request.open", "question": options.one("--question")?,
                "mission": options.optional("--mission"), "options": choices,
                "recommended": recommended,
            }),
            &options,
        ),
    )
}

/// `ws hook`: never fails the harness. Refusals go to standard error only.
fn hook(layout: &Layout, harness: &str, rest: &[&str]) {
    if let Err(code) = hook_inner(layout, harness, rest) {
        eprintln!("ws: hook {code}");
    }
}

fn hook_inner(layout: &Layout, harness: &str, rest: &[&str]) -> Result<(), String> {
    use std::io::Read as _;

    let text = match rest {
        [payload] => (*payload).to_owned(),
        [] => {
            let mut text = String::new();
            std::io::stdin()
                .take(1 << 20)
                .read_to_string(&mut text)
                .map_err(|_| "hook.payload_unreadable".to_owned())?;
            text
        }
        _ => return Err("hook.usage".to_owned()),
    };
    let payload: Value =
        serde_json::from_str(&text).map_err(|_| "hook.payload_invalid".to_owned())?;
    let event = work_supervision_cli::hook::translate(harness, &payload)?;
    if event.action == work_supervision_cli::hook::Action::Ignore {
        return Ok(());
    }
    let digest = {
        use sha2::Digest as _;
        let bytes: [u8; 32] = sha2::Sha256::digest(event.external_id.as_bytes()).into();
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let call = |body: &Value| {
        request(layout, body).map_err(|exit| match exit {
            Exit::Refused(code) => code,
            _ => "hook.request".to_owned(),
        })
    };
    let found =
        call(&json!({ "op": "session.find", "harness": harness, "external_digest": digest }))?;
    let session = match found.get("session").and_then(Value::as_str) {
        Some(session) => session.to_owned(),
        None if event.action == work_supervision_cli::hook::Action::End => return Ok(()),
        None => {
            let (repository, mission) = context_of(layout, event.cwd.as_deref());
            let registered = call(&json!({
                "op": "session.register", "harness": harness,
                "label": format!("{harness} session"), "repository": repository,
                "mission": mission, "external_digest": digest,
            }))?;
            registered
                .get("session")
                .and_then(Value::as_str)
                .ok_or("hook.request")?
                .to_owned()
        }
    };
    let actor = format!("session:{session}");
    match event.action {
        work_supervision_cli::hook::Action::Report { state, note } => call(&json!({
            "op": "session.report", "actor": actor, "session": session, "state": state, "note": note,
        }))?,
        work_supervision_cli::hook::Action::End => call(&json!({
            "op": "session.end", "actor": actor, "session": session, "outcome": "completed",
        }))?,
        work_supervision_cli::hook::Action::Ignore => Value::Null,
    };
    Ok(())
}

/// Repository (by configured name) and mission (by worktree) a working
/// directory belongs to, when it can be told.
fn context_of(layout: &Layout, cwd: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(cwd) = cwd else {
        return (None, None);
    };
    // Compare resolved paths: on macOS `/var/…` and `/private/var/…` name the
    // same directory, and a harness may report either.
    let resolve = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let cwd_path = resolve(Path::new(cwd));
    let cwd = cwd_path.to_string_lossy().into_owned();
    let cwd = cwd.as_str();
    let mission = cwd_path
        .strip_prefix(resolve(&layout.worktrees()))
        .ok()
        .and_then(|rest| rest.components().next())
        .and_then(|first| first.as_os_str().to_str())
        .filter(|name| {
            name.len() == 32
                && name
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
        .map(str::to_owned);
    let repository = Config::load(&layout.config()).ok().and_then(|config| {
        let repositories: Vec<(String, PathBuf)> = config
            .repositories()
            .into_iter()
            .map(|(name, path)| (name, resolve(&path)))
            .collect();
        work_supervision_cli::hook::repository_of(cwd, &repositories)
    });
    (repository, mission)
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
