//! The UI's save flow, pinned transition by transition. Every test drives `WordApp` headless, on a stopped clock, in a scratch folder of
//! its own, with synthetic documents only, and asserts the fields, what is on disk, that a failed
//! write leaves the document unsaved, that a recovery copy outlives every failed save, and what
//! the status bar or dialog says.
//!
//! # The state machine as it is (desktop unless marked Web)
//!
//! State = the fields that matter: `session.dirty`, `session.path`, `autosave_path` (AutoSave
//! armed: `autosaves()`), `autosave_lossy`, `autosave_job`, `save_conflict`, `dialog`,
//! `file_dialog`, `lossy_ok`, `quit_requested`; Web: `saves_in_flight`, `awaiting_save`.
//!
//! | # | State | Event | Next state, side effects |
//! |---|---|---|---|
//! | 1 | Untitled, dirty | Save, no picker | unchanged; `{saved:false}`; nothing written |
//! | 2 | Untitled, dirty | Save, picker answers P | `path=P`, clean, `autosave_path=P`; P written |
//! | 3 | Saved here, dirty | Save As, picker answers Q | `path=Q`, clean, AutoSave follows Q; old file keeps its last save |
//! | 4 | Any | Save As, picker cancelled | unchanged; `{saved:false}` |
//! | 5 | Picker open | second file dialog | ignored (`Asked::Busy`); status "A file dialog is already open."; a Save-Changes Save answered Busy drops the prompt with `{done:false}` |
//! | 6 | Picker open | document replaced, then the answer arrives | error "the document for this save has been replaced"; nothing written |
//! | 7 | Saved here, dirty | Save, folder refuses the temporary file | error (status bar, no dialog); dirty; file and save stamp unchanged; recovery copy kept; AutoSave still armed |
//! | 8 | Saved here, dirty | Save As into a folder that refuses new files | error (status bar); `path` unchanged; dirty; nothing created |
//! | 9 | Saved here, dirty | Save, folder gone | as row 11 ("file changed or is unavailable"): `dialog=SaveConflict{read_only:false}`, `autosave_path=None`; dirty; nothing created |
//! | 10 | Any | Save As onto a folder's name | error "that is a folder" (status bar); unchanged |
//! | 11 | Saved here, dirty | Save, file changed by another program (#813) | `dialog=SaveConflict{read_only:false}`, `save_conflict=None` (moved into it), `autosave_path=None`; dirty; file untouched; copy kept |
//! | 11a | SaveConflict | Keep editing | `dialog=None`; dirty; AutoSave stays off until a save succeeds |
//! | 11b | SaveConflict | Save copy…, picker answers Q | `path=Q`, clean, AutoSave on; the other program's file untouched; copy discarded |
//! | 11c | SaveConflict | Save copy…, picker cancelled | `dialog=None`; `{saved:false}`; dirty; same file; AutoSave off |
//! | 11d | SaveConflict | document replaced by a script | `dialog=None`; a late answer is an error |
//! | 12 | Saved here, dirty | Save, file deleted meanwhile | as row 11 (unavailable = changed); nothing written in its place; an explicit Save As to the name writes it |
//! | 13 | Saved here, dirty | Save, file read-only | `dialog=SaveConflict{read_only:true}`; dirty; file untouched; from the Save Changes prompt: `{done:false}`, the held-back command dropped, nothing closes |
//! | 13a | SaveChanges | Save, file changed by another program | `dialog=SaveConflict{read_only:false}`; `{done:false}`; `then` dropped; dirty |
//! | 14 | Saved here, dirty, idle ≥1.5 s, ≥2.5 s since last | AutoSave tick | `autosave_job=Some` (worker); finished OK → clean if nothing changed since |
//! | 14a | AutoSave writing | typing, then the write lands | dirty; the file holds the older text; `file_version` follows it, so the next Save isn't a conflict |
//! | 15 | Untitled / opened-not-saved-here / lossy format / switch off, dirty | AutoSave tick | no job; nothing written |
//! | 15a | Untitled | AutoSave switch on | error with the reason (#196); stays off |
//! | 16 | AutoSave writing | Save | waits for the write, then writes the newest text; `autosave_job=None`; clean |
//! | 17 | AutoSave writing | AutoSave write fails | status "AutoSave failed: …"; `autosave_path=None` (off until the next Save); dirty; save stamp back; file untouched; copy kept |
//! | 17a | AutoSave writing | file changed by another program / made read-only | status "AutoSave failed: …"; `dialog=SaveConflict{read_only}` (if no dialog is up); AutoSave off; dirty; stamp back; file untouched; copy kept |
//! | 18 | AutoSave writing | window close | waits for the write; closes; recovery copy discarded (clean) |
//! | 18a | AutoSave writing, the write fails | window close | waits; "AutoSave failed"; dirty, so `dialog=SaveChanges{quit}`; `close_requested()` false; copy kept |
//! | 19 | Dirty | window close | `dialog=SaveChanges{then:file.close, quit:true}`; `close_requested()` false |
//! | 19a | SaveChanges (quit) | Cancel | `dialog=None`; `{done:false}`; nothing else |
//! | 19b | SaveChanges (quit) | Don't Save | `quit_requested`; recovery copy purged; file untouched |
//! | 19c | SaveChanges (quit) | Save | written; clean; `quit_requested`; copy discarded on the next close check |
//! | 20 | Saved here, clean | window close | closes; recovery copy discarded |
//! | 21 | Dirty | New / Open / Close | `dialog=SaveChanges{then}`; `{pending:saveChanges}`; document untouched |
//! | 21a | SaveChanges | Cancel | `dialog=None`; document stays; copy kept |
//! | 21b | SaveChanges | Don't Save | `then` runs; recovery copy purged |
//! | 21c | SaveChanges, untitled | Save | Save As picker; `{pending:saveAs}`; once saved `then` runs (`{done:true}`) |
//! | 22 | SaveChanges | Save fails | prompt back with the reason (`message`), same on the status bar; `{Err(message)}`; `then` still waiting; dirty; copy kept |
//! | 23 | SaveChanges, untitled | Save As fails / is cancelled | fails: prompt back with the reason; cancelled: `{done:false}`, prompt gone, `then` dropped, document stays |
//! | 24 | SaveChanges | document replaced by a script | `dialog=None`; a late answer is an error |
//! | 25 | Own file in a lossy format, dirty, with something it loses | Save | `dialog=SaveFormat{lost}`; `{pending:saveFormat}`; nothing written |
//! | 25a | SaveFormat | Cancel | `{saved:false}`; dirty; `lossy_ok=None`; asked again next time |
//! | 25b | SaveFormat | Keep format | saved in place; clean; `lossy_ok=(doc,path,lost)`; AutoSave blocked (Format); not asked again until more would be lost |
//! | 25c | SaveFormat | Save as .docx…, picker answers Q | `path=Q`, clean, AutoSave on; the lossy file untouched |
//! | 25d | Own `.txt`, dirty, nothing lost | Save | `dialog=SaveFormat{lost:[]}` all the same (asked once per document and file) |
//! | 25e | SaveFormat | Keep format, the save fails | error on the status bar; dirty; file untouched; `lossy_ok` kept, so the next Save isn't asked |
//! | 26 | SaveChanges | Save over a lossy own file | `{pending:saveFormat}` with `then` carried; Cancel drops `then`; Keep format saves then runs `then` |
//! | 27 | Web, dirty | Save (picker) | download; `saves_in_flight+=1`; clean but `save_unconfirmed()`; New/Open still ask |
//! | 27a | Web, save in flight | picker Saved | confirmed; clean |
//! | 27b | Web, save in flight | picker Failed / Cancelled | dirty again; Failed: status "Couldn't download …" |
//! | 27c | Web, dirty | Save, the download refused | error "Couldn't download …" on the status bar; dirty; nothing in flight; from the prompt: prompt back with the reason |
//! | 28 | Web, SaveChanges | Save (picker) | `awaiting_save=Some`; status "Waiting for the save to finish…"; `{pending:save}` |
//! | 28a | Web, awaiting | Saved, unchanged since | `then` runs |
//! | 28b | Web, awaiting | Saved, edited meanwhile | prompt back (no message); dirty |
//! | 28c | Web, awaiting | Cancelled / Failed | prompt back with the reason; dirty |
//! | 29 | Start, clean, recovery copy on disk | recovery scan ready | File › Recover opens (`backstage_page=recover`); restore → dirty, `path=None` |
//! | 30 | Start, dirty before the scan is ready | recovery scan ready | nothing opens (the typing comes first) |
//!
//! Failures the file system can produce for real are driven here (a folder that refuses new
//! files, a read-only file, a folder gone, a folder in the file's place, a file changed or
//! deleted by another program). `io/fault.rs` (write, fsync, rename and disk-full faults, a
//! change between preparing the file and verifying the destination) is compiled only for the
//! engine's own tests (`cfg(test)` in `io.rs`, `io/safe_save.rs` and `recovery/desktop.rs`), so
//! this crate can't reach it: those faults all surface here as the same `CmdError::Failed` this
//! file drives through the refused temporary file, `FileConflict` (rows 9, 11, 12) or
//! `ReadOnly` (row 13). The task report maps each fault kind to what the person sees.
//!
//! Known wrong, pinned as `#[ignore = "known: …"]` with the right expectation: none at present.
#![allow(clippy::unwrap_used)]
use super::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};
use wordcraft_doc::{Document, StoryRef};
use wordcraft_engine::io;

const TYPED: &str = "Synthetic chapter one.";
const MORE: &str = " A second sentence.";
#[cfg(unix)]
const FOLDER_FIXTURE_SKIPPED: &str = "skipped: this account writes into read-only folders (root?)";

// Fixtures.

/// A scratch folder for one test, removed (writable again first) when the test ends.
struct Scratch(PathBuf);
impl Scratch {
    fn new(what: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!("etch-save-flow-{what}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn folder(&self, name: &str) -> PathBuf {
        let d = self.0.join(name);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    /// A `.docx` this app didn't write (another program's file).
    fn docx_on_disk(&self, name: &str, text: &str) -> PathBuf {
        let p = self.file(name);
        io::save_path(&p, &Document::from_text(text)).unwrap();
        p
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        writable_again(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
fn writable_again(dir: &Path) {
    set_mode(dir, 0o755);
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if e.path().is_dir() {
            writable_again(&e.path());
        }
    }
}
#[cfg(not(unix))]
fn writable_again(_dir: &Path) {}

/// Make `folder` refuse new files (a read-only share, a protected folder). `false` when this
/// account writes there anyway (root), in which case the test has nothing to drive.
#[cfg(unix)]
fn lock_folder(folder: &Path) -> bool {
    set_mode(folder, 0o555);
    let refused = std::fs::File::create_new(folder.join(".probe")).is_err();
    if !refused {
        let _ = std::fs::remove_file(folder.join(".probe"));
        eprintln!("{FOLDER_FIXTURE_SKIPPED}");
    }
    refused
}

#[cfg(unix)]
fn unlock_folder(folder: &Path) {
    set_mode(folder, 0o755);
}

fn set_read_only(path: &Path, on: bool) {
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(on);
    std::fs::set_permissions(path, perms).unwrap();
}

/// A headless app on a stopped clock with a blank document.
fn app() -> WordApp {
    with_services(Services::default())
}

fn with_services(services: Services) -> WordApp {
    let mut a = WordApp::new(Session::new(Document::new()), services);
    a.session.clock = wordcraft_engine::clock::Clock { now: Some(1_700_000_000), zone: None };
    a
}

/// A new document with unsaved typing (state: untitled, dirty).
fn typed() -> WordApp {
    let mut a = app();
    a.run("text.insert", json!({"text": TYPED})).unwrap();
    assert!(a.session.dirty);
    a
}

/// A document saved here (state: saved here, clean; AutoSave covers it).
fn saved(d: &Scratch, name: &str) -> (WordApp, PathBuf) {
    let p = d.file(name);
    let mut a = typed();
    a.run("file.save", json!({"path": p.to_string_lossy()})).unwrap();
    assert!(!a.session.dirty && a.autosaves() && a.saved_here());
    (a, p)
}

/// A document saved here and edited since (state: saved here, dirty), with the file's bytes.
fn edited(d: &Scratch, name: &str) -> (WordApp, PathBuf, Vec<u8>) {
    let (mut a, p) = saved(d, name);
    let bytes = std::fs::read(&p).unwrap();
    a.run("text.insert", json!({"text": MORE})).unwrap();
    assert!(a.session.dirty);
    (a, p, bytes)
}

fn body(a: &WordApp) -> String {
    a.session.doc.plain_text(StoryRef::Body)
}

fn on_disk(p: &Path) -> String {
    io::open_path(p).unwrap().plain_text(StoryRef::Body)
}

fn status(a: &WordApp) -> String {
    a.status_msg.as_ref().map(|(m, _)| m.clone()).unwrap_or_default()
}

fn dialog(a: &WordApp) -> Option<&'static str> {
    a.dialog.as_ref().map(dialogs::Dialog::name)
}

fn prompt_message(a: &WordApp) -> Option<String> {
    match &a.dialog {
        Some(dialogs::Dialog::SaveChanges { message, .. }) => Some(message.clone()),
        _ => None,
    }
}

/// A blocking save picker that answers `path` (none: cancelled).
fn picker(path: Option<&Path>) -> Box<dyn Fn(&str) -> Option<String>> {
    let path = path.map(|p| p.to_string_lossy().into_owned());
    Box::new(move |_| path.clone())
}

/// Dialogs an asynchronous host was asked to show, each with the channel its answer goes through.
type Shown = Rc<RefCell<Vec<(file_dialogs::FileDialogRequest, Sender<file_dialogs::FileDialogAnswer>)>>>;

/// An app whose host shows file dialogs asynchronously: nothing is answered until the test does.
fn async_app() -> (WordApp, Shown) {
    let shown: Shown = Rc::default();
    let log = shown.clone();
    let services = Services {
        file_dialog: Some(Box::new(move |req| {
            let (tx, rx) = channel();
            log.borrow_mut().push((req, tx));
            rx
        })),
        ..Default::default()
    };
    (with_services(services), shown)
}

fn answer(shown: &Shown, picked: Option<&Path>) {
    let (_, tx) = shown.borrow_mut().pop().unwrap();
    tx.send(Ok(picked.map(|p| p.to_string_lossy().into_owned()).into_iter().collect())).unwrap();
}

/// A browser app: saves are downloads the picker finishes later (#1315).
fn web_app() -> (WordApp, async_saves::AsyncSaves) {
    let saves = async_saves::AsyncSaves::default();
    let hook = saves.clone();
    let services = Services {
        download: Some(Box::new(move |_, _| {
            hook.start();
            Ok(())
        })),
        async_saves: Some(saves.clone()),
        ..Default::default()
    };
    let mut a = with_services(services);
    a.run("text.insert", json!({"text": TYPED})).unwrap();
    (a, saves)
}

// Recovery copies.

fn with_recovery(a: &mut WordApp, root: &Path) {
    a.session.enable_recovery(root.to_path_buf(), std::sync::Arc::new(|| {})).unwrap();
    settle(a);
}

/// Let the recovery worker finish what it is doing.
fn settle(a: &mut WordApp) {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        a.session.recovery_tick();
        if a.session.recovery_ready() && !a.session.recovery_busy() {
            return;
        }
        assert!(Instant::now() < end, "recovery did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Write the document's recovery copy now (the periodic copy would come 10 s after start).
fn copy_now(a: &mut WordApp) {
    assert_eq!(a.session.emergency_snapshot(Duration::from_secs(10)), Ok(true));
}

/// Recovery copies (generations) under `root`.
fn copies(root: &Path) -> usize {
    let mut n = 0;
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            n += copies(&p);
        } else if p.file_name().and_then(|f| f.to_str()).is_some_and(|f| f.starts_with("snapshot-") && f.ends_with(".json")) {
            n += 1;
        }
    }
    n
}

fn tick_autosave(a: &mut WordApp) {
    a.autosave_tick(now_ms() + 10_000.0);
}

// Save and Save As (rows 1–6).

/// Rows 1–2: Save of an untitled document asks where; without an answer nothing changes, with
/// one the document lives there and AutoSave covers it.
#[test]
fn save_of_an_untitled_document_asks_where_and_saves_there() {
    let d = Scratch::new("untitled");
    let mut a = typed();
    let r = a.run("file.save", json!({})).unwrap();
    assert_eq!(r["saved"], false);
    assert!(a.session.path.is_none() && a.session.dirty && a.dialog.is_none() && !a.file_dialog_open());
    assert!(std::fs::read_dir(&d.0).unwrap().next().is_none(), "nothing was written");

    let p = d.file("novel.docx");
    a.services.pick_save = Some(picker(Some(&p)));
    a.run("file.save", json!({})).unwrap();
    assert_eq!(a.session.path.as_deref(), Some(p.as_path()));
    assert!(!a.session.dirty && a.autosaves() && a.saved_here());
    assert_eq!(on_disk(&p), TYPED);
    assert!(a.dialog.is_none() && !a.file_dialog_open());
}

/// Row 3: Save As moves the document to the new file; the old file keeps its last save and
/// AutoSave writes the new one from then on.
#[test]
fn save_as_moves_the_document_and_autosave_follows() {
    let d = Scratch::new("save-as");
    let (mut a, p, _) = edited(&d, "novel.docx");
    let q = d.file("renamed.docx");
    a.services.pick_save = Some(picker(Some(&q)));
    a.run("file.saveAs", json!({})).unwrap();
    assert_eq!(a.session.path.as_deref(), Some(q.as_path()));
    assert!(!a.session.dirty && a.autosaves());
    assert_eq!(on_disk(&q), format!("{TYPED}{MORE}"));
    assert_eq!(on_disk(&p), TYPED, "the old file keeps its last save");

    a.run("text.insert", json!({"text": " Third."})).unwrap();
    tick_autosave(&mut a);
    assert!(a.autosave_job.is_some());
    a.finish_autosave_job(true);
    assert!(!a.session.dirty);
    assert_eq!(on_disk(&q), format!("{TYPED}{MORE} Third."));
    assert_eq!(on_disk(&p), TYPED, "AutoSave never goes back to the old file");
}

/// Row 4: a cancelled Save As picker changes nothing: same file, still dirty, nothing written.
#[test]
fn save_as_cancelled_in_the_picker_changes_nothing() {
    let d = Scratch::new("save-as-cancel");
    let (mut a, p, bytes) = edited(&d, "novel.docx");
    a.services.pick_save = Some(picker(None));
    let r = a.run("file.saveAs", json!({})).unwrap();
    assert_eq!(r["saved"], false);
    assert_eq!(a.session.path.as_deref(), Some(p.as_path()));
    assert!(a.session.dirty && a.autosaves());
    assert_eq!(std::fs::read(&p).unwrap(), bytes);
    assert_eq!(body(&a), format!("{TYPED}{MORE}"));
    assert!(a.dialog.is_none() && !a.file_dialog_open());
}

/// Row 5: one file dialog at a time. A second Save As while one is open is ignored and says so;
/// a Save Changes prompt whose Save needs the picker is answered "busy" and gives up (`done:
/// false`), the document staying as it is.
#[test]
fn a_second_file_dialog_while_one_is_open_is_refused() {
    let d = Scratch::new("busy");
    let (mut a, shown) = async_app();
    a.run("text.insert", json!({"text": TYPED})).unwrap();
    a.run("file.saveAs", json!({})).unwrap();
    assert!(a.file_dialog_open());
    assert_eq!(shown.borrow().len(), 1);
    a.status_msg = None;
    a.run("file.saveAs", json!({})).unwrap();
    assert_eq!(shown.borrow().len(), 1, "no second dialog");
    assert_eq!(status(&a), "A file dialog is already open.");

    assert_eq!(a.run("file.new", json!({})).unwrap()["pending"], "saveChanges");
    let r = a.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
    assert_eq!(r["done"], false);
    assert!(a.dialog.is_none(), "the prompt gave up");
    assert_eq!(body(&a), TYPED, "the document stays");
    assert_eq!(shown.borrow().len(), 1);

    // The first dialog's answer still saves the document.
    let p = d.file("novel.docx");
    answer(&shown, Some(&p));
    a.poll_file_dialog().unwrap().unwrap();
    assert!(!a.session.dirty && a.session.path.as_deref() == Some(p.as_path()));
    assert_eq!(on_disk(&p), TYPED);
}

/// Row 6: a Save As answer for a document that has been replaced meanwhile is refused and
/// writes nothing (it would have saved the new document under the old one's chosen name).
#[test]
fn a_late_save_as_answer_for_a_replaced_document_is_refused() {
    let d = Scratch::new("stale-answer");
    let (mut a, shown) = async_app();
    a.run("text.insert", json!({"text": TYPED})).unwrap();
    a.run("file.saveAs", json!({})).unwrap();
    a.execute("file.new", json!({})).unwrap();
    a.run("text.insert", json!({"text": "Another document"})).unwrap();
    let p = d.file("novel.docx");
    answer(&shown, Some(&p));
    let err = a.poll_file_dialog().unwrap().unwrap_err();
    assert!(err.contains("replaced"), "{err}");
    assert!(!p.exists());
    assert!(a.session.path.is_none() && a.session.dirty);
    assert!(!a.file_dialog_open());
}

// Failures the file system produces for real (rows 7–13).

/// Row 7 and 17: the folder refuses the temporary file a safe save needs (a read-only share, a
/// protected folder). Save fails with a plain error on the status bar, no dialog: the file, the
/// edits, the save stamp and the recovery copy all stay. AutoSave meets the same refusal, says
/// so once and stops until the next Save; the next Save (folder writable again) writes, arms
/// AutoSave again and only then lets the recovery copy go.
#[test]
#[cfg(unix)]
fn save_into_a_folder_that_refuses_new_files_keeps_the_file_the_edits_and_the_recovery_copy() {
    let d = Scratch::new("ro-folder");
    let folder = d.folder("docs");
    let root = d.file("recovery");
    let mut a = typed();
    with_recovery(&mut a, &root);
    let p = folder.join("novel.docx");
    a.run("file.save", json!({"path": p.to_string_lossy()})).unwrap();
    a.run("text.insert", json!({"text": MORE})).unwrap();
    copy_now(&mut a);
    assert_eq!(copies(&root), 1);
    let bytes = std::fs::read(&p).unwrap();
    let core = a.session.doc.core.clone();
    if !lock_folder(&folder) {
        return;
    }

    a.status_msg = None;
    let err = a.run("file.save", json!({})).unwrap_err();
    assert!(err.contains("temporary file") && err.contains("Save As"), "{err}");
    assert!(a.session.dirty, "a failed save never marks the document saved");
    assert_eq!(std::fs::read(&p).unwrap(), bytes, "the file is untouched");
    assert_eq!(body(&a), format!("{TYPED}{MORE}"));
    assert_eq!(a.session.doc.core, core, "the save stamp is back");
    assert_eq!(dialog(&a), None, "not a conflict: the status bar says why");
    assert_eq!(status(&a), err);
    assert!(a.autosaves(), "AutoSave stays armed after a plain failure (it stops on its own failure)");
    a.session.recovery_tick();
    settle(&mut a);
    assert_eq!(copies(&root), 1, "the recovery copy outlives a failed save");

    a.status_msg = None;
    tick_autosave(&mut a);
    a.finish_autosave_job(true);
    assert!(status(&a).starts_with("AutoSave failed"), "{}", status(&a));
    assert!(!a.autosaves() && a.session.dirty);
    assert_eq!(std::fs::read(&p).unwrap(), bytes);
    assert_eq!(a.session.doc.core, core, "AutoSave's failed write puts the stamp back too");
    a.status_msg = None;
    tick_autosave(&mut a);
    a.finish_autosave_job(true);
    assert_eq!(status(&a), "", "not retried until the next Save");
    assert_eq!(copies(&root), 1);

    unlock_folder(&folder);
    a.run("file.save", json!({})).unwrap();
    assert!(!a.session.dirty && a.autosaves());
    assert_eq!(on_disk(&p), format!("{TYPED}{MORE}"));
    a.session.recovery_tick();
    settle(&mut a);
    assert_eq!(copies(&root), 0, "the copy goes once the real save succeeded");
}

/// Row 8: Save As into a folder that refuses new files leaves the document where it was, with
/// its edits, and says why on the status bar, whether the path was given or picked.
#[test]
#[cfg(unix)]
fn save_as_into_a_folder_that_refuses_new_files_leaves_the_document_where_it_was() {
    let d = Scratch::new("ro-save-as");
    let (mut a, p, bytes) = edited(&d, "novel.docx");
    let locked = d.folder("locked");
    if !lock_folder(&locked) {
        return;
    }
    let q = locked.join("copy.docx");

    a.status_msg = None;
    let err = a.run("file.saveAs", json!({"path": q.to_string_lossy()})).unwrap_err();
    assert!(err.contains("can't create files in this folder"), "{err}");
    assert!(!q.exists());
    assert_eq!(a.session.path.as_deref(), Some(p.as_path()));
    assert!(a.session.dirty && a.autosaves());
    assert_eq!(std::fs::read(&p).unwrap(), bytes);
    assert_eq!(status(&a), err);
    assert_eq!(dialog(&a), None);

    a.status_msg = None;
    a.services.pick_save = Some(picker(Some(&q)));
    a.run("file.saveAs", json!({})).unwrap();
    assert!(status(&a).contains("can't create files in this folder"), "the picked Save As says why too: {:?}", status(&a));
    assert!(!q.exists());
    assert_eq!(a.session.path.as_deref(), Some(p.as_path()));
    assert!(a.session.dirty);
}

/// Row 9: the file's folder is gone (an unplugged drive, a share that dropped). The file is
/// unavailable, which the save treats like a file changed by another program (#813): the
/// conflict dialog, AutoSave off, the edits kept; Save copy… writes them elsewhere.
#[test]
fn save_when_the_folder_is_gone_offers_a_copy_and_keeps_the_edits() {
    let d = Scratch::new("gone");
    let folder = d.folder("drive");
    let p = folder.join("novel.docx");
    let mut a = typed();
    a.run("file.save", json!({"path": p.to_string_lossy()})).unwrap();
    a.run("text.insert", json!({"text": MORE})).unwrap();
    std::fs::remove_dir_all(&folder).unwrap();

    a.status_msg = None;
    let err = a.run("file.save", json!({})).unwrap_err();
    assert!(err.contains("file changed or is unavailable"), "{err}");
    assert_eq!(status(&a), err);
    assert!(matches!(a.dialog, Some(dialogs::Dialog::SaveConflict { read_only: false, .. })), "{:?}", dialog(&a));
    assert!(a.session.dirty && !a.autosaves());
    assert_eq!(body(&a), format!("{TYPED}{MORE}"));
    assert!(!folder.exists(), "nothing was created in the folder's place");

    let copy = d.file("novel copy.docx");
    a.services.pick_save = Some(picker(Some(&copy)));
    a.run("ui.saveConflict", json!({"answer": "copy"})).unwrap();
    assert_eq!(a.session.path.as_deref(), Some(copy.as_path()));
    assert!(!a.session.dirty && a.autosaves());
    assert_eq!(on_disk(&copy), format!("{TYPED}{MORE}"));
}

/// Row 10: Save As onto a folder's name is refused with a plain error; nothing changes.
#[test]
fn save_as_onto_a_folders_name_is_refused() {
    let d = Scratch::new("is-a-folder");
    let (mut a, p, bytes) = edited(&d, "novel.docx");
    let folder = d.folder("afolder.docx");
    a.status_msg = None;
    let err = a.run("file.saveAs", json!({"path": folder.to_string_lossy()})).unwrap_err();
    assert!(err.contains("folder"), "{err}");
    assert!(folder.is_dir());
    assert_eq!(a.session.path.as_deref(), Some(p.as_path()));
    assert!(a.session.dirty);
    assert_eq!(std::fs::read(&p).unwrap(), bytes);
    assert_eq!(status(&a), err);
    assert_eq!(dialog(&a), None);
}

/// Rows 11, 11a, 11b: the file was changed by another program (#813). Save is refused with the
/// conflict dialog (`save_conflict` moves into it) and AutoSave stops; Keep editing leaves
/// everything as it is; Save copy… moves the document to the picked file, arms AutoSave and
/// only then lets the recovery copy go. The other program's file is never overwritten.
#[test]
fn save_over_a_file_changed_by_another_program_is_refused_and_each_answer_keeps_the_edits() {
    let d = Scratch::new("conflict");
    let root = d.file("recovery");
    let (mut a, p, _) = edited(&d, "novel.docx");
    with_recovery(&mut a, &root);
    copy_now(&mut a);
    let theirs = io::save_bytes("novel.docx", &Document::from_text("Their version")).unwrap();
    std::fs::write(&p, &theirs).unwrap();

    a.status_msg = None;
    let err = a.run("file.save", json!({})).unwrap_err();
    assert!(matches!(a.dialog, Some(dialogs::Dialog::SaveConflict { read_only: false, .. })), "{:?}", dialog(&a));
    assert!(a.save_conflict.is_none(), "the refusal moved into the dialog");
    assert!(!a.autosaves(), "AutoSave stops");
    assert!(a.session.dirty);
    assert_eq!(std::fs::read(&p).unwrap(), theirs);
    assert_eq!(status(&a), err);
    a.session.recovery_tick();
    settle(&mut a);
    assert_eq!(copies(&root), 1);

    a.run("ui.saveConflict", json!({"answer": "cancel"})).unwrap();
    assert_eq!(dialog(&a), None);
    assert!(a.session.dirty && !a.autosaves());
    assert_eq!(body(&a), format!("{TYPED}{MORE}"));
    tick_autosave(&mut a);
    assert!(a.autosave_job.is_none(), "AutoSave is off until a save succeeds");
    assert_eq!(std::fs::read(&p).unwrap(), theirs);

    a.run("file.save", json!({})).unwrap_err();
    let copy = d.file("novel copy.docx");
    a.services.pick_save = Some(picker(Some(&copy)));
    a.run("ui.saveConflict", json!({"answer": "copy"})).unwrap();
    assert_eq!(dialog(&a), None);
    assert_eq!(a.session.path.as_deref(), Some(copy.as_path()));
    assert!(!a.session.dirty && a.autosaves());
    assert_eq!(on_disk(&copy), format!("{TYPED}{MORE}"));
    assert_eq!(std::fs::read(&p).unwrap(), theirs, "their file is never overwritten");
    a.session.recovery_tick();
    settle(&mut a);
    assert_eq!(copies(&root), 0);
}

/// Rows 11c–11d: Save copy… whose picker is cancelled changes nothing (the dialog closes, the
/// document stays on its file, unsaved, AutoSave off); a conflict dialog left up while a script
/// replaces the document is closed with it, and a late answer is an error.
#[test]
fn the_conflict_dialogs_copy_cancelled_or_outlived_changes_nothing() {
    let d = Scratch::new("conflict-cancel");
    let (mut a, p, _) = edited(&d, "novel.docx");
    std::fs::write(&p, io::save_bytes("novel.docx", &Document::from_text("Their version")).unwrap()).unwrap();
    let theirs = std::fs::read(&p).unwrap();
    a.run("file.save", json!({})).unwrap_err();
    a.services.pick_save = Some(picker(None));
    assert_eq!(a.run("ui.saveConflict", json!({"answer": "copy"})).unwrap()["saved"], false);
    assert_eq!(dialog(&a), None);
    assert_eq!(a.session.path.as_deref(), Some(p.as_path()));
    assert!(a.session.dirty && !a.autosaves());
    assert_eq!(std::fs::read(&p).unwrap(), theirs);

    a.run("file.save", json!({})).unwrap_err();
    assert_eq!(dialog(&a), Some("saveConflict"));
    a.execute("file.new", json!({"force": true})).unwrap();
    assert_eq!(dialog(&a), None, "the dialog was about the old document");
    let err = a.run("ui.saveConflict", json!({"answer": "copy"})).unwrap_err();
    assert!(err.contains("no save conflict dialog"), "{err}");
    assert_eq!(std::fs::read(&p).unwrap(), theirs);
}

/// Row 13a: Save in the Save Changes prompt over a file another program changed: the conflict
/// dialog, the held-back New dropped (`done: false`), the document open with its edits.
#[test]
fn the_prompts_save_over_a_changed_file_offers_a_copy_and_runs_nothing() {
    let d = Scratch::new("prompt-conflict");
    let (mut a, p, _) = edited(&d, "novel.docx");
    std::fs::write(&p, io::save_bytes("novel.docx", &Document::from_text("Their version")).unwrap()).unwrap();
    let theirs = std::fs::read(&p).unwrap();
    let document = a.session.document_id();
    assert_eq!(a.run("file.new", json!({})).unwrap()["pending"], "saveChanges");
    assert_eq!(a.run("ui.saveChanges", json!({"answer": "save"})).unwrap()["done"], false);
    assert!(matches!(a.dialog, Some(dialogs::Dialog::SaveConflict { read_only: false, .. })), "{:?}", dialog(&a));
    assert_eq!(a.session.document_id(), document, "New didn't run");
    assert!(a.session.dirty && !a.autosaves());
    assert_eq!(body(&a), format!("{TYPED}{MORE}"));
    assert_eq!(std::fs::read(&p).unwrap(), theirs);
}

/// Row 12: the file was deleted meanwhile (moved away, removed by a sync client). Unavailable is
/// treated as changed (#813): nothing is written in its place, the conflict dialog offers a
/// copy, and the edits stay unsaved.
#[test]
fn save_over_a_file_deleted_meanwhile_is_refused_as_unavailable() {
    let d = Scratch::new("deleted");
    let (mut a, p, _) = edited(&d, "novel.docx");
    std::fs::remove_file(&p).unwrap();
    let err = a.run("file.save", json!({})).unwrap_err();
    assert!(err.contains("file changed or is unavailable"), "{err}");
    assert!(!p.exists(), "nothing written in its place");
    assert!(matches!(a.dialog, Some(dialogs::Dialog::SaveConflict { read_only: false, .. })), "{:?}", dialog(&a));
    assert!(a.session.dirty && !a.autosaves());
    a.run("ui.saveConflict", json!({"answer": "cancel"})).unwrap();
    assert_eq!(body(&a), format!("{TYPED}{MORE}"));
    // Save copy… to another name works as for a changed file.
    let copy = d.file("novel copy.docx");
    a.run("file.save", json!({})).unwrap_err();
    a.services.pick_save = Some(picker(Some(&copy)));
    a.run("ui.saveConflict", json!({"answer": "copy"})).unwrap();
    assert!(!a.session.dirty && a.session.path.as_deref() == Some(copy.as_path()));
    assert_eq!(on_disk(&copy), format!("{TYPED}{MORE}"));
}

/// Row 13: a read-only file. Save from the Save Changes prompt is refused with the read-only
/// dialog, the held-back Close is dropped (`done: false`) and the document stays open with its
/// edits; once the file is writable again the same prompt saves and closes.
#[test]
fn a_read_only_file_refuses_the_prompts_save_and_nothing_closes() {
    let d = Scratch::new("read-only");
    let (mut a, p, bytes) = edited(&d, "novel.docx");
    set_read_only(&p, true);
    assert_eq!(a.run("file.close", json!({})).unwrap()["pending"], "saveChanges");
    let r = a.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
    assert_eq!(r["done"], false);
    assert!(matches!(a.dialog, Some(dialogs::Dialog::SaveConflict { read_only: true, .. })), "{:?}", dialog(&a));
    assert!(a.has_document() && !a.quit_requested && a.session.dirty);
    assert_eq!(std::fs::read(&p).unwrap(), bytes);
    assert!(!a.autosaves());
    a.run("ui.saveConflict", json!({"answer": "cancel"})).unwrap();
    assert_eq!(dialog(&a), None);

    set_read_only(&p, false);
    assert_eq!(a.run("file.close", json!({})).unwrap()["pending"], "saveChanges");
    assert_eq!(a.run("ui.saveChanges", json!({"answer": "save"})).unwrap()["done"], true);
    assert!(!a.has_document(), "Close ran once the save succeeded");
    assert_eq!(on_disk(&p), format!("{TYPED}{MORE}"));
}

// AutoSave (rows 14–18).

/// Row 14: AutoSave waits for a pause in typing, writes on a worker, and marks the document
/// saved only when nothing changed since; with nothing to save it doesn't start.
#[test]
fn autosave_waits_for_a_pause_then_writes_and_marks_the_document_saved() {
    let d = Scratch::new("autosave");
    let (mut a, p, bytes) = edited(&d, "novel.docx");
    a.autosave_tick(now_ms() + 100.0);
    assert!(a.autosave_job.is_none(), "typing a moment ago: not yet");
    assert_eq!(std::fs::read(&p).unwrap(), bytes);
    tick_autosave(&mut a);
    assert!(a.autosave_job.is_some(), "the write runs on a worker");
    a.finish_autosave_job(true);
    assert!(a.autosave_job.is_none() && !a.session.dirty);
    assert_eq!(on_disk(&p), format!("{TYPED}{MORE}"));
    a.autosave_tick(now_ms() + 20_000.0);
    assert!(a.autosave_job.is_none(), "nothing to save");
}

/// Row 15: AutoSave never writes a document it doesn't cover: untitled, opened but not saved
/// in this session, saved here in a format that can't keep everything, or with the switch off.
#[test]
fn autosave_never_writes_a_document_it_does_not_cover() {
    let d = Scratch::new("autosave-blocked");
    let mut a = typed();
    assert_eq!(a.autosave_block(), Some(AutoSaveBlock::Unsaved));
    tick_autosave(&mut a);
    assert!(a.autosave_job.is_none() && a.session.dirty);
    // The switch says why instead of turning on and saving nothing (#196).
    let err = a.run("file.autosave", json!({"value": true})).unwrap_err();
    assert_eq!(err, AutoSaveBlock::Unsaved.reason());
    assert!(!a.autosaves());

    let p = d.docx_on_disk("theirs.docx", "Original");
    let before = std::fs::read(&p).unwrap();
    let mut b = app();
    b.run("file.open", json!({"path": p.to_string_lossy()})).unwrap();
    b.run("text.insert", json!({"text": "Typed "})).unwrap();
    assert_eq!(b.autosave_block(), Some(AutoSaveBlock::NotSavedHere));
    tick_autosave(&mut b);
    assert!(b.autosave_job.is_none() && b.session.dirty);
    assert_eq!(std::fs::read(&p).unwrap(), before, "an opened file is never rewritten unasked");

    // A Save of an untitled document to a Markdown name writes a copy: the document stays
    // untitled and unsaved, so AutoSave has nothing to cover.
    let md = d.file("notes.md");
    let mut c = typed();
    assert_eq!(c.run("file.save", json!({"path": md.to_string_lossy()})).unwrap()["saved"], true);
    assert!(c.session.dirty && c.session.path.is_none(), "a copy, not the document's file");
    assert_eq!(std::fs::read_to_string(&md).unwrap().trim(), TYPED);
    assert_eq!(c.autosave_block(), Some(AutoSaveBlock::Unsaved));
    // The document's own Markdown file: AutoSave never writes a format that can't keep everything.
    let mut c = app();
    c.run("file.open", json!({"path": md.to_string_lossy()})).unwrap();
    c.run("text.insert", json!({"text": "Typed "})).unwrap();
    assert_eq!(c.autosave_block(), Some(AutoSaveBlock::Format { name: "notes.md".into() }));
    tick_autosave(&mut c);
    assert!(c.autosave_job.is_none() && c.session.dirty);
    assert_eq!(std::fs::read_to_string(&md).unwrap().trim(), TYPED);

    let (mut e, q, bytes) = edited(&d, "switch-off.docx");
    e.run("file.autosave", json!({"value": false})).unwrap();
    assert!(!e.autosaves());
    tick_autosave(&mut e);
    assert!(e.autosave_job.is_none() && e.session.dirty);
    assert_eq!(std::fs::read(&q).unwrap(), bytes);
}

/// Row 16: a Save while AutoSave writes waits for the write, then writes the newest text, so
/// the older copy can't land on top of the newer one.
#[test]
fn a_save_while_autosave_writes_waits_for_it_and_writes_the_newest_text() {
    let d = Scratch::new("autosave-race");
    let (mut a, p, _) = edited(&d, "novel.docx");
    tick_autosave(&mut a);
    assert!(a.autosave_job.is_some());
    a.run("text.insert", json!({"text": " Third."})).unwrap();
    a.run("file.save", json!({})).unwrap();
    assert!(a.autosave_job.is_none(), "Save waited for the write");
    assert!(!a.session.dirty);
    assert_eq!(on_disk(&p), format!("{TYPED}{MORE} Third."));
    tick_autosave(&mut a);
    assert!(a.autosave_job.is_none(), "nothing left to AutoSave");
}

/// Row 18: closing the window while AutoSave writes waits for the write; the document is
/// saved, so the window closes without a prompt and the recovery copy goes.
#[test]
fn closing_the_window_during_an_autosave_write_waits_for_it() {
    let d = Scratch::new("autosave-quit");
    let root = d.file("recovery");
    let (mut a, p, _) = edited(&d, "novel.docx");
    with_recovery(&mut a, &root);
    copy_now(&mut a);
    tick_autosave(&mut a);
    assert!(a.autosave_job.is_some());
    assert!(a.close_requested(), "nothing unsaved once the write is in");
    assert!(a.autosave_job.is_none() && !a.session.dirty && a.dialog.is_none());
    assert_eq!(on_disk(&p), format!("{TYPED}{MORE}"));
    assert_eq!(copies(&root), 0, "a saved document's copy is discarded on close");
}

/// Row 14a: the person types while AutoSave writes. The write lands (the file holds the older
/// text and the session expects it, so the next Save isn't refused as someone else's change),
/// but the document stays unsaved: the newer typing isn't in the file.
#[test]
fn typing_during_an_autosave_write_keeps_the_document_unsaved() {
    let d = Scratch::new("autosave-typing");
    let (mut a, p, _) = edited(&d, "novel.docx");
    tick_autosave(&mut a);
    assert!(a.autosave_job.is_some());
    a.run("text.insert", json!({"text": " Third."})).unwrap();
    a.finish_autosave_job(true);
    assert!(a.autosave_job.is_none());
    assert!(a.session.dirty, "the newer typing isn't in the file");
    assert_eq!(on_disk(&p), format!("{TYPED}{MORE}"), "the write itself landed");
    assert!(a.autosaves(), "a good write keeps AutoSave on");
    a.run("file.save", json!({})).unwrap();
    assert!(!a.session.dirty && a.dialog.is_none(), "the next Save expects AutoSave's version");
    assert_eq!(on_disk(&p), format!("{TYPED}{MORE} Third."));
}

/// Row 17a: AutoSave meets a file another program changed (#813) or one made read-only. It
/// writes nothing, says so on the status bar, stops, and offers Save copy… in the conflict
/// dialog; the edits, the save stamp and the recovery copy stay.
#[test]
fn autosave_meeting_a_changed_or_read_only_file_stops_and_offers_a_copy() {
    for read_only in [false, true] {
        let d = Scratch::new("autosave-conflict");
        let root = d.file("recovery");
        let (mut a, p, _) = edited(&d, "novel.docx");
        with_recovery(&mut a, &root);
        copy_now(&mut a);
        let core = a.session.doc.core.clone();
        if read_only {
            set_read_only(&p, true);
        } else {
            std::fs::write(&p, io::save_bytes("novel.docx", &Document::from_text("Their version")).unwrap()).unwrap();
        }
        let theirs = std::fs::read(&p).unwrap();
        a.status_msg = None;
        tick_autosave(&mut a);
        a.finish_autosave_job(true);
        assert!(status(&a).starts_with("AutoSave failed"), "{read_only}: {}", status(&a));
        assert!(status(&a).ends_with("Save the document to turn AutoSave back on."), "{}", status(&a));
        match &a.dialog {
            Some(dialogs::Dialog::SaveConflict { read_only: r, .. }) => assert_eq!(*r, read_only),
            other => panic!("{read_only}: {:?}", other.as_ref().map(dialogs::Dialog::name)),
        }
        assert!(a.save_conflict.is_none());
        assert!(a.session.dirty && !a.autosaves(), "{read_only}");
        assert_eq!(a.session.doc.core, core, "{read_only}: the save stamp is back");
        assert_eq!(std::fs::read(&p).unwrap(), theirs, "{read_only}: the file is untouched");
        a.session.recovery_tick();
        settle(&mut a);
        assert_eq!(copies(&root), 1, "{read_only}");
        tick_autosave(&mut a);
        assert!(a.autosave_job.is_none(), "{read_only}: not retried");
        set_read_only(&p, false);
    }
}

/// Row 18a: closing the window while an AutoSave write fails. The close waits for the write,
/// the failure is reported, and since the document is still unsaved the Save Changes prompt
/// asks; the recovery copy stays.
#[test]
#[cfg(unix)]
fn closing_the_window_while_autosave_fails_asks_and_keeps_the_copy() {
    let d = Scratch::new("autosave-quit-fails");
    let folder = d.folder("docs");
    let root = d.file("recovery");
    let mut a = typed();
    with_recovery(&mut a, &root);
    let p = folder.join("novel.docx");
    a.run("file.save", json!({"path": p.to_string_lossy()})).unwrap();
    a.run("text.insert", json!({"text": MORE})).unwrap();
    copy_now(&mut a);
    let bytes = std::fs::read(&p).unwrap();
    if !lock_folder(&folder) {
        return;
    }
    tick_autosave(&mut a);
    assert!(a.autosave_job.is_some());
    assert!(!a.close_requested(), "unsaved after the failed write: asks");
    assert!(a.autosave_job.is_none() && !a.quit_requested && !a.autosaves());
    assert!(
        matches!(&a.dialog, Some(dialogs::Dialog::SaveChanges { then, params, .. }) if then == "file.close" && params["quit"] == true),
        "{:?}",
        dialog(&a)
    );
    assert!(a.session.dirty);
    assert_eq!(std::fs::read(&p).unwrap(), bytes);
    assert_eq!(copies(&root), 1);
    unlock_folder(&folder);
}

// The window's close button (rows 19–20).

/// Rows 19–19c: the window's close button with unsaved changes asks first. Cancel keeps
/// everything; Don't Save quits and purges the recovery copy (the person gave the work up);
/// Save writes, quits, and the copy goes with the saved document.
#[test]
fn closing_the_window_with_unsaved_changes_asks_and_each_answer_does_what_it_says() {
    for choice in ["cancel", "dontSave", "save"] {
        let d = Scratch::new("close-window");
        let root = d.file("recovery");
        let (mut a, p, bytes) = edited(&d, "novel.docx");
        with_recovery(&mut a, &root);
        copy_now(&mut a);
        assert!(!a.close_requested(), "{choice}: asks first");
        assert!(!a.quit_requested);
        match &a.dialog {
            Some(dialogs::Dialog::SaveChanges { then, params, message, .. }) => {
                assert_eq!(then, "file.close");
                assert_eq!(params["quit"], true);
                assert!(message.is_empty());
            }
            other => panic!("{choice}: {:?}", other.as_ref().map(dialogs::Dialog::name)),
        }
        assert_eq!(copies(&root), 1);
        let r = a.run("ui.saveChanges", json!({"answer": choice})).unwrap();
        assert_eq!(dialog(&a), None, "{choice}");
        match choice {
            "cancel" => {
                assert_eq!(r["done"], false);
                assert!(!a.quit_requested && a.session.dirty);
                assert_eq!(body(&a), format!("{TYPED}{MORE}"));
                assert_eq!(std::fs::read(&p).unwrap(), bytes);
                assert_eq!(copies(&root), 1, "cancel keeps the recovery copy");
            }
            "dontSave" => {
                assert_eq!(r["done"], true);
                assert!(a.quit_requested);
                assert_eq!(std::fs::read(&p).unwrap(), bytes, "Don't Save writes nothing");
                assert_eq!(copies(&root), 0, "Don't Save gives the copy up with the changes");
            }
            _ => {
                assert_eq!(r["done"], true);
                assert!(a.quit_requested && !a.session.dirty);
                assert_eq!(on_disk(&p), format!("{TYPED}{MORE}"));
                assert!(a.close_requested(), "the window may close now");
                assert_eq!(copies(&root), 0);
            }
        }
    }
}

/// Row 20: closing the window on a saved document closes at once and discards its recovery
/// copy (it would be offered as unsaved work at the next start).
#[test]
fn closing_the_window_on_a_saved_document_discards_its_recovery_copy() {
    let d = Scratch::new("close-clean");
    let root = d.file("recovery");
    let (mut a, _p, _) = edited(&d, "novel.docx");
    with_recovery(&mut a, &root);
    copy_now(&mut a);
    a.run("file.save", json!({})).unwrap();
    assert!(a.close_requested());
    assert_eq!(dialog(&a), None);
    assert_eq!(copies(&root), 0);
}

// The Save Changes prompt before New / Open / Close (rows 21–24).

/// Rows 21–21c: New with unsaved changes asks; Cancel keeps the document and its recovery copy,
/// Don't Save replaces it and purges the copy, Save (untitled) goes through Save As and runs
/// New once the file is written.
#[test]
fn new_with_unsaved_changes_asks_and_each_answer_does_what_it_says() {
    for choice in ["cancel", "dontSave", "save"] {
        let d = Scratch::new("new-prompt");
        let root = d.file("recovery");
        let mut a = typed();
        with_recovery(&mut a, &root);
        copy_now(&mut a);
        let document = a.session.document_id();
        assert_eq!(a.run("file.new", json!({})).unwrap()["pending"], "saveChanges");
        assert_eq!(dialog(&a), Some("saveChanges"));
        assert_eq!(body(&a), TYPED, "{choice}: untouched while the prompt is up");
        let p = d.file("novel.docx");
        a.services.pick_save = Some(picker(Some(&p)));
        let r = a.run("ui.saveChanges", json!({"answer": choice})).unwrap();
        assert_eq!(dialog(&a), None);
        match choice {
            "cancel" => {
                assert_eq!(r["done"], false);
                assert_eq!(a.session.document_id(), document);
                assert!(a.session.dirty && !p.exists());
                assert_eq!(copies(&root), 1);
            }
            "dontSave" => {
                assert_eq!(r["done"], true);
                assert_ne!(a.session.document_id(), document);
                assert!(!p.exists());
                assert_eq!(copies(&root), 0);
            }
            _ => {
                assert_eq!(r["done"], true);
                assert_ne!(a.session.document_id(), document);
                assert_eq!(on_disk(&p), TYPED);
                assert!(a.session.path.is_none() && !a.autosaves(), "the new document is untitled");
                a.session.recovery_tick();
                settle(&mut a);
                assert_eq!(copies(&root), 0, "the saved document's copy is out of date");
            }
        }
    }
}

/// Row 22: Save in the prompt fails (here: the folder refuses the temporary file). The prompt
/// comes back with the reason, the same on the status bar, the held-back New still waiting,
/// the document open with its edits and its recovery copy; the next Save (folder writable)
/// writes and lets New run.
#[test]
#[cfg(unix)]
fn save_in_the_prompt_that_fails_brings_the_prompt_back_with_the_reason() {
    let d = Scratch::new("prompt-save-fails");
    let folder = d.folder("docs");
    let root = d.file("recovery");
    let mut a = typed();
    with_recovery(&mut a, &root);
    let p = folder.join("novel.docx");
    a.run("file.save", json!({"path": p.to_string_lossy()})).unwrap();
    a.run("text.insert", json!({"text": MORE})).unwrap();
    copy_now(&mut a);
    let bytes = std::fs::read(&p).unwrap();
    let document = a.session.document_id();
    if !lock_folder(&folder) {
        return;
    }

    assert_eq!(a.run("file.new", json!({})).unwrap()["pending"], "saveChanges");
    a.status_msg = None;
    let err = a.run("ui.saveChanges", json!({"answer": "save"})).unwrap_err();
    assert!(err.contains("still open") && err.contains("nothing was lost") && err.contains("novel.docx"), "{err}");
    assert_eq!(prompt_message(&a).as_deref(), Some(err.as_str()), "the prompt is back with the reason");
    assert_eq!(status(&a), err);
    assert_eq!(a.session.document_id(), document);
    assert!(a.session.dirty);
    assert_eq!(body(&a), format!("{TYPED}{MORE}"));
    assert_eq!(std::fs::read(&p).unwrap(), bytes);
    a.session.recovery_tick();
    settle(&mut a);
    assert_eq!(copies(&root), 1);

    unlock_folder(&folder);
    assert_eq!(a.run("ui.saveChanges", json!({"answer": "save"})).unwrap()["done"], true);
    assert_ne!(a.session.document_id(), document, "New ran once the save succeeded");
    assert_eq!(on_disk(&p), format!("{TYPED}{MORE}"));
}

/// Row 23: Save in the prompt for an untitled document goes through Save As. A Save As that
/// fails brings the prompt back with the reason; one that is cancelled gives up the Close
/// (`done: false`) and keeps the document; one that succeeds closes.
#[test]
fn save_as_in_the_prompt_that_fails_or_is_cancelled_keeps_the_document() {
    let d = Scratch::new("prompt-save-as");
    let (mut a, shown) = async_app();
    a.run("text.insert", json!({"text": TYPED})).unwrap();

    assert_eq!(a.run("file.close", json!({})).unwrap()["pending"], "saveChanges");
    assert_eq!(a.run("ui.saveChanges", json!({"answer": "save"})).unwrap()["pending"], "saveAs");
    assert!(a.file_dialog_open() && a.dialog.is_none());
    answer(&shown, Some(&d.file("missing").join("novel.docx")));
    let err = a.poll_file_dialog().unwrap().unwrap_err();
    assert!(err.contains("still open") && err.contains("nothing was lost"), "{err}");
    assert_eq!(prompt_message(&a).as_deref(), Some(err.as_str()));
    assert!(a.has_document() && a.session.dirty && !a.quit_requested && a.session.path.is_none());
    assert_eq!(body(&a), TYPED);

    assert_eq!(a.run("ui.saveChanges", json!({"answer": "save"})).unwrap()["pending"], "saveAs");
    answer(&shown, None);
    assert_eq!(a.poll_file_dialog().unwrap().unwrap()["done"], false);
    assert_eq!(dialog(&a), None, "a cancelled Save As gives the Close up");
    assert!(a.has_document() && a.session.dirty && !a.quit_requested);

    let p = d.file("novel.docx");
    assert_eq!(a.run("file.close", json!({})).unwrap()["pending"], "saveChanges");
    a.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
    answer(&shown, Some(&p));
    assert_eq!(a.poll_file_dialog().unwrap().unwrap()["done"], true);
    assert!(!a.has_document() && !a.quit_requested, "File › Close closed the document, not the window");
    assert_eq!(on_disk(&p), TYPED);
}

/// Row 24: the prompt is bound to the document it asked about: a script replacing the document
/// closes it, and a late answer is an error rather than an action on the new document.
#[test]
fn the_prompt_is_bound_to_the_document_it_asked_about() {
    let mut a = typed();
    assert_eq!(a.run("file.new", json!({})).unwrap()["pending"], "saveChanges");
    a.execute("file.new", json!({})).unwrap();
    assert_eq!(dialog(&a), None, "the prompt was about the old document");
    let err = a.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap_err();
    assert!(err.contains("no Save Changes prompt"), "{err}");
}

// Saving in a format that can't keep everything (rows 25–26).

/// A `.txt` on disk opened and given underlined text, which plain text loses.
fn lossy_txt(d: &Scratch) -> (WordApp, PathBuf) {
    let p = d.file("notes.txt");
    std::fs::write(&p, "Plain notes\n").unwrap();
    let mut a = app();
    a.run("file.open", json!({"path": p.to_string_lossy()})).unwrap();
    a.run("text.insert", json!({"text": "Underlined "})).unwrap();
    a.run("select.all", json!({})).unwrap();
    a.run("format.underline", json!({})).unwrap();
    assert!(a.session.dirty);
    (a, p)
}

/// Rows 25–25b: the first Save over the document's own lossy file asks; Cancel saves nothing
/// and asks again next time; Keep format saves in place, marks the document saved, blocks
/// AutoSave for the file and isn't asked again, until the document gains something more the
/// format loses.
#[test]
fn saving_over_a_lossy_file_asks_once_and_keep_format_saves() {
    let d = Scratch::new("lossy");
    let (mut a, p) = lossy_txt(&d);
    let document = a.session.document_id();
    assert_eq!(a.run("file.save", json!({})).unwrap()["pending"], "saveFormat");
    match &a.dialog {
        Some(dialogs::Dialog::SaveFormat { lost, target, .. }) => {
            assert_eq!(lost, &["formatting"]);
            assert!(target.is_none(), "a save over the document's own file");
        }
        other => panic!("{:?}", other.as_ref().map(dialogs::Dialog::name)),
    }
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "Plain notes\n", "nothing is written while the prompt is up");
    assert!(a.session.dirty);

    assert_eq!(a.run("ui.saveFormat", json!({"answer": "cancel"})).unwrap()["saved"], false);
    assert_eq!(dialog(&a), None);
    assert!(a.session.dirty && a.lossy_ok.is_none());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "Plain notes\n");
    assert_eq!(a.run("file.save", json!({})).unwrap()["pending"], "saveFormat", "asked again");

    assert_eq!(a.run("ui.saveFormat", json!({"answer": "keep"})).unwrap()["saved"], true);
    assert_eq!(dialog(&a), None);
    assert!(!a.session.dirty);
    assert_eq!(a.lossy_ok, Some((document, p.clone(), vec!["formatting".to_string()])));
    assert_eq!(std::fs::read_to_string(&p).unwrap().trim(), "Underlined Plain notes");
    assert_eq!(a.autosave_block(), Some(AutoSaveBlock::Format { name: "notes.txt".into() }));

    a.run("text.insert", json!({"text": "More "})).unwrap();
    assert_eq!(a.run("file.save", json!({})).unwrap()["saved"], true, "not asked again for the same loss");
    assert!(!a.session.dirty);
    a.run("references.footnote", json!({"text": "A note"})).unwrap();
    assert_eq!(a.run("file.save", json!({})).unwrap()["pending"], "saveFormat", "asked again when more would be lost");
    match &a.dialog {
        Some(dialogs::Dialog::SaveFormat { lost, .. }) => assert!(lost.iter().any(|f| f == "footnotes"), "{lost:?}"),
        other => panic!("{:?}", other.as_ref().map(dialogs::Dialog::name)),
    }
    assert!(a.session.dirty);
}

/// Row 25d: plain typing in a `.txt` loses nothing, and the first Save still asks (with an
/// empty list): the prompt is about the format, once per document and file.
#[test]
fn the_first_save_over_a_text_file_asks_even_when_nothing_is_lost() {
    let d = Scratch::new("lossy-nothing");
    let p = d.file("notes.txt");
    std::fs::write(&p, "Plain notes\n").unwrap();
    let mut a = app();
    a.run("file.open", json!({"path": p.to_string_lossy()})).unwrap();
    a.run("text.insert", json!({"text": "More "})).unwrap();
    assert_eq!(a.run("file.save", json!({})).unwrap()["pending"], "saveFormat");
    assert!(matches!(&a.dialog, Some(dialogs::Dialog::SaveFormat { lost, .. }) if lost.is_empty()), "{:?}", dialog(&a));
    assert_eq!(a.run("ui.saveFormat", json!({"answer": "keep"})).unwrap()["saved"], true);
    assert!(!a.session.dirty);
    assert_eq!(std::fs::read_to_string(&p).unwrap().trim(), "More Plain notes");
}

/// Row 25e: Keep format, then the save fails (the folder refuses the temporary file). The
/// error is on the status bar, the document stays unsaved and the file untouched; the choice
/// itself is remembered, so the next Save (folder writable again) saves without asking.
#[test]
#[cfg(unix)]
fn keep_format_whose_save_fails_keeps_the_edits_and_the_choice() {
    let d = Scratch::new("lossy-fails");
    let folder = d.folder("docs");
    let p = folder.join("notes.txt");
    std::fs::write(&p, "Plain notes\n").unwrap();
    let mut a = app();
    a.run("file.open", json!({"path": p.to_string_lossy()})).unwrap();
    a.run("text.insert", json!({"text": "More "})).unwrap();
    assert_eq!(a.run("file.save", json!({})).unwrap()["pending"], "saveFormat");
    if !lock_folder(&folder) {
        return;
    }
    a.status_msg = None;
    let err = a.run("ui.saveFormat", json!({"answer": "keep"})).unwrap_err();
    assert!(err.contains("temporary file"), "{err}");
    assert_eq!(status(&a), err);
    assert!(a.session.dirty && a.dialog.is_none());
    assert!(a.lossy_ok.is_some(), "the person's choice stands");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "Plain notes\n");
    unlock_folder(&folder);
    assert_eq!(a.run("file.save", json!({})).unwrap()["saved"], true, "not asked again");
    assert!(!a.session.dirty);
    assert_eq!(std::fs::read_to_string(&p).unwrap().trim(), "More Plain notes");
}

/// Row 25c: Save as .docx… in the format prompt goes to Save As with a Word document suggested;
/// the document moves there, AutoSave covers it, and the lossy file is left as it was.
#[test]
fn save_as_docx_from_the_format_prompt_moves_the_document_to_a_word_file() {
    let d = Scratch::new("lossy-docx");
    let (mut a, p) = lossy_txt(&d);
    a.run("file.save", json!({})).unwrap();
    let q = d.file("notes.docx");
    let suggested = Rc::new(RefCell::new(String::new()));
    let seen = suggested.clone();
    let target = q.to_string_lossy().into_owned();
    a.services.pick_save = Some(Box::new(move |name| {
        *seen.borrow_mut() = name.to_string();
        Some(target.clone())
    }));
    assert_eq!(a.run("ui.saveFormat", json!({"answer": "docx"})).unwrap()["saved"], true);
    assert!(suggested.borrow().ends_with(".docx"), "suggested {:?}", suggested.borrow());
    assert_eq!(a.session.path.as_deref(), Some(q.as_path()));
    assert!(!a.session.dirty && a.autosaves() && a.lossy_ok.is_none());
    assert_eq!(on_disk(&q).trim_end(), "Underlined Plain notes");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "Plain notes\n");
}

/// Row 26: Save in the Save Changes prompt over a lossy own file asks about the format first,
/// carrying the held-back New. Cancel drops it (nothing saved, document stays); Keep format
/// saves and then lets New run.
#[test]
fn the_format_prompt_in_the_save_changes_flow_runs_the_held_back_command_only_after_keep() {
    let d = Scratch::new("lossy-prompt");
    let (mut a, p) = lossy_txt(&d);
    let document = a.session.document_id();
    assert_eq!(a.run("file.new", json!({})).unwrap()["pending"], "saveChanges");
    assert_eq!(a.run("ui.saveChanges", json!({"answer": "save"})).unwrap()["pending"], "saveFormat");
    match &a.dialog {
        Some(dialogs::Dialog::SaveFormat { then: Some(then), .. }) => assert_eq!(then.then, "file.new"),
        other => panic!("{:?}", other.as_ref().map(dialogs::Dialog::name)),
    }
    assert_eq!(a.run("ui.saveFormat", json!({"answer": "cancel"})).unwrap()["saved"], false);
    assert_eq!(dialog(&a), None);
    assert_eq!(a.session.document_id(), document);
    assert!(a.session.dirty);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "Plain notes\n");

    assert_eq!(a.run("file.new", json!({})).unwrap()["pending"], "saveChanges");
    a.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
    assert_eq!(a.run("ui.saveFormat", json!({"answer": "keep"})).unwrap()["done"], true);
    assert_ne!(a.session.document_id(), document, "New ran after the save");
    assert_eq!(std::fs::read_to_string(&p).unwrap().trim(), "Underlined Plain notes");
}

// The browser (rows 27–28).

/// Rows 27–27b: a web Save hands the bytes to the picker and is confirmed later. Until then the
/// document looks saved but New still asks; the picker's Saved confirms it, Failed or Cancelled
/// leaves it unsaved again (Failed says so on the status bar). Two saves in flight both have
/// to end well.
#[test]
fn web_save_through_the_picker_is_confirmed_only_when_the_picker_finishes() {
    for outcome in ["saved", "failed", "cancelled"] {
        let (mut a, saves) = web_app();
        let r = a.run("file.save", json!({"path": "novel.docx"})).unwrap();
        let ticket = r["pendingSave"].as_u64().unwrap();
        assert_eq!(a.saves_in_flight.len(), 1);
        assert!(!a.session.dirty && a.save_unconfirmed(), "{outcome}: saved as far as the app knows, not confirmed");
        assert_eq!(saves.pending(), 1);
        assert_eq!(a.run("file.new", json!({})).unwrap()["pending"], "saveChanges", "{outcome}: an unconfirmed save still asks");
        a.run("ui.saveChanges", json!({"answer": "cancel"})).unwrap();
        a.status_msg = None;
        saves.finish(
            ticket,
            match outcome {
                "saved" => async_saves::SaveOutcome::Saved,
                "failed" => async_saves::SaveOutcome::Failed("the disk is full".into()),
                _ => async_saves::SaveOutcome::Cancelled,
            },
        );
        a.settle_async_saves();
        assert!(a.saves_in_flight.is_empty() && !a.save_unconfirmed(), "{outcome}");
        match outcome {
            "saved" => {
                assert!(!a.session.dirty);
                assert_eq!(status(&a), "");
            }
            "failed" => {
                assert!(a.session.dirty, "a failed picker save leaves the document unsaved");
                assert!(status(&a).contains("Couldn't download novel.docx") && status(&a).contains("the disk is full"), "{}", status(&a));
            }
            _ => assert!(a.session.dirty, "a cancelled picker save leaves the document unsaved"),
        }
    }

    let (mut a, saves) = web_app();
    let first = a.run("file.save", json!({"path": "novel.docx"})).unwrap()["pendingSave"].as_u64().unwrap();
    a.run("text.insert", json!({"text": MORE})).unwrap();
    let second = a.run("file.save", json!({})).unwrap()["pendingSave"].as_u64().unwrap();
    assert_ne!(first, second);
    assert_eq!(a.saves_in_flight.len(), 2);
    saves.finish(second, async_saves::SaveOutcome::Saved);
    saves.finish(first, async_saves::SaveOutcome::Failed("gone".into()));
    a.settle_async_saves();
    assert!(a.saves_in_flight.is_empty());
    assert!(a.session.dirty, "one of the two saves failed: unsaved, never a false saved");
}

/// Row 27c: the browser refuses the download outright (the hook fails): the error is on the
/// status bar, the document stays unsaved, and nothing is in flight. AutoSave never runs there.
#[test]
fn web_save_whose_download_fails_leaves_the_document_unsaved() {
    let services = Services { download: Some(Box::new(|_, _| Err("the browser blocked it".into()))), ..Default::default() };
    let mut a = with_services(services);
    a.run("text.insert", json!({"text": TYPED})).unwrap();
    assert_eq!(a.autosave_block(), Some(AutoSaveBlock::Browser));
    a.status_msg = None;
    let err = a.run("file.save", json!({"path": "novel.docx"})).unwrap_err();
    assert!(err.contains("Couldn't download novel.docx") && err.contains("the browser blocked it"), "{err}");
    assert_eq!(status(&a), err);
    assert!(a.session.dirty && a.saves_in_flight.is_empty() && !a.save_unconfirmed());
    assert_eq!(a.run("file.new", json!({})).unwrap()["pending"], "saveChanges");
    let err = a.run("ui.saveChanges", json!({"answer": "save"})).unwrap_err();
    assert!(err.contains("still open") && err.contains("the browser blocked it"), "{err}");
    assert_eq!(prompt_message(&a).as_deref(), Some(err.as_str()));
    assert!(a.session.dirty);
}

/// Rows 28–28c: Save in the web Save Changes prompt waits for the picker. Saved and unchanged
/// since runs the held-back New; edited meanwhile brings the prompt back; Cancelled or Failed
/// brings it back with the reason, the document unsaved and still open.
#[test]
fn web_new_waits_for_the_picker_save_and_each_outcome_does_what_it_says() {
    for outcome in ["saved", "edited", "cancelled", "failed"] {
        let (mut a, saves) = web_app();
        let document = a.session.document_id();
        assert_eq!(a.run("file.new", json!({})).unwrap()["pending"], "saveChanges");
        a.status_msg = None;
        assert_eq!(a.run("ui.saveChanges", json!({"answer": "save"})).unwrap()["pending"], "save");
        let ticket = a.awaiting_save.as_ref().map(|w| w.ticket).unwrap();
        assert_eq!(status(&a), "Waiting for the save to finish…");
        assert!(a.dialog.is_none() && !a.session.dirty && a.save_unconfirmed());
        if outcome == "edited" {
            a.run("text.insert", json!({"text": MORE})).unwrap();
        }
        saves.finish(
            ticket,
            match outcome {
                "cancelled" => async_saves::SaveOutcome::Cancelled,
                "failed" => async_saves::SaveOutcome::Failed("no permission".into()),
                _ => async_saves::SaveOutcome::Saved,
            },
        );
        a.settle_async_saves();
        assert!(a.awaiting_save.is_none(), "{outcome}");
        match outcome {
            "saved" => {
                assert_ne!(a.session.document_id(), document, "New ran");
                assert_eq!(dialog(&a), None);
            }
            "edited" => {
                assert_eq!(a.session.document_id(), document, "the edit may not be in the file: ask again");
                assert_eq!(prompt_message(&a).as_deref(), Some(""));
                assert!(a.session.dirty);
            }
            "cancelled" => {
                assert_eq!(a.session.document_id(), document);
                let m = prompt_message(&a).unwrap();
                assert!(m.contains("was cancelled") && m.contains("still open"), "{m}");
                assert_eq!(status(&a), m);
                assert!(a.session.dirty);
            }
            _ => {
                assert_eq!(a.session.document_id(), document);
                let m = prompt_message(&a).unwrap();
                assert!(m.contains("Couldn't save") && m.contains("no permission") && m.contains("nothing was lost"), "{m}");
                assert_eq!(status(&a), m);
                assert!(a.session.dirty);
            }
        }
    }
}

// Recovery copies at start (rows 29–30).

/// One frame of the app's per-frame logic, without a window.
fn frame(ctx: &egui::Context, a: &mut WordApp) {
    ctx.run_ui(egui::RawInput::default(), |ui| a.logic(ui.ctx())).drop_without_applying_deltas();
}

/// Frames until the recovery scan has reported.
fn frames_until_ready(ctx: &egui::Context, a: &mut WordApp) {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        frame(ctx, a);
        if a.session.recovery_ready() && !a.session.recovery_busy() {
            break;
        }
        assert!(Instant::now() < end, "the recovery scan did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
    frame(ctx, a);
}

/// Rows 29–30: a recovery copy left by a session that ended without a save (a crash) opens
/// File › Recover at the next start, and restoring it brings the work back, unsaved and
/// untitled. A start that is already typing by the time the scan reports isn't interrupted.
#[test]
fn a_recovery_copy_found_at_start_opens_file_recover_and_restores_the_work() {
    let d = Scratch::new("recover-at-start");
    let root = d.file("recovery");
    let mut crashed = typed();
    with_recovery(&mut crashed, &root);
    copy_now(&mut crashed);
    assert_eq!(copies(&root), 1);
    drop(crashed);
    assert_eq!(copies(&root), 1, "an unsaved document's copy survives the session");

    let ctx = egui::Context::default();
    let mut a = app();
    a.session.enable_recovery(root.clone(), std::sync::Arc::new(|| {})).unwrap();
    assert!(!a.ui.backstage);
    frames_until_ready(&ctx, &mut a);
    assert!(a.recovery_seen);
    assert!(a.ui.backstage && a.ui.backstage_page == "recover", "File › Recover opens");
    let docs = a.session.recovery_documents();
    assert_eq!(docs.len(), 1, "{docs:?}");
    a.run("file.recover", json!({"restore": docs[0].id})).unwrap();
    settle(&mut a);
    assert_eq!(body(&a), TYPED, "the work is back");
    assert!(a.session.dirty && a.session.path.is_none(), "restored work is unsaved and untitled");

    let mut b = typed();
    b.session.enable_recovery(root, std::sync::Arc::new(|| {})).unwrap();
    frames_until_ready(&ctx, &mut b);
    assert!(b.recovery_seen && !b.ui.backstage, "typing came first: nothing opens over it");
    assert_eq!(body(&b), TYPED);
}

// What the dialogs show (kittest).

/// A harness that renders the app's dialogs only (fonts installed on the first frame).
fn dialog_harness(a: WordApp) -> egui_kittest::Harness<'static, WordApp> {
    let mut fonts = false;
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        move |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            crate::i18n::set_current(crate::i18n::Lang::EN);
            if fonts {
                dialogs::show(app, &ctx);
            } else {
                crate::theme::install_fonts_for(&ctx, false);
                fonts = true;
            }
        },
        a,
    );
    h.run();
    h
}

/// Row 22, as the person sees it: the Save Changes prompt that came back after a failed save
/// names the document, says the reason in red, and its three buttons do what they say.
#[test]
#[cfg(unix)]
fn the_save_changes_prompt_shows_why_the_last_save_failed() {
    use egui_kittest::kittest::Queryable;
    let d = Scratch::new("prompt-render");
    let folder = d.folder("docs");
    let mut a = typed();
    let p = folder.join("novel.docx");
    a.run("file.save", json!({"path": p.to_string_lossy()})).unwrap();
    a.run("text.insert", json!({"text": MORE})).unwrap();
    if !lock_folder(&folder) {
        return;
    }
    a.run("file.new", json!({})).unwrap();
    a.run("ui.saveChanges", json!({"answer": "save"})).unwrap_err();
    let mut h = dialog_harness(a);
    assert!(h.query_all_by_label("Do you want to save changes to novel.docx?").next().is_some());
    assert!(h.query_by_label_contains("Couldn't save novel.docx, so it is still open and nothing was lost").is_some());
    h.get_by_label("Cancel").click();
    h.run();
    assert!(h.state().dialog.is_none());
    assert!(h.state().session.dirty && h.state().has_document());
    assert_eq!(body(h.state()), format!("{TYPED}{MORE}"));

    unlock_folder(&folder);
    h.state_mut().run("file.new", json!({})).unwrap();
    h.run();
    h.get_by_label("Save").click();
    h.run();
    assert!(h.state().dialog.is_none());
    assert_eq!(on_disk(&p), format!("{TYPED}{MORE}"));
    assert_ne!(body(h.state()), format!("{TYPED}{MORE}"), "New ran");
}

/// Row 13, as the person sees it: the read-only dialog says so and offers Save copy…, which
/// saves to the picked file.
#[test]
fn the_read_only_dialog_offers_a_copy() {
    use egui_kittest::kittest::Queryable;
    let d = Scratch::new("read-only-render");
    let (mut a, p, bytes) = edited(&d, "novel.docx");
    set_read_only(&p, true);
    a.run("file.save", json!({})).unwrap_err();
    let copy = d.file("novel copy.docx");
    a.services.pick_save = Some(picker(Some(&copy)));
    let mut h = dialog_harness(a);
    assert!(h.query_by_label("The file is read-only").is_some());
    assert!(h.query_by_label("novel.docx").is_some());
    h.get_by_label("Save copy…").click();
    h.run();
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().session.path.as_deref(), Some(copy.as_path()));
    assert!(!h.state().session.dirty);
    assert_eq!(on_disk(&copy), format!("{TYPED}{MORE}"));
    assert_eq!(std::fs::read(&p).unwrap(), bytes);
    set_read_only(&p, false);
}

/// Row 25, as the person sees it: the format prompt names the file, lists what is lost, and
/// Keep format saves.
#[test]
fn the_format_prompt_lists_what_is_lost_and_keep_format_saves() {
    use egui_kittest::kittest::Queryable;
    let d = Scratch::new("format-render");
    let (mut a, p) = lossy_txt(&d);
    a.run("file.save", json!({})).unwrap();
    let mut h = dialog_harness(a);
    assert!(h.query_by_label("notes.txt is in a format that can't keep everything").is_some());
    assert!(h.query_by_label("Saving in this format loses:").is_some());
    assert!(h.query_by_label_contains("Formatting").is_some());
    h.get_by_label("Keep format").click();
    h.run();
    assert!(h.state().dialog.is_none());
    assert!(!h.state().session.dirty);
    assert_eq!(std::fs::read_to_string(&p).unwrap().trim(), "Underlined Plain notes");
}

/// A failed save puts the save stamp back, properties and settings alike: the document's saved
/// zoom (`settings.zoom`, set by `stamp_save`) is not for a save that didn't happen.
#[test]
#[cfg(unix)]
fn a_failed_save_leaves_the_documents_settings_alone() {
    let d = Scratch::new("stamp-settings");
    let folder = d.folder("docs");
    let mut a = typed();
    let p = folder.join("novel.docx");
    a.run("file.save", json!({"path": p.to_string_lossy()})).unwrap();
    a.run("view.zoom", json!({"value": 150})).unwrap();
    a.run("text.insert", json!({"text": MORE})).unwrap();
    let settings = a.session.doc.settings.clone();
    if !lock_folder(&folder) {
        return;
    }
    a.run("file.save", json!({})).unwrap_err();
    assert!(a.session.dirty);
    assert_eq!(a.session.doc.settings, settings, "a failed save changes no document setting");
}

/// Row 12, the person's way back: once the file is gone, an explicit Save As (or Save copy…)
/// to its own name has nothing to protect (no other version is there), so it writes the file;
/// it used to be refused as "changed or unavailable" for the rest of the session. Plain Save
/// still refuses (above), and so does a Save As once another file is in its place.
#[test]
fn save_as_to_a_deleted_files_own_name_writes_it_again() {
    let d = Scratch::new("deleted-save-as");
    let (mut a, p, _) = edited(&d, "novel.docx");
    std::fs::remove_file(&p).unwrap();
    a.run("file.saveAs", json!({"path": p.to_string_lossy()})).unwrap();
    assert!(!a.session.dirty && a.autosaves());
    assert_eq!(on_disk(&p), format!("{TYPED}{MORE}"));
    // The Save As dialog's own way (`file.save` with the picked path), and plain Save after it.
    a.run("text.insert", json!({"text": " Again."})).unwrap();
    std::fs::remove_file(&p).unwrap();
    a.run("file.save", json!({"path": p.to_string_lossy()})).unwrap();
    a.run("text.insert", json!({"text": " Once more."})).unwrap();
    a.run("file.save", json!({})).unwrap();
    assert_eq!(on_disk(&p), format!("{TYPED}{MORE} Again. Once more."));
}

/// Row 12 with another file put in the deleted one's place: a Save As to that name still
/// refuses it as changed, since what is there now isn't the version this session knows.
#[test]
fn save_as_over_a_file_put_in_the_deleted_ones_place_is_refused() {
    let d = Scratch::new("replaced-save-as");
    let (mut a, p, _) = edited(&d, "novel.docx");
    std::fs::remove_file(&p).unwrap();
    std::fs::write(&p, b"someone else's file").unwrap();
    let err = a.run("file.saveAs", json!({"path": p.to_string_lossy()})).unwrap_err();
    assert!(err.contains("file changed or is unavailable"), "{err}");
    assert_eq!(std::fs::read(&p).unwrap(), b"someone else's file");
    assert!(a.session.dirty);
}
