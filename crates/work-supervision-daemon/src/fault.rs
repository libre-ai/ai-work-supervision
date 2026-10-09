//! Crash injection for the recovery tests.
//!
//! In debug builds only, `WSD_FAULT=<point>` makes the daemon send itself
//! `SIGKILL` when it reaches that point — a real `kill -9`, not an error
//! return. Release builds compile [`hit`] to nothing and never read the
//! variable.

use work_supervision_worktree::FaultPoint;

/// Every point at which a crash can be injected, in the order a mission meets them.
pub const FAULT_POINTS: [&str; 14] = [
    "mission-created",
    "mission-readied",
    "worktree-intent",
    "worktree-during-create",
    "worktree-effect",
    "worktree-confirmed",
    "mission-provisioned",
    "run-started",
    "run-exited",
    "mission-exited",
    "result-submitted",
    "mission-accepted",
    "release-intent",
    "release-effect",
];

/// Kills the daemon with `SIGKILL` if `point` is the injected one.
pub fn hit(point: &str) {
    #[cfg(debug_assertions)]
    if std::env::var("WSD_FAULT").is_ok_and(|planned| planned == point) {
        let _ =
            rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::KILL);
        // SIGKILL cannot be caught; wait for it to land.
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = point;
}

/// The worktree crash plan matching the injected point, if it is a worktree point.
#[must_use]
pub fn worktree_faults() -> work_supervision_worktree::Faults {
    #[cfg(debug_assertions)]
    {
        let point = match std::env::var("WSD_FAULT").as_deref() {
            Ok("worktree-intent") => Some(FaultPoint::AfterCreateIntent),
            Ok("worktree-during-create") => Some(FaultPoint::DuringCreate),
            Ok("worktree-effect") => Some(FaultPoint::AfterCreateEffect),
            Ok("release-intent") => Some(FaultPoint::AfterRemoveIntent),
            Ok("release-effect") => Some(FaultPoint::AfterRemoveEffect),
            _ => None,
        };
        if let Some(point) = point {
            return work_supervision_worktree::Faults::at(point);
        }
    }
    work_supervision_worktree::Faults::none()
}

/// Turns an injected worktree fault into the real crash.
pub fn crash_on_worktree_fault(
    error: work_supervision_worktree::WorktreeError,
) -> work_supervision_worktree::WorktreeError {
    if let work_supervision_worktree::WorktreeError::Fault(point) = error {
        let name = match point {
            FaultPoint::AfterCreateIntent => "worktree-intent",
            FaultPoint::DuringCreate => "worktree-during-create",
            FaultPoint::AfterCreateEffect => "worktree-effect",
            FaultPoint::AfterRemoveIntent => "release-intent",
            FaultPoint::AfterArchive => "release-archived",
            FaultPoint::AfterRemoveEffect => "release-effect",
        };
        hit(name);
    }
    error
}
