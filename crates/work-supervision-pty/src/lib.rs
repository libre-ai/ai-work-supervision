//! Fresh PTY sessions of Work Supervision v0.
//!
//! One session per run, started in the mission's worktree:
//!
//! - the environment is **cleared**, then only [`ALLOWED_ENVIRONMENT`] is set
//!   (`PATH` given by the caller, `HOME` inside the root, `TERM`, `LANG`,
//!   `SHELL`): no secret of the host is inherited;
//! - the child is a session and process-group leader (`setsid`, by
//!   `portable-pty`), so the whole group can be signalled;
//! - every byte read from the terminal is appended to the run's log, with a
//!   rolling SHA-256; [`Observation::Checkpoint`] reports the cumulative byte
//!   count and digest at bounded intervals, which the daemon journals;
//! - input is written to the terminal and reported by length and digest only,
//!   never by content;
//! - a budget overrun (duration or output) sends `SIGTERM` to the group, then
//!   `SIGKILL` after the grace delay; when the leader exits, the rest of its
//!   group is terminated the same way, so no process survives a run.
//!
//! Executor resolution is closed: [`executor_program`] maps the single
//! profile [`ExecutorProfile::Fake`] to the fake agent (C0 guard).

mod session;

pub use session::{
    ALLOWED_ENVIRONMENT, Budget, Exit, InputRecord, InputWriter, Observation, PtyError, RunBudgets,
    Session, SpawnSpec,
};

use std::path::{Path, PathBuf};

use work_supervision_domain::ExecutorProfile;

/// Program to start for `profile`.
///
/// The match is exhaustive over the profiles this build knows; before the C0
/// confinement qualification the only one is the fake agent, whose binary the
/// caller locates (`fake_agent`).
#[must_use]
pub fn executor_program(profile: ExecutorProfile, fake_agent: &Path) -> PathBuf {
    match profile {
        ExecutorProfile::Fake => fake_agent.to_owned(),
    }
}
