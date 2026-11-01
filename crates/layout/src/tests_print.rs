//! Printing from the window's layout ([`prints_alike`]): wherever it says a layout made with the
//! window's options is the print layout, its pages draw what a layout made with the print options
//! draws, text and positions alike (proofing squiggles aside, which print leaves out).

use super::*;
use std::sync::Arc;
use wordcraft_doc::para::{InlineObject, NoteKind};
use wordcraft_doc::props::CharProps;
use wordcraft_doc::{Block, Paragraph, PartKind, Table, para_block};

const TEXT: &str = "The quick brown fox jumps over the lazy dog while the band plays on; teh misspelt wrod stays. ";

/// A paragraph of plain text with, as asked, a hidden run, a tracked deletion and a field or a
/// footnote reference.
fn paragraph(doc: &mut Document, i: usize, hidden: bool, deleted: bool) -> Paragraph {
    let mut p = Paragraph::with_text(&TEXT.repeat(1 + i % 4), Default::default());
    if hidden && i.is_multiple_of(3) {
        let _ = p.insert_text(4, "hidden words here ", &CharProps { hidden: Some(true), ..Default::default() });
    }
    if deleted && i % 3 == 1 {
        let _ = p.insert_text(10, "deleted words here ", &CharProps { del: Some(0), ..Default::default() });
    }
    match i % 5 {
        0 => {
            let field = InlineObject::Field {
                instr: "NUMPAGES".into(),
                result: "1".into(),
                locked: false,
                dirty: false,
                code: false,
                form: String::new(),
                nested: Vec::new(),
            };
            let _ = p.insert_object(0, field, &Default::default());
        }
        1 => {
            let id = doc.add_part(PartKind::Footnote, vec![para_block(Paragraph::with_text("A footnote.", Default::default()))]);
            let _ = p.insert_object(3, InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() }, &Default::default());
        }
        _ => {}
    }
    p
}

/// About ten pages of paragraphs and a table, with hidden text and tracked deletions as asked,
/// fields, footnotes and a header with a tracked deletion of its own.
fn document(hidden: bool, deleted: bool) -> Document {
    let mut d = Document::new();
    d.body.clear();
    if deleted {
        let revisions = wordcraft_doc::Shared::make_mut(&mut d.revisions);
        revisions.push(wordcraft_doc::Revision::new(wordcraft_doc::RevisionKind::Delete, "A", ""));
    }
    for i in 0..60 {
        let p = paragraph(&mut d, i, hidden, deleted);
        d.body.push(para_block(p));
        if i == 30 {
            let mut t = Table::new(12, 3, 468.0);
            for (r, row) in t.rows.iter_mut().enumerate() {
                for c in &mut row.cells {
                    let p = paragraph(&mut d, r, hidden, deleted);
                    c.blocks = vec![para_block(p)];
                }
            }
            t.props.fixed = false;
            d.body.push(Arc::new(Block::Table(t)));
        }
    }
    let head = paragraph(&mut d, 1, hidden, deleted);
    let h = d.add_part(PartKind::Header, vec![para_block(head)]);
    d.last_section.headers.default = Some(h);
    d
}

/// Each page's draw items with the squiggles left out, as print draws them.
fn drawn(doc: &Document, l: &DocLayout) -> Vec<String> {
    let opts = display::DisplayOptions { proofing: false, dim_header: false, ..Default::default() };
    l.pages.iter().map(|p| format!("{} {} {:?}", p.w, p.h, display::page_display(doc, p, &opts))).collect()
}

/// Window options: Print Layout or another view, hidden text shown or not, deletions hidden
/// (No Markup, balloons) or not, squiggles, field codes, digits, window width.
fn screen_options(k: u32) -> LayoutOptions {
    let bit = |n: u32| k >> n & 1 == 1;
    LayoutOptions {
        view: if k % 11 == 10 { ViewMode::Web } else { ViewMode::Print },
        web_width: 300.0 + (k % 7) as f32 * 50.0,
        show_hidden: bit(0),
        hide_deleted: bit(1),
        deletions_in_balloons: bit(2),
        proofing: bit(3),
        field_codes: k % 13 == 12,
        numeral: if k % 9 == 8 { NumeralMode::Hindi } else { NumeralMode::Arabic },
        ..Default::default()
    }
}

/// A layout the window's options make is the print layout wherever `prints_alike` says so, and
/// it says so in the common cases: the window in Print Layout with squiggles, printing without
/// markup a document without tracked deletions, and with markup one that has some.
#[test]
fn layouts_alike_for_print_draw_the_same() {
    let (mut alike, mut differ) = (0, 0);
    for (hidden, deleted) in [(false, false), (true, false), (false, true), (true, true)] {
        let doc = document(hidden, deleted);
        // What a PDF without markup, with markup, and with markup and comments lays out with.
        let prints = [
            LayoutOptions { hide_deleted: true, ..Default::default() },
            LayoutOptions { hide_deleted: false, ..Default::default() },
            LayoutOptions { comment_balloons: true, ..Default::default() },
        ];
        let print_pages: Vec<Vec<String>> = prints.iter().map(|o| drawn(&doc, &layout(&doc, &mut LayoutCache::new(), o))).collect();
        for k in 0..48 {
            let screen = screen_options(k);
            let screen_pages = drawn(&doc, &layout(&doc, &mut LayoutCache::new(), &screen));
            for (print, pages) in prints.iter().zip(&print_pages) {
                if prints_alike(&screen, print, &doc) {
                    alike += 1;
                    assert_eq!(screen_pages.len(), pages.len(), "hidden {hidden}, deleted {deleted}, screen {screen:?}, print {print:?}");
                    for (pi, (a, b)) in screen_pages.iter().zip(pages).enumerate() {
                        assert!(a == b, "hidden {hidden}, deleted {deleted}, screen {screen:?}, print {print:?}: page {pi} differs");
                    }
                } else {
                    differ += 1;
                }
            }
        }
        // The common case: the window shows the document with squiggles and all markup, and it
        // prints without markup (or with it, which keeps the deletions as the window shows them).
        let window = LayoutOptions { proofing: true, web_width: 812.0, ..Default::default() };
        assert_eq!(prints_alike(&window, &prints[0], &doc), !deleted, "hidden {hidden}, deleted {deleted}");
        assert!(prints_alike(&window, &prints[1], &doc));
        assert!(!prints_alike(&window, &prints[2], &doc), "Print Markup's comments widen the pages");
    }
    eprintln!("PRINTS ALIKE {alike}, not {differ}");
    assert!(alike > 50 && differ > 50, "alike {alike}, not {differ}");
}
