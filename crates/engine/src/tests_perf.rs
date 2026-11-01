//! Profiling runs on large synthetic documents (`#[ignore]`d: they take a while and print their
//! numbers). One document per process gives clean memory numbers:
//!
//! `WC_PERF_DOC=prose cargo test --release -p wordcraft-engine perf_profile -- --ignored --nocapture`
//!
//! `WC_PERF_DOC` is one of `prose` (about 600 pages of prose and headings with a table of
//! contents), `tables` (about 200 pages of small tables), `bigtable` (one 8,000-row table),
//! `notes` (two footnotes in every paragraph), `pictures` (300 distinct pictures) and `sections`
//! (20 sections, each with its own header and footer); unset runs them all in one process. Each
//! line printed is `PERF <doc> <what> <value>`: times in milliseconds, memory (`rss`, the
//! process's resident set from `/proc/self/status`) in MB.

use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use wordcraft_doc::para::{InlineObject, NoteKind};
use wordcraft_doc::props::ParaProps;
use wordcraft_doc::section::HeaderSet;
use wordcraft_doc::{Block, Document, PartKind, Path, Pos, StoryRef, Table, para_block};

use crate::{Selection, Session};

const WORDS: &[&str] = &[
    "the", "quick", "brown", "fox", "jumps", "over", "lazy", "dog", "while", "band", "plays", "river", "stone", "garden", "window", "morning",
    "evening", "letter", "number", "people", "history", "between", "through", "because", "without", "another", "village", "harbour", "journey",
    "question", "silver", "orchard", "lantern", "meadow", "pattern", "chapter", "measure", "market", "winter", "summer",
];

/// Deterministic words, `n` of them, starting from `seed`.
fn words(seed: usize, n: usize) -> String {
    let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    let mut out = String::with_capacity(n * 7);
    for i in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        if i > 0 {
            out.push(' ');
        }
        out.push_str(WORDS[x % WORDS.len()]);
    }
    out.push('.');
    out
}

fn para(text: &str, style: Option<&str>) -> Arc<Block> {
    let mut p = wordcraft_doc::Paragraph::with_text(text, Default::default());
    p.props = ParaProps { style: style.map(str::to_string), ..Default::default() };
    para_block(p)
}

fn empty_doc() -> Document {
    let mut d = Document::new();
    d.body.clear();
    d
}

/// About 600 pages: a chapter (Heading 1) every 60 paragraphs, a Heading 2 every 10.
fn prose() -> Document {
    let mut d = empty_doc();
    for i in 0..3600 {
        if i % 60 == 0 {
            d.body.push(para(&format!("Chapter {}", i / 60 + 1), Some("Heading1")));
        } else if i % 10 == 0 {
            d.body.push(para(&format!("Section {}.{}", i / 60 + 1, i % 60 / 10), Some("Heading2")));
        }
        d.body.push(para(&words(i, 70), None));
    }
    d
}

fn table(rows: usize, cols: usize, seed: usize) -> Arc<Block> {
    let mut t = Table::new(rows, cols, 468.0);
    for (r, row) in t.rows.iter_mut().enumerate() {
        for (c, cell) in row.cells.iter_mut().enumerate() {
            cell.blocks = vec![para(&words(seed + r * cols + c, 3), None)];
        }
    }
    Arc::new(Block::Table(t))
}

/// About 200 pages of 20-row tables with a heading paragraph before each.
fn tables() -> Document {
    let mut d = empty_doc();
    for i in 0..300 {
        d.body.push(para(&format!("Table {}", i + 1), Some("Heading2")));
        d.body.push(table(20, 5, i * 100));
    }
    d
}

fn bigtable() -> Document {
    let mut d = empty_doc();
    d.body.push(para("Before the table", None));
    d.body.push(table(8000, 5, 0));
    d.body.push(para("After the table", None));
    d
}

/// 1,500 paragraphs, each with two footnotes.
fn notes() -> Document {
    let mut d = empty_doc();
    for i in 0..1500 {
        d.body.push(para(&words(i, 60), None));
    }
    for i in 0..1500 {
        for k in 0..2 {
            let note = wordcraft_doc::Paragraph::with_text(&words(i * 2 + k, 12), Default::default()).styled("FootnoteText");
            let id = d.add_part(PartKind::Footnote, vec![para_block(note)]);
            let off = if k == 0 { 20 } else { 60 };
            let _ = d.insert_object(
                &Pos::body(i, off),
                InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() },
                &Default::default(),
            );
        }
    }
    d
}

/// A distinct 480×320 PNG.
fn png(i: usize) -> Vec<u8> {
    let img = image::RgbImage::from_fn(480, 320, |x, y| image::Rgb([(x as usize + i * 7) as u8, (y as usize + i * 13) as u8, (i * 31) as u8]));
    let mut out = Vec::new();
    let _ = img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png);
    out
}

/// 300 distinct inline pictures, one after every two paragraphs.
fn pictures() -> Document {
    let mut d = empty_doc();
    for i in 0..600 {
        d.body.push(para(&words(i, 50), None));
    }
    for i in 0..300 {
        let media = d.add_media(png(i), "png");
        let pic =
            InlineObject::Image { media, w: 240.0, h: 160.0, alt: format!("Picture {i}"), float: Default::default(), crop: [0.0; 4], ole: None };
        let _ = d.insert_object(&Pos::body(i * 2, 0), pic, &Default::default());
    }
    d
}

/// 20 sections of 60 paragraphs, each with its own header and footer.
fn sections() -> Document {
    let mut d = empty_doc();
    for s in 0..20 {
        for i in 0..60 {
            d.body.push(para(&words(s * 100 + i, 60), None));
        }
        let h = d.add_part(PartKind::Header, vec![para(&format!("Header of section {}", s + 1), Some("Header"))]);
        let f = d.add_part(PartKind::Footer, vec![para(&format!("Footer of section {}", s + 1), Some("Footer"))]);
        let props = wordcraft_doc::SectionProps {
            headers: HeaderSet { default: Some(h), ..Default::default() },
            footers: HeaderSet { default: Some(f), ..Default::default() },
            ..d.last_section.clone()
        };
        if s == 19 {
            d.last_section = props;
        } else if let Some(Block::Para(p)) = d.body.last_mut().map(Arc::make_mut) {
            p.section = Some(Box::new(props));
        }
    }
    d
}

/// The process's resident memory (Linux `VmRSS`), MB; 0 elsewhere.
fn rss_mb() -> f64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status.lines().find_map(|l| l.strip_prefix("VmRSS:")).and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<f64>().ok()).unwrap_or(0.0)
        / 1024.0
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

struct Out<'a>(&'a str);

impl Out<'_> {
    fn p(&self, what: &str, v: f64) {
        eprintln!("PERF {} {what} {v:.1}", self.0);
    }
}

/// A command and the layout after it (what the window shows next), timed together.
fn timed(s: &mut Session, id: &str, params: serde_json::Value) -> f64 {
    let t = Instant::now();
    let r = s.run(id, &params);
    let _ = s.layout();
    if let Err(e) = r {
        eprintln!("PERF note {id}: {e}");
    }
    ms(t)
}

/// The caret at the start of body block `block`; in a table, its middle row's first cell.
fn caret_in(s: &mut Session, block: usize) {
    let b = block.min(s.doc.body.len().saturating_sub(1));
    let path = match s.doc.body.get(b).map(|x| &**x) {
        Some(Block::Table(t)) => Path(vec![b as u32, (t.rows.len() / 2) as u32, 0, 0]),
        _ => Path::top(b),
    };
    s.sel = Selection::caret(Pos { story: StoryRef::Body, path, off: 0 });
}

fn profile(name: &str, doc: Document) {
    let o = Out(name);
    o.p("rss_start", rss_mb());
    // Files: written once, then opened as the app opens them.
    let t = Instant::now();
    let docx = crate::io::save_bytes("perf.docx", &doc).unwrap();
    o.p("save_docx_built", ms(t));
    o.p("docx_kb", docx.len() as f64 / 1024.0);
    // `WC_PERF_SAVE=dir` keeps the file, to open it with the app or the CLI.
    if let Ok(dir) = std::env::var("WC_PERF_SAVE") {
        let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{name}.docx")), &docx);
    }
    drop(doc);
    let t = Instant::now();
    let doc = crate::io::open_bytes("perf.docx", &docx).unwrap();
    o.p("open_docx", ms(t));
    o.p("rss_after_open", rss_mb());
    let mut s = Session::new(doc);
    let t = Instant::now();
    let l = s.layout();
    o.p("first_layout", ms(t));
    o.p("pages", l.pages.len() as f64);
    o.p("rss_after_layout", rss_mb());
    // Rendering at 100% on a 96-dpi screen (what the canvas rasterises).
    let opts = wordcraft_render::RenderOptions::default();
    let scale = 96.0 / 72.0;
    let render = |s: &Session, l: &wordcraft_layout::DocLayout, i: usize| {
        let t = Instant::now();
        if let Some(p) = l.pages.get(i) {
            let _ = wordcraft_render::render_page(&s.doc, p, scale, &opts);
        }
        ms(t)
    };
    o.p("render_page1_first", render(&s, &l, 0));
    o.p("render_page1_again", render(&s, &l, 0));
    o.p("render_pages2_3", render(&s, &l, 1) + render(&s, &l, 2));
    // A high-density screen (two device pixels a point) at 100%.
    let t = Instant::now();
    if let Some(p) = l.pages.first() {
        let _ = wordcraft_render::render_page(&s.doc, p, scale * 2.0, &opts);
    }
    o.p("render_page1_hidpi", ms(t));
    let only_keys = std::env::var("WC_PERF_STOP").is_ok_and(|v| v == "keys");
    let t = Instant::now();
    for i in 0..if only_keys { 0 } else { l.pages.len() } {
        let _ = render(&s, &l, i);
    }
    let all = ms(t);
    o.p("render_all_pages", all);
    o.p("render_per_page", all / l.pages.len().max(1) as f64);
    o.p("rss_after_render_all", rss_mb());
    drop(l);
    // Typing in the middle: the relayout and the re-render of the caret's page.
    let mid = s.doc.body.len() / 2;
    caret_in(&mut s, mid);
    let _ = s.layout();
    let mut keys = Vec::new();
    let mut page_renders = Vec::new();
    let verbose = std::env::var("WC_PERF_VERBOSE").is_ok();
    // Words with spaces, as a person types (one long word in a table cell would widen its
    // autofit column on every key, which lays the whole table out again: see `long_word`).
    for c in "the quick brown fox jumps over ".chars() {
        let t = Instant::now();
        let _ = s.run("text.insert", &json!({"text": c.to_string()}));
        let cmd = ms(t);
        let l = s.layout();
        keys.push(ms(t));
        if verbose {
            eprintln!("PERF {name} key command {cmd:.1} total {:.1}", ms(t));
        }
        let page = l.caret_on(&s.sel.focus, 0).map_or(0, |c| c.page);
        page_renders.push(render(&s, &l, page));
    }
    keys.sort_by(f64::total_cmp);
    page_renders.sort_by(f64::total_cmp);
    o.p("keystroke_median", keys[keys.len() / 2]);
    o.p("keystroke_max", keys[keys.len() - 1]);
    o.p("keystroke_page_render_median", page_renders[page_renders.len() / 2]);
    // `WC_PERF_STOP=keys` ends here (profiling the typing alone).
    if only_keys {
        return;
    }
    // Heavy commands, each with the layout after it.
    o.p("replace_all", timed(&mut s, "edit.replaceAll", json!({"text": "garden", "with": "gardens"})));
    o.p("undo_replace_all", timed(&mut s, "edit.undo", json!({})));
    o.p("select_all_bold", timed(&mut s, "select.all", json!({"whole": true})) + timed(&mut s, "format.bold", json!({})));
    o.p("undo_bold", timed(&mut s, "edit.undo", json!({})));
    if name == "prose" {
        caret_in(&mut s, 0);
        o.p("insert_toc", timed(&mut s, "references.toc", json!({})));
    }
    o.p("update_fields", timed(&mut s, "references.updateFields", json!({"all": true})));
    o.p("modify_style", timed(&mut s, "styles.modify", json!({"style": "Normal", "chr": {"size": 12.0}})));
    // Copy 1,000 paragraphs (or what there is) and paste them at the end.
    let n = s.doc.body.len();
    let last = n.saturating_sub(1);
    s.sel =
        Selection { anchor: Pos::body(last.saturating_sub(1000).min(last), 0), focus: Pos { story: StoryRef::Body, path: Path::top(last), off: 0 } };
    let _ = s.run("edit.copy", &json!({}));
    s.sel = Selection::caret(Pos { story: StoryRef::Body, path: Path::top(last), off: 0 });
    o.p("paste_1000", timed(&mut s, "edit.paste", json!({})));
    o.p("undo_paste", timed(&mut s, "edit.undo", json!({})));
    o.p("redo_paste", timed(&mut s, "edit.redo", json!({})));
    // Saving: serialising on the caller, and what a worker thread does with a copy.
    let t = Instant::now();
    let _ = crate::io::save_bytes("perf.docx", &s.doc);
    o.p("save_docx", ms(t));
    let t = Instant::now();
    let copy = s.doc.clone();
    o.p("save_copy_for_worker", ms(t));
    let t = Instant::now();
    let _ = std::thread::spawn(move || crate::io::save_bytes("perf.docx", &copy).map(|b| b.len())).join();
    o.p("save_docx_on_worker", ms(t));
    let t = Instant::now();
    let pdf = crate::io::save_bytes("perf.pdf", &s.doc);
    o.p("save_pdf", ms(t));
    o.p("pdf_kb", pdf.map_or(0, |b| b.len()) as f64 / 1024.0);
    o.p("rss_after_commands", rss_mb());
    // 1,000 edits spread over the document, each laid out as the window would.
    let t = Instant::now();
    for i in 0..1000 {
        if i % 50 == 0 {
            let n = s.doc.body.len();
            caret_in(&mut s, (i * 7919) % n.max(1));
        }
        let _ = s.run("text.insert", &json!({"text": "y"}));
        let _ = s.layout();
    }
    o.p("edits_1000", ms(t));
    o.p("rss_after_1000_edits", rss_mb());
    o.p("undo_steps", s.undo_depth() as f64);
    o.p("undo_mb", s.undo_bytes() as f64 / 1048576.0);
}

/// One 8,000-row table whose cells ask for no width (as HTML and Markdown tables come in):
/// AutoFit sizes its columns from their content alone.
fn autotable() -> Document {
    let mut d = bigtable();
    if let Some(Block::Table(t)) = d.body.get_mut(1).map(Arc::make_mut) {
        t.grid.clear();
        for row in &mut t.rows {
            for c in &mut wordcraft_doc::Shared::make_mut(row).cells {
                c.props.width = None;
            }
        }
    }
    d
}

/// A document as the app has it: saved as DOCX and opened again.
fn reopened(doc: &Document) -> Document {
    crate::io::open_bytes("perf.docx", &crate::io::save_bytes("perf.docx", doc).unwrap()).unwrap()
}

/// The caret in the first cell of the middle row of the body table nearest the middle.
fn caret_in_middle_table(s: &mut Session) {
    let n = s.doc.body.len();
    let mid = (n / 2..n).chain(0..n / 2).find(|i| matches!(s.doc.body.get(*i).map(|b| &**b), Some(Block::Table(_)))).unwrap_or(n / 2);
    caret_in(s, mid);
}

/// Each key of `keys` (a character, or `\u{8}` for Backspace) run and laid out: the median and
/// slowest times (command and layout) and the paragraphs laid out or measured per key.
fn type_keys(o: &Out, what: &str, s: &mut Session, keys: &str) {
    let mut times = Vec::new();
    let mut work = Vec::new();
    let mut shaped = 0;
    let verbose = std::env::var("WC_PERF_VERBOSE").is_ok();
    for c in keys.chars() {
        let (misses, (rebroken, measured)) = (s.cache_misses(), s.cache_rebroken_measured());
        let t = Instant::now();
        let _ = if c == '\u{8}' { s.run("text.backspace", &json!({})) } else { s.run("text.insert", &json!({"text": c.to_string()})) };
        let _ = s.layout();
        let ms = ms(t);
        let laid = s.cache_misses() - misses;
        // Of the paragraphs laid out, those shaped (not measured, nor broken into lines again).
        let (r, m) = s.cache_rebroken_measured();
        let shape = laid - (r - rebroken) - (m - measured);
        if verbose {
            eprintln!("PERF {} {what} key {c:?} {ms:.1} ms, {laid} laid out, {shape} shaped", o.0);
        }
        times.push(ms);
        work.push(laid as f64);
        shaped += shape;
    }
    let total: f64 = times.iter().sum();
    times.sort_by(f64::total_cmp);
    let laid: f64 = work.iter().sum();
    work.sort_by(f64::total_cmp);
    o.p(&format!("{what}_median"), times[times.len() / 2]);
    o.p(&format!("{what}_max"), times[times.len() - 1]);
    o.p(&format!("{what}_total"), total);
    o.p(&format!("{what}_laid_out_median"), work[work.len() / 2]);
    o.p(&format!("{what}_laid_out_total"), laid);
    o.p(&format!("{what}_shaped_total"), shaped as f64);
}

/// Typing in a cell of a long AutoFit table: words (no column changes), a long word (its column
/// widens on each key once the word is longer than any in the column), Backspace over it (the
/// column narrows again), and the long word pasted at once.
///
/// `WC_PERF_DOC=bigtable|tables|autotable cargo test --release -p wordcraft-engine perf_table_typing -- --ignored --nocapture`
#[test]
#[ignore]
fn perf_table_typing() {
    let which = std::env::var("WC_PERF_DOC").unwrap_or_else(|_| "bigtable".into());
    let make: fn() -> Document = match which.as_str() {
        "tables" => tables,
        "autotable" => autotable,
        _ => bigtable,
    };
    let o = Out(&which);
    let mut s = Session::new(reopened(&make()));
    let t = Instant::now();
    let l = s.layout();
    o.p("first_layout", ms(t));
    o.p("pages", l.pages.len() as f64);
    drop(l);
    caret_in_middle_table(&mut s);
    let _ = s.layout();
    type_keys(&o, "words", &mut s, "the quick brown fox jumps over ");
    let long = "Pneumonoultramicroscopicsilicovolcanoconiosis";
    type_keys(&o, "long_word", &mut s, long);
    type_keys(&o, "backspace", &mut s, &"\u{8}".repeat(long.chars().count()));
    let misses = s.cache_misses();
    let t = Instant::now();
    let _ = s.run("text.insert", &json!({"text": long}));
    let _ = s.layout();
    o.p("paste_long_word", ms(t));
    o.p("paste_long_word_laid_out", (s.cache_misses() - misses) as f64);
    let t = Instant::now();
    let _ = s.run("edit.undo", &json!({}));
    let _ = s.layout();
    o.p("undo_long_word", ms(t));
    o.p("rss", rss_mb());
}

/// Typing words (the columns keep their widths) in the middle row of a long table, many keys,
/// for profiling the relayout of a key inside the table: `WC_PERF_KEYS` keys (default 300),
/// `WC_PERF_DOC=bigtable|tables|autotable`.
///
/// `WC_PERF_DOC=bigtable cargo test --release -p wordcraft-engine perf_table_keys -- --ignored --nocapture`
#[test]
#[ignore]
fn perf_table_keys() {
    let which = std::env::var("WC_PERF_DOC").unwrap_or_else(|_| "bigtable".into());
    let make: fn() -> Document = match which.as_str() {
        "tables" => tables,
        "autotable" => autotable,
        _ => bigtable,
    };
    let n: usize = std::env::var("WC_PERF_KEYS").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
    let o = Out(&which);
    // `WC_PERF_FILE=path.docx`: that document instead (the frame bench's bt8000, say).
    let doc = match std::env::var("WC_PERF_FILE") {
        Ok(path) => crate::io::open_path(std::path::Path::new(&path)).unwrap(),
        Err(_) => reopened(&make()),
    };
    let mut s = Session::new(doc);
    let _ = s.layout();
    caret_in_middle_table(&mut s);
    let _ = s.layout();
    let keys: String = "the quick brown fox jumps over ".chars().cycle().take(n).collect();
    type_keys(&o, "keys", &mut s, &keys);
}

/// Typing in the middle of each large document, and 1,000 edits spread over it, each laid out:
/// typing outside AutoFit tables must not get slower.
///
/// `WC_PERF_DOC=prose|tables|notes|bigtable cargo test --release -p wordcraft-engine perf_typing -- --ignored --nocapture`
#[test]
#[ignore]
fn perf_typing() {
    let which = std::env::var("WC_PERF_DOC").unwrap_or_default();
    let docs: [(&str, fn() -> Document); 4] = [("prose", prose), ("tables", tables), ("notes", notes), ("bigtable", bigtable)];
    for (name, make) in docs {
        if !which.is_empty() && which != name {
            continue;
        }
        let o = Out(name);
        let mut s = Session::new(reopened(&make()));
        let _ = s.layout();
        let mid = s.doc.body.len() / 2;
        caret_in(&mut s, mid);
        let _ = s.layout();
        type_keys(&o, "keys", &mut s, "the quick brown fox jumps over ");
        let misses = s.cache_misses();
        let t = Instant::now();
        for i in 0..1000 {
            if i % 50 == 0 {
                let n = s.doc.body.len();
                caret_in(&mut s, (i * 7919) % n.max(1));
            }
            let _ = s.run("text.insert", &json!({"text": "y"}));
            let _ = s.layout();
        }
        o.p("edits_1000", ms(t));
        o.p("edits_1000_laid_out", (s.cache_misses() - misses) as f64);
        o.p("rss", rss_mb());
    }
}

/// PDF export of a document whose layout is up to date (as after the window drew it), through
/// each path: Print (`Session::print_pdf`), File › Export (`file.exportPdf`) and Save As PDF
/// (`file.saveAs`), and again after a keystroke.
///
/// `WC_PERF_DOC=prose|tables|bigtable|notes cargo test --release -p wordcraft-engine perf_pdf -- --ignored --nocapture`
#[test]
#[ignore]
fn perf_pdf() {
    let which = std::env::var("WC_PERF_DOC").unwrap_or_default();
    let docs: [(&str, fn() -> Document); 4] = [("prose", prose), ("tables", tables), ("bigtable", bigtable), ("notes", notes)];
    let dir = std::env::temp_dir().join(format!("wc-perf-pdf-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = |n: &str| dir.join(n).to_string_lossy().into_owned();
    for (name, make) in docs {
        if !which.is_empty() && which != name {
            continue;
        }
        let o = Out(name);
        let mut s = Session::new(reopened(&make()));
        let t = Instant::now();
        let _ = s.layout();
        o.p("first_layout", ms(t));
        let t = Instant::now();
        let pdf = s.print_pdf(&json!({})).map(|(b, _)| b.len()).unwrap_or(0);
        o.p("print_pdf", ms(t));
        o.p("print_pdf_kb", pdf as f64 / 1024.0);
        let t = Instant::now();
        let _ = s.run("file.exportPdf", &json!({"path": path("export.pdf")}));
        o.p("export_pdf", ms(t));
        let t = Instant::now();
        let _ = s.run("file.saveAs", &json!({"path": path("saved.pdf")}));
        o.p("save_as_pdf", ms(t));
        let mid = s.doc.body.len() / 2;
        caret_in(&mut s, mid);
        let _ = s.run("text.insert", &json!({"text": "x"}));
        let t = Instant::now();
        let _ = s.layout();
        o.p("key_layout", ms(t));
        let t = Instant::now();
        let _ = s.run("file.exportPdf", &json!({"path": path("export2.pdf")}));
        o.p("export_pdf_after_key", ms(t));
        o.p("rss", rss_mb());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore]
fn perf_zz_probe() {
    if std::env::var("WC_PROBE_DICT").is_ok() {
        let _ = wordcraft_proof::is_correct("warm");
    }
    let which = std::env::var("WC_PROBE_DOC").unwrap_or_default();
    let doc = match which.as_str() {
        "notes" => notes(),
        "tables" => tables(),
        "prose" => prose(),
        _ => bigtable(),
    };
    let doc = if std::env::var("WC_PROBE_DOCX").is_ok() {
        crate::io::open_bytes("p.docx", &crate::io::save_bytes("p.docx", &doc).unwrap()).unwrap()
    } else {
        doc
    };
    if let Some(Block::Table(t)) = doc.body.get(1).map(|b| &**b) {
        eprintln!(
            "PROBE table props {:?} grid {:?} first cell {:?}",
            t.props,
            t.grid.get(..2),
            t.rows.first().and_then(|r| r.cells.first()).map(|c| &c.props)
        );
    }
    let mut s = Session::new(doc);
    let l = s.layout();
    if std::env::var("WC_PROBE_RENDER").is_ok() {
        let t = Instant::now();
        for p in &l.pages {
            let _ = wordcraft_render::render_page(&s.doc, p, 1.0, &Default::default());
        }
        eprintln!("PROBE rendered {} pages in {:.1} ms", l.pages.len(), ms(t));
    }
    if std::env::var("WC_PROBE_PAGES").is_ok() {
        let t = Instant::now();
        let n: usize = l.pages.iter().map(|p| p.items.len()).sum();
        eprintln!("PROBE materialised the items of {} pages ({n}) in {:.1} ms", l.pages.len(), ms(t));
    }
    drop(l);
    if std::env::var("WC_PROBE_BOLD").is_ok() {
        let t = Instant::now();
        let r = s.run("select.all", &json!({})).and_then(|_| s.run("format.bold", &json!({})));
        let cmd = ms(t);
        let t = Instant::now();
        let l = s.layout();
        let bold = s
            .doc
            .para_paths(StoryRef::Body)
            .iter()
            .filter(|p| s.doc.para(StoryRef::Body, p).is_some_and(|q| q.runs.iter().all(|r| r.props.bold == Some(true))))
            .count();
        eprintln!("PROBE bold {:?}: command {cmd:.1} ms, layout {:.1} ms, {} pages, {bold} bold paragraphs", r.err(), ms(t), l.pages.len());
    }
    let n = s.doc.body.len();
    caret_in(&mut s, n / 2);
    let _ = s.layout();
    let revs = |d: &Document| {
        let mut v = Vec::new();
        for path in d.para_paths(StoryRef::Body) {
            v.push(d.para(StoryRef::Body, &path).map_or(0, |p| p.rev));
        }
        v
    };
    let keys: usize = std::env::var("WC_PROBE_KEYS").ok().and_then(|v| v.parse().ok()).unwrap_or(5);
    for k in 0..keys {
        let before = revs(&s.doc);
        let t = Instant::now();
        let _ = s.run("text.insert", &json!({"text": "x"}));
        let cmd = ms(t);
        let after = revs(&s.doc);
        let changed = before.iter().zip(&after).filter(|(a, b)| a != b).count();
        let t = Instant::now();
        let l = s.layout();
        let lay = ms(t);
        let t = Instant::now();
        let page = if std::env::var("WC_PROBE_CARET").is_ok() { l.caret_on(&s.sel.focus, 0).map(|c| c.page) } else { None };
        let caret = ms(t);
        let t = Instant::now();
        if let Some(p) = page.and_then(|i| l.pages.get(i))
            && std::env::var("WC_PROBE_PAGE").is_ok()
        {
            let _ = wordcraft_render::render_page(&s.doc, p, 1.0, &Default::default());
        }
        eprintln!(
            "PROBE key {k}: command {cmd:.1} ms, layout {lay:.1} ms, caret_on {caret:.1} ms (page {page:?}), render {:.1} ms, paragraphs with a new revision {changed} of {}",
            ms(t),
            after.len()
        );
    }
}

#[test]
#[ignore]
fn perf_profile() {
    let t = Instant::now();
    // What the app starts with: the font catalog, the US English dictionary and hyphenation
    // patterns (the window warms these on background threads while it opens).
    let t0 = Instant::now();
    let faces = wordcraft_fonts::FontDb::global().load_system_fonts();
    eprintln!("PERF start font_catalog {:.1} ({faces} faces)", ms(t0));
    let t0 = Instant::now();
    let _ = wordcraft_proof::dict::Dictionary::en_us();
    eprintln!("PERF start dictionary_en_us {:.1}", ms(t0));
    let t0 = Instant::now();
    let _ = wordcraft_proof::hyphen::Patterns::en_us();
    eprintln!("PERF start hyphenation_en_us {:.1}", ms(t0));
    let t0 = Instant::now();
    let _ = wordcraft_proof::is_correct("warm");
    eprintln!("PERF start spell_first_word {:.1}", ms(t0));
    let t0 = Instant::now();
    let mut s = Session::new(Document::from_text("One line to shape the default font."));
    let _ = s.layout();
    eprintln!("PERF start first_small_layout {:.1}", ms(t0));
    let which = std::env::var("WC_PERF_DOC").unwrap_or_default();
    let docs: [(&str, fn() -> Document); 6] =
        [("prose", prose), ("tables", tables), ("bigtable", bigtable), ("notes", notes), ("pictures", pictures), ("sections", sections)];
    for (name, make) in docs {
        if which.is_empty() || which == name {
            profile(name, make());
        }
    }
    eprintln!("PERF total {:.1}", ms(t));
}

/// 05's documents written as DOCX files, for timing other programs on them (the CLI's cold
/// `convert`, the app): `WC_PERF_SAVE=dir cargo test --release -p wordcraft-engine --lib
/// perf_save_documents -- --ignored`. Besides 05's six, `tables200` (200 of the `tables`
/// document's tables) and `autotable`.
#[test]
#[ignore]
fn perf_save_documents() {
    let dir = std::env::var("WC_PERF_SAVE").unwrap_or_else(|_| ".".into());
    let tables200 = || {
        let mut d = empty_doc();
        for i in 0..200 {
            d.body.push(para(&format!("Table {}", i + 1), Some("Heading2")));
            d.body.push(table(20, 5, i * 100));
        }
        d
    };
    let docs: [(&str, Document); 8] = [
        ("prose", prose()),
        ("tables", tables()),
        ("tables200", tables200()),
        ("bigtable", bigtable()),
        ("autotable", autotable()),
        ("notes", notes()),
        ("pictures", pictures()),
        ("sections", sections()),
    ];
    for (name, doc) in docs {
        let docx = crate::io::save_bytes("perf.docx", &doc).unwrap();
        std::fs::write(std::path::Path::new(&dir).join(format!("{name}.docx")), &docx).unwrap();
        eprintln!("PERF saved {name}.docx {} KB", docx.len() / 1024);
    }
}
