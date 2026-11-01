//! Headless frame benchmark: the real [`WordApp`] driven through `egui::Context::run_ui` the way
//! eframe drives it (logic + ui each frame, then tessellation), with a simulated event loop that
//! runs a frame whenever the app or a worker asks for one. It types, scrolls and idles in a tiny
//! document and in a large synthetic one (about 440 pages, like the `m400` benchmark document),
//! and prints where each frame's time goes (`prof` spans), how many frames one keystroke causes,
//! and how much CPU other threads use meanwhile (from `/proc/self/task/*/schedstat`).
//!
//! `cargo test --release -p wordcraft-ui-egui --lib frame_bench -- --ignored --nocapture`
//!
//! Environment: `WC_FRAME_DOC` = a comma-separated list of `tiny`, `m400` (about 440 pages of
//! prose) and `bt8000` (one 8,000-row table), or `all` (default); `WC_FRAME_PPP` pixels per
//! point (default 1); `WC_FRAME_KEYS` keystrokes (default 30); `WC_FRAME_A11Y=1` turns on the
//! AccessKit tree (as a screen reader would); `WC_FRAME_RECOVERY=1` turns on crash recovery copies
//! in a temporary folder (as the desktop app does).

use std::collections::BTreeMap;
use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::{Services, WordApp, prof};

const WORDS: &[&str] = &[
    "alder", "brook", "cinder", "dune", "ember", "fjord", "grove", "heron", "inlet", "juniper", "kestrel", "lumen", "meadow", "nectar", "orchid",
    "pebble", "quill", "russet", "sorrel", "thistle", "umber", "vale", "willow", "yarrow", "zephyr",
];

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn words(&mut self, n: usize) -> String {
        (0..n).map(|_| WORDS[(self.next() % WORDS.len() as u64) as usize]).collect::<Vec<_>>().join(" ")
    }
}

const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

fn para_xml(t: &str, style: Option<&str>, num: bool) -> String {
    let mut ppr = String::new();
    if style.is_some() || num {
        ppr.push_str("<w:pPr>");
        if let Some(s) = style {
            ppr.push_str(&format!(r#"<w:pStyle w:val="{s}"/>"#));
        }
        if num {
            ppr.push_str(r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr>"#);
        }
        ppr.push_str("</w:pPr>");
    }
    format!(r#"<w:p>{ppr}<w:r><w:t xml:space="preserve">{t}</w:t></w:r></w:p>"#)
}

/// A DOCX shaped like the `m400` benchmark document: 3,000 blocks, a Heading 1 every 25, a
/// numbered item every 7th, the rest 40–110 random words from a 25-word list (so repeated words,
/// which proofing marks, are common). About 440 pages.
pub(crate) fn m400_docx() -> Vec<u8> {
    let mut rng = Rng(400);
    let mut body = String::new();
    for i in 0..3000 {
        if i % 25 == 0 {
            body.push_str(&para_xml(&format!("Chapter {} {}", i / 25 + 1, rng.words(3)), Some("Heading1"), false));
        } else if i % 7 == 3 {
            body.push_str(&para_xml(&rng.words(8), None, true));
        } else {
            let n = 40 + (rng.next() % 71) as usize;
            body.push_str(&para_xml(&rng.words(n), None, false));
        }
    }
    docx(&body)
}

/// A DOCX shaped like the `bt8000` benchmark document: one 8,000-row, 3-column table with a
/// repeated header row and borders, each cell 2–8 random words, between two paragraphs.
pub(crate) fn bt8000_docx() -> Vec<u8> {
    let mut rng = Rng(8000);
    let mut body = para_xml("A long table follows.", None, false);
    body.push_str(r#"<w:tbl><w:tblPr><w:tblW w:w="5000" w:type="pct"/><w:tblBorders><w:top w:val="single" w:sz="4"/><w:bottom w:val="single" w:sz="4"/><w:insideH w:val="single" w:sz="4"/><w:insideV w:val="single" w:sz="4"/></w:tblBorders></w:tblPr><w:tblGrid><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/></w:tblGrid>"#);
    for r in 0..8000 {
        body.push_str(if r == 0 { "<w:tr><w:trPr><w:tblHeader/></w:trPr>" } else { "<w:tr>" });
        for _ in 0..3 {
            let n = 2 + (rng.next() % 7) as usize;
            body.push_str(r#"<w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/></w:tcPr>"#);
            body.push_str(&para_xml(&rng.words(n), None, false));
            body.push_str("</w:tc>");
        }
        body.push_str("</w:tr>");
    }
    body.push_str("</w:tbl>");
    body.push_str(&para_xml("The end.", None, false));
    docx(&body)
}

fn docx(body: &str) -> Vec<u8> {
    let numbering = format!(
        r#"<w:numbering {W}><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#
    );
    let files = [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/></Types>"#.to_string(),
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_string(),
        ),
        (
            "word/_rels/document.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdN" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/></Relationships>"#.to_string(),
        ),
        ("word/numbering.xml", numbering),
        (
            "word/document.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><w:document {W}><w:body>{body}<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720"/></w:sectPr></w:body></w:document>"#
            ),
        ),
    ];
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut out);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, text) in files {
            z.start_file(name, opts).unwrap();
            z.write_all(text.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }
    out.into_inner()
}

/// CPU time (ns) of every thread of this process, by thread id.
fn thread_cpu() -> BTreeMap<u32, (u64, String)> {
    let mut out = BTreeMap::new();
    let Ok(dir) = std::fs::read_dir("/proc/self/task") else { return out };
    for e in dir.flatten() {
        let Ok(tid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        let ns = std::fs::read_to_string(e.path().join("schedstat"))
            .ok()
            .and_then(|s| s.split_whitespace().next().and_then(|n| n.parse::<u64>().ok()))
            .unwrap_or(0);
        let name = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default().trim().to_string();
        out.insert(tid, (ns, name));
    }
    out
}

/// CPU time (ms) of the whole process, exited threads included (clock ticks: 10 ms steps).
fn process_cpu_ms() -> f64 {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let rest = s.rsplit_once(')').map_or("", |(_, r)| r);
    let f: Vec<&str> = rest.split_whitespace().collect();
    // After the name: state is field 3, utime 14 and stime 15.
    let tick = |i: usize| f.get(i - 3).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    (tick(14) + tick(15)) * 10.0
}

fn this_tid() -> u32 {
    std::fs::read_link("/proc/thread-self").ok().and_then(|p| p.file_name().and_then(|n| n.to_str()).and_then(|n| n.parse().ok())).unwrap_or(0)
}

/// What one phase (typing, scrolling, idling) cost.
#[derive(Default)]
struct Phase {
    frames: usize,
    passes: usize,
    run_ms: f64,
    tess_ms: f64,
    upload_bytes: usize,
    frame_ms: Vec<f64>,
    spans: BTreeMap<&'static str, (f64, u32)>,
    main_cpu_ms: f64,
    other_cpu_ms: f64,
    others: BTreeMap<String, f64>,
    /// What asked for the frames (egui's repaint causes: file:line).
    causes: BTreeMap<String, usize>,
}

struct Bench {
    app: WordApp,
    ctx: egui::Context,
    start: Instant,
    ppp: f32,
    size: egui::Vec2,
    /// When the app or a worker last asked for a frame.
    due: Arc<(Mutex<Option<Instant>>, std::sync::Condvar)>,
    pending: Vec<egui::Event>,
    /// When the last frame started: frames are at most 60 a second, as with vsync.
    last: Instant,
}

impl Bench {
    fn new(app: WordApp, ppp: f32, a11y: bool) -> Self {
        let ctx = egui::Context::default();
        if a11y {
            ctx.enable_accesskit();
        }
        let due = Arc::new((Mutex::new(Some(Instant::now())), std::sync::Condvar::new()));
        let d = due.clone();
        ctx.set_request_repaint_callback(move |info| {
            let at = Instant::now() + info.delay;
            let mut g = d.0.lock().unwrap();
            if g.is_none_or(|t| at < t) {
                *g = Some(at);
                d.1.notify_all();
            }
        });
        Self { app, ctx, start: Instant::now(), ppp, size: egui::vec2(1440.0, 900.0), due, pending: Vec::new(), last: Instant::now() }
    }

    fn raw(&mut self) -> egui::RawInput {
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
            max_texture_side: Some(8192),
            time: Some(self.start.elapsed().as_secs_f64()),
            predicted_dt: 1.0 / 60.0,
            focused: true,
            events: std::mem::take(&mut self.pending),
            ..Default::default()
        };
        let vp = raw.viewports.entry(egui::ViewportId::ROOT).or_default();
        vp.native_pixels_per_point = Some(self.ppp);
        vp.focused = Some(true);
        vp.inner_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size));
        vp.monitor_size = Some(egui::vec2(1920.0, 1080.0));
        raw
    }

    /// One frame as eframe runs it: raw input hook, logic + ui, tessellation.
    fn frame(&mut self, phase: &mut Phase) {
        *self.due.0.lock().unwrap() = None;
        let mut raw = self.raw();
        let t = Instant::now();
        self.app.raw_input_hook(&mut raw);
        self.last = t;
        let app = &mut self.app;
        let mut causes = Vec::new();
        let out = self.ctx.run_ui(raw, |ui| {
            let ctx = ui.ctx().clone();
            if causes.is_empty() {
                causes = ctx.repaint_causes();
            }
            app.logic(&ctx);
            app.ui(ui);
        });
        let run = t.elapsed().as_secs_f64() * 1e3;
        let t2 = Instant::now();
        let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        let tess = t2.elapsed().as_secs_f64() * 1e3;
        std::hint::black_box(prims.len());
        for ds in out.textures_delta.set.values() {
            for d in ds {
                let [w, h] = d.image.size();
                phase.upload_bytes += w * h * 4;
            }
        }
        if let Some(v) = out.viewport_output.get(&egui::ViewportId::ROOT) {
            // The delay egui reports (also covers repaints asked for during the pass).
            if v.repaint_delay < Duration::from_secs(3600) {
                let at = Instant::now() + v.repaint_delay;
                let mut g = self.due.0.lock().unwrap();
                if g.is_none_or(|x| at < x) {
                    *g = Some(at);
                }
            }
        }
        for c in causes {
            let file = c.file.rsplit('/').next().unwrap_or(c.file);
            *phase.causes.entry(format!("{file}:{} {}", c.line, c.reason)).or_default() += 1;
        }
        phase.frames += 1;
        phase.passes += out.platform_output.num_completed_passes;
        phase.run_ms += run;
        phase.tess_ms += tess;
        phase.frame_ms.push(run + tess);
        for (n, ms, c) in prof::take() {
            let e = phase.spans.entry(n).or_default();
            e.0 += ms;
            e.1 += c;
        }
    }

    /// Run the event loop for `d`: a frame whenever input is pending or a repaint is due.
    fn pump(&mut self, d: Duration, phase: &mut Phase) {
        let end = Instant::now() + d;
        loop {
            let now = Instant::now();
            let due = *self.due.0.lock().unwrap();
            let vsync = self.last + Duration::from_micros(16_667);
            if (!self.pending.is_empty() || due.is_some_and(|t| t <= now)) && now >= vsync {
                self.frame(phase);
                continue;
            }
            if now >= end {
                break;
            }
            let next = due.map_or(end, |t| t.max(vsync).min(end));
            // Sleep until then, or until a worker asks for a frame.
            let g = self.due.0.lock().unwrap();
            if *g == due {
                let _ = self.due.1.wait_timeout(g, next.saturating_duration_since(now)).unwrap();
            }
        }
    }

    /// Run `f` and account the CPU of this thread and the others to `phase`.
    fn measured(&mut self, phase: &mut Phase, f: impl FnOnce(&mut Self, &mut Phase)) {
        let me = this_tid();
        let before = thread_cpu();
        let total = process_cpu_ms();
        f(self, phase);
        let after = thread_cpu();
        let total = process_cpu_ms() - total;
        for (tid, (ns, name)) in &after {
            let was = before.get(tid).map_or(0, |b| b.0);
            let ms = ns.saturating_sub(was) as f64 / 1e6;
            if *tid == me {
                phase.main_cpu_ms += ms;
            } else if ms > 0.0 {
                *phase.others.entry(name.clone()).or_default() += ms;
            }
        }
        // Threads that ended meanwhile (workers) are only in the process total.
        phase.other_cpu_ms += (total - phase.main_cpu_ms).max(0.0);
    }

    fn key(&mut self, ch: char) {
        let key = egui::Key::from_name(&ch.to_ascii_uppercase().to_string()).unwrap_or(egui::Key::A);
        self.pending.push(egui::Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
        self.pending.push(egui::Event::Text(ch.to_string()));
    }

    fn key_up(&mut self, ch: char) {
        let key = egui::Key::from_name(&ch.to_ascii_uppercase().to_string()).unwrap_or(egui::Key::A);
        self.pending.push(egui::Event::Key { key, physical_key: Some(key), pressed: false, repeat: false, modifiers: egui::Modifiers::NONE });
    }
}

fn report(doc: &str, name: &str, per: usize, p: &Phase) {
    let per = per.max(1) as f64;
    let mut f = p.frame_ms.clone();
    f.sort_by(|a, b| a.total_cmp(b));
    let med = f.get(f.len() / 2).copied().unwrap_or(0.0);
    let max = f.last().copied().unwrap_or(0.0);
    println!(
        "FRAME {doc} {name}: per unit: frames {:.2}, passes {:.2}, main-thread CPU {:.2} ms (ui {:.2} + tessellate {:.2}), other threads {:.2} ms, texture upload {:.0} KB; frame median {:.2} ms, max {:.2} ms",
        p.frames as f64 / per,
        p.passes as f64 / per,
        p.main_cpu_ms / per,
        p.run_ms / per,
        p.tess_ms / per,
        p.other_cpu_ms / per,
        p.upload_bytes as f64 / per / 1024.0,
        med,
        max
    );
    let mut spans: Vec<_> = p.spans.iter().collect();
    spans.sort_by(|a, b| b.1.0.total_cmp(&a.1.0));
    for (n, (ms, c)) in spans {
        println!("FRAME {doc} {name}:   {n:<22} {:>8.3} ms/unit ({:.1} calls/unit)", ms / per, *c as f64 / per);
    }
    let mut causes: Vec<_> = p.causes.iter().collect();
    causes.sort_by(|a, b| b.1.cmp(a.1));
    for (c, n) in causes.iter().take(8) {
        println!("FRAME {doc} {name}:   cause {:>6.2}/unit {c}", **n as f64 / per);
    }
    for (n, ms) in &p.others {
        println!("FRAME {doc} {name}:   thread {n:<16} {:>8.3} ms/unit", ms / per);
    }
}

fn scratch() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("wc-frame-bench-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn bench_doc(doc: &str) {
    let ppp: f32 = std::env::var("WC_FRAME_PPP").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let keys: usize = std::env::var("WC_FRAME_KEYS").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
    let a11y = std::env::var("WC_FRAME_A11Y").is_ok_and(|v| v == "1");
    let dir = scratch();
    let path = dir.join(format!("{doc}.docx"));
    let bytes = match doc {
        "tiny" => docx(&para_xml("Hello world", None, false)),
        "bt8000" => bt8000_docx(),
        _ => m400_docx(),
    };
    std::fs::write(&path, bytes).unwrap();
    let mut session = wordcraft_engine::Session::new(wordcraft_doc::Document::new());
    if std::env::var("WC_FRAME_RECOVERY").is_ok_and(|v| v == "1") {
        session.enable_recovery(dir.join("recovery"), Arc::new(|| {})).unwrap();
    }
    let mut app = WordApp::new(session, Services::default());
    let t = Instant::now();
    app.run("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    let pages = app.session.layout().pages.len();
    println!("FRAME {doc}: opened in {:.0} ms, {pages} pages, {} blocks", t.elapsed().as_secs_f64() * 1e3, app.session.doc.body.len());
    let mut b = Bench::new(app, ppp, a11y);
    // Settle: fonts, first layout, the first page textures.
    let mut warm = Phase::default();
    b.pump(Duration::from_millis(1500), &mut warm);
    report(doc, "warmup", 1, &warm);
    // Typing at the start of the document (where the caret is after opening it), then in the
    // middle: in bt8000 the first cell paragraph of row 4,000.
    let middle = match doc {
        "tiny" => json!([0]),
        "bt8000" => json!([1, 4000, 0, 0]),
        _ => json!([1501]),
    };
    for (name, path) in [("typing-start", json!([0])), ("typing-middle", middle)] {
        b.app.session.run("caret.set", &json!({"pos": {"story": "body", "path": path, "off": 0}})).unwrap();
        b.app.canvas.want_focus = true;
        b.app.canvas.nav_to_caret = true;
        b.pump(Duration::from_millis(1500), &mut Phase::default());
        assert!(b.app.canvas.focused, "the page has the keyboard");
        // About 8 keys a second: key down + text, key up 60 ms later, the next key 60 ms after.
        let mut typing = Phase::default();
        let text_len = |b: &Bench| b.app.session.doc.plain_text(wordcraft_doc::StoryRef::Body).len();
        let before = text_len(&b);
        let mut engine_ms = 0.0;
        b.measured(&mut typing, |b, ph| {
            for i in 0..keys {
                let ch = (b'a' + (i % 26) as u8) as char;
                b.key(ch);
                b.pump(Duration::from_millis(60), ph);
                engine_ms += b.app.session.layout().ms;
                b.key_up(ch);
                b.pump(Duration::from_millis(60), ph);
            }
            // Workers a key started finish their work.
            b.pump(Duration::from_millis(400), ph);
        });
        assert_eq!(text_len(&b), before + keys, "every key typed a letter");
        report(doc, name, keys, &typing);
        println!("FRAME {doc} {name}:   engine layout (DocLayout::ms) {:.3} ms/unit", engine_ms / keys.max(1) as f64);
    }
    // Scrolling: mouse-wheel notches (one line each, as X11 and Wayland report a wheel) 300 ms
    // apart, so each one's easing runs to its end; then a touchpad-like burst of point deltas.
    let mut scroll = Phase::default();
    let notches = 12;
    b.measured(&mut scroll, |b, ph| {
        for _ in 0..notches {
            b.pending.push(egui::Event::PointerMoved(egui::pos2(720.0, 500.0)));
            b.pending.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, -1.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            });
            b.pump(Duration::from_millis(300), ph);
        }
    });
    report(doc, "scroll-wheel-notch", notches, &scroll);
    let mut scroll = Phase::default();
    b.measured(&mut scroll, |b, ph| {
        for _ in 0..20 {
            b.pending.push(egui::Event::PointerMoved(egui::pos2(720.0, 500.0)));
            b.pending.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -40.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            });
            b.pump(Duration::from_millis(16), ph);
        }
        b.pump(Duration::from_millis(1500), ph);
    });
    report(doc, "scroll-touchpad-20-events", 1, &scroll);
    // Idle with the caret blinking: 3 seconds right after input, then 3 more.
    let mut idle = Phase::default();
    b.measured(&mut idle, |b, ph| b.pump(Duration::from_secs(3), ph));
    report(doc, "idle-0-3s", 3, &idle);
    let mut idle = Phase::default();
    b.measured(&mut idle, |b, ph| b.pump(Duration::from_secs(3), ph));
    report(doc, "idle-3-6s", 3, &idle);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The app run on a virtual clock: a frame whenever input arrives or egui asks for one (at most
/// one each 1/60 s, as with vsync), each at its own time on the clock, however long it takes.
/// Animations are timed by egui's clock, so the frames counted don't depend on how fast the
/// machine is (the wall-clock [`Bench`] above does).
struct Clocked {
    app: WordApp,
    ctx: egui::Context,
    /// The virtual time of the last frame, and when egui asked for the next (seconds).
    last: f64,
    due: Option<f64>,
    frames: usize,
}

impl Clocked {
    const VSYNC: f64 = 1.0 / 60.0;

    fn new(app: WordApp) -> Self {
        Clocked { app, ctx: egui::Context::default(), last: 0.0, due: Some(0.0), frames: 0 }
    }

    fn frame(&mut self, now: f64, events: Vec<egui::Event>) {
        let size = egui::vec2(1440.0, 900.0);
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            max_texture_side: Some(8192),
            time: Some(now),
            predicted_dt: Self::VSYNC as f32,
            focused: true,
            events,
            ..Default::default()
        };
        let vp = raw.viewports.entry(egui::ViewportId::ROOT).or_default();
        vp.native_pixels_per_point = Some(1.0);
        vp.focused = Some(true);
        vp.inner_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size));
        self.app.raw_input_hook(&mut raw);
        let app = &mut self.app;
        let trace = std::env::var("WC_FRAME_TRACE").is_ok();
        let n = raw.events.len();
        let mut out = self.ctx.run_ui(raw, |ui| {
            let ctx = ui.ctx().clone();
            if trace {
                let causes: Vec<String> =
                    ctx.repaint_causes().iter().map(|c| format!("{}:{}", c.file.rsplit('/').next().unwrap_or(c.file), c.line)).collect();
                println!("TRACE {:.4} events {n} causes {causes:?}", now);
            }
            app.logic(&ctx);
            app.ui(ui);
        });
        // (Nothing is drawn: the textures are let go of.)
        out.textures_delta.clear();
        let delay = out.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay);
        self.due = (delay < Duration::from_secs(3600)).then_some(now + delay.as_secs_f64());
        self.last = now;
        self.frames += 1;
    }

    /// Run until `until` (virtual seconds), `inputs` arriving at their times; returns the frames.
    fn run(&mut self, mut inputs: Vec<(f64, egui::Event)>, until: f64) -> usize {
        let start = self.frames;
        loop {
            let next = inputs.first().map(|(t, _)| *t).into_iter().chain(self.due).fold(f64::INFINITY, f64::min);
            if next > until {
                break;
            }
            let now = next.max(self.last + Self::VSYNC);
            let n = inputs.iter().take_while(|(t, _)| *t <= now).count();
            let events = inputs.drain(..n).map(|(_, e)| e).collect();
            self.frame(now, events);
        }
        self.frames - start
    }
}

/// Frames a keystroke and a mouse-wheel notch cost on the virtual clock (with the caret's glide
/// and the wheel's easing as they look): eight keys typed 120 ms apart (key up after 60 ms) and
/// six notches 300 ms apart in a short document. `cargo test -p wordcraft-ui-egui --lib
/// a_keystroke_and_a_wheel_notch -- --nocapture` prints them (`WC_FRAME_TRACE=1`: each frame,
/// and what asked for it).
fn frames_per_key_and_notch() -> (f64, f64) {
    let dir = scratch();
    let path = dir.join("clocked.docx");
    std::fs::write(&path, docx(&(0..60).map(|i| para_xml(&format!("Line {i} of a short document to scroll."), None, false)).collect::<String>()))
        .unwrap();
    let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::new()), Services::default());
    app.run("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    let mut c = Clocked::new(app);
    c.run(Vec::new(), 1.5);
    c.app.session.run("caret.set", &json!({"pos": {"story": "body", "path": [3], "off": 0}})).unwrap();
    c.app.canvas.want_focus = true;
    c.run(Vec::new(), 3.0);
    assert!(c.app.canvas.focused, "the page has the keyboard");
    let before = c.app.session.doc.plain_text(wordcraft_doc::StoryRef::Body).len();
    let (keys, t0) = (8, 3.0);
    let key = |ch: char, pressed: bool| {
        let key = egui::Key::from_name(&ch.to_ascii_uppercase().to_string()).unwrap_or(egui::Key::A);
        egui::Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers: egui::Modifiers::NONE }
    };
    let mut inputs = Vec::new();
    for i in 0..keys {
        let ch = (b'a' + i as u8) as char;
        let at = t0 + i as f64 * 0.12;
        inputs.push((at, key(ch, true)));
        inputs.push((at, egui::Event::Text(ch.to_string())));
        inputs.push((at + 0.06, key(ch, false)));
    }
    let typed = c.run(inputs, t0 + keys as f64 * 0.12 + 0.3);
    assert_eq!(c.app.session.doc.plain_text(wordcraft_doc::StoryRef::Body).len(), before + keys, "every key typed a letter");
    // The caret blinks from here on, a frame each time it turns (the clock the blink reads is
    // the wall clock, so it is still lit).
    let t1 = t0 + keys as f64 * 0.12 + 1.0;
    c.run(Vec::new(), t1);
    let notches = 6;
    let mut inputs = Vec::new();
    for i in 0..notches {
        let at = t1 + i as f64 * 0.3;
        inputs.push((at, egui::Event::PointerMoved(egui::pos2(720.0, 500.0))));
        inputs.push((
            at,
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, -1.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            },
        ));
    }
    let scrolled = c.run(inputs, t1 + notches as f64 * 0.3);
    assert!(c.app.canvas.scroll_offset.y > 0.0, "the wheel scrolled the page");
    let _ = std::fs::remove_dir_all(&dir);
    (typed as f64 / keys as f64, scrolled as f64 / notches as f64)
}

/// Frames per keystroke and per wheel notch, on a clock that doesn't depend on the machine. A key
/// was 7.5 frames at 8 keys a second: the key's, the caret's glide to its end and a frame after
/// it drawing the same, and the key's release with egui's two frames after any input. The glide
/// now lands once less than half a pixel is left, and asks for no frame after its last: 7.0. A
/// notch is 12.3: the wheel's easing, the floating scroll bar's fade and egui's frames after the
/// input, as before.
#[test]
fn a_keystroke_and_a_wheel_notch_cost_only_the_frames_they_draw() {
    let (per_key, per_notch) = frames_per_key_and_notch();
    println!("FRAMES per key {per_key:.2}, per wheel notch {per_notch:.2}");
    assert!(per_key <= 7.0, "{per_key} frames a key");
    assert!(per_notch <= 12.5, "{per_notch} frames a notch");
}

#[test]
#[ignore]
fn frame_bench() {
    let which = std::env::var("WC_FRAME_DOC").unwrap_or_else(|_| "all".into());
    for doc in ["tiny", "m400", "bt8000"] {
        if which == "all" || which.split(',').any(|w| w == doc) {
            bench_doc(doc);
        }
    }
}
