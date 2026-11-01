use super::files::private_dir;
use super::format::{new_id, part_ok, sealed};
use super::read::sealed_header;
use super::write::Limited;
use super::*;
use crate::Session;
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use wordcraft_doc::StoryRef;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("wordcraft-recovery-test-{}", new_id().unwrap()));
        private_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
/// A store reopened on `root` whose scan reports an unreadable record. The record's lock can be
/// briefly held by a process another test is starting (a forked child holds the descriptor
/// until it execs), and a record that can't be locked is skipped, not reported: so try again
/// for up to a second rather than read a skipped record as a clean scan.
fn reopen_reporting_unreadable(root: &std::path::Path) -> Store {
    for _ in 0..50 {
        let mut store = Store::new(root.to_path_buf()).unwrap();
        if store.scan().is_err() {
            return store;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the scan never reported the unreadable record");
}
/// `store.load(id)`, tried again for up to a second while the record can't be locked: a process
/// another test is starting holds the lock for a moment (see [`reopen_reporting_unreadable`]).
fn load_retrying(store: &mut Store, id: &str) -> Snapshot {
    let mut last = String::new();
    for _ in 0..50 {
        match store.load(id, None) {
            Ok(snapshot) => return snapshot,
            Err(e) => last = e,
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the record never loaded: {last}");
}
fn snapshot(id: &str, text: &str) -> Snapshot {
    Snapshot::new(
        RecoveryInfo {
            id: id.into(),
            name: "Synthetic draft".into(),
            saved_at: crate::cmd::now_iso(),
            path: None,
            protected: false,
            omitted: Vec::new(),
            earlier: false,
            older_than_file: false,
        },
        Document::from_text(text),
    )
}
fn settle(s: &mut Session) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        s.recovery_tick();
        if s.recovery_ready() && !s.recovery_busy() {
            break;
        }
        assert!(Instant::now() < end, "recovery did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn session(root: &Path) -> Session {
    let mut s = Session::new(Document::new());
    s.enable_recovery(root.to_owned(), Arc::new(|| {})).unwrap();
    settle(&mut s);
    s
}
fn save_now(s: &mut Session) {
    s.recovery.as_mut().unwrap().due = Instant::now();
    s.recovery_tick();
    settle(s);
}

#[test]
fn recovery_commands_only_address_known_profile_records() {
    use crate::file_access::{AutomationRoots, confine};
    let profile = Temp::new();
    let files = Temp::new();
    let original = files.0.join("original.txt");
    fs::write(&original, b"External synthetic original").unwrap();
    let roots = AutomationRoots::new(Some(&files.0), Some(&files.0)).unwrap().unwrap();
    let mut s = session(&profile.0);
    s.run("document.setText", &json!({"text": "Recoverable synthetic draft"})).unwrap();
    save_now(&mut s);
    let id = s.recovery_documents()[0].id.clone();
    confine(Some(&roots), || {
        // A command cannot use a file path or choose a different storage folder.
        for bad in [original.to_string_lossy().into_owned(), "../original.txt".into()] {
            for action in ["restore", "discard"] {
                let err = s.run("file.recover", &json!({action: bad, "root": files.0})).unwrap_err();
                assert!(err.to_string().contains("no such recovery copy"), "{err}");
            }
        }
        assert_eq!(s.recovery_documents().len(), 1);
        // An untitled document's copy counts as outside the roots: neither restored nor discarded.
        for action in ["restore", "discard"] {
            let err = s.run("file.recover", &json!({action: id})).unwrap_err();
            assert!(err.to_string().contains("untitled"), "{err}");
        }
        assert_eq!(s.recovery_documents().len(), 1);
    });
    // The person (no roots) may restore it.
    s.run("file.recover", &json!({"restore": id})).unwrap();
    settle(&mut s);
    assert!(s.path.is_none());
    assert!(s.dirty);
    assert_eq!(s.doc.plain_text(StoryRef::Body), "Recoverable synthetic draft");
    assert_eq!(fs::read(&original).unwrap(), b"External synthetic original");
    assert_eq!(fs::read_dir(&files.0).unwrap().count(), 1);
}

/// Decision (#review 8a): a confined control connection restores only a copy whose original
/// file is inside its read root, and discards only one inside its write root. The listing
/// doesn't tell it where files outside its read root are.
#[test]
fn confined_recovery_follows_the_original_files_roots() {
    use crate::file_access::{AutomationRoots, confine};
    let profile = Temp::new();
    let files = Temp::new();
    let elsewhere = Temp::new();
    let original = files.0.join("original.txt");
    fs::write(&original, b"Synthetic original").unwrap();
    let read_only = AutomationRoots::new(Some(&files.0), None).unwrap().unwrap();
    let both = AutomationRoots::new(Some(&files.0), Some(&files.0)).unwrap().unwrap();
    let other = AutomationRoots::new(Some(&elsewhere.0), Some(&elsewhere.0)).unwrap().unwrap();
    let mut s = session(&profile.0);
    s.run("file.open", &json!({"path": original})).unwrap();
    s.run("document.setText", &json!({"text": "Recoverable edit"})).unwrap();
    save_now(&mut s);
    let id = s.recovery_documents()[0].id.clone();
    assert_eq!(s.recovery_documents()[0].path.as_deref(), original.to_str());
    // Roots elsewhere: refused both ways, and the path isn't listed.
    confine(Some(&other), || {
        let listed = s.run("file.recover", &json!({})).unwrap();
        assert!(listed["documents"][0].get("path").is_none(), "{listed}");
        for action in ["restore", "discard"] {
            let err = s.run("file.recover", &json!({action: id})).unwrap_err();
            assert!(err.to_string().contains("outside the"), "{err}");
        }
    });
    confine(Some(&read_only), || {
        let listed = s.run("file.recover", &json!({})).unwrap();
        assert_eq!(listed["documents"][0]["path"].as_str(), original.to_str());
        // Read access restores; discarding needs write access.
        assert!(s.run("file.recover", &json!({"discard": id})).unwrap_err().to_string().contains("write authority is absent"));
        s.run("file.recover", &json!({"restore": id})).unwrap();
        settle(&mut s);
        assert_eq!(s.doc.plain_text(StoryRef::Body), "Recoverable edit");
    });
    confine(Some(&both), || {
        s.run("file.recover", &json!({"discard": id})).unwrap();
        settle(&mut s);
        assert!(s.recovery_documents().is_empty());
    });
    assert_eq!(fs::read(&original).unwrap(), b"Synthetic original");
}

#[test]
fn failed_write_keeps_last_valid_generation_and_discovery_falls_back() {
    let t = Temp::new();
    let id = new_id().unwrap();
    let mut store = Store::new(t.0.clone()).unwrap();
    store.save(snapshot(&id, "First valid copy"), None, None, Limits::default()).unwrap();
    let first = store.entries[&id].path.clone();
    let bytes = fs::read(&first).unwrap();
    // Simulate a final rename failure without relying on permissions (root can bypass them).
    let obstruction = t.0.join(&id).join("snapshot-00000000000000000002.json");
    fs::create_dir(&obstruction).unwrap();
    assert!(store.save(snapshot(&id, "Failed copy"), None, None, Limits::default()).is_err());
    assert_eq!(fs::read(&first).unwrap(), bytes);
    fs::remove_dir(obstruction).unwrap();
    store.save(snapshot(&id, "Second valid copy"), None, None, Limits::default()).unwrap();
    let second = store.entries[&id].path.clone();
    fs::write(second, b"{truncated").unwrap();
    drop(store);
    let mut reopened = reopen_reporting_unreadable(&t.0);
    assert_eq!(load_retrying(&mut reopened, &id).document.plain_text(StoryRef::Body), "First valid copy");
    // Never reuse the corrupt generation's name or prune the valid fallback.
    reopened.save(snapshot(&id, "Third valid copy"), None, None, Limits::default()).unwrap();
    assert_eq!(fs::read(first).unwrap(), bytes);
    assert_eq!(Store::files(&t.0.join(&id)).unwrap().len(), 2);
}

#[test]
fn size_limit_and_lossy_values_fail_without_committing() {
    let mut out = Limited { inner: Vec::new(), remaining: 3 };
    assert!(out.write_all(b"four").is_err());
    assert!(out.inner.is_empty());
    let t = Temp::new();
    let id = new_id().unwrap();
    let mut store = Store::new(t.0.clone()).unwrap();
    store.save(snapshot(&id, "valid"), None, None, Limits::default()).unwrap();
    let first = store.entries[&id].path.clone();
    let bytes = fs::read(&first).unwrap();
    let mut invalid = snapshot(&id, "invalid");
    invalid.document.last_section.page_w = f32::NAN;
    assert!(store.save(invalid, None, None, Limits::default()).is_err());
    assert_eq!(fs::read(first).unwrap(), bytes);
    assert_eq!(Store::files(&t.0.join(id)).unwrap().len(), 1);
}

#[test]
fn failed_pruning_cannot_push_a_record_beyond_the_discovery_limit() {
    let t = Temp::new();
    let id = new_id().unwrap();
    let mut store = Store::new(t.0.clone()).unwrap();
    store.save(snapshot(&id, "Last valid copy"), None, None, Limits::default()).unwrap();
    let first = store.entries[&id].path.clone();
    let bytes = fs::read(&first).unwrap();
    // A failed cleanup must not allow the next write to exceed the reader's bound.
    for n in 2..=MAX_FILES {
        fs::write(t.0.join(&id).join(format!("snapshot-{n:020}.json")), b"{truncated").unwrap();
    }
    assert!(store.save(snapshot(&id, "New copy"), None, None, Limits::default()).is_err());
    assert_eq!(fs::read(first).unwrap(), bytes);
    drop(store);
    let mut reopened = reopen_reporting_unreadable(&t.0);
    assert_eq!(load_retrying(&mut reopened, &id).document.plain_text(StoryRef::Body), "Last valid copy");
}

#[test]
fn failed_first_write_does_not_consume_a_record_slot() {
    let t = Temp::new();
    let mut store = Store::new(t.0.clone()).unwrap();
    let mut invalid = snapshot(&new_id().unwrap(), "Synthetic draft");
    invalid.document.last_section.page_w = f32::NAN;
    assert!(store.save(invalid, None, None, Limits::default()).is_err());
    assert!(store.directories().unwrap().is_empty());
}

#[test]
fn discovery_reclaims_a_crashed_first_write_but_keeps_valid_records() {
    let t = Temp::new();
    let id = new_id().unwrap();
    let empty = new_id().unwrap();
    let mut store = Store::new(t.0.clone()).unwrap();
    store.save(snapshot(&id, "Valid synthetic draft"), None, None, Limits::default()).unwrap();
    drop(store);
    let dir = t.0.join(&empty);
    private_dir(&dir).unwrap();
    fs::write(dir.join(format!("pending-{}.tmp", new_id().unwrap())), b"{unfinished").unwrap();
    let mut reopened = Store::new(t.0.clone()).unwrap();
    reopened.scan().unwrap();
    assert!(!dir.exists());
    assert_eq!(load_retrying(&mut reopened, &id).document.plain_text(StoryRef::Body), "Valid synthetic draft");
}

/// One bad record (here more generations than a record may hold) aborted the whole scan, so
/// every other recovery copy disappeared from the list. It is skipped and the rest are listed.
#[test]
fn one_bad_record_does_not_hide_the_others() {
    let t = Temp::new();
    let good = new_id().unwrap();
    let bad = new_id().unwrap();
    let mut store = Store::new(t.0.clone()).unwrap();
    store.save(snapshot(&good, "Valid synthetic draft"), None, None, Limits::default()).unwrap();
    store.save(snapshot(&bad, "Overgrown synthetic draft"), None, None, Limits::default()).unwrap();
    drop(store);
    for n in 2..=MAX_FILES as u64 + 1 {
        fs::write(t.0.join(&bad).join(format!("snapshot-{n:020}.json")), b"{truncated").unwrap();
    }
    let mut reopened = Store::new(t.0.clone()).unwrap();
    let e = reopened.scan().unwrap_err();
    assert!(e.contains("could not be read"), "{e}");
    let listed: Vec<_> = reopened.inventory().into_iter().map(|i| i.id).collect();
    assert_eq!(listed, vec![good.clone()]);
    assert_eq!(reopened.load(&good, None).unwrap().document.plain_text(StoryRef::Body), "Valid synthetic draft");
    // The bad record is left as it was, never removed.
    assert_eq!(fs::read_dir(t.0.join(&bad)).unwrap().count(), MAX_FILES + 2, "generations and the lock");
}

#[test]
fn live_records_are_locked_and_discard_releases_storage() {
    let t = Temp::new();
    let id = new_id().unwrap();
    let mut owner = Store::new(t.0.clone()).unwrap();
    owner.save(snapshot(&id, "live"), None, None, Limits::default()).unwrap();
    let mut other = Store::new(t.0.clone()).unwrap();
    other.scan().unwrap();
    assert!(other.inventory().is_empty());
    assert!(other.discard(&id).is_err());
    assert!(other.discard("../outside").is_err());
    drop(owner);
    other.scan().unwrap();
    assert_eq!(other.inventory().len(), 1);
    other.discard(&id).unwrap();
    assert!(!t.0.join(id).exists(), "discard must release the record slot");
}

#[test]
fn new_draft_restarts_restores_unsaved_and_explicitly_discards() {
    let t = Temp::new();
    let mut s = session(&t.0);
    s.run("text.insert", &json!({"text": "Unsaved synthetic draft"})).unwrap();
    save_now(&mut s);
    assert!(s.recovery_wait().is_none(), "unchanged documents must not schedule idle work");
    let expected = s.doc.clone();
    drop(s);
    let mut s = session(&t.0);
    let id = s.recovery_documents()[0].id.clone();
    s.run("file.recover", &json!({"restore": id})).unwrap();
    settle(&mut s);
    assert_eq!(s.doc, expected);
    assert!(s.dirty && s.path.is_none());
    s.run("file.recover", &json!({"discard": id})).unwrap();
    settle(&mut s);
    save_now(&mut s);
    assert!(s.recovery_documents().is_empty(), "discard isn't undone by the next timer");
    assert_eq!(s.doc, expected, "discard deletes the copy, not the editing document");
}

#[test]
fn existing_file_is_unchanged_and_successful_save_removes_copy() {
    let t = Temp::new();
    let file = t.0.join("original.docx");
    let root = t.0.join("profile");
    crate::io::save_path(&file, &Document::from_text("Original business note")).unwrap();
    let original = fs::read(&file).unwrap();
    let mut s = session(&root);
    s.run("file.open", &json!({"path": file})).unwrap();
    s.run("text.insert", &json!({"text": "Unsaved edit. "})).unwrap();
    save_now(&mut s);
    let expected = s.doc.clone();
    drop(s);
    let mut s = session(&root);
    let id = s.recovery_documents()[0].id.clone();
    s.run("file.recover", &json!({"restore": id})).unwrap();
    settle(&mut s);
    assert_eq!(s.doc, expected);
    assert!(s.path.is_none() && s.dirty);
    assert_eq!(fs::read(&file).unwrap(), original);
    let saved = t.0.join("recovered.json");
    s.run("file.save", &json!({"path": saved})).unwrap();
    settle(&mut s);
    assert!(s.recovery_documents().is_empty());
    assert_eq!(fs::read(file).unwrap(), original);
}

/// Applying a password removes the plaintext copy (even one being written) before the
/// document is protected; later copies are encrypted.
#[test]
fn password_barrier_clears_in_flight_plaintext_and_encrypts_new_copies() {
    let t = Temp::new();
    let mut s = session(&t.0);
    s.run("text.insert", &json!({"text": format!("{MARKER} synthetic draft")})).unwrap();
    s.recovery.as_mut().unwrap().due = Instant::now();
    s.recovery_tick();
    assert!(s.recovery_busy());
    s.run("file.encrypt", &json!({"password": "synthetic-password"})).unwrap();
    assert!(s.recovery_documents().is_empty());
    assert!(fs::read_dir(&t.0).unwrap().next().is_none(), "the plaintext copy is gone");
    s.run("text.insert", &json!({"text": "More confidential edits"})).unwrap();
    save_now(&mut s);
    assert_eq!(s.recovery_documents().len(), 1);
    assert!(s.recovery_documents()[0].protected);
    assert_no_plaintext(&t.0, &[MARKER.as_bytes(), b"confidential"]);
    // Removing protection makes plain copies again, from the next edit.
    s.run("file.encrypt", &json!({"password": null})).unwrap();
    s.run("text.insert", &json!({"text": " unprotected"})).unwrap();
    save_now(&mut s);
    assert_eq!(s.recovery_documents().len(), 1);
    assert!(!s.recovery_documents()[0].protected);
}

#[test]
fn failed_password_cleanup_rejects_protection_and_preserves_snapshot() {
    let t = Temp::new();
    let mut s = session(&t.0);
    s.run("text.insert", &json!({"text": "Synthetic text"})).unwrap();
    save_now(&mut s);
    let id = s.recovery_documents()[0].id.clone();
    let dir = t.0.join(id);
    // An unknown file prevents removal: don't report that plaintext was purged.
    let first = Store::files(&dir).unwrap()[0].clone();
    let bytes = fs::read(&first).unwrap();
    fs::write(dir.join("unexpected"), b"unexpected").unwrap();
    assert!(s.run("file.encrypt", &json!({"password": "synthetic-password"})).is_err());
    assert!(s.password.is_none());
    assert_eq!(fs::read(first).unwrap(), bytes);
}

#[test]
fn delayed_restore_cannot_replace_new_edits() {
    let t = Temp::new();
    let mut s = session(&t.0);
    s.run("text.insert", &json!({"text": "Saved draft"})).unwrap();
    save_now(&mut s);
    drop(s);
    let mut s = session(&t.0);
    let id = s.recovery_documents()[0].id.clone();
    s.run("file.recover", &json!({"restore": id})).unwrap();
    s.run("text.insert", &json!({"text": "New work"})).unwrap();
    settle(&mut s);
    assert_eq!(s.doc.plain_text(StoryRef::Body), "New work");
    assert!(s.recovery_error().unwrap().contains("document changed"));
}

#[test]
fn opening_protected_file_and_password_save_never_leave_plaintext_copies() {
    let t = Temp::new();
    let original = t.0.join("protected.docx");
    let root = t.0.join("profile");
    crate::io::save_path_with(
        &original,
        &Document::from_text("Synthetic confidential note"),
        Some("synthetic-password"),
        &crate::io::ExportOptions::default(),
    )
    .unwrap();
    let bytes = fs::read(&original).unwrap();
    let mut s = session(&root);
    s.run("file.open", &json!({"path": original, "password": "synthetic-password"})).unwrap();
    s.run("text.insert", &json!({"text": format!("{MARKER} edit")})).unwrap();
    save_now(&mut s);
    assert_no_plaintext(&root, &[MARKER.as_bytes(), b"Synthetic confidential note"]);
    assert_eq!(fs::read(&original).unwrap(), bytes);
    let protected = s.recovery_documents()[0].id.clone();
    // The unsaved protected document's copy is kept when another replaces it.
    s.run("file.new", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": "New draft "})).unwrap();
    save_now(&mut s);
    assert_eq!(s.recovery_documents().len(), 2);
    // Saving with a password removes the new draft's plaintext copy before protecting it.
    let saved = t.0.join("saved.docx");
    s.run("file.save", &json!({"path": saved, "password": "synthetic-password"})).unwrap();
    settle(&mut s);
    let left: Vec<_> = s.recovery_documents().into_iter().map(|d| d.id).collect();
    assert_eq!(left, vec![protected]);
    assert_no_plaintext(&root, &[MARKER.as_bytes(), b"New draft"]);
    assert_eq!(fs::read(original).unwrap(), bytes);
}

#[test]
fn protection_after_document_switch_does_not_discard_another_draft() {
    let t = Temp::new();
    let mut s = session(&t.0);
    s.run("text.insert", &json!({"text": "First synthetic draft"})).unwrap();
    save_now(&mut s);
    s.run("file.new", &json!({})).unwrap();
    // Protection can be commanded before the host's next recovery tick.
    s.run("file.encrypt", &json!({"password": "synthetic-password"})).unwrap();
    settle(&mut s);
    assert_eq!(s.recovery_documents().len(), 1);
}

/// `private_dir` sets the mode only on folders it creates: an existing recovery folder that
/// others could read is made private before any copy is written into it.
#[cfg(unix)]
#[test]
fn an_existing_recovery_folder_is_made_private() {
    use std::os::unix::fs::PermissionsExt;
    let t = Temp::new();
    let root = t.0.join("recovery");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    let mut s = session(&root);
    assert_eq!(fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o700);
    s.run("text.insert", &json!({"text": "Private synthetic draft"})).unwrap();
    save_now(&mut s);
    let id = s.recovery_documents()[0].id.clone();
    for f in Store::files(&root.join(&id)).unwrap() {
        assert_eq!(fs::metadata(f).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(fs::metadata(root.join(id)).unwrap().permissions().mode() & 0o777, 0o700);
}

/// A document saved and replaced before the next tick discarded its copy left that copy to
/// be offered as unsaved work at every start; Don't Save left its copy too.
#[test]
fn copies_of_saved_or_abandoned_documents_do_not_outlive_them() {
    let t = Temp::new();
    let root = t.0.join("profile");
    let mut s = session(&root);
    s.run("text.insert", &json!({"text": "Saved synthetic draft"})).unwrap();
    save_now(&mut s);
    assert_eq!(s.recovery_documents().len(), 1);
    // Saved, then replaced in the same frame: no tick saw it clean.
    s.run("file.save", &json!({"path": t.0.join("saved.docx")})).unwrap();
    s.run("file.new", &json!({})).unwrap();
    assert!(s.recovery_wait().is_some(), "the stale copy schedules its own removal");
    save_now(&mut s);
    settle(&mut s);
    assert!(s.recovery_documents().is_empty());
    assert!(fs::read_dir(&root).unwrap().next().is_none(), "the stale copy is gone from disk");

    // Don't Save: the copy goes at once, without waiting for a tick.
    s.run("text.insert", &json!({"text": "Abandoned synthetic draft"})).unwrap();
    save_now(&mut s);
    assert_eq!(s.recovery_documents().len(), 1);
    s.discard_recovery();
    assert!(fs::read_dir(&root).unwrap().next().is_none());

    // An unsaved document replaced without a choice (an agent opening a file) keeps its copy.
    s.run("text.insert", &json!({"text": "Only copy of this work"})).unwrap();
    save_now(&mut s);
    s.run("file.new", &json!({})).unwrap();
    save_now(&mut s);
    assert_eq!(s.recovery_documents().len(), 1);
    drop(s);
    let s = session(&root);
    assert_eq!(s.recovery_documents().len(), 1, "offered at the next start");
}

/// A slow snapshot spaces out the next one, so recovery stays a small share of a core.
#[test]
fn slow_snapshots_space_out_the_next() {
    let t = Temp::new();
    let mut s = session(&t.0);
    s.run("text.insert", &json!({"text": "Synthetic draft"})).unwrap();
    s.recovery.as_mut().unwrap().due = Instant::now();
    s.recovery_tick();
    // As if the snapshot under way had taken 3 s.
    s.recovery.as_mut().unwrap().saving_since = Some(Instant::now() - Duration::from_secs(3));
    settle(&mut s);
    let wait = s.recovery.as_ref().unwrap().due.saturating_duration_since(Instant::now());
    assert!(wait > Duration::from_secs(55), "{wait:?}");
    assert!(wait <= MAX_INTERVAL);
}

/// Measures what a recovery snapshot costs on a large synthetic document (run on demand:
/// `cargo test --release -p wordcraft-engine recovery_cost -- --ignored --nocapture`).
#[test]
#[ignore = "measurement"]
fn recovery_cost_on_a_large_document() {
    let para = "Synthetic recovery measurement text with several ordinary words in it. ".repeat(8);
    let text: Vec<String> = (0..6000).map(|i| format!("{i} {para}")).collect();
    let mut doc = Document::from_text(&text.join("\n"));
    let mut noise = vec![0u8; 27 << 20];
    let mut x = 0x2545_f491_4f6c_dd1du64;
    for b in noise.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x as u8;
    }
    doc.media.insert("media/image1.png".into(), Arc::new(noise));
    let t = Temp::new();
    let mut s = session(&t.0);
    s.set_document(doc);
    s.run("text.insert", &json!({"text": "edit"})).unwrap();
    let t0 = Instant::now();
    let clone = s.doc.clone();
    println!("UI thread: doc.clone() {:?}", t0.elapsed());
    drop(clone);
    let t0 = Instant::now();
    s.recovery.as_mut().unwrap().due = Instant::now();
    s.recovery_tick();
    println!("UI thread: recovery_tick that queues a snapshot {:?}", t0.elapsed());
    settle(&mut s);
    println!("worker: snapshot written in {:?} (error: {:?})", t0.elapsed(), s.recovery_error());
    let size: u64 = walk(&t.0.join(&s.recovery_documents()[0].id));
    println!("snapshot size {} MB", size >> 20);
    drop(s);
    let t0 = Instant::now();
    let s = session(&t.0);
    println!("start-up scan listing {} copies: {:?}", s.recovery_documents().len(), t0.elapsed());
}

/// Bytes this process has passed to `write` so far (Linux `/proc/self/io` `wchar`).
fn bytes_written() -> u64 {
    fs::read_to_string("/proc/self/io")
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("wchar:").map(|v| v.trim().parse().unwrap_or(0))))
        .unwrap_or(0)
}

/// Measures a document with 300 MB of pictures (ten of 30 MB): two snapshots, the second
/// after a small text edit. Run on demand:
/// `cargo test -p wordcraft-engine recovery_cost_300mb -- --ignored --nocapture`.
#[test]
#[ignore = "measurement"]
fn recovery_cost_300mb_media() {
    let mut doc = Document::from_text("Synthetic recovery measurement with large pictures.");
    let mut x = 0x2545_f491_4f6c_dd1du64;
    for i in 0..10 {
        let mut noise = vec![0u8; 30 << 20];
        for b in noise.chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let bytes = x.to_le_bytes();
            b.copy_from_slice(&bytes[..b.len()]);
        }
        doc.media.insert(format!("media/image{i}.png"), Arc::new(noise));
    }
    let t = Temp::new();
    let mut s = session(&t.0);
    s.set_document(doc);
    for round in 1..=4 {
        // Rounds 3 and 4: the same document protected with a password (encrypted copies).
        if round == 3 {
            s.run("file.encrypt", &json!({"password": "synthetic-pw"})).unwrap();
        }
        s.run("text.insert", &json!({"text": "edit "})).unwrap();
        let w0 = bytes_written();
        let t0 = Instant::now();
        save_now(&mut s);
        let elapsed = t0.elapsed();
        let written = bytes_written() - w0;
        let on_disk: u64 = walk(&t.0);
        println!(
            "snapshot {round}: {elapsed:?}, {} MB written, {} MB on disk, copies {}, error {:?}",
            written >> 20,
            on_disk >> 20,
            s.recovery_documents().len(),
            s.recovery_error()
        );
    }
}

fn walk(p: &Path) -> u64 {
    fs::read_dir(p)
        .map(|d| {
            d.flatten()
                .map(|e| {
                    let m = e.metadata().unwrap();
                    if m.is_dir() { walk(&e.path()) } else { m.len() }
                })
                .sum()
        })
        .unwrap_or(0)
}

// ---- Protected documents (encrypted copies) and pictures in side files ----

const MARKER: &str = "PLAINTEXT-MARKER-7f3a91";
const MEDIA_MARKER: &[u8] = b"MEDIA-PLAINTEXT-MARKER-c0ffee";

/// Every byte under `root`, file by file.
fn all_bytes(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    for e in fs::read_dir(root).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(all_bytes(&p));
        } else {
            out.push((p.clone(), fs::read(&p).unwrap()));
        }
    }
    out
}
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}
/// No file in the recovery folder holds any of `needles`, and none is a plain generation or
/// side file (where pictures would be, as they are, or base64 in JSON).
fn assert_no_plaintext(root: &Path, needles: &[&[u8]]) {
    let files = all_bytes(root);
    assert!(!files.is_empty(), "a copy was written");
    for (path, bytes) in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(!name.ends_with(".json") && !name.ends_with(".bin"), "{name} isn't encrypted");
        for n in needles {
            assert!(!contains(&bytes, n), "{} holds plaintext {:?}", path.display(), String::from_utf8_lossy(n));
        }
    }
}
fn picture(seed: u64, len: usize) -> Arc<Vec<u8>> {
    let mut x = seed | 1;
    Arc::new(
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect(),
    )
}
fn with_pictures(text: &str, pictures: &[(&str, Arc<Vec<u8>>)]) -> Snapshot {
    let mut s = snapshot(&new_id().unwrap(), text);
    for (name, bytes) in pictures {
        s.document.media.insert((*name).into(), bytes.clone());
    }
    s
}
fn part_files(dir: &Path) -> Vec<String> {
    Store::parts(dir).unwrap().into_keys().collect()
}

/// Gap 1: a password-protected document got no recovery at all. Its copy is now encrypted
/// with the document's password; nothing of its text, pictures or file name reaches the disk
/// in plain form; the list shows only "Protected document"; restoring asks for the password,
/// and a wrong one is an error that keeps the copy.
#[test]
fn protected_documents_get_an_encrypted_copy_restored_with_their_password() {
    let t = Temp::new();
    let files = Temp::new();
    let original = files.0.join("synthetic-merger-plan.docx");
    crate::io::save_path_with(
        &original,
        &Document::from_text("Synthetic protected note"),
        Some("synthetic-pw"),
        &crate::io::ExportOptions::default(),
    )
    .unwrap();
    let mut s = session(&t.0);
    s.run("file.open", &json!({"path": original, "password": "synthetic-pw"})).unwrap();
    s.run("text.insert", &json!({"text": format!("{MARKER} unsaved words ")})).unwrap();
    s.doc.media.insert("media/secret.png".into(), Arc::new(MEDIA_MARKER.repeat(500)));
    save_now(&mut s);
    assert_eq!(s.recovery_error(), None);
    let listed = s.recovery_documents();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, PROTECTED_NAME);
    assert!(listed[0].protected && listed[0].path.is_none());
    assert_no_plaintext(&t.0, &[MARKER.as_bytes(), MEDIA_MARKER, b"merger", b"secret.png"]);
    let id = listed[0].id.clone();
    let dir = t.0.join(&id);
    assert!(Store::files(&dir).unwrap().iter().all(|p| sealed(p)), "only encrypted generations");
    assert_eq!(part_files(&dir).len(), 1);
    assert!(part_files(&dir)[0].ends_with(".enc"));
    let expected = s.doc.clone();
    drop(s);

    let mut s = session(&t.0);
    let before = all_bytes(&t.0);
    // No password: asked for, like opening a protected file.
    let err = s.run("file.recover", &json!({"restore": id})).unwrap_err().to_string();
    assert!(crate::io::needs_password(&err), "{err}");
    // A wrong one: an error, never a crash, and the copy stays.
    s.run("file.recover", &json!({"restore": id, "password": "not-it"})).unwrap();
    settle(&mut s);
    assert!(crate::io::wrong_password(s.recovery_error().unwrap()), "{:?}", s.recovery_error());
    assert_eq!(s.take_recovery_wrong_password().as_deref(), Some(id.as_str()), "the front end asks again");
    assert_eq!(s.take_recovery_wrong_password(), None);
    assert_eq!(s.recovery_documents().len(), 1);
    assert_eq!(
        all_bytes(&t.0).into_iter().filter(|(p, _)| !p.ends_with("owner.lock")).collect::<Vec<_>>(),
        before.into_iter().filter(|(p, _)| !p.ends_with("owner.lock")).collect::<Vec<_>>()
    );
    // The right one restores the text and the picture, and the document stays protected.
    s.run("file.recover", &json!({"restore": id, "password": "synthetic-pw"})).unwrap();
    settle(&mut s);
    assert_eq!(s.recovery_error(), None);
    assert_eq!(s.doc, expected);
    assert_eq!(s.password.as_ref().map(|p| p.as_str()), Some("synthetic-pw"));
    assert!(s.dirty && s.path.is_none());
    // Later copies go on into the same record, still encrypted.
    s.run("text.insert", &json!({"text": "more "})).unwrap();
    save_now(&mut s);
    assert_eq!(s.recovery_documents().len(), 1);
    assert_eq!(s.recovery_documents()[0].id, id);
    assert_no_plaintext(&t.0, &[MARKER.as_bytes(), MEDIA_MARKER, b"merger"]);
    assert_eq!(part_files(&dir).len(), 1, "the unchanged picture isn't written again");
}

/// A damaged newest protected generation (discovery can't check it without the password)
/// restores the retained earlier one; a wrong password is still just an error.
#[test]
fn a_damaged_protected_generation_falls_back_to_the_earlier_one() {
    let t = Temp::new();
    let mut store = Store::new(t.0.clone()).unwrap();
    let pw = Password::new("synthetic-pw");
    let id = new_id().unwrap();
    store.save(snapshot(&id, "First protected copy"), Some(&pw), None, Limits::default()).unwrap();
    store.save(snapshot(&id, "Second protected copy"), Some(&pw), None, Limits::default()).unwrap();
    let newest = store.entries[&id].path.clone();
    let mut bytes = fs::read(&newest).unwrap();
    let (_, start) = sealed_header(&bytes).unwrap();
    let n = bytes.len();
    for b in &mut bytes[start + 512..n - 512] {
        *b ^= 0x5a;
    }
    fs::write(&newest, bytes).unwrap();
    drop(store);
    let mut store = Store::new(t.0.clone()).unwrap();
    store.scan().unwrap();
    assert!(crate::io::wrong_password(&store.load(&id, Some(&Password::new("wrong"))).err().unwrap()));
    let restored = load_retrying_with(&mut store, &id, Some(&pw));
    assert_eq!(restored.document.plain_text(StoryRef::Body), "First protected copy");
    // The restore says it is the earlier copy (`RESTORED_EARLIER`).
    assert!(restored.info.earlier);
}

fn load_retrying_with(store: &mut Store, id: &str, pw: Option<&Password>) -> Snapshot {
    let mut last = String::new();
    for _ in 0..50 {
        match store.load(id, pw) {
            Ok(snapshot) => return snapshot,
            Err(e) => last = e,
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the record never loaded: {last}");
}

/// Changing the password encrypts everything again: a copy never needs an old password.
#[test]
fn a_new_password_purges_copies_under_the_old_one() {
    let t = Temp::new();
    let mut s = session(&t.0);
    s.run("file.encrypt", &json!({"password": "first-pw"})).unwrap();
    s.run("text.insert", &json!({"text": "Synthetic"})).unwrap();
    s.doc.media.insert("media/a.png".into(), picture(3, 4096));
    save_now(&mut s);
    let first = s.recovery_documents()[0].id.clone();
    s.run("file.encrypt", &json!({"password": "second-pw"})).unwrap();
    assert!(!t.0.join(&first).exists(), "the copy under the old password is removed");
    s.run("text.insert", &json!({"text": " more"})).unwrap();
    save_now(&mut s);
    let id = s.recovery_documents()[0].id.clone();
    let expected = s.doc.clone();
    drop(s);
    let mut s = session(&t.0);
    s.run("file.recover", &json!({"restore": id, "password": "second-pw"})).unwrap();
    settle(&mut s);
    assert_eq!(s.doc, expected);
}

/// Gap 2: pictures are side files named by their content, written once and shared by later
/// generations; the JSON only refers to them.
#[test]
fn pictures_are_side_files_written_once() {
    let t = Temp::new();
    let mut store = Store::new(t.0.clone()).unwrap();
    let big = picture(1, 300_000);
    let small = picture(2, 1000);
    let mut s = with_pictures(
        "First synthetic text",
        &[("media/big.png", big.clone()), ("media/small.png", small.clone()), ("media/copy.png", small.clone())],
    );
    let id = s.info.id.clone();
    s.document.passthrough.insert("word/embeddings/part.bin".into(), picture(9, 500));
    let w0 = store.written;
    assert_eq!(store.save(s, None, None, Limits::default()).unwrap(), Vec::<String>::new());
    let first = store.written - w0;
    let dir = t.0.join(&id);
    assert_eq!(part_files(&dir).len(), 3, "the same bytes twice are one file");
    let main = Store::files(&dir).unwrap()[0].clone();
    assert!(fs::metadata(&main).unwrap().len() < 100_000, "the JSON doesn't hold the pictures");
    assert!(first >= 300_000);
    // Text edited, pictures unchanged: only the text is written.
    let mut s =
        with_pictures("Second synthetic text", &[("media/big.png", big.clone()), ("media/small.png", small.clone()), ("media/copy.png", small)]);
    s.info.id = id.clone();
    s.document.passthrough.insert("word/embeddings/part.bin".into(), picture(9, 500));
    let w1 = store.written;
    store.save(s, None, None, Limits::default()).unwrap();
    assert!(store.written - w1 < 50_000, "{} bytes written for a text edit", store.written - w1);
    let loaded = store.load(&id, None).unwrap();
    assert_eq!(loaded.document.plain_text(StoryRef::Body), "Second synthetic text");
    assert_eq!(loaded.document.media["media/big.png"], big);
    assert_eq!(loaded.document.media.len(), 3);
    assert_eq!(loaded.document.passthrough.len(), 1);
    // A picture removed: its file goes once neither kept generation uses it.
    for text in ["Third", "Fourth"] {
        let mut s = snapshot(&id, text);
        s.document.media.insert("media/big.png".into(), big.clone());
        store.save(s, None, None, Limits::default()).unwrap();
    }
    assert_eq!(part_files(&dir).len(), 1);
}

/// Over the budget, the text and structure are still kept, with the pictures that fit; the
/// copy says which it left out. Nothing too large is written and then deleted.
#[test]
fn over_budget_keeps_the_text_and_names_the_pictures_left_out() {
    let t = Temp::new();
    let mut store = Store::new(t.0.clone()).unwrap();
    let limits = Limits { main: 1 << 20, record: 400 << 10, disk: 1 << 30 };
    let s = with_pictures(
        "Synthetic text that must endure",
        &[("media/a.png", picture(1, 50_000)), ("media/b.png", picture(2, 100_000)), ("media/huge.png", picture(3, 10 << 20))],
    );
    let id = s.info.id.clone();
    let omitted = store.save(s, None, None, limits).unwrap();
    assert_eq!(omitted, vec!["media/huge.png".to_string()]);
    assert!(store.written < 400 << 10, "{} bytes written: the huge picture was never written", store.written);
    let loaded = store.load(&id, None).unwrap();
    assert_eq!(loaded.document.plain_text(StoryRef::Body), "Synthetic text that must endure");
    assert_eq!(loaded.document.media.len(), 2);
    assert_eq!(loaded.info.omitted, vec!["media/huge.png".to_string()]);
    assert_eq!(store.inventory()[0].omitted, vec!["media/huge.png".to_string()]);
    // Text alone over the budget: refused before writing anything; the last copy stays.
    let before = fs::read(&store.entries[&id].path).unwrap();
    let w = store.written;
    let tight = Limits { main: 1 << 20, record: 1024, disk: 1 << 30 };
    assert!(store.save(snapshot(&id, &"long text ".repeat(1000)), None, None, tight).is_err());
    assert_eq!(store.written, w);
    assert_eq!(fs::read(&store.entries[&id].path).unwrap(), before);
}

/// The session reports what its copy can't cover, for the status bar.
#[test]
fn the_session_reports_what_recovery_cannot_cover() {
    let t = Temp::new();
    let mut s = session(&t.0);
    s.recovery.as_mut().unwrap().limits = Limits { main: 1 << 20, record: 200 << 10, disk: 1 << 30 };
    s.run("text.insert", &json!({"text": "Synthetic"})).unwrap();
    save_now(&mut s);
    assert_eq!(s.recovery_gap(), None);
    s.doc.media.insert("media/huge.png".into(), picture(5, 1 << 20));
    s.run("text.insert", &json!({"text": " edit"})).unwrap();
    save_now(&mut s);
    assert_eq!(s.recovery_gap(), Some(crate::recovery::RecoveryGap::Omitted(vec!["media/huge.png".into()])));
    assert_eq!(s.run("file.recover", &json!({})).unwrap()["gap"]["omitted"][0], "media/huge.png");
    // Saved: the file protects it, nothing to report.
    s.run("file.save", &json!({"path": t.0.join("saved.json")})).unwrap();
    assert_eq!(s.recovery_gap(), None);
    // The text alone doesn't fit: a failure, reported until a copy succeeds.
    s.recovery.as_mut().unwrap().limits = Limits { main: 1 << 20, record: 10, disk: 1 << 30 };
    s.run("text.insert", &json!({"text": " more"})).unwrap();
    save_now(&mut s);
    assert!(matches!(s.recovery_gap(), Some(crate::recovery::RecoveryGap::Failed(_))));
}

/// Copies written before pictures moved to side files still open: version 1 with pictures
/// inline as base64, and as arrays of numbers (older still).
#[test]
fn old_copies_with_inline_pictures_still_load() {
    let t = Temp::new();
    let bytes = vec![0u8, 1, 2, 250, 255];
    for numbers in [false, true] {
        let id = new_id().unwrap();
        let mut doc = Document::from_text("Old synthetic copy");
        doc.media.insert("media/old.png".into(), Arc::new(bytes.clone()));
        let mut value = json!({
            "version": 1, "generation": 1,
            "info": {"id": id, "name": "Synthetic draft", "savedAt": "2026-01-01T00:00:00Z"},
            "document": doc,
        });
        if numbers {
            value["document"]["media"]["media/old.png"] = json!(bytes);
        }
        let dir = t.0.join(&id);
        private_dir(&dir).unwrap();
        fs::write(dir.join("snapshot-00000000000000000001.json"), serde_json::to_vec(&value).unwrap()).unwrap();
        let mut store = Store::new(t.0.clone()).unwrap();
        store.scan().unwrap();
        let loaded = store.load(&id, None).unwrap();
        assert_eq!(loaded.document.plain_text(StoryRef::Body), "Old synthetic copy");
        assert_eq!(*loaded.document.media["media/old.png"], bytes, "numbers: {numbers}");
        // The next copy of it moves the picture to a side file.
        store.save(loaded, None, None, Limits::default()).unwrap();
        assert_eq!(part_files(&dir).len(), 1);
    }
}

/// A damaged or missing picture file costs that picture, never the text.
#[test]
fn a_damaged_picture_file_leaves_out_the_picture_not_the_text() {
    let t = Temp::new();
    let mut store = Store::new(t.0.clone()).unwrap();
    let s = with_pictures("Synthetic text", &[("media/a.png", picture(1, 2000)), ("media/b.png", picture(2, 2000))]);
    let id = s.info.id.clone();
    store.save(s, None, None, Limits::default()).unwrap();
    let dir = t.0.join(&id);
    let parts = part_files(&dir);
    fs::write(dir.join(&parts[0]), b"damaged").unwrap();
    drop(store);
    let mut store = Store::new(t.0.clone()).unwrap();
    store.scan().unwrap();
    let loaded = load_retrying(&mut store, &id);
    assert_eq!(loaded.document.plain_text(StoryRef::Body), "Synthetic text");
    assert_eq!(loaded.document.media.len(), 1);
    assert_eq!(loaded.info.omitted.len(), 1);
}

/// A copy naming a side file outside its record (`../`) is unreadable, not followed.
#[test]
fn hostile_part_names_are_refused() {
    let t = Temp::new();
    let id = new_id().unwrap();
    let dir = t.0.join(&id);
    private_dir(&dir).unwrap();
    for (n, file) in ["../../outside", "part-zz.bin", "/etc/passwd"].into_iter().enumerate() {
        let value = json!({
            "version": 2, "generation": n + 1,
            "info": {"id": id, "name": "Synthetic draft", "savedAt": "2026-01-01T00:00:00Z"},
            "document": Document::from_text("x"),
            "parts": {"media": {"a.png": {"file": file, "sha256": "00", "len": 1}}},
        });
        fs::write(dir.join(format!("snapshot-{:020}.json", n + 1)), serde_json::to_vec(&value).unwrap()).unwrap();
    }
    let mut store = Store::new(t.0.clone()).unwrap();
    assert!(store.scan().is_err());
    assert!(store.inventory().is_empty());
    for bad in [
        "../x",
        "part-.bin",
        "part-0000000000000000000000000000000000000000000000000000000000000000.exe",
        "PART-0000000000000000000000000000000000000000000000000000000000000000.bin",
    ] {
        assert!(!part_ok(bad), "{bad}");
    }
    assert!(part_ok("part-0000000000000000000000000000000000000000000000000000000000000000.bin"));
    // A hostile sealed header: an error, not a panic.
    for bytes in [&b"WordCraft protected recovery copy\n{"[..], b"WordCraft protected recovery copy\n", b"\0\0\0"] {
        assert!(sealed_header(bytes).is_err());
    }
}

/// Pictures around the budgets (scaled down: a record of 256 KiB, a folder of 384 KiB of
/// which another record already holds 150 KiB), plain and protected: every copy restores to
/// the document it was made of, less exactly the pictures it names as left out (nothing it
/// keeps is damaged or missing), and its record and the folder stay within their budgets.
#[test]
fn copies_near_the_budgets_restore_whole_or_name_what_they_left_out() {
    let t = Temp::new();
    let mut store = Store::new(t.0.clone()).unwrap();
    let limits = Limits { main: 1 << 20, record: 256 << 10, disk: 384 << 10 };
    let other = with_pictures("Another synthetic document", &[("media/other.png", picture(9, 150 << 10))]);
    store.save(other, None, None, limits).unwrap();
    let mut kept = 0;
    let mut left_out = 0;
    for password in [None, Some(Password::new("synthetic-budget-pass"))] {
        for size in [10usize << 10, 60 << 10, 120 << 10, 160 << 10, 200 << 10, 220 << 10, 228 << 10, 232 << 10, 240 << 10, 300 << 10] {
            let s = with_pictures(
                "Synthetic text that must endure",
                &[("media/a.png", picture(size as u64, size)), ("media/small.png", picture(1, 1000))],
            );
            let id = s.info.id.clone();
            let made_of = s.document.clone();
            let omitted = store.save(s, password.as_ref(), None, limits).unwrap();
            let loaded = store.load(&id, password.as_ref()).unwrap();
            let mut want = made_of.clone();
            for name in &omitted {
                want.media.remove(name);
            }
            let what = format!("{size} bytes, protected {}", password.is_some());
            assert_eq!(loaded.document, want, "{what}: omitted {omitted:?}");
            assert_eq!(loaded.info.omitted, omitted, "{what}");
            assert!(walk(&t.0.join(&id)) <= limits.record, "{what}: the record is over its budget");
            assert!(walk(&t.0) <= limits.disk, "{what}: the folder is over its budget");
            if omitted.iter().any(|n| n == "media/a.png") {
                left_out += 1;
            } else {
                kept += 1;
            }
            store.discard(&id).unwrap();
        }
    }
    assert!(kept > 0 && left_out > 0, "kept {kept}, left out {left_out}: the boundary was crossed");
}

/// 09: the recovery budgets follow from the document limits (`wordcraft_doc::limits`): a
/// document's copy fits a generation file of its saved-size estimate and [`PART_LISTING`] a
/// side file, and two generations and its pictures fit a record of twice that and the
/// pictures' bytes, nothing left out. The same sums size [`MAX_BYTES`] and [`RECORD_BUDGET`]
/// for a document at the limits (asserted where they are defined).
#[test]
fn recovery_budgets_follow_from_the_document_limits() {
    let doc = crate::tests_limits::generated(
        "paras=400,runs=3,chars=8,notes=20,comments=20,styles=20,lists=5,revisions=30,sources=5,pictures=20,picbytes=3000",
    );
    let t = wordcraft_doc::limits::Tally::of(&doc);
    let parts = (doc.media.len() + doc.passthrough.len()) as u64;
    let main = t.saved_bytes + PART_LISTING * parts + (64 << 10);
    let limits = Limits { main, record: 2 * main + t.media_bytes, disk: 1 << 30 };
    let tmp = Temp::new();
    let mut store = Store::new(tmp.0.clone()).unwrap();
    let id = new_id().unwrap();
    let generations: Vec<Document> = (0..3)
        .map(|i| {
            let mut d = doc.clone();
            d.body.push(wordcraft_doc::para_block(wordcraft_doc::Paragraph::with_text(&format!("Edit {i}"), Default::default())));
            d
        })
        .collect();
    for (i, d) in generations.iter().enumerate() {
        let mut s = snapshot(&id, "");
        s.document = d.clone();
        let omitted = store.save(s, None, None, limits).unwrap();
        assert!(omitted.is_empty(), "generation {i}: {omitted:?} left out");
    }
    assert_eq!(Some(&store.load(&id, None).unwrap().document), generations.last());
    assert!(walk(&tmp.0.join(&id)) <= limits.record);
}

/// 09: a recovery copy of a document at every limit at once is written whole, nothing left
/// out, two generations of it within the record's budget, and loads back the same.
#[test]
#[ignore = "a document at every limit at once: minutes and some GB"]
fn a_recovery_copy_at_the_limits_loads_back() {
    let doc = crate::tests_limits::generated(crate::tests_limits::AT_LIMITS);
    let tmp = Temp::new();
    let mut store = Store::new(tmp.0.clone()).unwrap();
    let id = new_id().unwrap();
    let mut last = doc.clone();
    for (i, letter) in ["x", "y"].into_iter().enumerate() {
        let t0 = std::time::Instant::now();
        if let Some(wordcraft_doc::Block::Para(p)) = last.body.first_mut().map(Arc::make_mut) {
            p.text.replace_range(0..1, letter);
        }
        let mut s = snapshot(&id, "");
        s.document = last.clone();
        let omitted = store.save(s, None, None, Limits::default()).unwrap();
        assert!(omitted.is_empty(), "generation {i}: {} left out", omitted.len());
        eprintln!("generation {i}: {:.1} s, peak {:.0} MB", t0.elapsed().as_secs_f64(), crate::tests_limits::peak_mb());
    }
    drop(doc);
    let t0 = std::time::Instant::now();
    let loaded = store.load(&id, None).unwrap();
    eprintln!(
        "loaded in {:.1} s, peak {:.0} MB, record {} MB",
        t0.elapsed().as_secs_f64(),
        crate::tests_limits::peak_mb(),
        walk(&tmp.0.join(&id)) >> 20
    );
    assert!(loaded.document == last, "the copy at the limits isn't the document");
}
