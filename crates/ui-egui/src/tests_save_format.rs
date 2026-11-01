//! Saving in a format that can't keep everything (`save_format`), with synthetic files only.
#![allow(clippy::unwrap_used)]
use super::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use wordcraft_doc::Document;
use wordcraft_engine::io;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!("wordcraft-ui-format-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
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

/// An app with `name` (written in its format) open and an equation added, and the Save As names
/// the file picker was asked to suggest.
fn app_with_equation(d: &Scratch, name: &str) -> (WordApp, PathBuf, Rc<RefCell<Vec<String>>>) {
    let p = d.file(name);
    io::save_path(&p, &Document::from_text("Synthetic body text")).unwrap();
    let mut a = WordApp::new(Session::new(Document::new()), Services::default());
    let asked = Rc::new(RefCell::new(Vec::new()));
    let seen = asked.clone();
    a.services.pick_save = Some(Box::new(move |name| {
        seen.borrow_mut().push(name.to_string());
        None
    }));
    a.execute("file.open", json!({"path": p})).unwrap();
    // An equation: ODT and RTF write it as its text (they keep notes, comments and headers).
    a.execute("insert.equation", json!({"linear": "a^2+b^2=c^2", "edit": false})).unwrap();
    assert!(a.session.dirty);
    (a, p, asked)
}

/// Save of an .odt holding an equation asks first and names what ODT loses; Keep format saves and
/// the document counts as saved (as in Word); later saves don't ask again; AutoSave never writes
/// the file.
#[test]
fn saving_an_odt_with_an_equation_asks_first_and_autosave_never_writes_it() {
    let d = Scratch::new();
    let (mut a, p, _) = app_with_equation(&d, "letter.odt");
    let before = std::fs::read(&p).unwrap();
    // AutoSave can't be on for it, and its timer writes nothing.
    assert!(matches!(a.autosave_block(), Some(AutoSaveBlock::Format { .. })));
    assert!(a.execute("file.autosave", json!({"value": true})).is_err());
    a.autosave = true;
    a.autosave_tick(now_ms() + 60_000.0);
    a.finish_autosave_job(true);
    assert_eq!(std::fs::read(&p).unwrap(), before, "AutoSave never writes a lossy format");

    assert_eq!(a.run("file.save", json!({})).unwrap()["pending"], "saveFormat");
    let Some(dialogs::Dialog::SaveFormat { lost, .. }) = &a.dialog else { panic!("no prompt: {:?}", a.dialog.as_ref().map(|d| d.name())) };
    assert!(lost.iter().any(|f| f == "equations"), "{lost:?}");
    assert_eq!(std::fs::read(&p).unwrap(), before, "nothing written before the answer");
    // Cancel: nothing saved.
    a.run("ui.saveFormat", json!({"answer": "cancel"})).unwrap();
    assert!(a.dialog.is_none() && a.session.dirty);
    assert_eq!(std::fs::read(&p).unwrap(), before);
    // Keep format: saved in its format, and the document is saved.
    a.run("file.save", json!({})).unwrap();
    a.run("ui.saveFormat", json!({"answer": "keep"})).unwrap();
    assert!(a.dialog.is_none());
    assert!(!a.session.dirty);
    assert_ne!(std::fs::read(&p).unwrap(), before);
    // The next save doesn't ask again for this document and file.
    a.execute("text.insert", json!({"text": " more"})).unwrap();
    let r = a.run("file.save", json!({})).unwrap();
    assert_eq!(r["saved"], true);
    assert!(a.dialog.is_none() && !a.session.dirty);
    // Still no AutoSave after the person's save.
    a.execute("text.insert", json!({"text": " and more"})).unwrap();
    let saved = std::fs::read(&p).unwrap();
    a.autosave_tick(now_ms() + 60_000.0);
    a.finish_autosave_job(true);
    assert_eq!(std::fs::read(&p).unwrap(), saved);
    assert!(a.session.dirty);
}

/// Save as .docx… opens Save As with a Word document suggested.
#[test]
fn save_as_docx_from_the_prompt_suggests_a_word_document() {
    let d = Scratch::new();
    let (mut a, p, asked) = app_with_equation(&d, "notes.rtf");
    let before = std::fs::read(&p).unwrap();
    assert_eq!(a.run("file.save", json!({})).unwrap()["pending"], "saveFormat");
    a.run("ui.saveFormat", json!({"answer": "docx"})).unwrap();
    assert_eq!(*asked.borrow(), ["notes.docx"]);
    assert_eq!(std::fs::read(&p).unwrap(), before, "the picker was cancelled: nothing written");
    assert!(a.session.dirty);
}

/// Scripts are never asked: `file.save` saves and reports what the format lost.
#[test]
fn scripts_save_lossy_formats_without_a_prompt_and_learn_what_was_lost() {
    let d = Scratch::new();
    let (mut a, p, _) = app_with_equation(&d, "letter.odt");
    let r = a.execute("file.save", json!({})).unwrap();
    assert!(a.dialog.is_none());
    assert_eq!(r["saved"], true);
    assert!(r["lossy"].as_array().unwrap().iter().any(|f| f == "equations"), "{r}");
    assert!(io::open_path(&p).is_ok());
    // A format that keeps everything reports no `lossy`.
    let docx = d.file("letter.docx");
    let r = a.execute("file.saveAs", json!({"path": docx})).unwrap();
    assert!(r.get("lossy").is_none(), "{r}");
}

/// The quit prompt's Save of an .odt asks about the format too, then carries on.
#[test]
fn the_quit_prompts_save_asks_about_the_format_then_carries_on() {
    let d = Scratch::new();
    let (mut a, p, _) = app_with_equation(&d, "letter.odt");
    assert_eq!(a.run("file.close", json!({"quit": true})).unwrap()["pending"], "saveChanges");
    assert_eq!(a.run("ui.saveChanges", json!({"answer": "save"})).unwrap()["pending"], "saveFormat");
    let r = a.run("ui.saveFormat", json!({"answer": "keep"})).unwrap();
    assert_eq!(r["done"], true, "{r}");
    assert!(a.quit_requested && !a.session.dirty, "closed after saving");
    assert!(io::open_path(&p).unwrap().plain_text(wordcraft_doc::StoryRef::Body).contains("Synthetic body text"));
}

/// Save of a Word 97-2003 document (which WordCraft opens but can't write) goes to Save As with
/// a .docx suggested, instead of failing; so does the quit prompt's Save. A script gets an error
/// that names the alternative.
#[test]
fn saving_a_doc_file_goes_to_save_as_docx() {
    let d = Scratch::new();
    let mut a = WordApp::new(Session::new(Document::new()), Services::default());
    let asked = Rc::new(RefCell::new(Vec::new()));
    let seen = asked.clone();
    a.services.pick_save = Some(Box::new(move |name| {
        seen.borrow_mut().push(name.to_string());
        None
    }));
    // A synthetic document whose file is a .doc (the session as `file.open` of one leaves it).
    a.execute("text.insert", json!({"text": "Synthetic old document"})).unwrap();
    a.session.path = Some(d.file("old.doc"));
    let e = a.execute("file.save", json!({})).unwrap_err();
    assert!(e.contains(".docx") && e.contains("file.saveAs"), "{e}");
    assert!(a.dialog.is_none());
    a.run("file.save", json!({})).unwrap();
    assert_eq!(*asked.borrow(), ["old.docx"]);
    assert!(!d.file("old.doc").exists());
    assert_eq!(a.run("file.close", json!({})).unwrap()["pending"], "saveChanges");
    a.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
    assert_eq!(asked.borrow().len(), 2, "the quit prompt's Save goes to Save As too");
    assert!(a.session.dirty, "nothing saved: the picker was cancelled");
}

/// A Word 2003 XML Document keeps less than its `.xml` name says (no equations, text boxes,
/// content controls or drawings): saving over one asks first, as any format that can't keep
/// everything does, and once the person keeps the format AutoSave doesn't write the file (it
/// wrote a Word XML Document over it before, changing its format behind the person's back).
#[test]
fn saving_over_a_word_2003_xml_file_asks_first_and_autosave_leaves_it() {
    let d = Scratch::new();
    let p = d.file("report.xml");
    let opts = io::ExportOptions { word2003: true, ..Default::default() };
    std::fs::write(&p, io::save_bytes_opts("report.xml", &Document::from_text("Synthetic body text"), &opts).unwrap()).unwrap();
    let mut a = WordApp::new(Session::new(Document::new()), Services::default());
    a.execute("file.open", json!({"path": p})).unwrap();
    a.execute("insert.equation", json!({"linear": "a^2+b^2=c^2", "edit": false})).unwrap();
    let before = std::fs::read(&p).unwrap();
    assert_eq!(a.run("file.save", json!({})).unwrap()["pending"], "saveFormat");
    let Some(dialogs::Dialog::SaveFormat { lost, .. }) = &a.dialog else { panic!("no prompt: {:?}", a.dialog.as_ref().map(|d| d.name())) };
    assert!(lost.iter().any(|f| f == "equations"), "{lost:?}");
    assert_eq!(std::fs::read(&p).unwrap(), before, "nothing written before the answer");
    a.run("ui.saveFormat", json!({"answer": "keep"})).unwrap();
    assert!(!a.session.dirty);
    let saved = std::fs::read(&p).unwrap();
    assert_ne!(saved, before);
    assert!(String::from_utf8_lossy(&saved).contains("w:wordDocument"), "still a Word 2003 XML Document");
    // AutoSave doesn't take over the file.
    assert!(matches!(a.autosave_block(), Some(AutoSaveBlock::Format { .. })), "{:?}", a.autosave_block().map(|b| b.reason()));
    a.execute("text.insert", json!({"text": " more"})).unwrap();
    a.autosave = true;
    a.autosave_tick(now_ms() + 60_000.0);
    a.finish_autosave_job(true);
    assert_eq!(std::fs::read(&p).unwrap(), saved, "AutoSave never writes it");
    assert!(a.session.dirty);
}

/// Every feature a save can report losing (`io::lossy_features`) is named in the prompt; fields
/// nested in other fields' codes were shown as "Formatting".
#[test]
fn every_feature_a_save_can_lose_is_named_in_the_prompt() {
    let ids = [
        "comments",
        "footnotes",
        "endnotes",
        "headersFooters",
        "trackedChanges",
        "textBoxes",
        "equations",
        "contentControls",
        "drawings",
        "nestedTables",
        "pictures",
        "nestedFields",
        "fields",
        "mergedCells",
        "sections",
        "pageSetup",
        "properties",
        "macros",
    ];
    let fallback = crate::save_format::feature_label("anything else");
    for id in ids {
        assert_ne!(crate::save_format::feature_label(id), fallback, "{id}");
    }
}

/// Keep format covers what the prompt named: once the document gains something more the format
/// can't keep (a content control added to an `.odt` kept with an equation), the next save asks
/// again, naming it, rather than dropping it without a word; nothing is written until then.
#[test]
fn a_save_that_would_lose_something_new_asks_again() {
    let d = Scratch::new();
    let (mut a, p, _) = app_with_equation(&d, "letter.odt");
    assert_eq!(a.run("file.save", json!({})).unwrap()["pending"], "saveFormat");
    a.run("ui.saveFormat", json!({"answer": "keep"})).unwrap();
    assert!(!a.session.dirty);
    // More of what was named: not asked again.
    a.execute("insert.equation", json!({"linear": "x+y", "edit": false})).unwrap();
    assert_eq!(a.run("file.save", json!({})).unwrap()["saved"], true);
    // Something new: asked again, and nothing written before the answer.
    a.execute("caret.docEnd", json!({})).unwrap();
    a.execute("developer.plainText", json!({"tag": "synthetic"})).unwrap();
    let before = std::fs::read(&p).unwrap();
    assert_eq!(a.run("file.save", json!({})).unwrap()["pending"], "saveFormat");
    let Some(dialogs::Dialog::SaveFormat { lost, .. }) = &a.dialog else { panic!("no prompt: {:?}", a.dialog.as_ref().map(|d| d.name())) };
    assert!(lost.iter().any(|f| f == "contentControls"), "{lost:?}");
    assert_eq!(std::fs::read(&p).unwrap(), before);
    assert!(a.session.dirty);
    a.run("ui.saveFormat", json!({"answer": "keep"})).unwrap();
    assert!(!a.session.dirty);
}

/// storytold/wordcraft#2376: a text file saved in its own format (Keep format) is saved: closing
/// asks nothing more, and the window closes. With unsaved changes, the prompt's Save saves in
/// the file's format and closes.
#[test]
fn a_text_file_saved_in_its_format_closes_without_asking_again() {
    let d = Scratch::new();
    let p = d.file("notes.txt");
    std::fs::write(&p, "Synthetic notes\n").unwrap();
    let mut a = WordApp::new(Session::new(Document::new()), Services::default());
    a.execute("file.open", json!({"path": p})).unwrap();
    a.execute("text.insert", json!({"text": "More "})).unwrap();
    assert_eq!(a.run("file.save", json!({})).unwrap()["pending"], "saveFormat");
    a.run("ui.saveFormat", json!({"answer": "keep"})).unwrap();
    assert!(!a.session.dirty);
    assert!(a.close_requested(), "a saved document closes without a prompt");
    assert!(a.dialog.is_none());
    // Unsaved again: the close prompt's Save writes the file and closes.
    let mut b = WordApp::new(Session::new(Document::new()), Services::default());
    b.execute("file.open", json!({"path": p})).unwrap();
    b.execute("text.insert", json!({"text": "Last "})).unwrap();
    assert!(!b.close_requested());
    let r = b.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
    let r =
        if r.get("pending").and_then(Value::as_str) == Some("saveFormat") { b.run("ui.saveFormat", json!({"answer": "keep"})).unwrap() } else { r };
    assert_eq!(r["done"], true, "{r}");
    assert!(b.quit_requested && !b.session.dirty);
    assert!(std::fs::read_to_string(&p).unwrap().starts_with("Last More "));
}

/// A stand-in for a compiled macro project.
const VBA: &[u8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 7, 7];

/// A synthetic document holding a macro project, as the reader keeps one from a `.docm`.
fn with_macros(text: &str) -> Document {
    let mut doc = Document::from_text(text);
    doc.passthrough.insert("word/vbaProject.bin".into(), std::sync::Arc::new(VBA.to_vec()));
    doc
}

fn has_macros(path: &std::path::Path) -> bool {
    io::open_path(path).unwrap().passthrough.get("word/vbaProject.bin").is_some_and(|b| b.as_slice() == VBA)
}

/// An app with `name` (a `.docm` or `.dotm` holding macros) open and edited; the Save As picker
/// answers with whatever `pick` holds, and the names it was asked to suggest are recorded.
fn app_with_macros(d: &Scratch, name: &str) -> (WordApp, PathBuf, Rc<RefCell<Option<PathBuf>>>, Rc<RefCell<Vec<String>>>) {
    let p = d.file(name);
    io::save_path(&p, &with_macros("Synthetic macro document")).unwrap();
    assert!(has_macros(&p));
    let mut a = WordApp::new(Session::new(Document::new()), Services::default());
    let pick: Rc<RefCell<Option<PathBuf>>> = Rc::default();
    let asked: Rc<RefCell<Vec<String>>> = Rc::default();
    let (answer, seen) = (pick.clone(), asked.clone());
    a.services.pick_save = Some(Box::new(move |name| {
        seen.borrow_mut().push(name.to_string());
        answer.borrow().as_ref().map(|p| p.to_string_lossy().into_owned())
    }));
    a.execute("file.open", json!({"path": p})).unwrap();
    a.execute("caret.docEnd", json!({})).unwrap();
    a.execute("text.insert", json!({"text": " More"})).unwrap();
    (a, p, pick, asked)
}

/// Save As .docx from a `.docm` (and .dotx from a `.dotm`) asks first, naming the macros, as
/// Word warns before saving a macro-free document (06, question 2: they were dropped without a
/// word). Nothing is written before the answer; the other button goes back to Save As with the
/// macro-enabled format suggested; Keep format writes the macro-free file, which becomes the
/// document's, and later saves of it don't ask again. AutoSave never writes it (it would leave the
/// macros out every time) and says why; the `.docm` keeps its macros.
#[test]
fn save_as_a_macro_free_word_document_asks_first() {
    for (source, target, offered) in [("m.docm", "m.docx", "m.docm"), ("t.dotm", "t.dotx", "t.dotm")] {
        let d = Scratch::new();
        let (mut a, src, pick, asked) = app_with_macros(&d, source);
        let to = d.file(target);
        *pick.borrow_mut() = Some(to.clone());
        // F12, then the picker answers with the macro-free file.
        a.run("file.saveAs", json!({})).unwrap();
        let Some(dialogs::Dialog::SaveFormat { lost, target: asked_for, .. }) = &a.dialog else {
            panic!("{target}: no prompt: {:?}", a.dialog.as_ref().map(|d| d.name()))
        };
        assert_eq!(lost, &vec!["macros".to_string()], "{target}");
        assert_eq!(asked_for.as_deref(), Some(to.to_string_lossy().as_ref()), "{target}");
        assert!(!to.exists(), "{target}: nothing written before the answer");
        assert!(a.session.dirty);
        // The other button: Save As again, a macro-enabled file suggested.
        *pick.borrow_mut() = None;
        a.run("ui.saveFormat", json!({"answer": "docx"})).unwrap();
        assert_eq!(asked.borrow().last().map(String::as_str), Some(offered), "{target}: {:?}", asked.borrow());
        assert!(!to.exists() && a.session.dirty, "{target}");
        // Save As .docx again, and Keep format.
        *pick.borrow_mut() = Some(to.clone());
        a.run("file.saveAs", json!({})).unwrap();
        assert!(matches!(a.dialog, Some(dialogs::Dialog::SaveFormat { .. })), "{target}");
        a.run("ui.saveFormat", json!({"answer": "keep"})).unwrap();
        assert!(to.exists() && !has_macros(&to), "{target}: written without the macros");
        assert_eq!(a.session.path.as_deref(), Some(to.as_path()), "{target}");
        assert!(!a.session.dirty, "{target}");
        assert!(has_macros(&src), "{source} keeps its macros");
        // AutoSave doesn't take the file over: it says why, and its timer writes nothing.
        let block = a.autosave_block();
        assert!(matches!(block, Some(AutoSaveBlock::Macros { .. })), "{target}: {:?}", block.as_ref().map(AutoSaveBlock::reason));
        assert!(a.execute("file.autosave", json!({"value": true})).is_err());
        let saved = std::fs::read(&to).unwrap();
        a.execute("text.insert", json!({"text": " later"})).unwrap();
        a.autosave = true;
        a.autosave_tick(now_ms() + 60_000.0);
        a.finish_autosave_job(true);
        assert_eq!(std::fs::read(&to).unwrap(), saved, "{target}: AutoSave never writes it");
        // The person's own Save of it: told once, not asked again.
        assert_eq!(a.run("file.save", json!({})).unwrap()["saved"], true, "{target}");
        assert!(!a.session.dirty);
    }
}

/// File › Export to a macro-free format, a Save Copy and the Save As of a Save Changes prompt
/// ask the same; the prompt's held-back command (New) runs once the file is written. A Save As
/// to a macro-enabled file or WordCraft's JSON doesn't ask.
#[test]
fn every_save_as_that_would_drop_the_macros_asks_first() {
    let d = Scratch::new();
    let (mut a, _, pick, _) = app_with_macros(&d, "m.docm");
    // Export › Change File Type: not written, and not reported as exported, until answered.
    let exported = d.file("export.docx");
    let r = a.finish_pick(crate::file_dialogs::AfterPick::Export { ext: "docx".into() }, Some(exported.to_string_lossy().into_owned())).unwrap();
    assert_eq!(r["pending"], "saveFormat", "{r}");
    assert!(!exported.exists());
    assert!(a.status_msg.as_ref().is_none_or(|(m, _)| !m.contains("Exported")), "{:?}", a.status_msg);
    a.run("ui.saveFormat", json!({"answer": "keep"})).unwrap();
    assert!(exported.exists() && !has_macros(&exported));
    // The save itself names them for scripts.
    let copy = d.file("copy.odt");
    let r = a.execute("file.saveAs", json!({"path": copy})).unwrap();
    assert!(r["lossy"].as_array().is_some_and(|l| l.iter().any(|f| f == "macros")), "{r}");
    // Formats that keep them: no prompt.
    for keeps in ["again.docm", "again.json"] {
        let p = d.file(keeps);
        assert_eq!(a.run("file.saveAs", json!({"path": p})).unwrap()["saved"], true, "{keeps}");
    }
    // A restored copy of a .docm (no file of its own) at New: Save Changes › Save › Save As .docx.
    let mut b = WordApp::new(Session::new(with_macros("Restored synthetic text")), Services::default());
    b.session.dirty = true;
    let to = d.file("restored.docx");
    let answer = to.to_string_lossy().into_owned();
    b.services.pick_save = Some(Box::new(move |_| Some(answer.clone())));
    let document = b.session.document_id();
    assert_eq!(b.run("file.new", json!({})).unwrap()["pending"], "saveChanges");
    assert_eq!(b.run("ui.saveChanges", json!({"answer": "save"})).unwrap()["pending"], "saveFormat");
    assert!(!to.exists() && b.session.document_id() == document, "nothing written or replaced before the answer");
    let r = b.run("ui.saveFormat", json!({"answer": "keep"})).unwrap();
    assert_eq!(r["done"], true, "{r}");
    assert!(to.exists() && !has_macros(&to));
    assert_ne!(b.session.document_id(), document, "New ran once the file was written");
    drop(pick);
}

/// In the browser a Save As downloads at once (there is no file to protect): a download that
/// leaves out the macros says so on the status bar and in `lossy`; the opened format keeps them.
#[test]
fn a_browser_download_without_the_macros_says_so() {
    let got: Rc<RefCell<Vec<(String, Vec<u8>)>>> = Rc::default();
    let sink = got.clone();
    let services = Services {
        download: Some(Box::new(move |n: &str, b: &[u8]| {
            sink.borrow_mut().push((n.to_string(), b.to_vec()));
            Ok(())
        })),
        ..Default::default()
    };
    let mut a = WordApp::new(Session::new(with_macros("Synthetic")), services);
    a.session.path = Some("m.docm".into());
    let r = a.run("file.save", json!({})).unwrap();
    assert!(r.get("lossy").is_none(), "{r}");
    let r = a.run("file.saveAs", json!({"path": "m.docx"})).unwrap();
    assert_eq!(r["lossy"], json!(["macros"]), "{r}");
    let (msg, _) = a.status_msg.clone().unwrap();
    assert!(msg.contains("m.docx") && msg.contains("macros"), "{msg}");
    assert_eq!(got.borrow().len(), 2);
}
