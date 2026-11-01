//! Original synthetic save cases. Every test owns its directory and document copies.
#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use wordcraft_doc::{Document, StoryRef};
use wordcraft_engine::{Session, io};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!("wordcraft-safe-save-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(&self.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn original(path: &Path) -> Vec<u8> {
    let bytes = io::save_bytes("original.docx", &Document::from_text("Original synthetic contract.")).unwrap();
    std::fs::write(path, &bytes).unwrap();
    bytes
}
fn edited() -> Document {
    Document::from_text("Original synthetic contract. Edited sentinel.")
}
fn saved(path: &Path) {
    assert_eq!(io::open_path(path).unwrap().plain_text(StoryRef::Body), edited().plain_text(StoryRef::Body));
}

#[test]
fn an_existing_temporary_file_is_not_overwritten_or_removed() {
    let d = Scratch::new();
    let path = d.path("contract.docx");
    original(&path);
    let neighbour = d.path("contract.docx.tmp");
    std::fs::write(&neighbour, b"Keep this existing sibling.").unwrap();
    io::save_path(&path, &edited()).unwrap();
    saved(&path);
    assert_eq!(std::fs::read(&neighbour).unwrap(), b"Keep this existing sibling.");
    assert_eq!(d.names(), ["contract.docx", "contract.docx.tmp"]);
}

#[test]
fn an_existing_temporary_directory_does_not_block_a_save() {
    let d = Scratch::new();
    let path = d.path("contract.docx");
    original(&path);
    let neighbour = d.path("contract.docx.tmp");
    std::fs::create_dir(&neighbour).unwrap();
    std::fs::write(neighbour.join("keep"), b"Keep this directory.").unwrap();
    io::save_path(&path, &edited()).unwrap();
    saved(&path);
    assert_eq!(std::fs::read(neighbour.join("keep")).unwrap(), b"Keep this directory.");
}

#[cfg(unix)]
#[test]
fn temporary_links_cannot_redirect_a_save_into_the_original_or_a_neighbour() {
    for link_to_original in [false, true] {
        let d = Scratch::new();
        let path = d.path("contract.docx");
        original(&path);
        let other = d.path("neighbour.txt");
        std::fs::write(&other, b"Keep this neighbour.").unwrap();
        let link = d.path("contract.docx.tmp");
        let target = if link_to_original { &path } else { &other };
        std::os::unix::fs::symlink(target, &link).unwrap();
        io::save_path(&path, &edited()).unwrap();
        assert!(!path.is_symlink());
        saved(&path);
        assert_eq!(std::fs::read_link(&link).unwrap(), *target);
        assert_eq!(std::fs::read(&other).unwrap(), b"Keep this neighbour.");
    }
}

#[test]
fn failed_save_as_keeps_original_dirty_session_and_destination() {
    let d = Scratch::new();
    let path = d.path("original.docx");
    let bytes = original(&path);
    let blocked = d.path("blocked.docx");
    std::fs::create_dir(&blocked).unwrap();
    std::fs::write(blocked.join("keep"), b"Keep this destination.").unwrap();
    let mut s = Session::new(edited());
    s.path = Some(path.clone());
    s.dirty = true;
    let before = s.doc.plain_text(StoryRef::Body);
    assert!(s.run("file.saveAs", &json!({"path": blocked})).is_err());
    assert!(s.dirty);
    assert_eq!(s.path.as_ref(), Some(&path));
    assert_eq!(s.doc.plain_text(StoryRef::Body), before);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(std::fs::read(blocked.join("keep")).unwrap(), b"Keep this destination.");
    assert_eq!(d.names(), ["blocked.docx", "original.docx"]);
}

#[test]
fn successful_save_as_keeps_original_and_updates_session_only_after_saving() {
    let d = Scratch::new();
    let path = d.path("original.docx");
    let bytes = original(&path);
    let copy = d.path("copy.docx");
    let mut s = Session::new(edited());
    s.path = Some(path.clone());
    s.dirty = true;
    s.run("file.saveAs", &json!({"path": copy})).unwrap();
    assert!(!s.dirty);
    assert_eq!(s.path.as_ref(), Some(&copy));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    saved(&copy);
    assert_eq!(d.names(), ["copy.docx", "original.docx"]);
}

#[test]
fn missing_parent_keeps_original_and_session_unchanged() {
    let d = Scratch::new();
    let path = d.path("original.docx");
    let bytes = original(&path);
    let mut s = Session::new(edited());
    s.path = Some(path.clone());
    s.dirty = true;
    assert!(s.run("file.saveAs", &json!({"path": d.path("missing/copy.docx")})).is_err());
    assert!(s.dirty);
    assert_eq!(s.path, Some(path.clone()));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    assert_eq!(d.names(), ["original.docx"]);
}

/// A save with a password that fails to write keeps neither the password nor the save stamp:
/// the next plain Save writes an unencrypted file, stamped once.
#[test]
fn failed_save_with_password_keeps_neither_password_nor_stamp() {
    let d = Scratch::new();
    let blocked = d.path("blocked.docx");
    std::fs::create_dir(&blocked).unwrap();
    let mut s = Session::new(edited());
    let core = s.doc.core.clone();
    assert!(s.run("file.save", &json!({"path": blocked, "password": "mistyped-password"})).is_err());
    assert!(s.password.is_none());
    assert_eq!(s.doc.core, core);
    let path = d.path("contract.docx");
    s.run("file.save", &json!({"path": path})).unwrap();
    assert!(s.password.is_none());
    let (doc, encrypted) = io::open_path_with(&path, None).unwrap();
    assert!(!encrypted);
    assert_eq!(doc.core.revision, core.revision + 1);
    // A save that succeeds keeps its password for the next one.
    s.run("file.save", &json!({"password": "synthetic-test-password"})).unwrap();
    assert!(s.password.is_some());
    s.run("file.save", &json!({})).unwrap();
    assert!(io::needs_password(&io::open_path(&path).unwrap_err()));
}

/// Upstream 6b66f88f: `stamp_save` also writes the window's zoom into the document's settings
/// (the zoom the file remembers). A failed write put the core stamps back but kept that zoom, so
/// the document changed (and a later save persisted it) for a save that never happened.
#[test]
fn failed_save_leaves_the_saved_zoom_untouched_too() {
    let d = Scratch::new();
    let blocked = d.path("blocked.docx");
    std::fs::create_dir(&blocked).unwrap();
    let mut s = Session::new(edited());
    s.run("view.zoom", &json!({"value": 150})).unwrap();
    let zoom = s.doc.settings.zoom;
    assert_eq!(zoom, None, "a new document remembers no zoom yet");
    let core = s.doc.core.clone();
    assert!(s.run("file.save", &json!({"path": blocked})).is_err());
    assert_eq!(s.doc.core, core, "modified/created/author/revision are unchanged");
    assert_eq!(s.doc.settings.zoom, zoom, "the zoom the file remembers is unchanged");
    assert!(s.path.is_none(), "the failed save names no file");
    // The save that works stamps the zoom once.
    let path = d.path("contract.docx");
    s.run("file.save", &json!({"path": path})).unwrap();
    assert_eq!(s.doc.settings.zoom.map(|z| z.percent), Some(150));
}

#[test]
fn encrypted_save_reopens_only_with_password_and_leaves_neighbours_alone() {
    let d = Scratch::new();
    let path = d.path("contract.docx");
    original(&path);
    let neighbour = d.path("contract.docx.tmp");
    std::fs::write(&neighbour, b"Keep this sibling.").unwrap();
    io::save_path_with(&path, &edited(), Some("synthetic-test-password"), &io::ExportOptions::default()).unwrap();
    assert!(io::needs_password(&io::open_path(&path).unwrap_err()));
    let (doc, encrypted) = io::open_path_with(&path, Some("synthetic-test-password")).unwrap();
    assert!(encrypted);
    assert_eq!(doc.plain_text(StoryRef::Body), edited().plain_text(StoryRef::Body));
    assert_eq!(std::fs::read(&neighbour).unwrap(), b"Keep this sibling.");
    assert_eq!(d.names(), ["contract.docx", "contract.docx.tmp"]);
}

#[cfg(unix)]
#[test]
fn replacement_keeps_existing_unix_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let d = Scratch::new();
    let path = d.path("contract.docx");
    original(&path);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    io::save_path(&path, &edited()).unwrap();
    saved(&path);
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
}

/// A device that never ends, a named pipe nobody writes to, a directory and an oversized file are
/// all refused by every command that reads a path — without hanging or reading them whole.
#[cfg(unix)]
#[test]
fn special_and_oversized_files_are_refused_without_reading_them() {
    let dir = Scratch::new();
    let fifo = dir.path("pipe.docx");
    let made_fifo = std::process::Command::new("mkfifo").arg(&fifo).status().is_ok_and(|s| s.success());
    let big = dir.path("big.png");
    // Sparse: larger than the read cap without using the disk.
    std::fs::File::create(&big).unwrap().set_len(io::MAX_READ_BYTES + 1).unwrap();
    let mut paths = vec![PathBuf::from("/dev/zero"), dir.0.clone(), big.clone()];
    if made_fifo {
        paths.push(fifo);
    }
    for path in &paths {
        let p = path.to_string_lossy().into_owned();
        assert!(io::read_file(&p).is_err(), "{p}");
        assert!(io::open_path(path).is_err(), "{p}");
        let mut s = Session::new(Document::from_text("Keep."));
        for (id, params) in [("file.open", json!({"path": p})), ("insert.textFromFile", json!({"path": p})), ("insert.picture", json!({"path": p}))] {
            assert!(s.run(id, &params).is_err(), "{id} {p}");
        }
        assert_eq!(s.doc.plain_text(StoryRef::Body), "Keep.");
    }
    assert!(io::read_file(&big.to_string_lossy()).unwrap_err().contains("larger than"));
}

/// #3666 (upstream 4095cc34): Save decides whether the file it saves over is a Word 2003 XML
/// document by reading its head, which opened the path with no type check: a named pipe nobody
/// writes to blocked `open` and hung Save. The head read refuses anything but a regular file.
#[cfg(unix)]
#[test]
fn saving_over_a_named_pipe_does_not_hang() {
    let dir = Scratch::new();
    let fifo = dir.path("pipe.xml");
    if !std::process::Command::new("mkfifo").arg(&fifo).status().is_ok_and(|s| s.success()) {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let path = fifo.clone();
    std::thread::spawn(move || {
        let head = io::read_file_head(&path.to_string_lossy(), 64 * 1024);
        let mut s = Session::new(Document::from_text("Keep."));
        let saved = s.run("file.save", &json!({"path": path.to_string_lossy()}));
        let _ = tx.send((head, saved.map(|_| ()), s.path.is_none()));
    });
    let (head, saved, unnamed) =
        rx.recv_timeout(std::time::Duration::from_secs(10)).expect("the head read and the save returned (a pipe would block)");
    assert!(head.unwrap_err().contains("not a file"));
    assert!(saved.is_err(), "nothing is saved over a pipe");
    assert!(unnamed, "the pipe never becomes the document's file");
    assert_eq!(dir.names(), ["pipe.xml"]);
}

/// Exports (PNG, print to PDF) replace the target the way a save does: a new file renamed over
/// the old one (a hard link to the old file keeps the old bytes, where an in-place write would
/// have truncated both), with no temporary file left behind.
#[cfg(unix)]
#[test]
fn write_file_replaces_by_rename_and_leaves_no_temporary() {
    let dir = Scratch::new();
    let path = dir.path("page.png");
    let link = dir.path("link.png");
    std::fs::write(&path, b"old bytes that are longer").unwrap();
    std::fs::hard_link(&path, &link).unwrap();
    io::write_file(&path.to_string_lossy(), b"new").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"new");
    assert_eq!(std::fs::read(&link).unwrap(), b"old bytes that are longer");
    assert_eq!(dir.names(), vec!["link.png".to_string(), "page.png".to_string()]);
    assert!(io::write_file(&dir.path("missing/page.png").to_string_lossy(), b"x").is_err());
    assert_eq!(dir.names(), vec!["link.png".to_string(), "page.png".to_string()]);
}
