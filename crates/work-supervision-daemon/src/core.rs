//! The daemon's state and the operations requests and runs perform on it.

use std::collections::HashMap;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use work_supervision_domain::{Budgets, Command, Digest32, Mission, MissionId, RunId, State};
use work_supervision_journal::{Event, OpenMode};
use work_supervision_pty::{
    InputWriter, Observation, RunBudgets, Session, SpawnSpec, executor_program,
};
use work_supervision_store::{BlobStore, Layout, Supervisor};
use work_supervision_worktree::{Git, Release, Worktrees};

use crate::{Config, Failure, clock, fault};

/// Default budgets of a mission created without explicit ones.
const DEFAULT_DURATION_SECONDS: u64 = 600;
const DEFAULT_OUTPUT_BYTES: u64 = 10 << 20;
/// Longest a `wait` request may block.
const MAX_WAIT: Duration = Duration::from_secs(60);

struct RunHandle {
    input: InputWriter,
    thread: Option<JoinHandle<()>>,
}

/// Everything the daemon holds, behind one lock.
pub(crate) struct Core {
    pub(crate) layout: Layout,
    pub(crate) config: Config,
    pub(crate) supervisor: Supervisor,
    pub(crate) worktrees: Worktrees,
    runs: HashMap<String, RunHandle>,
    /// Missions whose checks are running, with the thread running them.
    pub(crate) checks: HashMap<String, Option<JoinHandle<()>>>,
    anchored: u64,
}

/// The shared daemon state.
pub(crate) type Shared = Arc<Mutex<Core>>;

/// Counts of what startup recovery did.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Recovered {
    pub(crate) journal_entries: u64,
    pub(crate) confirmed: u64,
    pub(crate) aborted: u64,
    pub(crate) removals: u64,
    pub(crate) interrupted_runs: u64,
    pub(crate) exited_missions: u64,
    pub(crate) provisioned_missions: u64,
    pub(crate) released_worktrees: u64,
    pub(crate) checks_interrupted: u64,
    pub(crate) promotions_confirmed: u64,
    pub(crate) promotions_aborted: u64,
    pub(crate) scope_checks: u64,
    pub(crate) scope_check_failures: u64,
}

pub(crate) fn lock(shared: &Shared) -> Result<MutexGuard<'_, Core>, Failure> {
    shared.lock().map_err(|_| Failure::new("daemon.poisoned"))
}

pub(crate) fn random_bytes() -> Result<[u8; 16], Failure> {
    let mut bytes = [0_u8; 16];
    fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|_| Failure::new("daemon.random"))?;
    Ok(bytes)
}

pub(crate) fn field<'a>(request: &'a Value, key: &str) -> Result<&'a str, Failure> {
    request
        .get(key)
        .and_then(Value::as_str)
        .ok_or(Failure::new("request.field_invalid"))
}

pub(crate) fn mission_field(request: &Value) -> Result<MissionId, Failure> {
    MissionId::parse(field(request, "mission")?).map_err(|_| Failure::new("request.field_invalid"))
}

pub(crate) fn optional_u64(request: &Value, key: &str, default: u64) -> Result<u64, Failure> {
    match request.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value.as_u64().ok_or(Failure::new("request.field_invalid")),
    }
}

impl Core {
    /// Opens the root, verifies and recovers it (plan §2.7), ready to serve.
    pub(crate) fn open(layout: Layout, config: Config) -> Result<(Shared, Recovered), Failure> {
        let mut recovered = Recovered::default();
        // (1) Full verification by the independent verifier, outside the projection.
        match fs::File::open(layout.journal()) {
            Ok(file) => match work_supervision_journal_verifier::verify(file) {
                Ok(work_supervision_journal_verifier::Outcome::Valid { entries, .. }) => {
                    recovered.journal_entries = entries;
                }
                // (2) A torn tail is quarantined explicitly when the writer opens below.
                Ok(work_supervision_journal_verifier::Outcome::TornTail { verified, .. }) => {
                    recovered.journal_entries = verified;
                }
                Ok(_) => return Err(Failure::new("journal.invalid")),
                Err(_) => return Err(Failure::new("journal.unreadable")),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Failure::new("journal.unreadable")),
        }
        // (1b) The head anchored outside the root must still be in the journal:
        // the chain alone cannot see a complete, consistent rewrite.
        if let Some(path) = config.anchor() {
            match crate::Anchor::read(path)? {
                Some(anchor) => crate::check_anchor(&layout.journal(), &anchor)?,
                None if recovered.journal_entries > 0 => {
                    return Err(Failure::new("journal.anchor_missing"));
                }
                None => {}
            }
        }
        // (3) Writer lock, torn-tail quarantine, projection caught up.
        let supervisor =
            Supervisor::open(&layout, OpenMode::RecoverTornTail { at: clock::now()? })?;
        let worktrees = Worktrees::new(layout.clone(), Git::system());
        let mut core = Self {
            layout,
            config,
            supervisor,
            worktrees,
            runs: HashMap::new(),
            checks: HashMap::new(),
            anchored: 0,
        };
        core.recover(&mut recovered)?;
        core.anchor()?;
        Ok((Arc::new(Mutex::new(core)), recovered))
    }

    /// Steps (4) and (5) of the recovery order, plus the mission steps a crash
    /// between two journal entries can leave undone.
    fn recover(&mut self, recovered: &mut Recovered) -> Result<(), Failure> {
        let config = self.config.clone();
        let resolver = |name: &str| config.repository(name).ok().map(std::path::Path::to_owned);
        let report = self
            .worktrees
            .reconcile(&mut self.supervisor, &resolver, clock::now()?)?;
        recovered.confirmed += report.confirmed();
        recovered.aborted += report.aborted();
        recovered.removals += report.finished_removals();
        // Runs left running: their PTY died with the previous daemon.
        for row in self.supervisor.store().running_runs()? {
            if self.runs.contains_key(&row.mission) {
                continue;
            }
            self.supervisor.append(
                clock::now()?,
                event(
                    "run.interrupted",
                    &[("mission", &row.mission), ("run", &row.run)],
                )?,
            )?;
            recovered.interrupted_runs += 1;
        }
        for mission in self.supervisor.missions()? {
            if self.runs.contains_key(mission.id().as_str()) {
                continue;
            }
            match mission.state() {
                State::Running | State::WaitingInput => {
                    let exited = mission.current_run().map_or(Ok(None), |run| {
                        self.supervisor
                            .store()
                            .runs_of(mission.id().as_str())
                            .map(|runs| runs.into_iter().find(|row| row.run == run.as_str()))
                    })?;
                    let interrupted = exited.is_none_or(|row| row.state != "exited");
                    self.execute(mission.id(), &Command::ExitRun { interrupted })?;
                    recovered.exited_missions += 1;
                }
                State::Ready => {
                    let row = self.supervisor.store().worktree(mission.id().as_str())?;
                    if let Some(row) = row.filter(|row| row.state == "created") {
                        let base = work_supervision_domain::CommitId::parse(&row.base_commit)
                            .map_err(|_| Failure::new("projection.sqlite"))?;
                        self.execute(mission.id(), &Command::Provision { base_commit: base })?;
                        recovered.provisioned_missions += 1;
                    }
                }
                state if state.is_terminal() => {
                    let row = self.supervisor.store().worktree(mission.id().as_str())?;
                    if row.is_some_and(|row| row.state == "created") {
                        let release = if state == State::Accepted {
                            Release::Keep
                        } else {
                            Release::Abandon
                        };
                        if release == Release::Keep && !self.worktrees.is_clean(mission.id())? {
                            continue;
                        }
                        let repository = self.config.repository(mission.repository())?.to_owned();
                        self.worktrees.release(
                            &mut self.supervisor,
                            &repository,
                            mission.id(),
                            release,
                            &mut work_supervision_worktree::Faults::none(),
                            clock::now()?,
                        )?;
                        recovered.released_worktrees += 1;
                    }
                }
                _ => {}
            }
        }
        crate::coordination::recover(self, recovered)
    }

    /// Records the journal head in the anchor file when it moved.
    pub(crate) fn anchor(&mut self) -> Result<(), Failure> {
        let Some(path) = self.config.anchor().map(std::path::Path::to_owned) else {
            return Ok(());
        };
        let Some(head) = self.supervisor.journal().head().copied() else {
            return Ok(());
        };
        if head.seq() == self.anchored {
            return Ok(());
        }
        crate::Anchor::new(head.seq(), &head.digest().to_hex())?.write(&path)?;
        self.anchored = head.seq();
        Ok(())
    }

    pub(crate) fn mission(&self, id: &MissionId) -> Result<Mission, Failure> {
        self.supervisor
            .mission(id)?
            .ok_or(Failure::new("mission.not_found"))
    }

    pub(crate) fn execute(
        &mut self,
        id: &MissionId,
        command: &Command,
    ) -> Result<Mission, Failure> {
        let revision = self
            .supervisor
            .mission(id)?
            .map_or(0, |mission| mission.revision());
        Ok(self
            .supervisor
            .execute(id, command, revision, clock::now()?)?)
    }

    pub(crate) fn worktree_state(&self, id: &MissionId) -> Result<Option<String>, Failure> {
        Ok(self
            .supervisor
            .store()
            .worktree(id.as_str())?
            .map(|row| row.state))
    }

    fn release(&mut self, mission: &Mission, release: Release) -> Result<(), Failure> {
        if self.worktree_state(mission.id())?.as_deref() != Some("created") {
            return Ok(());
        }
        let repository = self.config.repository(mission.repository())?.to_owned();
        let mut faults = fault::worktree_faults();
        self.worktrees
            .release(
                &mut self.supervisor,
                &repository,
                mission.id(),
                release,
                &mut faults,
                clock::now()?,
            )
            .map_err(fault::crash_on_worktree_fault)?;
        Ok(())
    }
}

fn event(kind: &str, fields: &[(&str, &str)]) -> Result<Event, Failure> {
    let data: Map<String, Value> = fields
        .iter()
        .map(|(key, value)| ((*key).to_owned(), Value::String((*value).to_owned())))
        .collect();
    Event::new(kind, data).map_err(|_| Failure::new("journal.invalid"))
}

pub(crate) fn event_with(kind: &str, data: Value) -> Result<Event, Failure> {
    let Value::Object(map) = data else {
        return Err(Failure::new("journal.invalid"));
    };
    Event::new(kind, map).map_err(|_| Failure::new("journal.invalid"))
}

/// Executes one request.
pub(crate) fn handle(shared: &Shared, request: &Value) -> Result<Value, Failure> {
    let op = request
        .get("op")
        .and_then(Value::as_str)
        .ok_or(Failure::new("request.malformed"))?;
    crate::coordination::authorize(op, request)?;
    match op {
        "status" => {
            let core = lock(shared)?;
            Ok(json!({
                "missions": core.supervisor.missions()?.len(),
                "active_runs": core.runs.len(),
                "journal_entries": core.supervisor.journal_entries(),
            }))
        }
        "mission.new" => mission_new(shared, request),
        "mission.list" => mission_list(shared),
        "mission.show" => {
            let id = mission_field(request)?;
            let core = lock(shared)?;
            let mission = core.mission(&id)?;
            show(&core, &mission)
        }
        "mission.ready" => {
            let id = mission_field(request)?;
            let mut core = lock(shared)?;
            let mission = core.execute(&id, &Command::Ready)?;
            fault::hit("mission-readied");
            Ok(json!({ "state": mission.state().as_str() }))
        }
        "run" => run(shared, &mission_field(request)?),
        "send" => send(shared, &mission_field(request)?, field(request, "text")?),
        "result.submit" => submit(shared, request),
        "decide" => decide(shared, request),
        "note" => {
            let id = mission_field(request)?;
            let text = field(request, "text")?.to_owned();
            lock(shared)?.execute(&id, &Command::Note { text })?;
            Ok(json!({}))
        }
        "wait" => wait(shared, request),
        "worktree.gc" => {
            let core = lock(shared)?;
            let report = core
                .worktrees
                .gc(&core.supervisor, &core.config.repositories())?;
            Ok(
                json!({ "removed_worktrees": report.removed_worktrees(), "removed_branches": report.removed_branches() }),
            )
        }
        "doctor" => {
            let mut core = lock(shared)?;
            let mut recovered = Recovered {
                journal_entries: core.supervisor.journal_entries(),
                ..Recovered::default()
            };
            core.recover(&mut recovered)?;
            Ok(report_json(&recovered))
        }
        op if crate::coordination::handles(op) => crate::coordination::handle(shared, op, request),
        _ => Err(Failure::new("request.unknown_op")),
    }
}

pub(crate) fn report_json(recovered: &Recovered) -> Value {
    json!({
        "journal_entries": recovered.journal_entries,
        "worktrees_confirmed": recovered.confirmed,
        "worktrees_aborted": recovered.aborted,
        "worktree_removals_finished": recovered.removals,
        "runs_interrupted": recovered.interrupted_runs,
        "missions_exited": recovered.exited_missions,
        "missions_provisioned": recovered.provisioned_missions,
        "worktrees_released": recovered.released_worktrees,
        "checks_interrupted": recovered.checks_interrupted,
        "promotions_confirmed": recovered.promotions_confirmed,
        "promotions_aborted": recovered.promotions_aborted,
        "scope_checks_recorded": recovered.scope_checks,
        "scope_checks_failed": recovered.scope_check_failures,
    })
}

fn mission_new(shared: &Shared, request: &Value) -> Result<Value, Failure> {
    let repository = field(request, "repository")?.to_owned();
    let criteria = match request.get("criteria") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or(Failure::new("request.field_invalid"))?,
        Some(_) => return Err(Failure::new("request.field_invalid")),
    };
    let budgets = Budgets::new(
        optional_u64(request, "max_duration_seconds", DEFAULT_DURATION_SECONDS)?,
        optional_u64(request, "max_output_bytes", DEFAULT_OUTPUT_BYTES)?,
    )?;
    let mut core = lock(shared)?;
    core.config.repository(&repository)?;
    let id = MissionId::from_bytes(random_bytes()?);
    let command = Command::Create {
        title: field(request, "title")?.to_owned(),
        repository,
        brief: field(request, "brief")?.to_owned(),
        criteria,
        budgets,
        executor: core.config.executor(),
    };
    core.execute(&id, &command)?;
    fault::hit("mission-created");
    Ok(json!({ "mission": id.as_str() }))
}

fn mission_list(shared: &Shared) -> Result<Value, Failure> {
    let core = lock(shared)?;
    let mut list = Vec::new();
    for mission in core.supervisor.missions()? {
        list.push(json!({
            "id": mission.id().as_str(),
            "title": mission.title(),
            "state": mission.state().as_str(),
            "revision": mission.revision(),
            "worktree": core.worktree_state(mission.id())?,
            "running": core.runs.contains_key(mission.id().as_str()),
            "simulation": is_simulation(&mission),
            "blockers": crate::coordination::blocker_codes(&core, &mission)?,
        }));
    }
    Ok(Value::Array(list))
}

fn show(core: &Core, mission: &Mission) -> Result<Value, Failure> {
    let mut shown = show_core(core, mission)?;
    if let (Value::Object(shown), Value::Object(extras)) = (
        &mut shown,
        crate::coordination::mission_extras(core, mission)?,
    ) {
        shown.extend(extras);
    }
    Ok(shown)
}

fn show_core(core: &Core, mission: &Mission) -> Result<Value, Failure> {
    let worktree = core.supervisor.store().worktree(mission.id().as_str())?;
    let runs = core.supervisor.store().runs_of(mission.id().as_str())?;
    Ok(json!({
        "id": mission.id().as_str(),
        "title": mission.title(),
        "repository": mission.repository(),
        "brief": mission.brief(),
        "criteria": mission.criteria(),
        "state": mission.state().as_str(),
        "revision": mission.revision(),
        "budgets": {
            "max_duration_seconds": mission.budgets().max_duration_seconds(),
            "max_output_bytes": mission.budgets().max_output_bytes(),
        },
        "executor": mission.executor().as_str(),
        "simulation": is_simulation(mission),
        "base_commit": mission.base_commit().map(work_supervision_domain::CommitId::as_str),
        "current_run": mission.current_run().map(RunId::as_str),
        "result": mission.result().map(|result| json!({
            "commit": result.commit().as_str(),
            "evidence_digest": result.evidence().to_hex(),
            "summary": result.summary(),
        })),
        "verdict": mission.verdict().map(|verdict| json!({
            "state": verdict.state().as_str(),
            "reason": verdict.reason(),
        })),
        "worktree": worktree.map(|row| json!({
            "state": row.state,
            "branch": row.branch,
            "head": row.head,
            "archive_digest": row.archive_digest,
        })),
        "runs": runs.iter().map(|row| json!({
            "run": row.run,
            "state": row.state,
            "output_bytes": row.output_bytes,
            "output_digest": row.output_digest,
            "inputs": row.inputs,
            "exit_code": row.exit_code,
            "signal": row.signal,
            "budget": row.budget,
        })).collect::<Vec<_>>(),
        "active": core.runs.contains_key(mission.id().as_str()),
    }))
}

fn run(shared: &Shared, id: &MissionId) -> Result<Value, Failure> {
    let mut core = lock(shared)?;
    if core.runs.contains_key(id.as_str()) {
        return Err(Failure::new("run.already_active"));
    }
    let mission = core.mission(id)?;
    crate::coordination::guard_run(&core, &mission)?;
    if mission.state() == State::Ready {
        if core.worktree_state(id)?.as_deref() != Some("created") {
            let repository = core.config.repository(mission.repository())?.to_owned();
            let base = core.worktrees.head_of(&repository)?;
            let mut faults = fault::worktree_faults();
            let Core {
                worktrees,
                supervisor,
                ..
            } = &mut *core;
            worktrees
                .provision(
                    supervisor,
                    &repository,
                    id,
                    mission.repository(),
                    &base,
                    &mut faults,
                    clock::now()?,
                )
                .map_err(fault::crash_on_worktree_fault)?;
            fault::hit("worktree-confirmed");
        }
        let row = core
            .supervisor
            .store()
            .worktree(id.as_str())?
            .ok_or(Failure::new("worktree.not_provisioned"))?;
        let base = work_supervision_domain::CommitId::parse(&row.base_commit)
            .map_err(|_| Failure::new("projection.sqlite"))?;
        core.execute(id, &Command::Provision { base_commit: base })?;
        fault::hit("mission-provisioned");
    }
    let run = RunId::from_bytes(random_bytes()?);
    let mission = core.execute(id, &Command::StartRun { run: run.clone() })?;
    match start_session(&mut core, &mission, &run) {
        Ok(session) => {
            fault::hit("run-started");
            let input = session.input();
            let thread_shared = Arc::clone(shared);
            let mission_id = id.clone();
            let run_id = run.clone();
            let thread = std::thread::spawn(move || {
                supervise(&thread_shared, &mission_id, &run_id, session)
            });
            core.runs.insert(
                id.as_str().to_owned(),
                RunHandle {
                    input,
                    thread: Some(thread),
                },
            );
            Ok(json!({ "run": run.as_str() }))
        }
        Err(failure) => {
            // The run never started: the mission leaves `running` as interrupted.
            core.execute(id, &Command::ExitRun { interrupted: true })?;
            Err(failure)
        }
    }
}

fn start_session(core: &mut Core, mission: &Mission, run: &RunId) -> Result<Session, Failure> {
    let directory = core.layout.runs().join(run.as_str());
    let home = directory.join("home");
    let io = |_| Failure::new("run.io");
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&home)
        .map_err(io)?;
    let brief = directory.join("brief");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&brief)
        .map_err(io)?;
    file.write_all(mission.brief().as_bytes()).map_err(io)?;
    file.sync_all().map_err(io)?;
    let program = executor_program(mission.executor(), core.config.fake_agent()?);
    let cwd = core.worktrees.path_of(mission.id().as_str());
    let argv = [program.as_os_str(), brief.as_os_str()];
    let mut hasher = Sha256::new();
    for argument in argv {
        hasher.update(argument.as_encoded_bytes());
        hasher.update([0]);
    }
    let argv_digest = Digest32::from_bytes(hasher.finalize().into()).to_hex();
    let budgets = mission.budgets();
    let spec = SpawnSpec::new(
        program.clone(),
        vec![brief.into_os_string()],
        cwd,
        home,
        directory.join("pty.log"),
        RunBudgets::new(
            Duration::from_secs(budgets.max_duration_seconds()),
            budgets.max_output_bytes(),
            Duration::from_millis(core.config.grace_ms()),
        ),
    )
    .with_path(core.config.path().into())
    .with_idle_after(Duration::from_millis(core.config.idle_after_ms()));
    let session = Session::spawn(spec)?;
    core.supervisor.append(
        clock::now()?,
        event_with(
            "run.started",
            json!({
                "mission": mission.id().as_str(),
                "run": run.as_str(),
                "argv_digest": argv_digest,
                "cwd": work_supervision_domain::worktree_of(mission.id()),
            }),
        )?,
    )?;
    Ok(session)
}

/// Supervises one run to its end on its own thread, journalling what it observes.
fn supervise(shared: &Shared, mission: &MissionId, run: &RunId, session: Session) {
    let outcome = session.wait_with(|observation| {
        if let Err(failure) = observe(shared, mission, run, &observation) {
            eprintln!("wsd: run observation refused: {failure}");
        }
        anchor(shared);
    });
    let finished = (|| -> Result<(), Failure> {
        let mut core = lock(shared)?;
        core.runs.remove(mission.as_str());
        let exit = outcome?;
        core.supervisor.append(
            clock::now()?,
            event_with(
                "run.exited",
                json!({
                    "mission": mission.as_str(),
                    "run": run.as_str(),
                    "bytes": exit.output_bytes(),
                    "digest": exit.output_digest(),
                    "exit_code": exit.exit_code(),
                    "signal": exit.signal(),
                    "budget": exit.budget_exceeded().map(work_supervision_pty::Budget::as_str),
                    "escalated": exit.escalated_to_kill(),
                }),
            )?,
        )?;
        fault::hit("run-exited");
        core.execute(mission, &Command::ExitRun { interrupted: false })?;
        fault::hit("mission-exited");
        Ok(())
    })();
    if let Err(failure) = finished {
        eprintln!("wsd: run end refused: {failure}");
    }
    anchor(shared);
}

fn observe(
    shared: &Shared,
    mission: &MissionId,
    run: &RunId,
    observation: &Observation,
) -> Result<(), Failure> {
    let mut core = lock(shared)?;
    match observation {
        Observation::Checkpoint { bytes, digest } => {
            core.supervisor.append(
                clock::now()?,
                event_with(
                    "run.output.checkpoint",
                    json!({ "mission": mission.as_str(), "run": run.as_str(), "bytes": bytes, "digest": digest }),
                )?,
            )?;
        }
        Observation::Idle => {
            if core.mission(mission)?.state() == State::Running {
                core.execute(mission, &Command::AwaitInput)?;
            }
        }
        Observation::Active => {
            if core.mission(mission)?.state() == State::WaitingInput {
                core.execute(mission, &Command::ResumeInput)?;
            }
        }
        Observation::BudgetExceeded(_) => {}
    }
    Ok(())
}

fn send(shared: &Shared, id: &MissionId, text: &str) -> Result<Value, Failure> {
    let mut core = lock(shared)?;
    let mission = core.mission(id)?;
    let handle = core
        .runs
        .get(id.as_str())
        .ok_or(Failure::new("run.not_active"))?;
    let record = handle.input.write(text.as_bytes())?;
    let run = mission
        .current_run()
        .ok_or(Failure::new("run.not_active"))?
        .clone();
    core.supervisor.append(
        clock::now()?,
        event_with(
            "run.input",
            json!({ "mission": id.as_str(), "run": run.as_str(), "bytes": record.bytes(), "digest": record.digest() }),
        )?,
    )?;
    if mission.state() == State::WaitingInput {
        core.execute(id, &Command::ResumeInput)?;
    }
    Ok(json!({ "bytes": record.bytes(), "digest": record.digest() }))
}

fn submit(shared: &Shared, request: &Value) -> Result<Value, Failure> {
    let id = mission_field(request)?;
    let evidence = Digest32::parse(field(request, "evidence_digest")?)
        .map_err(|_| Failure::new("request.field_invalid"))?;
    let summary = field(request, "summary")?.to_owned();
    let mut core = lock(shared)?;
    let store = BlobStore::open(&core.layout.evidence())?;
    let named = work_supervision_journal::Digest::from_hex(&evidence.to_hex())
        .ok_or(Failure::new("request.field_invalid"))?;
    store
        .get(&named)
        .map_err(|_| Failure::new("evidence.missing"))?;
    let commit = if core.worktree_state(&id)?.as_deref() == Some("created") {
        Some(
            core.worktrees
                .head_of(&core.worktrees.path_of(id.as_str()))?,
        )
    } else {
        None
    };
    core.execute(
        &id,
        &Command::SubmitResult {
            commit,
            evidence: Some(evidence),
            summary,
        },
    )?;
    fault::hit("result-submitted");
    // The result is journalled: a failing scope check is reported, not
    // returned as a refusal of a submission that took place.
    match crate::coordination::record_scope_check(&mut core, &id) {
        Ok(outside) => Ok(json!({ "scope_outside": outside })),
        Err(failure) => Ok(json!({ "scope_outside": null, "scope_check_failed": failure.code() })),
    }
}

fn decide(shared: &Shared, request: &Value) -> Result<Value, Failure> {
    let id = mission_field(request)?;
    let reason = field(request, "reason")?.to_owned();
    // A decision while checks run would race their worktree: the test is made
    // under the very lock that executes the decision, never before it.
    let idle = |core: &Core| {
        if core.checks.contains_key(id.as_str()) {
            Err(Failure::new("check.running"))
        } else {
            Ok(())
        }
    };
    match field(request, "decision")? {
        "accept" => {
            let mut core = lock(shared)?;
            idle(&core)?;
            let mission = core.mission(&id)?;
            crate::coordination::guard_accept(&mut core, &mission)?;
            if core.worktree_state(&id)?.as_deref() == Some("created")
                && !core.worktrees.is_clean(&id)?
            {
                return Err(Failure::new("worktree.dirty"));
            }
            let mission = core.execute(&id, &Command::Accept { reason })?;
            fault::hit("mission-accepted");
            core.release(&mission, Release::Keep)?;
            Ok(json!({ "state": mission.state().as_str() }))
        }
        "reject" => {
            let mut core = lock(shared)?;
            idle(&core)?;
            let mission = core.execute(&id, &Command::Reject { reason })?;
            Ok(json!({ "state": mission.state().as_str() }))
        }
        "abandon" => {
            let mut core = lock(shared)?;
            idle(&core)?;
            let mission = core.execute(&id, &Command::Abandon { reason })?;
            core.release(&mission, Release::Abandon)?;
            Ok(json!({ "state": mission.state().as_str() }))
        }
        "cancel" => {
            let thread = {
                let mut core = lock(shared)?;
                core.mission(&id)?;
                core.runs.get_mut(id.as_str()).and_then(|handle| {
                    handle.input.cancel();
                    handle.thread.take()
                })
            };
            if let Some(thread) = thread {
                thread
                    .join()
                    .map_err(|_| Failure::new("run.supervisor_failed"))?;
            }
            let mut core = lock(shared)?;
            idle(&core)?;
            let mission = core.execute(&id, &Command::Cancel { reason })?;
            core.release(&mission, Release::Abandon)?;
            Ok(json!({ "state": mission.state().as_str() }))
        }
        _ => Err(Failure::new("request.field_invalid")),
    }
}

fn wait(shared: &Shared, request: &Value) -> Result<Value, Failure> {
    let id = mission_field(request)?;
    let states: Vec<String> = request
        .get("states")
        .and_then(Value::as_array)
        .ok_or(Failure::new("request.field_invalid"))?
        .iter()
        .map(|state| state.as_str().map(str::to_owned))
        .collect::<Option<_>>()
        .ok_or(Failure::new("request.field_invalid"))?;
    let timeout = Duration::from_millis(optional_u64(request, "timeout_ms", 10_000)?).min(MAX_WAIT);
    let deadline = Instant::now() + timeout;
    loop {
        let state = lock(shared)?.mission(&id)?.state().as_str().to_owned();
        if states.contains(&state) {
            return Ok(json!({ "state": state }));
        }
        if Instant::now() >= deadline {
            return Err(Failure::new("wait.timeout"));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Where the socket lives.
pub(crate) fn socket_path(layout: &Layout) -> PathBuf {
    layout.root().join("run").join("wsd.sock")
}

/// Anchors the head after a request or a run event; a failure is reported by code.
pub(crate) fn anchor(shared: &Shared) {
    let outcome = lock(shared).and_then(|mut core| core.anchor());
    if let Err(failure) = outcome {
        eprintln!("wsd: anchor not written: {failure}");
    }
}

/// Whether the mission's runs are a simulation: every run of the fake agent is
/// one, and never counts toward B′ (the v0 exit criterion of the card).
pub(crate) const fn is_simulation(mission: &Mission) -> bool {
    match mission.executor() {
        work_supervision_domain::ExecutorProfile::Fake => true,
    }
}
