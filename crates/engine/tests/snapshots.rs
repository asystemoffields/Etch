//! Snapshot tests: the safety net under the maintainability refactor. A synthetic corpus of
//! small documents (`snapshots/corpus.rs`) is laid out, written as PDF, rasterised (this binary)
//! and saved in every format and driven through scripted command sequences
//! (`snapshots_commands.rs`), and each result is compared with a golden file under
//! `tests/goldens/`. A refactor that changes no behaviour leaves every golden as it is.
//!
//! Families (one `#[test]` each, so a filter runs or blesses one family at a time):
//!
//! | Test | Binary | Golden | Pins |
//! |---|---|---|---|
//! | `layout_snapshots` | this | `layout/<doc>.<variant>.txt` | pages, every placed item, each line's text, position and height; table rows, notes, floats; the fonts used |
//! | `pdf_snapshots` | this | `pdf/<doc>.txt` | the PDF's objects and page content streams (uncompressed), binary streams hashed |
//! | `render_snapshots` | this | `render/<doc>.p<n>.<light|dark>.png` | a few pages rasterised by the CPU renderer, compared pixel by pixel |
//! | `save_snapshots` | `snapshots_commands` | `save/<doc>.<ext>.txt` | DOCX and ODT parts (one element a line), RTF, HTML, Markdown and plain text, timestamps blanked |
//! | `command_snapshots` | `snapshots_commands` | `commands/<script>.txt` | the document model after each of about 70 commands |
//!
//! Run: `cargo test -p wordcraft-engine --test 'snapshots*'`. Bless (rewrite the goldens the run
//! reaches): `WC_BLESS=1 cargo test -p wordcraft-engine --test 'snapshots*' [family]`, with
//! `WC_SNAPSHOT_DOC=name[,name]` to limit it to some documents. A failure prints the differing
//! hunks with their page, part or step, writes the actual output under `WC_SNAPSHOT_OUT` (or the
//! temp dir), and names the bless command.
//!
//! Determinism: the corpus uses the bundled fonts only (`corpus::pin_fonts`; the system-font
//! fallback is switched off in this process, and the fonts a layout or a PDF used are checked
//! and part of the golden), sessions run on a stopped clock, the renderer runs single-threaded,
//! and the dumps blank the values a save takes from the wall clock. The command scripts run in
//! their own binary because commands load system faces by name (a bullet's "Arial"), and a face
//! once loaded serves as fallback for every later layout in the process.
#![cfg(not(target_arch = "wasm32"))]

#[path = "snapshots/mod.rs"]
mod snapshots;

use std::fmt::Write as _;
use std::sync::Once;

use snapshots::corpus::{self, Doc};
use snapshots::dump;
use snapshots::golden::{self, Failures};
use wordcraft_layout::{DocLayout, LayoutCache, LayoutOptions};

/// Bundled fonts only: a character no bundled face has is not looked up among the system's
/// fonts, and no family is found among them by name (the right-to-left fallbacks name "Arial",
/// which is Liberation Sans on one machine and absent on another).
fn bundled_fonts_only() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let db = wordcraft_fonts::FontDb::global();
        assert!(db.skip_system_fonts(), "the system fonts were cataloged before the snapshots began");
        db.set_system_fallback(false);
    });
}

fn lay(doc: &wordcraft_doc::Document, opts: &LayoutOptions) -> DocLayout {
    wordcraft_layout::layout(doc, &mut LayoutCache::new(), opts)
}

/// The families a snapshot layout may use: the bundled fonts, and the craft fonts when the
/// build has them.
fn allowed_fonts() -> Vec<String> {
    let mut v: Vec<String> = corpus::BUNDLED.iter().map(|s| s.to_string()).collect();
    v.extend(
        wordcraft_fonts::japanese_fonts()
            .chain(wordcraft_fonts::chinese_fonts())
            .chain(wordcraft_fonts::korean_fonts())
            .map(|f| f.family.to_string()),
    );
    v
}

fn selected(docs: Vec<Doc>) -> Vec<Doc> {
    docs.into_iter().filter(|d| golden::wanted(d.name)).collect()
}

/// The layout of `doc` with `opts`, its fonts checked against the bundled set.
fn checked_layout(
    fails: &mut Failures,
    what: &str,
    doc: &wordcraft_doc::Document,
    opts: &LayoutOptions,
) -> (DocLayout, std::collections::BTreeSet<String>) {
    let l = lay(doc, opts);
    let fonts = dump::fonts_used(&l);
    let allowed = allowed_fonts();
    let foreign: Vec<&String> = fonts.iter().filter(|f| !allowed.contains(f)).collect();
    if !foreign.is_empty() {
        fails.push(format!(
            "--- {what}: the layout used fonts that aren't bundled: {foreign:?}. Snapshot layouts must use only the bundled fonts \
             (corpus::pin_fonts pins a document's fonts; a craft-fonts build, CRAFT_FONTS_DIR, lays text out differently from the goldens captured without it)."
        ));
    }
    (l, fonts)
}

#[test]
fn layout_snapshots() {
    bundled_fonts_only();
    let mut fails = Failures::default();
    for d in selected(corpus::corpus()) {
        for (variant, opts) in &d.variants {
            let what = format!("layout/{}.{variant}", d.name);
            let (l, fonts) = checked_layout(&mut fails, &what, &d.doc, opts);
            let text = dump::layout_text(&d.doc, &l, d.name, variant, &fonts);
            golden::check_text(&mut fails, "layout", &format!("{}.{variant}", d.name), &text);
        }
    }
    fails.finish("layout");
}

/// The PostScript family names the bundled fonts embed as (`/BaseFont/XXXXXX+SourceSerif4-…`).
const BUNDLED_PS: &[&str] = &["SourceSerif4", "SourceSans3", "Inter", "JetBrainsMono"];

#[test]
fn pdf_snapshots() {
    bundled_fonts_only();
    let mut fails = Failures::default();
    for d in selected(corpus::corpus()) {
        let opts = wordcraft_pdf::PdfOptions {
            compress: false,
            include_markup: d.export.markup,
            comments: d.export.markup && d.export.comments,
            ..Default::default()
        };
        // Laid out as the export lays out (Print Layout, deletions as the options say, no
        // proofing: the squiggles aren't drawn).
        let lo = wordcraft_pdf::layout_options(&opts);
        let (l, _) = checked_layout(&mut fails, &format!("pdf/{}", d.name), &d.doc, &lo);
        let bytes = match wordcraft_pdf::export_layout(&d.doc, &l, &opts) {
            Ok(b) => b,
            Err(e) => {
                fails.push(format!("--- pdf/{}: the export failed: {e}", d.name));
                continue;
            }
        };
        let text = dump::pdf_text(&bytes);
        let foreign: Vec<&str> = text
            .lines()
            .filter_map(|l| l.split("/BaseFont/").nth(1))
            .map(|f| f.split(['/', '>', ' ']).next().unwrap_or(""))
            .map(|f| f.split('+').next_back().unwrap_or(f))
            .filter(|f| !BUNDLED_PS.iter().any(|b| f.starts_with(b)))
            .collect();
        if !foreign.is_empty() {
            fails.push(format!("--- pdf/{}: the PDF embeds fonts that aren't bundled: {foreign:?} (see layout_snapshots' font rule).", d.name));
        }
        if std::env::var_os("WC_SNAPSHOT_OUT").is_some() {
            let _ = std::fs::create_dir_all(golden::out_dir().join("pdf"));
            let _ = std::fs::write(golden::out_dir().join("pdf").join(format!("{}.pdf", d.name)), &bytes);
        }
        golden::check_text(&mut fails, "pdf", d.name, &text);
    }
    fails.finish("pdf");
}

/// The PDF export lays out without proofing: on every corpus document, with the
/// export options its snapshot uses, the bytes are those of an export from a layout with
/// proofing on (no output draws the squiggles), so the export can skip the spelling check.
#[test]
fn pdf_export_is_the_same_bytes_with_proofing_on() {
    bundled_fonts_only();
    let mut fails = Vec::new();
    let mut checked = 0;
    for d in selected(corpus::corpus()) {
        let opts = wordcraft_pdf::PdfOptions {
            compress: false,
            include_markup: d.export.markup,
            comments: d.export.markup && d.export.comments,
            ..Default::default()
        };
        let proofed = LayoutOptions { proofing: true, ..wordcraft_pdf::layout_options(&opts) };
        let lay = wordcraft_layout::layout(&d.doc, &mut wordcraft_layout::LayoutCache::new(), &proofed);
        match (wordcraft_pdf::export_layout(&d.doc, &lay, &opts), wordcraft_pdf::export(&d.doc, &opts)) {
            (Ok(a), Ok(b)) if a == b => checked += 1,
            (Ok(a), Ok(b)) => fails.push(format!("{}: the bytes differ ({} vs {} bytes)", d.name, a.len(), b.len())),
            (a, b) => fails.push(format!("{}: {:?} / {:?}", d.name, a.err(), b.err())),
        }
    }
    assert!(fails.is_empty(), "{}", fails.join("\n"));
    assert!(checked > 0);
}

/// Pixels a point for the page images: a Letter page is 306×396.
const RENDER_SCALE: f32 = 0.5;

fn render_options(dark: bool) -> wordcraft_render::RenderOptions {
    wordcraft_render::RenderOptions { dark, threads: Some(0), ..Default::default() }
}

#[test]
fn render_snapshots() {
    bundled_fonts_only();
    let mut fails = Failures::default();
    for d in selected(corpus::corpus()) {
        if d.render.is_empty() {
            continue;
        }
        let (l, _) = checked_layout(&mut fails, &format!("render/{}", d.name), &d.doc, &d.variants[0].1);
        for &page in d.render {
            let Some(p) = l.pages.get(page) else {
                fails.push(format!("--- render/{}.p{}: the layout has only {} page(s)", d.name, page + 1, l.pages.len()));
                continue;
            };
            for (mode, dark) in [("light", false), ("dark", true)] {
                let img = wordcraft_render::render_page(&d.doc, p, RENDER_SCALE, &render_options(dark));
                golden::check_png(&mut fails, "render", &format!("{}.p{}.{mode}", d.name, page + 1), &img);
            }
        }
    }
    fails.finish("render");
}

/// Every page of every document, light mode, as a pixel hash: the whole corpus rasterised.
/// Heavier and stricter than `render_snapshots` (exact pixels, no tolerance), so it is ignored
/// by default: `cargo test -p wordcraft-engine --test snapshots render_every_page -- --ignored`.
#[test]
#[ignore]
fn render_every_page_snapshots() {
    bundled_fonts_only();
    let mut fails = Failures::default();
    let mut text = String::from("# every page of every document at 0.5 px/pt, light mode: sha256 of the RGBA pixels\n");
    for d in selected(corpus::corpus()) {
        let (l, _) = checked_layout(&mut fails, &format!("render/{} (every page)", d.name), &d.doc, &d.variants[0].1);
        for (i, p) in l.pages.iter().enumerate() {
            let img = wordcraft_render::render_page(&d.doc, p, RENDER_SCALE, &render_options(false));
            let _ = writeln!(text, "{} p{} {}x{} {}", d.name, i + 1, img.width, img.height, dump::sha256(&img.to_straight()));
        }
    }
    golden::check_text(&mut fails, "render", "every_page", &text);
    fails.finish("render");
}
