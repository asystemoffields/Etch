//! Writing a generation: pictures and parts into side files within the budget, the text with
//! its checksum, then pruning what no retained generation uses.
use super::files::{private_dir, private_file, reads_back, sync_dir};
use super::format::{check_line, generation, hex, new_id, part_file, sealed_file_size, sealed_size, sha256};
use super::{
    Entry, Limits, MAX_FILES, MAX_RECORDS, PLAIN_MAGIC, PROTECTED_NAME, PartRef, Parts, SEALED_MAGIC, SealedHeader, Secret, Snapshot, Store,
    TEXT_MARGIN, VERSION,
};
use crate::io::Password;
use crate::recovery::RecoveryInfo;
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

struct Candidate {
    media: bool,
    name: String,
    bytes: Arc<Vec<u8>>,
    part: PartRef,
}
pub(super) struct Limited<W> {
    pub(super) inner: W,
    pub(super) remaining: u64,
}
impl<W: Write> Write for Limited<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(io::Error::other("recovery snapshot or folder size limit exceeded"));
        }
        let n = self.inner.write(bytes)?;
        self.remaining -= n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Store {
    /// SHA-256 of a picture, remembered while the picture is unchanged.
    fn hash(&mut self, bytes: &Arc<Vec<u8>>) -> [u8; 32] {
        let key = Arc::as_ptr(bytes) as usize;
        if let Some((weak, hash)) = self.hashes.get(&key)
            && weak.upgrade().is_some_and(|a| Arc::ptr_eq(&a, bytes))
        {
            return *hash;
        }
        let hash = sha256(bytes);
        self.hashes.insert(key, (Arc::downgrade(bytes), hash));
        hash
    }
    /// The salt of a protected record's side-file names: kept while the password stays the
    /// same, a new one (so every part is encrypted again) when it changes.
    fn salt_for(&mut self, id: &str, password: &Password) -> Result<[u8; 16], String> {
        if let Some(s) = self.secrets.get(id)
            && s.password == *password
        {
            return Ok(s.salt);
        }
        let mut salt = [0u8; 16];
        getrandom::fill(&mut salt).map_err(|e| e.to_string())?;
        self.secrets.insert(id.to_string(), Secret { password: password.clone(), salt });
        Ok(salt)
    }
    /// Write `pieces`, one after the other, to `target` in `dir`: exclusively to a temporary
    /// file, synchronized, read back and compared, then renamed into place.
    fn write_file(&mut self, dir: &Path, target: &Path, pieces: &[&[u8]]) -> Result<(), String> {
        let tmp = dir.join(format!("pending-{}.tmp", new_id()?));
        let result = (|| {
            // Tests fail these steps on demand (`crate::io::fault`).
            #[cfg(test)]
            crate::io::fault::at(target, crate::io::fault::Step::Create).map_err(|e| e.to_string())?;
            let mut file = private_file(&tmp, true).map_err(|e| e.to_string())?;
            #[cfg(not(test))]
            for piece in pieces {
                file.write_all(piece).map_err(|e| e.to_string())?;
            }
            // One write of the file for the seam, whatever its pieces.
            #[cfg(test)]
            crate::io::fault::write_all(target, &mut file, &pieces.concat()).map_err(|e| e.to_string())?;
            #[cfg(test)]
            crate::io::fault::at(target, crate::io::fault::Step::SyncFile).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            drop(file);
            let len = pieces.iter().map(|p| p.len() as u64).fold(0u64, u64::saturating_add);
            self.written = self.written.saturating_add(len);
            #[cfg(test)]
            crate::io::fault::at(target, crate::io::fault::Step::ReadBack).map_err(|e| e.to_string())?;
            if !reads_back(&tmp, pieces)? {
                return Err("recovery copy did not read back intact".into());
            }
            #[cfg(test)]
            crate::io::fault::at(target, crate::io::fault::Step::Rename).map_err(|e| e.to_string())?;
            fs::rename(&tmp, target).map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
    /// Save a generation of `s`; `Ok` lists the pictures and parts it couldn't keep.
    pub(super) fn save(
        &mut self,
        s: Snapshot,
        password: Option<&Password>,
        deadline: Option<Instant>,
        limits: Limits,
    ) -> Result<Vec<String>, String> {
        let id = s.info.id.clone();
        let result = self.write_snapshot(s, password, deadline, limits);
        // Include failures before temp-file creation (quota checks, locking, etc.).
        // A record containing any committed generation is never removed here.
        if result.is_err()
            && Self::files(&self.root.join(&id)).is_ok_and(|files| files.is_empty())
            && let Err(e) = self.discard(&id)
        {
            log::warn!("recovery: cannot remove failed empty record: {e}");
        }
        result
    }
    fn write_snapshot(
        &mut self,
        mut s: Snapshot,
        password: Option<&Password>,
        deadline: Option<Instant>,
        limits: Limits,
    ) -> Result<Vec<String>, String> {
        let id = s.info.id.clone();
        let dir = self.root.join(&id);
        if !dir.exists() {
            if self.directories()?.len() >= MAX_RECORDS {
                return Err("recovery folder is full; discard an older copy".into());
            }
            private_dir(&dir).map_err(|e| e.to_string())?;
        }
        self.acquire(&id)?;
        let files = Self::files(&dir)?;
        if files.len() >= MAX_FILES {
            return Err("older recovery generations could not be pruned; keeping the last valid copy".into());
        }
        s.version = VERSION;
        s.generation = files.first().and_then(|p| generation(p)).unwrap_or(0).checked_add(1).ok_or("recovery generation overflow")?;
        let previous = self.entries.get(&id).map(|e| (e.path.clone(), e.parts.clone()));
        let existing = Self::parts(&dir)?;
        // The side files the retained generation uses stay until its successor is valid (all
        // of them when that isn't known).
        let kept_before: BTreeSet<String> = match &previous {
            Some((_, Some(parts))) => parts.clone(),
            _ => existing.keys().cloned().collect(),
        };
        let previous_main = previous.as_ref().and_then(|(p, _)| fs::metadata(p).ok()).map_or(0, |m| m.len());
        let salt = match password {
            Some(pw) => Some(self.salt_for(&id, pw)?),
            None => None,
        };
        s.salt = salt.map(|s| hex(&s));

        // Pictures and parts leave the JSON for side files named by their content.
        let media = wordcraft_doc::Shared::into_inner(std::mem::take(&mut s.document.media));
        let passthrough = wordcraft_doc::Shared::into_inner(std::mem::take(&mut s.document.passthrough));
        let mut candidates = Vec::new();
        for (is_media, map) in [(true, media), (false, passthrough)] {
            for (name, bytes) in map {
                let hash = self.hash(&bytes);
                let part = PartRef { file: part_file(&hash, salt.as_ref()), sha256: hex(&hash), len: bytes.len() as u64 };
                candidates.push(Candidate { media: is_media, name, bytes, part });
            }
        }
        self.hashes.retain(|_, (w, _)| w.strong_count() > 0);
        // Smallest first: when the budget runs out, the most pictures are kept.
        candidates.sort_by_key(|c| c.part.len);
        for c in &candidates {
            let map = if c.media { &mut s.parts.media } else { &mut s.parts.passthrough };
            map.insert(c.name.clone(), c.part.clone());
        }

        // Plan within the budget before writing anything: the text and structure first (with
        // every part listed, an upper bound), then the side files that fit.
        let mut counter = Limited { inner: io::sink(), remaining: limits.main };
        serde_json::to_writer(&mut counter, &s).map_err(|_| "the document's text is too large for a recovery copy".to_string())?;
        let mut main = limits.main.saturating_sub(counter.remaining);
        if salt.is_some() {
            main = sealed_size(main);
        }
        let limit = limits.record.min(limits.disk.saturating_sub(self.disk_used(Some(&dir))?));
        let mut total = previous_main.saturating_add(main).saturating_add(kept_before.iter().filter_map(|f| existing.get(f)).sum());
        if total > limit {
            return Err("there is no room for a recovery copy of the text: discard older recovery copies".into());
        }
        let mut planned = BTreeSet::new();
        let mut omitted = Vec::new();
        let mut plan = Vec::new();
        for c in candidates {
            let cost = if planned.contains(&c.part.file) || kept_before.contains(&c.part.file) && existing.contains_key(&c.part.file) {
                0
            } else if let Some(n) = existing.get(&c.part.file) {
                *n
            } else if salt.is_some() {
                sealed_size(c.part.len)
            } else {
                c.part.len
            };
            if total.saturating_add(cost) > limit {
                omitted.push(c.name);
                continue;
            }
            total = total.saturating_add(cost);
            planned.insert(c.part.file.clone());
            plan.push(c);
        }

        // New side files (an unchanged picture's file is already there).
        s.parts = Parts::default();
        let mut written = BTreeSet::new();
        for c in plan {
            // A plain file of the wrong length is damaged: written again.
            let present = written.contains(&c.part.file) || existing.get(&c.part.file).is_some_and(|n| salt.is_some() || *n == c.part.len);
            if !present {
                // An emergency copy keeps time for the text.
                if deadline.is_some_and(|d| Instant::now() + TEXT_MARGIN >= d) {
                    omitted.push(c.name);
                    continue;
                }
                let data = match password {
                    Some(pw) => std::borrow::Cow::Owned(wordcraft_docx::encrypt(&c.bytes, pw.as_str()).map_err(|e| e.to_string())?),
                    None => std::borrow::Cow::Borrowed(c.bytes.as_slice()),
                };
                self.write_file(&dir, &dir.join(&c.part.file), &[&data])?;
                written.insert(c.part.file.clone());
            }
            let map = if c.media { &mut s.parts.media } else { &mut s.parts.passthrough };
            map.insert(c.name, c.part);
        }
        omitted.sort();
        s.info.omitted = omitted.clone();

        // The generation itself.
        let mut json = zeroize::Zeroizing::new(Vec::new());
        let mut writer = Limited { inner: &mut *json, remaining: limits.main };
        serde_json::to_writer(&mut writer, &s).map_err(|e| e.to_string())?;
        // The format must give back exactly this document (a NaN, say, doesn't survive JSON).
        let check: Snapshot = serde_json::from_slice(&json).map_err(|e| e.to_string())?;
        if check.document != s.document || check.parts != s.parts {
            return Err("recovery format did not preserve the full document".into());
        }
        // A plain generation's JSON follows its checksum line ([`PLAIN_MAGIC`]).
        let mut head = Vec::new();
        let (bytes, final_path, listed) = match password {
            Some(pw) => {
                let info = RecoveryInfo {
                    id: id.clone(),
                    name: PROTECTED_NAME.into(),
                    saved_at: s.info.saved_at.clone(),
                    path: None,
                    protected: true,
                    omitted: Vec::new(),
                    earlier: false,
                    older_than_file: false,
                };
                let mut out = SEALED_MAGIC.to_vec();
                serde_json::to_writer(&mut out, &SealedHeader { version: VERSION, info: info.clone(), generation: s.generation })
                    .map_err(|e| e.to_string())?;
                out.push(b'\n');
                out.extend(wordcraft_docx::encrypt(&json, pw.as_str()).map_err(|e| e.to_string())?);
                (out, dir.join(format!("snapshot-{:020}.enc", s.generation)), info)
            }
            None => {
                head.extend_from_slice(PLAIN_MAGIC);
                head.extend_from_slice(check_line(&json).as_bytes());
                (std::mem::take(&mut *json), dir.join(format!("snapshot-{:020}.json", s.generation)), s.info.clone())
            }
        };
        // The JSON was held to `limits.main` as it was written; encrypting it adds a little.
        let cap = if password.is_some() { sealed_file_size(limits.main) } else { limits.main };
        if bytes.len() as u64 > cap {
            return Err("recovery snapshot or folder size limit exceeded".into());
        }
        // A new generation is never written over the last valid file.
        if final_path.exists() {
            return Err("a recovery generation with this number already exists".into());
        }
        self.write_file(&dir, &final_path, &[&head, &bytes])?;
        sync_dir(&dir).map_err(|e| e.to_string())?;
        let used = s.parts.files();
        self.entries.insert(id, Entry { info: listed, path: final_path.clone(), parts: Some(used.clone()) });
        let previous_path = previous.map(|(p, _)| p);
        for old in Self::files(&dir)?.into_iter().filter(|p| p != &final_path && Some(p) != previous_path.as_ref()) {
            if let Err(e) = fs::remove_file(old) {
                log::warn!("recovery: cannot prune old generation: {e}");
            }
        }
        // Side files neither generation uses.
        for name in Self::parts(&dir)?.into_keys().filter(|f| !used.contains(f) && !kept_before.contains(f)) {
            if let Err(e) = fs::remove_file(dir.join(name)) {
                log::warn!("recovery: cannot remove an unused part: {e}");
            }
        }
        Ok(omitted)
    }
}
