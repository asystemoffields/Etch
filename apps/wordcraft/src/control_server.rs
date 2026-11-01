//! Loopback JSON-lines control server: one request per line, one reply per line.
//! Only in builds with the `automation` feature (tests and development; never release builds).
//!
//! Every request needs the window's key (`docs/control-protocol.md`, Keys): once the port is
//! bound, the app writes a random key to `<settings>/control-key.<instance>` (mode 0600) and
//! removes the file when the window closes.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wordcraft_engine::file_access::AutomationRoots;
use wordcraft_ui_egui::ControlRequest;

/// Longer request lines are refused (and the connection closed).
const MAX_LINE: usize = 1 << 20;

/// Until a connection has sent an authorised request, its lines may be at most this long: room
/// for `auth` with the key and two root paths, while a client without the key can't make the app
/// hold a [`MAX_LINE`] buffer per connection.
const MAX_LINE_BEFORE_AUTH: usize = 64 << 10;

/// At most this many connections are served at once (each has a thread and up to a
/// [`MAX_LINE`] buffer); more are refused and closed, so a local process can't exhaust threads
/// or memory by opening connections.
const MAX_CONNECTIONS: usize = 16;

/// A connection that sends no authorised request in this time is closed.
const FIRST_REQUEST: Duration = Duration::from_secs(30);

/// The key file of a running control server; [`KeyFile::remove`] it when the window closes.
pub struct KeyFile(PathBuf);

impl KeyFile {
    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn remove(&self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Bind `127.0.0.1:port`, write the key file into `settings` and serve. `None` when the port
/// cannot be bound or there is no random key (the control channel is off); the key file is
/// `None` when it could not be written (requests are then refused).
///
/// `floor`: the roots every connection is confined to from the start (`wordcraft
/// --automation-read-root DIR --automation-write-root DIR`); a connection can only narrow them.
pub fn start(
    port: u16,
    ctx: egui::Context,
    settings: Option<&Path>,
    floor: Option<Arc<AutomationRoots>>,
) -> Option<(Receiver<ControlRequest>, Option<KeyFile>)> {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            log::error!("control server failed to bind 127.0.0.1:{port}: {e}");
            return None;
        }
    };
    let key = match random_key() {
        Ok(k) => k,
        Err(e) => {
            log::error!("control server off: no random key ({e})");
            return None;
        }
    };
    let key_file = match settings {
        Some(dir) => {
            let p = wordcraft_control_key::key_path(dir, port);
            match write_key_file(&p, &key) {
                Ok(()) => Some(KeyFile(p)),
                Err(e) => {
                    log::error!("control key file not written ({e}): requests will be refused");
                    None
                }
            }
        }
        None => {
            log::error!("no settings folder for the control key file: requests will be refused");
            None
        }
    };
    match &key_file {
        Some(f) => log::info!("control server listening on 127.0.0.1:{port}, key in {}", f.path().display()),
        None => log::info!("control server listening on 127.0.0.1:{port}"),
    }
    Some((serve_on(listener, ctx, key, FIRST_REQUEST, floor), key_file))
}

/// A new key: 32 random bytes from the operating system, as hex.
fn random_key() -> Result<String, String> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|e| e.to_string())?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Write the key file: a stale one (from a run that did not exit cleanly) is replaced; on Unix
/// the new file is created with mode 0600.
fn write_key_file(p: &Path, key: &str) -> Result<(), String> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
    }
    let _ = std::fs::remove_file(p);
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    let mut f = open.open(p).map_err(|e| format!("cannot create {}: {e}", p.display()))?;
    f.write_all(key.as_bytes()).map_err(|e| format!("cannot write {}: {e}", p.display()))
}

/// Compare a key with the window's in time that does not depend on where they differ. An
/// empty key never matches.
fn keys_match(given: &str, key: &str) -> bool {
    let (a, b) = (given.as_bytes(), key.as_bytes());
    !b.is_empty() && a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn serve_on(
    listener: TcpListener,
    ctx: egui::Context,
    key: String,
    first: Duration,
    floor: Option<Arc<AutomationRoots>>,
) -> Receiver<ControlRequest> {
    let (tx, rx) = channel::<ControlRequest>();
    let key: Arc<str> = key.into();
    let live = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            // Count the connection before its thread starts; the guard gives the slot back when
            // the thread ends (or never starts).
            if live.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                live.fetch_sub(1, Ordering::SeqCst);
                let _ = writeln!(stream, "{}", json!({"ok": false, "error": "too many connections"}));
                continue;
            }
            let slot = Slot(live.clone());
            let (tx, ctx, key, floor) = (tx.clone(), ctx.clone(), key.clone(), floor.clone());
            let _ = std::thread::Builder::new().name("wordcraft-control".into()).spawn(move || {
                let _slot = slot;
                serve(stream, tx, ctx, &key, first, floor);
            });
        }
    });
    rx
}

/// One live connection's place in the count; dropping it frees the place.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The read side of a connection with an optional overall deadline: each read waits only for
/// the time left, so blank lines, bad JSON or a slow drip of bytes cannot extend it.
struct Timed {
    s: TcpStream,
    deadline: Option<Instant>,
}

impl Read for Timed {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(d) = self.deadline {
            let left = d.checked_duration_since(Instant::now()).filter(|l| !l.is_zero()).ok_or(std::io::ErrorKind::TimedOut)?;
            self.s.set_read_timeout(Some(left))?;
        }
        self.s.read(buf)
    }
}

impl Timed {
    fn set_deadline(&mut self, d: Option<Instant>) {
        self.deadline = d;
        if d.is_none() {
            let _ = self.s.set_read_timeout(None);
        }
    }
}

/// The next line (trimmed), `None` at the end of the stream or after a read error. A line over
/// `max` bytes is an error as soon as `max + 1` bytes have arrived: no more is read or held.
fn read_line(r: &mut BufReader<Timed>, max: usize) -> Option<Result<String, &'static str>> {
    let mut buf = Vec::new();
    let n = r.by_ref().take(max as u64 + 1).read_until(b'\n', &mut buf).ok()?;
    if n == 0 {
        return None;
    }
    if buf.len() > max {
        return Some(Err("line too long"));
    }
    Some(String::from_utf8(buf).map(|s| s.trim().to_string()).map_err(|_| "not UTF-8"))
}

/// Read and drop the rest of an oversize line, so closing the socket does not reset the
/// connection before the client has read the error. At most `left` bytes, within 2 s, a buffer at
/// a time (nothing is kept).
fn discard_rest_of_line(r: &mut BufReader<Timed>, mut left: usize) {
    r.get_mut().set_deadline(Some(Instant::now() + Duration::from_secs(2)));
    while left > 0 {
        let (used, done) = match r.fill_buf() {
            Ok([]) | Err(_) => return,
            Ok(b) => match b.iter().position(|&c| c == b'\n') {
                Some(i) => (i + 1, true),
                None => (b.len(), false),
            },
        };
        r.consume(used);
        if done {
            return;
        }
        left = left.saturating_sub(used);
    }
}

/// The roots an `auth` or `confine` request asks for (`params.readRoot`, `params.writeRoot`, each a
/// folder or null): `None` when it names neither (nothing changes), else the new roots, which must
/// narrow `current`.
fn requested_roots(params: &Value, current: Option<&Arc<AutomationRoots>>) -> Result<Option<Arc<AutomationRoots>>, String> {
    let root = |name: &str| -> Result<Option<&Path>, String> {
        match params.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(p)) => Ok(Some(Path::new(p.as_str()))),
            Some(_) => Err(format!("`{name}` must be a folder path or null")),
        }
    };
    if params.get("readRoot").is_none() && params.get("writeRoot").is_none() {
        return Ok(None);
    }
    AutomationRoots::narrowed(current, root("readRoot")?, root("writeRoot")?).map(Some)
}

/// What `auth` and `confine` answer: the roots now in force on the connection.
fn roots_result(roots: Option<&Arc<AutomationRoots>>) -> Value {
    let shown = |p: Option<&Path>| p.map(|p| p.to_string_lossy().into_owned());
    match roots {
        Some(r) => json!({"confined": true, "readRoot": shown(r.read_root()), "writeRoot": shown(r.write_root())}),
        None => json!({"confined": false, "readRoot": null, "writeRoot": null}),
    }
}

fn serve(stream: TcpStream, tx: Sender<ControlRequest>, ctx: egui::Context, key: &str, first: Duration, floor: Option<Arc<AutomationRoots>>) {
    let Ok(read) = stream.try_clone() else { return };
    let mut out = stream;
    // One overall deadline from accept to the first authorised request; after it, none.
    let mut reader = BufReader::new(Timed { s: read, deadline: Some(Instant::now() + first) });
    let mut authorised = false;
    // The key an `auth` request gave this connection.
    let mut conn_key: Option<String> = None;
    // The folders this connection's commands may read and write: the app's floor, narrowed by
    // `auth` / `confine`, never widened.
    let mut roots = floor;
    loop {
        let max = if authorised { MAX_LINE } else { MAX_LINE_BEFORE_AUTH };
        let Some(line) = read_line(&mut reader, max) else { break };
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                let _ = writeln!(out, "{}", json!({"ok": false, "error": e}));
                if e == "line too long" {
                    // Read and dropped, never held; before the key, less of it (and no more of our time).
                    discard_rest_of_line(&mut reader, if authorised { 64 << 20 } else { MAX_LINE });
                }
                break;
            }
        };
        if line.is_empty() {
            continue;
        }
        let msg = match serde_json::from_str::<Value>(&line) {
            Ok(m) => m,
            // Close on a line that isn't JSON: an HTTP request (say, a web page's `fetch` to this
            // port) starts with a request line that never parses, so it can't carry a command in
            // its body, even before the key is checked.
            Err(e) => {
                let _ = writeln!(out, "{}", json!({"ok": false, "error": format!("bad JSON: {e}; closing the connection")}));
                break;
            }
        };
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("").to_string();
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        // The top-level `key`; `auth` may carry it in its params. For other methods `params.key`
        // is an ordinary parameter (`ui.key {"key": "Enter"}`).
        let given =
            msg.get("key").and_then(Value::as_str).or_else(|| if method == "auth" { params.get("key").and_then(Value::as_str) } else { None });
        let mut reply = |mut v: Value| -> bool {
            if let Some(o) = v.as_object_mut() {
                o.insert("id".into(), id.clone());
            }
            writeln!(out, "{v}").is_ok()
        };
        // Checked on every request.
        let k = given.or(conn_key.as_deref()).unwrap_or("");
        if !keys_match(k, key) {
            let _ = reply(json!({"ok": false, "error": "unauthorized"}));
            break;
        }
        if !authorised {
            authorised = true;
            reader.get_mut().set_deadline(None);
        }
        if method == "auth" || method == "confine" {
            if method == "auth" {
                conn_key = Some(k.to_string());
            }
            // A client that asked to be confined is never left running unconfined: a request
            // for roots that can't be had closes the connection.
            match requested_roots(&params, roots.as_ref()) {
                Ok(Some(narrower)) => roots = Some(narrower),
                Ok(None) => {}
                Err(e) => {
                    let _ = reply(json!({"ok": false, "error": e}));
                    break;
                }
            }
            if !reply(json!({"ok": true, "result": roots_result(roots.as_ref())})) {
                break;
            }
            continue;
        }
        let (req, rrx) = ControlRequest::new(method, params);
        let req = req.confined(roots.clone());
        if tx.send(req).is_err() {
            break;
        }
        ctx.request_repaint();
        let r = rrx.recv_timeout(Duration::from_secs(60)).unwrap_or_else(|_| json!({"ok": false, "error": "timeout"}));
        if !reply(r) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;

    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn boot_with(first: Duration) -> (u16, Receiver<ControlRequest>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        (port, serve_on(listener, egui::Context::default(), KEY.to_string(), first, None))
    }

    fn boot() -> (u16, Receiver<ControlRequest>) {
        boot_with(FIRST_REQUEST)
    }

    /// Answer the next request that reaches the UI with `ok`, and return it.
    fn answer(rx: &Receiver<ControlRequest>) -> ControlRequest {
        let req = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let _ = req.reply.send(json!({"ok": true, "result": {}}));
        req
    }

    /// One connection, several lines.
    struct Conn(BufReader<TcpStream>, TcpStream);

    impl Conn {
        fn open(port: u16) -> Self {
            let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            Self(BufReader::new(s.try_clone().unwrap()), s)
        }
        fn send(&mut self, line: &str) {
            writeln!(self.1, "{line}").unwrap();
        }
        fn recv(&mut self) -> Value {
            let mut r = String::new();
            let _ = self.0.read_line(&mut r);
            serde_json::from_str(&r).unwrap_or_default()
        }
        fn call(&mut self, line: &str) -> Value {
            self.send(line);
            self.recv()
        }
        /// The server closed the connection.
        fn closed(&mut self) -> bool {
            let mut r = String::new();
            matches!(self.0.read_line(&mut r), Ok(0))
        }
    }

    #[test]
    fn a_request_without_the_key_is_refused_and_closed() {
        let (port, rx) = boot();
        let mut c = Conn::open(port);
        let r = c.call(r#"{"id":1,"method":"document.inspect"}"#);
        assert_eq!((r["ok"].clone(), r["error"].clone(), r["id"].clone()), (json!(false), json!("unauthorized"), json!(1)));
        assert!(c.closed());
        assert!(rx.try_recv().is_err(), "nothing reached the UI");
    }

    #[test]
    fn a_wrong_key_is_refused_and_closed() {
        let (port, rx) = boot();
        for key in ["nope", &KEY[..63], &format!("{KEY}0"), &KEY.to_uppercase()] {
            let mut c = Conn::open(port);
            assert_eq!(c.call(&format!(r#"{{"id":1,"key":"{key}","method":"document.inspect"}}"#))["error"], "unauthorized", "{key}");
            assert!(c.closed());
        }
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn the_key_on_each_request() {
        let (port, rx) = boot();
        let mut c = Conn::open(port);
        c.send(&format!(r#"{{"id":7,"key":"{KEY}","method":"document.inspect","params":{{"text":true}}}}"#));
        let req = answer(&rx);
        assert_eq!((req.method.as_str(), &req.params), ("document.inspect", &json!({"text": true})));
        let r = c.recv();
        assert_eq!((r["ok"].clone(), r["id"].clone()), (json!(true), json!(7)));
        // Every request needs it: one without the key is refused, also after good ones.
        assert_eq!(c.call(r#"{"id":8,"method":"document.inspect"}"#)["error"], "unauthorized");
        assert!(c.closed());
    }

    #[test]
    fn auth_once_per_connection() {
        let (port, rx) = boot();
        let mut c = Conn::open(port);
        assert_eq!(c.call(&format!(r#"{{"id":1,"method":"auth","key":"{KEY}"}}"#))["ok"], true);
        for id in 2..4 {
            c.send(&format!(r#"{{"id":{id},"method":"document.inspect"}}"#));
            answer(&rx);
            assert_eq!(c.recv()["ok"], true);
        }
        // `params.key` works for `auth` too.
        let mut c = Conn::open(port);
        assert_eq!(c.call(&format!(r#"{{"id":1,"method":"auth","params":{{"key":"{KEY}"}}}}"#))["ok"], true);
        // A wrong key on an authenticated connection is still refused, and closes it.
        assert_eq!(c.call(r#"{"id":2,"key":"nope","method":"document.inspect"}"#)["error"], "unauthorized");
        assert!(c.closed());
        // A failed auth closes the connection.
        let mut c = Conn::open(port);
        assert_eq!(c.call(r#"{"id":1,"method":"auth","key":"nope"}"#)["error"], "unauthorized");
        assert!(c.closed());
    }

    #[test]
    fn params_key_is_only_a_secret_for_auth() {
        let (port, rx) = boot();
        // `params.key` does not authenticate other methods.
        let mut c = Conn::open(port);
        assert_eq!(c.call(&format!(r#"{{"id":1,"method":"document.inspect","params":{{"key":"{KEY}"}}}}"#))["error"], "unauthorized");
        // It is an ordinary parameter for them (`ui.key` presses a key).
        let mut c = Conn::open(port);
        c.send(&format!(r#"{{"id":1,"key":"{KEY}","method":"ui.key","params":{{"key":"Enter"}}}}"#));
        let req = answer(&rx);
        assert_eq!((req.method.as_str(), &req.params), ("ui.key", &json!({"key": "Enter"})));
        assert_eq!(c.recv()["ok"], true);
    }

    /// Connections past the cap are refused and closed; closing one frees its place.
    #[test]
    fn connections_are_capped() {
        let (port, rx) = boot();
        let mut open: Vec<Conn> = (0..MAX_CONNECTIONS).map(|_| Conn::open(port)).collect();
        // All of them are being served (each answers an authorised request).
        for c in &mut open {
            c.send(&format!(r#"{{"id":1,"key":"{KEY}","method":"document.inspect"}}"#));
            answer(&rx);
            assert_eq!(c.recv()["ok"], true);
        }
        let mut extra = Conn::open(port);
        assert_eq!(extra.recv()["error"], "too many connections");
        assert!(extra.closed());
        assert!(rx.try_recv().is_err(), "nothing from the refused connection reached the UI");
        // One closes: a new connection is served again (the slot frees once its thread ends).
        drop(open.pop());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut c = Conn::open(port);
            // Opened before the slot was free, the connection is refused and closed by the
            // server, and writing to it can fail (a broken pipe): try again.
            let sent = writeln!(c.1, r#"{{"id":2,"key":"{KEY}","method":"document.inspect"}}"#).is_ok();
            if sent && let Ok(req) = rx.recv_timeout(Duration::from_millis(200)) {
                let _ = req.reply.send(json!({"ok": true, "result": {}}));
                assert_eq!(c.recv()["ok"], true);
                break;
            }
            assert!(Instant::now() < deadline, "a freed slot is reused");
        }
    }

    #[test]
    fn a_line_over_1_mib_is_refused_and_closed() {
        let (port, rx) = boot();
        let mut c = Conn::open(port);
        let big = format!(r#"{{"id":1,"key":"{KEY}","method":"document.inspect","pad":"{}"}}"#, "x".repeat(MAX_LINE));
        assert_eq!(c.call(&big)["error"], "line too long");
        assert!(c.closed());
        // After `auth` too.
        let mut c = Conn::open(port);
        assert_eq!(c.call(&format!(r#"{{"id":1,"method":"auth","key":"{KEY}"}}"#))["ok"], true);
        assert_eq!(c.call(&big)["error"], "line too long");
        assert!(c.closed());
        assert!(rx.try_recv().is_err());
    }

    /// Before the key, a line may be only [`MAX_LINE_BEFORE_AUTH`] long: the server answers as
    /// soon as that much has arrived (it holds no more), and closes the connection.
    #[test]
    fn a_long_line_before_the_key_is_refused_early() {
        let (port, rx) = boot();
        let mut c = Conn::open(port);
        // No newline, and the connection stays open: the answer can't wait for the line's end.
        c.1.write_all(&vec![b'x'; MAX_LINE_BEFORE_AUTH + 4096]).unwrap();
        let t0 = Instant::now();
        assert_eq!(c.recv()["error"], "line too long");
        assert!(t0.elapsed() < Duration::from_secs(3));
        assert!(c.closed());
        // The same with the key on the line: the key isn't read before the line is.
        let mut c = Conn::open(port);
        let big = format!(r#"{{"id":1,"key":"{KEY}","method":"document.inspect","pad":"{}"}}"#, "x".repeat(MAX_LINE_BEFORE_AUTH));
        assert_eq!(c.call(&big)["error"], "line too long");
        assert!(c.closed());
        assert!(rx.try_recv().is_err(), "nothing reached the UI");
        // After `auth`, lines up to MAX_LINE are taken.
        let mut c = Conn::open(port);
        assert_eq!(c.call(&format!(r#"{{"id":1,"method":"auth","key":"{KEY}"}}"#))["ok"], true);
        c.send(&format!(r#"{{"id":2,"method":"text.insert","params":{{"text":"{}"}}}}"#, "y".repeat(4 * MAX_LINE_BEFORE_AUTH)));
        let req = answer(&rx);
        assert_eq!(req.params["text"].as_str().map(str::len), Some(4 * MAX_LINE_BEFORE_AUTH));
        assert_eq!(c.recv()["ok"], true);
    }

    /// Folders for the confinement tests: `in`, `in/sub`, `outside`, and `in/escape`, a link to
    /// `outside`.
    fn confinement_dirs(name: &str) -> std::path::PathBuf {
        let dir = temp_dir(name);
        for d in ["in/sub", "outside"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("outside"), dir.join("in/escape")).unwrap();
        std::fs::canonicalize(dir).unwrap()
    }

    fn auth_with(roots: Value) -> String {
        json!({"id": 1, "method": "auth", "key": KEY, "params": roots}).to_string()
    }

    /// The roots of the next request that reaches the UI.
    fn roots_of(c: &mut Conn, rx: &Receiver<ControlRequest>) -> Option<Arc<AutomationRoots>> {
        c.send(r#"{"id":9,"method":"document.inspect"}"#);
        let req = answer(rx);
        assert_eq!(c.recv()["ok"], true);
        req.roots
    }

    /// `auth` with `readRoot`/`writeRoot` confines the connection: every request it sends reaches
    /// the UI with those roots, and `auth` answers with them. Without them, nothing is confined.
    #[test]
    fn auth_confines_the_connection() {
        let dir = confinement_dirs("auth");
        let (port, rx) = boot();
        let s = |p: &str| dir.join(p).to_string_lossy().into_owned();
        let mut c = Conn::open(port);
        let r = c.call(&auth_with(json!({"readRoot": s("in"), "writeRoot": s("in/sub")})));
        assert_eq!(r["result"], json!({"confined": true, "readRoot": s("in"), "writeRoot": s("in/sub")}), "{r}");
        let roots = roots_of(&mut c, &rx).unwrap();
        assert_eq!((roots.read_root(), roots.write_root()), (Some(dir.join("in").as_path()), Some(dir.join("in/sub").as_path())));
        // A request carrying the key itself is confined all the same.
        c.send(&format!(r#"{{"id":3,"key":"{KEY}","method":"file.saveAs"}}"#));
        assert!(answer(&rx).roots.is_some());
        assert_eq!(c.recv()["ok"], true);
        // Another connection is not.
        let mut other = Conn::open(port);
        assert_eq!(other.call(&format!(r#"{{"id":1,"method":"auth","key":"{KEY}"}}"#))["result"]["confined"], false);
        assert!(roots_of(&mut other, &rx).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A connection's roots only narrow: `confine` (or another `auth`) inside them is taken; a
    /// folder outside them, through a link out, a kind of access they don't grant, a relative
    /// path or a non-string is refused and closes the connection, so a client that asked to be
    /// confined never runs wider than it asked. `auth` without roots keeps them.
    #[test]
    fn the_client_can_narrow_but_never_widen() {
        let dir = confinement_dirs("narrow");
        let (port, rx) = boot();
        let s = |p: &str| dir.join(p).to_string_lossy().into_owned();
        let confined = |c: &mut Conn| {
            assert_eq!(c.call(&auth_with(json!({"readRoot": s("in"), "writeRoot": null})))["ok"], true);
        };
        let mut c = Conn::open(port);
        confined(&mut c);
        assert_eq!(c.call(&format!(r#"{{"id":2,"method":"auth","key":"{KEY}"}}"#))["result"]["readRoot"], s("in"), "auth alone keeps them");
        let r = c.call(&json!({"id": 3, "method": "confine", "params": {"readRoot": s("in/sub")}}).to_string());
        assert_eq!(r["result"]["readRoot"], s("in/sub"), "{r}");
        assert_eq!(roots_of(&mut c, &rx).unwrap().read_root(), Some(dir.join("in/sub").as_path()));
        let mut refused: Vec<Value> = vec![
            json!({"readRoot": s("outside")}),
            json!({"readRoot": s("in/sub/../../outside")}),
            json!({"readRoot": dir.to_string_lossy()}),
            json!({"writeRoot": s("in")}),
            json!({"readRoot": "in"}),
            json!({"readRoot": 7}),
        ];
        #[cfg(unix)]
        refused.push(json!({"readRoot": s("in/escape")}));
        for (method, params) in refused.iter().flat_map(|p| [("confine", p.clone()), ("auth", p.clone())]) {
            let mut c = Conn::open(port);
            confined(&mut c);
            let mut msg = json!({"id": 4, "method": method, "params": params});
            if method == "auth" {
                msg["key"] = json!(KEY);
            }
            let r = c.call(&msg.to_string());
            assert_eq!(r["ok"], false, "{msg}: {r}");
            assert!(c.closed(), "{msg}: closed");
        }
        assert!(rx.try_recv().is_err(), "nothing from a refused connection reached the UI");
        // Without the key, `confine` is refused like any request.
        let mut c = Conn::open(port);
        assert_eq!(c.call(&json!({"id": 1, "method": "confine", "params": {"readRoot": s("in")}}).to_string())["error"], "unauthorized");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The app's own roots (`wordcraft --automation-read-root …`) confine every connection from
    /// the start; a client can narrow them, not leave them.
    #[test]
    fn the_app_roots_are_a_floor() {
        let dir = confinement_dirs("floor");
        let floor = AutomationRoots::new(Some(&dir.join("in")), Some(&dir.join("in"))).unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let rx = serve_on(listener, egui::Context::default(), KEY.to_string(), FIRST_REQUEST, floor);
        let s = |p: &str| dir.join(p).to_string_lossy().into_owned();
        let mut c = Conn::open(port);
        assert_eq!(c.call(&format!(r#"{{"id":1,"method":"auth","key":"{KEY}"}}"#))["result"]["confined"], true);
        assert_eq!(roots_of(&mut c, &rx).unwrap().read_root(), Some(dir.join("in").as_path()));
        // A request with the key and no `auth` is confined too.
        let mut c = Conn::open(port);
        c.send(&format!(r#"{{"id":1,"key":"{KEY}","method":"document.inspect"}}"#));
        assert!(answer(&rx).roots.is_some());
        assert_eq!(c.recv()["ok"], true);
        let mut c = Conn::open(port);
        assert_eq!(c.call(&auth_with(json!({"readRoot": s("outside"), "writeRoot": s("outside")})))["ok"], false);
        assert!(c.closed());
        let mut c = Conn::open(port);
        assert_eq!(c.call(&auth_with(json!({"readRoot": s("in/sub"), "writeRoot": null})))["result"]["readRoot"], s("in/sub"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_silent_connection_is_closed() {
        let (port, rx) = boot_with(Duration::from_millis(300));
        let mut quiet = Conn::open(port);
        let t0 = Instant::now();
        assert!(quiet.closed());
        assert!(t0.elapsed() < Duration::from_secs(3));
        // After an authorised request the connection may stay idle.
        let mut busy = Conn::open(port);
        assert_eq!(busy.call(&format!(r#"{{"id":1,"method":"auth","key":"{KEY}"}}"#))["ok"], true);
        std::thread::sleep(Duration::from_millis(600));
        busy.send(r#"{"id":2,"method":"document.inspect"}"#);
        answer(&rx);
        assert_eq!(busy.recv()["ok"], true);
    }

    #[test]
    fn the_first_request_limit_is_one_overall_deadline() {
        // Blank lines and bad JSON do not keep a connection without the key open.
        let (port, _rx) = boot_with(Duration::from_millis(300));
        let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut w = s.try_clone().unwrap();
        let feeder = std::thread::spawn(move || {
            for i in 0..40 {
                if w.write_all(if i % 2 == 0 { b"\n" } else { b"not json\n" }).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let t0 = Instant::now();
        let mut r = BufReader::new(s);
        r.get_ref().set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut line = String::new();
        loop {
            line.clear();
            if matches!(r.read_line(&mut line), Ok(0) | Err(_)) {
                break;
            }
            assert!(line.contains("bad JSON"), "{line}");
        }
        let _ = feeder.join();
        assert!(t0.elapsed() < Duration::from_millis(1500), "{:?}", t0.elapsed());
    }

    #[test]
    fn bad_json_closes_the_connection() {
        let (port, rx) = boot();
        let mut c = Conn::open(port);
        assert_eq!(c.call("not json")["ok"], false);
        assert!(c.closed());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn http_request_cannot_smuggle_a_command() {
        // What a web page's `fetch("http://127.0.0.1:<port>/", {method: "POST", body})` sends,
        // even with the key in the body.
        let (port, rx) = boot();
        let body = format!("\n{{\"key\":\"{KEY}\",\"method\":\"file.saveAs\",\"params\":{{\"path\":\"/tmp/x.docx\"}}}}\n");
        let req = format!("POST / HTTP/1.1\r\nHost: 127.0.0.1:7981\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        let mut c = Conn::open(port);
        c.1.write_all(req.as_bytes()).unwrap();
        let _ = c.recv();
        assert!(c.closed());
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn keys_compare_whole() {
        assert!(keys_match(KEY, KEY));
        assert!(!keys_match(KEY, &KEY[..63]));
        assert!(!keys_match(&KEY[..63], KEY));
        assert!(!keys_match(KEY, &KEY.replace('f', "e")));
        assert!(!keys_match("", ""), "an empty key never matches");
    }

    #[test]
    fn random_keys_are_256_bits() {
        let (a, b) = (random_key().unwrap(), random_key().unwrap());
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wordcraft-control-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_key_file_is_written_after_the_bind_and_removed_on_exit() {
        let dir = temp_dir("life");
        // The port is taken: no key file.
        let taken = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = taken.local_addr().unwrap().port();
        assert!(start(port, egui::Context::default(), Some(&dir), None).is_none());
        assert!(!dir.exists() || std::fs::read_dir(&dir).unwrap().next().is_none(), "no key file after a failed bind");
        drop(taken);
        // Bound: the key file holds the key that opens the port.
        let mut free_port = port.wrapping_add(1).max(1024);
        let mut started = None;
        for _ in 0..100 {
            if let Some(res) = start(free_port, egui::Context::default(), Some(&dir), None) {
                started = Some((res, free_port));
                break;
            }
            free_port = free_port.wrapping_add(1).max(1024);
        }
        let ((rx, key_file), port) = started.unwrap();
        let key_file = key_file.unwrap();
        assert_eq!(key_file.path(), wordcraft_control_key::key_path(&dir, port));
        let key = std::fs::read_to_string(key_file.path()).unwrap();
        assert_eq!(key.len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(key_file.path()).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let mut c = Conn::open(port);
        c.send(&format!(r#"{{"id":1,"key":"{key}","method":"document.inspect"}}"#));
        answer(&rx);
        assert_eq!(c.recv()["ok"], true);
        key_file.remove();
        assert!(!key_file.path().exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_key_file_is_replaced() {
        let dir = temp_dir("stale");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("control-key.test");
        std::fs::write(&p, "old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        write_key_file(&p, KEY).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), KEY);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
