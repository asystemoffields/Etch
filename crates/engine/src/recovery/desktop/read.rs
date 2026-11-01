//! Reading a copy back: a generation's header or whole body (plain or protected), its side
//! files, and [`Store::load`], which falls back to the earlier generation when the newest is
//! damaged.
use super::files::read_bounded;
use super::format::{generation, hex, id_ok, parse_check_line, part_ok, sealed, sealed_file_size, sealed_size, sha256, unhex16};
use super::{
    DAMAGED, MAX_BYTES, MAX_HEADER, PLAIN_HEADER, PLAIN_MAGIC, PartRef, RECORD_BUDGET, SEALED_MAGIC, SealedHeader, Secret, Snapshot, Store, VERSION,
};
use crate::io::Password;
use crate::recovery::RecoveryInfo;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::Path;
use std::sync::Arc;

fn check_snapshot(s: &Snapshot) -> Result<(), String> {
    let known = s.version == VERSION || s.version == 1 && s.parts.is_empty() && s.salt.is_none();
    if !known || !id_ok(&s.info.id) {
        return Err("unsupported recovery file".into());
    }
    if !s.parts.media.values().chain(s.parts.passthrough.values()).all(|r| part_ok(&r.file)) {
        return Err("recovery file names an invalid part".into());
    }
    Ok(())
}
/// A reader that hashes and counts what is read through it.
struct Hashing<R> {
    inner: R,
    hasher: Sha256,
    read: u64,
}
impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(buf.get(..n).unwrap_or_default());
        self.read = self.read.saturating_add(n as u64);
        Ok(n)
    }
}
/// A plain generation (`.json`): its checksum line verified when it has one ([`PLAIN_MAGIC`]);
/// a copy written before checksums is the JSON alone. Version 1 copies keep pictures inline, as
/// base64 or (older still) arrays of numbers. The JSON is parsed as it is read, not held whole.
fn read_snapshot(path: &Path) -> Result<Snapshot, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_BYTES + PLAIN_HEADER as u64 {
        return Err("not a regular recovery file, or larger than a recovery file may be".into());
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("not a regular recovery file".into());
    }
    let mut head = Vec::with_capacity(PLAIN_HEADER);
    (&mut file).take(PLAIN_HEADER as u64).read_to_end(&mut head).map_err(|e| e.to_string())?;
    let s: Snapshot = if head.starts_with(PLAIN_MAGIC) {
        let (digest, len) = parse_check_line(head.get(PLAIN_MAGIC.len()..).unwrap_or_default()).ok_or(DAMAGED)?;
        if len > MAX_BYTES || meta.len() != (PLAIN_HEADER as u64).saturating_add(len) {
            return Err(DAMAGED.into());
        }
        let mut body = Hashing { inner: file.take(len.saturating_add(1)), hasher: Sha256::new(), read: 0 };
        // A bit flip can also break the JSON: refused either way.
        let parsed: Result<Snapshot, _> = serde_json::from_reader(BufReader::new(&mut body));
        // The rest of the file counts, whether or not the JSON read to its end.
        io::copy(&mut body, &mut io::sink()).map_err(|e| e.to_string())?;
        let got: [u8; 32] = body.hasher.finalize().into();
        if body.read != len || hex(&got) != digest {
            return Err(DAMAGED.into());
        }
        parsed.map_err(|e| e.to_string())?
    } else {
        // Written before checksums: the JSON alone.
        let rest = file.take((MAX_BYTES + 1).saturating_sub(head.len() as u64));
        serde_json::from_reader(BufReader::new(io::Cursor::new(head).chain(rest))).map_err(|e| e.to_string())?
    };
    check_snapshot(&s)?;
    Ok(s)
}
/// A protected generation's readable header, and where the encrypted copy starts.
pub(super) fn sealed_header(bytes: &[u8]) -> Result<(SealedHeader, usize), String> {
    let rest = bytes.strip_prefix(SEALED_MAGIC).ok_or("not a protected recovery copy")?;
    let end = rest.iter().take(MAX_HEADER).position(|b| *b == b'\n').ok_or("damaged protected recovery copy")?;
    let head: SealedHeader = serde_json::from_slice(rest.get(..end).unwrap_or(&[])).map_err(|e| e.to_string())?;
    if head.version != VERSION || !id_ok(&head.info.id) || !head.info.protected {
        return Err("unsupported protected recovery copy".into());
    }
    Ok((head, SEALED_MAGIC.len() + end + 1))
}
/// What the recovery list needs of a generation, without its password: the listing, its
/// generation and (plain copies) the side files it uses.
pub(super) fn listing(path: &Path) -> Result<(RecoveryInfo, u64, Option<BTreeSet<String>>), String> {
    if sealed(path) {
        let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !meta.is_file() || meta.len() > sealed_file_size(MAX_BYTES) {
            return Err("not a regular recovery file, or larger than a recovery file may be".into());
        }
        let mut head = Vec::new();
        File::open(path)
            .map_err(|e| e.to_string())?
            .take((SEALED_MAGIC.len() + MAX_HEADER + 1) as u64)
            .read_to_end(&mut head)
            .map_err(|e| e.to_string())?;
        let (h, _) = sealed_header(&head)?;
        return Ok((h.info, h.generation, None));
    }
    let s = read_snapshot(path)?;
    let files = s.parts.files();
    Ok((s.info, s.generation, Some(files)))
}
/// A generation, decrypted with `password` when it is protected (no password, or a wrong one,
/// is an error that leaves the copy as it is).
fn load_main(path: &Path, password: Option<&Password>) -> Result<Snapshot, String> {
    if !sealed(path) {
        return read_snapshot(path);
    }
    let bytes = read_bounded(path, sealed_file_size(MAX_BYTES))?;
    let (head, start) = sealed_header(&bytes)?;
    let Some(password) = password else {
        return Err(format!("{}: enter it to restore this recovery copy", wordcraft_docx::PASSWORD_REQUIRED));
    };
    let inner =
        zeroize::Zeroizing::new(wordcraft_docx::decrypt(bytes.get(start..).unwrap_or(&[]), Some(password.as_str())).map_err(|e| e.to_string())?);
    let s: Snapshot = serde_json::from_slice(&inner).map_err(|e| e.to_string())?;
    check_snapshot(&s)?;
    if s.version != VERSION || s.info.id != head.info.id || s.generation != head.generation {
        return Err("mismatched protected recovery copy".into());
    }
    Ok(s)
}
/// Was the file the copy `info` was made from (its `path`) changed after the copy's generation
/// file at `generation` was written? Then the copy is older than the saved file. A file that is
/// gone, or times that can't be read, say no.
pub(super) fn older_than_file(info: &RecoveryInfo, generation: &Path) -> bool {
    let modified = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    match (info.path.as_deref().map(Path::new).and_then(modified), modified(generation)) {
        (Some(file), Some(copy)) => file > copy,
        _ => false,
    }
}
/// A side file's bytes, checked against the length and hash the snapshot recorded.
fn read_part(dir: &Path, part: &PartRef, password: Option<&Password>) -> Result<Vec<u8>, String> {
    if !part_ok(&part.file) {
        return Err("invalid part name".into());
    }
    let path = dir.join(&part.file);
    let bytes = if sealed(&path) {
        let password = password.ok_or("a protected part needs the password")?;
        let raw = read_bounded(&path, sealed_size(part.len).min(RECORD_BUDGET))?;
        wordcraft_docx::decrypt(&raw, Some(password.as_str())).map_err(|e| e.to_string())?
    } else {
        read_bounded(&path, part.len.min(RECORD_BUDGET))?
    };
    if bytes.len() as u64 != part.len || hex(&sha256(&bytes)) != part.sha256 {
        return Err("a picture or part of the copy is damaged".into());
    }
    Ok(bytes)
}

impl Store {
    /// The copy `id`, with its pictures and parts. A protected one needs its password (wrong or
    /// missing: an error, and the copy stays). A side file that is missing or damaged leaves out
    /// that picture (listed in `info.omitted`), not the text.
    pub(super) fn load(&mut self, id: &str, password: Option<&Password>) -> Result<Snapshot, String> {
        if !self.entries.contains_key(id) {
            return Err("no such recovery copy".into());
        }
        self.acquire(id)?;
        let entry = self.entries.get(id).ok_or("no such recovery copy")?;
        let (path, earlier, older_than_file) = (entry.path.clone(), entry.info.earlier, entry.info.older_than_file);
        // Discovery can't check a protected generation without its password, and a plain one
        // can be damaged after it was listed: a damaged one falls back to the retained earlier
        // generation, and the restore says so (`earlier`). A missing or wrong password doesn't.
        let mut s = match load_main(&path, password) {
            Ok(mut s) => {
                s.info.earlier = earlier;
                s
            }
            Err(e) if crate::io::needs_password(&e) || crate::io::wrong_password(&e) => return Err(e),
            Err(e) => {
                let mut found = None;
                for older in Self::files(&self.root.join(id))?.into_iter().filter(|p| generation(p) < generation(&path)) {
                    match load_main(&older, password) {
                        Ok(mut s) if s.info.id == id && Some(s.generation) == generation(&older) => {
                            log::warn!("recovery: {e}; restoring the earlier copy {}", older.display());
                            s.info.earlier = true;
                            found = Some(s);
                            break;
                        }
                        Err(e) if crate::io::needs_password(&e) || crate::io::wrong_password(&e) => return Err(e),
                        _ => {}
                    }
                }
                found.ok_or(e)?
            }
        };
        s.info.older_than_file = older_than_file;
        let parts = std::mem::take(&mut s.parts);
        let used = parts.files();
        if let Some(e) = self.entries.get_mut(id) {
            e.parts = Some(used);
        }
        if let (Some(pw), Some(salt)) = (password, s.salt.as_deref().and_then(unhex16)) {
            self.secrets.insert(id.to_string(), Secret { password: pw.clone(), salt });
        }
        let dir = self.root.join(id);
        for (is_media, map) in [(true, parts.media), (false, parts.passthrough)] {
            for (name, part) in map {
                match read_part(&dir, &part, password) {
                    Ok(bytes) => {
                        let store = if is_media { &mut s.document.media } else { &mut s.document.passthrough };
                        store.insert(name, Arc::new(bytes));
                    }
                    Err(e) => {
                        log::warn!("recovery: a picture or part could not be restored: {e}");
                        s.info.omitted.push(name);
                    }
                }
            }
        }
        s.info.omitted.sort();
        s.info.omitted.dedup();
        Ok(s)
    }
}
