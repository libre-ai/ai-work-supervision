use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read as _, Write as _};
use std::os::unix::process::ExitStatusExt as _;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use rustix::process::{Pid, Signal, kill_process_group, test_kill_process_group};
use sha2::{Digest as _, Sha256};

/// Environment variables a run receives; every other variable of the host is dropped.
///
/// `SHELL` is always set by `portable-pty`; it is pinned to `/bin/sh` here so
/// that the host's password database is not consulted.
pub const ALLOWED_ENVIRONMENT: [&str; 5] = ["HOME", "LANG", "PATH", "SHELL", "TERM"];

const DEFAULT_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const READ_CHUNK: usize = 64 * 1024;
const DEFAULT_CHECKPOINT_BYTES: u64 = 1 << 20;
const DEFAULT_CHECKPOINT_INTERVAL: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(10);
/// How long the log reader may lag once the whole group is gone.
const DRAIN_LIMIT: Duration = Duration::from_secs(5);

/// Which budget a run overran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Budget {
    /// Wall-clock duration.
    Duration,
    /// Terminal output bytes.
    Output,
}

impl Budget {
    /// Name used in events.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Duration => "duration",
            Self::Output => "output",
        }
    }
}

/// Limits of one run and the delay between `SIGTERM` and `SIGKILL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunBudgets {
    max_duration: Duration,
    max_output_bytes: u64,
    grace: Duration,
}

impl RunBudgets {
    /// Budgets of a run.
    #[must_use]
    pub const fn new(max_duration: Duration, max_output_bytes: u64, grace: Duration) -> Self {
        Self {
            max_duration,
            max_output_bytes,
            grace,
        }
    }
}

/// What to start, where, and with which log and budgets.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    program: PathBuf,
    args: Vec<OsString>,
    cwd: PathBuf,
    home: PathBuf,
    log: PathBuf,
    budgets: RunBudgets,
    path: OsString,
    idle_after: Option<Duration>,
    checkpoint_bytes: u64,
    checkpoint_interval: Duration,
    size: (u16, u16),
}

impl SpawnSpec {
    /// A run of `program` with `args` in `cwd`, `HOME` set to `home`, its
    /// terminal output appended to `log` (which must not exist yet).
    #[must_use]
    pub fn new(
        program: PathBuf,
        args: Vec<OsString>,
        cwd: PathBuf,
        home: PathBuf,
        log: PathBuf,
        budgets: RunBudgets,
    ) -> Self {
        Self {
            program,
            args,
            cwd,
            home,
            log,
            budgets,
            path: OsString::from(DEFAULT_PATH),
            idle_after: None,
            checkpoint_bytes: DEFAULT_CHECKPOINT_BYTES,
            checkpoint_interval: DEFAULT_CHECKPOINT_INTERVAL,
            size: (24, 80),
        }
    }

    /// Reports [`Observation::Idle`] after `idle` without output.
    #[must_use]
    pub const fn with_idle_after(mut self, idle: Duration) -> Self {
        self.idle_after = Some(idle);
        self
    }

    /// Replaces the working directory.
    #[must_use]
    pub fn with_cwd(mut self, cwd: PathBuf) -> Self {
        self.cwd = cwd;
        self
    }

    /// Replaces the `PATH` given to the run.
    #[must_use]
    pub fn with_path(mut self, path: OsString) -> Self {
        self.path = path;
        self
    }

    /// Replaces the checkpoint thresholds (bytes and interval).
    #[must_use]
    pub const fn with_checkpoints(mut self, bytes: u64, interval: Duration) -> Self {
        self.checkpoint_bytes = bytes;
        self.checkpoint_interval = interval;
        self
    }

    /// Replaces the initial terminal size (rows, columns).
    #[must_use]
    pub const fn with_size(mut self, rows: u16, cols: u16) -> Self {
        self.size = (rows, cols);
        self
    }
}

/// Why a session could not be started or supervised. `Display` carries no path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyError {
    /// The working directory does not exist or is not a directory.
    CwdInvalid,
    /// The program is not an absolute path to a file.
    ProgramInvalid,
    /// The log could not be created (it must not exist yet) or written.
    LogIo,
    /// The terminal could not be opened or the program could not be started.
    Spawn,
    /// Waiting for, signalling or writing to the run failed.
    Io,
}

impl PtyError {
    /// Stable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::CwdInvalid => "pty.cwd_invalid",
            Self::ProgramInvalid => "pty.program_invalid",
            Self::LogIo => "pty.log_io",
            Self::Spawn => "pty.spawn",
            Self::Io => "pty.io",
        }
    }
}

impl fmt::Display for PtyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for PtyError {}

/// What a session reports while it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    /// Cumulative output so far: byte count and SHA-256 of the whole log prefix.
    Checkpoint {
        /// Bytes logged.
        bytes: u64,
        /// Lowercase hex SHA-256 of those bytes.
        digest: String,
    },
    /// No output for the idle delay: the executor is likely waiting for input.
    Idle,
    /// Output resumed after [`Observation::Idle`].
    Active,
    /// A budget was overrun; termination of the group has started.
    BudgetExceeded(Budget),
}

/// An input written to the terminal: its length and digest, never its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputRecord {
    bytes: u64,
    digest: String,
}

impl InputRecord {
    /// Bytes written.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Lowercase hex SHA-256 of the bytes written.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// Shared handle on a session's terminal: input, resize, cancellation.
#[derive(Clone)]
pub struct InputWriter {
    writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    cancel: Sender<Message>,
}

impl fmt::Debug for InputWriter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InputWriter")
    }
}

impl InputWriter {
    /// Writes `bytes` to the terminal.
    ///
    /// # Errors
    ///
    /// [`PtyError::Io`] when the terminal refuses the write.
    pub fn write(&self, bytes: &[u8]) -> Result<InputRecord, PtyError> {
        let mut writer = self.writer.lock().map_err(|_| PtyError::Io)?;
        writer
            .write_all(bytes)
            .and_then(|()| writer.flush())
            .map_err(|_| PtyError::Io)?;
        Ok(InputRecord {
            bytes: u64::try_from(bytes.len()).map_err(|_| PtyError::Io)?,
            digest: hex(&Sha256::digest(bytes)),
        })
    }

    /// Resizes the terminal.
    ///
    /// # Errors
    ///
    /// [`PtyError::Io`].
    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), PtyError> {
        let master = self.master.lock().map_err(|_| PtyError::Io)?;
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|_| PtyError::Io)
    }

    /// Asks the supervising loop to terminate the group (`SIGTERM`, then `SIGKILL`).
    pub fn cancel(&self) {
        // A closed channel means the session already ended: nothing to stop.
        let _ = self.cancel.send(Message::Cancel);
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    exit_code: Option<i32>,
    signal: Option<i32>,
    output_bytes: u64,
    output_digest: String,
    budget_exceeded: Option<Budget>,
    escalated_to_kill: bool,
    cancelled: bool,
}

impl Exit {
    /// Exit code, when the leader exited by itself.
    #[must_use]
    pub const fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Signal number, when the leader was terminated by a signal.
    #[must_use]
    pub const fn signal(&self) -> Option<i32> {
        self.signal
    }

    /// Bytes logged.
    #[must_use]
    pub const fn output_bytes(&self) -> u64 {
        self.output_bytes
    }

    /// Lowercase hex SHA-256 of the whole log.
    #[must_use]
    pub fn output_digest(&self) -> &str {
        &self.output_digest
    }

    /// The budget that ended the run, if any.
    #[must_use]
    pub const fn budget_exceeded(&self) -> Option<Budget> {
        self.budget_exceeded
    }

    /// Whether `SIGKILL` had to follow `SIGTERM`.
    #[must_use]
    pub const fn escalated_to_kill(&self) -> bool {
        self.escalated_to_kill
    }

    /// Whether the run was cancelled through [`InputWriter::cancel`].
    #[must_use]
    pub const fn cancelled(&self) -> bool {
        self.cancelled
    }
}

enum Message {
    Output(u64),
    Checkpoint {
        bytes: u64,
        digest: String,
    },
    Ended {
        bytes: u64,
        digest: String,
        failed: bool,
    },
    Cancel,
}

/// A running session.
pub struct Session {
    child: Box<std::process::Child>,
    pgid: Pid,
    group: u32,
    messages: Receiver<Message>,
    input: InputWriter,
    budgets: RunBudgets,
    idle_after: Option<Duration>,
    started: Instant,
}

impl fmt::Debug for Session {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Session")
            .field("group", &self.group)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// Opens a fresh terminal and starts the program in it.
    ///
    /// # Errors
    ///
    /// [`PtyError::CwdInvalid`] (never falls back to another directory),
    /// [`PtyError::ProgramInvalid`], [`PtyError::LogIo`], [`PtyError::Spawn`].
    pub fn spawn(spec: SpawnSpec) -> Result<Self, PtyError> {
        let cwd = fs::canonicalize(&spec.cwd).map_err(|_| PtyError::CwdInvalid)?;
        if !cwd.is_dir() {
            return Err(PtyError::CwdInvalid);
        }
        if !spec.program.is_absolute() || !spec.program.is_file() {
            return Err(PtyError::ProgramInvalid);
        }
        let log = OpenOptions::new()
            .append(true)
            .create_new(true)
            .open(&spec.log)
            .map_err(|_| PtyError::LogIo)?;
        let (rows, cols) = spec.size;
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|_| PtyError::Spawn)?;
        let mut command = CommandBuilder::new(spec.program.as_os_str());
        command.args(&spec.args);
        command.cwd(cwd.as_os_str());
        command.env_clear();
        command.env("HOME", spec.home.as_os_str());
        command.env("PATH", &spec.path);
        command.env("TERM", "xterm-256color");
        command.env("LANG", "C.UTF-8");
        command.env("SHELL", "/bin/sh");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|_| PtyError::Spawn)?;
        // Our copy of the slave must close, or the log reader never sees the end.
        drop(pair.slave);
        let child: Box<dyn Child> = child;
        let child = child
            .downcast::<std::process::Child>()
            .map_err(|_| PtyError::Spawn)?;
        let group = child.id();
        let pgid = Pid::from_raw(i32::try_from(group).map_err(|_| PtyError::Spawn)?)
            .ok_or(PtyError::Spawn)?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|_| PtyError::Spawn)?;
        let writer = pair.master.take_writer().map_err(|_| PtyError::Spawn)?;
        let (sender, messages) = mpsc::channel();
        let log_sender = sender.clone();
        let (bytes_threshold, interval) = (spec.checkpoint_bytes, spec.checkpoint_interval);
        thread::spawn(move || read_log(reader, log, &log_sender, bytes_threshold, interval));
        Ok(Self {
            child,
            pgid,
            group,
            messages,
            input: InputWriter {
                writer: Arc::new(Mutex::new(writer)),
                master: Arc::new(Mutex::new(pair.master)),
                cancel: sender,
            },
            budgets: spec.budgets,
            idle_after: spec.idle_after,
            started: Instant::now(),
        })
    }

    /// Process-group identifier of the run (the leader's pid).
    #[must_use]
    pub const fn process_group(&self) -> u32 {
        self.group
    }

    /// A handle to write input, resize or cancel from another thread.
    #[must_use]
    pub fn input(&self) -> InputWriter {
        self.input.clone()
    }

    /// Supervises the run to its end, reporting observations to `observe`.
    ///
    /// # Errors
    ///
    /// [`PtyError::Io`] when the child cannot be waited for or signalled,
    /// [`PtyError::LogIo`] when the log could not be written.
    pub fn wait_with<F: FnMut(Observation)>(self, mut observe: F) -> Result<Exit, PtyError> {
        self.wait_with_input(|observation, _| observe(observation))
    }

    /// As [`Session::wait_with`], giving the observer the input handle.
    ///
    /// # Errors
    ///
    /// As [`Session::wait_with`].
    pub fn wait_with_input<F>(mut self, mut observe: F) -> Result<Exit, PtyError>
    where
        F: FnMut(Observation, &InputWriter),
    {
        let input = self.input.clone();
        let mut termination: Option<Instant> = None;
        let mut escalated = false;
        let mut budget_exceeded = None;
        let mut cancelled = false;
        let mut status = None;
        let mut ended: Option<(u64, String, bool)> = None;
        let mut group_gone: Option<Instant> = None;
        let mut output = 0_u64;
        let mut last_output = Instant::now();
        let mut idle_reported = false;
        loop {
            match self.messages.recv_timeout(POLL) {
                Ok(Message::Output(bytes)) => {
                    output = bytes;
                    last_output = Instant::now();
                    if idle_reported {
                        idle_reported = false;
                        observe(Observation::Active, &input);
                    }
                }
                Ok(Message::Checkpoint { bytes, digest }) => {
                    observe(Observation::Checkpoint { bytes, digest }, &input);
                }
                Ok(Message::Ended {
                    bytes,
                    digest,
                    failed,
                }) => ended = Some((bytes, digest, failed)),
                Ok(Message::Cancel) => {
                    if termination.is_none() {
                        cancelled = true;
                        termination = Some(self.signal_group(Signal::TERM)?);
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
            }
            if status.is_none() {
                status = self.child.try_wait().map_err(|_| PtyError::Io)?;
            }
            if termination.is_none() {
                let overrun = if output > self.budgets.max_output_bytes {
                    Some(Budget::Output)
                } else if self.started.elapsed() > self.budgets.max_duration {
                    Some(Budget::Duration)
                } else {
                    None
                };
                if let Some(budget) = overrun {
                    budget_exceeded = Some(budget);
                    observe(Observation::BudgetExceeded(budget), &input);
                    termination = Some(self.signal_group(Signal::TERM)?);
                } else if status.is_some() && self.group_alive() {
                    // The leader is gone; what it left in its group goes too.
                    termination = Some(self.signal_group(Signal::TERM)?);
                }
            }
            if let Some(since) = termination
                && !escalated
                && since.elapsed() >= self.budgets.grace
                && self.group_alive()
            {
                self.signal_group(Signal::KILL)?;
                escalated = true;
            }
            if let Some(idle) = self.idle_after
                && !idle_reported
                && status.is_none()
                && termination.is_none()
                && last_output.elapsed() >= idle
            {
                idle_reported = true;
                observe(Observation::Idle, &input);
            }
            let alive = self.group_alive();
            if status.is_some() && !alive && group_gone.is_none() {
                group_gone = Some(Instant::now());
            }
            let drained =
                ended.is_some() || group_gone.is_some_and(|since| since.elapsed() > DRAIN_LIMIT);
            if let (Some(exit_status), false, true) = (status, alive, drained) {
                let (bytes, digest, failed) = ended.unwrap_or((output, String::new(), true));
                if failed {
                    return Err(PtyError::LogIo);
                }
                return Ok(Exit {
                    exit_code: exit_status.code(),
                    signal: exit_status.signal(),
                    output_bytes: bytes,
                    output_digest: digest,
                    budget_exceeded,
                    escalated_to_kill: escalated,
                    cancelled,
                });
            }
        }
    }

    fn signal_group(&self, signal: Signal) -> Result<Instant, PtyError> {
        match kill_process_group(self.pgid, signal) {
            // ESRCH: the group is already empty.
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(Instant::now()),
            Err(_) => Err(PtyError::Io),
        }
    }

    fn group_alive(&self) -> bool {
        // EPERM would mean a process we may not signal joined the group: treat it as alive.
        !matches!(
            test_kill_process_group(self.pgid),
            Err(rustix::io::Errno::SRCH)
        )
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // A session dropped without being waited for must not leave its group running.
        if self.group_alive() {
            let _ = kill_process_group(self.pgid, Signal::KILL);
        }
        let _ = self.child.try_wait();
    }
}

fn read_log(
    mut reader: Box<dyn std::io::Read + Send>,
    mut log: File,
    sender: &Sender<Message>,
    checkpoint_bytes: u64,
    checkpoint_interval: Duration,
) {
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut since_checkpoint = 0_u64;
    let mut last_checkpoint = Instant::now();
    let mut buffer = vec![0_u8; READ_CHUNK];
    let mut failed = false;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            // EIO is how a terminal master reports that every slave closed.
            Err(_) => break,
        };
        let Some(chunk) = buffer.get(..read) else {
            break;
        };
        if log.write_all(chunk).is_err() {
            failed = true;
            break;
        }
        hasher.update(chunk);
        let read = u64::try_from(read).unwrap_or(u64::MAX);
        total = total.saturating_add(read);
        since_checkpoint = since_checkpoint.saturating_add(read);
        let _ = sender.send(Message::Output(total));
        if since_checkpoint >= checkpoint_bytes || last_checkpoint.elapsed() >= checkpoint_interval
        {
            if log.sync_data().is_err() {
                failed = true;
                break;
            }
            let digest = hex(&hasher.clone().finalize());
            let _ = sender.send(Message::Checkpoint {
                bytes: total,
                digest,
            });
            since_checkpoint = 0;
            last_checkpoint = Instant::now();
        }
    }
    if log.sync_data().is_err() {
        failed = true;
    }
    let _ = sender.send(Message::Ended {
        bytes: total,
        digest: hex(&hasher.finalize()),
        failed,
    });
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            use fmt::Write as _;
            let _ = write!(text, "{byte:02x}");
            text
        })
}
