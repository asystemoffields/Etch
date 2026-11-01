//! Content identity for a session's native file. No polling or typing-path filesystem work.
use super::FileConflict;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The largest file compared: the largest one opened.
const MAX_BYTES: u64 = super::MAX_OPEN_BYTES;

/// The identity of the file a session opened or last saved (#813): what a later save expects to
/// find there. Opaque; cheap to clone.
#[derive(Clone, Debug)]
pub struct FileVersion {
    path: PathBuf,
    resolved: PathBuf,
    handle: Option<Arc<same_file::Handle>>,
    /// The content's length: a file of another length changed without reading it.
    len: u64,
    digest: [u8; 32],
}

/// Keep the logical spelling: a retargeted parent symlink still names the session's file.
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Resolve aliases even when an externally deleted final entry no longer canonicalizes.
fn resolved_spelling(path: &Path) -> Option<PathBuf> {
    match (path.parent().and_then(|p| fs::canonicalize(p).ok()), path.file_name()) {
        (Some(parent), Some(name)) => Some(parent.join(name)),
        _ => None,
    }
}

fn regular(path: &Path) -> std::io::Result<File> {
    let meta = fs::metadata(path)?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Err(std::io::Error::other("expected a regular file no larger than 2 GB"));
    }
    let file = File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Err(std::io::Error::other("expected a regular file no larger than 2 GB"));
    }
    Ok(file)
}

impl FileVersion {
    /// The version of the file at `path` whose `bytes` were just read from `file`
    /// (`super::read_capped_file`, the one capped read every open goes through).
    pub(super) fn of_read(path: &Path, file: File, bytes: &[u8]) -> std::io::Result<Self> {
        Ok(Self {
            path: absolute(path),
            resolved: fs::canonicalize(path)?,
            handle: Some(Arc::new(same_file::Handle::from_file(file)?)),
            len: bytes.len() as u64,
            digest: Sha256::digest(bytes).into(),
        })
    }

    /// Path spellings, symlinks and hard links to the open file share its expected content.
    pub(crate) fn owns(&self, path: &Path) -> Result<bool, FileConflict> {
        let absolute = absolute(path);
        if absolute == self.path || absolute == self.resolved {
            return Ok(true);
        }
        if fs::canonicalize(path).ok().or_else(|| resolved_spelling(path)).is_some_and(|p| p == self.resolved) {
            return Ok(true);
        }
        // Not a regular file (a folder, a device), or nothing that can be looked at: not this
        // session's file, which was a regular file at a path we could read. The save itself then
        // says what is wrong with the destination, instead of "changed by someone else".
        match fs::metadata(path) {
            Ok(meta) if meta.is_file() => {}
            _ => return Ok(false),
        }
        let Some(original) = &self.handle else { return Err(FileConflict { path: path.to_path_buf() }) };
        // Unix: the file's identity (device and inode) comes from its metadata, so an alias
        // that can't be opened (no read permission) is still told apart from this file.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let (Ok(mine), Ok(theirs)) = (original.as_file().metadata(), fs::metadata(path)) else {
                return Err(FileConflict { path: path.to_path_buf() });
            };
            Ok(mine.dev() == theirs.dev() && mine.ino() == theirs.ino())
        }
        // same-file's Windows comparison includes size: refresh the retained file handle's
        // metadata so a changed-size hard link cannot bypass ownership. A file that can't be
        // opened isn't this one, which this session opened (its links share its permissions).
        #[cfg(not(unix))]
        {
            let Ok(original) = original.as_file().try_clone().and_then(same_file::Handle::from_file) else {
                return Err(FileConflict { path: path.to_path_buf() });
            };
            Ok(same_file::Handle::from_path(path).is_ok_and(|current| original == current))
        }
    }

    pub(super) fn verify(&self, path: &Path) -> Result<(), FileConflict> {
        let check = || -> std::io::Result<bool> {
            let file = regular(path)?;
            // Most changes change the length: refused without reading the file.
            if file.metadata()?.len() != self.len {
                return Ok(false);
            }
            let mut reader = BufReader::new(file).take(MAX_BYTES + 1);
            let mut digest = Sha256::new();
            let mut buffer = [0u8; 64 * 1024];
            let mut total = 0u64;
            loop {
                let n = reader.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                total += n as u64;
                if total > MAX_BYTES {
                    return Ok(false);
                }
                let Some(read) = buffer.get(..n) else { return Ok(false) };
                digest.update(read);
            }
            Ok(<[u8; 32]>::from(digest.finalize()) == self.digest)
        };
        if check().unwrap_or(false) { Ok(()) } else { Err(FileConflict { path: path.to_path_buf() }) }
    }

    /// Keep the identity of serialized bytes, never a second read that might capture an
    /// external writer's version. Handle lookup is best effort after a committed replacement.
    pub(super) fn saved(path: &Path, bytes: &[u8]) -> Self {
        Self {
            path: absolute(path),
            resolved: fs::canonicalize(path).unwrap_or_else(|_| absolute(path)),
            handle: same_file::Handle::from_path(path).ok().map(Arc::new),
            len: bytes.len() as u64,
            digest: Sha256::digest(bytes).into(),
        }
    }
}
