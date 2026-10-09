use std::fmt;

/// A refusal or failure of the daemon, named by a stable code only.
///
/// Codes come from the domain (`mission.*`, `agent.*`), the projection
/// (`projection.*`), the worktree manager (`worktree.*`), the PTY layer
/// (`pty.*`) or the daemon itself (`config.*`, `root.*`, `request.*`,
/// `socket.*`, `run.*`). `Display` writes the code: never a text, a path or a
/// value, so a failure can be logged and sent to a client as is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure(&'static str);

impl Failure {
    /// A failure with `code`.
    #[must_use]
    pub const fn new(code: &'static str) -> Self {
        Self(code)
    }

    /// Its code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.0
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for Failure {}

impl From<work_supervision_domain::Refusal> for Failure {
    fn from(refusal: work_supervision_domain::Refusal) -> Self {
        Self(refusal.code())
    }
}

impl From<work_supervision_store::SupervisorError> for Failure {
    fn from(error: work_supervision_store::SupervisorError) -> Self {
        Self(error.code())
    }
}

impl From<work_supervision_store::StoreError> for Failure {
    fn from(error: work_supervision_store::StoreError) -> Self {
        Self(error.code())
    }
}

impl From<work_supervision_journal::JournalError> for Failure {
    fn from(error: work_supervision_journal::JournalError) -> Self {
        let _ = error;
        Self("journal.invalid")
    }
}

impl From<work_supervision_worktree::WorktreeError> for Failure {
    fn from(error: work_supervision_worktree::WorktreeError) -> Self {
        Self(error.code())
    }
}

impl From<work_supervision_pty::PtyError> for Failure {
    fn from(error: work_supervision_pty::PtyError) -> Self {
        Self(error.code())
    }
}
