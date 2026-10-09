use std::fmt;

use work_supervision_journal::JournalError;

/// Every way the store refuses an operation.
///
/// `Display` names a stable code and, where relevant, a journal sequence
/// number: it never echoes a text, a digest, a path or a database value, so
/// that an error can be logged without leaking the content of a mission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    /// SQLite refused an operation on the projection.
    Sqlite,
    /// The projection database was written by a newer schema than this crate knows.
    SchemaTooNew,
    /// The journal refused to be read; carries the journal's own refusal.
    Journal(JournalError),
    /// The projection records more entries than the journal holds.
    ProjectionAhead {
        /// Last sequence number applied to the projection.
        projection: u64,
        /// Last sequence number of the journal.
        journal: u64,
    },
    /// The journal entry at the projection's position has another digest.
    ProjectionDiverged {
        /// Sequence number whose digest differs.
        seq: u64,
    },
    /// An entry was applied out of order.
    OutOfOrder {
        /// Sequence number of the entry offered.
        seq: u64,
        /// Sequence number the projection expected.
        expected: u64,
    },
    /// The event kind is not part of the projection catalogue.
    UnknownKind {
        /// Sequence number of the entry.
        seq: u64,
    },
    /// The event data does not match its kind, or contradicts the projection
    /// (unknown mission, stale revision, duplicate creation).
    EventInvalid {
        /// Sequence number of the entry.
        seq: u64,
    },
    /// A text referenced by an entry is absent from the blob store.
    BlobMissing {
        /// Sequence number of the entry.
        seq: u64,
    },
    /// A blob could not be read or written.
    BlobIo,
    /// A blob's bytes do not hash to its name, or a text blob is not UTF-8.
    BlobCorrupt,
    /// The rebuild target already exists; a rebuild never overwrites a database.
    TargetExists,
}

impl StoreError {
    /// Stable machine-readable code of the refusal.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Sqlite => "projection.sqlite",
            Self::SchemaTooNew => "projection.schema_too_new",
            Self::Journal(JournalError::Io) => "journal.io",
            Self::Journal(_) => "journal.invalid",
            Self::ProjectionAhead { .. } => "projection.ahead",
            Self::ProjectionDiverged { .. } => "projection.diverged",
            Self::OutOfOrder { .. } => "projection.out_of_order",
            Self::UnknownKind { .. } => "projection.unknown_kind",
            Self::EventInvalid { .. } => "projection.event_invalid",
            Self::BlobMissing { .. } => "projection.blob_missing",
            Self::BlobIo => "blob.io",
            Self::BlobCorrupt => "blob.corrupt",
            Self::TargetExists => "projection.target_exists",
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Journal(error) => write!(formatter, "{}: {error}", self.code()),
            Self::ProjectionAhead {
                projection,
                journal,
            } => write!(
                formatter,
                "{}: projection at {projection}, journal at {journal}",
                self.code()
            ),
            Self::ProjectionDiverged { seq }
            | Self::UnknownKind { seq }
            | Self::EventInvalid { seq }
            | Self::BlobMissing { seq } => write!(formatter, "{} at seq {seq}", self.code()),
            Self::OutOfOrder { seq, expected } => write!(
                formatter,
                "{}: seq {seq} offered, {expected} expected",
                self.code()
            ),
            Self::Sqlite
            | Self::SchemaTooNew
            | Self::BlobIo
            | Self::BlobCorrupt
            | Self::TargetExists => formatter.write_str(self.code()),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<JournalError> for StoreError {
    fn from(error: JournalError) -> Self {
        Self::Journal(error)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Sqlite
    }
}
