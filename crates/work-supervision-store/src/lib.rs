//! SQLite projection of the Work Supervision v0 journal.
//!
//! The journal (`work-supervision-journal`) is the authority of a root; this
//! crate keeps a SQLite database that can always be rebuilt from it
//! (ADR-0042 §5 in `libre-ai/project-governance`):
//!
//! - **journal first, projection second**: an entry is applied only once it is
//!   durable in the journal; [`Store::catch_up`] replays the entries the
//!   projection has not seen yet, so a crash between the two writes is
//!   repaired on the next open;
//! - a projection **ahead** of its journal, or whose last entry has another
//!   digest than the journal's, is a corruption and is refused
//!   ([`StoreError::ProjectionAhead`], [`StoreError::ProjectionDiverged`]);
//! - [`Store::rebuild`] produces a fresh database from the journal alone, and
//!   [`Store::dump`] gives a canonical byte form in which a rebuilt database
//!   equals the live one;
//! - free texts (titles, briefs, criteria, notes, summaries, reasons) live in
//!   a content-addressed [`BlobStore`]; the journal carries their digests only.
//!
//! The SQLite migrations have PostgreSQL equivalents in `migrations/postgres/`;
//! both are checked against `migrations/schema.v0.json`, the SQLite one by this
//! crate's tests and the PostgreSQL one in PGlite (`postgres-check/`).
//!
//! ```
//! use work_supervision_store::{BlobStore, Store};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let root = tempfile::tempdir()?;
//! let blobs = BlobStore::open(&root.path().join("blobs"))?;
//! let digest = blobs.put_text("a brief")?;
//! assert_eq!(blobs.get_text(&digest)?, "a brief");
//! let store = Store::open(&root.path().join("state.sqlite"))?;
//! assert_eq!(store.position()?, None);
//! # Ok(())
//! # }
//! ```

mod blob;
mod coordination;
mod error;
mod projection;
mod records;
mod schema;
mod supervisor;

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, params};
use serde_json::Value;
use work_supervision_journal::{Digest, Entry, Head, Journal, replay};

pub use blob::BlobStore;
pub use error::StoreError;
pub use records::{
    CheckExit, CheckRunRow, CriterionCheck, IdeaRow, OptionRow, RequestRow, ScopeCheckRow,
    SessionRow, TimelineRow,
};
pub use supervisor::{Layout, Supervisor, SupervisorError};

/// Last journal entry applied to a projection: its sequence number and digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    seq: u64,
    digest: Digest,
}

impl Position {
    /// Sequence number of the last applied entry.
    #[must_use]
    pub const fn seq(&self) -> u64 {
        self.seq
    }

    /// Digest of the last applied entry.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }
}

/// An open projection database.
#[derive(Debug)]
pub struct Store {
    connection: Connection,
}

impl Store {
    /// Opens or creates the projection at `path` and applies pending migrations.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] when the database cannot be opened or migrated,
    /// [`StoreError::SchemaTooNew`] when it was migrated by a newer version.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;",
        )?;
        schema::migrate(&mut connection)?;
        Ok(Self { connection })
    }

    /// Opens an existing projection read-only (no migration, no write).
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] when it cannot be opened,
    /// [`StoreError::SchemaTooNew`] when its schema is newer than this crate's.
    pub fn open_read_only(path: &Path) -> Result<Self, StoreError> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let known = i64::try_from(schema::MIGRATIONS.len()).map_err(|_| StoreError::Sqlite)?;
        if version > known {
            return Err(StoreError::SchemaTooNew);
        }
        Ok(Self { connection })
    }

    /// Builds a new projection at `target` from the journal at `journal` alone.
    ///
    /// The database is built under a temporary name next to `target` and
    /// renamed into place only once every entry has been applied: a failed
    /// rebuild leaves no database behind.
    ///
    /// # Errors
    ///
    /// [`StoreError::TargetExists`] when `target` exists, and any refusal of
    /// [`Store::catch_up_from`].
    pub fn rebuild(journal: &Path, blobs: &BlobStore, target: &Path) -> Result<Self, StoreError> {
        if target.exists() {
            return Err(StoreError::TargetExists);
        }
        let partial = sibling(target, ".partial");
        remove_database(&partial)?;
        let built = Self::open(&partial).and_then(|mut store| {
            store.catch_up_from(journal, blobs)?;
            store
                .connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode = DELETE;")?;
            Ok(store)
        });
        match built {
            Ok(store) => {
                store.connection.close().map_err(|_| StoreError::Sqlite)?;
                fs::rename(&partial, target).map_err(|_| StoreError::Sqlite)?;
                Self::open(target)
            }
            Err(error) => {
                remove_database(&partial)?;
                Err(error)
            }
        }
    }

    /// Last entry applied, `None` for a projection that has seen no entry.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] when the position cannot be read or is malformed.
    pub fn position(&self) -> Result<Option<Position>, StoreError> {
        let row: Option<(i64, String)> = self
            .connection
            .query_row(
                "SELECT seq, digest FROM projection_position WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(seq, digest)| {
            Ok(Position {
                seq: u64::try_from(seq).map_err(|_| StoreError::Sqlite)?,
                digest: Digest::from_hex(&digest).ok_or(StoreError::Sqlite)?,
            })
        })
        .transpose()
    }

    /// Applies, through the open writer, every journal entry the projection has not seen.
    ///
    /// Returns the number of entries applied.
    ///
    /// # Errors
    ///
    /// [`StoreError::ProjectionAhead`], [`StoreError::ProjectionDiverged`], the
    /// journal's refusals, and any refusal of [`Store::apply`].
    pub fn catch_up(&mut self, journal: &Journal, blobs: &BlobStore) -> Result<u64, StoreError> {
        let position = self.position()?;
        let mut applied = 0;
        let head =
            journal.replay_after(0, |entry| self.visit(&entry, position, blobs, &mut applied))?;
        check_not_ahead(position, head)?;
        Ok(applied)
    }

    /// As [`Store::catch_up`], reading the journal file at `journal` read-only.
    ///
    /// # Errors
    ///
    /// As [`Store::catch_up`]; an unreadable or absent journal is
    /// [`StoreError::Journal`] with [`work_supervision_journal::JournalError::Io`],
    /// never an empty one.
    pub fn catch_up_from(&mut self, journal: &Path, blobs: &BlobStore) -> Result<u64, StoreError> {
        let position = self.position()?;
        let mut applied = 0;
        let head = replay(journal, |entry| {
            self.visit(&entry, position, blobs, &mut applied)
        })?;
        check_not_ahead(position, head)?;
        Ok(applied)
    }

    fn visit(
        &mut self,
        entry: &Entry,
        position: Option<Position>,
        blobs: &BlobStore,
        applied: &mut u64,
    ) -> Result<(), StoreError> {
        match position {
            Some(position) if entry.seq() < position.seq => Ok(()),
            Some(position) if entry.seq() == position.seq => {
                if *entry.digest() == position.digest {
                    Ok(())
                } else {
                    Err(StoreError::ProjectionDiverged { seq: entry.seq() })
                }
            }
            _ => {
                self.apply(entry, blobs)?;
                *applied += 1;
                Ok(())
            }
        }
    }

    /// Applies one entry, which must immediately follow the current position.
    ///
    /// The entry and the new position are written in one transaction: a
    /// refused entry leaves the projection unchanged.
    ///
    /// # Errors
    ///
    /// [`StoreError::OutOfOrder`], [`StoreError::UnknownKind`],
    /// [`StoreError::EventInvalid`], [`StoreError::BlobMissing`],
    /// [`StoreError::BlobCorrupt`] and [`StoreError::Sqlite`].
    pub fn apply(&mut self, entry: &Entry, blobs: &BlobStore) -> Result<(), StoreError> {
        let expected = self.position()?.map_or(1, |position| position.seq + 1);
        if entry.seq() != expected {
            return Err(StoreError::OutOfOrder {
                seq: entry.seq(),
                expected,
            });
        }
        let seq = i64::try_from(entry.seq()).map_err(|_| StoreError::Sqlite)?;
        let transaction = self.connection.transaction()?;
        projection::apply(&transaction, entry, blobs)?;
        transaction.execute(
            "INSERT INTO projection_position (id, seq, digest) VALUES (1, ?1, ?2)
             ON CONFLICT (id) DO UPDATE SET seq = excluded.seq, digest = excluded.digest",
            params![seq, entry.digest().to_hex()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Canonical byte form of every table: tables by name, rows by primary key,
    /// one JSON array per row. Two projections of the same journal dump equal.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] when a table cannot be read.
    pub fn dump(&self) -> Result<Vec<u8>, StoreError> {
        let mut out = Vec::new();
        for table in schema::tables(&self.connection)? {
            let columns = schema::columns(&self.connection, &table)?;
            let mut keys: Vec<(i64, String)> = columns
                .iter()
                .filter(|(_, _, _, rank)| *rank > 0)
                .map(|(name, _, _, rank)| (*rank, name.clone()))
                .collect();
            keys.sort();
            let order = keys
                .iter()
                .map(|(_, name)| format!("\"{name}\""))
                .collect::<Vec<_>>()
                .join(", ");
            out.extend_from_slice(format!("# {table}\n").as_bytes());
            let mut statement = self
                .connection
                .prepare(&format!("SELECT * FROM \"{table}\" ORDER BY {order}"))?;
            let width = statement.column_count();
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let mut values = Vec::with_capacity(width);
                for index in 0..width {
                    values.push(match row.get_ref(index)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(integer) => Value::from(integer),
                        ValueRef::Text(text) => {
                            Value::String(String::from_utf8_lossy(text).into_owned())
                        }
                        ValueRef::Real(_) | ValueRef::Blob(_) => return Err(StoreError::Sqlite),
                    });
                }
                let line =
                    serde_json::to_vec(&Value::Array(values)).map_err(|_| StoreError::Sqlite)?;
                out.extend_from_slice(&line);
                out.push(b'\n');
            }
        }
        Ok(out)
    }

    /// Describes the schema in the format of `migrations/schema.v0.json`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] when the schema cannot be read.
    pub fn describe_schema(&self) -> Result<Value, StoreError> {
        schema::describe(&self.connection)
    }

    /// The worktree row of mission `mission` (32 hex characters), if any.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn worktree(&self, mission: &str) -> Result<Option<WorktreeRow>, StoreError> {
        Ok(self
            .connection
            .query_row(
                &format!("SELECT {WORKTREE_COLUMNS} FROM worktrees WHERE mission_id = ?1"),
                [mission],
                WorktreeRow::read,
            )
            .optional()?)
    }

    /// Every worktree row, by mission identifier.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn worktrees(&self) -> Result<Vec<WorktreeRow>, StoreError> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {WORKTREE_COLUMNS} FROM worktrees ORDER BY mission_id"
        ))?;
        let rows = statement
            .query_map([], WorktreeRow::read)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Every run of mission `mission`, in start order.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn runs_of(&self, mission: &str) -> Result<Vec<RunRow>, StoreError> {
        self.query_runs("WHERE mission_id = ?1 ORDER BY started_seq", Some(mission))
    }

    /// Notes of mission `mission`: instant and text, in journal order.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn notes_of(&self, mission: &str) -> Result<Vec<(String, String)>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT at, text FROM mission_notes WHERE mission_id = ?1 ORDER BY seq")?;
        let notes = statement
            .query_map([mission], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(notes)
    }

    /// Every run still projected as `running`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn running_runs(&self) -> Result<Vec<RunRow>, StoreError> {
        self.query_runs("WHERE state = 'running' ORDER BY started_seq", None)
    }

    fn query_runs(&self, filter: &str, mission: Option<&str>) -> Result<Vec<RunRow>, StoreError> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT run_id, mission_id, state, output_bytes, output_digest, inputs, exit_code, signal, budget
             FROM runs {filter}"
        ))?;
        let rows = match mission {
            Some(mission) => statement
                .query_map([mission], RunRow::read)?
                .collect::<Result<Vec<_>, _>>()?,
            None => statement
                .query_map([], RunRow::read)?
                .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(rows)
    }

    /// The underlying connection, for read queries of the crates built on the projection.
    #[must_use]
    pub const fn connection(&self) -> &Connection {
        &self.connection
    }
}

fn check_not_ahead(position: Option<Position>, head: Option<Head>) -> Result<(), StoreError> {
    let journal = head.map_or(0, |head| head.seq());
    match position {
        Some(position) if position.seq > journal => Err(StoreError::ProjectionAhead {
            projection: position.seq,
            journal,
        }),
        _ => Ok(()),
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

/// Removes a database file and its WAL companions; absent files are not an error.
fn remove_database(path: &Path) -> Result<(), StoreError> {
    for candidate in [
        path.to_owned(),
        sibling(path, "-wal"),
        sibling(path, "-shm"),
    ] {
        match fs::remove_file(&candidate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(StoreError::Sqlite),
        }
    }
    Ok(())
}

const WORKTREE_COLUMNS: &str =
    "mission_id, repository, path, branch, base_commit, state, head, delete_branch, archive_digest";

/// A projected worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRow {
    /// Mission identifier.
    pub mission: String,
    /// Repository name from the private configuration.
    pub repository: String,
    /// Path relative to the root.
    pub path: String,
    /// Branch.
    pub branch: String,
    /// Base commit.
    pub base_commit: String,
    /// `creating`, `created`, `aborted`, `releasing` or `removed`.
    pub state: String,
    /// HEAD observed at creation.
    pub head: Option<String>,
    /// Whether the release deletes the branch (known once released).
    pub delete_branch: Option<bool>,
    /// Digest of the archived diff, when one was archived.
    pub archive_digest: Option<String>,
}

impl WorktreeRow {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            mission: row.get(0)?,
            repository: row.get(1)?,
            path: row.get(2)?,
            branch: row.get(3)?,
            base_commit: row.get(4)?,
            state: row.get(5)?,
            head: row.get(6)?,
            delete_branch: row.get::<_, Option<i64>>(7)?.map(|flag| flag != 0),
            archive_digest: row.get(8)?,
        })
    }
}

/// A projected run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    /// Run identifier.
    pub run: String,
    /// Mission identifier.
    pub mission: String,
    /// `running`, `exited` or `interrupted`.
    pub state: String,
    /// Output bytes at the last checkpoint or exit.
    pub output_bytes: i64,
    /// Digest of that output.
    pub output_digest: Option<String>,
    /// Inputs written.
    pub inputs: i64,
    /// Exit code, when the leader exited by itself.
    pub exit_code: Option<i64>,
    /// Signal number, when it was terminated by one.
    pub signal: Option<i64>,
    /// Overrun budget, if any.
    pub budget: Option<String>,
}

impl RunRow {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            run: row.get(0)?,
            mission: row.get(1)?,
            state: row.get(2)?,
            output_bytes: row.get(3)?,
            output_digest: row.get(4)?,
            inputs: row.get(5)?,
            exit_code: row.get(6)?,
            signal: row.get(7)?,
            budget: row.get(8)?,
        })
    }
}
