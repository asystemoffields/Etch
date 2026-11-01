//! Records on disk: the folder's records and their files, the per-record lock, the start-up
//! scan and the list it produces, and disk usage.
use super::files::{private_dir, private_file, tighten};
use super::format::{generation, id_ok, part_ok, pending_ok};
use super::read::{listing, older_than_file};
use super::{Entry, LOCK_RETRIES, MAX_FILES, MAX_PARTS, MAX_RECORDS, Store};
use crate::recovery::RecoveryInfo;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::Duration;

impl Store {
    pub(super) fn new(root: PathBuf) -> Result<Self, String> {
        private_dir(&root).map_err(|e| e.to_string())?;
        tighten(&root).map_err(|e| e.to_string())?;
        Ok(Self { root, entries: BTreeMap::new(), locks: BTreeMap::new(), secrets: BTreeMap::new(), hashes: HashMap::new(), written: 0 })
    }
    pub(super) fn directories(&self) -> Result<Vec<PathBuf>, String> {
        let mut dirs = Vec::new();
        for e in fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            if e.file_type().map_err(|e| e.to_string())?.is_dir() && e.file_name().to_str().is_some_and(id_ok) {
                if dirs.len() >= MAX_RECORDS {
                    return Err("recovery folder has too many records".into());
                }
                dirs.push(e.path());
            }
        }
        Ok(dirs)
    }
    pub(super) fn files(dir: &Path) -> Result<Vec<PathBuf>, String> {
        let mut paths = Vec::new();
        for e in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            if e.file_type().map_err(|e| e.to_string())?.is_file() && generation(&e.path()).is_some() {
                if paths.len() >= MAX_FILES {
                    return Err("recovery record has too many generations".into());
                }
                paths.push(e.path());
            }
        }
        paths.sort_by_key(|p| std::cmp::Reverse(generation(p)));
        Ok(paths)
    }
    /// A record's side files and their sizes.
    pub(super) fn parts(dir: &Path) -> Result<BTreeMap<String, u64>, String> {
        let mut parts = BTreeMap::new();
        for e in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            let Some(name) = e.file_name().to_str().filter(|n| part_ok(n)).map(str::to_string) else { continue };
            let meta = e.metadata().map_err(|e| e.to_string())?;
            if meta.is_file() {
                if parts.len() >= MAX_PARTS {
                    return Err("recovery record has too many parts".into());
                }
                parts.insert(name, meta.len());
            }
        }
        Ok(parts)
    }
    /// Locks a record's `owner.lock`, retrying [`LOCK_RETRIES`] times 10 ms apart.
    fn lock(&self, id: &str) -> Result<File, String> {
        if !id_ok(id) {
            return Err("invalid recovery id".into());
        }
        let file = private_file(&self.root.join(id).join("owner.lock"), false).map_err(|e| e.to_string())?;
        // A process another part of the app (or a test) is starting holds the lock for a moment
        // (a forked child shares it until it execs): wait that out, but not a live owner.
        for _ in 0..LOCK_RETRIES {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(format!("recovery copy is in use or cannot be locked: {e}")),
            }
        }
        file.try_lock().map_err(|e| format!("recovery copy is in use or cannot be locked: {e}"))?;
        Ok(file)
    }
    pub(super) fn acquire(&mut self, id: &str) -> Result<(), String> {
        if !self.locks.contains_key(id) {
            let lock = self.lock(id)?;
            self.locks.insert(id.to_string(), lock);
        }
        Ok(())
    }
    /// List the records in the folder. A record that can't be read (too many generations, a
    /// file that can't be listed or cleaned) is skipped with a warning: the others are still
    /// listed, and the scan reports that some files could not be read.
    pub(super) fn scan(&mut self) -> Result<(), String> {
        let mut entries = BTreeMap::new();
        let mut unreadable = false;
        for dir in self.directories()? {
            let Some(id) = dir.file_name().and_then(|s| s.to_str()) else { continue };
            let id = id.to_string();
            match self.scan_record(&dir, &id) {
                Ok((entry, complete)) => {
                    unreadable |= !complete;
                    if let Some(entry) = entry {
                        entries.insert(id, entry);
                    }
                }
                Err(e) => {
                    unreadable = true;
                    log::warn!("recovery: skipped the record {}: {e}", dir.display());
                }
            }
        }
        self.entries = entries;
        if unreadable { Err("Some recovery files could not be read. Earlier valid copies are listed when available.".into()) } else { Ok(()) }
    }
    /// One record of [`Self::scan`]: its newest readable generation (`None`: none, or another
    /// instance holds it), and whether every generation tried could be read.
    fn scan_record(&mut self, dir: &Path, id: &str) -> Result<(Option<Entry>, bool), String> {
        let guard = if self.locks.contains_key(id) {
            None
        } else {
            match self.lock(id) {
                Ok(f) => Some(f),
                Err(e) => {
                    log::debug!("recovery: {e}");
                    return Ok((None, true));
                }
            }
        };
        // A crashed writer can leave an incomplete temporary file. Only clean records
        // we have locked, so another instance's write is never interrupted.
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map_err(|e| e.to_string())?.is_file() && entry.file_name().to_str().is_some_and(pending_ok) {
                fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
            }
        }
        let mut found = None;
        let mut complete = true;
        for path in Self::files(dir)? {
            match listing(&path) {
                Ok((mut info, generation_no, parts)) if info.id == id && Some(generation_no) == generation(&path) => {
                    // A protected generation this session wrote keeps the side files it knows.
                    let parts = parts.or_else(|| self.entries.get(id).filter(|e| e.path == path).and_then(|e| e.parts.clone()));
                    // A newer generation was damaged: this one is offered, and said to be earlier.
                    info.earlier = !complete;
                    info.older_than_file = older_than_file(&info, &path);
                    found = Some(Entry { info, path, parts });
                    break;
                }
                Ok(_) => {
                    complete = false;
                    log::warn!("recovery: mismatched record {}", path.display());
                }
                Err(e) => {
                    complete = false;
                    log::warn!("recovery: unreadable {}: {e}", path.display());
                }
            }
        }
        drop(guard);
        // A crash before the first commit leaves no document to recover. Reclaim
        // that slot only under a fresh lock and the discard path's entry checks.
        if Self::files(dir)?.is_empty()
            && let Err(e) = self.discard(id)
        {
            log::warn!("recovery: cannot remove empty record: {e}");
        }
        Ok((found, complete))
    }
    pub(super) fn inventory(&self) -> Vec<RecoveryInfo> {
        let mut v: Vec<_> = self.entries.values().map(|e| e.info.clone()).collect();
        v.sort_by(|a, b| b.saved_at.cmp(&a.saved_at).then(a.id.cmp(&b.id)));
        v
    }
    /// Bytes in the recovery folder's records (generations and side files), leaving out the
    /// record folder `except`.
    pub(super) fn disk_used(&self, except: Option<&Path>) -> Result<u64, String> {
        let mut used = 0u64;
        for dir in self.directories()? {
            if except == Some(dir.as_path()) {
                continue;
            }
            for p in Self::files(&dir)? {
                used = used.saturating_add(fs::metadata(p).map_err(|e| e.to_string())?.len());
            }
            used = used.saturating_add(Self::parts(&dir)?.values().sum());
        }
        Ok(used)
    }
}
