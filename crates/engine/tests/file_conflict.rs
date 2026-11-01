//! Synthetic external-writer cases; originals are frozen and only working copies are saved.
#![cfg(not(target_arch = "wasm32"))]
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use wordcraft_doc::{Document, StoryRef};
use wordcraft_engine::{Session, io};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!("wordcraft-conflict-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn open(path: &Path) -> Session {
    let mut s = Session::new(Document::new());
    s.run("file.open", &json!({"path": path})).unwrap();
    s.run("document.setText", &json!({"text": "Local edit"})).unwrap();
    s
}
fn original(d: &Scratch) -> PathBuf {
    let p = d.path("working.docx");
    io::save_path(&p, &Document::from_text("Original A")).unwrap();
    p
}

#[test]
fn versioned_open_keeps_the_automation_read_boundary() {
    use wordcraft_engine::file_access::{AutomationRoots, confine};
    let d = Scratch::new();
    let path = original(&d);
    let input = d.path("input");
    std::fs::create_dir(&input).unwrap();
    let allowed = input.join("allowed.docx");
    std::fs::copy(&path, &allowed).unwrap();
    let roots = AutomationRoots::new(Some(&input), None).unwrap().unwrap();
    let mut s = Session::new(Document::from_text("Unsaved draft"));
    confine(Some(&roots), || {
        let result = s.run("file.open", &json!({"path": path}));
        assert!(result.is_err_and(|e| e.to_string().contains("outside the read root")));
        assert_eq!(s.doc.plain_text(StoryRef::Body), "Unsaved draft");
        assert!(s.path.is_none());
        s.run("file.open", &json!({"path": allowed})).unwrap();
        assert_eq!(s.doc.plain_text(StoryRef::Body), "Original A");
    });
}

#[test]
fn versioned_save_keeps_the_automation_write_boundary() {
    use wordcraft_engine::file_access::{AutomationRoots, confine};
    let d = Scratch::new();
    let path = original(&d);
    let before = std::fs::read(&path).unwrap();
    let mut s = open(&path);
    let core = s.doc.core.clone();
    let output = d.path("output");
    std::fs::create_dir(&output).unwrap();
    let copy = output.join("copy.docx");
    let roots = AutomationRoots::new(None, Some(&output)).unwrap().unwrap();
    confine(Some(&roots), || {
        let result = s.run("file.save", &json!({}));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(result.is_err_and(|e| e.to_string().contains("outside the write root")));
        assert!(s.dirty);
        assert_eq!(s.path.as_ref(), Some(&path));
        assert_eq!(s.doc.core, core);
        assert_eq!(s.doc.plain_text(StoryRef::Body), "Local edit");
        s.run("file.saveAs", &json!({"path": copy})).unwrap();
        assert!(!s.dirty);
        assert_eq!(s.path.as_ref(), Some(&copy));
    });
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(io::open_path(&copy).unwrap().plain_text(StoryRef::Body), "Local edit");
}

fn conflict(s: &mut Session, path: &Path, expected: Option<&[u8]>) {
    let core = s.doc.core.clone();
    let password = s.password.clone();
    let err = s.run("file.save", &json!({})).unwrap_err();
    assert!(err.to_string().contains("file"), "{err}");
    assert!(s.dirty);
    assert_eq!(s.path.as_deref(), Some(path));
    assert_eq!(s.doc.plain_text(StoryRef::Body), "Local edit");
    assert_eq!(s.doc.core, core);
    assert_eq!(s.password, password);
    match expected {
        Some(bytes) => assert_eq!(std::fs::read(path).unwrap(), bytes),
        None => assert!(!path.exists()),
    }
}
#[test]
fn refuses_external_rewrite_atomic_replacement_and_identical_metadata() {
    for mode in ["rewrite", "atomic", "metadata"] {
        let d = Scratch::new();
        let p = original(&d);
        let mut original = std::fs::read(&p).unwrap();
        let mut b = io::save_bytes("external.docx", &Document::from_text("External B")).unwrap();
        if mode == "metadata" {
            // Compression does not promise equal sizes for equal-length text. ZIP comments
            // pad the original synthetic packages without changing their document contents.
            let size = original.len().max(b.len());
            for bytes in [&mut original, &mut b] {
                let end = bytes.len();
                assert_eq!(&bytes[end - 22..end - 18], b"PK\x05\x06");
                assert_eq!(&bytes[end - 2..], &[0, 0]);
                let comment = u16::try_from(size - end).unwrap();
                bytes[end - 2..].copy_from_slice(&comment.to_le_bytes());
                bytes.resize(size, b' ');
            }
            std::fs::write(&p, &original).unwrap();
            assert_eq!(io::open_bytes("external.docx", &b).unwrap().plain_text(StoryRef::Body), "External B");
            assert_eq!(io::open_path(&p).unwrap().plain_text(StoryRef::Body), "Original A");
        }
        let meta = std::fs::metadata(&p).unwrap();
        let mut s = open(&p);
        if mode == "atomic" {
            let tmp = d.path("external.docx");
            std::fs::write(&tmp, &b).unwrap();
            std::fs::rename(tmp, &p).unwrap();
        } else {
            std::fs::write(&p, &b).unwrap();
        }
        if mode == "metadata" {
            assert_eq!(b.len(), original.len());
            std::fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(meta.modified().unwrap()))
                .unwrap();
            assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), meta.modified().unwrap());
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                assert_eq!(std::fs::metadata(&p).unwrap().ino(), meta.ino());
            }
        }
        conflict(&mut s, &p, Some(&b));
        assert_eq!(std::fs::read_dir(&d.0).unwrap().count(), 1);
    }
}
#[test]
fn deletion_is_a_conflict_but_a_separate_save_as_preserves_external_bytes() {
    let d = Scratch::new();
    let p = original(&d);
    let mut s = open(&p);
    std::fs::remove_file(&p).unwrap();
    conflict(&mut s, &p, None);
    let b = io::save_bytes("external.docx", &Document::from_text("External B")).unwrap();
    std::fs::write(&p, &b).unwrap();
    conflict(&mut s, &p, Some(&b));
    let copy = d.path("copy.docx");
    s.run("file.saveAs", &json!({"path": copy})).unwrap();
    assert!(!s.dirty);
    assert_eq!(s.path.as_ref(), Some(&copy));
    assert_eq!(std::fs::read(&p).unwrap(), b);
    assert_eq!(io::open_path(&copy).unwrap().plain_text(StoryRef::Body), "Local edit");
}

#[test]
fn identical_replacement_is_benign_and_success_advances_expected_content() {
    let d = Scratch::new();
    let p = original(&d);
    let mut s = open(&p);
    let a = std::fs::read(&p).unwrap();
    let replacement = d.path("replacement.docx");
    std::fs::write(&replacement, &a).unwrap();
    std::fs::rename(&replacement, &p).unwrap();
    s.run("file.save", &json!({})).unwrap();
    assert!(!s.dirty);
    s.run("document.setText", &json!({"text": "Second local edit"})).unwrap();
    s.run("file.save", &json!({})).unwrap();
    assert!(!s.dirty);
    assert_eq!(io::open_path(&p).unwrap().plain_text(StoryRef::Body), "Second local edit");
    s.run("document.setText", &json!({"text": "Local edit"})).unwrap();
    std::fs::write(&p, &a).unwrap();
    conflict(&mut s, &p, Some(&a));
    s.run("file.open", &json!({"path": p})).unwrap();
    s.run("document.setText", &json!({"text": "Reloaded edit"})).unwrap();
    s.run("file.save", &json!({})).unwrap();
}

#[test]
fn failure_keeps_expected_version_and_does_not_stamp_or_change_password() {
    let d = Scratch::new();
    let p = original(&d);
    let mut s = open(&p);
    let a = std::fs::read(&p).unwrap();
    let core = s.doc.core.clone();
    let blocked = d.path("blocked.docx");
    std::fs::create_dir(&blocked).unwrap();
    assert!(s.run("file.saveAs", &json!({"path": blocked, "password": "synthetic-password"})).is_err());
    assert_eq!(s.doc.core, core);
    assert!(s.password.is_none());
    assert!(s.dirty);
    assert_eq!(std::fs::read(&p).unwrap(), a);
    let b = io::save_bytes("external.docx", &Document::from_text("External B")).unwrap();
    std::fs::write(&p, &b).unwrap();
    conflict(&mut s, &p, Some(&b));
    std::fs::write(&p, &a).unwrap();
    s.run("file.save", &json!({})).unwrap();
}

#[test]
fn replacing_document_clears_identity_and_each_session_keeps_its_own_version() {
    let d = Scratch::new();
    let p = original(&d);
    let mut first = open(&p);
    let mut second = open(&p);
    first.run("file.save", &json!({})).unwrap();
    let changed = std::fs::read(&p).unwrap();
    conflict(&mut second, &p, Some(&changed));
    second.run("file.new", &json!({})).unwrap();
    assert!(second.path.is_none());
    second.run("document.setText", &json!({"text": "New draft"})).unwrap();
    second.run("file.saveAs", &json!({"path": d.path("draft.docx")})).unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), changed);
    // Host-supplied bytes do not claim a native filesystem baseline.
    first.set_document(Document::from_text("Replacement draft"));
    first.path = None;
    first.run("file.saveAs", &json!({"path": d.path("replacement-draft.docx")})).unwrap();
}

#[test]
fn protected_open_and_save_compare_ciphertext_without_plaintext_files() {
    let d = Scratch::new();
    let p = d.path("protected.docx");
    io::save_path_with(&p, &Document::from_text("Protected A"), Some("synthetic-password"), &io::ExportOptions::default()).unwrap();
    let mut s = Session::new(Document::new());
    s.run("file.open", &json!({"path": p, "password": "synthetic-password"})).unwrap();
    s.run("document.setText", &json!({"text": "Local edit"})).unwrap();
    s.run("file.save", &json!({})).unwrap();
    assert!(wordcraft_docx::is_encrypted(&std::fs::read(&p).unwrap()));
    s.run("document.setText", &json!({"text": "Local edit"})).unwrap();
    let b =
        io::save_bytes_with("external.docx", &Document::from_text("Protected B"), Some("synthetic-password"), &io::ExportOptions::default()).unwrap();
    std::fs::write(&p, &b).unwrap();
    conflict(&mut s, &p, Some(&b));
    assert_eq!(std::fs::read_dir(&d.0).unwrap().count(), 1);
    let copy = d.path("protected-copy.docx");
    s.run("file.saveAs", &json!({"path": copy})).unwrap();
    assert!(wordcraft_docx::is_encrypted(&std::fs::read(&copy).unwrap()));
    assert!(io::needs_password(&io::open_path(&copy).unwrap_err()));
    assert_eq!(io::open_path_with(&copy, Some("synthetic-password")).unwrap().0.plain_text(StoryRef::Body), "Local edit");
}

#[test]
fn save_as_path_spellings_symlinks_and_hardlinks_cannot_bypass_conflicts() {
    for mode in [
        "dot",
        #[cfg(unix)]
        "symlink",
        "hardlink",
    ] {
        let d = Scratch::new();
        let p = original(&d);
        let mut s = open(&p);
        let alias = d.path("alias.docx");
        let target = match mode {
            "dot" => d.0.join("./working.docx"),
            #[cfg(unix)]
            "symlink" => {
                std::os::unix::fs::symlink(&p, &alias).unwrap();
                alias.clone()
            }
            _ => {
                std::fs::hard_link(&p, &alias).unwrap();
                alias.clone()
            }
        };
        let b = io::save_bytes("external.docx", &Document::from_text("External B with longer synthetic content to exercise changed-size hard links"))
            .unwrap();
        std::fs::write(&p, &b).unwrap();
        let core = s.doc.core.clone();
        assert!(matches!(s.run("file.saveAs", &json!({"path": target})), Err(wordcraft_engine::CmdError::FileConflict(_))));
        assert!(s.dirty);
        assert_eq!(s.path.as_ref(), Some(&p));
        assert_eq!(s.doc.core, core);
        assert_eq!(std::fs::read(&p).unwrap(), b);
        assert_eq!(std::fs::read(&target).unwrap(), b);
    }
}

#[test]
fn unreadable_non_file_destination_refuses_save_without_losing_edits() {
    let d = Scratch::new();
    let p = original(&d);
    let mut s = open(&p);
    std::fs::remove_file(&p).unwrap();
    std::fs::create_dir(&p).unwrap();
    std::fs::write(p.join("keep"), b"External directory sentinel").unwrap();
    assert!(matches!(s.run("file.save", &json!({})), Err(wordcraft_engine::CmdError::FileConflict(_))));
    assert!(s.dirty);
    assert_eq!(s.doc.plain_text(StoryRef::Body), "Local edit");
    assert_eq!(std::fs::read(p.join("keep")).unwrap(), b"External directory sentinel");
    assert_eq!(std::fs::read_dir(&d.0).unwrap().count(), 1);
}

#[test]
fn explicit_password_save_conflict_retains_the_original_password_and_metadata() {
    let d = Scratch::new();
    let path = original(&d);
    let mut s = open(&path);
    let external = io::save_bytes("external.docx", &Document::from_text("External B")).unwrap();
    std::fs::write(&path, &external).unwrap();
    let core = s.doc.core.clone();
    assert!(matches!(s.run("file.save", &json!({"password": "synthetic-password"})), Err(wordcraft_engine::CmdError::FileConflict(_))));
    assert!(s.dirty && s.password.is_none());
    assert_eq!(s.doc.core, core);
    assert_eq!(std::fs::read(path).unwrap(), external);
}

#[cfg(unix)]
#[test]
fn retargeting_a_parent_symlink_keeps_logical_path_ownership() {
    let d = Scratch::new();
    let first = d.path("first");
    let second = d.path("second");
    let link = d.path("alias");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let original = io::save_bytes("original.docx", &Document::from_text("Original A")).unwrap();
    let external = io::save_bytes("external.docx", &Document::from_text("External B")).unwrap();
    std::fs::write(first.join("working.docx"), &original).unwrap();
    std::fs::write(second.join("working.docx"), &external).unwrap();
    std::os::unix::fs::symlink(&first, &link).unwrap();
    let path = link.join("working.docx");
    let mut s = open(&path);
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&second, &link).unwrap();
    assert!(matches!(s.run("file.save", &json!({})), Err(wordcraft_engine::CmdError::FileConflict(_))));
    assert!(s.dirty);
    assert_eq!(std::fs::read(first.join("working.docx")).unwrap(), original);
    assert_eq!(std::fs::read(second.join("working.docx")).unwrap(), external);
}

/// A `.txt` (or other format that doesn't keep everything) saved over itself: the second Save was
/// refused as "changed by someone else", because only Word-format saves advanced the expected
/// version. Each save now expects what the last one wrote; an external edit is still refused.
#[test]
fn repeated_saves_over_a_plain_text_file_are_not_conflicts() {
    let d = Scratch::new();
    let path = d.path("notes.txt");
    std::fs::write(&path, "hello\n").unwrap();
    let mut s = open(&path);
    s.run("file.save", &json!({})).unwrap();
    s.run("document.setText", &json!({"text": "Second edit"})).unwrap();
    s.run("file.save", &json!({})).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("Second edit"));
    // Same length as the saved text, so only the content check can tell.
    std::fs::write(&path, std::fs::read_to_string(&path).unwrap().replace("Second", "Fourth")).unwrap();
    let external = std::fs::read(&path).unwrap();
    s.run("document.setText", &json!({"text": "Third edit"})).unwrap();
    assert!(matches!(s.run("file.save", &json!({})), Err(wordcraft_engine::CmdError::FileConflict(_))));
    assert_eq!(std::fs::read(&path).unwrap(), external);
    // A copy elsewhere doesn't change what the session's own file is expected to hold.
    s.run("file.save", &json!({"path": d.path("copy.txt")})).unwrap();
    assert!(matches!(s.run("file.save", &json!({})), Err(wordcraft_engine::CmdError::FileConflict(_))));
}

/// Save As to a folder, a device or a file that can't be read reported "changed or unavailable"
/// (the #813 conflict) although nobody changed anything: each is now a plain error that says what
/// is wrong, and a write-only file is just replaced, as any other Save As destination.
#[test]
fn save_as_to_a_folder_device_or_unreadable_file_is_not_a_conflict() {
    let d = Scratch::new();
    let p = original(&d);
    let mut s = open(&p);
    let folder = d.path("folder.docx");
    std::fs::create_dir(&folder).unwrap();
    let e = s.run("file.saveAs", &json!({"path": folder})).unwrap_err();
    assert!(!matches!(e, wordcraft_engine::CmdError::FileConflict(_)), "{e}");
    assert!(e.to_string().contains("folder"), "{e}");
    assert!(folder.is_dir() && s.dirty && s.path.as_deref() == Some(p.as_path()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // A device, named through a link (the save's format comes from the name).
        let device = d.path("device.docx");
        std::os::unix::fs::symlink("/dev/null", &device).unwrap();
        let e = s.run("file.saveAs", &json!({"path": device})).unwrap_err();
        assert!(!matches!(e, wordcraft_engine::CmdError::FileConflict(_)), "{e}");
        assert!(e.to_string().contains("not a regular file"), "{e}");
        let unreadable = d.path("write-only.docx");
        std::fs::write(&unreadable, b"unrelated synthetic bytes").unwrap();
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o200)).unwrap();
        s.run("file.saveAs", &json!({"path": unreadable})).unwrap();
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(io::open_path(&unreadable).unwrap().plain_text(StoryRef::Body), "Local edit");
    }
}

/// A save through a symbolic link replaced the link with a regular file, leaving the file it
/// pointed to stale. It now writes the file at the end of the link chain and the link stays.
#[cfg(unix)]
#[test]
fn saving_through_a_symlink_updates_its_target_and_keeps_the_link() {
    let d = Scratch::new();
    let real_dir = d.path("real");
    std::fs::create_dir(&real_dir).unwrap();
    let real = real_dir.join("working.docx");
    io::save_path(&real, &Document::from_text("Original A")).unwrap();
    // A chain: link2 -> link1 (relative) -> real.
    let link1 = d.path("link1.docx");
    let link2 = d.path("link2.docx");
    std::os::unix::fs::symlink(&real, &link1).unwrap();
    std::os::unix::fs::symlink("link1.docx", &link2).unwrap();
    let mut s = open(&link2);
    s.run("file.save", &json!({})).unwrap();
    assert!(std::fs::symlink_metadata(&link2).unwrap().file_type().is_symlink(), "the link stays a link");
    assert!(std::fs::symlink_metadata(&link1).unwrap().file_type().is_symlink());
    assert_eq!(io::open_path(&real).unwrap().plain_text(StoryRef::Body), "Local edit");
    // The temporary file was made beside the target and nothing else is left over.
    assert_eq!(std::fs::read_dir(&real_dir).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(&d.0).unwrap().count(), 3);
    // A second save still sees its own version (no false conflict).
    s.run("document.setText", &json!({"text": "Second edit"})).unwrap();
    s.run("file.save", &json!({})).unwrap();
    assert_eq!(io::open_path(&real).unwrap().plain_text(StoryRef::Body), "Second edit");
}

/// Confinement is checked against the file a save would really write: a link inside the write
/// root that leads outside it is refused, and the outside file is unchanged.
#[cfg(unix)]
#[test]
fn a_confined_save_through_a_link_out_of_the_write_root_is_refused() {
    use wordcraft_engine::file_access::{AutomationRoots, confine};
    let d = Scratch::new();
    let inside = d.path("inside");
    let outside = d.path("outside");
    std::fs::create_dir(&inside).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let secret = outside.join("secret.docx");
    io::save_path(&secret, &Document::from_text("Outside")).unwrap();
    let before = std::fs::read(&secret).unwrap();
    let link = inside.join("leak.docx");
    std::os::unix::fs::symlink(&secret, &link).unwrap();
    let roots = AutomationRoots::new(Some(&inside), Some(&inside)).unwrap().unwrap();
    let mut s = Session::new(Document::from_text("Agent text"));
    let e = confine(Some(&roots), || s.run("file.saveAs", &json!({"path": link}))).unwrap_err();
    assert!(e.to_string().contains("outside the write root"), "{e}");
    assert_eq!(std::fs::read(&secret).unwrap(), before);
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
}

/// A read-only file was silently replaced by Save. Like Word, the save is refused with an error
/// that suggests Save As; the file and the edits are both kept.
#[test]
fn a_read_only_file_is_not_overwritten() {
    let d = Scratch::new();
    let p = original(&d);
    let mut s = open(&p);
    let before = std::fs::read(&p).unwrap();
    let mut perms = std::fs::metadata(&p).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&p, perms.clone()).unwrap();
    let e = s.run("file.save", &json!({})).unwrap_err();
    assert!(matches!(e, wordcraft_engine::CmdError::ReadOnly(_)), "{e}");
    assert!(e.to_string().contains("read-only") && e.to_string().contains("Save As"), "{e}");
    // A password save is refused before it changes anything, too.
    let e = s.run("file.save", &json!({"password": "synthetic-password"})).unwrap_err();
    assert!(matches!(e, wordcraft_engine::CmdError::ReadOnly(_)), "{e}");
    assert!(s.password.is_none());
    assert!(s.dirty);
    assert_eq!(std::fs::read(&p).unwrap(), before);
    // Save As elsewhere keeps the edits.
    s.run("file.saveAs", &json!({"path": d.path("copy.docx")})).unwrap();
    assert_eq!(io::open_path(&d.path("copy.docx")).unwrap().plain_text(StoryRef::Body), "Local edit");
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&p, perms).unwrap();
}
