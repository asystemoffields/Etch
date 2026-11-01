//! A confined control connection (`docs/control-protocol.md`, Confinement) reaches only the files
//! inside its roots, for every command the app has: the engine's registry and the app's own `ui.*`
//! commands and control methods, found in the source so new ones are covered without a list.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use wordcraft_engine::Session;
use wordcraft_engine::file_access::{self, AutomationRoots};

use crate::{ControlRequest, WordApp};

const SENTINEL: &str = "OUTSIDE-SENTINEL-7f3a";

/// `in` (the connection's read and write root) and `outside`, which holds a document, a text
/// file, a CSV file and a picture.
struct Folders {
    base: PathBuf,
    roots: Arc<AutomationRoots>,
}

impl Folders {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!("wc-control-confine-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for d in ["in", "outside"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        let base = std::fs::canonicalize(&base).unwrap();
        for dir in ["in", "outside"] {
            let at = |f: &str| base.join(dir).join(f).to_string_lossy().into_owned();
            Session::new(wordcraft_doc::Document::from_text(SENTINEL)).run("file.save", &json!({"path": at("secret.docx")})).unwrap();
            std::fs::write(at("secret.txt"), SENTINEL).unwrap();
            std::fs::write(at("secret.csv"), format!("Name,City\n{SENTINEL},Leeds\n")).unwrap();
            std::fs::write(at("secret.png"), wordcraft_render::Rendered { width: 2, height: 2, pixels: vec![120; 16] }.to_png().unwrap()).unwrap();
        }
        let roots = AutomationRoots::narrowed(None, Some(&base.join("in")), Some(&base.join("in"))).unwrap();
        Self { base, roots }
    }

    fn path(&self, rel: &str) -> String {
        self.base.join(rel).to_string_lossy().into_owned()
    }

    /// Every file under `outside`, with its size and modification time.
    fn outside_snapshot(&self) -> BTreeMap<PathBuf, (u64, std::time::SystemTime)> {
        fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, (u64, std::time::SystemTime)>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                let m = std::fs::symlink_metadata(&p).unwrap();
                if m.is_dir() {
                    walk(&p, out);
                } else {
                    out.insert(p, (m.len(), m.modified().unwrap()));
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.base.join("outside"), &mut out);
        out
    }
}

impl Drop for Folders {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn app() -> WordApp {
    WordApp::new(Session::new(wordcraft_engine::sample::sample_document()), crate::Services::default())
}

/// Send one request through the path the control server's requests take, confined to `roots`,
/// and return its reply (`None` for a screenshot waiting for a frame).
fn control(app: &mut WordApp, roots: Option<&Arc<AutomationRoots>>, method: &str, params: Value) -> Option<Value> {
    let (req, rx) = ControlRequest::new(method, params);
    app.handle_control(&egui::Context::default(), req.confined(roots.cloned()));
    rx.try_recv().ok()
}

fn exec(app: &mut WordApp, roots: Option<&Arc<AutomationRoots>>, id: &str, params: Value) -> Value {
    control(app, roots, "engine.execute", json!({"command": id, "params": params})).unwrap_or(Value::Null)
}

/// The parameter names a command's documentation mentions (`"path": string, "original"?: …`).
fn param_names(doc: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut rest = doc;
    while let Some(i) = rest.find('"') {
        rest = &rest[i + 1..];
        let Some(j) = rest.find('"') else { break };
        let (name, after) = (&rest[..j], &rest[j + 1..]);
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && (after.starts_with(':') || after.starts_with("?:")) {
            names.insert(name.to_string());
        }
        rest = after;
    }
    names
}

/// Parameter names that may name a file.
fn path_like(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["path", "file", "original", "template", "source", "dir", "folder", "dest", "output", "attach"].iter().any(|w| n.contains(w))
}

/// The app's own command ids and control methods (`"ui.…"`, `"app.…"` string literals in this
/// crate's source).
fn ui_ids() -> BTreeSet<String> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut ids = BTreeSet::new();
    let mut files = vec![src];
    while let Some(dir) = files.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                files.push(p);
                continue;
            }
            if p.extension().is_none_or(|x| x != "rs") || p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("tests")) {
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap();
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            for part in code.split('"').skip(1).step_by(2) {
                let id_like = part.split('.').count() >= 2 && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '.');
                if id_like && (part.starts_with("ui.") || part.starts_with("app.")) {
                    ids.insert(part.to_string());
                }
            }
        }
    }
    ids
}

/// Every command, with every parameter that may name a file pointing outside the connection's
/// roots (one at a time, and all at once), through a confined control connection: each check
/// the engine makes is refused, nothing outside is written or changed, and nothing read from
/// there reaches the document. Commands that turn out to reach for a file are collected; the
/// ones known to must be among them, so the parameters really got that far.
#[test]
fn a_confined_connection_reaches_no_file_outside_its_roots_through_any_command() {
    let f = Folders::new("every");
    let before = f.outside_snapshot();
    let outside_values: Vec<String> =
        ["secret.docx", "secret.txt", "secret.csv", "secret.png", "new.docx", "new.png", "new.pdf", "sub/new.docx", "../outside/new.txt"]
            .iter()
            .map(|n| f.path(&format!("outside/{n}")))
            .collect();
    let registry = app().session.registry.clone();
    let mut commands: Vec<(String, BTreeSet<String>)> =
        registry.all().iter().map(|c| (c.id.to_string(), param_names(c.params).into_iter().filter(|n| path_like(n)).collect())).collect();
    for id in ui_ids() {
        // Zotero's commands talk to a local Zotero over a socket, not to files.
        if !id.starts_with("ui.zotero") && !commands.iter().any(|(c, _)| *c == id) {
            commands.push((id, BTreeSet::new()));
        }
    }
    assert!(commands.len() > 300, "{}", commands.len());
    let mut reached: BTreeSet<String> = BTreeSet::new();
    let mut escapes = vec![];
    for (id, mut keys) in commands {
        // Not every parameter list is documented: the common names are always tried.
        keys.extend(["path", "original", "file"].map(String::from));
        let mut variants: Vec<Value> = vec![];
        for value in &outside_values {
            for key in &keys {
                variants.push(json!({ key.as_str(): value }));
            }
            variants.push(Value::Object(keys.iter().map(|k| (k.clone(), json!(value))).collect()));
        }
        let mut a = app();
        for params in variants {
            let (reply, seen) = file_access::audited(|| {
                // Both ways a client names a command: `engine.execute` and the bare method.
                let r = exec(&mut a, Some(&f.roots), &id, params.clone());
                (r, control(&mut a, Some(&f.roots), &id, params.clone()))
            });
            for c in &seen {
                if c.allowed && f.roots.check(&c.path, c.access).is_err() {
                    escapes.push(format!("{id} {params}: {:?} {} was allowed", c.access, c.path));
                }
                if !c.allowed {
                    reached.insert(id.clone());
                }
            }
            let _ = reply;
        }
        let body = a.session.doc.plain_text(wordcraft_doc::StoryRef::Body);
        assert!(!body.contains(SENTINEL), "{id}: text from outside the read root reached the document");
        let after = f.outside_snapshot();
        assert_eq!(before, after, "{id}: a file outside the write root was written");
    }
    assert!(escapes.is_empty(), "{escapes:#?}");
    let known = [
        "file.open",
        "file.save",
        "file.saveAs",
        "file.exportPdf",
        "file.exportPng",
        "file.printPdf",
        "file.saveTemplate",
        "file.newFromTemplate",
        "insert.picture",
        "insert.textFromFile",
        "insert.object",
        "review.compare",
        "styles.organizer",
        "tools.updateStyles",
        "review.combine",
        "font.add",
        "ui.screenshot",
        "ui.render",
    ];
    let missing: Vec<&str> = known.iter().copied().filter(|k| !reached.contains(*k)).collect();
    assert!(missing.is_empty(), "these never reached the roots check: {missing:?}; reached: {reached:?}");
}

/// The same connection may use files inside its roots, and an unconfined one (or the person, in
/// the window) is not held to them.
#[test]
fn inside_the_roots_works_and_unconfined_is_unchanged() {
    let f = Folders::new("inside");
    let mut a = app();
    let r = Some(&f.roots);
    for (id, params) in [
        ("file.saveAs", json!({"path": f.path("in/copy.docx")})),
        ("file.exportPdf", json!({"path": f.path("in/copy.pdf")})),
        ("file.open", json!({"path": f.path("in/secret.docx")})),
        ("insert.picture", json!({"path": f.path("in/secret.png")})),
        ("insert.textFromFile", json!({"path": f.path("in/secret.txt")})),
    ] {
        let v = exec(&mut a, r, id, params.clone());
        assert_eq!(v["ok"], true, "{id} {params}: {v}");
    }
    assert!(Path::new(&f.path("in/copy.docx")).is_file() && Path::new(&f.path("in/copy.pdf")).is_file());
    let v = control(&mut a, r, "ui.render", json!({"path": f.path("in/page.png")})).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    // Commands that reach for a file only once something else is in place (the enumeration above
    // gives them only a path): a copy naming its styles.
    exec(&mut a, r, "file.new", json!({}));
    let v = exec(&mut a, r, "styles.copyFrom", json!({"path": f.path("outside/secret.docx"), "styles": ["Normal"]}));
    assert!(v["error"].as_str().unwrap().contains("outside the read root"), "{v}");
    assert_eq!(exec(&mut a, r, "styles.copyFrom", json!({"path": f.path("in/secret.docx"), "styles": ["Normal"]}))["ok"], true);
    // Outside, refused with the reason.
    let v = exec(&mut a, r, "file.saveAs", json!({"path": f.path("outside/x.docx")}));
    assert!(v["error"].as_str().unwrap().contains("automation path rejected: outside the write root"), "{v}");
    let v = control(&mut a, r, "ui.screenshot", json!({"path": f.path("outside/shot.png")})).unwrap();
    assert!(v["error"].as_str().unwrap().contains("outside the write root"), "{v}");
    // Unconfined: as before.
    let v = exec(&mut a, None, "file.open", json!({"path": f.path("outside/secret.docx")}));
    assert_eq!(v["ok"], true, "{v}");
    assert!(a.session.doc.plain_text(wordcraft_doc::StoryRef::Body).contains(SENTINEL));
    let v = exec(&mut a, None, "file.saveAs", json!({"path": f.path("outside/unconfined.docx")}));
    assert_eq!(v["ok"], true, "{v}");
    // Naming a file outside the write root for a document opened from bytes is refused too: a
    // later Save (by the person, or AutoSave) would write there.
    let data = wordcraft_engine::cmd::insert::base64_encode(SENTINEL.as_bytes());
    let v = exec(&mut a, r, "file.open", json!({"path": f.path("outside/named.txt"), "data": data}));
    assert!(v["error"].as_str().unwrap().contains("outside the write root"), "{v}");
    assert_eq!(exec(&mut a, r, "file.open", json!({"path": f.path("in/named.txt"), "data": data}))["ok"], true);
}

/// A link inside the root that leads out is followed before comparing: refused for reading and
/// writing, for files and folders.
#[cfg(unix)]
#[test]
fn a_link_out_of_the_roots_is_refused() {
    let f = Folders::new("links");
    std::os::unix::fs::symlink(f.base.join("outside"), f.base.join("in/escape")).unwrap();
    std::os::unix::fs::symlink(f.base.join("outside/secret.docx"), f.base.join("in/leak.docx")).unwrap();
    std::os::unix::fs::symlink(f.base.join("outside/gone.docx"), f.base.join("in/dangling.docx")).unwrap();
    let before = f.outside_snapshot();
    let mut a = app();
    let r = Some(&f.roots);
    for (id, path) in [
        ("file.open", "in/leak.docx"),
        ("file.open", "in/escape/secret.docx"),
        ("insert.textFromFile", "in/escape/secret.txt"),
        ("file.saveAs", "in/escape/new.docx"),
        ("file.saveAs", "in/leak.docx"),
        ("file.saveAs", "in/dangling.docx"),
        ("file.exportPdf", "in/escape/x.pdf"),
        ("file.saveAs", "in/escape/../escape/y.docx"),
    ] {
        let v = exec(&mut a, r, id, json!({"path": f.path(path)}));
        assert_eq!(v["ok"], false, "{id} {path}: {v}");
        assert!(v["error"].as_str().unwrap().contains("automation path rejected"), "{id} {path}: {v}");
    }
    assert!(!a.session.doc.plain_text(wordcraft_doc::StoryRef::Body).contains(SENTINEL));
    assert_eq!(before, f.outside_snapshot());
}

/// Clicks and keys a confined connection queues play inside its roots: what they set off in the
/// window (here a file the open dialog hands back while they play) is held to the roots, as
/// is the built-in file browser's listing. Once they have played, the window is the person's
/// again.
#[test]
fn synthetic_input_from_a_confined_connection_plays_inside_its_roots() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc::{Sender, channel};
    type Shown = Rc<RefCell<Vec<Sender<crate::FileDialogAnswer>>>>;
    let f = Folders::new("input");
    let shown: Shown = Rc::default();
    let log = shown.clone();
    let services = crate::Services {
        file_dialog: Some(Box::new(move |_req| {
            let (tx, rx) = channel();
            log.borrow_mut().push(tx);
            rx
        })),
        ..Default::default()
    };
    let mut a = WordApp::new(Session::new(wordcraft_doc::Document::new()), services);
    let ctx = egui::Context::default();
    let frame = |a: &mut WordApp| {
        let mut raw = egui::RawInput::default();
        a.raw_input_hook(&mut raw);
        ctx.run_ui(raw, |ui| {
            a.logic(ui.ctx());
            a.ui(ui);
        })
        .drop_without_applying_deltas();
    };
    let pick = |path: String| {
        let tx = shown.borrow_mut().pop().unwrap();
        tx.send(Ok(vec![path])).unwrap();
    };
    let secret = f.path("outside/secret.docx");
    let body = |a: &WordApp| a.session.doc.plain_text(wordcraft_doc::StoryRef::Body);

    // An unconfined connection's input leaves the window unconfined.
    control(&mut a, None, "ui.key", json!({"key": "Escape"}));
    assert!(a.input_roots.is_none());
    // A confined one's: the dialog's answer arrives while it plays, and is refused.
    a.open_dialog();
    control(&mut a, Some(&f.roots), "ui.key", json!({"key": "Escape"}));
    assert!(a.input_roots.is_some(), "the confined connection's input is pending");
    pick(secret.clone());
    frame(&mut a);
    assert!(!body(&a).contains(SENTINEL), "a file outside the roots opened while confined input played");
    let listed = |a: &WordApp| {
        let roots = a.input_roots.clone();
        file_access::confine(roots.as_ref(), || crate::file_browser::list_dir(&f.base.join("outside"), &["*".to_string()], 100).is_ok())
    };
    assert!(!listed(&a), "the built-in browser lists no folder outside while it plays");
    // The input has played: a few frames later the window is the person's again.
    let mut frames = 1;
    while a.input_roots.is_some() && frames < 20 {
        frame(&mut a);
        frames += 1;
    }
    assert!(a.input_roots.is_none() && frames <= 1 + usize::from(crate::INPUT_ROOTS_FRAMES), "{frames} frames");
    assert!(listed(&a));
    a.open_dialog();
    pick(secret);
    frame(&mut a);
    assert!(body(&a).contains(SENTINEL), "the person opens any file");
}

/// The app's own code reads and writes files through the engine's `io`, where the roots are
/// checked; nothing else here touches the file system except the built-in file browser, which
/// checks the roots before it lists a folder, and a size check on pasted picture files (whose
/// bytes `insert.picture` then reads through `io`). A new direct use must be checked and added
/// here.
#[test]
fn the_app_touches_files_only_through_io() {
    let allowed = [
        ("file_browser.rs", "std::fs::read_dir(dir)"),
        ("file_browser.rs", "std::fs::metadata(&path).is_ok_and"),
        ("paste_picture.rs", "std::fs::metadata(&path).map(|m| m.len())"),
    ];
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = vec![];
    let mut dirs = vec![src];
    while let Some(dir) = dirs.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
                continue;
            }
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !name.ends_with(".rs") || name.starts_with("tests") {
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap();
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            for (n, line) in code.lines().enumerate() {
                let line = line.trim_start();
                let touches = ["std::fs", "File::open", "File::create", "OpenOptions"].iter().any(|w| line.contains(w));
                if touches && !line.starts_with("//") && !allowed.iter().any(|(f, what)| *f == name && line.contains(what)) {
                    offenders.push(format!("{}:{}: {line}", p.display(), n + 1));
                }
            }
        }
    }
    assert!(offenders.is_empty(), "use wordcraft_engine::io::{{read_file, write_file, …}}: {offenders:#?}");
}

/// Run one AutoSave tick now and wait for its write.
fn autosave_now(a: &mut WordApp) {
    a.last_autosave = 0.0;
    a.autosave_tick(crate::now_ms() + 1.0e9);
    a.finish_autosave_job(true);
}

/// Review finding: AutoSave wrote on a worker thread, where no roots are in force. A confined
/// connection refused `file.save` of a file the person had saved outside its roots could turn
/// AutoSave on (it is on by default anyway), insert text, and one tick wrote the file. AutoSave
/// now writes only where every confined connection that changed the document may write.
#[test]
fn autosave_does_not_write_a_confined_connections_edits_outside_its_roots() {
    let f = Folders::new("autosave");
    let r = Some(&f.roots);
    let secret = f.path("outside/secret.docx");
    let mut a = app();
    // The person saves the document outside the connection's roots: AutoSave covers it.
    a.run("file.saveAs", json!({"path": secret})).unwrap();
    assert!(a.autosaves());
    let before = std::fs::read(&secret).unwrap();
    // The connection can't save there, nor turn AutoSave on for it.
    let v = exec(&mut a, r, "file.save", json!({}));
    assert!(v["error"].as_str().unwrap().contains("outside the write root"), "{v}");
    let v = exec(&mut a, r, "file.autosave", json!({"value": true}));
    assert!(v["error"].as_str().unwrap().contains("outside the write root"), "{v}");
    // AutoSave is on all the same (the person's setting): the connection's edit isn't written.
    assert_eq!(exec(&mut a, r, "text.insert", json!({"text": "AGENT"}))["ok"], true);
    assert!(a.session.dirty);
    autosave_now(&mut a);
    assert_eq!(std::fs::read(&secret).unwrap(), before, "AutoSave wrote outside the connection's write root");
    assert!(a.session.dirty);
    assert!(a.status_msg.as_ref().is_some_and(|(m, _)| m.contains("outside the write root")), "{:?}", a.status_msg);
    // Once the person saves it themselves, AutoSave is theirs again.
    a.run("file.save", json!({})).unwrap();
    a.run("text.insert", json!({"text": " person"})).unwrap();
    autosave_now(&mut a);
    assert!(!a.session.dirty, "the person's own edit is AutoSaved");
    assert_ne!(std::fs::read(&secret).unwrap(), before);
}

/// The same connection's edits to a file inside its write root are AutoSaved as before.
#[test]
fn autosave_writes_a_confined_connections_edits_inside_its_roots() {
    let f = Folders::new("autosave-in");
    let r = Some(&f.roots);
    let mine = f.path("in/mine.docx");
    let mut a = app();
    a.run("file.saveAs", json!({"path": mine})).unwrap();
    let before = std::fs::read(&mine).unwrap();
    assert_eq!(exec(&mut a, r, "file.autosave", json!({"value": true}))["ok"], true);
    assert_eq!(exec(&mut a, r, "text.insert", json!({"text": "AGENT"}))["ok"], true);
    autosave_now(&mut a);
    assert!(!a.session.dirty);
    assert_ne!(std::fs::read(&mine).unwrap(), before);
}

/// Decision (review 8b): a confined connection never discards the person's unsaved work, not
/// with `force`, not by replacing the document, not by answering Don't Save.
#[test]
fn a_confined_connection_cannot_discard_unsaved_work() {
    let f = Folders::new("discard");
    let r = Some(&f.roots);
    let mut a = app();
    a.run("text.insert", json!({"text": "UNSAVED"})).unwrap();
    assert!(a.session.dirty);
    let v = control(&mut a, r, "app.quit", json!({"force": true})).unwrap();
    assert!(v["error"].as_str().unwrap().contains("confined"), "{v}");
    assert!(!a.quit_requested);
    for (id, params) in [
        ("file.close", json!({"force": true})),
        ("file.new", json!({})),
        ("file.open", json!({"path": f.path("in/secret.docx")})),
        ("mailings.labels", json!({"text": "x"})),
    ] {
        let v = exec(&mut a, r, id, params.clone());
        assert!(v["error"].as_str().is_some_and(|e| e.contains("confined")), "{id} {params}: {v}");
        let v = control(&mut a, r, id, params.clone()).unwrap();
        assert!(v["error"].as_str().is_some_and(|e| e.contains("confined")), "{id} {params}: {v}");
    }
    // The person's Save Changes prompt can't be answered Don't Save by the connection.
    a.run("file.new", json!({})).unwrap();
    assert!(matches!(a.dialog, Some(crate::dialogs::Dialog::SaveChanges { .. })));
    let v = exec(&mut a, r, "ui.saveChanges", json!({"answer": "dontSave"}));
    assert!(v["error"].as_str().unwrap().contains("confined"), "{v}");
    assert!(matches!(a.dialog, Some(crate::dialogs::Dialog::SaveChanges { .. })), "the prompt stays for the person");
    assert!(a.session.doc.plain_text(wordcraft_doc::StoryRef::Body).contains("UNSAVED"));
    // The person (unconfined) may discard; with nothing unsaved the connection may quit.
    a.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
    assert!(!a.session.doc.plain_text(wordcraft_doc::StoryRef::Body).contains("UNSAVED"));
    assert_eq!(control(&mut a, r, "app.quit", json!({})).unwrap()["ok"], true);
}
