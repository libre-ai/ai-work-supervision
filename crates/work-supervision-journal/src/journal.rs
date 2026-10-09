use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::codec::{Head, decode_line, encode_line};
use crate::{Digest, Event, JournalError, MAX_LINE_BYTES, Timestamp};

/// Kind of the entry appended after a torn tail has been quarantined.
pub const RECOVERED_KIND: &str = "journal.recovered";

/// How [`Journal::open`] treats a last line without its terminating newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenMode {
    /// Refuse with [`JournalError::TornTail`]; nothing is modified.
    Strict,
    /// Move the torn bytes to a quarantine file next to the journal, truncate
    /// them, then append a [`RECOVERED_KIND`] entry stamped `at`.
    RecoverTornTail {
        /// Instant recorded on the recovery entry.
        at: Timestamp,
    },
}

/// What a recovery set aside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    quarantine_path: PathBuf,
    torn_bytes: u64,
    torn_digest: Digest,
}

impl Recovery {
    /// File that now holds the torn bytes, byte for byte.
    #[must_use]
    pub fn quarantine_path(&self) -> &Path {
        &self.quarantine_path
    }

    /// Number of torn bytes removed from the journal.
    #[must_use]
    pub const fn torn_bytes(&self) -> u64 {
        self.torn_bytes
    }

    /// SHA-256 of the torn bytes.
    #[must_use]
    pub const fn torn_digest(&self) -> &Digest {
        &self.torn_digest
    }
}

/// An open journal, held by exactly one writer.
///
/// Opening verifies every entry from the first; appending writes one canonical
/// line and its newline, then synchronises the file before returning. A line is
/// committed once its newline is durable: a crash before that leaves a torn
/// tail, never a silently truncated entry.
#[derive(Debug)]
pub struct Journal {
    file: File,
    path: PathBuf,
    head: Option<Head>,
    entries: u64,
    poisoned: bool,
}

struct Scan {
    head: Option<Head>,
    entries: u64,
    committed_len: u64,
    torn: Option<Vec<u8>>,
}

impl Journal {
    /// Opens or creates the journal at `path`, takes its exclusive lock and verifies it.
    ///
    /// # Errors
    ///
    /// [`JournalError::Locked`] when another writer holds it, [`JournalError::Io`]
    /// or [`JournalError::NotRegularFile`] for the file itself, and any entry
    /// refusal with the line it concerns. A torn tail is refused in
    /// [`OpenMode::Strict`].
    pub fn open(path: &Path, mode: OpenMode) -> Result<(Self, Option<Recovery>), JournalError> {
        let existed = match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => true,
            Ok(_) => return Err(JournalError::NotRegularFile),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return Err(JournalError::Io),
        };
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(path)
            .map_err(|_| JournalError::Io)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(JournalError::Locked),
            Err(TryLockError::Error(_)) => return Err(JournalError::Io),
        }
        if !existed {
            sync_parent(path)?;
        }

        let scan = scan(&file)?;
        let mut journal = Self {
            file,
            path: path.to_owned(),
            head: scan.head,
            entries: scan.entries,
            poisoned: false,
        };
        let Some(torn) = scan.torn else {
            return Ok((journal, None));
        };
        let torn_line = scan.entries + 1;
        let OpenMode::RecoverTornTail { at } = mode else {
            return Err(JournalError::TornTail { line: torn_line });
        };
        let recovery = journal.quarantine(&torn, torn_line, scan.committed_len)?;
        let mut data = Map::new();
        data.insert(
            "quarantine".to_owned(),
            Value::String(file_name(recovery.quarantine_path()).to_owned()),
        );
        data.insert("torn_bytes".to_owned(), Value::from(recovery.torn_bytes));
        data.insert(
            "torn_digest".to_owned(),
            Value::String(recovery.torn_digest.to_hex()),
        );
        journal.append(at, Event::new(RECOVERED_KIND, data)?)?;
        Ok((journal, Some(recovery)))
    }

    /// Position of the last entry, `None` for an empty journal.
    #[must_use]
    pub const fn head(&self) -> Option<&Head> {
        self.head.as_ref()
    }

    /// Number of entries in the journal.
    #[must_use]
    pub const fn entries(&self) -> u64 {
        self.entries
    }

    /// Appends `event` as the next entry and returns its position once it is durable.
    ///
    /// # Errors
    ///
    /// [`JournalError::LineTooLong`] before any write; [`JournalError::Io`] when
    /// the write or the synchronisation fails, after which the handle is
    /// poisoned and every later append returns [`JournalError::Poisoned`].
    pub fn append(&mut self, at: Timestamp, event: Event) -> Result<Head, JournalError> {
        if self.poisoned {
            return Err(JournalError::Poisoned);
        }
        let seq = match &self.head {
            None => 1,
            Some(head) => head
                .seq()
                .checked_add(1)
                .ok_or(JournalError::SequenceInvalid { line: u64::MAX })?,
        };
        let prev = self.head.as_ref().map(Head::digest);
        let encoded = encode_line(seq, prev, &at, &event)?;
        let mut line = Vec::with_capacity(encoded.bytes().len() + 1);
        line.extend_from_slice(encoded.bytes());
        line.push(b'\n');
        if self
            .file
            .write_all(&line)
            .and_then(|()| self.file.sync_all())
            .is_err()
        {
            self.poisoned = true;
            return Err(JournalError::Io);
        }
        let head = Head::new(seq, *encoded.digest());
        self.head = Some(head);
        self.entries = seq;
        Ok(head)
    }

    fn quarantine(
        &mut self,
        torn: &[u8],
        line: u64,
        committed_len: u64,
    ) -> Result<Recovery, JournalError> {
        let quarantine_path = self
            .path
            .with_file_name(format!("{}.torn-{line}", file_name(&self.path)));
        let mut quarantine = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&quarantine_path)
            .map_err(|_| JournalError::Io)?;
        quarantine
            .write_all(torn)
            .and_then(|()| quarantine.sync_all())
            .map_err(|_| JournalError::Io)?;
        sync_parent(&quarantine_path)?;
        self.file
            .set_len(committed_len)
            .and_then(|()| self.file.sync_all())
            .map_err(|_| JournalError::Io)?;
        Ok(Recovery {
            quarantine_path,
            torn_bytes: u64::try_from(torn.len()).map_err(|_| JournalError::Io)?,
            torn_digest: Digest::of(torn),
        })
    }
}

fn scan(file: &File) -> Result<Scan, JournalError> {
    let mut reader = BufReader::new(file);
    let limit = u64::try_from(MAX_LINE_BYTES).map_err(|_| JournalError::Io)? + 1;
    let mut buffer = Vec::new();
    let mut scan = Scan {
        head: None,
        entries: 0,
        committed_len: 0,
        torn: None,
    };
    loop {
        buffer.clear();
        let read = (&mut reader)
            .take(limit)
            .read_until(b'\n', &mut buffer)
            .map_err(|_| JournalError::Io)?;
        if read == 0 {
            return Ok(scan);
        }
        let line = scan.entries + 1;
        if buffer.last() != Some(&b'\n') {
            if buffer.len() > MAX_LINE_BYTES {
                return Err(JournalError::LineTooLong { line });
            }
            scan.torn = Some(buffer);
            return Ok(scan);
        }
        buffer.pop();
        let head = decode_line(&buffer, line, scan.head.as_ref())?;
        scan.head = Some(head);
        scan.entries = line;
        scan.committed_len += u64::try_from(read).map_err(|_| JournalError::Io)?;
    }
}

fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("journal")
}

fn sync_parent(path: &Path) -> Result<(), JournalError> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| JournalError::Io)
}
