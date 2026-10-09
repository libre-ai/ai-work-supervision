//! Append-only, hash-chained journal of Work Supervision v0.
//!
//! The journal is the authority of a Work Supervision root: the SQLite state
//! is a projection rebuilt from it (ADR-0042 §5 in `libre-ai/project-governance`).
//! The format is `libre-ai.work-supervision.journal.v0`, specified in
//! `docs/work-supervision/journal-v0.md`:
//!
//! - one entry per line, each line the RFC 8785 (JCS) encoding of
//!   `{at, digest, event: {data, kind}, prev, schema, seq}` followed by `\n`;
//! - `digest` is the SHA-256 of the canonical entry without its `digest` key;
//!   `prev` is the previous entry's digest, `null` for `seq = 1`;
//! - values are strings, booleans, `null`, integers within ±(2^53 − 1),
//!   arrays and objects — no floats — nested at most [`MAX_DEPTH`] deep, and
//!   a line holds at most [`MAX_LINE_BYTES`].
//!
//! The independent verifier, `ws-journal-verify` (crate
//! `work-supervision-journal-verifier`), re-implements these rules without this
//! crate's code.
//!
//! ```
//! use serde_json::{Map, Value};
//! use work_supervision_journal::{Event, Journal, OpenMode, Timestamp};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let root = tempfile::tempdir()?;
//! let path = root.path().join("journal.v0.jsonl");
//! let (mut journal, recovery) = Journal::open(&path, OpenMode::Strict)?;
//! assert!(recovery.is_none());
//! let data = Map::from_iter([("text".to_owned(), Value::from("first note"))]);
//! let head = journal.append(
//!     Timestamp::parse("2026-10-09T00:00:00.000Z")?,
//!     Event::new("mission.note", data)?,
//! )?;
//! assert_eq!(head.seq(), 1);
//! # Ok(())
//! # }
//! ```

mod codec;
mod digest;
mod entry;
mod error;
mod event;
mod journal;
mod timestamp;

pub use codec::{EncodedLine, Head, encode_line};
pub use digest::Digest;
pub use entry::{Entry, replay};
pub use error::JournalError;
pub use event::Event;
pub use journal::{Journal, OpenMode, RECOVERED_KIND, Recovery};
pub use timestamp::Timestamp;

/// Value of the `schema` field of every v0 entry.
pub const SCHEMA: &str = "libre-ai.work-supervision.journal.v0";

/// Largest accepted line, in bytes, without its newline (1 MiB).
pub const MAX_LINE_BYTES: usize = 1 << 20;

/// Deepest accepted container nesting, the entry object itself being depth 1.
pub const MAX_DEPTH: usize = 32;

/// Longest accepted event kind, in bytes.
pub const MAX_KIND_BYTES: usize = 64;

/// Largest magnitude of an accepted integer: 2^53 − 1, exact in every JSON implementation.
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
