//! Anchoring the journal head outside the root.
//!
//! The hash chain detects any altered, removed or reordered entry, but not a
//! **complete and consistent rewrite** of the journal file: a forged history
//! chains as well as the true one. The anchor closes that gap: after every
//! request and run event the daemon records the current head — sequence
//! number and digest — in a private file **outside the root**
//! (`[anchor] path` of the configuration), and at start it refuses a journal
//! whose entry at the anchored sequence number is missing or has another
//! digest (`journal.anchor_mismatch`).
//!
//! Format: one line, `{"digest":"<hex64>","schema":"libre-ai.work-supervision.anchor.v0","seq":<n>}`,
//! written to a temporary name, synchronised, renamed. No content.

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use serde_json::{Map, Value};
use work_supervision_journal::{Digest, JournalError};

use crate::Failure;

/// Schema identifier of an anchor line.
pub const ANCHOR_SCHEMA: &str = "libre-ai.work-supervision.anchor.v0";

/// A recorded journal head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    seq: u64,
    digest: String,
}

impl Anchor {
    /// The head `seq` with `digest` (64 lowercase hex characters).
    ///
    /// # Errors
    ///
    /// `journal.anchor_invalid`.
    pub fn new(seq: u64, digest: &str) -> Result<Self, Failure> {
        if seq == 0 || Digest::from_hex(digest).is_none() {
            return Err(Failure::new("journal.anchor_invalid"));
        }
        Ok(Self {
            seq,
            digest: digest.to_owned(),
        })
    }

    /// Reads the anchor file; `None` when it does not exist.
    ///
    /// # Errors
    ///
    /// `journal.anchor_unreadable`, `journal.anchor_invalid` (anything but
    /// exactly the three keys with their types, then a newline).
    pub fn read(path: &Path) -> Result<Option<Self>, Failure> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Failure::new("journal.anchor_unreadable")),
        };
        let invalid = Failure::new("journal.anchor_invalid");
        let line = text.strip_suffix('\n').ok_or(invalid)?;
        let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(line) else {
            return Err(invalid);
        };
        if fields.len() != 3 || fields.get("schema").and_then(Value::as_str) != Some(ANCHOR_SCHEMA)
        {
            return Err(invalid);
        }
        let seq = fields.get("seq").and_then(Value::as_u64).ok_or(invalid)?;
        let digest = fields
            .get("digest")
            .and_then(Value::as_str)
            .ok_or(invalid)?;
        Self::new(seq, digest).map(Some)
    }

    /// Writes the anchor atomically (temporary file, `fsync`, rename, directory `fsync`), mode 0600.
    ///
    /// # Errors
    ///
    /// `journal.anchor_io`.
    pub fn write(&self, path: &Path) -> Result<(), Failure> {
        let io = |_| Failure::new("journal.anchor_io");
        let mut fields = Map::new();
        fields.insert("digest".to_owned(), Value::String(self.digest.clone()));
        fields.insert("schema".to_owned(), Value::String(ANCHOR_SCHEMA.to_owned()));
        fields.insert("seq".to_owned(), Value::from(self.seq));
        let mut line = serde_json::to_vec(&Value::Object(fields))
            .map_err(|_| Failure::new("journal.anchor_io"))?;
        line.push(b'\n');
        let mut temporary_name = path
            .file_name()
            .map(std::ffi::OsStr::to_os_string)
            .unwrap_or_default();
        temporary_name.push(".partial");
        let temporary = path.with_file_name(temporary_name);
        match fs::remove_file(&temporary) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io(error)),
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(io)?;
        file.write_all(&line)
            .and_then(|()| file.sync_all())
            .map_err(io)?;
        drop(file);
        fs::rename(&temporary, path).map_err(io)?;
        if let Some(parent) = path.parent() {
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(io)?;
        }
        Ok(())
    }

    /// Anchored sequence number.
    #[must_use]
    pub const fn seq(&self) -> u64 {
        self.seq
    }

    /// Anchored digest.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// Checks that the journal at `journal` holds, at the anchored sequence
/// number, an entry with the anchored digest.
///
/// # Errors
///
/// `journal.anchor_mismatch` (shorter journal or other digest),
/// `journal.invalid` / `journal.unreadable` when the journal itself is refused.
pub fn check_anchor(journal: &Path, anchor: &Anchor) -> Result<(), Failure> {
    let mut found = None;
    let outcome = work_supervision_journal::replay(journal, |entry| {
        if entry.seq() == anchor.seq {
            found = Some(entry.digest().to_hex());
        }
        Ok::<(), JournalError>(())
    });
    match outcome {
        Ok(_) | Err(JournalError::TornTail { .. }) => {}
        Err(JournalError::Io) => return Err(Failure::new("journal.unreadable")),
        Err(_) => return Err(Failure::new("journal.invalid")),
    }
    if found.as_deref() == Some(anchor.digest.as_str()) {
        Ok(())
    } else {
        Err(Failure::new("journal.anchor_mismatch"))
    }
}
