//! Private folders and files, durable writes and bounded reads: the operating-system
//! primitives the rest of the store is built on.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

pub(super) fn private_dir(path: &Path) -> io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path)
}
/// Make an existing recovery folder private too (`private_dir`'s mode applies only to folders it
/// creates): copies of unsaved documents must not be readable by other users.
pub(super) fn tighten(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::symlink_metadata(path)?;
        if !meta.is_dir() {
            return Err(io::Error::other("the recovery folder is not a folder"));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
pub(super) fn private_file(path: &Path, exclusive: bool) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.read(true).write(true);
    if exclusive {
        o.create_new(true);
    } else {
        o.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}
pub(super) fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(test)]
    crate::io::fault::at(path, crate::io::fault::Step::SyncDir)?;
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
/// A regular file's bytes, at most `cap` of them.
pub(super) fn read_bounded(path: &Path, cap: u64) -> Result<Vec<u8>, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > cap {
        return Err("not a regular recovery file, or larger than a recovery file may be".into());
    }
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.take(cap.saturating_add(1)).read_to_end(&mut out).map_err(|e| e.to_string())?;
    if out.len() as u64 > cap {
        return Err("recovery file grew while it was read".into());
    }
    Ok(out)
}
/// Does the file at `path` hold exactly `pieces`, one after the other? Compared a megabyte at a
/// time.
pub(super) fn reads_back(path: &Path, pieces: &[&[u8]]) -> Result<bool, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 20];
    for piece in pieces {
        let mut at = 0usize;
        while at < piece.len() {
            let want = (piece.len() - at).min(buf.len());
            let Some(chunk) = buf.get_mut(..want) else { return Ok(false) };
            let n = file.read(chunk).map_err(|e| e.to_string())?;
            if n == 0 {
                return Ok(false);
            }
            let end = at.saturating_add(n);
            if piece.get(at..end) != buf.get(..n) {
                return Ok(false);
            }
            at = end;
        }
    }
    // Nothing more.
    Ok(file.read(&mut buf).map_err(|e| e.to_string())? == 0)
}
