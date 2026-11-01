//! Etch desktop app.
//!
//! Usage: `etch [--control <port>] [--automation-read-root DIR] [--automation-write-root DIR] [--sample] [files…]`; `--help` prints `USAGE`.
//!
//! Built with the `automation` feature (tests and development only; release builds leave it off
//! and refuse these flags and `WORDCRAFT_CONTROL_PORT` with exit code 2),
//! `--control <port>` (or `WORDCRAFT_CONTROL_PORT`) starts a localhost JSON-lines control server:
//! `{"id":1,"key":"…","method":"engine.execute","params":{"command":"text.insert","params":{"text":"Hi"}}}`.
//! Every request needs the key the app writes to `<settings>/control-key.<instance>` (mode 0600,
//! removed on exit). `--automation-read-root` / `--automation-write-root` confine the files every
//! control connection's commands read and write (a connection can narrow them further, never
//! widen them); what the person does in the window is not confined. See `docs/control-protocol.md`.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod animations;
#[cfg(target_os = "macos")]
mod apple_events;
mod clipboard;
#[cfg(feature = "automation")]
mod control_server;
mod default_apps;
mod file_dialogs;
#[cfg(any(target_os = "windows", target_os = "macos", test))]
mod graphics;
mod logging;
mod print;
mod region;
mod settings;
mod termination;
#[cfg(target_os = "macos")]
mod traffic_lights;

#[cfg(feature = "automation")]
use control_server::KeyFile;
use wordcraft_engine::Session;
use wordcraft_ui_egui::{
    Services, UiState, WordApp,
    window_geometry::{WindowGeometry, take_rescue},
};

/// Built without the `automation` feature there is no control server, so never a key file.
#[cfg(not(feature = "automation"))]
enum KeyFile {}

#[cfg(not(feature = "automation"))]
impl KeyFile {
    fn remove(&self) {
        match *self {}
    }
}

/// The app, the restored window geometry (or, with none, the default: no position) until the
/// first frame that knows the monitor has checked it, the control server's key file (removed on
/// exit), the file dialogs the app asked for and, on macOS, the documents opened from Finder.
struct App(WordApp, Option<WindowGeometry>, Option<KeyFile>, file_dialogs::Launcher, #[cfg(target_os = "macos")] fmv_macos_events::Inbox);

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        apple_events::poll(&self.4, &mut self.0, ctx);
        // A panic asked for a recovery copy; a termination signal asks for one and to quit,
        // without asking about unsaved changes (the copy is offered at the next start).
        if termination::take_snapshot_request() {
            termination::snapshot(&mut self.0);
        }
        if termination::quit_signal().is_some() && !self.0.quit_requested {
            termination::snapshot(&mut self.0);
            self.0.quit_requested = true;
        }
        self.0.logic(ctx);
        self.3.show(frame);
        // A window that doesn't fit on its screen (the default size on a laptop) shrinks to fit
        // there, so all of the scroll bar can be dragged (#1215). Checked on the frame that first
        // knows the monitor: the off-screen check below takes the pending geometry then.
        if let Some((pos, size)) = ctx.input(|i| self.1.and_then(|g| g.fit_on_monitor(i.viewport()))) {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        }
        let prev = self.0.ui.window;
        self.0.ui.window = ctx.input(|i| WindowGeometry::track(prev, i.viewport(), i.viewport_rect().size()));
        // The first frame, once the monitor is known (as `take_rescue` waits for): maximize a
        // window that opened at its normal size so it never flashed on screen half-opened.
        let maximize = self.1.and_then(|g| g.maximize_after_open()).filter(|_| ctx.input(|i| i.viewport().monitor_size.is_some()));
        if let Some(pos) = ctx.input(|i| take_rescue(&mut self.1, i.viewport())) {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        }
        if let Some(command) = maximize {
            ctx.send_viewport_cmd(command);
        }
        // The window's close button: with unsaved changes, ask first (`WordApp::close_requested`).
        let closing = ctx.input(|i| i.viewport().close_requested());
        if closing && !self.0.close_requested() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        } else if (closing || self.0.quit_requested) && !self.0.printing.busy() {
            // On macOS, letting eframe drop the window inside `-[NSApplication run]` aborts in
            // `-[_NSTouchBarFinderObservation invalidate]`. Command-Q calls `terminate:`. Once
            // the close is allowed, take that path instead of dropping the window on this turn.
            #[cfg(target_os = "macos")]
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.0.quit_requested = false;
                schedule_macos_terminate(&mut self.0, self.2.as_ref());
            }
            #[cfg(not(target_os = "macos"))]
            if self.0.quit_requested {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.0.raw_input_hook(raw);
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.0.ui(ui);
        self.3.show(frame);
    }
    fn on_exit(&mut self) {
        print::finish_pending_jobs();
        // A save just before the close queued the discard of its recovery copy; wait for it, or
        // the next start offers to recover a document that was saved cleanly (#3140).
        self.0.session.finish_recovery();
        save_prefs(&self.0);
        if let Some(key_file) = &self.2 {
            key_file.remove();
        }
    }
}

/// The app is going away with unsaved changes: closed by an agent's forced quit or a signal, or a
/// panic unwinding the UI thread (which owns the session, so no lock is needed here). Write an
/// up-to-date recovery copy so they are offered at the next start; a saved document has none.
impl Drop for App {
    fn drop(&mut self) {
        if self.0.session.dirty {
            termination::snapshot(&mut self.0);
        }
    }
}

/// Quit the way Command-Q does, on the next turn of the main queue.
///
/// Called when the window is closing. Doing it on the next turn lets the close
/// button's display cycle finish while the window still exists; `terminate:`
/// then ends the process. Preferences are written and the control key file is
/// removed first, as `on_exit` does, because a fallback [`std::process::exit`]
/// skips `on_exit`. A second close while a quit is already scheduled is ignored.
#[cfg(target_os = "macos")]
fn schedule_macos_terminate(app: &mut WordApp, key_file: Option<&KeyFile>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    if app.printing.busy() {
        return;
    }
    static QUIT: AtomicBool = AtomicBool::new(false);
    if QUIT.swap(true, Ordering::Relaxed) {
        return;
    }
    log::info!("window close: quitting");
    // This path can bypass on_exit. Join the owned workers before termination and cleanup.
    print::finish_pending_jobs();
    app.session.finish_recovery();
    save_prefs(app);
    if let Some(key_file) = key_file {
        key_file.remove();
    }
    dispatch2::DispatchQueue::main().exec_async(|| {
        if let Some(mtm) = objc2::MainThreadMarker::new() {
            objc2_app_kit::NSApplication::sharedApplication(mtm).terminate(None);
        }
        // `terminate:` does not return when it succeeds. If it did, leave anyway:
        // staying up after the close was cancelled would trap the user in the window.
        std::process::exit(0);
    });
}

/// The app's settings folder (preferences, logs, recovery, user fonts and dictionary, the control
/// key file): macOS `~/Library/Application Support/Etch`, Windows `%APPDATA%\Etch`, otherwise
/// `$XDG_CONFIG_HOME/etch` or `~/.config/etch`; `ETCH_PROFILE_DIR` (or `WORDCRAFT_PROFILE_DIR`)
/// overrides the entire folder for isolated runs. Worked out once: the first call copies
/// WordCraft's folder to Etch's place when only the old one exists (`settings`), so `main` calls
/// it before anything reads or writes the folder.
fn settings_dir() -> Option<std::path::PathBuf> {
    static DIR: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| settings::location().map(|loc| settings::resolve(loc, prefs_enabled()))).clone()
}

/// `ui.json` in the settings folder.
fn prefs_path() -> Option<std::path::PathBuf> {
    settings_dir().map(|b| b.join("ui.json"))
}

/// Where the log files live: `logs` in the preferences folder (see `logging`).
fn log_dir() -> Option<std::path::PathBuf> {
    Some(prefs_path()?.parent()?.join("logs"))
}

/// `custom-dictionary.txt` in the settings folder: the words Add to Dictionary kept (#40).
fn user_dict_path() -> Option<std::path::PathBuf> {
    settings_dir().map(|b| b.join(wordcraft_engine::Session::USER_DICT_FILE))
}

fn load_user_dictionary(app: &mut WordApp) {
    if !prefs_enabled() {
        return;
    }
    if let Some(p) = user_dict_path() {
        wordcraft_proof::load_user_dictionary(&p);
        app.session.user_dict_path = Some(p);
    }
}

/// Runs without preferences (`WORDCRAFT_NO_PREFS`, agents' test runs) neither read nor write them.
fn prefs_enabled() -> bool {
    std::env::var_os("WORDCRAFT_NO_PREFS").is_none()
}

fn load_prefs(app: &mut WordApp) {
    if !prefs_enabled() {
        return;
    }
    if let Some(p) = prefs_path()
        && let Ok(bytes) = std::fs::read(&p)
        && let Some(ui) = UiState::from_json(&bytes)
    {
        app.apply_prefs(ui);
    }
}

/// Fonts the user added (#582) live in `fonts` in the settings folder: load them, and store fonts
/// added later there. Runs without preferences neither read nor keep them.
fn load_user_fonts() {
    if !prefs_enabled() {
        return;
    }
    let Some(dir) = settings_dir().map(|d| d.join("fonts")) else { return };
    let (loaded, skipped) = wordcraft_fonts::FontDb::global().load_user_fonts(&dir);
    if !loaded.is_empty() || !skipped.is_empty() {
        log::info!("user fonts: {} loaded, {} skipped from {}", loaded.len(), skipped.len(), dir.display());
    }
}

/// The saved window geometry, read before the window opens (`load_prefs` runs after).
fn saved_window() -> Option<WindowGeometry> {
    if !prefs_enabled() {
        return None;
    }
    let bytes = std::fs::read(prefs_path()?).ok()?;
    UiState::from_json(&bytes)?.window?.sanitized()
}

fn save_prefs(app: &WordApp) {
    if !prefs_enabled() {
        return;
    }
    if let Some(p) = prefs_path() {
        if let Some(dir) = p.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            log::warn!("preferences: cannot create {}: {e}", dir.display());
            return;
        }
        match serde_json::to_vec_pretty(&app.prefs()) {
            Ok(bytes) => {
                if let Err(e) = write_prefs(&p, &bytes) {
                    log::warn!("preferences: cannot save {}: {e}", p.display());
                }
            }
            Err(e) => log::warn!("preferences: cannot serialize: {e}"),
        }
    }
}

/// Writes `bytes` to a sibling temporary file, synchronizes it to disk and renames it over
/// `path` (then synchronizes the folder, on Unix), so a failed or interrupted write, or a power
/// loss, leaves the previous preferences in place rather than an empty file.
fn write_prefs(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "ui.json".into());
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let written = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        sync_file(&file)?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written?;
    // The preferences are in place; a folder that can't be synchronized doesn't undo that.
    #[cfg(unix)]
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
        && let Err(e) = std::fs::File::open(dir).and_then(|d| d.sync_all())
    {
        log::warn!("preferences: cannot synchronize {}: {e}", dir.display());
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// Tests: the next synchronization fails, as a failing drive's would.
    static FAIL_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn sync_file(file: &std::fs::File) -> std::io::Result<()> {
    #[cfg(test)]
    if FAIL_SYNC.with(|f| f.replace(false)) {
        return Err(std::io::Error::other("injected sync failure"));
    }
    file.sync_all()
}

/// Native file dialogs, shown without blocking the window (`file_dialogs`, #94), File › Print
/// through the system spooler (`print`), the system clipboard's text, formatted copies,
/// pictures and format names (`clipboard`, #45, #584, #687, #833), the system's animation setting
/// (`animations`), and making Etch the default app for Word documents (`default_apps`, #295).
fn services(file_dialog: file_dialogs::Hook, ctx: &egui::Context) -> Services {
    Services {
        file_dialog: Some(file_dialog),
        printers: Some(print::discover()),
        print_native: Some(print::submit()),
        clipboard_picture: Some(Box::new(clipboard::picture)),
        make_default_app: default_apps::hook(),
        animations_enabled: Some(animations::hook(ctx)),
        clipboard_text: Some(Box::new(clipboard::text)),
        clipboard_html: Some(Box::new(clipboard::html)),
        set_clipboard_html: Some(Box::new(clipboard::set_html)),
        clipboard_rtf: Some(Box::new(clipboard::rtf)),
        clipboard_formats: Some(Box::new(clipboard::formats)),
        ..Default::default()
    }
}

/// Window, Dock and taskbar icon.
fn app_icon() -> Option<egui::IconData> {
    #[cfg(target_os = "macos")]
    let png: &[u8] = include_bytes!("../../../assets/app-icon/wordcraft-macos-512.png");
    #[cfg(not(target_os = "macos"))]
    let png: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/ai.storyteller.wordcraft.png");
    eframe::icon_data::from_png_bytes(png).map_err(|e| log::warn!("app icon: {e}")).ok()
}

#[cfg(feature = "automation")]
const USAGE: &str = "\
etch — the Etch desktop app

USAGE:
  etch [--sample] [--control PORT] [FILE…]

OPTIONS:
  --sample            open the sample document
  --control PORT      start the localhost control server on PORT, for tests and scripts
                      (or set WORDCRAFT_CONTROL_PORT); see docs/control-protocol.md
  --version           print the version
  -h, --help          print this help
";

#[cfg(not(feature = "automation"))]
const USAGE: &str = "\
etch — the Etch desktop app

USAGE:
  etch [--sample] [FILE…]

OPTIONS:
  --sample            open the sample document
  --version           print the version
  -h, --help          print this help
";

/// The commit the build came from (short), or `dev`.
fn build_sha() -> &'static str {
    option_env!("WORDCRAFT_BUILD_SHA").map(|s| s.get(..8).unwrap_or(s)).unwrap_or("dev")
}

/// Start the lazy work the first layout would otherwise do one after the other on the UI thread
/// (~25 ms cataloging the system font folders, ~25 ms decoding the dictionary) on background
/// threads while the window opens. A lookup that needs either before it is done waits for that
/// work rather than repeating it; a failed spawn leaves it to the first use.
fn warm_start_up() {
    wordcraft_fonts::FontDb::global().scan_in_background();
    let _ = std::thread::Builder::new().name("dictionary-warm".into()).spawn(|| {
        let _ = wordcraft_proof::dict::Dictionary::en_us();
        let _ = wordcraft_proof::hyphen::Patterns::en_us();
        let _ = wordcraft_proof::is_correct("warm");
    });
}

/// The arguments as text, and apart the files whose names aren't valid Unicode, such as a Latin-1
/// one (#1440). A `--control` port that isn't is an error.
fn text_args() -> (Vec<String>, Vec<std::path::PathBuf>) {
    match split_args(std::env::args_os().skip(1)) {
        Ok(split) => split,
        Err(e) => {
            // Unlike `eprintln!`, never panics.
            let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("etch: {e}\n"));
            std::process::exit(2);
        }
    }
}

/// [`text_args`] of `args` (the program name left out). A flag, or a flag's value, that isn't
/// valid Unicode is an error, never a file to open: `--automation-read-root=<such a folder>` taken
/// for a file name would leave the control channel unconfined.
fn split_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<(Vec<String>, Vec<std::path::PathBuf>), String> {
    let (mut args_out, mut files) = (Vec::new(), Vec::new());
    for (i, a) in args.into_iter().enumerate() {
        match a.into_string() {
            Ok(a) => args_out.push(a),
            Err(a) => {
                let lossy = a.to_string_lossy().into_owned();
                let after_flag = args_out
                    .last()
                    .is_some_and(|p: &String| matches!(p.as_str(), "--control" | "--automation-read-root" | "--automation-write-root"));
                if after_flag || lossy.starts_with("--") {
                    return Err(format!("argument {} is not valid Unicode: {lossy}", i + 1));
                }
                files.push(a.into());
            }
        }
    }
    Ok((args_out, files))
}

/// Open a file whose name isn't valid Unicode (#1440). `file.open` takes its path as text: the
/// session is told the real path its lossy text stands for, so it opens that file by path (capped,
/// regular files only, as every open) and keeps the version of the bytes it read, so a later save
/// refuses to overwrite someone else's change (#813). A password asked for first opens it too.
fn open_non_unicode(app: &mut WordApp, path: &std::path::Path) {
    let name = app.session.expect_open(path);
    if let Err(e) = app.run("file.open", serde_json::json!({"path": name})) {
        log::warn!("{name}: {e}");
    }
}

/// Start the control server on `port`, if asked (`--control`), and hand its requests to `app`.
#[cfg(feature = "automation")]
fn start_control(
    app: WordApp,
    port: Option<u16>,
    ctx: &egui::Context,
    roots: Option<std::sync::Arc<wordcraft_engine::file_access::AutomationRoots>>,
) -> (WordApp, Option<KeyFile>) {
    match port.and_then(|port| control_server::start(port, ctx.clone(), settings_dir().as_deref(), roots)) {
        Some((rx, key_file)) => (app.with_control(rx), key_file),
        None => (app, None),
    }
}

/// Built without the `automation` feature: no control server (asking for one was refused).
#[cfg(not(feature = "automation"))]
fn start_control(
    app: WordApp,
    _port: Option<u16>,
    _ctx: &egui::Context,
    _roots: Option<std::sync::Arc<wordcraft_engine::file_access::AutomationRoots>>,
) -> (WordApp, Option<KeyFile>) {
    (app, None)
}

fn main() -> eframe::Result {
    // First, so the panic hook and every start-up warning are recorded (`logging`).
    let logger = logging::install();
    logging::install_panic_hook();
    // Dates shown are local time (#1707).
    wordcraft_engine::clock::set_local_zone(wordcraft_engine::clock::system_zone());
    let mut control_port: Option<u16> = std::env::var("WORDCRAFT_CONTROL_PORT").ok().and_then(|p| p.parse().ok());
    let mut files = Vec::new();
    let mut sample = false;
    let mut bad_control: Option<Option<String>> = None;
    let mut control_flag = false;
    let mut roots = wordcraft_engine::automation_args::AutomationArgs::new(None, None);
    let mut bad_roots: Option<String> = None;
    let (args, other_files) = text_args();
    let mut args = args.into_iter().peekable();
    while let Some(a) = args.next() {
        // `--automation-read-root DIR` / `--automation-write-root DIR`: confine every control
        // connection (docs/control-protocol.md, Confinement); what the person does stays free.
        match roots.consume(&a, &mut args) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                bad_roots = Some(e);
                continue;
            }
        }
        match a.as_str() {
            // A flag after `--control` stays a flag; a bad or missing port is an error (#1426).
            "--control" => {
                control_flag = true;
                let port = args.next_if(|p| p.strip_prefix('-').is_none_or(|n| n.starts_with(|c: char| c.is_ascii_digit())));
                match port.as_deref().and_then(|p| p.parse::<std::num::NonZeroU16>().ok()) {
                    Some(p) => control_port = Some(p.get()),
                    None => bad_control = Some(port),
                }
            }
            "--sample" => sample = true,
            "--version" => {
                println!("etch {} ({})", env!("CARGO_PKG_VERSION"), build_sha());
                return Ok(());
            }
            // Before the window opens (#968). Unlike `print!`, a closed stdout can't panic.
            "--help" | "-h" => {
                let _ = std::io::Write::write_all(&mut std::io::stdout(), USAGE.as_bytes());
                return Ok(());
            }
            _ => files.push(a),
        }
    }
    // After the loop, so `--version` later on the line still wins. Unlike `eprintln!`, never panics.
    // Built without the `automation` feature there is no control server: asking for one is an
    // error, never a window that quietly runs without the channel a script waits for.
    if !cfg!(feature = "automation") {
        let asked = if control_flag {
            Some("--control")
        } else if roots.read_root.is_some() || roots.write_root.is_some() || bad_roots.is_some() {
            Some("--automation-read-root / --automation-write-root")
        } else if std::env::var_os("WORDCRAFT_CONTROL_PORT").is_some_and(|p| !p.is_empty()) {
            Some("WORDCRAFT_CONTROL_PORT")
        } else {
            None
        };
        if let Some(what) = asked {
            let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("etch: {what}: built without automation support\n"));
            std::process::exit(2);
        }
    }
    if let Some(port) = bad_control {
        let got = port.map(|p| format!(", got {p:?}")).unwrap_or_default();
        let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("etch: --control needs a port number from 1 to 65535{got}\n"));
        std::process::exit(2);
    }
    // A root that can't be had is an error, never a control channel that quietly runs unconfined.
    let control_roots = match bad_roots
        .map_or_else(|| wordcraft_engine::file_access::AutomationRoots::new(roots.read_root.as_deref(), roots.write_root.as_deref()), Err)
    {
        Ok(r) => r,
        Err(e) => {
            let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("etch: {e}\n"));
            std::process::exit(2);
        }
    };
    // Before anything reads or writes the settings folder: find it, copying WordCraft's folder to
    // Etch's place on the first run (`settings`). Logged into the log file attached next.
    let _ = settings_dir();
    // The log file lives in the settings directory, next to the preferences; opened after the
    // arguments, so `--version` leaves no file behind. Records logged until now are written to it
    // first. Runs without preferences (agents' test runs) log to standard error only, so they
    // don't rotate away the user's own logs.
    if let Some(logger) = logger {
        match log_dir().filter(|_| prefs_enabled()) {
            Some(dir) => match logger.attach_dir(&dir) {
                Ok(path) => log::info!("Etch {} ({}), log file {}", env!("CARGO_PKG_VERSION"), build_sha(), path.display()),
                // Standard error only by now (`attach_dir` gave up on the file); unlike `eprintln!`, never panics.
                Err(e) => log::warn!("no log file: {e}"),
            },
            None => logger.no_file(),
        }
    }
    warm_start_up();
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Etch")
            .with_app_id("io.github.asystemoffields.Etch")
            .with_inner_size([1440.0, 920.0])
            .with_min_inner_size([760.0, 480.0])
            .with_drag_and_drop(true)
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false),
        ..Default::default()
    };
    if let Some(icon) = app_icon() {
        options.viewport = options.viewport.with_icon(icon);
    }
    // Before eframe creates the wgpu instance: default Windows to DirectX 12 only, or OpenGL when
    // DirectX 12 has no GPU driver or this is a 32-bit build, and compile DirectX 12 shaders with
    // FXC, which needs no DLL of ours (#603; see graphics.rs).
    #[cfg(target_os = "windows")]
    graphics::configure(&mut options, eframe::wgpu::Backends::from_env());
    // Intel Macs: Metal's compiler aborts on wgpu's indirect-validation shaders (#2652).
    #[cfg(target_os = "macos")]
    graphics::configure_indirect_validation(&mut options, graphics::skips_indirect_validation(true, cfg!(target_arch = "x86_64")));
    load_user_fonts();
    let restored = saved_window();
    if let Some(window) = restored {
        options.viewport = window.apply(options.viewport);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    // winit has no file drag-and-drop on Wayland (only on X11), so dropping files from the file
    // manager showed a "no" cursor. Run through XWayland when it's there; WORDCRAFT_WAYLAND=1
    // keeps the native Wayland backend.
    if std::env::var_os("DISPLAY").is_some() && std::env::var_os("WORDCRAFT_WAYLAND").is_none() {
        options.event_loop_builder = Some(Box::new(|b| {
            use winit::platform::x11::EventLoopBuilderExtX11;
            b.with_x11();
        }));
    }
    // Registered before the event loop starts, so it catches the Finder event that launched us as
    // well as later ones. Lives until the event loop returns; the app creator only borrows it.
    #[cfg(target_os = "macos")]
    let apple_events = apple_events::AppleEvents::install();
    #[cfg(target_os = "macos")]
    let apple_events = &apple_events;
    eframe::run_native(
        "Etch",
        options,
        Box::new(move |cc| {
            // The system region picks the new document's paper, the units and the decimal
            // separator (#838).
            let region = region::system();
            let doc = if sample { wordcraft_engine::sample::sample_document() } else { wordcraft_engine::blank_document(&region) };
            let (file_dialog, dialogs) = file_dialogs::hook(&cc.egui_ctx);
            let mut session = Session::new(doc);
            session.system_region = region;
            let mut app = WordApp::new(session, services(file_dialog, &cc.egui_ctx));
            load_prefs(&mut app);
            load_user_dictionary(&mut app);
            // A region chosen in File › Options replaces the system's for the first blank
            // document too (#838).
            if !sample && app.session.prefs.region.is_some() {
                let doc = app.session.blank_document();
                app.session.set_document(doc);
            }
            app.watch_desktop_theme(&cc.egui_ctx);
            if prefs_enabled()
                && let Some(dir) = settings_dir()
            {
                let ctx = cc.egui_ctx.clone();
                if let Err(e) = app.session.enable_recovery(dir.join("recovery"), std::sync::Arc::new(move || ctx.request_repaint())) {
                    log::warn!("local recovery unavailable: {e}");
                }
            }
            // SIGTERM, SIGINT, SIGHUP: write a recovery copy of unsaved changes, then quit.
            let ctx = cc.egui_ctx.clone();
            termination::install(move || ctx.request_repaint());
            app.ui.window = restored;
            app.integrated_titlebar = cfg!(target_os = "macos");
            #[cfg(target_os = "macos")]
            if let Some(bar) = traffic_lights::title_bar() {
                log::info!("title bar: {bar:?}");
                app.mac_titlebar = bar;
            }
            let (mut app, key_file) = start_control(app, control_port, &cc.egui_ctx, control_roots.clone());
            // Nothing to open (no files, no sample, no control channel driving the editor): start
            // on File › Home, as Word does (#766), unless the Start screen option is off. Files
            // macOS hands over later open on top of it and close it (`apple_events`).
            if files.is_empty() && other_files.is_empty() && !sample && control_port.is_none() {
                app.start_screen();
            }
            for f in files {
                if let Err(e) = app.run("file.open", serde_json::json!({"path": f})) {
                    log::warn!("{f}: {e}");
                }
            }
            for f in other_files {
                open_non_unicode(&mut app, &f);
            }
            Ok(Box::new(App(
                app,
                // With nothing restored, the window the system placed is checked all the same.
                Some(restored.unwrap_or_default()),
                key_file,
                dialogs,
                #[cfg(target_os = "macos")]
                apple_events.connect(&cc.egui_ctx),
            )))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::write_prefs;

    #[test]
    fn settings_dir_is_the_apps_folder() {
        // Worked out without `settings_dir`, which would copy the real folder.
        if let Some(super::settings::Location::Default { old, new }) = super::settings::location() {
            let name = |d: &std::path::Path| d.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            assert!(name(&new) == "etch" || name(&new) == "Etch", "{}", new.display());
            assert!(name(&old) == "wordcraft" || name(&old) == "WordCraft", "{}", old.display());
        }
    }

    #[test]
    fn failed_preferences_write_keeps_the_old_file_and_reports_the_error() {
        let dir = std::env::temp_dir().join(format!("wordcraft-prefs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ui.json");
        std::fs::write(&path, b"old").unwrap();
        write_prefs(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        // A directory squatting on the temporary name makes the preparation step fail.
        std::fs::create_dir(dir.join(format!(".ui.json.{}.tmp", std::process::id()))).unwrap();
        assert!(write_prefs(&path, b"newer").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Preferences are synchronized to disk before they replace the file (06: written and
    /// renamed, never synchronized, so a power loss could leave an empty `ui.json`). A drive that
    /// fails to synchronize fails the write, which keeps the old file and leaves no temporary one.
    #[test]
    fn preferences_are_synchronized_before_they_replace_the_file() {
        let dir = std::env::temp_dir().join(format!("wordcraft-prefs-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ui.json");
        write_prefs(&path, b"old").unwrap();
        super::FAIL_SYNC.with(|f| f.set(true));
        let e = write_prefs(&path, b"new").unwrap_err();
        assert!(e.to_string().contains("injected"), "{e}");
        assert!(!super::FAIL_SYNC.with(std::cell::Cell::get), "the write synchronized the file");
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file is left");
        write_prefs(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// Not on macOS, whose APFS refuses file names that aren't valid UTF-8 (nor Windows: not `unix`).
#[cfg(all(test, unix, not(target_os = "macos")))]
mod non_unicode_tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn a_file_whose_name_isnt_unicode_opens_and_saves_back_to_itself() {
        let dir = std::env::temp_dir().join(format!("wc-app-non-utf8-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(std::ffi::OsString::from_vec(b"caf\xe9.txt".to_vec()));
        std::fs::write(&path, "Bonjour").unwrap();

        let mut app = WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default());
        open_non_unicode(&mut app, &path);
        assert_eq!(app.session.doc.plain_text(wordcraft_doc::StoryRef::Body).trim(), "Bonjour");
        assert_eq!(app.session.path.as_deref(), Some(path.as_path()));

        app.run("caret.docEnd", serde_json::json!({})).unwrap();
        app.run("text.insert", serde_json::json!({"text": " encore"})).unwrap();
        // Plain text can't keep everything: the first Save asks, and Keep format saves.
        assert_eq!(app.run("file.save", serde_json::json!({})).unwrap()["pending"], "saveFormat");
        app.run("ui.saveFormat", serde_json::json!({"answer": "keep"})).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("Bonjour encore"));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "a second file was written");

        // A file that can't be read is a warning, as one that fails to open by name is.
        open_non_unicode(&mut app, &dir.join(std::ffi::OsString::from_vec(b"gon\xe9.txt".to_vec())));
        assert_eq!(app.session.path.as_deref(), Some(path.as_path()));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The read is capped and takes only regular files: a device that never ends, or a folder,
    /// is a warning and leaves the open document alone (it read until memory ran out).
    #[test]
    fn a_non_unicode_name_that_isnt_a_regular_file_is_not_read() {
        let dir = std::env::temp_dir().join(format!("wc-app-non-utf8-dev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let folder = dir.join(std::ffi::OsString::from_vec(b"dossi\xe9.txt".to_vec()));
        std::fs::create_dir_all(&folder).unwrap();
        let zero = dir.join(std::ffi::OsString::from_vec(b"z\xe9ro.txt".to_vec()));
        std::os::unix::fs::symlink("/dev/zero", &zero).unwrap();
        let mut app = WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default());
        app.run("text.insert", serde_json::json!({"text": "kept"})).unwrap();
        for p in [&zero, &folder] {
            open_non_unicode(&mut app, p);
            assert_eq!(app.session.path, None, "{p:?}");
            assert!(app.session.doc.plain_text(wordcraft_doc::StoryRef::Body).contains("kept"));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `--automation-read-root=<a folder whose name isn't Unicode>` was taken for a file to open,
    /// so the control channel quietly ran unconfined. Every flag spelling with such a value is an
    /// error (exit 2), as a non-Unicode `--control` port already was.
    #[test]
    fn a_flag_or_flag_value_that_isnt_unicode_is_an_error() {
        let bad = |prefix: &[u8]| {
            let mut v = prefix.to_vec();
            v.extend_from_slice(b"/tmp/caf\xe9");
            std::ffi::OsString::from_vec(v)
        };
        let os = |s: &str| std::ffi::OsString::from(s);
        for flag in ["--automation-read-root", "--automation-write-root", "--control"] {
            // `=` form.
            let e = split_args([os("--sample"), bad(format!("{flag}=").as_bytes())]).unwrap_err();
            assert!(e.contains("argument 2 is not valid Unicode"), "{flag}: {e}");
            // Space form.
            let e = split_args([os(flag), bad(b"")]).unwrap_err();
            assert!(e.contains("argument 2 is not valid Unicode"), "{flag}: {e}");
        }
        // Any other non-Unicode flag too; a plain file name still opens.
        assert!(split_args([bad(b"--whatever=")]).is_err());
        let (args, files) = split_args([os("--sample"), bad(b"")]).unwrap();
        assert_eq!(args, ["--sample"]);
        assert_eq!(files.len(), 1);
    }

    /// A file whose name isn't Unicode was opened through `file.open`'s `data`, which keeps no
    /// version of the file, so a save overwrote a change someone else made meanwhile (#813).
    #[test]
    fn a_non_unicode_file_changed_by_someone_else_is_a_conflict() {
        let dir = std::env::temp_dir().join(format!("wc-app-non-utf8-conflict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(std::ffi::OsString::from_vec(b"r\xe9sum\xe9.txt".to_vec()));
        std::fs::write(&path, "Original").unwrap();
        let mut app = WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default());
        open_non_unicode(&mut app, &path);
        assert_eq!(app.session.path.as_deref(), Some(path.as_path()));
        assert!(app.session.file_version().is_some(), "the version of the bytes read is kept");
        app.run("text.insert", serde_json::json!({"text": "Local "})).unwrap();
        std::fs::write(&path, "Someone else's change").unwrap();
        let e = app.execute("file.save", serde_json::json!({})).unwrap_err();
        assert!(e.contains("changed or is unavailable"), "{e}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Someone else's change");
        let _ = std::fs::remove_dir_all(dir);
    }
}
