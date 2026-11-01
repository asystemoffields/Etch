//! Saving and recovery under injected faults (`io::fault`): a full disk, a read-only folder, a
//! failing drive or another program at each durable step of a write. After every failure the
//! file on disk is the one that was there, the person's edits are still in the session, the
//! error says what went wrong, and no temporary file is left behind. Synthetic documents only,
//! each test in a folder of its own.
#![cfg(not(target_arch = "wasm32"))]

use crate::Session;
use crate::io::fault::{self, Act, Step};
use serde_json::json;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wordcraft_doc::{Document, StoryRef};

struct Scratch(PathBuf);
impl Scratch {
    fn new(what: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("wordcraft-save-safety-{what}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    /// The names in the folder, sorted.
    fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(&self.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fault::clear(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const ORIGINAL: &str = "Original synthetic letter.";
const EDIT: &str = " Edited sentinel words.";

/// A `.docx` at `path` holding [`ORIGINAL`], opened in a session that has typed [`EDIT`].
fn edited_session(path: &Path) -> (Session, Vec<u8>) {
    let bytes = crate::io::save_bytes("letter.docx", &Document::from_text(ORIGINAL)).unwrap();
    std::fs::write(path, &bytes).unwrap();
    let mut s = Session::new(Document::new());
    s.run("file.open", &json!({"path": path})).unwrap();
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": EDIT})).unwrap();
    assert!(s.dirty);
    (s, bytes)
}

fn body_text(path: &Path) -> String {
    crate::io::open_path(path).unwrap().plain_text(StoryRef::Body)
}

/// The faults a save can meet before its file replaces the old one, and how each is described.
fn save_faults() -> Vec<(Step, Act, &'static str)> {
    vec![
        (Step::Create, Act::Fail(ErrorKind::PermissionDenied), "a folder the user can't write"),
        (Step::Create, Act::Fail(ErrorKind::ReadOnlyFilesystem), "a read-only disk"),
        (Step::Write, Act::Fail(ErrorKind::StorageFull), "a full disk"),
        (Step::Write, Act::Partial(ErrorKind::StorageFull), "a disk that fills up midway"),
        (Step::SyncFile, Act::Fail(ErrorKind::Other), "a drive that fails to synchronize"),
        (Step::Rename, Act::Fail(ErrorKind::PermissionDenied), "a rename refused"),
        (Step::Rename, Act::Fail(ErrorKind::ReadOnlyFilesystem), "a disk made read-only during the save"),
    ]
}

/// Save (and AutoSave, which writes through the same `save_path_versioned`) over the document's
/// own file, failing at each step: the file keeps its bytes, the folder holds nothing new, the
/// session keeps its edits, its path, its save stamp and the file's identity, and says why; the
/// next save, once the fault is gone, writes the edits.
#[test]
fn a_save_that_fails_at_any_step_keeps_the_file_and_the_edits() {
    for (step, act, what) in save_faults() {
        let d = Scratch::new("save");
        let path = d.path("letter.docx");
        let (mut s, original) = edited_session(&path);
        let revision = s.doc.core.revision;
        fault::inject(&d.0, step, 1, act);
        let e = s.run("file.save", &json!({})).expect_err(what).to_string();
        assert!(fault::spent(&d.0), "{what}: the fault was met");
        assert!(e.contains("injected"), "{what}: the error says why: {e}");
        assert_eq!(std::fs::read(&path).unwrap(), original, "{what}: the file is untouched");
        assert_eq!(d.names(), ["letter.docx"], "{what}: no temporary file is left");
        assert!(s.dirty, "{what}: the edits are still unsaved");
        assert_eq!(s.path.as_deref(), Some(path.as_path()), "{what}");
        assert_eq!(s.doc.core.revision, revision, "{what}: a failed save doesn't stamp the document");
        assert!(s.doc.plain_text(StoryRef::Body).ends_with(EDIT.trim_start()), "{what}");
        // The fault is gone: the same session saves, as the file is still the version it opened.
        s.run("file.save", &json!({})).unwrap_or_else(|e| panic!("{what}: the next save: {e}"));
        assert!(!s.dirty);
        assert_eq!(body_text(&path), format!("{ORIGINAL}{EDIT}"), "{what}");
        assert_eq!(d.names(), ["letter.docx"], "{what}");
    }
}

/// Save As to a new file that fails at any step leaves no file there, and the session on its own
/// file with its edits unsaved.
#[test]
fn a_save_as_that_fails_at_any_step_makes_no_file() {
    for (step, act, what) in save_faults() {
        let d = Scratch::new("save-as");
        let path = d.path("letter.docx");
        let (mut s, original) = edited_session(&path);
        fault::inject(&d.0, step, 1, act);
        let copy = d.path("copy.docx");
        let e = s.run("file.saveAs", &json!({"path": copy})).expect_err(what).to_string();
        assert!(e.contains("injected"), "{what}: {e}");
        assert!(!copy.exists(), "{what}: no file where the save failed");
        assert_eq!(d.names(), ["letter.docx"], "{what}");
        assert_eq!(std::fs::read(&path).unwrap(), original, "{what}");
        assert!(s.dirty, "{what}");
        assert_eq!(s.path.as_deref(), Some(path.as_path()), "{what}: the document is still its own file's");
    }
}

/// A folder that can't be synchronized after the rename (a failing disk, some network mounts)
/// doesn't make a written save a failure: the new bytes are in place, and the session expects
/// them (were it reported as failed, the next save would be refused as someone else's change).
#[test]
fn a_folder_that_fails_to_synchronize_after_the_rename_still_saves() {
    let d = Scratch::new("sync-dir");
    let path = d.path("letter.docx");
    let (mut s, _) = edited_session(&path);
    fault::inject(&d.0, Step::SyncDir, 1, Act::Fail(ErrorKind::Other));
    s.run("file.save", &json!({})).unwrap();
    assert!(fault::spent(&d.0));
    assert!(!s.dirty);
    assert_eq!(body_text(&path), format!("{ORIGINAL}{EDIT}"));
    s.run("text.insert", &json!({"text": " More."})).unwrap();
    s.run("file.save", &json!({})).unwrap();
    assert_eq!(body_text(&path), format!("{ORIGINAL}{EDIT} More."));
}

/// Another program replaces the file while the save is being prepared (after the bytes are
/// written to the temporary file, before the rename): the save is refused as a conflict, the
/// other program's file stays, and nothing is left behind.
#[test]
fn a_file_replaced_while_the_save_is_prepared_is_not_overwritten() {
    let d = Scratch::new("replaced");
    let path = d.path("letter.docx");
    let (mut s, _) = edited_session(&path);
    let theirs = crate::io::save_bytes("theirs.docx", &Document::from_text("Another program's synthetic version.")).unwrap();
    let (target, bytes) = (path.clone(), theirs.clone());
    fault::inject(
        &d.0,
        Step::Verify,
        1,
        Act::Run(Box::new(move || {
            // Written the way editors save: a new file renamed over the old one.
            let tmp = target.with_file_name("other-program.tmp");
            std::fs::write(&tmp, &bytes).unwrap();
            std::fs::rename(&tmp, &target).unwrap();
        })),
    );
    let e = s.run("file.save", &json!({})).unwrap_err();
    assert!(matches!(e, crate::CmdError::FileConflict(_)), "{e}");
    assert_eq!(std::fs::read(&path).unwrap(), theirs);
    assert_eq!(d.names(), ["letter.docx"]);
    assert!(s.dirty);
    // The edits can still go to a file of their own.
    s.run("file.saveAs", &json!({"path": d.path("mine.docx")})).unwrap();
    assert_eq!(body_text(&d.path("mine.docx")), format!("{ORIGINAL}{EDIT}"));
    assert_eq!(std::fs::read(&path).unwrap(), theirs);
}

/// Exports (PDF, a page image, Print to PDF) replace an earlier export only once the new one is
/// complete: a failure at any step leaves the earlier export whole and nothing else.
#[test]
fn an_export_that_fails_keeps_the_previous_export() {
    let exports: [(&str, &str, serde_json::Value); 3] = [
        ("file.exportPdf", "copy.pdf", json!({})),
        ("file.exportPng", "page.png", json!({"page": 1, "scale": 0.5})),
        ("file.printPdf", "printed.pdf", json!({})),
    ];
    for (id, name, extra) in exports {
        for (step, act, what) in save_faults() {
            let d = Scratch::new("export");
            let out = d.path(name);
            std::fs::write(&out, b"previous export").unwrap();
            let mut s = Session::new(Document::from_text("Synthetic export text."));
            let mut params = extra.clone();
            params["path"] = json!(out);
            fault::inject(&d.0, step, 1, act);
            let e = s.run(id, &params).expect_err(what).to_string();
            assert!(e.contains("injected"), "{id}, {what}: {e}");
            assert_eq!(std::fs::read(&out).unwrap(), b"previous export", "{id}, {what}");
            assert_eq!(d.names(), [name], "{id}, {what}");
        }
    }
}

/// What the desktop's AutoSave does (`ui-egui` `start_autosave`/`finish_autosave_job`): the
/// document is copied, written on a worker thread with the version it expects, and the result
/// collected later. Typing goes on while the write is under way (held here, deterministically,
/// at the moment before the rename): the file gets the copy, the session keeps what was typed
/// since, still unsaved, and the next save (expecting the version AutoSave wrote) writes it.
#[test]
fn typing_during_an_autosave_write_is_neither_lost_nor_marked_saved() {
    let d = Scratch::new("autosave");
    let path = d.path("letter.docx");
    let (mut s, _) = edited_session(&path);
    s.run("file.save", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": " Typed before AutoSave."})).unwrap();

    let (reached, wait_reached) = std::sync::mpsc::channel::<()>();
    let (release, wait_release) = std::sync::mpsc::channel::<()>();
    fault::inject(
        &d.0,
        Step::Verify,
        1,
        Act::Run(Box::new(move || {
            let _ = reached.send(());
            let _ = wait_release.recv_timeout(Duration::from_secs(30));
        })),
    );
    s.stamp_save();
    let (doc, rev, expected, target) = (s.doc.clone(), s.rev(), s.file_version().cloned(), path.clone());
    let worker =
        std::thread::spawn(move || crate::io::save_path_versioned(&target, &doc, None, &crate::io::ExportOptions::default(), expected.as_ref()));
    wait_reached.recv_timeout(Duration::from_secs(30)).unwrap();
    // The worker is between preparing its file and replacing the old one: type meanwhile.
    s.run("text.insert", &json!({"text": " Typed during AutoSave."})).unwrap();
    s.run("text.newParagraph", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": "A new paragraph."})).unwrap();
    release.send(()).unwrap();
    let version = worker.join().unwrap().unwrap();
    // `finish_autosave_job`: the file holds what was written; clean only if nothing changed.
    s.set_file_version(version);
    if s.rev() == rev {
        s.dirty = false;
    }
    assert!(s.dirty, "what was typed during the write is still unsaved");
    let on_disk = body_text(&path);
    assert!(on_disk.ends_with("Typed before AutoSave."), "the file got the copy: {on_disk:?}");
    s.run("file.save", &json!({})).unwrap();
    let on_disk = body_text(&path);
    assert!(on_disk.contains("Typed during AutoSave.") && on_disk.ends_with("A new paragraph."), "{on_disk:?}");
    assert_eq!(d.names(), ["letter.docx"]);
}

/// AutoSave's write on its worker thread meets the same faults as a Save, with the same result:
/// the file is untouched and the error comes back to be shown.
#[test]
fn an_autosave_write_that_fails_on_its_worker_keeps_the_file() {
    for (step, act, what) in save_faults() {
        let d = Scratch::new("autosave-fault");
        let path = d.path("letter.docx");
        let (mut s, original) = edited_session(&path);
        fault::inject(&d.0, step, 1, act);
        let (doc, expected, target) = (s.doc.clone(), s.file_version().cloned(), path.clone());
        let r =
            std::thread::spawn(move || crate::io::save_path_versioned(&target, &doc, None, &crate::io::ExportOptions::default(), expected.as_ref()))
                .join()
                .unwrap();
        let e = r.expect_err(what).to_string();
        assert!(e.contains("injected"), "{what}: {e}");
        assert_eq!(std::fs::read(&path).unwrap(), original, "{what}");
        assert_eq!(d.names(), ["letter.docx"], "{what}");
        // The session still expects the file it opened: its own Save goes ahead.
        s.run("file.save", &json!({})).unwrap();
        assert_eq!(body_text(&path), format!("{ORIGINAL}{EDIT}"), "{what}");
    }
}

// Recovery copies.

fn recovery_session(root: &Path) -> Session {
    let mut s = Session::new(Document::new());
    s.enable_recovery(root.to_path_buf(), Arc::new(|| {})).unwrap();
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

/// The copy a new session (the next start) restores from `root`.
fn restored(root: &Path) -> Document {
    let mut s = recovery_session(root);
    settle(&mut s);
    let docs = s.recovery_documents();
    assert_eq!(docs.len(), 1, "{docs:?}");
    s.run("file.recover", &json!({"restore": docs[0].id})).unwrap();
    settle(&mut s);
    s.doc.clone()
}

/// Files under `dir` whose name starts with `prefix`, in every record.
fn files_named(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(files_named(&p, prefix));
        } else if p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(prefix)) {
            out.push(p);
        }
    }
    out
}

/// A recovery copy that fails at any step of writing its generation or a picture's side file
/// (a full disk, a read-only profile, a drive that reads back other bytes) keeps the last good
/// copy, which the next start restores whole; the session says no copy could be written (the red
/// "Not protected by recovery"), no temporary file is left, and the next copy is written once
/// the fault is gone. Pictures are side files written before the generation that uses them.
#[test]
fn a_recovery_copy_that_fails_at_any_step_keeps_the_last_good_one() {
    let faults = [
        (Step::Create, Act::Fail(ErrorKind::ReadOnlyFilesystem)),
        (Step::Write, Act::Partial(ErrorKind::StorageFull)),
        (Step::SyncFile, Act::Fail(ErrorKind::Other)),
        (Step::ReadBack, Act::Fail(ErrorKind::InvalidData)),
        (Step::Rename, Act::Fail(ErrorKind::PermissionDenied)),
    ];
    for (nth, which) in [(1, "the new picture's side file"), (2, "the generation")] {
        for (step, act) in faults.iter().map(|(s, a)| (*s, clone_act(a))) {
            let what = format!("{step:?} of {which}");
            let t = Scratch::new("recovery");
            let root = t.path("recovery");
            let mut s = recovery_session(&root);
            s.run("text.insert", &json!({"text": "First synthetic draft."})).unwrap();
            s.doc.media.insert("media/one.png".into(), Arc::new(vec![1u8; 4096]));
            assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true), "{what}");
            let good = s.doc.clone();
            // A new picture: this copy writes its side file (write 1), then the generation (2).
            s.run("text.insert", &json!({"text": " Second thoughts."})).unwrap();
            s.doc.media.insert("media/two.png".into(), Arc::new(vec![2u8; 4096]));
            fault::inject(&t.0, step, nth, act);
            assert!(s.emergency_snapshot(Duration::from_secs(10)).is_err(), "{what}");
            assert!(fault::spent(&t.0), "{what}: the fault was met");
            assert!(matches!(s.recovery_gap(), Some(crate::recovery::RecoveryGap::Failed(_))), "{what}: {:?}", s.recovery_gap());
            assert!(files_named(&root, "pending-").is_empty(), "{what}: no temporary file is left");
            drop(s);
            assert_eq!(restored(&root), good, "{what}: the last good copy restores whole");

            // The fault gone, the next copy is written and restores.
            fault::clear(&t.0);
            let mut s = recovery_session(&root);
            settle(&mut s);
            let id = s.recovery_documents()[0].id.clone();
            s.run("file.recover", &json!({"restore": id})).unwrap();
            settle(&mut s);
            s.run("caret.docEnd", &json!({})).unwrap();
            s.run("text.insert", &json!({"text": " Third."})).unwrap();
            s.doc.media.insert("media/two.png".into(), Arc::new(vec![2u8; 4096]));
            assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true), "{what}");
            assert_eq!(s.recovery_gap(), None, "{what}");
            let latest = s.doc.clone();
            drop(s);
            assert_eq!(restored(&root), latest, "{what}");
        }
    }
}

fn clone_act(a: &Act) -> Act {
    match a {
        Act::Fail(k) => Act::Fail(*k),
        Act::Partial(k) => Act::Partial(*k),
        Act::Run(_) => Act::Fail(ErrorKind::Other),
    }
}

/// The folder can't be synchronized after the new generation is renamed into place: the copy
/// isn't counted as protecting the document (it may not survive a power loss), and the copy
/// before it, kept until a successor is known good, still restores. The next copy succeeds.
#[test]
fn a_recovery_folder_that_fails_to_synchronize_keeps_the_copy_before() {
    let t = Scratch::new("recovery-sync");
    let root = t.path("recovery");
    let mut s = recovery_session(&root);
    s.run("text.insert", &json!({"text": "First synthetic draft."})).unwrap();
    assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true));
    s.run("text.insert", &json!({"text": " Second."})).unwrap();
    // The record's folder, not the recovery root: the generation is renamed inside it.
    let record = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.path()).find(|p| p.is_dir()).unwrap();
    fault::inject(&record, Step::SyncDir, 1, Act::Fail(ErrorKind::Other));
    assert!(s.emergency_snapshot(Duration::from_secs(10)).is_err());
    assert!(matches!(s.recovery_gap(), Some(crate::recovery::RecoveryGap::Failed(_))));
    assert_eq!(files_named(&root, "snapshot-").len(), 2, "the copy before is kept");
    s.run("text.insert", &json!({"text": " Third."})).unwrap();
    assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true));
    let latest = s.doc.clone();
    drop(s);
    fault::clear(&record);
    assert_eq!(restored(&root), latest);
}

// The paragraph cap.

fn lines(n: usize) -> Document {
    Document::from_text(&(1..=n).map(|i| format!("Synthetic line {i}")).collect::<Vec<_>>().join("\n"))
}

/// A document has at most as many paragraphs as a file may have to be opened again
/// (`wordcraft_doc::MAX_PARAGRAPHS`, scaled down here): a paste or an Enter that would take it
/// past them is refused with the reason and leaves the document as it was, so what was written
/// is never in a file WordCraft refuses to open. Typing, deleting, Undo and Redo are unaffected;
/// paragraphs in tables, headers and notes count, as the readers count them.
#[test]
fn no_edit_takes_a_document_past_the_paragraph_cap() {
    let mut s = Session::new(lines(15));
    s.limits.paragraphs = 20;
    assert_eq!(crate::session::paragraph_total(&s.doc), 15);
    // Copy everything and paste it at the end: 30 paragraphs, refused.
    s.run("select.all", &json!({})).unwrap();
    s.run("edit.copy", &json!({})).unwrap();
    s.run("caret.docEnd", &json!({})).unwrap();
    let before = s.doc.clone();
    let e = s.run("edit.paste", &json!({})).unwrap_err();
    assert!(matches!(e, crate::CmdError::Disabled(_)) && e.to_string().contains("paragraphs, more than the 20 a document may have"), "{e}");
    assert_eq!(s.doc, before, "the refused paste left nothing behind");
    assert_ne!(s.undo_label(), Some("Paste"), "no undo step for it");
    // Enter up to the cap, not past it; typing goes on at the cap.
    for _ in 0..5 {
        s.run("text.newParagraph", &json!({})).unwrap();
    }
    assert_eq!(crate::session::paragraph_total(&s.doc), 20);
    assert!(s.run("text.newParagraph", &json!({})).is_err());
    s.run("text.insert", &json!({"text": "Still typing at the cap."})).unwrap();
    assert_eq!(crate::session::paragraph_total(&s.doc), 20);
    // Undo and Redo of an Enter, and Enter again after a paragraph is gone.
    s.run("edit.undo", &json!({})).unwrap();
    s.run("edit.redo", &json!({})).unwrap();
    s.run("caret.docStart", &json!({})).unwrap();
    s.run("caret.end", &json!({})).unwrap();
    s.run("text.delete", &json!({})).unwrap();
    assert_eq!(crate::session::paragraph_total(&s.doc), 19);
    s.run("text.newParagraph", &json!({})).unwrap();
    assert_eq!(crate::session::paragraph_total(&s.doc), 20);
    // A table's cells count: a 2 x 2 table at the cap is refused.
    assert!(s.run("insert.table", &json!({"rows": 2, "cols": 2})).is_err());
    assert_eq!(crate::session::paragraph_total(&s.doc), 20);
    // So do notes and headers.
    assert!(s.run("references.footnote", &json!({"text": "Synthetic note"})).is_err());
    assert!(s.run("insert.header", &json!({"text": "Synthetic header"})).is_err());
    assert_eq!(crate::session::paragraph_total(&s.doc), 20);
}

/// The count of what an edit adds looks only at what changed, but agrees with a full count:
/// random edits (typing, Enter, Backspace, paste, tables and their rows, notes, undo) on a
/// document with a table, and the cap holds at every step.
#[test]
fn the_paragraph_cap_follows_every_kind_of_edit() {
    let mut s = Session::new(lines(6));
    s.limits.paragraphs = 25;
    s.run("insert.table", &json!({"rows": 3, "cols": 3})).unwrap();
    let ops: [(&str, serde_json::Value); 13] = [
        ("text.insert", json!({"text": "word "})),
        ("text.newParagraph", json!({})),
        ("text.backspace", json!({})),
        ("table.insertRowBelow", json!({})),
        ("table.deleteRow", json!({})),
        ("edit.paste", json!({})),
        ("caret.docEnd", json!({})),
        ("caret.docStart", json!({})),
        ("caret.down", json!({})),
        ("references.footnote", json!({"text": "Synthetic note"})),
        ("edit.undo", json!({})),
        ("edit.redo", json!({})),
        ("select.paragraph", json!({})),
    ];
    let mut seed = 0x9E37_79B9u32;
    for step in 0..400 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let (id, params) = &ops[seed as usize % ops.len()];
        if *id == "edit.paste" {
            let _ = s.run("edit.copy", &json!({}));
        }
        let before = crate::session::paragraph_total(&s.doc);
        let doc = s.doc.clone();
        let r = s.run(id, params);
        let after = crate::session::paragraph_total(&s.doc);
        if !matches!(*id, "edit.undo" | "edit.redo") {
            assert!(after as u64 <= s.limits.paragraphs.max(before as u64), "step {step}: {id} took the document to {after} paragraphs");
        }
        // The count kept from what each command changed is the document's, counted whole.
        assert_eq!(s.tally, Some(wordcraft_doc::limits::Tally::of(&s.doc)), "step {step}: {id} ({r:?})");
        let _ = &doc;
    }
}

// Word 2003 XML Documents.

/// A Word 2003 XML Document can't hold equations, content controls, text boxes or drawings
/// (the 2003 writer leaves them out), though its `.xml` name is a Word XML Document's: a save
/// over one reports them (`lossy`), as any save in a format that can't keep everything does,
/// and what it reports is what the file is without. A Word XML Document (Flat OPC) keeps them.
#[test]
fn a_save_over_a_word_2003_xml_file_reports_what_it_leaves_out() {
    let d = Scratch::new("wordml2003");
    let path = d.path("report.xml");
    let opts = crate::io::ExportOptions { word2003: true, ..Default::default() };
    std::fs::write(&path, crate::io::save_bytes_opts("report.xml", &Document::from_text("Synthetic report."), &opts).unwrap()).unwrap();
    let mut s = Session::new(Document::new());
    s.run("file.open", &json!({"path": path})).unwrap();
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("insert.equation", &json!({"linear": "E=mc^2"})).unwrap();
    s.run("developer.plainText", &json!({"tag": "synthetic"})).unwrap();
    s.run("insert.textBox", &json!({"text": "Synthetic box"})).unwrap();
    let expected = ["equations", "contentControls", "textBoxes"];
    let asked = crate::cmd::file::lossy_on_save(&s, &json!({})).unwrap_or_default();
    let r = s.run("file.save", &json!({})).unwrap();
    let lost: Vec<&str> = r["lossy"].as_array().map(|l| l.iter().filter_map(|f| f.as_str()).collect()).unwrap_or_default();
    for f in expected {
        assert!(lost.contains(&f), "{f} isn't reported: {r}");
        assert!(asked.contains(&f), "{f} isn't asked about: {asked:?}");
    }
    let head = std::fs::read(&path).unwrap();
    assert_eq!(wordcraft_docx::sniff_xml(&head), wordcraft_docx::XmlKind::WordMl2003, "the file keeps its format");
    let back = crate::io::open_path(&path).unwrap();
    let objects: Vec<_> = back.body.iter().filter_map(|b| b.as_para()).flat_map(|p| p.objects.iter()).collect();
    assert!(!objects.iter().any(|o| matches!(o, wordcraft_doc::InlineObject::Equation { .. })), "the report is true: no equation");
    // Saved as a Word XML Document instead, everything is kept and nothing is reported.
    let flat = d.path("flat.xml");
    let r = s.run("file.saveAs", &json!({"path": flat, "format": "flatopc"})).unwrap();
    assert!(r.get("lossy").is_none(), "{r}");
    let back = crate::io::open_path(&flat).unwrap();
    let objects: Vec<_> = back.body.iter().filter_map(|b| b.as_para()).flat_map(|p| p.objects.iter()).collect();
    assert!(objects.iter().any(|o| matches!(o, wordcraft_doc::InlineObject::Equation { .. })));
}

// Damaged recovery copies.

/// A recovery copy damaged on disk (a digit changed, a number made huge, a byte flipped, the file
/// cut short) is refused: its checksum doesn't match, so a damage that keeps the JSON valid never
/// restores a silently different document. A copy written before checksums (the JSON alone) is
/// refused or restores a document that lays out, edits and saves. Either way offering it never
/// crashes the app, and the worker that reads it keeps working. Seeded changes to a real
/// generation of a document with a table, a note, a comment and text in several scripts.
#[test]
fn a_damaged_recovery_copy_is_refused_or_restored_never_a_panic() {
    let t = Scratch::new("recovery-damage");
    let root = t.path("recovery");
    let mut s = recovery_session(&root);
    s.run("text.insert", &json!({"text": "Synthetic draft with café, 日本語 and Ελληνικά."})).unwrap();
    s.run("references.footnote", &json!({"text": "Synthetic note"})).unwrap();
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("review.newComment", &json!({"text": "Synthetic comment"})).unwrap();
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("insert.table", &json!({"rows": 2, "cols": 2})).unwrap();
    s.run("text.insert", &json!({"text": "In a cell"})).unwrap();
    assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true));
    drop(s);
    let generation = files_named(&root, "snapshot-").pop().unwrap();
    let checked = std::fs::read(&generation).unwrap();
    assert!(checked.starts_with(b"WordCraft recovery copy\n"), "a copy has its checksum line");
    // The same copy as written before checksums: its JSON alone, after the two header lines.
    let json_at = checked.iter().enumerate().filter(|(_, b)| **b == b'\n').nth(1).unwrap().0 + 1;
    let unchecked = checked[json_at..].to_vec();
    for (format, good) in [("checksummed", checked), ("written before checksums", unchecked)] {
        damage_rounds(&root, &generation, &good, format);
    }
}

/// [`a_damaged_recovery_copy_is_refused_or_restored_never_a_panic`] for one format of the copy
/// at `generation`, whose undamaged bytes are `good`.
fn damage_rounds(root: &Path, generation: &Path, good: &[u8], format: &str) {
    let checksummed = good.starts_with(b"WordCraft recovery copy\n");
    let digits: Vec<usize> = good.iter().enumerate().filter(|(_, b)| b.is_ascii_digit()).map(|(i, _)| i).collect();
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let (mut refused, mut restored) = (0, 0);
    for round in 0..120 {
        let mut bytes = good.to_vec();
        match round % 4 {
            // A digit changed: a run's length, an offset, a size, an id.
            0 | 1 => {
                for _ in 0..1 + next() % 3 {
                    let at = digits[next() as usize % digits.len()];
                    bytes[at] = b'0' + (next() % 10) as u8;
                }
            }
            // A number made huge.
            2 => {
                let at = digits[next() as usize % digits.len()];
                bytes.splice(at..at, b"999999999999".iter().copied());
            }
            // A byte flipped, or the file cut short.
            _ => {
                if next() % 2 == 0 {
                    let at = next() as usize % bytes.len();
                    bytes[at] ^= 1 << (next() % 8);
                } else {
                    bytes.truncate(next() as usize % bytes.len());
                }
            }
        }
        std::fs::write(generation, &bytes).unwrap();
        let damaged = bytes != good;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut s = recovery_session(root);
            settle(&mut s);
            let Some(id) = s.recovery_documents().first().map(|d| d.id.clone()) else { return false };
            if s.run("file.recover", &json!({"restore": id})).is_err() {
                return false;
            }
            settle(&mut s);
            assert!(!s.recovery_error().is_some_and(|e| e.contains("unavailable")), "the worker stopped: {:?}", s.recovery_error());
            if !s.dirty {
                return false;
            }
            let _ = s.layout();
            for (id, params) in [
                ("caret.docEnd", json!({})),
                ("text.insert", json!({"text": "typed after restoring"})),
                ("caret.docStart", json!({})),
                ("text.newParagraph", json!({})),
                ("select.all", json!({})),
                ("format.bold", json!({})),
                ("edit.undo", json!({})),
            ] {
                if let Err(e) = s.run(id, &params) {
                    assert!(!e.to_string().contains("panicked"), "{id}: {e}");
                }
            }
            let _ = s.layout();
            let _ = crate::io::save_bytes("restored.docx", &s.doc);
            true
        }));
        match outcome {
            Ok(true) => {
                assert!(!(checksummed && damaged), "{format}, round {round}: a damaged checksummed copy was restored");
                restored += 1;
            }
            Ok(false) => refused += 1,
            Err(_) => panic!("{format}, round {round}: a damaged recovery copy crashed the app"),
        }
    }
    if checksummed {
        // Only a "damage" that changed nothing (a digit set to itself) restores.
        assert!(refused > 100, "{format}: refused {refused}, restored {restored}");
    } else {
        assert!(refused > 0 && restored > 0, "{format}: refused {refused}, restored {restored}");
    }
}

/// A recovery copy's newest generation damaged on disk in a way that keeps its JSON valid (one
/// bit of its text flipped: "Second" read "Secnnd") restored that text without a word; plain
/// generations had no checksum of their own (06). Now the checksum refuses it: File › Recover
/// lists the copy before it, says so, and restores that, saying so too. A generation damaged
/// after it was listed (before Restore) falls back the same way.
#[test]
fn a_recovery_copy_damaged_on_disk_restores_the_one_before_it_and_says_so() {
    for when in ["before listing", "after listing"] {
        let t = Scratch::new("recovery-checksum");
        let root = t.path("recovery");
        let mut s = recovery_session(&root);
        s.run("text.insert", &json!({"text": "First synthetic draft."})).unwrap();
        assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true));
        let first = s.doc.clone();
        s.run("text.insert", &json!({"text": " Second thoughts."})).unwrap();
        assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true));
        drop(s);
        let mut generations = files_named(&root, "snapshot-");
        generations.sort();
        assert_eq!(generations.len(), 2, "{when}: the copy before is kept until the next one");
        let newest = generations.pop().unwrap();
        let mut bytes = std::fs::read(&newest).unwrap();
        let at = bytes.windows(15).position(|w| w == b"Second thoughts").unwrap() + 3;
        bytes[at] ^= 1;
        let damage = || std::fs::write(&newest, &bytes).unwrap();
        if when == "before listing" {
            damage();
        }
        let mut s = recovery_session(&root);
        settle(&mut s);
        let docs = s.recovery_documents();
        assert_eq!(docs.len(), 1, "{when}");
        if when == "before listing" {
            assert!(docs[0].earlier, "{when}: listed as the copy before the damaged one");
            assert_eq!(s.run("file.recover", &json!({})).unwrap()["documents"][0]["earlier"], true, "{when}");
        } else {
            assert!(!docs[0].earlier, "{when}");
            damage();
        }
        s.run("file.recover", &json!({"restore": docs[0].id})).unwrap();
        settle(&mut s);
        assert_eq!(s.doc, first, "{when}: the copy before restores, not the damaged text");
        assert!(!s.doc.plain_text(StoryRef::Body).contains("Secnnd"), "{when}");
        assert_eq!(s.recovery_error(), Some(crate::recovery::RESTORED_EARLIER), "{when}: the restore says so");
        assert_eq!(s.status, crate::recovery::RESTORED_EARLIER, "{when}");
    }
}

/// A recovery copy written before generations had checksums (the snapshot's JSON alone, as this
/// fixture is) is listed and restores as before; the next copy of it has a checksum.
#[test]
fn a_recovery_copy_written_before_checksums_still_restores() {
    let t = Scratch::new("recovery-old-format");
    let root = t.path("recovery");
    let id = "0123456789abcdef0123456789abcdef";
    std::fs::create_dir_all(root.join(id)).unwrap();
    let old = r#"{"version":2,"info":{"id":"0123456789abcdef0123456789abcdef","name":"Old synthetic draft","savedAt":"2026-10-01T09:00:00Z"},"generation":1,"document":{"body":[{"kind":"para","text":"Written before checksums.","runs":[{"len":25}]}]}}"#;
    std::fs::write(root.join(id).join("snapshot-00000000000000000001.json"), old).unwrap();
    let mut s = recovery_session(&root);
    settle(&mut s);
    let docs = s.recovery_documents();
    assert_eq!(docs.len(), 1, "{:?}", s.recovery_error());
    assert_eq!(docs[0].name, "Old synthetic draft");
    assert!(!docs[0].earlier && !docs[0].older_than_file);
    s.run("file.recover", &json!({"restore": id})).unwrap();
    settle(&mut s);
    assert_eq!(s.doc.plain_text(StoryRef::Body), "Written before checksums.");
    assert_eq!(s.recovery_error(), None);
    // Its next copy is checksummed.
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": " And after."})).unwrap();
    assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true));
    drop(s);
    let mut generations = files_named(&root, "snapshot-");
    generations.sort();
    assert!(std::fs::read(generations.last().unwrap()).unwrap().starts_with(b"WordCraft recovery copy\n"));
    assert_eq!(restored(&root).plain_text(StoryRef::Body), "Written before checksums. And after.");
}

/// File › Recover says when a copy is older than its saved file (06: it didn't): the file it was
/// made from was changed after the copy was made (saved again, by this or another program). A
/// copy of edits the file doesn't have yet isn't marked.
#[test]
fn file_recover_says_when_a_copy_is_older_than_its_saved_file() {
    let d = Scratch::new("older-than-file");
    let path = d.path("letter.docx");
    let root = d.path("recovery");
    std::fs::write(&path, crate::io::save_bytes("letter.docx", &Document::from_text(ORIGINAL)).unwrap()).unwrap();
    let mut s = recovery_session(&root);
    s.run("file.open", &json!({"path": path})).unwrap();
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": EDIT})).unwrap();
    assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true));
    drop(s);
    let listed = |root: &Path| {
        let mut s = recovery_session(root);
        settle(&mut s);
        let v = s.run("file.recover", &json!({})).unwrap();
        (s.recovery_documents(), v)
    };
    let (docs, _) = listed(&root);
    assert_eq!(docs.len(), 1);
    assert!(!docs[0].older_than_file, "the copy has edits the file doesn't");
    // The file is saved again after the copy was made.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&path, crate::io::save_bytes("letter.docx", &Document::from_text("Saved later.")).unwrap()).unwrap();
    let (docs, v) = listed(&root);
    assert!(docs[0].older_than_file, "{docs:?}");
    assert_eq!(v["documents"][0]["olderThanFile"], true, "{v}");
}

/// At the real cap (`wordcraft_doc::MAX_PARAGRAPHS`): a document of exactly that many paragraphs
/// refuses one more, and what it saves opens again; one paragraph more (what a paste or an
/// Enter made before edits were held to the cap) saves into a file every open refuses. Heavy
/// (250,000 paragraphs since the shared limits, about 1 GB at its peak), so not run by default:
/// `cargo test -p wordcraft-engine --lib at_the_real_paragraph_cap -- --ignored`.
#[test]
#[ignore]
fn at_the_real_paragraph_cap_what_saves_opens_again() {
    let n = wordcraft_doc::MAX_PARAGRAPHS;
    let mut s = Session::new(Document::from_text(&"x\n".repeat(n - 1)));
    assert_eq!(crate::session::paragraph_total(&s.doc), n);
    s.run("caret.docEnd", &json!({})).unwrap();
    assert!(s.run("text.newParagraph", &json!({})).is_err(), "one more is refused");
    s.run("text.insert", &json!({"text": "typed at the cap"})).unwrap();
    let bytes = crate::io::save_bytes("cap.docx", &s.doc).unwrap();
    let back = crate::io::open_bytes("cap.docx", &bytes).unwrap();
    assert_eq!(crate::session::paragraph_total(&back), n);
    drop(back);
    // What an edit could make before: one paragraph more saves, and doesn't open.
    let mut over = std::mem::replace(&mut s.doc, Document::new());
    drop(s);
    over.body.push(std::sync::Arc::new(wordcraft_doc::Block::Para(wordcraft_doc::Paragraph::new())));
    let bytes = crate::io::save_bytes("over.docx", &over).unwrap();
    drop(over);
    assert!(crate::io::open_bytes("over.docx", &bytes).is_err(), "a file of one paragraph more is refused");
}

/// Two windows on the same file save at once: the first one's write (on a worker, as AutoSave
/// writes) is held at the moment before it replaces the file while the second window saves. The
/// first then finds the file changed under it and is refused: the second window's save stays,
/// and the first keeps its edits, unsaved, to save elsewhere.
#[test]
fn two_windows_saving_the_same_file_at_once_never_overwrite_each_other() {
    let d = Scratch::new("two-windows");
    let path = d.path("letter.docx");
    let (mut first, _) = edited_session(&path);
    let mut second = Session::new(Document::new());
    second.run("file.open", &json!({"path": path})).unwrap();
    second.run("caret.docEnd", &json!({})).unwrap();
    second.run("text.insert", &json!({"text": " The second window's words."})).unwrap();

    let (reached, wait_reached) = std::sync::mpsc::channel::<()>();
    let (release, wait_release) = std::sync::mpsc::channel::<()>();
    fault::inject(
        &d.0,
        Step::Verify,
        1,
        Act::Run(Box::new(move || {
            let _ = reached.send(());
            let _ = wait_release.recv_timeout(Duration::from_secs(30));
        })),
    );
    let (doc, expected, target) = (first.doc.clone(), first.file_version().cloned(), path.clone());
    let worker =
        std::thread::spawn(move || crate::io::save_path_versioned(&target, &doc, None, &crate::io::ExportOptions::default(), expected.as_ref()));
    wait_reached.recv_timeout(Duration::from_secs(30)).unwrap();
    second.run("file.save", &json!({})).unwrap();
    release.send(()).unwrap();
    let r = worker.join().unwrap();
    assert!(matches!(r, Err(crate::CmdError::FileConflict(_))), "{r:?}");
    assert_eq!(body_text(&path), format!("{ORIGINAL} The second window's words."));
    assert_eq!(d.names(), ["letter.docx"]);
    // The first window's own Save is refused too (the file isn't the version it opened); its
    // edits go to a file of their own.
    assert!(matches!(first.run("file.save", &json!({})), Err(crate::CmdError::FileConflict(_))));
    assert!(first.dirty);
    first.run("file.saveAs", &json!({"path": d.path("first.docx")})).unwrap();
    assert_eq!(body_text(&d.path("first.docx")), format!("{ORIGINAL}{EDIT}"));
}

// Pasted comments (the integration's paste fix: a pasted comment gets anchors of its own).

fn comment_starts(doc: &Document) -> usize {
    doc.body
        .iter()
        .filter_map(|b| b.as_para())
        .flat_map(|p| p.objects.iter())
        .filter(|o| matches!(o, wordcraft_doc::InlineObject::CommentStart { .. }))
        .count()
}

/// A paragraph with a comment, copied and pasted: the copy's comment is a comment of its own,
/// and every save keeps both, anchored, with their text: `.docx`, ODT and RTF (which keep
/// comments), WordCraft's JSON, and a recovery copy restored at the next start.
#[test]
fn pasted_comments_survive_every_save() {
    let mut s = Session::new(Document::from_text("Commented synthetic words.\nPlain line."));
    s.run("select.text", &json!({"text": "Commented"})).unwrap();
    s.run("review.newComment", &json!({"text": "Synthetic remark"})).unwrap();
    s.run("caret.docStart", &json!({})).unwrap();
    s.run("select.paragraph", &json!({})).unwrap();
    s.run("edit.copy", &json!({})).unwrap();
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("text.newParagraph", &json!({})).unwrap();
    s.run("edit.paste", &json!({})).unwrap();
    assert_eq!(s.doc.comments.len(), 2, "the pasted comment is a comment of its own");
    assert_eq!(comment_starts(&s.doc), 2);
    let remarks = |d: &Document| {
        d.parts
            .values()
            .filter(|p| p.kind == wordcraft_doc::PartKind::Comment)
            .filter(|p| p.blocks.iter().filter_map(|b| b.as_para()).any(|q| q.plain_text().contains("Synthetic remark")))
            .count()
    };
    assert_eq!(remarks(&s.doc), 2);
    for ext in ["docx", "odt", "rtf", "json"] {
        let name = format!("pasted.{ext}");
        let back = crate::io::open_bytes(&name, &crate::io::save_bytes(&name, &s.doc).unwrap()).unwrap();
        assert_eq!(back.comments.len(), 2, "{ext}");
        assert_eq!(comment_starts(&back), 2, "{ext}");
        assert_eq!(remarks(&back), 2, "{ext}");
    }
    let t = Scratch::new("pasted-comments");
    let root = t.path("recovery");
    let mut r = recovery_session(&root);
    r.set_document(s.doc.clone());
    r.run("caret.docEnd", &json!({})).unwrap();
    r.run("text.insert", &json!({"text": " Unsaved."})).unwrap();
    assert_eq!(r.emergency_snapshot(Duration::from_secs(10)), Ok(true));
    let expected = r.doc.clone();
    drop(r);
    let back = restored(&root);
    assert_eq!(back, expected);
    assert_eq!(back.comments.len(), 2);
}

/// A document well under the paragraph cap whose main part holds more XML nodes than the reader
/// takes (`xml::MAX_ELEMENTS`, 8 million elements and attributes) saves as `.docx` and is then refused when opened. 120,000
/// paragraphs of 10 runs in alternating bold, italic and size (about 70 nodes a paragraph as the
/// writer writes them) are enough; since the shared limits they are past the runs a document may
/// have, and 70,000 (700,000 runs) are tried. Heavy (about 1 GB), so not run by default:
/// `cargo test -p wordcraft-engine --lib a_heavily_formatted_document -- --ignored`.
#[test]
#[ignore]
fn a_heavily_formatted_document_under_the_paragraph_cap_opens_again() {
    use wordcraft_doc::props::CharProps;
    let runs = 10;
    let paragraphs = 70_000;
    let word = "word ";
    let mut doc = Document::new();
    doc.body.clear();
    for _ in 0..paragraphs {
        let mut p = wordcraft_doc::Paragraph::new();
        p.text = word.repeat(runs);
        p.runs = (0..runs)
            .map(|i| wordcraft_doc::Run {
                len: word.len(),
                props: CharProps {
                    bold: Some(i % 2 == 0),
                    italic: Some(i % 3 == 0),
                    size: Some(if i % 2 == 0 { 12.0 } else { 11.0 }),
                    ..Default::default()
                },
            })
            .collect();
        doc.body.push(Arc::new(wordcraft_doc::Block::Para(p)));
    }
    assert_eq!(wordcraft_doc::limits::Tally::of(&doc).over(&wordcraft_doc::limits::Limits::DOCUMENT), None);
    let bytes = crate::io::save_bytes("formatted.docx", &doc).unwrap();
    drop(doc);
    let back = crate::io::open_bytes("formatted.docx", &bytes);
    assert!(back.is_ok(), "a document WordCraft wrote doesn't open: {:?}", back.err());
}

// A copy older than the saved file.

/// The app ended between a save and the removal of the document's recovery copy (a crash or a
/// kill in that moment): the next start offers the copy, which is older than the file. Restoring
/// it never touches the file, which keeps the newer saved version; the restored copy is an
/// untitled document (no path to save or AutoSave over the file), and the file still opens with
/// everything that was saved.
#[test]
fn a_copy_older_than_the_saved_file_never_overwrites_it() {
    let d = Scratch::new("stale");
    let path = d.path("letter.docx");
    let root = d.path("recovery");
    std::fs::write(&path, crate::io::save_bytes("letter.docx", &Document::from_text(ORIGINAL)).unwrap()).unwrap();
    let mut s = recovery_session(&root);
    s.run("file.open", &json!({"path": path})).unwrap();
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": EDIT})).unwrap();
    assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true));
    s.run("text.insert", &json!({"text": " Newer saved words."})).unwrap();
    s.run("file.save", &json!({})).unwrap();
    let saved = std::fs::read(&path).unwrap();
    // No recovery tick ran: the copy is still there, as after a crash at this moment.
    drop(s);
    let mut s = recovery_session(&root);
    settle(&mut s);
    let docs = s.recovery_documents();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].path.as_deref(), Some(path.to_string_lossy().as_ref()));
    s.run("file.recover", &json!({"restore": docs[0].id})).unwrap();
    settle(&mut s);
    assert!(s.doc.plain_text(StoryRef::Body).ends_with(EDIT.trim_start()));
    assert!(s.path.is_none(), "the restored copy is untitled");
    assert_eq!(s.run("file.save", &json!({})).unwrap()["saved"], false, "Save asks where, it doesn't write the file");
    assert_eq!(std::fs::read(&path).unwrap(), saved, "the newer file is untouched");
    assert!(body_text(&path).ends_with("Newer saved words."));
}

// Upstream reports reproduced here.

fn zip_parts(entries: &[(&str, &str)]) -> Vec<u8> {
    use std::io::Write;
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, body) in entries {
        zw.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(body.as_bytes()).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

fn part_text(bytes: &[u8], name: &str) -> String {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut f = z.by_name(name).unwrap();
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    s
}

/// A `.docx` package whose body is `body` (WordprocessingML), with no other parts.
fn docx_with_body(body: &str) -> Vec<u8> {
    const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W}" xmlns:r="{R}"><w:body>{body}<w:sectPr/></w:body></w:document>"#
    );
    let ct = r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    let rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/officeDocument" Target="word/document.xml"/></Relationships>"#
    );
    zip_parts(&[("[Content_Types].xml", ct), ("_rels/.rels", &rels), ("word/document.xml", &document)])
}

/// A TOC building-block control (`w:sdt` with `w:docPartObj`, gallery "Table of Contents").
const TOC_SDT: &str = r#"<w:sdt><w:sdtPr><w:id w:val="-1234567"/><w:docPartObj><w:docPartGallery w:val="Table of Contents"/><w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtContent>"#;
const TOC_BEGIN: &str = r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \o "1-3" \h \z \u </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>"#;
const FLD_END: &str = r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#;

/// storytold/wordcraft#2427: a table of contents in Word's building-block control
/// (`w:docPartObj`, gallery "Table of Contents"), with entries in no TOC style, keeps its
/// control through a save, and again through a second one. It was dropped: the reader dropped
/// every TOC gallery control (`docx/src/read/story.rs` `wrap_in_control`) because the writer
/// puts one back, but only around the TOC it makes itself (`write/story.rs` `toc_span`: an empty
/// TOC field and TOC-styled entries), so a TOC read from a file lost its control.
#[test]
fn a_table_of_contents_keeps_its_building_block_control_through_a_save() {
    let body = format!(
        r#"{TOC_SDT}<w:p><w:r><w:t>Contents</w:t></w:r></w:p><w:p>{TOC_BEGIN}<w:r><w:t>Synthetic introduction 1</w:t></w:r></w:p><w:p><w:r><w:t>Synthetic part two 2</w:t></w:r>{FLD_END}</w:p></w:sdtContent></w:sdt><w:p><w:r><w:t>Body text.</w:t></w:r></w:p>"#
    );
    let doc = crate::io::open_bytes("toc.docx", &docx_with_body(&body)).unwrap();
    let out = crate::io::save_bytes("toc.docx", &doc).unwrap();
    let xml = part_text(&out, "word/document.xml");
    assert!(xml.contains("docPartGallery") && xml.contains("Table of Contents"), "{xml}");
    assert!(xml.contains("<w:sdt>") || xml.contains("<w:sdt "), "{xml}");
    assert!(xml.contains("Synthetic introduction 1") && xml.contains("Synthetic part two 2"), "{xml}");
    assert!(xml.contains("-1234567"), "the control's id is kept: {xml}");
    let again = crate::io::save_bytes("toc.docx", &crate::io::open_bytes("toc.docx", &out).unwrap()).unwrap();
    assert_eq!(part_text(&again, "word/document.xml"), xml, "a second save keeps it the same");
}

/// The content control markers of `doc`'s body: (starts, ends), and how many of the starts are
/// TOC building-block controls.
fn control_markers(doc: &Document) -> (usize, usize, usize) {
    let (mut starts, mut ends, mut tocs) = (0, 0, 0);
    for b in doc.body.iter() {
        let Some(p) = b.as_para() else { continue };
        for o in &p.objects {
            match o {
                wordcraft_doc::InlineObject::ControlStart { control } => {
                    starts += 1;
                    tocs += usize::from(
                        matches!(&control.kind, wordcraft_doc::control::ControlKind::Gallery { gallery, .. } if gallery == "Table of Contents"),
                    );
                }
                wordcraft_doc::InlineObject::ControlEnd => ends += 1,
                _ => {}
            }
        }
    }
    (starts, ends, tocs)
}

/// The text of the `w:sdtContent` of the TOC building-block control in `xml` (the first and only
/// one: asserted).
fn toc_control_content(xml: &str) -> String {
    assert_eq!(xml.matches("w:docPartGallery w:val=\"Table of Contents\"").count(), 1, "one TOC control: {xml}");
    let at = xml.find("Table of Contents").unwrap();
    let from = at + xml[at..].find("<w:sdtContent>").unwrap();
    let to = from + xml[from..].find("</w:sdtContent>").unwrap();
    xml[from..to].to_string()
}

/// #2427, Update Table: a TOC inside its kept building-block control is rebuilt with the
/// control's two markers balanced and around the heading and the new entries, the field still
/// spans the entries, the file has one TOC control (never a second one around it), and a second
/// update, a reopen and another save change nothing. In each layout Word and other writers use:
/// the heading inside the control and the field's end in a paragraph of its own (Word's), the
/// heading outside it, entries in no TOC style (#2427's), and WordCraft's own TOC saved and
/// opened again. Adding a heading and updating keeps it balanced; Remove Table of Contents
/// removes the control with the TOC.
#[test]
fn update_table_keeps_the_kept_building_block_control_balanced() {
    let headings = r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Synthetic introduction</w:t></w:r></w:p><w:p><w:r><w:t>Body text.</w:t></w:r></w:p><w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Synthetic part two</w:t></w:r></w:p><w:p><w:r><w:t>More body text.</w:t></w:r></w:p>"#;
    let entry = |style: &str, text: &str| format!(r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#);
    let heading = r#"<w:p><w:pPr><w:pStyle w:val="TOCHeading"/></w:pPr><w:r><w:t>Contents</w:t></w:r></w:p>"#;
    let word = format!(
        r#"{TOC_SDT}{heading}<w:p><w:pPr><w:pStyle w:val="TOC1"/></w:pPr>{TOC_BEGIN}<w:r><w:t>Old entry one	1</w:t></w:r></w:p>{}<w:p>{FLD_END}</w:p></w:sdtContent></w:sdt>{headings}"#,
        entry("TOC2", "Old entry two\t1")
    );
    let outside = format!(
        r#"{heading}{TOC_SDT}<w:p><w:pPr><w:pStyle w:val="TOC1"/></w:pPr>{TOC_BEGIN}<w:r><w:t>Old entry one	1</w:t></w:r></w:p><w:p><w:pPr><w:pStyle w:val="TOC2"/></w:pPr><w:r><w:t>Old entry two	1</w:t></w:r>{FLD_END}</w:p></w:sdtContent></w:sdt>{headings}"#
    );
    let unstyled = format!(
        r#"{TOC_SDT}<w:p><w:r><w:t>Contents</w:t></w:r></w:p><w:p>{TOC_BEGIN}<w:r><w:t>Old entry one 1</w:t></w:r></w:p><w:p><w:r><w:t>Old entry two 2</w:t></w:r>{FLD_END}</w:p></w:sdtContent></w:sdt>{headings}"#
    );
    // WordCraft's own TOC, saved and opened again (its control written by the writer).
    let own = {
        let mut s = Session::new(crate::io::open_bytes("h.docx", &docx_with_body(headings)).unwrap());
        s.run("caret.docStart", &json!({})).unwrap();
        s.run("references.toc", &json!({})).unwrap();
        crate::io::save_bytes("own.docx", &s.doc).unwrap()
    };
    let cases = [
        ("Word's layout", docx_with_body(&word)),
        ("heading outside", docx_with_body(&outside)),
        ("unstyled entries", docx_with_body(&unstyled)),
        ("WordCraft's own", own),
    ];
    for (case, bytes) in cases {
        // WordCraft's own control is left to the writer, which puts it back (the model holds the
        // TOC as the engine makes it); Word's (with an id) and the others are kept.
        let kept = if case == "WordCraft's own" { (0, 0, 0) } else { (1, 1, 1) };
        // Remove Table of Contents straight after opening (a TOC read as a range field) takes
        // the TOC and its control with it, and nothing else.
        let mut fresh = Session::new(crate::io::open_bytes("toc.docx", &bytes).unwrap());
        fresh.run("references.removeToc", &json!({})).unwrap();
        assert_eq!(control_markers(&fresh.doc), (0, 0, 0), "{case}: removed with the TOC");
        let xml = part_text(&crate::io::save_bytes("toc.docx", &fresh.doc).unwrap(), "word/document.xml");
        assert!(!xml.contains("TOC \\o") && !xml.contains("Table of Contents") && !xml.contains("Old entry"), "{case}: {xml}");
        assert!(xml.contains("More body text"), "{case}: the body stays: {xml}");
        // Inserting a table of contents with other settings replaces it: balanced, one control.
        let mut replaced = Session::new(crate::io::open_bytes("toc.docx", &bytes).unwrap());
        replaced.run("references.toc", &json!({"levels": 1})).unwrap();
        let (starts, ends, _) = control_markers(&replaced.doc);
        assert_eq!(starts, ends, "{case}: balanced after replacing the TOC");
        let xml = part_text(&crate::io::save_bytes("toc.docx", &replaced.doc).unwrap(), "word/document.xml");
        let inside = toc_control_content(&xml);
        assert!(inside.contains("Synthetic introduction") && !inside.contains("Synthetic part two"), "{case}: one level: {inside}");
        let mut s = Session::new(crate::io::open_bytes("toc.docx", &bytes).unwrap());
        assert_eq!(control_markers(&s.doc), kept, "{case}: the control is read");
        s.run("references.updateToc", &json!({})).unwrap();
        assert_eq!(control_markers(&s.doc), kept, "{case}: balanced after Update Table");
        let text = s.doc.plain_text(StoryRef::Body);
        assert!(text.contains("Synthetic introduction") && text.contains("Synthetic part two"), "{case}: {text}");
        assert!(!text.contains("Old entry"), "{case}: the old entries are replaced: {text}");
        let xml = part_text(&crate::io::save_bytes("toc.docx", &s.doc).unwrap(), "word/document.xml");
        let inside = toc_control_content(&xml);
        assert!(
            inside.contains("Synthetic introduction") && inside.contains("Synthetic part two"),
            "{case}: the entries are in the control: {inside}"
        );
        assert!(inside.contains("TOC \\o"), "{case}: so is the field: {inside}");
        assert!(!inside.contains("More body text"), "{case}: the body isn't: {inside}");
        assert_eq!(xml.matches("w:fldCharType=\"begin\"").count(), xml.matches("w:fldCharType=\"end\"").count(), "{case}: {xml}");
        // Again: nothing changes.
        let body = s.doc.body.clone();
        s.run("references.updateToc", &json!({})).unwrap();
        assert_eq!(s.doc.body, body, "{case}: a second Update Table changes nothing");
        let reopened = crate::io::open_bytes("toc.docx", &crate::io::save_bytes("toc.docx", &s.doc).unwrap()).unwrap();
        assert_eq!(control_markers(&reopened), kept, "{case}: reopened");
        let mut r = Session::new(reopened);
        r.run("references.updateToc", &json!({})).unwrap();
        let xml2 = part_text(&crate::io::save_bytes("toc.docx", &r.doc).unwrap(), "word/document.xml");
        let at = xml.bytes().zip(xml2.bytes()).position(|(a, b)| a != b).unwrap_or(xml.len().min(xml2.len()));
        let show = |x: &str| x.get(at.saturating_sub(400)..(at + 600).min(x.len())).unwrap_or("").to_string();
        assert!(xml2 == xml, "{case}: reopened, updated and saved again: the same\nfirst:  {}\nsecond: {}", show(&xml), show(&xml2));
        // A new heading: still balanced, and in the control.
        s.run("caret.docEnd", &json!({})).unwrap();
        s.run("text.newParagraph", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "Synthetic appendix"})).unwrap();
        s.run("para.style", &json!({"style": "Heading1"})).unwrap();
        s.run("references.updateToc", &json!({})).unwrap();
        assert_eq!(control_markers(&s.doc), kept, "{case}: balanced with a new entry");
        let xml = part_text(&crate::io::save_bytes("toc.docx", &s.doc).unwrap(), "word/document.xml");
        assert!(toc_control_content(&xml).contains("Synthetic appendix"), "{case}: {xml}");
        // Remove Table of Contents takes the control with it.
        s.run("references.removeToc", &json!({})).unwrap();
        assert_eq!(control_markers(&s.doc), (0, 0, 0), "{case}: the control goes with the TOC");
        let xml = part_text(&crate::io::save_bytes("toc.docx", &s.doc).unwrap(), "word/document.xml");
        assert!(!xml.contains("Table of Contents") && !xml.contains("Synthetic appendix\t"), "{case}: {xml}");
    }
}

/// A stand-in for a compiled macro project (an OLE compound file in real life).
const VBA: &[u8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, b's', b'y', b'n', b't', b'h'];

/// The bytes of a `.docm` (or `.dotm`, by `name`) holding [`ORIGINAL`] and a macro project with
/// its VBA data, as the reader keeps them (`wordcraft_docx` `VBA_PROJECT_PART`, `VBA_RELATED`).
fn docm_bytes(name: &str) -> Vec<u8> {
    let mut doc = Document::from_text(ORIGINAL);
    doc.passthrough.insert("word/vbaProject.bin".into(), Arc::new(VBA.to_vec()));
    let data = b"<wne:vbaSuppData xmlns:wne=\"http://schemas.microsoft.com/office/word/2006/wordml\"/>";
    doc.passthrough.insert("word/vbaData.xml".into(), Arc::new(data.to_vec()));
    let related = "http://schemas.microsoft.com/office/2006/relationships/wordVbaData\tword/vbaData.xml\tapplication/vnd.ms-word.vbaData+xml\n";
    doc.passthrough.insert("wordcraft:vbaProject.related".into(), Arc::new(related.as_bytes().to_vec()));
    let bytes = crate::io::save_bytes(name, &doc).unwrap();
    assert!(wordcraft_docx::has_macro_project(&crate::io::open_bytes(name, &bytes).unwrap()), "{name}: the fixture holds macros");
    bytes
}

/// A save of a document with a macro project in a format that can't hold one (Save As .docx
/// from a `.docm`, `.dotx` from a `.dotm`, Word XML, ODT, RTF, text…) names `macros` in what it
/// loses, so the save prompt asks and scripts and the CLI are told; `.docm`, `.dotm` and
/// WordCraft's JSON keep the project and name nothing. (06, question 2: it was dropped without a
/// word; `lossy_features` returned `None` for a `.docx`.)
#[test]
fn a_save_that_leaves_out_the_macro_project_names_it() {
    for source in ["m.docm", "t.dotm"] {
        let doc = crate::io::open_bytes(source, &docm_bytes(source)).unwrap();
        for name in ["m.docx", "t.dotx", "m.xml", "m.odt", "m.rtf", "m.txt", "m.html", "m.md", "m.tex"] {
            let lost = crate::io::lossy_features(name, &doc).unwrap_or_default();
            assert!(lost.contains(&"macros"), "{source} → {name}: {lost:?}");
        }
        let word2003 = crate::io::ExportOptions { word2003: true, ..Default::default() };
        assert!(crate::io::lossy_features_with("m.xml", &doc, &word2003).unwrap_or_default().contains(&"macros"), "{source} → Word 2003 XML");
        // A Word document loses nothing else.
        assert_eq!(crate::io::lossy_features("m.docx", &doc), Some(vec!["macros"]), "{source}");
        for name in ["m.docm", "t.dotm", "m.json", "M.DOCM"] {
            assert_eq!(crate::io::lossy_features(name, &doc), None, "{source} → {name}");
        }
    }
    // A document without macros: Word documents keep everything, as before.
    let plain = Document::from_text(ORIGINAL);
    assert_eq!(crate::io::lossy_features("m.docx", &plain), None);
    assert!(!crate::io::lossy_features("m.odt", &plain).unwrap_or_default().contains(&"macros"));
}

/// Save As .docx (and .dotx) from a macro-enabled file through `file.save`, as the CLI, scripts
/// and the app's Save As run it: the result names `macros`, the new file is the document's own
/// and saved (as in Word), it has no macro project, the open document still has it, and Save As
/// `.docm` afterwards writes it back byte for byte; the `.docm` on disk is untouched.
#[test]
fn save_as_a_macro_free_word_document_says_so_and_keeps_the_project_in_the_open_document() {
    for (source, target, again) in [("m.docm", "m.docx", "again.docm"), ("t.dotm", "t.dotx", "again.dotm")] {
        let d = Scratch::new("macros");
        let src = d.path(source);
        let original = docm_bytes(source);
        std::fs::write(&src, &original).unwrap();
        let mut s = Session::new(Document::new());
        s.run("file.open", &json!({"path": src})).unwrap();
        s.run("caret.docEnd", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": EDIT})).unwrap();
        // The source's own format keeps them: nothing named.
        assert!(crate::cmd::file::lossy_on_save(&s, &json!({})).is_none(), "{source}");
        let to = d.path(target);
        assert_eq!(crate::cmd::file::lossy_on_save(&s, &json!({"path": to})), Some(vec!["macros"]), "{target}: known before saving");
        let r = s.run("file.save", &json!({"path": to})).unwrap();
        assert_eq!(r["lossy"], json!(["macros"]), "{target}: {r}");
        assert_eq!(s.path.as_deref(), Some(to.as_path()), "{target}: the document's file from now on");
        assert!(!s.dirty, "{target}");
        let saved = crate::io::open_path(&to).unwrap();
        assert!(!wordcraft_docx::has_macro_project(&saved), "{target}: a macro-free package");
        assert!(saved.plain_text(StoryRef::Body).contains(EDIT.trim()), "{target}");
        assert!(wordcraft_docx::has_macro_project(&s.doc), "{target}: the open document keeps the project");
        assert_eq!(std::fs::read(&src).unwrap(), original, "{source} untouched");
        // Saving the .docx again still says so.
        assert_eq!(s.run("file.save", &json!({})).unwrap()["lossy"], json!(["macros"]), "{target}");
        // Save As a macro-enabled file again brings the project back.
        let back = d.path(again);
        assert!(s.run("file.save", &json!({"path": back})).unwrap().get("lossy").is_none(), "{again}");
        let reopened = crate::io::open_path(&back).unwrap();
        assert_eq!(reopened.passthrough.get("word/vbaProject.bin").map(|b| b.as_slice()), Some(VBA), "{again}: byte for byte");
    }
}
