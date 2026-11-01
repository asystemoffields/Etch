//! Removing a record: only files the store itself writes, under the record's lock, then the folder.
use super::Store;
use super::files::sync_dir;
use super::format::{generation, id_ok, part_ok, pending_ok};
use std::fs;

impl Store {
    pub(super) fn discard(&mut self, id: &str) -> Result<(), String> {
        if !id_ok(id) {
            return Err("invalid recovery id".into());
        }
        let dir = self.root.join(id);
        if !dir.exists() {
            self.entries.remove(id);
            self.secrets.remove(id);
            return Ok(());
        }
        self.acquire(id)?;
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let known = name == "owner.lock" || generation(&entry.path()).is_some() || name.to_str().is_some_and(|n| pending_ok(n) || part_ok(n));
            if !known || !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                return Err("recovery folder contains an unexpected entry".into());
            }
        }
        for p in Self::files(&dir)? {
            fs::remove_file(p).map_err(|e| e.to_string())?;
        }
        for e in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            if e.file_name().to_str().is_some_and(|n| pending_ok(n) || part_ok(n)) {
                fs::remove_file(e.path()).map_err(|e| e.to_string())?;
            }
        }
        sync_dir(&dir).map_err(|e| e.to_string())?;
        self.entries.remove(id);
        self.secrets.remove(id);
        self.locks.remove(id);
        fs::remove_file(dir.join("owner.lock")).map_err(|e| e.to_string())?;
        fs::remove_dir(&dir).map_err(|e| e.to_string())?;
        sync_dir(&self.root).map_err(|e| e.to_string())?;
        Ok(())
    }
}
