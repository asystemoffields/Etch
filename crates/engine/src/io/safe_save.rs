//! Sibling temporary files owned by one save. The destination changes only after preparation
//! succeeds; concurrent saves have separate files and the last successful replacement wins.
//!
//! A save through a symbolic link updates the file the link leads to and leaves the link a link:
//! the temporary file is made beside that file and renamed over it. A replaced file keeps its
//! mode, its extended attributes (Linux and macOS; on Linux these carry POSIX ACLs too) and, where
//! the user may set them, its owner and group. Not kept: macOS ACLs and Windows security
//! descriptors (the new file takes its folder's inherited ones), and the file's creation time.
//! A read-only file is not replaced ([`ReadOnly`]), nor is anything that isn't a regular file.
//!
//! When the folder refuses the temporary file (macOS privacy protection, #2138, or a read-only
//! folder), an existing destination is never written in place: it is left exactly as it was and
//! the error says to use Save As to another folder or to give WordCraft access to this one. Only
//! a destination that doesn't exist yet, so has no previous version to lose, is created directly;
//! a failed direct write removes it again.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The destination is a read-only file: a save refuses to replace it (as Word does) and suggests
/// Save As. Carried inside the [`io::Error`] (`kind` `PermissionDenied`), see [`is_read_only`].
#[derive(Debug)]
pub(super) struct ReadOnly;

impl std::fmt::Display for ReadOnly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the file is read-only; use Save As to save your changes under another name")
    }
}

impl std::error::Error for ReadOnly {}

/// Whether `e` is a refused save over a read-only file.
pub(super) fn is_read_only(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<ReadOnly>())
}

/// Links followed at most, as the system's own limit (`ELOOP`).
const MAX_LINKS: usize = 40;

/// The file a save to `path` replaces: `path` itself, or, when it is a symbolic link, the file at
/// the end of its chain of links (which may not exist yet).
pub(super) fn target(path: &Path) -> io::Result<PathBuf> {
    let mut at = path.to_path_buf();
    for _ in 0..MAX_LINKS {
        match fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let next = fs::read_link(&at)?;
                // A relative link is relative to the folder holding it.
                at = match at.parent() {
                    Some(parent) if next.is_relative() => parent.join(next),
                    _ => next,
                };
            }
            Ok(_) => return Ok(at),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(at),
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("too many levels of symbolic links"))
}

pub(super) fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    prepare(path, bytes)?.commit()
}

/// The regular file a save to `destination` (links followed) replaces, if there is one. Anything
/// else there is refused with a plain error, and so is a read-only file ([`ReadOnly`]).
fn existing(destination: &Path) -> io::Result<Option<fs::Metadata>> {
    let meta = match fs::metadata(destination) {
        Ok(meta) if meta.is_file() => meta,
        Ok(meta) if meta.is_dir() => return Err(io::Error::new(io::ErrorKind::IsADirectory, "that is a folder; choose a file name to save to")),
        Ok(_) => return Err(io::Error::new(io::ErrorKind::InvalidInput, "that is not a regular file; choose another file name to save to")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if meta.permissions().readonly() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, ReadOnly));
    }
    Ok(Some(meta))
}

/// A completed sibling replacement, still owned until its destination is checked and committed.
pub(super) fn prepare(path: &Path, bytes: &[u8]) -> io::Result<Temporary> {
    prepare_unexplained(path, bytes).map_err(explain)
}

fn prepare_unexplained(path: &Path, bytes: &[u8]) -> io::Result<Temporary> {
    let destination = target(path)?;
    let existing = existing(&destination)?;
    // A new document gets the usual mode (0666 less the umask); a replaced one keeps its own,
    // and its temporary file stays private until that mode is applied.
    let private = existing.is_some();
    let mut temp = Temporary::open(&destination, private)?;
    {
        let file = temp.file()?;
        #[cfg(not(test))]
        file.write_all(bytes)?;
        #[cfg(test)]
        super::fault::write_all(&destination, file, bytes)?;
        if let Some(meta) = &existing {
            keep_owner(file, meta);
            keep_mode(&destination, file.set_permissions(meta.permissions()))?;
            // After the mode: on Linux an ACL travels as an extended attribute and sets it again.
            keep_xattrs(&destination, file);
        }
        // Synchronize the prepared file before replacing the destination. On macOS and iOS the
        // standard library's `sync_all` is `fcntl(F_FULLFSYNC)`, which also flushes the drive's
        // own cache (a plain `fsync` there doesn't), so no extra call is needed.
        #[cfg(test)]
        super::fault::at(&destination, super::fault::Step::SyncFile)?;
        file.sync_all()?;
        trace("sync file");
    }
    Ok(temp)
}

/// The outcome of giving the prepared file the replaced file's mode. A folder that keeps no Unix
/// modes refuses to set one with "operation not supported": a NAS share mounted by GVFS (`smb://`
/// in the Linux file manager) refuses every mode change (#3003). There is no mode to keep there,
/// so the save goes on and the refusal is logged; any other failure still stops the save, and the
/// original stays as it was.
fn keep_mode(destination: &Path, set: io::Result<()>) -> io::Result<()> {
    match set {
        Err(e) if not_supported(&e) => {
            log::warn!("{}: the folder refuses a mode change ({e}); saved without one", destination.display());
            Ok(())
        }
        set => set,
    }
}

/// Is `e` "operation not supported"? `ENOTSUP` and `EOPNOTSUPP` by number, which the standard
/// library doesn't class as [`io::ErrorKind::Unsupported`] (only `ENOSYS` is), plus that kind.
fn not_supported(e: &io::Error) -> bool {
    const CODES: &[i32] = if cfg!(any(target_os = "linux", target_os = "android")) {
        &[95]
    } else if cfg!(any(target_os = "macos", target_os = "ios")) {
        &[45, 102]
    } else if cfg!(target_os = "openbsd") {
        &[45, 91]
    } else if cfg!(target_os = "netbsd") {
        &[45, 86]
    } else if cfg!(any(target_os = "freebsd", target_os = "dragonfly")) {
        &[45]
    } else {
        &[]
    };
    e.kind() == io::ErrorKind::Unsupported || e.raw_os_error().is_some_and(|code| CODES.contains(&code))
}

/// Make a rename in the folder `dir` durable: on Unix a renamed file's new name is only on disk
/// once its folder is synchronized, so after a power loss the save could otherwise be gone (the
/// folder still naming the old file) although it had been reported as saved. Windows and the web
/// have no folder handle to synchronize; NTFS journals the rename itself.
///
/// Some file systems (some network and FUSE mounts) can't synchronize a folder and say so with
/// `EINVAL` or "unsupported": that is not a failed save, the file is already in place.
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
        #[cfg(test)]
        super::fault::at(dir, super::fault::Step::SyncDir)?;
        match File::open(dir).and_then(|d| d.sync_all()) {
            Ok(()) => {}
            Err(e) if matches!(e.kind(), io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported) => {
                log::debug!("save: {} can't be synchronized: {e}", dir.display());
            }
            Err(e) => return Err(e),
        }
        trace("sync dir");
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

// The order of the durable steps of a save, recorded per thread for tests.
#[cfg(test)]
thread_local! {
    static TRACE: std::cell::RefCell<Vec<&'static str>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn trace(step: &'static str) {
    TRACE.with(|t| t.borrow_mut().push(step));
}

#[cfg(not(test))]
fn trace(_: &'static str) {}

/// Give the replacement the replaced file's owner and group, where the user may (root; a group
/// the user is in). Otherwise it keeps the user's own, as a copy would.
#[cfg(unix)]
fn keep_owner(file: &File, original: &fs::Metadata) {
    use std::os::unix::fs::MetadataExt;
    let Ok(own) = file.metadata() else { return };
    if own.uid() == original.uid() && own.gid() == original.gid() {
        return;
    }
    // Owner and group together, else the group alone (allowed for a group the user is in).
    if std::os::unix::fs::fchown(file, Some(original.uid()), Some(original.gid())).is_err()
        && own.gid() != original.gid()
        && let Err(e) = std::os::unix::fs::fchown(file, None, Some(original.gid()))
    {
        log::debug!("save: the file's group is not kept: {e}");
    }
}

#[cfg(not(unix))]
fn keep_owner(_: &File, _: &fs::Metadata) {}

/// The most extended attribute names and value bytes copied.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const MAX_XATTR_BYTES: usize = 1 << 20;

/// Copy the extended attributes of `original` to `file`, as far as the file system and the
/// user's rights allow (one that can't be copied is left out; the save never fails for it).
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn keep_xattrs(original: &Path, file: &File) {
    let Ok(size) = rustix::fs::listxattr(original, &mut [0u8; 0][..]) else { return };
    if size == 0 || size > MAX_XATTR_BYTES {
        return;
    }
    let mut names = vec![0u8; size];
    let Ok(n) = rustix::fs::listxattr(original, &mut names[..]) else { return };
    let names = names.get(..n).unwrap_or_default();
    let mut budget = MAX_XATTR_BYTES;
    for name in names.split(|b| *b == 0).filter(|n| !n.is_empty()) {
        let Ok(name) = std::ffi::CString::new(name) else { continue };
        let Ok(len) = rustix::fs::getxattr(original, name.as_c_str(), &mut [0u8; 0][..]) else { continue };
        if len > budget {
            log::debug!("save: extended attributes over {MAX_XATTR_BYTES} bytes are not kept");
            return;
        }
        budget -= len;
        let mut value = vec![0u8; len];
        let Ok(got) = rustix::fs::getxattr(original, name.as_c_str(), &mut value[..]) else { continue };
        let value = value.get(..got).unwrap_or_default();
        if let Err(e) = rustix::fs::fsetxattr(file, name.as_c_str(), value, rustix::fs::XattrFlags::empty()) {
            log::debug!("save: extended attribute {name:?} not kept: {e}");
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn keep_xattrs(_: &Path, _: &File) {}

#[cfg(test)]
fn replace_with(path: &Path, private: bool, prepare: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
    let mut temp = Temporary::open(path, private)?;
    prepare(temp.file()?)?;
    temp.commit()
}

/// [`Temporary::direct`] then `prepare`, committed only when it succeeds (dropping it removes the
/// incomplete new file).
#[cfg(test)]
fn create_directly(path: &Path, refused: io::Error, prepare: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
    let mut temp = Temporary::direct(path, refused)?;
    prepare(temp.file()?)?;
    temp.commit()
}

/// A save refused for want of folder access, already worded with what to do about it.
#[derive(Debug)]
struct Guided(String);

impl std::fmt::Display for Guided {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Guided {}

/// `e`, then `why`, then how to save anyway: Save As to another folder, or access to this one.
fn guided(e: io::Error, why: &str) -> io::Error {
    let access = if cfg!(target_os = "macos") {
        "allow Etch to use this folder in System Settings › Privacy & Security › Files and Folders"
    } else {
        "get write access to this folder"
    };
    io::Error::new(e.kind(), Guided(format!("{e}. {why} Use Save As to save the document in another folder, or {access}, then save again.")))
}

/// A permission error on macOS usually means its privacy protection keeps WordCraft out of the
/// folder; say where to allow it (errors from [`guided`] already do).
fn explain(e: io::Error) -> io::Error {
    // A refused read-only file keeps its own error (and wording), which callers recognise.
    let worded = e.get_ref().is_some_and(|inner| inner.is::<Guided>() || inner.is::<ReadOnly>());
    if cfg!(target_os = "macos") && e.kind() == io::ErrorKind::PermissionDenied && !worded {
        return io::Error::new(
            e.kind(),
            format!(
                "{e}. macOS may be blocking Etch from this folder: allow it in System Settings › Privacy & Security › Files and Folders, or save somewhere else"
            ),
        );
    }
    e
}

pub(super) struct Temporary {
    path: PathBuf,
    /// The file it replaces once committed (links already followed, [`target`]).
    destination: PathBuf,
    file: Option<File>,
    committed: bool,
    /// The new file is written at `destination` itself (#2138: the folder refused a temporary
    /// file and nothing was there yet), so committing renames nothing and an uncommitted one is
    /// removed: it is this save's own incomplete file, never a previous version.
    direct: bool,
    /// For a direct file: what it was when this save created it ([`same_file`]), so cleanup
    /// never removes a file someone else put at the destination since.
    created: Option<fs::Metadata>,
}

impl Temporary {
    /// `private`: create the file readable by its owner only (0600) rather than with the default
    /// mode (0666 less the umask).
    fn create(destination: &Path, private: bool) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        if destination.file_name().is_none() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "save destination needs a file name"));
        }
        // A short name also works beside a destination already at the filename-length limit.
        for _ in 0..128 {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = destination.with_file_name(format!(".etch-save-{}-{id}.tmp", std::process::id()));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            if private {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            #[cfg(not(unix))]
            let _ = private;
            #[cfg(test)]
            super::fault::at(destination, super::fault::Step::Create)?;
            match options.open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        destination: destination.to_path_buf(),
                        file: Some(file),
                        committed: false,
                        direct: false,
                        created: None,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "could not create a save temporary file"))
    }

    /// A sibling temporary file for `destination`, or, where the folder refuses one (macOS only
    /// lets an app write the file picked in the Save panel inside a protected folder such as
    /// Desktop, Documents, Downloads or iCloud Drive, #2138; a read-only folder refuses it too),
    /// the destination itself when it doesn't exist yet ([`Temporary::direct`]).
    fn open(destination: &Path, private: bool) -> io::Result<Self> {
        match Self::create(destination, private) {
            Ok(temp) => Ok(temp),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Self::direct(destination, e),
            Err(e) => Err(e),
        }
    }

    /// No temporary file may be created beside `destination` (`refused` says why). A file that
    /// doesn't exist yet is created directly (there is no previous version to lose) and removed
    /// again unless the save commits. Anything already at `destination` is left untouched:
    /// overwriting it in place could destroy the only copy, so the error says how to save safely.
    fn direct(destination: &Path, refused: io::Error) -> io::Result<Self> {
        // Tests: an injected refusal stands for a folder that refuses every new file (a fault plan
        // only fails the temporary file's creation itself).
        #[cfg(test)]
        if super::fault::is_injected(&refused, super::fault::Step::Create) {
            let why = if fs::symlink_metadata(destination).is_ok() {
                "This folder doesn't let Etch create the temporary file a safe save needs, so the existing file was left unchanged."
            } else {
                "Etch can't create files in this folder."
            };
            return Err(guided(refused, why));
        }
        // `create_new` never opens an existing file or follows a link, so it can't touch one.
        match OpenOptions::new().write(true).create_new(true).open(destination) {
            Ok(file) => {
                let created = file.metadata().ok();
                Ok(Self {
                    path: destination.to_path_buf(),
                    destination: destination.to_path_buf(),
                    file: Some(file),
                    committed: false,
                    direct: true,
                    created,
                })
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(guided(
                refused,
                "This folder doesn't let Etch create the temporary file a safe save needs, so the existing file was left unchanged.",
            )),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(guided(e, "Etch can't create files in this folder.")),
            Err(e) => Err(e),
        }
    }

    fn file(&mut self) -> io::Result<&mut File> {
        self.file.as_mut().ok_or_else(|| io::Error::other("save temporary file is closed"))
    }

    pub(super) fn commit(mut self) -> io::Result<()> {
        // Close before replacement, including on Windows.
        drop(self.file.take());
        if self.direct {
            // Already written and synchronized at the destination itself.
            self.committed = true;
        } else {
            #[cfg(test)]
            super::fault::at(&self.destination, super::fault::Step::Rename)?;
            fs::rename(&self.path, &self.destination).map_err(explain)?;
            self.committed = true;
            trace("rename");
        }
        // The new name is durable only once the folder is (a power loss could otherwise bring
        // back the old file after the save was reported). The file is in place by now: a folder
        // that fails to synchronize (a failing disk) is logged, not reported as a failed save,
        // which would leave the session expecting the old file and refuse the next save.
        if let Err(e) = sync_dir(self.destination.parent().unwrap_or(Path::new("."))) {
            log::error!("save: {} was written, but its folder couldn't be synchronized to disk: {e}", self.destination.display());
        }
        Ok(())
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        let file = self.file.take();
        if self.committed {
            return;
        }
        if !self.direct {
            drop(file);
            // If filesystem permissions change, cleanup can fail too. Never remove a neighbour
            // or the destination to compensate for a failed save. (The temporary file's name is
            // this save's own.)
            let _ = fs::remove_file(&self.path);
            return;
        }
        // A direct file is this save's own new, incomplete file at a destination that didn't
        // exist before it: removed, unless what is there now is another file (a sync client or
        // another program replaced it meanwhile), which is left alone.
        let ours = file.as_ref().and_then(|f| f.metadata().ok()).or_else(|| self.created.clone());
        drop(file);
        let still_ours = match (ours, fs::symlink_metadata(&self.path)) {
            (Some(ours), Ok(now)) => same_file(&ours, &now),
            _ => false,
        };
        if !still_ours {
            log::warn!("save: {} is no longer the file this save created; left as it is", self.path.display());
            return;
        }
        if let Err(e) = fs::remove_file(&self.path) {
            log::error!("save: removing the incomplete new file {} failed: {e}", self.path.display());
        }
    }
}

/// Whether `a` (an open file's metadata) and `b` (a path's, links not followed) are the same file:
/// device and inode on Unix; elsewhere the same regular file length and modification time.
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        b.is_file() && a.dev() == b.dev() && a.ino() == b.ino()
    }
    #[cfg(not(unix))]
    {
        b.is_file() && a.len() == b.len() && a.modified().ok().is_some_and(|m| b.modified().ok() == Some(m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path =
                std::env::temp_dir().join(format!("wordcraft-save-owner-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn target(&self) -> PathBuf {
            self.0.join("original.docx")
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// #3003 (upstream fdc325fa): a folder that refuses mode changes (a GVFS-mounted NAS share)
    /// still takes the save; a mode change refused for any other reason still stops it and keeps
    /// the original.
    #[test]
    fn a_folder_without_unix_modes_still_takes_the_save() {
        let d = Scratch::new();
        let path = d.target();
        fs::write(&path, b"Original bytes.").unwrap();
        replace_with(&path, true, |file| {
            file.write_all(b"Saved on the share.")?;
            keep_mode(&path, Err(io::Error::from(io::ErrorKind::Unsupported)))?;
            file.sync_all()
        })
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"Saved on the share.");
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 1, "no temporary file left");

        let err = replace_with(&path, true, |file| {
            file.write_all(b"Refused.")?;
            keep_mode(&path, Err(io::Error::from(io::ErrorKind::PermissionDenied)))
        })
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(&path).unwrap(), b"Saved on the share.", "the original is kept");
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 1);
    }

    /// GVFS reports a refused mode change as `ENOTSUP`, which the standard library leaves
    /// uncategorized; other OS errors still stop the save.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn enotsup_from_the_os_is_an_unsupported_mode_change() {
        let enotsup = if cfg!(target_os = "linux") { 95 } else { 45 };
        assert!(not_supported(&io::Error::from_raw_os_error(enotsup)));
        assert!(keep_mode(Path::new("share.docx"), Err(io::Error::from_raw_os_error(enotsup))).is_ok());
        let eacces = 13;
        assert!(!not_supported(&io::Error::from_raw_os_error(eacces)));
        assert_eq!(keep_mode(Path::new("share.docx"), Err(io::Error::from_raw_os_error(eacces))).unwrap_err().raw_os_error(), Some(eacces));
        assert!(keep_mode(Path::new("share.docx"), Ok(())).is_ok());
    }

    /// A save synchronizes the new file before renaming it over the old one, and then (Unix) the
    /// folder, so the new name survives a power loss: both before the save is reported.
    #[test]
    fn a_save_syncs_the_file_then_renames_then_syncs_the_folder() {
        let d = Scratch::new();
        let path = d.target();
        fs::write(&path, b"Original bytes.").unwrap();
        TRACE.with(|t| t.borrow_mut().clear());
        write(&path, b"Saved bytes.").unwrap();
        let steps = TRACE.with(|t| t.borrow().clone());
        #[cfg(unix)]
        assert_eq!(steps, ["sync file", "rename", "sync dir"]);
        #[cfg(not(unix))]
        assert_eq!(steps, ["sync file", "rename"]);
        assert_eq!(fs::read(&path).unwrap(), b"Saved bytes.");
        // A folder that is gone can't be synchronized: an error, not a silent success.
        #[cfg(unix)]
        assert!(sync_dir(&d.0.join("missing")).is_err());
    }

    #[test]
    fn partial_and_complete_preparation_failures_leave_original_and_no_owned_temp() {
        for partial in [true, false] {
            let d = Scratch::new();
            let path = d.target();
            fs::write(&path, b"Original bytes.").unwrap();
            let err = replace_with(&path, true, |file| {
                file.write_all(if partial { b"Partial" } else { b"Complete replacement bytes." })?;
                Err(io::Error::other("injected preparation failure"))
            })
            .unwrap_err();
            assert!(err.to_string().contains("injected"));
            assert_eq!(fs::read(&path).unwrap(), b"Original bytes.");
            assert_eq!(fs::read_dir(&d.0).unwrap().count(), 1);
        }
    }

    #[test]
    fn overlapping_prepared_saves_keep_separate_bytes_and_commit_complete_files() {
        let d = Scratch::new();
        let path = d.target();
        fs::write(&path, b"Original bytes.").unwrap();
        let mut first = Temporary::create(&path, true).unwrap();
        let mut second = Temporary::create(&path, true).unwrap();
        assert_ne!(first.path, second.path);
        first.file().unwrap().write_all(b"First complete save.").unwrap();
        second.file().unwrap().write_all(b"Second complete save.").unwrap();
        first.file().unwrap().sync_all().unwrap();
        second.file().unwrap().sync_all().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"Original bytes.");
        first.commit().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"First complete save.");
        second.commit().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"Second complete save.");
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 1);
    }

    /// Exports (`write_file`) replace the destination instead of truncating it in place: another
    /// hard link to the old file keeps the old bytes, and no temporary file is left behind.
    #[test]
    fn write_file_replaces_the_destination_instead_of_truncating_it() {
        let d = Scratch::new();
        let path = d.target();
        let alias = d.0.join("alias.docx");
        fs::write(&path, b"Original bytes.").unwrap();
        fs::hard_link(&path, &alias).unwrap();
        crate::io::write_file(&path.to_string_lossy(), b"Exported bytes.").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"Exported bytes.");
        assert_eq!(fs::read(&alias).unwrap(), b"Original bytes.");
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 2);
    }

    /// Makes `dir` read-only while `f` runs. `None` when running as root, where the folder isn't
    /// read-only to us.
    #[cfg(unix)]
    fn in_read_only<T>(dir: &Path, f: impl FnOnce() -> T) -> Option<T> {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
        let probe = fs::write(dir.join("probe"), b"");
        let out = f();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        probe.is_err().then_some(out)
    }

    /// #2138 review: where no sibling temporary file may be created (macOS privacy protection,
    /// or a read-only folder), an existing document is left byte for byte as it was, and the
    /// error says to use Save As or get access to the folder.
    #[cfg(unix)]
    #[test]
    fn an_existing_file_in_a_folder_refusing_the_temporary_file_is_left_untouched() {
        let d = Scratch::new();
        let path = d.target();
        fs::write(&path, b"Original bytes.").unwrap();
        let Some(saved) = in_read_only(&d.0, || write(&path, b"Saved bytes.")) else { return };
        let err = saved.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        let message = err.to_string();
        assert!(message.contains("left unchanged") && message.contains("Save As") && message.contains("this folder"), "{message}");
        assert_eq!(message.matches("Privacy & Security").count(), usize::from(cfg!(target_os = "macos")), "{message}");
        assert_eq!(fs::read(&path).unwrap(), b"Original bytes.");
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 1);
    }

    /// FredApps' #2138 regression: a save that would fail partway through still leaves the
    /// previous document byte-identical when the folder refuses the temporary file.
    #[cfg(unix)]
    #[test]
    fn a_failed_save_in_a_folder_refusing_the_temporary_file_keeps_the_original() {
        let d = Scratch::new();
        let path = d.target();
        fs::write(&path, b"Original bytes.").unwrap();
        let Some(result) = in_read_only(&d.0, || {
            replace_with(&path, true, |file| {
                file.write_all(b"Partial")?;
                Err(io::Error::other("injected partial write failure"))
            })
        }) else {
            return;
        };
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"Original bytes.", "failed save must preserve the previous document");
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 1);
    }

    /// A folder that refuses new files altogether refuses a new document too, with the same
    /// guidance, and leaves nothing behind.
    #[cfg(unix)]
    #[test]
    fn a_new_file_in_a_read_only_folder_is_refused_with_guidance() {
        let d = Scratch::new();
        let path = d.target();
        let Some(saved) = in_read_only(&d.0, || write(&path, b"New document.")) else { return };
        let message = saved.unwrap_err().to_string();
        assert!(message.contains("Save As"), "{message}");
        assert!(!path.exists());
    }

    /// When only the temporary file is refused (macOS lets the app write the file picked in the
    /// Save panel), a file that doesn't exist yet is created directly; nothing is replaced.
    #[test]
    fn a_new_file_is_created_directly_when_only_the_temporary_file_is_refused() {
        let d = Scratch::new();
        let path = d.target();
        let refused = io::Error::from(io::ErrorKind::PermissionDenied);
        create_directly(&path, refused, |file| file.write_all(b"New document.")).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"New document.");
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 1);
    }

    /// A direct save that doesn't commit removes its new file only while it is still the file it
    /// created: one put in its place meanwhile (a sync client, another program) is left alone.
    #[cfg(unix)]
    #[test]
    fn an_uncommitted_direct_save_leaves_a_file_put_in_its_place() {
        let d = Scratch::new();
        let path = d.target();
        let mut temp = Temporary::direct(&path, io::Error::from(io::ErrorKind::PermissionDenied)).unwrap();
        temp.file().unwrap().write_all(b"Partial").unwrap();
        // Another program replaces the file (a new inode at the same name).
        let other = d.0.join("other.docx");
        fs::write(&other, b"Someone else's file.").unwrap();
        fs::rename(&other, &path).unwrap();
        drop(temp);
        assert_eq!(fs::read(&path).unwrap(), b"Someone else's file.");
        // Untouched, the incomplete file goes as before.
        let path2 = d.0.join("new.docx");
        let mut temp = Temporary::direct(&path2, io::Error::from(io::ErrorKind::PermissionDenied)).unwrap();
        temp.file().unwrap().write_all(b"Partial").unwrap();
        drop(temp);
        assert!(!path2.exists());
    }

    #[test]
    fn a_failed_direct_save_of_a_new_file_leaves_nothing() {
        let d = Scratch::new();
        let path = d.target();
        let refused = io::Error::from(io::ErrorKind::PermissionDenied);
        let err = create_directly(&path, refused, |file| {
            file.write_all(b"Partial")?;
            Err(io::Error::other("injected"))
        })
        .unwrap_err();
        assert!(err.to_string().contains("injected"));
        assert!(!path.exists());
    }

    /// The direct path never opens an existing file and never runs the save: the original is
    /// untouched and the error carries the Save As guidance.
    #[test]
    fn an_existing_file_is_never_written_directly() {
        let d = Scratch::new();
        let path = d.target();
        fs::write(&path, b"Original bytes.").unwrap();
        let refused = io::Error::from(io::ErrorKind::PermissionDenied);
        let mut ran = false;
        let err = create_directly(&path, refused, |file| {
            ran = true;
            file.write_all(b"New")
        })
        .unwrap_err();
        assert!(!ran);
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("Save As") && err.to_string().contains("left unchanged"), "{err}");
        assert_eq!(fs::read(&path).unwrap(), b"Original bytes.");
    }

    #[cfg(unix)]
    #[test]
    fn new_temporary_files_are_private_until_prepared() {
        use std::os::unix::fs::PermissionsExt;
        let d = Scratch::new();
        let temp = Temporary::create(&d.target(), true).unwrap();
        assert_eq!(fs::metadata(&temp.path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    /// A new document gets the default mode, as `fs::write` would give it — not 0600; a replaced
    /// document keeps its own mode.
    #[cfg(unix)]
    #[test]
    fn new_documents_get_the_default_mode_and_replaced_ones_keep_theirs() {
        use std::os::unix::fs::PermissionsExt;
        let d = Scratch::new();
        let reference = d.0.join("reference.docx");
        fs::write(&reference, b"x").unwrap();
        let default_mode = fs::metadata(&reference).unwrap().permissions().mode() & 0o777;
        let path = d.target();
        write(&path, b"New document.").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, default_mode);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        write(&path, b"Saved again.").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640);
        assert_eq!(fs::read(&path).unwrap(), b"Saved again.");
    }

    /// A save replaces the file, which dropped its extended attributes (a tag, a "where from"
    /// note, on Linux a POSIX ACL). They are copied to the replacement. Skipped where the file
    /// system has no user attributes.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn extended_attributes_survive_a_save() {
        let d = Scratch::new();
        let path = d.target();
        fs::write(&path, b"Original bytes.").unwrap();
        if let Err(e) = rustix::fs::setxattr(&path, "user.wordcraft.test", b"synthetic tag", rustix::fs::XattrFlags::empty()) {
            eprintln!("skipped: this file system has no user extended attributes: {e}");
            return;
        }
        write(&path, b"Saved again.").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"Saved again.");
        let mut value = [0u8; 64];
        let n = rustix::fs::getxattr(&path, "user.wordcraft.test", &mut value[..]).unwrap();
        assert_eq!(&value[..n], b"synthetic tag");
    }

    /// A save through a symbolic link writes the file at its end; the link stays a link.
    #[cfg(unix)]
    #[test]
    fn a_save_through_a_link_replaces_its_target() {
        let d = Scratch::new();
        let path = d.target();
        fs::write(&path, b"Original bytes.").unwrap();
        let link = d.0.join("link.docx");
        std::os::unix::fs::symlink("original.docx", &link).unwrap();
        assert_eq!(target(&link).unwrap(), path);
        write(&link, b"Through the link.").unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&path).unwrap(), b"Through the link.");
        // A loop of links is an error, not a hang.
        let a = d.0.join("a");
        let b = d.0.join("b");
        std::os::unix::fs::symlink(&b, &a).unwrap();
        std::os::unix::fs::symlink(&a, &b).unwrap();
        assert!(write(&a, b"x").is_err());
    }
}
