use std::fs::{self, File};
use std::io::{BufRead as _, BufReader, Read as _};
use std::path::Path;

use crate::MAX_LINE_BYTES;
use crate::codec::{Head, decode_entry};
use crate::{Digest, Event, JournalError, Timestamp};

/// One verified entry read back from a journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    head: Head,
    at: Timestamp,
    event: Event,
}

impl Entry {
    pub(crate) const fn new(head: Head, at: Timestamp, event: Event) -> Self {
        Self { head, at, event }
    }

    /// Sequence number of the entry, starting at 1.
    #[must_use]
    pub const fn seq(&self) -> u64 {
        self.head.seq()
    }

    /// Digest recorded in the entry.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        self.head.digest()
    }

    /// Position of the entry in the chain.
    #[must_use]
    pub const fn head(&self) -> Head {
        self.head
    }

    /// Instant recorded by the writer.
    #[must_use]
    pub const fn at(&self) -> &Timestamp {
        &self.at
    }

    /// The event the entry carries.
    #[must_use]
    pub const fn event(&self) -> &Event {
        &self.event
    }
}

/// Reads and verifies every committed entry of the journal at `path`, in order.
///
/// The file is opened read-only and without the writer's lock. Each entry is
/// verified before `visit` sees it; the first refusal stops the replay. A torn
/// tail is refused with [`JournalError::TornTail`] after the entries before it
/// have been visited: replaying never repairs a file.
///
/// # Errors
///
/// [`JournalError::Io`] when the file cannot be read (a missing file is never
/// an empty journal), any entry refusal with its line, or the first error
/// `visit` returns.
pub fn replay<E, F>(path: &Path, visit: F) -> Result<Option<Head>, E>
where
    E: From<JournalError>,
    F: FnMut(Entry) -> Result<(), E>,
{
    let metadata = fs::symlink_metadata(path).map_err(|_| E::from(JournalError::Io))?;
    if !metadata.file_type().is_file() {
        return Err(E::from(JournalError::NotRegularFile));
    }
    let file = File::open(path).map_err(|_| E::from(JournalError::Io))?;
    replay_file(&file, 0, visit)
}

/// Visits the entries of `file` whose `seq` exceeds `after`; every entry is verified.
pub(crate) fn replay_file<E, F>(file: &File, after: u64, mut visit: F) -> Result<Option<Head>, E>
where
    E: From<JournalError>,
    F: FnMut(Entry) -> Result<(), E>,
{
    let mut reader = BufReader::new(file);
    let limit = u64::try_from(MAX_LINE_BYTES).map_err(|_| E::from(JournalError::Io))? + 1;
    let mut buffer = Vec::new();
    let mut head: Option<Head> = None;
    loop {
        buffer.clear();
        let read = (&mut reader)
            .take(limit)
            .read_until(b'\n', &mut buffer)
            .map_err(|_| E::from(JournalError::Io))?;
        if read == 0 {
            return Ok(head);
        }
        let line = head.as_ref().map_or(1, |current| current.seq() + 1);
        if buffer.last() != Some(&b'\n') {
            if buffer.len() > MAX_LINE_BYTES {
                return Err(E::from(JournalError::LineTooLong { line }));
            }
            return Err(E::from(JournalError::TornTail { line }));
        }
        buffer.pop();
        let entry = decode_entry(&buffer, line, head.as_ref()).map_err(E::from)?;
        head = Some(entry.head());
        if entry.seq() > after {
            visit(entry)?;
        }
    }
}
