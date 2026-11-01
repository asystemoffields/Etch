//! Being terminated by the system, or panicking, without losing unsaved changes.
//!
//! SIGTERM (logout, shutdown, `kill`), SIGINT (Ctrl+C in the terminal that started the app) and
//! SIGHUP (that terminal closed) used to end the process on the spot: up to 10 seconds of typing
//! since the last periodic recovery copy were lost. Now a watcher thread asks the window to write
//! an up-to-date recovery copy synchronously (`Session::emergency_snapshot`) and quit, offering
//! the copy at the next start (docs/recovery.md). A second signal, or no quit within
//! [`GRACE`], ends the process at once, as the signal would have.
//!
//! A panic asks for the same copy ([`request_snapshot`], from the panic hook): the hook only sets
//! a flag. It never takes a lock or touches the session, which the panicking thread may be in the
//! middle of changing (a lock it holds would deadlock, a poisoned one would panic again inside
//! the hook, which aborts). The UI thread, which owns the session, writes the copy: on its next
//! frame, or while a panic unwinds it, when the app is dropped ([`crate::App`]'s `Drop`).
//!
//! Windows: winit doesn't pass on the session-end messages (`WM_QUERYENDSESSION`/`WM_ENDSESSION`),
//! so logging off relies on the periodic copy there.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

/// How long the window has to write the copy and quit after a signal.
#[cfg(unix)]
const GRACE: Duration = Duration::from_secs(if cfg!(test) { 3600 } else { 20 });

/// The longest an emergency copy waits for the recovery worker.
pub const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);

/// A panic asked for a recovery copy.
static SNAPSHOT: AtomicBool = AtomicBool::new(false);
/// The signal that asked the app to quit (0: none).
static SIGNAL: AtomicI32 = AtomicI32::new(0);

/// Ask for an emergency recovery copy (the panic hook). Lock-free and infallible.
pub fn request_snapshot() {
    SNAPSHOT.store(true, Ordering::SeqCst);
}

/// Whether a copy was asked for since the last call.
pub fn take_snapshot_request() -> bool {
    SNAPSHOT.swap(false, Ordering::SeqCst)
}

/// The signal that asked the app to quit, if one did.
pub fn quit_signal() -> Option<i32> {
    match SIGNAL.load(Ordering::SeqCst) {
        0 => None,
        s => Some(s),
    }
}

/// Watch for SIGTERM, SIGINT and SIGHUP; `wake` makes the window run a frame to act on it.
#[cfg(unix)]
pub fn install(wake: impl Fn() + Send + 'static) {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    let mut signals = match signal_hook::iterator::Signals::new([SIGTERM, SIGINT, SIGHUP]) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("termination signals aren't watched (no recovery copy on logout or kill): {e}");
            return;
        }
    };
    let watcher = std::thread::Builder::new().name("wordcraft-signals".into()).spawn(move || {
        for signal in signals.forever() {
            if SIGNAL.swap(signal, Ordering::SeqCst) != 0 {
                log::warn!("signal {signal} again: quitting now");
                std::process::exit(128 + signal);
            }
            log::info!("signal {signal}: writing a recovery copy of unsaved changes, then quitting");
            wake();
            // A window that can't run its frame (hung, or the event loop stopped) mustn't keep
            // the process alive past the system's patience.
            let _ = std::thread::Builder::new().name("wordcraft-quit-watchdog".into()).spawn(move || {
                std::thread::sleep(GRACE);
                log::error!("signal {signal}: the window didn't quit within {} s; quitting now", GRACE.as_secs());
                std::process::exit(128 + signal);
            });
        }
    });
    if let Err(e) = watcher {
        log::warn!("termination signals aren't watched: {e}");
    }
}

#[cfg(not(unix))]
pub fn install(_wake: impl Fn() + Send + 'static) {}

/// Write the emergency recovery copy of the app's unsaved changes now (logged, never fails).
pub fn snapshot(app: &mut wordcraft_ui_egui::WordApp) {
    match app.session.emergency_snapshot(SNAPSHOT_TIMEOUT) {
        Ok(true) => log::info!("recovery copy of unsaved changes written"),
        Ok(false) => {}
        Err(e) => log::error!("recovery copy of unsaved changes not written: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panic hook's request is a flag the UI thread takes once.
    #[test]
    fn a_snapshot_request_is_taken_once() {
        request_snapshot();
        assert!(take_snapshot_request());
        assert!(!take_snapshot_request());
    }

    /// A signal makes the window write an emergency copy and quit; the copy is on disk when
    /// `snapshot` returns. Synthetic text, a temporary recovery folder.
    #[cfg(unix)]
    #[test]
    fn a_termination_signal_asks_the_window_to_quit() {
        let (tx, rx) = std::sync::mpsc::channel();
        install(move || {
            let _ = tx.send(());
        });
        assert_eq!(quit_signal(), None);
        raise_sighup();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(quit_signal(), Some(signal_hook::consts::SIGHUP));

        let dir = std::env::temp_dir().join(format!("wordcraft-termination-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app =
            wordcraft_ui_egui::WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::new()), wordcraft_ui_egui::Services::default());
        app.session.enable_recovery(dir.join("recovery"), std::sync::Arc::new(|| {})).unwrap();
        app.execute("text.insert", serde_json::json!({"text": "Synthetic words typed just before logout"})).unwrap();
        snapshot(&mut app);
        assert_eq!(app.session.emergency_snapshot(SNAPSHOT_TIMEOUT), Ok(true), "already up to date");
        drop(app);
        let mut s = wordcraft_engine::Session::new(wordcraft_doc::Document::new());
        s.enable_recovery(dir.join("recovery"), std::sync::Arc::new(|| {})).unwrap();
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while !(s.recovery_ready() && !s.recovery_busy()) {
            assert!(std::time::Instant::now() < end);
            s.recovery_tick();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(s.recovery_documents().len(), 1);
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Send SIGHUP to this process (handled by the watcher above, so it doesn't end the tests).
    #[cfg(unix)]
    fn raise_sighup() {
        signal_hook::low_level::raise(signal_hook::consts::SIGHUP).unwrap();
    }
}
