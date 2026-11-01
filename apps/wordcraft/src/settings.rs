//! Where the settings folder is (preferences, logs, recovery copies, user fonts, the custom
//! dictionary, the control key file), and moving it from WordCraft's location to Etch's.
//!
//! Etch keeps its settings in macOS `~/Library/Application Support/Etch`, Windows
//! `%APPDATA%\Etch`, otherwise `$XDG_CONFIG_HOME/etch` or `~/.config/etch`. Earlier builds,
//! named WordCraft, kept them in the same places under `WordCraft` / `wordcraft`. At start-up,
//! before anything reads or writes the folder, [`migrate_settings`] copies the old folder to
//! the new place when only the old one exists: into a temporary sibling folder first, every file
//! synchronized to disk, then renamed into place in one step, so the new folder is either
//! complete or absent. The old folder is never moved, changed or deleted: it stays as a
//! fallback. A copy that fails leaves no partial folder behind, and that run uses the old folder,
//! so recovery copies and settings are still found (the next start tries again).
//!
//! `ETCH_PROFILE_DIR`, or else `WORDCRAFT_PROFILE_DIR`, replaces the whole folder for isolated
//! runs; nothing is migrated then.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The deepest folder nesting copied; anything deeper is skipped (and logged).
const MAX_DEPTH: usize = 32;

/// Where the settings folder may be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Location {
    /// `ETCH_PROFILE_DIR` / `WORDCRAFT_PROFILE_DIR`: used as it is, never migrated.
    Profile(PathBuf),
    /// The platform's folder: Etch's (`new`) and the one WordCraft used (`old`).
    Default { old: PathBuf, new: PathBuf },
}

/// The platform whose folder conventions apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Other,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Other
        }
    }
}

/// The settings location from the environment variables `var` reads, on `os`.
pub fn location_from(var: impl Fn(&str) -> Option<OsString>, os: Platform) -> Option<Location> {
    let set = |name: &str| var(name).filter(|v| !v.is_empty());
    if let Some(dir) = set("ETCH_PROFILE_DIR").or_else(|| set("WORDCRAFT_PROFILE_DIR")) {
        return Some(Location::Profile(PathBuf::from(dir)));
    }
    let (base, old, new) = match os {
        Platform::MacOs => (set("HOME").map(|h| PathBuf::from(h).join("Library/Application Support")), "WordCraft", "Etch"),
        Platform::Windows => (set("APPDATA").map(PathBuf::from), "WordCraft", "Etch"),
        Platform::Other => {
            (set("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| set("HOME").map(|h| PathBuf::from(h).join(".config"))), "wordcraft", "etch")
        }
    };
    let base = base?;
    Some(Location::Default { old: base.join(old), new: base.join(new) })
}

/// This process's settings location.
pub fn location() -> Option<Location> {
    location_from(|name| std::env::var_os(name), Platform::current())
}

/// The folder to use for `location`. With `migrate` (a normal run), the old folder is copied to
/// the new place first if only it exists; without (`WORDCRAFT_NO_PREFS`, which writes no
/// settings), nothing is copied or created: the new folder if it exists, else the old one if
/// that does, else the new one.
pub fn resolve(location: Location, migrate: bool) -> PathBuf {
    match location {
        Location::Profile(dir) => dir,
        Location::Default { old, new } if migrate => choose(&old, &new),
        Location::Default { old, new } => {
            if is_dir(&new) || !is_dir(&old) {
                new
            } else {
                old
            }
        }
    }
}

/// [`migrate_settings`], falling back to `old` for this run when the copy failed.
pub fn choose(old: &Path, new: &Path) -> PathBuf {
    match migrate_settings(old, new) {
        Ok(dir) => dir,
        Err(e) => {
            log::warn!(
                "settings: couldn't copy {} to {}: {e}. Using the old folder for this run; nothing in it was changed, and the next start tries again",
                old.display(),
                new.display()
            );
            old.to_path_buf()
        }
    }
}

/// Copy the WordCraft settings folder `old` to Etch's `new` when `new` doesn't exist and `old`
/// does, and return the folder to use: `new` once it is in place (or when there was nothing to
/// copy). The copy goes into a temporary sibling of `new`, each file synchronized to disk, and is
/// then renamed to `new` in one step; `old` is only read. Symbolic links are not followed (a
/// link inside the folder is skipped, never copied as the file it points to), and entries that
/// can't be copied (special files, unreadable files) are skipped and logged. Any other failure
/// removes the temporary folder and is returned: the caller then uses `old` ([`choose`]). When
/// another instance renames its copy into place first, that copy is used.
pub fn migrate_settings(old: &Path, new: &Path) -> io::Result<PathBuf> {
    if fs::symlink_metadata(new).is_ok() {
        // Already migrated (or made by this version): never copy over it.
        return if is_dir(new) { Ok(new.to_path_buf()) } else { Err(io::Error::other(format!("{} exists and isn't a folder", new.display()))) };
    }
    // The old folder itself may be a link (to a dotfiles checkout, say): follow that one.
    if !is_dir(old) {
        return Ok(new.to_path_buf());
    }
    let parent = new.parent().filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| io::Error::other("the settings folder has no parent"))?;
    fs::create_dir_all(parent)?;
    let name = new.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "settings".into());
    let tmp = unique_temp(parent, &name)?;
    let copied = copy_tree(old, &tmp, 0).and_then(|n| {
        sync_dir(parent)?;
        Ok(n)
    });
    let files = match copied {
        Ok(n) => n,
        Err(e) => {
            remove_temp(&tmp);
            return Err(e);
        }
    };
    match fs::rename(&tmp, new) {
        Ok(()) => {
            if let Err(e) = sync_dir(parent) {
                log::warn!("settings: couldn't synchronize {}: {e}", parent.display());
            }
            log::info!("settings: copied {} ({files} files) to {}; the old folder was left as it was, as a fallback", old.display(), new.display());
            Ok(new.to_path_buf())
        }
        // Another instance started at the same time and put its copy in place first: use it.
        Err(_) if is_dir(new) => {
            remove_temp(&tmp);
            log::info!("settings: another instance copied {} to {} first; using that copy", old.display(), new.display());
            Ok(new.to_path_buf())
        }
        Err(e) => {
            remove_temp(&tmp);
            Err(e)
        }
    }
}

fn is_dir(p: &Path) -> bool {
    fs::metadata(p).is_ok_and(|m| m.is_dir())
}

/// A new, empty, private folder `.<name>.migrating-<pid>-<n>` in `parent`.
fn unique_temp(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let mut last = io::Error::other("no temporary folder name was free");
    for attempt in 0u32..16 {
        let tmp = parent.join(format!(".{name}.migrating-{}-{nanos}-{attempt}", std::process::id()));
        match private_dir(&tmp) {
            Ok(()) => return Ok(tmp),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

fn private_dir(p: &Path) -> io::Result<()> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut b = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(p)
}

/// Remove a temporary copy. It holds only copies, so removing it loses nothing.
fn remove_temp(tmp: &Path) {
    if let Err(e) = fs::remove_dir_all(tmp) {
        log::warn!("settings: couldn't remove the incomplete copy {}: {e}", tmp.display());
    }
}

/// Copy the contents of the folder `from` into the existing folder `to`; the number of files
/// copied. Skips (and logs) links, special files and what can't be read for lack of permission.
fn copy_tree(from: &Path, to: &Path, depth: usize) -> io::Result<u64> {
    let mut files = 0u64;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        // Not `metadata`: a link is seen as a link, never as what it points to.
        let meta = fs::symlink_metadata(&src)?;
        let kind = meta.file_type();
        if kind.is_symlink() {
            log::warn!("settings: skipped {}: a link isn't followed out of the settings folder", src.display());
        } else if kind.is_dir() {
            if depth >= MAX_DEPTH {
                log::warn!("settings: skipped {}: folders nested too deeply", src.display());
                continue;
            }
            match fs::read_dir(&src) {
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    log::warn!("settings: skipped {}: {e}", src.display());
                    continue;
                }
                Err(e) => return Err(e),
                Ok(_) => {}
            }
            private_dir(&dst)?;
            files = files.saturating_add(copy_tree(&src, &dst, depth + 1)?);
            // As private as the original (recovery folders are 0700), and always the owner's to
            // write in (and to remove, should a later step fail).
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dst, fs::Permissions::from_mode(meta.permissions().mode() | 0o700))?;
            }
            sync_dir(&dst)?;
        } else if kind.is_file() {
            if copy_file(&src, &dst, &meta)? {
                files = files.saturating_add(1);
            }
        } else {
            log::warn!("settings: skipped {}: not a regular file or folder", src.display());
        }
    }
    Ok(files)
}

/// Copy one regular file, synchronized to disk, with the original's permissions. `Ok(false)`
/// when it can't be read for lack of permission (skipped and logged).
fn copy_file(src: &Path, dst: &Path, meta: &fs::Metadata) -> io::Result<bool> {
    #[cfg(test)]
    if FAIL_COPY.with(|f| {
        let n = f.get();
        f.set(n.saturating_sub(1));
        n == 1
    }) {
        return Err(io::Error::other("injected copy failure"));
    }
    let mut input = match fs::File::open(src) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            log::warn!("settings: skipped {}: {e}", src.display());
            return Ok(false);
        }
        Err(e) => return Err(e),
    };
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    // Private until the original's permissions are applied below.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut output = options.open(dst)?;
    io::copy(&mut input, &mut output)?;
    output.set_permissions(meta.permissions())?;
    output.sync_all()?;
    Ok(true)
}

/// Synchronize a folder's entries to disk (Unix; Windows has no folder handle for that).
fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// Tests: the n-th file copied (1-based) fails, as a full disk would; 0 never.
    static FAIL_COPY: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use wordcraft_engine::Session;
    use wordcraft_ui_egui::{Services, UiState, WordApp};

    /// A fresh temporary folder (never the real settings folder), removed when dropped.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("etch-settings-{tag}-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Scratch(p)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            #[cfg(unix)]
            make_writable(&self.0);
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn make_writable(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(m) = fs::symlink_metadata(p)
            && !m.file_type().is_symlink()
        {
            let _ = fs::set_permissions(p, fs::Permissions::from_mode(if m.is_dir() { 0o700 } else { 0o600 }));
            if m.is_dir()
                && let Ok(rd) = fs::read_dir(p)
            {
                for e in rd.flatten() {
                    make_writable(&e.path());
                }
            }
        }
    }

    /// Every file under `dir` (relative path → bytes); links and other odd entries left out.
    fn tree(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for e in fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                let m = fs::symlink_metadata(&p).unwrap();
                if m.is_dir() {
                    walk(root, &p, out);
                } else if m.is_file()
                    && let Ok(bytes) = fs::read(&p)
                {
                    out.insert(p.strip_prefix(root).unwrap().to_path_buf(), bytes);
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(dir, dir, &mut out);
        out
    }

    /// The temporary folders a migration into `parent` left behind.
    fn leftovers(parent: &Path) -> Vec<PathBuf> {
        fs::read_dir(parent).unwrap().map(|e| e.unwrap().path()).filter(|p| p.to_string_lossy().contains(".migrating-")).collect()
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

    #[test]
    fn locations_per_platform_and_profile_overrides() {
        let env =
            |pairs: &'static [(&'static str, &'static str)]| move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| OsString::from(v));
        let d = |old: &str, new: &str| Some(Location::Default { old: PathBuf::from(old), new: PathBuf::from(new) });
        assert_eq!(location_from(env(&[("HOME", "/h")]), Platform::Other), d("/h/.config/wordcraft", "/h/.config/etch"));
        assert_eq!(location_from(env(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "/x")]), Platform::Other), d("/x/wordcraft", "/x/etch"));
        assert_eq!(location_from(env(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "")]), Platform::Other), d("/h/.config/wordcraft", "/h/.config/etch"));
        assert_eq!(
            location_from(env(&[("HOME", "/Users/a")]), Platform::MacOs),
            d("/Users/a/Library/Application Support/WordCraft", "/Users/a/Library/Application Support/Etch")
        );
        let win = location_from(env(&[("APPDATA", "C:/Users/a/AppData/Roaming")]), Platform::Windows);
        assert_eq!(win, d("C:/Users/a/AppData/Roaming/WordCraft", "C:/Users/a/AppData/Roaming/Etch"));
        assert_eq!(location_from(env(&[]), Platform::Other), None);
        let p = |dir: &str| Some(Location::Profile(PathBuf::from(dir)));
        assert_eq!(location_from(env(&[("HOME", "/h"), ("WORDCRAFT_PROFILE_DIR", "/w")]), Platform::Other), p("/w"));
        assert_eq!(location_from(env(&[("HOME", "/h"), ("ETCH_PROFILE_DIR", "/e")]), Platform::MacOs), p("/e"));
        assert_eq!(location_from(env(&[("ETCH_PROFILE_DIR", "/e"), ("WORDCRAFT_PROFILE_DIR", "/w")]), Platform::Windows), p("/e"), "Etch's wins");
        assert_eq!(location_from(env(&[("ETCH_PROFILE_DIR", ""), ("WORDCRAFT_PROFILE_DIR", "/w")]), Platform::Other), p("/w"), "empty is unset");
        // A profile folder is used as it is: nothing is copied into it.
        let t = Scratch::new("profile");
        fs::create_dir(t.0.join("wordcraft")).unwrap();
        fs::write(t.0.join("wordcraft/ui.json"), b"{}").unwrap();
        assert_eq!(resolve(Location::Profile(t.0.join("p")), true), t.0.join("p"));
        assert!(!t.0.join("p").exists());
    }

    /// Recovery copies and preferences survive the move: a recovery copy of unsaved work and a
    /// `ui.json` in a WordCraft-style folder are found and loaded from Etch's folder afterwards.
    #[test]
    fn recovery_copies_and_preferences_survive_the_migration() {
        let t = Scratch::new("recovery");
        let (old, new) = (t.0.join("wordcraft"), t.0.join("etch"));
        // Unsaved synthetic work, copied for recovery by the old app's session.
        let mut s = Session::new(wordcraft_doc::Document::new());
        s.enable_recovery(old.join("recovery"), Arc::new(|| {})).unwrap();
        s.run("text.insert", &serde_json::json!({"text": "Synthetic unsaved draft"})).unwrap();
        assert_eq!(s.emergency_snapshot(Duration::from_secs(10)), Ok(true));
        drop(s);
        // Preferences that differ from the defaults.
        let mut app = WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default());
        app.ui.recent = vec!["/synthetic/letter.docx".into()];
        app.ui.ribbon_collapsed = !app.ui.ribbon_collapsed;
        let prefs = serde_json::to_vec_pretty(&app.prefs()).unwrap();
        fs::write(old.join("ui.json"), &prefs).unwrap();
        let before = tree(&old);

        assert_eq!(resolve(Location::Default { old: old.clone(), new: new.clone() }, true), new);
        assert_eq!(tree(&new), before, "everything was copied");
        assert_eq!(tree(&old), before, "the old folder is untouched");
        assert!(leftovers(&t.0).is_empty());

        let mut s = Session::new(wordcraft_doc::Document::new());
        s.enable_recovery(new.join("recovery"), Arc::new(|| {})).unwrap();
        settle(&mut s);
        let docs = s.recovery_documents();
        assert_eq!(docs.len(), 1, "{docs:?}");
        s.run("file.recover", &serde_json::json!({"restore": docs[0].id})).unwrap();
        settle(&mut s);
        assert!(s.doc.plain_text(wordcraft_doc::StoryRef::Body).contains("Synthetic unsaved draft"));

        let mut loaded = WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default());
        loaded.apply_prefs(UiState::from_json(&fs::read(new.join("ui.json")).unwrap()).unwrap());
        assert_eq!(serde_json::to_value(loaded.prefs()).unwrap(), serde_json::to_value(app.prefs()).unwrap());
    }

    /// Etch's folder already exists: nothing is copied, and neither folder changes.
    #[test]
    fn an_existing_new_folder_is_used_as_it_is() {
        let t = Scratch::new("exists");
        let (old, new) = (t.0.join("wordcraft"), t.0.join("etch"));
        fs::create_dir_all(old.join("recovery")).unwrap();
        fs::write(old.join("ui.json"), b"{\"old\":true}").unwrap();
        fs::create_dir(&new).unwrap();
        fs::write(new.join("ui.json"), b"{\"new\":true}").unwrap();
        let (old_before, new_before) = (tree(&old), tree(&new));
        assert_eq!(migrate_settings(&old, &new).unwrap(), new);
        assert_eq!(tree(&old), old_before);
        assert_eq!(tree(&new), new_before);
        assert!(leftovers(&t.0).is_empty());
        // A file where the folder should be is an error, and the old folder is used.
        let file = t.0.join("file");
        fs::write(&file, b"x").unwrap();
        assert!(migrate_settings(&old, &file).is_err());
        assert_eq!(choose(&old, &file), old);
        assert_eq!(fs::read(&file).unwrap(), b"x");
    }

    /// Nothing to migrate: the new folder is used (created later by whatever writes to it).
    #[test]
    fn a_fresh_install_uses_the_new_folder() {
        let t = Scratch::new("fresh");
        let (old, new) = (t.0.join("wordcraft"), t.0.join("etch"));
        assert_eq!(migrate_settings(&old, &new).unwrap(), new);
        assert!(!new.exists() && !old.exists());
        // Without preferences (WORDCRAFT_NO_PREFS) nothing is copied or created: the old folder
        // is used while it is the only one.
        let loc = || Location::Default { old: old.clone(), new: new.clone() };
        assert_eq!(resolve(loc(), false), new);
        fs::create_dir(&old).unwrap();
        assert_eq!(resolve(loc(), false), old);
        assert!(!new.exists());
        fs::create_dir(&new).unwrap();
        assert_eq!(resolve(loc(), false), new);
    }

    /// A copy that fails part-way (a full disk) leaves no partial folder: no new folder, no
    /// temporary one, the old folder untouched and used for this run. The next start succeeds.
    #[test]
    fn a_failed_copy_falls_back_to_the_old_folder() {
        let t = Scratch::new("fail");
        let (old, new) = (t.0.join("wordcraft"), t.0.join("etch"));
        fs::create_dir_all(old.join("recovery/0123")).unwrap();
        fs::write(old.join("ui.json"), b"{}").unwrap();
        fs::write(old.join("custom-dictionary.txt"), b"synthword\n").unwrap();
        fs::write(old.join("recovery/0123/snapshot-1.json"), b"synthetic").unwrap();
        let before = tree(&old);
        FAIL_COPY.with(|f| f.set(2));
        assert!(migrate_settings(&old, &new).unwrap_err().to_string().contains("injected"));
        FAIL_COPY.with(|f| f.set(2));
        assert_eq!(choose(&old, &new), old, "the old folder is used for this run");
        FAIL_COPY.with(|f| f.set(0));
        assert!(!new.exists(), "no partial new folder");
        assert!(leftovers(&t.0).is_empty(), "the temporary copy was removed");
        assert_eq!(tree(&old), before);
        assert_eq!(choose(&old, &new), new);
        assert_eq!(tree(&new), before);
    }

    /// A parent folder the copy can't be made in: the old folder is used, unchanged.
    #[cfg(unix)]
    #[test]
    fn an_unwritable_parent_falls_back_to_the_old_folder() {
        use std::os::unix::fs::PermissionsExt;
        let t = Scratch::new("ro");
        let old = t.0.join("wordcraft");
        fs::create_dir(&old).unwrap();
        fs::write(old.join("ui.json"), b"{}").unwrap();
        let ro = t.0.join("ro");
        fs::create_dir(&ro).unwrap();
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o500)).unwrap();
        let new = ro.join("etch");
        if fs::File::create(ro.join("probe")).is_ok() {
            return; // Running as root: permissions don't stop anything.
        }
        assert_eq!(choose(&old, &new), old);
        assert!(!new.exists());
        assert_eq!(fs::read(old.join("ui.json")).unwrap(), b"{}");
    }

    /// Links aren't followed out of the folder, and sockets and unreadable files are skipped:
    /// all logged, none of it fails the start-up, and everything else is copied.
    #[cfg(unix)]
    #[test]
    fn links_and_odd_files_are_skipped_never_followed() {
        use std::os::unix::fs::PermissionsExt;
        let t = Scratch::new("odd");
        let (old, new, outside) = (t.0.join("wordcraft"), t.0.join("etch"), t.0.join("outside"));
        fs::create_dir_all(old.join("fonts")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), b"not ours").unwrap();
        fs::write(old.join("ui.json"), b"{}").unwrap();
        fs::write(old.join("fonts/Synthetic.ttf"), b"font bytes").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), old.join("link-to-file")).unwrap();
        std::os::unix::fs::symlink(&outside, old.join("link-to-dir")).unwrap();
        std::os::unix::fs::symlink(t.0.join("missing"), old.join("dangling")).unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(old.join("control.sock")).unwrap();
        fs::write(old.join("unreadable"), b"x").unwrap();
        fs::set_permissions(old.join("unreadable"), fs::Permissions::from_mode(0o000)).unwrap();
        let root = fs::read(old.join("unreadable")).is_ok();
        fs::create_dir(old.join("closed")).unwrap();
        fs::write(old.join("closed/inside"), b"y").unwrap();
        fs::set_permissions(old.join("closed"), fs::Permissions::from_mode(0o000)).unwrap();

        assert_eq!(choose(&old, &new), new);
        let names: Vec<String> = fs::read_dir(&new).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        for skipped in ["link-to-file", "link-to-dir", "dangling", "control.sock"] {
            assert!(!names.contains(&skipped.to_string()), "{skipped} in {names:?}");
        }
        if !root {
            assert!(!names.contains(&"unreadable".to_string()) && !names.contains(&"closed".to_string()), "{names:?}");
        }
        assert_eq!(fs::read(new.join("ui.json")).unwrap(), b"{}");
        assert_eq!(fs::read(new.join("fonts/Synthetic.ttf")).unwrap(), b"font bytes");
        assert_eq!(fs::read(outside.join("secret.txt")).unwrap(), b"not ours");
        assert!(fs::symlink_metadata(old.join("link-to-dir")).unwrap().file_type().is_symlink(), "the old folder is untouched");
        assert!(leftovers(&t.0).is_empty());
    }

    /// The old folder itself may be a link (a dotfiles checkout): that one is followed.
    #[cfg(unix)]
    #[test]
    fn an_old_folder_that_is_a_link_is_copied() {
        let t = Scratch::new("oldlink");
        let real = t.0.join("dotfiles-wordcraft");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("ui.json"), b"{}").unwrap();
        let (old, new) = (t.0.join("wordcraft"), t.0.join("etch"));
        std::os::unix::fs::symlink(&real, &old).unwrap();
        assert_eq!(choose(&old, &new), new);
        assert!(!fs::symlink_metadata(&new).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(new.join("ui.json")).unwrap(), b"{}");
    }

    /// Recovery folders stay private after the copy.
    #[cfg(unix)]
    #[test]
    fn permissions_are_kept() {
        use std::os::unix::fs::PermissionsExt;
        let t = Scratch::new("perm");
        let (old, new) = (t.0.join("wordcraft"), t.0.join("etch"));
        fs::create_dir_all(old.join("recovery")).unwrap();
        fs::set_permissions(old.join("recovery"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(old.join("recovery/copy"), b"z").unwrap();
        fs::set_permissions(old.join("recovery/copy"), fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(choose(&old, &new), new);
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&new.join("recovery")), 0o700);
        assert_eq!(mode(&new.join("recovery/copy")), 0o600);
    }

    /// Two instances starting at once: both end up in the same complete folder, nothing lost,
    /// no temporary folder left.
    #[test]
    fn two_instances_migrating_at_once_share_one_copy() {
        for round in 0..8 {
            let t = Scratch::new(&format!("race{round}"));
            let (old, new) = (t.0.join("wordcraft"), t.0.join("etch"));
            fs::create_dir_all(old.join("recovery/abc")).unwrap();
            for i in 0..20 {
                fs::write(old.join(format!("recovery/abc/part-{i}.bin")), vec![i as u8; 4096]).unwrap();
            }
            fs::write(old.join("ui.json"), b"{}").unwrap();
            let before = tree(&old);
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    let (old, new, barrier) = (old.clone(), new.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        barrier.wait();
                        choose(&old, &new)
                    })
                })
                .collect();
            for h in handles {
                assert_eq!(h.join().unwrap(), new);
            }
            assert_eq!(tree(&new), before);
            assert_eq!(tree(&old), before);
            assert!(leftovers(&t.0).is_empty(), "{:?}", leftovers(&t.0));
        }
    }
}
