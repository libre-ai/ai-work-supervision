use std::fmt;

/// Every way the journal refuses an operation.
///
/// `line` is the 1-based line of the journal file the refusal concerns; it is
/// `0` when the refused value has not been placed in a file yet (an event or a
/// timestamp validated before an append). `Display` names the stable code and
/// the line only: it never echoes journal content, paths or rejected values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalError {
    /// The file or its directory could not be read, written or synchronised.
    Io,
    /// The journal path exists but is not a regular file (a symbolic link, a directory…).
    NotRegularFile,
    /// Another writer holds the exclusive lock on this journal.
    Locked,
    /// An earlier append failed mid-way; the handle refuses further writes until reopened.
    Poisoned,
    /// A line exceeds [`crate::MAX_LINE_BYTES`].
    LineTooLong {
        /// Line concerned.
        line: u64,
    },
    /// A line is not a JSON document.
    Malformed {
        /// Line concerned.
        line: u64,
    },
    /// A line is valid JSON but not its own RFC 8785 canonical encoding.
    NonCanonical {
        /// Line concerned.
        line: u64,
    },
    /// The entry object does not have exactly the v0 envelope fields with their types.
    EnvelopeInvalid {
        /// Line concerned.
        line: u64,
    },
    /// The `schema` field names a format this crate does not read.
    SchemaUnknown {
        /// Line concerned.
        line: u64,
    },
    /// The first entry is not `seq = 1` with `prev = null`.
    GenesisInvalid {
        /// Line concerned.
        line: u64,
    },
    /// `seq` is not the previous `seq` plus one.
    SequenceInvalid {
        /// Line concerned.
        line: u64,
    },
    /// `prev` is not the digest of the previous entry.
    PreviousDigestMismatch {
        /// Line concerned.
        line: u64,
    },
    /// `digest` is not the SHA-256 of the canonical entry without its digest.
    DigestMismatch {
        /// Line concerned.
        line: u64,
    },
    /// `at` is not a UTC timestamp `YYYY-MM-DDTHH:MM:SS.mmmZ` on a real calendar date.
    TimestampInvalid {
        /// Line concerned.
        line: u64,
    },
    /// The event kind does not follow the dotted lowercase grammar.
    KindInvalid {
        /// Line concerned.
        line: u64,
    },
    /// A number is a float or an integer outside ±(2^53 − 1).
    NumberInvalid {
        /// Line concerned.
        line: u64,
    },
    /// The entry nests containers deeper than [`crate::MAX_DEPTH`].
    DepthExceeded {
        /// Line concerned.
        line: u64,
    },
    /// The last line has no terminating newline: an append was interrupted.
    TornTail {
        /// Line concerned.
        line: u64,
    },
}

impl JournalError {
    /// Stable machine-readable code of the refusal.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Io => "io",
            Self::NotRegularFile => "not-regular-file",
            Self::Locked => "locked",
            Self::Poisoned => "poisoned",
            Self::LineTooLong { .. } => "line-too-long",
            Self::Malformed { .. } => "malformed",
            Self::NonCanonical { .. } => "non-canonical",
            Self::EnvelopeInvalid { .. } => "envelope-invalid",
            Self::SchemaUnknown { .. } => "schema-unknown",
            Self::GenesisInvalid { .. } => "genesis-invalid",
            Self::SequenceInvalid { .. } => "sequence-invalid",
            Self::PreviousDigestMismatch { .. } => "previous-digest-mismatch",
            Self::DigestMismatch { .. } => "digest-mismatch",
            Self::TimestampInvalid { .. } => "timestamp-invalid",
            Self::KindInvalid { .. } => "kind-invalid",
            Self::NumberInvalid { .. } => "number-invalid",
            Self::DepthExceeded { .. } => "depth-exceeded",
            Self::TornTail { .. } => "torn-tail",
        }
    }

    /// Line the refusal concerns, when it concerns a line of the file.
    #[must_use]
    pub const fn line(&self) -> Option<u64> {
        match self {
            Self::Io | Self::NotRegularFile | Self::Locked | Self::Poisoned => None,
            Self::LineTooLong { line }
            | Self::Malformed { line }
            | Self::NonCanonical { line }
            | Self::EnvelopeInvalid { line }
            | Self::SchemaUnknown { line }
            | Self::GenesisInvalid { line }
            | Self::SequenceInvalid { line }
            | Self::PreviousDigestMismatch { line }
            | Self::DigestMismatch { line }
            | Self::TimestampInvalid { line }
            | Self::KindInvalid { line }
            | Self::NumberInvalid { line }
            | Self::DepthExceeded { line }
            | Self::TornTail { line } => {
                if *line == 0 {
                    None
                } else {
                    Some(*line)
                }
            }
        }
    }

    /// The same refusal attributed to `line`, for values validated before placement.
    pub(crate) const fn at_line(self, line: u64) -> Self {
        match self {
            Self::Io | Self::NotRegularFile | Self::Locked | Self::Poisoned => self,
            Self::LineTooLong { .. } => Self::LineTooLong { line },
            Self::Malformed { .. } => Self::Malformed { line },
            Self::NonCanonical { .. } => Self::NonCanonical { line },
            Self::EnvelopeInvalid { .. } => Self::EnvelopeInvalid { line },
            Self::SchemaUnknown { .. } => Self::SchemaUnknown { line },
            Self::GenesisInvalid { .. } => Self::GenesisInvalid { line },
            Self::SequenceInvalid { .. } => Self::SequenceInvalid { line },
            Self::PreviousDigestMismatch { .. } => Self::PreviousDigestMismatch { line },
            Self::DigestMismatch { .. } => Self::DigestMismatch { line },
            Self::TimestampInvalid { .. } => Self::TimestampInvalid { line },
            Self::KindInvalid { .. } => Self::KindInvalid { line },
            Self::NumberInvalid { .. } => Self::NumberInvalid { line },
            Self::DepthExceeded { .. } => Self::DepthExceeded { line },
            Self::TornTail { .. } => Self::TornTail { line },
        }
    }
}

impl fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line() {
            Some(line) => write!(formatter, "journal refused: {} at line {line}", self.code()),
            None => write!(formatter, "journal refused: {}", self.code()),
        }
    }
}

impl std::error::Error for JournalError {}
