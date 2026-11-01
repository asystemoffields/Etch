//! The emergency recovery copy written when the app is terminated (SIGTERM, SIGINT, SIGHUP) or
//! panics: written synchronously, and offered at the next start. Synthetic documents only, in a
//! temporary recovery folder.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used)]

use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wordcraft_doc::Document;
use wordcraft_engine::Session;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("wordcraft-emergency-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn session(root: &Path) -> Session {
    let mut s = Session::new(Document::new());
    s.enable_recovery(root.to_owned(), Arc::new(|| {})).unwrap();
    s
}

fn settle(s: &mut Session) {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        s.recovery_tick();
        if s.recovery_ready() && !s.recovery_busy() {
            break;
        }
        assert!(Instant::now() < end, "recovery did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Snapshot files under the recovery folder.
fn snapshots(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| {
            let p = e.path();
            if p.is_dir() {
                snapshots(&p)
            } else {
                usize::from(
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("snapshot-") && (n.ends_with(".json") || n.ends_with(".enc"))),
                )
            }
        })
        .sum()
}

/// #3140 (upstream f550ab7b): a save that lands right before the app closes can race the process
/// exiting before the now-redundant recovery copy is removed, so the next start wrongly offers to
/// recover a document that was saved cleanly. `finish_recovery` (desktop hosts call it on exit,
/// where they wait out pending print jobs) discards it synchronously, with no recovery tick in
/// between; a document with unsaved changes keeps its copy for the next start.
#[test]
fn finish_recovery_discards_the_copy_of_a_document_saved_right_before_exit() {
    let t = Temp::new();
    let mut s = session(&t.0);
    settle(&mut s);
    s.run("text.insert", &json!({"text": "About to be saved"})).unwrap();
    assert!(s.emergency_snapshot(Duration::from_secs(10)).unwrap());
    assert_eq!(snapshots(&t.0), 1, "a recovery copy exists for the dirty document");
    // A real Save does this, then the app can close on the very next frame, with no recovery
    // tick in between to notice the clean document and queue the discard itself.
    s.dirty = false;
    assert_eq!(snapshots(&t.0), 1, "the race: a clean document alone discards nothing");
    s.finish_recovery();
    assert_eq!(snapshots(&t.0), 0, "the redundant copy is gone before finish_recovery returns");

    // Unsaved changes keep their copy: that is what the next start offers.
    s.run("text.insert", &json!({"text": " and then edited again"})).unwrap();
    assert!(s.emergency_snapshot(Duration::from_secs(10)).unwrap());
    assert_eq!(snapshots(&t.0), 1);
    s.finish_recovery();
    assert_eq!(snapshots(&t.0), 1, "unsaved work is never discarded on exit");
}

#[test]
fn an_emergency_snapshot_is_on_disk_when_it_returns_and_restores_at_the_next_start() {
    let t = Temp::new();
    let mut s = session(&t.0);
    // No recovery tick at all: the start-up scan is still under way, and the periodic copy
    // wouldn't be due for 10 seconds. The emergency copy waits for the one and skips the other.
    s.run("text.insert", &json!({"text": "Unsaved synthetic words that must endure"})).unwrap();
    assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true));
    assert_eq!(snapshots(&t.0), 1, "written synchronously, before it returned");
    // Nothing changed since: no second copy.
    assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true));
    assert_eq!(snapshots(&t.0), 1);
    let expected = s.doc.clone();
    drop(s);

    let mut s = session(&t.0);
    settle(&mut s);
    let docs = s.recovery_documents();
    assert_eq!(docs.len(), 1);
    s.run("file.recover", &json!({"restore": docs[0].id})).unwrap();
    settle(&mut s);
    assert_eq!(s.doc, expected);
    assert!(s.dirty);
}

#[test]
fn saved_documents_and_headless_sessions_get_no_emergency_copy() {
    let t = Temp::new();
    let mut s = session(&t.0);
    assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(false), "nothing unsaved");
    assert_eq!(snapshots(&t.0), 0);
    // A session without recovery (headless, the browser) has nothing to write.
    let mut headless = Session::new(Document::new());
    headless.run("text.insert", &json!({"text": "x"})).unwrap();
    assert_eq!(headless.emergency_snapshot(Duration::from_secs(1)), Ok(false));
}

/// Every file under `dir`, with its bytes.
fn files(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .flat_map(|e| {
            let p = e.path();
            if p.is_dir() { files(&p) } else { vec![(p.clone(), std::fs::read(&p).unwrap())] }
        })
        .collect()
}

/// A password-protected document's emergency copy is encrypted with its password: nothing of
/// it reaches the disk in plain form, and it restores with the password.
#[test]
fn a_protected_documents_emergency_copy_is_encrypted() {
    const MARKER: &[u8] = b"EMERGENCY-PLAINTEXT-MARKER-51d2";
    let t = Temp::new();
    let mut s = session(&t.0);
    s.run("text.insert", &json!({"text": String::from_utf8_lossy(MARKER)})).unwrap();
    s.doc.media.insert("media/p.png".into(), Arc::new(MARKER.repeat(1000)));
    s.run("file.encrypt", &json!({"password": "synthetic-pass"})).unwrap();
    assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true));
    assert_eq!(snapshots(&t.0), 1);
    for (path, bytes) in files(&t.0) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(!name.ends_with(".json") && !name.ends_with(".bin"), "{name} is not encrypted");
        assert!(!bytes.windows(MARKER.len()).any(|w| w == MARKER), "{name} holds plaintext");
    }
    let expected = s.doc.clone();
    drop(s);
    let mut s = session(&t.0);
    settle(&mut s);
    let id = s.recovery_documents()[0].id.clone();
    assert!(s.recovery_documents()[0].protected);
    s.run("file.recover", &json!({"restore": id, "password": "synthetic-pass"})).unwrap();
    settle(&mut s);
    assert_eq!(s.doc, expected);
    assert!(s.password.is_some());
}

/// Large pictures are side files written once: the emergency copy of a document whose pictures
/// the periodic copy already wrote writes only the text, well within its time bound. With no
/// time for new pictures, the text is still written.
#[test]
fn an_emergency_copy_with_large_pictures_keeps_the_text_in_time() {
    let t = Temp::new();
    let mut s = session(&t.0);
    let picture: Vec<u8> = (0..8u32 << 20).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    s.doc.media.insert("media/large.png".into(), Arc::new(picture));
    s.run("text.insert", &json!({"text": "Words beside a large picture"})).unwrap();
    assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true));
    s.run("text.insert", &json!({"text": " and more words"})).unwrap();
    let start = Instant::now();
    assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true));
    assert!(start.elapsed() < Duration::from_secs(5), "{:?}: the picture isn't written again", start.elapsed());
    assert_eq!(s.recovery_gap(), None);

    // A new picture and almost no time: the text is kept, the picture named as left out.
    s.doc.media.insert("media/new.png".into(), Arc::new(vec![7u8; 1 << 20]));
    s.run("text.insert", &json!({"text": " last words"})).unwrap();
    assert_eq!(s.emergency_snapshot(Duration::from_millis(1000)), Ok(true));
    assert_eq!(s.recovery_gap(), Some(wordcraft_engine::recovery::RecoveryGap::Omitted(vec!["media/new.png".into()])));
    let text = s.doc.plain_text(wordcraft_doc::StoryRef::Body);
    drop(s);
    let mut s = session(&t.0);
    settle(&mut s);
    let id = s.recovery_documents()[0].id.clone();
    s.run("file.recover", &json!({"restore": id})).unwrap();
    settle(&mut s);
    assert_eq!(s.doc.plain_text(wordcraft_doc::StoryRef::Body), text);
    assert!(s.doc.media.contains_key("media/large.png"));
}
