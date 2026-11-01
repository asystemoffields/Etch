//! The desktop recovery store: copies of unsaved documents in the profile folder, written by a
//! worker thread. This file holds the limits and the on-disk types; the work is in the
//! submodules: `files` (private folders, durable writes), `format` (names, hashes, sizes),
//! `write` (a generation and its side files), `read` (reading and restoring a copy), `scan`
//! (finding, locking and listing records), `discard` (removing a record) and `worker` (the
//! thread, its pacing and the session-facing [`Recovery`]).
use super::RecoveryInfo;
use crate::io::Password;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::path::PathBuf;
use std::sync::Weak;
use std::time::Duration;
use wordcraft_doc::Document;

mod discard;
mod files;
mod format;
mod read;
mod scan;
#[cfg(test)]
mod tests;
mod worker;
mod write;
pub(crate) use worker::{Recovery, Restored};

const INTERVAL: Duration = Duration::from_secs(10);
/// A snapshot takes at most this share of the worker's time: after one that took `t`, the next
/// waits at least `t * DUTY` (a 1.2 s snapshot of a 27 MB document every 10 s kept ~12% of a
/// core busy while typing).
const DUTY: u32 = 20;
/// The longest wait between snapshots of a changed document, however slow they are.
const MAX_INTERVAL: Duration = Duration::from_secs(300);
/// The largest generation file: the text and structure, without pictures and parts. A document
/// within the limits (`wordcraft_doc::limits`) fits: its compact JSON with the listing of its
/// side files is within the saved-size estimate and [`PART_LISTING`] a side file
/// (`tests_limits::the_saved_size_estimate_bounds_what_the_writer_emits`), and the copy's
/// details (file name, path) within the last 64 KB.
const MAX_BYTES: u64 = wordcraft_doc::limits::SAVED_BYTES + PART_LISTING * MAX_PARTS as u64 + (64 << 10);
/// What a side file's entry in a generation's listing takes beyond its name: the file's name, its
/// hash and length, and the JSON around them.
pub(crate) const PART_LISTING: u64 = 256;
const _: () = assert!(MAX_PARTS >= wordcraft_doc::limits::PICTURES + wordcraft_doc::limits::KEPT_PARTS);
/// A record's whole size: its generation files and the pictures and parts kept beside them. Two
/// generations of a document at the limits and its pictures fit.
const RECORD_BUDGET: u64 = 1 << 30;
const _: () = assert!(RECORD_BUDGET >= 2 * MAX_BYTES + wordcraft_doc::limits::MEDIA_BYTES);
const _: () = assert!(DISK_BUDGET >= RECORD_BUDGET);
const MAX_RECORDS: usize = 128;
const MAX_FILES: usize = 16;
/// The most picture and part files one record may hold (its generations share them).
const MAX_PARTS: usize = 65_536;
const DISK_BUDGET: u64 = 2 << 30;
/// Times a record lock held for a moment (by a process being started) is tried again.
const LOCK_RETRIES: u32 = 10;
/// The generation format: version 2 keeps pictures and parts in side files ([`Parts`]);
/// version 1 (still read) kept them inline in the document JSON.
const VERSION: u32 = 2;
/// A protected document's generation (`snapshot-N.enc`) starts with this line, then one line of
/// JSON metadata that says nothing about the document ([`SealedHeader`]), then the encrypted
/// copy: the whole version 2 snapshot, in the same password-encrypted container as a protected
/// Word document (`wordcraft_docx::encrypt`, agile encryption).
const SEALED_MAGIC: &[u8] = b"WordCraft protected recovery copy\n";
/// The longest [`SealedHeader`] line read.
const MAX_HEADER: usize = 4096;
/// A plain generation (`snapshot-N.json`) starts with this line, then [`check_line`] (the
/// SHA-256 and the length of the rest), then the snapshot's JSON: a copy damaged on disk in a way
/// that keeps the JSON valid (a bit flip in the text) is refused, and the generation before it is
/// restored instead, rather than a silently different document. A copy written before checksums
/// is the JSON alone and restores as before, unchecked.
const PLAIN_MAGIC: &[u8] = b"WordCraft recovery copy\n";
/// The length of [`check_line`].
const CHECK_LINE: usize = "sha256=".len() + 64 + " length=".len() + 20 + 1;
/// The bytes before a plain generation's JSON.
const PLAIN_HEADER: usize = PLAIN_MAGIC.len() + CHECK_LINE;
/// What a damaged checksummed generation is refused with.
const DAMAGED: &str = "the recovery copy is damaged: its checksum doesn't match";
/// What a protected document's copy is listed as: its file name could say what it is about.
pub(crate) const PROTECTED_NAME: &str = "Protected document";
/// An emergency copy stops writing new pictures this long before its deadline, so the text is
/// still written in time.
const TEXT_MARGIN: Duration = Duration::from_millis(1500);

/// Size limits of the recovery store (tests use small ones).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    /// One generation file.
    pub main: u64,
    /// One record, its generations and side files together.
    pub record: u64,
    /// The whole recovery folder.
    pub disk: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self { main: MAX_BYTES, record: RECORD_BUDGET, disk: DISK_BUDGET }
    }
}

/// A picture or part kept beside the snapshot, in a file named by its content: unchanged
/// pictures are written once and shared by later generations.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct PartRef {
    /// `part-<64 hex>.bin` (plain) or `.enc` (encrypted).
    file: String,
    /// SHA-256 of the bytes, checked when the copy is restored.
    sha256: String,
    len: u64,
}
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Parts {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    media: BTreeMap<String, PartRef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    passthrough: BTreeMap<String, PartRef>,
}
impl Parts {
    fn is_empty(&self) -> bool {
        self.media.is_empty() && self.passthrough.is_empty()
    }
    fn files(&self) -> BTreeSet<String> {
        self.media.values().chain(self.passthrough.values()).map(|r| r.file.clone()).collect()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    version: u32,
    pub info: RecoveryInfo,
    generation: u64,
    /// Version 2: without `media` and `passthrough`, which are in [`Self::parts`].
    pub document: Document,
    #[serde(default, skip_serializing_if = "Parts::is_empty")]
    parts: Parts,
    /// A protected copy's salt for its side files' names (hex), so they say nothing about the
    /// pictures: `part-` + SHA-256(salt + SHA-256(bytes)) + `.enc`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    salt: Option<String>,
}
impl Snapshot {
    pub fn new(info: RecoveryInfo, document: Document) -> Self {
        Self { version: VERSION, info, generation: 0, document, parts: Parts::default(), salt: None }
    }
}
/// The readable line of a protected generation: what the recovery list shows, nothing more.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SealedHeader {
    version: u32,
    info: RecoveryInfo,
    generation: u64,
}
struct Entry {
    info: RecoveryInfo,
    path: PathBuf,
    /// The side files this generation uses (`None`: not known, a protected copy not opened).
    parts: Option<BTreeSet<String>>,
}
/// A protected record's password and the salt of its side files' names, while this session
/// writes it.
struct Secret {
    password: Password,
    salt: [u8; 16],
}
struct Store {
    root: PathBuf,
    entries: BTreeMap<String, Entry>,
    // Advisory locks prevent another running instance from restoring/discarding a live copy.
    locks: BTreeMap<String, File>,
    secrets: BTreeMap<String, Secret>,
    /// SHA-256 of pictures already hashed, by allocation: an unchanged picture isn't hashed
    /// again. A changed one is a new allocation (`Arc::make_mut` disowns weak references).
    hashes: HashMap<usize, (Weak<Vec<u8>>, [u8; 32])>,
    /// Bytes written to files so far (measurements and tests).
    written: u64,
}
