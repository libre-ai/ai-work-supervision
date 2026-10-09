use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use work_supervision_journal::Digest;

use crate::StoreError;

/// Content-addressed store of the texts and evidence a journal entry refers to.
///
/// The journal carries digests only, never free text (titles, briefs,
/// criteria, notes, summaries, reasons): they may hold personal data, and an
/// append-only hash chain cannot forget. Each blob is a file named by the
/// lowercase hexadecimal SHA-256 of its bytes, written to a temporary name,
/// synchronised, then renamed into place, so a reader never sees a partial
/// blob. Reads verify the digest.
#[derive(Debug, Clone)]
pub struct BlobStore {
    dir: PathBuf,
}

impl BlobStore {
    /// Opens the store rooted at `dir`, creating the directory if needed.
    ///
    /// # Errors
    ///
    /// [`StoreError::BlobIo`] when the directory cannot be created, or when
    /// `dir` exists and is not a directory.
    pub fn open(dir: &Path) -> Result<Self, StoreError> {
        match fs::symlink_metadata(dir) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Err(StoreError::BlobIo),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                fs::create_dir_all(dir).map_err(|_| StoreError::BlobIo)?;
            }
            Err(_) => return Err(StoreError::BlobIo),
        }
        Ok(Self {
            dir: dir.to_owned(),
        })
    }

    /// Stores `bytes` and returns their digest; storing the same bytes twice is a no-op.
    ///
    /// # Errors
    ///
    /// [`StoreError::BlobIo`] when the blob cannot be written, and
    /// [`StoreError::BlobCorrupt`] when a blob of the same name already exists
    /// with other bytes.
    pub fn put(&self, bytes: &[u8]) -> Result<Digest, StoreError> {
        let digest = Digest::of(bytes);
        let path = self.path_of(&digest);
        match self.get(&digest) {
            Ok(_) => return Ok(digest),
            Err(StoreError::BlobIo) if !path.exists() => {}
            Err(error) => return Err(error),
        }
        let temporary = self.dir.join(format!(".{}.partial", digest.to_hex()));
        // A partial file can only be the residue of an interrupted put.
        match fs::remove_file(&temporary) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => return Err(StoreError::BlobIo),
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| StoreError::BlobIo)?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| StoreError::BlobIo)?;
        drop(file);
        fs::rename(&temporary, &path).map_err(|_| StoreError::BlobIo)?;
        File::open(&self.dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| StoreError::BlobIo)?;
        Ok(digest)
    }

    /// Stores a UTF-8 text.
    ///
    /// # Errors
    ///
    /// As [`BlobStore::put`].
    pub fn put_text(&self, text: &str) -> Result<Digest, StoreError> {
        self.put(text.as_bytes())
    }

    /// Reads the blob named `digest` and checks that its bytes hash to it.
    ///
    /// # Errors
    ///
    /// [`StoreError::BlobIo`] when it cannot be read (including when it is
    /// absent), [`StoreError::BlobCorrupt`] when its bytes do not match.
    pub fn get(&self, digest: &Digest) -> Result<Vec<u8>, StoreError> {
        let path = self.path_of(digest);
        let metadata = fs::symlink_metadata(&path).map_err(|_| StoreError::BlobIo)?;
        if !metadata.file_type().is_file() {
            return Err(StoreError::BlobCorrupt);
        }
        let bytes = fs::read(&path).map_err(|_| StoreError::BlobIo)?;
        if Digest::of(&bytes) == *digest {
            Ok(bytes)
        } else {
            Err(StoreError::BlobCorrupt)
        }
    }

    /// Reads a text blob.
    ///
    /// # Errors
    ///
    /// As [`BlobStore::get`]; [`StoreError::BlobCorrupt`] when it is not UTF-8.
    pub fn get_text(&self, digest: &Digest) -> Result<String, StoreError> {
        String::from_utf8(self.get(digest)?).map_err(|_| StoreError::BlobCorrupt)
    }

    /// Whether a blob named `digest` is present (its bytes are not checked).
    #[must_use]
    pub fn contains(&self, digest: &Digest) -> bool {
        self.path_of(digest).is_file()
    }

    /// File that holds, or would hold, the blob named `digest`.
    #[must_use]
    pub fn path_of(&self, digest: &Digest) -> PathBuf {
        self.dir.join(digest.to_hex())
    }
}
