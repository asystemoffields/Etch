//! AutoFit tables while typing. A column AutoFit widens or narrows lays its cells out again by
//! breaking their lines again (`para::rebreak`) instead of shaping their text again, with the same
//! result as laying them out; typing that leaves the columns alone measures and lays out only the
//! cell typed in. Every layout here is checked against one from scratch.

use super::*;
use std::sync::Arc;
use wordcraft_doc::numbering::ListKind;
use wordcraft_doc::para::{InlineObject, NoteKind};
use wordcraft_doc::props::{Align, LineSpacing, NumRef, ParaProps, TabAlign, TabStop};
use wordcraft_doc::{Block, Paragraph, PartKind, Table, para_block};

/// A small deterministic generator (the same sequence every run).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const WORDS: &[&str] = &[
    "alpha",
    "bravo",
    "charlie",
    "delta",
    "echo",
    "foxtrot",
    "golf",
    "hotel",
    "india",
    "juliet",
    "Pneumonoultramicroscopic",
    "שלום",
    "مرحبا",
    "日本語のテキスト",
    "co\u{AD}op\u{AD}er\u{AD}a\u{AD}tion",
    "and/or",
    "x",
];

fn words(rng: &mut Lcg, n: usize) -> String {
    (0..n).map(|_| WORDS[rng.below(WORDS.len())]).collect::<Vec<_>>().join(" ")
}

/// A paragraph of one of many kinds: alignments, indents, line spacing, right to left, hyphens,
/// line breaks, a list, fields, notes, a picture, a tab or a drop cap.
fn some_paragraph(rng: &mut Lcg, doc: &mut Document, list: u32) -> Paragraph {
    let n = 1 + rng.below(30);
    let mut text = words(rng, n);
    if rng.below(5) == 0 {
        text.push(wordcraft_doc::para::LINE_BREAK);
        text.push_str(&words(rng, 3));
    }
    let tabbed = rng.below(8) == 0;
    if tabbed {
        text.push('\t');
        text.push_str("12.50 tabbed");
    }
    let mut p = Paragraph::with_text(&text, Default::default());
    let aligns = [None, Some(Align::Left), Some(Align::Center), Some(Align::Right), Some(Align::Justify), Some(Align::Distribute)];
    p.props = ParaProps {
        align: aligns[rng.below(aligns.len())],
        indent_left: [None, Some(18.0), Some(-9.0)][rng.below(3)],
        indent_right: [None, Some(12.0)][rng.below(2)],
        indent_first: [None, Some(24.0), Some(-18.0)][rng.below(3)],
        line_spacing: [None, Some(LineSpacing::Multiple(1.5)), Some(LineSpacing::Exactly(20.0)), Some(LineSpacing::AtLeast(9.0))][rng.below(4)],
        bidi: (rng.below(6) == 0).then_some(true),
        numbering: (rng.below(4) == 0).then_some(NumRef { num: list, level: 0 }),
        drop_cap: (rng.below(15) == 0).then_some(2),
        word_wrap: (rng.below(10) == 0).then_some(false),
        // Right, centre and decimal tabs place the text after them from where it ends.
        tabs: tabbed.then(|| {
            let align = [TabAlign::Left, TabAlign::Right, TabAlign::Center, TabAlign::Decimal][rng.below(4)];
            vec![TabStop { pos: 30.0 + rng.below(150) as f32, align, leader: Default::default() }]
        }),
        ..Default::default()
    };
    let len = p.text.len();
    let at = |rng: &mut Lcg, p: &Paragraph| p.text.char_indices().map(|(i, _)| i).nth(rng.below(p.text.chars().count())).unwrap_or(len);
    match rng.below(8) {
        0 => {
            let off = at(rng, &p);
            let field = InlineObject::Field {
                instr: "PAGE".into(),
                result: "1".into(),
                locked: false,
                dirty: false,
                code: false,
                form: String::new(),
                nested: Vec::new(),
            };
            let _ = p.insert_object(off, field, &Default::default());
        }
        1 => {
            let id = doc.add_part(PartKind::Footnote, vec![para_block(Paragraph::with_text("A note.", Default::default()))]);
            let off = at(rng, &p);
            let _ = p.insert_object(off, InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() }, &Default::default());
        }
        2 => {
            let media = doc.add_media(vec![0x89, b'P', b'N', b'G'], "png");
            let pic = InlineObject::Image {
                media,
                w: 40.0 + rng.below(300) as f32,
                h: 30.0,
                alt: String::new(),
                float: Default::default(),
                crop: [0.0; 4],
                ole: None,
            };
            let off = at(rng, &p);
            let _ = p.insert_object(off, pic, &Default::default());
        }
        _ => {}
    }
    p.touch();
    p
}

/// Where two texts first differ, with some of each from there.
fn first_difference(a: &str, b: &str) -> String {
    let at = a.char_indices().zip(b.chars()).find(|((_, x), y)| x != y).map_or(a.len().min(b.len()), |((i, _), _)| i);
    let from = a.floor_char_boundary(at.saturating_sub(120));
    let tail = |s: &str| s.get(from..).unwrap_or("").chars().take(300).collect::<String>();
    format!("at byte {at}:\n  {}\n  {}", tail(a), tail(b))
}

/// Breaking a paragraph's lines again at another width gives what laying it out there does,
/// for paragraphs of every kind `some_paragraph` makes; the kinds it can't (tabs, drop caps)
/// are laid out instead.
#[test]
fn rebreaking_at_another_width_is_laying_out_there() {
    let mut doc = Document::new();
    doc.settings.auto_hyphenation = true;
    let list = doc.numbering.add_list(ListKind::Numbered);
    let mut rng = Lcg(7);
    let paras: Vec<Paragraph> = (0..400).map(|_| some_paragraph(&mut rng, &mut doc, list)).collect();
    let fields = FieldCtx::default();
    let (mut rebroken, mut refused) = (0, 0);
    for (i, p) in paras.iter().enumerate() {
        let label = p.props.numbering.and_then(|n| wordcraft_doc::numbering::Counters::default().next_label(&doc.numbering, n.num, n.level));
        let env = |width: f32| para::ParaEnv {
            doc: &doc,
            width,
            label: label.clone(),
            fields: &fields,
            show_hidden: false,
            field_codes: false,
            hide_deleted: crate::HideDeleted::default(),
            typing_word: None,
            fitted: &[],
            table: None,
            proofing: Some(ProofOptions::default()),
            numeral: NumeralMode::default(),
            exclusions: &[],
            eq_number: 0,
            read_line_spacing: None,
            grid: para::Grid::default(),
            fit_objects: false,
        };
        let w1 = [40.0, 90.0, 150.0, 468.0][i % 4];
        let w2 = [12.0, 61.5, 93.6, 200.0, 2000.0][rng.below(5)];
        let old = para::layout_para(p, &env(w1));
        match para::rebreak(&old, p, &env(w2)) {
            Some(again) => {
                let fresh = para::layout_para(p, &env(w2));
                let (a, b) = (format!("{again:?}"), format!("{fresh:?}"));
                assert!(a == b, "paragraph {i} {:?} from {w1} to {w2}: {}", p.text, first_difference(&a, &b));
                assert!(again.shares_shaping(&old), "paragraph {i}: broken again without copying its shaped text");
                rebroken += 1;
            }
            None => {
                let tab = p.text.contains('\t');
                assert!(tab || p.props.drop_cap.is_some(), "paragraph {i} {:?} wasn't broken again", p.text);
                refused += 1;
            }
        }
    }
    assert!(rebroken > 300 && refused > 10, "rebroken {rebroken}, refused {refused}");
}

/// A paragraph with a picture shrunk to fit a text box is laid out again at another width: the
/// picture's size follows the width.
#[test]
fn a_picture_fitted_to_its_box_is_laid_out_again() {
    let mut doc = Document::new();
    let media = doc.add_media(vec![0x89, b'P', b'N', b'G'], "png");
    let mut p = Paragraph::with_text("before  after", Default::default());
    let pic = InlineObject::Image { media, w: 300.0, h: 30.0, alt: String::new(), float: Default::default(), crop: [0.0; 4], ole: None };
    p.insert_object(7, pic, &Default::default()).unwrap();
    let fields = FieldCtx::default();
    let env = |width: f32, fit_objects: bool| para::ParaEnv {
        doc: &doc,
        width,
        label: None,
        fields: &fields,
        show_hidden: false,
        field_codes: false,
        hide_deleted: crate::HideDeleted::default(),
        typing_word: None,
        fitted: &[],
        table: None,
        proofing: None,
        numeral: NumeralMode::default(),
        exclusions: &[],
        eq_number: 0,
        read_line_spacing: None,
        grid: para::Grid::default(),
        fit_objects,
    };
    let old = para::layout_para(&p, &env(200.0, true));
    assert!(para::rebreak(&old, &p, &env(100.0, true)).is_none());
    let old = para::layout_para(&p, &env(200.0, false));
    let again = para::rebreak(&old, &p, &env(100.0, false)).unwrap();
    assert_eq!(format!("{again:?}"), format!("{:?}", para::layout_para(&p, &env(100.0, false))));
}

fn para(text: &str) -> Arc<Block> {
    para_block(Paragraph::with_text(text, Default::default()))
}

/// Long AutoFit tables between paragraphs: one whose cells have preferred widths (AutoFit only
/// widens a column for a word too long for it), one whose cells ask for none (its columns follow
/// their content), one of those with a cell spanning two columns, and a numbered first column.
fn autofit_tables(seed: u64) -> Document {
    let mut rng = Lcg(seed);
    let mut d = Document::new();
    d.body.clear();
    let list = d.numbering.add_list(ListKind::Numbered);
    for k in 0..3 {
        d.body.push(para(&words(&mut rng, 40)));
        let rows = 60 + rng.below(60);
        let mut t = Table::new(rows, 4, 468.0);
        for (r, row) in t.rows.iter_mut().enumerate() {
            for (c, cell) in row.cells.iter_mut().enumerate() {
                let n = 1 + (r + c) % 4;
                let mut p = Paragraph::with_text(&words(&mut rng, n).replace("Pneumonoultramicroscopic", "long"), Default::default());
                if k == 2 && c == 0 {
                    p.props.numbering = Some(NumRef { num: list, level: 0 });
                }
                cell.blocks = vec![para_block(p)];
                if k > 0 {
                    cell.props.width = None;
                }
            }
        }
        if k > 0 {
            t.grid.clear();
        }
        if k == 1 {
            // A cell spanning the middle two columns.
            let row = wordcraft_doc::Shared::make_mut(&mut t.rows[5]);
            row.cells.remove(2);
            row.cells[1].props.span = 2;
        }
        t.rows[0].props.header = true;
        d.body.push(Arc::new(Block::Table(t)));
    }
    d.body.push(para(&words(&mut rng, 40)));
    d
}

/// The body tables' indexes.
fn tables_in(d: &Document) -> Vec<usize> {
    d.body.iter().enumerate().filter(|(_, b)| matches!(&***b, Block::Table(_))).map(|(i, _)| i).collect()
}

/// A cell's first paragraph.
fn cell_para(d: &mut Document, ti: usize, r: usize, c: usize) -> Option<&mut Paragraph> {
    let Block::Table(t) = Arc::make_mut(d.body.get_mut(ti)?) else { return None };
    let row = wordcraft_doc::Shared::make_mut(t.rows.get_mut(r)?);
    match row.cells.get_mut(c)?.blocks.first_mut().map(Arc::make_mut)? {
        Block::Para(p) => Some(p),
        Block::Table(_) => None,
    }
}

/// One edit in a cell of an AutoFit table, as the keyboard or a paste makes it: a letter typed,
/// a long word growing letter by letter (its column widens), Backspace (it narrows), a word
/// pasted, a cell's text deleted, or Enter in a cell.
fn autofit_edit(d: &mut Document, rng: &mut Lcg, long: &mut Option<(usize, usize, usize)>) -> &'static str {
    // Keep growing or shrinking the long word for a while.
    if let Some((ti, r, c)) = *long
        && rng.below(12) != 0
    {
        let grow = rng.below(3) != 0;
        let Some(p) = cell_para(d, ti, r, c) else { return "nothing" };
        if grow {
            let _ = p.insert_text(0, "w", &Default::default());
        } else if !p.text.is_empty() {
            let end = p.text.char_indices().nth(1).map_or(p.text.len(), |(i, _)| i);
            let _ = p.delete(0, end);
        }
        p.touch();
        return if grow { "grow the long word" } else { "backspace in the long word" };
    }
    let tables = tables_in(d);
    let ti = tables[rng.below(tables.len())];
    let Some(Block::Table(t)) = d.body.get(ti).map(|b| &**b) else { return "nothing" };
    let r = rng.below(t.rows.len());
    let c = rng.below(t.rows[r].cells.len());
    match rng.below(7) {
        0 => {
            *long = Some((ti, r, c));
            "start a long word"
        }
        1 => {
            let Some(p) = cell_para(d, ti, r, c) else { return "nothing" };
            let _ = p.insert_text(0, "Supercalifragilisticexpialidocious ", &Default::default());
            p.touch();
            "paste a long word"
        }
        2 => {
            let Some(p) = cell_para(d, ti, r, c) else { return "nothing" };
            let len = p.text.len();
            let _ = p.delete(0, len);
            p.touch();
            "delete a cell's text"
        }
        3 => {
            let Block::Table(t) = Arc::make_mut(&mut d.body[ti]) else { return "nothing" };
            let row = wordcraft_doc::Shared::make_mut(&mut t.rows[r]);
            row.cells[c].blocks.push(para("a second paragraph"));
            "enter in a cell"
        }
        _ => {
            let w = words(rng, 1);
            let Some(p) = cell_para(d, ti, r, c) else { return "nothing" };
            let _ = p.insert_text(0, &format!("{w} "), &Default::default());
            p.touch();
            "type a word"
        }
    }
}

/// Typing, deleting and pasting in cells of AutoFit tables, columns widening and narrowing:
/// every layout resumed from the last is the layout from scratch, and most cells laid out again
/// for a column's new width had their lines broken again rather than their text shaped again.
#[test]
fn typing_in_autofit_tables_matches_layouts_from_scratch() {
    let opts = LayoutOptions::default();
    let (mut rebroken, mut misses, mut widened) = (0, 0, 0);
    for seed in 1..=3 {
        let mut d = autofit_tables(seed);
        let mut cache = LayoutCache::new();
        let mut prev = layout(&d, &mut cache, &opts);
        let mut rng = Lcg(seed * 97);
        let mut long = None;
        let mut history: Vec<Document> = Vec::new();
        for step in 0..80 {
            let what = if step % 11 == 10 && !history.is_empty() {
                d = history.pop().unwrap();
                long = None;
                "undo"
            } else {
                history.push(d.clone());
                autofit_edit(&mut d, &mut rng, &mut long)
            };
            let (r0, m0) = (cache.rebroken, cache.misses);
            let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
            rebroken += cache.rebroken - r0;
            misses += cache.misses - m0;
            widened += usize::from(cache.rebroken > r0);
            let full = layout(&d, &mut LayoutCache::new(), &opts);
            assert!(
                format!("{:?}", inc.pages) == format!("{:?}", full.pages),
                "seed {seed}, step {step} ({what}): resumed from {:?} row {:?}, spliced at {:?}",
                cache.resumed,
                cache.resumed_row,
                cache.spliced
            );
            prev = inc;
        }
    }
    eprintln!("AUTOFIT rebroken {rebroken} of {misses} missed, {widened} passes widened or narrowed a column");
    assert!(widened > 20 && rebroken * 10 > misses * 8, "rebroken {rebroken} of {misses} missed, {widened} passes");
}

/// The work a keystroke in a long AutoFit table does: one that leaves the columns as they were
/// measures and lays out only its cell and takes the other rows as they were; one that widens a
/// column lays every cell out again at the new widths, breaking their lines again, but shapes
/// only the cell typed in.
#[test]
fn a_keystroke_in_an_autofit_table_does_only_the_work_it_must() {
    let opts = LayoutOptions::default();
    let mut d = Document::new();
    d.body.clear();
    d.body.push(para("Before the table."));
    let mut t = Table::new(400, 3, 468.0);
    for (r, row) in t.rows.iter_mut().enumerate() {
        for (c, cell) in row.cells.iter_mut().enumerate() {
            cell.blocks = vec![para(&format!("row {r} cell {c}"))];
        }
    }
    d.body.push(Arc::new(Block::Table(t)));
    d.body.push(para("After the table."));
    let mut cache = LayoutCache::new();
    let mut prev = layout(&d, &mut cache, &opts);
    let cells = 400 * 3;
    // A short word: the column keeps its width.
    let mut key = |d: &mut Document, text: &str| {
        let p = cell_para(d, 1, 200, 0).unwrap();
        p.insert_text(0, text, &Default::default()).unwrap();
        p.touch();
        let (m0, r0, me0) = (cache.misses, cache.rebroken, cache.measured);
        let l = layout_after(d, &mut cache, &opts, Some(&prev));
        let work = (cache.misses - m0, cache.rebroken - r0, cache.measured - me0, cache.rows_reused);
        assert_eq!(format!("{:?}", l.pages), format!("{:?}", layout(d, &mut LayoutCache::new(), &opts).pages));
        prev = l;
        work
    };
    let (misses, rebroken, measured, rows) = key(&mut d, "a");
    assert_eq!((misses, rebroken, measured, rows), (2, 0, 1, 399), "the cell measured and laid out, the other rows taken");
    // A word longer than the column is wide: AutoFit widens it on each key.
    let (misses, rebroken, measured, _) = key(&mut d, "Pneumonoultramicroscopicsilicovolcanoconiosis");
    assert_eq!(measured, 1, "only the cell typed in is measured");
    assert_eq!(misses - measured - rebroken, 1, "only the cell typed in is shaped: {misses} missed, {rebroken} broken again");
    assert!(rebroken >= cells as u64 - 1, "every other cell is broken again at its column's new width: {rebroken}");
    let (misses, rebroken, measured, _) = key(&mut d, "s");
    assert_eq!((measured, misses - measured - rebroken), (1, 1), "{misses} missed, {rebroken} broken again");
}

/// Typing in a cell of a long table looks only at the rows it changes for what the table's memo
/// records (merged or turned cells, note references): the rows another version of the table
/// shares are the same rows. Once a note is in the table, its references are collected from
/// every row again; with merged cells, rows aren't taken from the earlier layout at all. Every
/// layout equals one from scratch.
#[test]
fn keys_in_a_long_table_look_only_at_the_rows_they_change() {
    let opts = LayoutOptions::default();
    let mut d = Document::new();
    d.body.clear();
    d.body.push(para("Before the table."));
    let mut t = Table::new(400, 3, 468.0);
    for (r, row) in t.rows.iter_mut().enumerate() {
        for (c, cell) in wordcraft_doc::Shared::make_mut(row).cells.iter_mut().enumerate() {
            cell.blocks = vec![para(&format!("row {r} cell {c}"))];
        }
    }
    d.body.push(Arc::new(Block::Table(t)));
    d.body.push(para("After the table."));
    let mut cache = LayoutCache::new();
    let mut prev = layout(&d, &mut cache, &opts);
    let mut key = |d: &mut Document, row: usize, text: &str| {
        let p = cell_para(d, 1, row, 1).unwrap();
        p.insert_text(0, text, &Default::default()).unwrap();
        p.touch();
        let l = layout_after(d, &mut cache, &opts, Some(&prev));
        assert_eq!(format!("{:?}", l.pages), format!("{:?}", layout(d, &mut LayoutCache::new(), &opts).pages), "{text}");
        prev = l;
        (cache.rows_scanned, cache.rows_reused)
    };
    // The first key: the table's first layout didn't check its cells, so all its rows are.
    let (scanned, reused) = key(&mut d, 200, "a");
    assert_eq!((scanned, reused), (400 + 1, 399));
    for (i, row) in [200, 200, 37, 399, 0].into_iter().enumerate() {
        assert_eq!(key(&mut d, row, "b"), (2, 399), "key {i}: the row typed in, for its cells and its notes");
    }
    // A footnote in a cell: its references are collected from the whole table from then on.
    let id = d.add_part(PartKind::Footnote, vec![para("A note.")]);
    let p = cell_para(&mut d, 1, 10, 0).unwrap();
    p.insert_object(0, InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() }, &Default::default()).unwrap();
    p.touch();
    let l = layout_after(&d, &mut cache, &opts, Some(&prev));
    assert_eq!(format!("{:?}", l.pages), format!("{:?}", layout(&d, &mut LayoutCache::new(), &opts).pages), "the note");
    prev = l;
    let mut key = |d: &mut Document, row: usize| {
        let p = cell_para(d, 1, row, 1).unwrap();
        p.insert_text(0, "c", &Default::default()).unwrap();
        p.touch();
        let l = layout_after(d, &mut cache, &opts, Some(&prev));
        assert_eq!(format!("{:?}", l.pages), format!("{:?}", layout(d, &mut LayoutCache::new(), &opts).pages));
        prev = l;
        (cache.rows_scanned, cache.rows_reused)
    };
    assert_eq!(key(&mut d, 300).0, 1 + 400, "the row typed in for its cells, every row for the notes");
    // A merged cell: no row is taken, whichever rows are looked at.
    if let Some(Block::Table(t)) = d.body.get_mut(1).map(Arc::make_mut)
        && let Some(row) = t.rows.get_mut(50)
    {
        wordcraft_doc::Shared::make_mut(row).cells[0].props.vmerge = wordcraft_doc::props::VMerge::Restart;
    }
    assert_eq!(key(&mut d, 120).1, 0, "with a merged cell rows aren't taken");
    assert_eq!(key(&mut d, 121).1, 0);
}

/// The rows placed by reference on every page of `l`, and how many of them have their items made.
fn rows_made(l: &DocLayout) -> (usize, usize) {
    let rows: Vec<_> = l.pages.iter().flat_map(|p| p.items.rows()).collect();
    (rows.len(), rows.iter().filter(|r| r.items_made()).count())
}

/// Rows' items are made when a page holding them is read, not when the table is laid out (a
/// key that widens an AutoFit column lays every row out again): after the first layout and
/// after such a key, only the rows the page breaks looked at have items; reading a page makes
/// its rows'. A row counts its items without making them, and the count is right.
#[test]
fn rows_make_their_items_when_their_page_is_read() {
    let opts = LayoutOptions::default();
    let mut d = Document::new();
    d.body.clear();
    d.body.push(para("Before the table."));
    let mut t = Table::new(400, 3, 468.0);
    for (r, row) in t.rows.iter_mut().enumerate() {
        for (c, cell) in row.cells.iter_mut().enumerate() {
            cell.blocks = vec![para(&format!("row {r} cell {c}"))];
        }
    }
    d.body.push(Arc::new(Block::Table(t)));
    d.body.push(para("After the table."));
    let mut cache = LayoutCache::new();
    let l = layout(&d, &mut cache, &opts);
    let pages = l.pages.len();
    assert!(pages > 5, "{pages} pages");
    let (placed, made) = rows_made(&l);
    // (The last page copies its rows out when the paragraph after the table is placed on it, so
    // up to a page of rows, which depends on the fonts, isn't placed by reference.)
    assert!(placed > 330 && placed <= 400, "{placed} rows placed by reference");
    assert!(made < pages, "{made} rows made their items before any page was read ({pages} pages)");
    // Reading a page makes its rows' items, and no other row's.
    let on_page = l.pages[3].items.rows().len();
    assert!(on_page > 10, "{on_page} rows on page 3");
    let n = l.pages[3].items.len();
    assert_eq!(l.pages[3].items.iter().count(), n, "the count a page gives without copying its rows out");
    let (_, made_after) = rows_made(&l);
    assert!(
        made_after <= made + on_page && made_after >= on_page,
        "{made_after} rows made after reading page 3 ({on_page} rows on it, {made} before)"
    );
    for row in l.pages.iter().flat_map(|p| p.items.rows()) {
        assert_eq!(row.item_count(), row.items().len(), "a row's count is its items'");
    }
    // A key that widens a column lays every row out again: still no items made but for the
    // rows at the page breaks.
    let p = cell_para(&mut d, 1, 200, 0).unwrap();
    p.insert_text(0, "Pneumonoultramicroscopicsilicovolcanoconiosis", &Default::default()).unwrap();
    p.touch();
    let (r0, m0) = (cache.rebroken, cache.misses);
    let l2 = layout_after(&d, &mut cache, &opts, Some(&l));
    assert!(cache.rebroken - r0 >= 1199 && cache.misses - m0 >= 1200, "the column widened: {} broken again", cache.rebroken - r0);
    let (placed, made) = rows_made(&l2);
    assert!(placed > 330 && placed <= 400, "{placed} rows placed by reference");
    assert!(made < l2.pages.len(), "{made} rows made their items after the widening key ({} pages)", l2.pages.len());
    assert_eq!(format!("{:?}", l2.pages), format!("{:?}", layout(&d, &mut LayoutCache::new(), &opts).pages));
}

/// A table of every kind of content, for comparing rows made when read with rows made at once.
fn kitchen_table(rng: &mut Lcg, doc: &mut Document, list: u32, rows: usize) -> Table {
    use wordcraft_doc::props::{Border, BorderStyle, Borders, HeightRule, Rgb, TextDirection, VAlign, VMerge};
    let border = Border { style: BorderStyle::Single, width: 0.75, color: Some(Rgb(0, 0, 0)), space: 0.0 };
    let mut t = Table::new(rows, 4, 468.0);
    t.props.bidi_visual = rng.below(4) == 0;
    for r in 0..rows {
        let row = wordcraft_doc::Shared::make_mut(&mut t.rows[r]);
        row.props.header = r == 0;
        match rng.below(8) {
            0 => {
                row.props.height = Some(20.0 + rng.below(40) as f32);
                row.props.height_rule = HeightRule::Exact;
            }
            1 => {
                row.props.height = Some(30.0 + rng.below(40) as f32);
                row.props.height_rule = HeightRule::AtLeast;
            }
            _ => {}
        }
        row.props.cant_split = rng.below(6) == 0;
        for c in 0..row.cells.len() {
            let cell = &mut row.cells[c];
            let n = 1 + rng.below(3);
            let mut blocks: Vec<Arc<Block>> = Vec::new();
            for _ in 0..n {
                let mut p = some_paragraph(rng, doc, list);
                // Paragraph shading and borders make items of their own.
                if rng.below(6) == 0 {
                    p.props.shading = Some(Rgb(230, 230, 200));
                }
                if rng.below(6) == 0 {
                    p.props.borders = Some(Borders { top: Some(border), bottom: Some(border), ..Default::default() });
                }
                if rng.below(8) == 0 {
                    p.props.keep_next = Some(true);
                }
                p.touch();
                blocks.push(para_block(p));
            }
            if rng.below(10) == 0 {
                let mut inner = Table::new(2, 2, 100.0);
                for row in &mut inner.rows {
                    for cell in &mut wordcraft_doc::Shared::make_mut(row).cells {
                        cell.blocks = vec![para(&words(rng, 3))];
                    }
                }
                blocks.push(Arc::new(Block::Table(inner)));
            }
            if rng.below(12) == 0 {
                blocks.push(para(""));
            }
            cell.blocks = blocks;
            if rng.below(5) == 0 {
                cell.props.shading = Some(Rgb(200, 220, 255));
            }
            if rng.below(5) == 0 {
                cell.props.borders = Some(Borders { left: Some(border), right: Some(border), ..Default::default() });
            }
            cell.props.valign = [None, Some(VAlign::Center), Some(VAlign::Bottom)][rng.below(3)];
            if rng.below(10) == 0 {
                cell.props.text_direction = [TextDirection::Down, TextDirection::Up][rng.below(2)];
            }
            if rng.below(12) == 0 {
                cell.props.hide_mark = true;
            }
            // Vertical merges: a restart continued by the rows below.
            if r > 0 && rng.below(8) == 0 {
                cell.props.vmerge = VMerge::Continue;
            } else if rng.below(10) == 0 {
                cell.props.vmerge = VMerge::Restart;
            }
        }
        // A cell spanning two columns now and then.
        if rng.below(8) == 0 && row.cells.len() == 4 {
            row.cells.remove(2);
            row.cells[1].props.span = 2;
        }
    }
    t.props.borders = Some(Borders {
        top: Some(border),
        bottom: Some(border),
        left: Some(border),
        right: Some(border),
        inside_v: Some(border),
        between: Some(border),
    });
    t
}

/// Rows whose items are made when their page is read are, item for item, the rows made as the
/// table was laid out: tables of every kind of content (AutoFit, lists, notes, pictures, tabs,
/// fields, paragraph shading and borders, nested tables, cell shading, borders and vertical
/// alignment, turned text, merged cells, exact and minimum heights, right to left), both ways.
#[test]
fn rows_made_when_read_equal_rows_made_at_once() {
    let opts = LayoutOptions::default();
    let mut docs: Vec<Document> = (1..=2).map(autofit_tables).collect();
    for seed in 1..=4 {
        let mut rng = Lcg(seed * 31);
        let mut d = Document::new();
        d.body.clear();
        d.settings.auto_hyphenation = seed % 2 == 0;
        let list = d.numbering.add_list(ListKind::Numbered);
        d.body.push(para(&words(&mut rng, 20)));
        for _ in 0..2 {
            let rows = 12 + rng.below(30);
            let t = kitchen_table(&mut rng, &mut d, list, rows);
            d.body.push(Arc::new(Block::Table(t)));
            d.body.push(para(&words(&mut rng, 30)));
        }
        docs.push(d);
    }
    let both = |d: &Document| {
        let lazy = layout(d, &mut LayoutCache::new(), &opts);
        table::EAGER_CELLS.with(|e| e.set(true));
        let eager = layout(d, &mut LayoutCache::new(), &opts);
        table::EAGER_CELLS.with(|e| e.set(false));
        (lazy, eager)
    };
    let mut lean_rows = 0;
    for (i, d) in docs.iter().enumerate() {
        let (lazy, eager) = both(d);
        assert_eq!(lazy.pages.len(), eager.pages.len(), "document {i}");
        for (pi, (a, b)) in lazy.pages.iter().zip(&eager.pages).enumerate() {
            assert_eq!(a.items.len(), b.items.len(), "document {i} page {pi}: items counted");
            for row in a.items.rows() {
                lean_rows += usize::from(!row.items_made());
                assert_eq!(row.item_count(), row.items().len(), "document {i} page {pi}: a row's count");
            }
            let (x, y) = (format!("{:?}", a.items), format!("{:?}", b.items));
            assert!(x == y, "document {i} page {pi}: {}", first_difference(&x, &y));
        }
    }
    assert!(lean_rows > 100, "{lean_rows} rows had their items made when read");
}
