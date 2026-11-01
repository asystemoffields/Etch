//! Printing and exporting from the window's layout ([`Session::print_layout`]): a PDF printed,
//! exported or saved, and a page image, are the same bytes as from a layout made from scratch
//! with the output's options, in every view state the window can be in; and they cost no layout
//! of their own when the window's layout is the print layout.

use serde_json::json;
use wordcraft_doc::{Document, Pos};

use crate::Session;

/// A PDF of `doc` laid out from scratch with the options a copy saved with Print Markup
/// `markup` (and `comments`) has.
fn fresh_pdf(doc: &Document, markup: bool, comments: bool) -> Vec<u8> {
    wordcraft_pdf::export(doc, &wordcraft_pdf::PdfOptions { include_markup: markup, comments, ..Default::default() }).unwrap()
}

/// A document's creation date, which the PDF writer records (`CreationDate`). Without one it
/// records the wall clock, and two exports a second apart differ in those bytes.
const CREATED: &str = "2023-11-14T22:13:20Z";

/// `s` on a stopped clock, its document dated: every PDF written from it has the same dates,
/// so their bytes compare (the stamps a save adds come from the clock too).
fn pinned(mut s: Session) -> Session {
    s.clock = crate::clock::Clock::fixed(1_700_000_000, wordcraft_tz::Zone::UTC);
    s.doc.core.created = CREATED.into();
    s.doc.core.modified = CREATED.into();
    s
}

/// Documents to print: the sample, and one with tracked changes, a comment, a footnote, hidden
/// text and an AutoFit table made through the commands.
fn documents() -> Vec<(&'static str, Session)> {
    let sample = pinned(Session::new(crate::sample::sample_document()));
    let mut s = pinned(Session::new(Document::from_text(&"The quick brown fox jumps over the lazy dog. ".repeat(40))));
    let run = |s: &mut Session, id: &str, v: serde_json::Value| s.run(id, &v).unwrap_or_else(|e| panic!("{id}: {e}"));
    run(&mut s, "select.text", json!({"text": "quick brown"}));
    run(&mut s, "format.hidden", json!({}));
    run(&mut s, "review.trackChanges", json!({"value": true}));
    run(&mut s, "select.text", json!({"text": "lazy dog"}));
    run(&mut s, "text.delete", json!({}));
    run(&mut s, "text.insert", json!({"text": "sleepy cat "}));
    run(&mut s, "review.newComment", json!({"text": "A comment."}));
    run(&mut s, "references.footnote", json!({"text": "A footnote."}));
    s.sel = crate::Selection::caret(Pos::body(0, 0));
    run(&mut s, "insert.table", json!({"rows": 6, "cols": 3}));
    run(&mut s, "text.insert", json!({"text": "Supercalifragilisticexpialidocious"}));
    vec![("sample", sample), ("tracked", s)]
}

/// The view states the window can print from: as opened, without squiggles, with ¶ shown (and
/// hidden text with it), No Markup, Print Markup, field codes, Web Layout, and back; then with the
/// tracked changes accepted.
const STATES: &[(&str, &str)] = &[
    ("as opened", ""),
    ("spelling off", r#"review.proofing {"value": false}"#),
    ("marks shown", r#"view.marks {"value": true}"#),
    ("no markup", r#"review.markup {"value": "noMarkup"}"#),
    ("marks hidden", r#"view.marks {"value": false}"#),
    ("all markup", r#"review.markup {"value": "all"}"#),
    ("print markup", r#"file.printMarkup {"value": true}"#),
    ("no print markup", r#"file.printMarkup {"value": false}"#),
    ("field codes", r#"view.fieldCodes {"value": true}"#),
    ("results", r#"view.fieldCodes {"value": false}"#),
    ("web layout", "view.webLayout {}"),
    ("print layout", "view.printLayout {}"),
    ("spelling on", r#"review.proofing {"value": true}"#),
    ("changes accepted", "review.acceptAll {}"),
    ("no markup again", r#"review.markup {"value": "noMarkup"}"#),
];

/// File › Export › PDF, Save As PDF and Print write the same PDF from the window's layout as from
/// a layout from scratch (PDF writing is deterministic, so the bytes compare), and so does a page
/// image; in Print Layout with the print options' text, none lays the document out again.
#[test]
fn output_from_the_window_layout_is_output_from_scratch() {
    let dir = std::env::temp_dir().join(format!("wc-print-layout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = |n: &str| dir.join(n).to_string_lossy().into_owned();
    let mut reused = 0;
    for (name, mut s) in documents() {
        // Writing is deterministic: the same layout gives the same bytes, dated from the
        // document (not the wall clock, which two exports a second apart would straddle).
        let first = fresh_pdf(&s.doc, false, false);
        assert_eq!(first, fresh_pdf(&s.doc, false, false), "{name}");
        assert!(first.windows(16).any(|w| w == b"D:20231114221320"), "{name}: the PDF is dated from the document");
        for (state, cmd) in STATES {
            if let Some((id, v)) = cmd.split_once(' ') {
                s.run(id, &serde_json::from_str(v).unwrap()).unwrap();
            }
            // The window draws its layout first.
            let _ = s.layout();
            let markup = s.prefs.print_markup;
            let comments = markup && s.prefs.markup.comments;
            let what = format!("{name}, {state}");
            let before = crate::print::fresh_layouts();
            s.run("file.exportPdf", &json!({"path": path("export.pdf")})).unwrap();
            let fresh = fresh_pdf(&s.doc, markup, comments);
            assert!(std::fs::read(path("export.pdf")).unwrap() == fresh, "{what}: File › Export › PDF");
            let (printed, _) = s.print_pdf(&json!({})).unwrap();
            assert!(printed == fresh, "{what}: Print");
            let page = s
                .run("file.exportPng", &json!({"path": path("page.png"), "page": 1, "scale": 0.5}))
                .map(|_| std::fs::read(path("page.png")).unwrap());
            let lo = s.export_layout_options(markup);
            let l = wordcraft_layout::layout(&s.doc, &mut wordcraft_layout::LayoutCache::new(), &lo);
            let mut ro = wordcraft_render::RenderOptions::default();
            ro.display.dim_header = false;
            let image = wordcraft_render::render_page(&s.doc, &l.pages[0], 0.5, &ro).to_png().unwrap();
            assert!(page.unwrap() == image, "{what}: page image");
            // Without a print layout of its own when the window's is one.
            let alike = wordcraft_layout::prints_alike(
                &s.layout_options(),
                &wordcraft_pdf::layout_options(&crate::io_ext::pdf_options(&crate::cmd::file::export_options(&s, &json!({})))),
                &s.doc,
            );
            let made = crate::print::fresh_layouts() - before;
            if alike {
                reused += 1;
                assert_eq!(made, usize::from(!wordcraft_layout::prints_alike(&s.layout_options(), &lo, &s.doc)), "{what}: {made} layouts made");
            } else {
                assert!(made >= 2, "{what}: {made} layouts made");
            }
            // Save As PDF last: it stamps the document as saved.
            s.run("file.saveAs", &json!({"path": path("saved.pdf")})).unwrap();
            assert!(std::fs::read(path("saved.pdf")).unwrap() == fresh_pdf(&s.doc, markup, comments), "{what}: Save As PDF");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(reused >= 12, "{reused} states printed from the window's layout");
}

/// A session no window has laid the document out for (the command line's `convert`) writes its
/// output from a layout made for the output alone, as from scratch: it makes no window layout,
/// which would check the spelling of every word for squiggles nothing draws (a cold convert of a
/// 200-table document took 13% more instructions and 13 MB more with it). Once the window has
/// laid the document out, output takes the window's layout.
#[test]
fn output_of_a_document_no_window_laid_out_makes_no_window_layout() {
    let dir = std::env::temp_dir().join(format!("wc-print-unshown-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = |n: &str| dir.join(n).to_string_lossy().into_owned();
    // On a stopped clock with a dated document, as the test above: the PDFs' bytes compare
    // only when their dates don't come from the wall clock.
    let mut s = pinned(Session::new(crate::sample::sample_document()));
    let before = crate::print::fresh_layouts();
    s.run("file.exportPdf", &json!({"path": path("export.pdf")})).unwrap();
    assert!(std::fs::read(path("export.pdf")).unwrap() == fresh_pdf(&s.doc, false, false), "File › Export › PDF");
    let (printed, _) = s.print_pdf(&json!({})).unwrap();
    assert!(printed == fresh_pdf(&s.doc, false, false), "Print");
    s.run("file.saveAs", &json!({"path": path("saved.pdf")})).unwrap();
    assert!(std::fs::read(path("saved.pdf")).unwrap() == fresh_pdf(&s.doc, false, false), "Save As PDF");
    assert_eq!(crate::print::fresh_layouts() - before, 3, "each laid out for the output");
    assert_eq!((s.cache_len(), s.cache_misses()), (0, 0), "no window layout was made");
    // The window lays it out: output takes that layout from then on.
    let _ = s.layout();
    let before = crate::print::fresh_layouts();
    s.run("file.exportPdf", &json!({"path": path("export.pdf")})).unwrap();
    assert!(std::fs::read(path("export.pdf")).unwrap() == fresh_pdf(&s.doc, false, false), "File › Export › PDF");
    assert_eq!(crate::print::fresh_layouts() - before, 0, "the window's layout");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Print without Print Markup prints the final text, as File › Export › PDF does: tracked
/// deletions take no room. (Print laid them out and left blank space where they were.)
#[test]
fn printing_without_markup_leaves_tracked_deletions_out() {
    let mut s = Session::new(Document::from_text("Keep this text, cut these words out, and keep the rest."));
    s.run("review.trackChanges", &json!({"value": true})).unwrap();
    s.run("select.text", &json!({"text": "cut these words out, "})).unwrap();
    s.run("text.delete", &json!({})).unwrap();
    let (printed, _) = s.print_pdf(&json!({})).unwrap();
    assert!(printed == fresh_pdf(&s.doc, false, false), "the final text");
    let kept = wordcraft_layout::LayoutOptions { hide_deleted: false, ..Default::default() };
    let gaps = wordcraft_layout::layout(&s.doc, &mut wordcraft_layout::LayoutCache::new(), &kept);
    let opts = wordcraft_pdf::PdfOptions { pages: Some(vec![0]), ..Default::default() };
    assert!(printed != wordcraft_pdf::export_layout(&s.doc, &gaps, &opts).unwrap(), "not the text with room left for the deletion");
}
