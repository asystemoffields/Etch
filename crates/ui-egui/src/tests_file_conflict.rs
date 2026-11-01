//! Save-conflict behavior at the UI dispatch boundary, with original synthetic files only.
#![allow(clippy::unwrap_used)]
use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::channel;
use wordcraft_doc::Document;
use wordcraft_engine::io;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!("wordcraft-ui-conflict-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn app(d: &Scratch) -> WordApp {
    let p = d.file("original.docx");
    io::save_path(&p, &Document::from_text("Original")).unwrap();
    let mut a = WordApp::new(Session::new(Document::new()), Services::default());
    a.execute("file.open", json!({"path": p})).unwrap();
    a.execute("document.setText", json!({"text": "Local edit"})).unwrap();
    a
}
fn external(d: &Scratch) -> Vec<u8> {
    let b = io::save_bytes("external.docx", &Document::from_text("External version")).unwrap();
    std::fs::write(d.file("original.docx"), &b).unwrap();
    b
}
fn assert_local(a: &WordApp, d: &Scratch, b: &[u8]) {
    assert!(a.session.dirty);
    assert_eq!(a.session.doc.plain_text(wordcraft_doc::StoryRef::Body), "Local edit");
    assert_eq!(std::fs::read(d.file("original.docx")).unwrap(), b);
}
#[test]
fn scripts_get_error_user_gets_dialog_and_keep_editing_retains_both_versions() {
    let d = Scratch::new();
    let mut a = app(&d);
    let b = external(&d);
    assert!(a.execute("file.save", json!({})).is_err());
    assert!(a.dialog.is_none());
    assert!(a.run("file.save", json!({})).is_err());
    assert!(matches!(a.dialog, Some(dialogs::Dialog::SaveConflict { .. })));
    assert_local(&a, &d, &b);
    assert!(a.run("ui.saveConflict", json!({"answer": "overwrite"})).is_err());
    assert!(a.dialog.is_some());
    a.run("ui.saveConflict", json!({"answer": "cancel"})).unwrap();
    assert!(a.dialog.is_none());
    assert_local(&a, &d, &b);
}
#[test]
fn save_copy_picker_success_cancel_and_changed_document_are_bound_to_the_conflict() {
    for mode in ["copy", "cancel", "new-document", "same-path"] {
        let d = Scratch::new();
        let mut a = app(&d);
        let b = external(&d);
        let (tx, rx) = channel();
        let rx = std::cell::RefCell::new(Some(rx));
        a.services.file_dialog = Some(Box::new(move |request| {
            assert!(matches!(request, file_dialogs::FileDialogRequest::Save { name } if name == "original copy.docx"));
            rx.borrow_mut().take().unwrap()
        }));
        assert!(a.run("file.save", json!({})).is_err());
        a.run("ui.saveConflict", json!({"answer": "copy"})).unwrap();
        assert!(a.file_dialog_open());
        let copy = d.file("copy.docx");
        if mode == "new-document" {
            a.execute("file.new", json!({})).unwrap();
        }
        tx.send(Ok(match mode {
            "cancel" => None,
            "same-path" => Some(d.file("original.docx").display().to_string()),
            _ => Some(copy.display().to_string()),
        }
        .into_iter()
        .collect()))
            .unwrap();
        let result = a.poll_file_dialog().unwrap();
        if mode == "copy" {
            result.unwrap();
            assert!(!a.session.dirty);
            assert_eq!(a.session.path.as_ref(), Some(&copy));
            assert_eq!(io::open_path(&copy).unwrap().plain_text(wordcraft_doc::StoryRef::Body), "Local edit");
        } else {
            assert!(!copy.exists());
            if mode == "cancel" {
                result.unwrap();
                assert_local(&a, &d, &b);
            } else {
                assert!(result.is_err());
            }
            if mode == "same-path" {
                assert_local(&a, &d, &b);
                assert!(matches!(a.dialog, Some(dialogs::Dialog::SaveConflict { .. })));
            }
        }
        assert_eq!(std::fs::read(d.file("original.docx")).unwrap(), b);
    }
}
#[test]
fn autosave_pauses_after_conflict_and_pending_close_does_not_discard_edits() {
    let d = Scratch::new();
    let mut a = app(&d);
    a.execute("file.save", json!({})).unwrap();
    a.execute("document.setText", json!({"text": "Local edit"})).unwrap();
    let b = external(&d);
    a.autosave_tick(now_ms() + 10_000.0);
    a.finish_autosave_job(true);
    assert!(!a.autosaves());
    assert!(matches!(a.dialog, Some(dialogs::Dialog::SaveConflict { .. })));
    assert_local(&a, &d, &b);
    a.run("ui.saveConflict", json!({"answer": "cancel"})).unwrap();
    a.autosave_tick(now_ms() + 20_000.0);
    a.finish_autosave_job(true);
    assert!(a.dialog.is_none());
    assert_eq!(a.run("file.close", json!({})).unwrap()["pending"], "saveChanges");
    let r = a.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
    assert_eq!(r["done"], false);
    assert!(matches!(a.dialog, Some(dialogs::Dialog::SaveConflict { .. })));
    assert!(!a.quit_requested);
    assert_local(&a, &d, &b);
}

/// An AutoSave whose write fails (here the file was changed by another program) puts the save
/// stamp back (revision, modified, last modified by), as a failed Save does (06: it kept the
/// stamp, so every failed attempt advanced the revision); the next save advances it once.
#[test]
fn a_failed_autosave_puts_the_save_stamp_back() {
    let d = Scratch::new();
    let mut a = app(&d);
    a.execute("file.save", json!({})).unwrap();
    let saved = a.session.doc.core.clone();
    a.execute("document.setText", json!({"text": "Local edit"})).unwrap();
    external(&d);
    a.autosave_tick(now_ms() + 10_000.0);
    a.finish_autosave_job(true);
    assert!(a.session.dirty && !a.autosaves(), "the write was refused");
    assert_eq!(a.session.doc.core, saved, "the stamp is back");
    a.run("ui.saveConflict", json!({"answer": "cancel"})).unwrap();
    a.execute("file.save", json!({"path": d.file("copy.docx")})).unwrap();
    assert_eq!(a.session.doc.core.revision, saved.revision + 1, "one save, one revision");
}

/// Upstream 6b66f88f, AutoSave's side: `stamp_save` also writes the window's zoom into the
/// document (the zoom the file remembers). A failed AutoSave put the core stamp back but kept that
/// zoom, so the document changed, and the next save wrote it, for a write that never happened.
#[test]
fn a_failed_autosave_puts_the_saved_zoom_back_too() {
    let d = Scratch::new();
    let mut a = app(&d);
    a.execute("file.save", json!({})).unwrap();
    let zoom = a.session.doc.settings.zoom;
    a.execute("view.zoom", json!({"value": 150})).unwrap();
    a.execute("document.setText", json!({"text": "Local edit"})).unwrap();
    external(&d);
    a.autosave_tick(now_ms() + 10_000.0);
    a.finish_autosave_job(true);
    assert!(a.session.dirty && !a.autosaves(), "the write was refused");
    assert_eq!(a.session.doc.settings.zoom, zoom, "the zoom the file remembers is back");
    a.run("ui.saveConflict", json!({"answer": "cancel"})).unwrap();
    a.execute("file.save", json!({"path": d.file("copy.docx")})).unwrap();
    assert_eq!(a.session.doc.settings.zoom.map(|z| z.percent), Some(150), "a save that works stamps it");
}

#[test]
fn dialog_buttons_keep_edits_and_render_selected_direction() {
    use egui_kittest::kittest::Queryable;
    let d = Scratch::new();
    let mut a = app(&d);
    let b = external(&d);
    a.run("file.save", json!({})).unwrap_err();
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
    h.get_by_label("Keep editing").click();
    h.run();
    assert!(h.state().dialog.is_none());
    assert_local(h.state(), &d, &b);
    h.state_mut().run("file.save", json!({})).unwrap_err();
    h.run();
    h.get_by_label("Save copy…").click();
    h.run();
    // This harness has no file picker, so Save Copy is cancelled without writing.
    assert!(h.state().dialog.is_none());
    assert_local(h.state(), &d, &b);
}

#[test]
fn save_copy_name_has_no_trailing_dot_without_an_extension() {
    assert_eq!(copy_name("Report.docx"), "Report copy.docx");
    assert_eq!(copy_name("Report"), "Report copy");
    assert_eq!(copy_name("Report."), "Report copy");
}

fn set_read_only(path: &std::path::Path, on: bool) {
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(on);
    std::fs::set_permissions(path, perms).unwrap();
}

/// A read-only file was silently overwritten by Save and AutoSave. Like Word, the save is refused:
/// the person gets the refused-save dialog saying the file is read-only (Save copy… is offered),
/// a script gets the error, and AutoSave stops without writing.
#[test]
fn a_read_only_file_is_refused_with_the_read_only_dialog_and_autosave_leaves_it() {
    let d = Scratch::new();
    let mut a = app(&d);
    let p = d.file("original.docx");
    a.execute("file.save", json!({})).unwrap();
    assert!(a.autosaves());
    let saved = std::fs::read(&p).unwrap();
    set_read_only(&p, true);
    // AutoSave: refused, stops, and shows the dialog.
    a.execute("document.setText", json!({"text": "Local edit"})).unwrap();
    a.autosave_tick(now_ms() + 10_000.0);
    a.finish_autosave_job(true);
    assert_eq!(std::fs::read(&p).unwrap(), saved);
    assert!(!a.autosaves());
    assert!(matches!(a.dialog, Some(dialogs::Dialog::SaveConflict { read_only: true, .. })), "{:?}", a.dialog.as_ref().map(|d| d.name()));
    a.run("ui.saveConflict", json!({"answer": "cancel"})).unwrap();
    // A script: an error that suggests Save As, no dialog.
    let e = a.execute("file.save", json!({})).unwrap_err();
    assert!(e.contains("read-only") && e.contains("Save As"), "{e}");
    assert!(a.dialog.is_none());
    // The person: the read-only dialog.
    assert!(a.run("file.save", json!({})).is_err());
    assert!(matches!(a.dialog, Some(dialogs::Dialog::SaveConflict { read_only: true, .. })));
    assert_eq!(std::fs::read(&p).unwrap(), saved);
    assert!(a.session.dirty);
    set_read_only(&p, false);
}
