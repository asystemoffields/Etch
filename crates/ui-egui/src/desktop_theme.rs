//! The desktop's own light/dark choice, for an interface setting that follows it.
//!
//! winit reports no system theme on Linux — `ActiveEventLoop::system_theme` is `None` on every
//! desktop there — so "Use system setting" has to ask the desktop itself. The XDG Desktop Portal's
//! Settings interface answers on GNOME, KDE and XFCE alike, over X11 and Wayland alike.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

/// How often [`DesktopTheme::start`] re-reads the desktop's answer. The portal also publishes a
/// change signal, but subscribing needs a connection kept alive for the life of every window; one
/// round trip every few seconds is cheap and reacts soon enough after a theme switch.
const POLL: Duration = Duration::from_secs(4);

/// Whether the desktop asks applications to draw dark. `None` when it says nothing, and when
/// there is no session bus or no portal to ask (a bare X server, a container), so the caller
/// keeps whatever it chose itself.
pub fn prefers_dark() -> Option<bool> {
    read(&mut Bus::new())
}

/// Watches the desktop and reports its choice when it changes. [`prefers_dark`] covers the
/// answer at start-up; this covers the user switching theme while the app runs.
#[derive(Default)]
pub struct DesktopTheme {
    /// `None` when no watcher thread could be started; the one-shot read still works.
    changes: Option<Receiver<Option<bool>>>,
}

impl DesktopTheme {
    /// Start watching. Nothing runs before this is called, and calling it once is enough. The
    /// first answer is sent straight away, so the caller does not have to ask for it separately.
    /// `changed` runs on the watcher's thread after each answer is sent (the app asks for a
    /// repaint there, so an idle window picks the change up). The app binary starts this; tests
    /// and headless screenshots don't, so they never depend on the machine's desktop theme.
    pub fn start(changed: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = channel();
        let started = std::thread::Builder::new().name("wordcraft-desktop-theme".into()).spawn(move || {
            // One session-bus connection for the life of the watcher, opened again only after
            // it fails.
            let mut bus = Bus::new();
            let mut last = read(&mut bus);
            if tx.send(last).is_err() {
                return;
            }
            changed();
            loop {
                std::thread::sleep(POLL);
                let now = read(&mut bus);
                if now != last {
                    last = now;
                    if tx.send(now).is_err() {
                        return;
                    }
                    changed();
                }
            }
        });
        Self { changes: started.ok().map(|_| rx) }
    }

    /// Whether a watcher thread is running.
    pub fn watching(&self) -> bool {
        self.changes.is_some()
    }

    /// The desktop's new answer if it changed since the last call, to be polled once a frame:
    /// `Some(None)` when it changed to no preference, which the caller applies too (dropping it
    /// left "Use system setting" stuck on dark). Several changes in one frame collapse to the
    /// newest.
    pub fn take_change(&mut self) -> Option<Option<bool>> {
        let Some(changes) = &self.changes else { return None };
        let mut newest = None;
        loop {
            match changes.try_recv() {
                Ok(changed) => newest = Some(changed),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return newest,
            }
        }
    }
}

/// The watcher's session-bus connection, kept between polls.
#[cfg(target_os = "linux")]
struct Bus(Option<zbus::blocking::Connection>);

#[cfg(target_os = "linux")]
impl Bus {
    /// No connection yet: the first read opens one.
    fn new() -> Self {
        Bus(None)
    }
}

/// Elsewhere there is no session bus to keep.
#[cfg(not(target_os = "linux"))]
struct Bus;

#[cfg(not(target_os = "linux"))]
impl Bus {
    fn new() -> Self {
        Bus
    }
}

/// Ask the portal once, over `bus`'s connection (opened when there is none). D-Bus calls
/// block, so this belongs off the UI thread. A connection whose calls all fail is dropped, so
/// the next poll opens a fresh one (the bus or portal may have restarted).
///
/// GNOME's own `color-scheme` comes first: there `default` is the Light style, which the
/// standard key reports only as "no preference". The standard key (portal Settings namespace
/// `org.freedesktop.appearance`, a `u32`) answers on KDE and the rest.
#[cfg(target_os = "linux")]
fn read(bus: &mut Bus) -> Option<bool> {
    const PORTAL: &str = "org.freedesktop.portal.Desktop";
    const SETTINGS: &str = "org.freedesktop.portal.Settings";
    const SCHEMAS: [(&str, &str); 2] = [("org.gnome.desktop.interface", "color-scheme"), ("org.freedesktop.appearance", "color-scheme")];
    if bus.0.is_none() {
        bus.0 = zbus::blocking::Connection::session().ok();
    }
    let connection = bus.0.as_ref()?;
    let mut answered = false;
    for (schema, key) in SCHEMAS {
        let Ok(reply) = connection.call_method(Some(PORTAL), "/org/freedesktop/portal/desktop", Some(SETTINGS), "ReadOne", &(schema, key)) else {
            continue;
        };
        answered = true;
        let Ok((answer,)) = reply.body().deserialize::<(zbus::zvariant::OwnedValue,)>() else { continue };
        // Some portals wrap the value in one more variant.
        let value = match &*answer {
            zbus::zvariant::Value::Value(inner) => &**inner,
            v => v,
        };
        if let Ok(scheme) = <&str>::try_from(value) {
            return parse(scheme);
        }
        if let Ok(n) = u32::try_from(value) {
            return parse_appearance(n);
        }
    }
    if !answered {
        bus.0 = None;
    }
    None
}

/// GNOME's `color-scheme` (`org.gnome.desktop.interface`). `default` is what GNOME's Settings
/// sets for its Light style, so it reads as light.
#[cfg(target_os = "linux")]
fn parse(scheme: &str) -> Option<bool> {
    match scheme {
        "prefer-dark" => Some(true),
        "prefer-light" | "default" => Some(false),
        _ => None,
    }
}

/// The standard portal `color-scheme` (`org.freedesktop.appearance`): 1 prefers dark, 2 prefers
/// light, 0 (and anything else) states no preference.
#[cfg(target_os = "linux")]
fn parse_appearance(n: u32) -> Option<bool> {
    match n {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// macOS and Windows answer through winit, which the caller already has.
#[cfg(not(target_os = "linux"))]
fn read(_bus: &mut Bus) -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GNOME answers with one of three strings; `default` is its Light style.
    #[test]
    #[cfg(target_os = "linux")]
    fn the_gnome_answer_reads_as_a_choice() {
        assert_eq!(parse("prefer-dark"), Some(true));
        assert_eq!(parse("prefer-light"), Some(false));
        assert_eq!(parse("default"), Some(false));
        assert_eq!(parse(""), None);
        assert_eq!(parse("prefer-sepia"), None);
    }

    /// The standard portal key is a number: 1 dark, 2 light, 0 no preference.
    #[test]
    #[cfg(target_os = "linux")]
    fn the_standard_answer_reads_as_a_choice_or_as_none() {
        assert_eq!(parse_appearance(1), Some(true));
        assert_eq!(parse_appearance(2), Some(false));
        assert_eq!(parse_appearance(0), None);
        assert_eq!(parse_appearance(7), None);
    }

    /// Going from dark to "no preference" is a change the window applies.
    #[test]
    fn no_preference_is_still_a_change() {
        let (tx, rx) = channel();
        let mut w = DesktopTheme { changes: Some(rx) };
        assert_eq!(w.take_change(), None, "nothing sent, nothing changed");
        tx.send(Some(true)).unwrap();
        assert_eq!(w.take_change(), Some(Some(true)));
        tx.send(None).unwrap();
        assert_eq!(w.take_change(), Some(None));
        tx.send(Some(true)).unwrap();
        tx.send(Some(false)).unwrap();
        assert_eq!(w.take_change(), Some(Some(false)), "the newest of several wins");
    }

    /// Off Linux the caller already has winit's answer, so this adds nothing.
    #[test]
    #[cfg(not(target_os = "linux"))]
    fn other_platforms_answer_through_winit() {
        assert_eq!(prefers_dark(), None);
    }

    /// A watcher that was never started is inert rather than a panic.
    #[test]
    fn a_watcher_without_a_thread_is_inert() {
        assert_eq!(DesktopTheme::default().take_change(), None);
    }

    /// A new app doesn't watch the desktop (the app binary starts the watcher), so tests and
    /// headless screenshots never depend on the host's theme or spawn D-Bus pollers.
    #[test]
    fn a_new_app_does_not_watch_the_desktop() {
        let app = crate::WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::new()), Default::default());
        assert!(!app.desktop_theme.watching());
    }
}
