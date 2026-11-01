//! Incremental pagination ([`resume`]): after every edit in a sequence, the layout resumed from
//! the last one is the same as laying the document out from scratch.

use super::*;
use std::sync::Arc;
use wordcraft_doc::para::{Anchor, Float, InlineObject, NoteKind, ShapeKind, Wrap};
use wordcraft_doc::props::{NumRef, ParaProps};
use wordcraft_doc::{Block, Paragraph, PartKind, Pos, Table, para_block};

const WORDS: &[&str] = &["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet"];

/// A small deterministic generator (no dependency, same sequence every run).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    /// `least` words and up to `more` more.
    fn some_words(&mut self, least: usize, more: usize) -> String {
        let n = least + self.below(more);
        self.words(n)
    }
    fn words(&mut self, n: usize) -> String {
        (0..n).map(|_| WORDS[self.below(WORDS.len())]).collect::<Vec<_>>().join(" ")
    }
}

fn para(text: &str, props: ParaProps) -> Arc<Block> {
    let mut p = Paragraph::with_text(text, Default::default());
    p.props = props;
    para_block(p)
}

/// A table whose first column continues the numbered list `list`.
fn table(rng: &mut Lcg, rows: usize, list: u32) -> Arc<Block> {
    let mut t = Table::new(rows, 3, 468.0);
    for row in &mut t.rows {
        for (ci, c) in row.cells.iter_mut().enumerate() {
            let props =
                if ci == 0 { ParaProps { numbering: Some(NumRef { num: list, level: 0 }), ..Default::default() } } else { ParaProps::default() };
            c.blocks = vec![para(&rng.some_words(3, 6), props)];
        }
    }
    t.rows[0].props.header = true;
    // Rows of an exact height (never split, text past it cut off, #1617) and of a minimum
    // height (moved on whole where less room is left, #1618).
    for (ri, row) in t.rows.iter_mut().enumerate().skip(1) {
        match ri % 5 {
            2 => (row.props.height, row.props.height_rule) = (Some(60.0), wordcraft_doc::props::HeightRule::AtLeast),
            4 => (row.props.height, row.props.height_rule) = (Some(18.0), wordcraft_doc::props::HeightRule::Exact),
            _ => {}
        }
    }
    Arc::new(Block::Table(t))
}

fn field(d: &mut Document, pos: &Pos, instr: &str) {
    d.insert_object(
        pos,
        InlineObject::Field {
            instr: instr.into(),
            result: String::new(),
            locked: false,
            dirty: false,
            code: false,
            form: String::new(),
            nested: Vec::new(),
        },
        &Default::default(),
    )
    .unwrap();
}

/// A document with a bit of everything that paginates: headings (kept with the next
/// paragraph), tables with repeated header rows, a numbered list, footnotes, a header with
/// page numbers and the page count, a page break before, and (with `sections`) a two-column
/// continuous section and a new-page section restarting the page numbers.
fn sample(seed: u64, sections: bool) -> Document {
    let mut rng = Lcg(seed);
    let mut d = Document::new();
    d.body.clear();
    let list = d.numbering.add_list(wordcraft_doc::ListKind::Numbered);
    for i in 0..70 {
        match i % 10 {
            0 => d.body.push(para(&format!("Heading {i}"), ParaProps { style: Some("Heading1".into()), ..Default::default() })),
            3 => d.body.push({
                let rows = 3 + rng.below(12);
                table(&mut rng, rows, list)
            }),
            5 | 6 => d.body.push(para(&rng.words(4), ParaProps { numbering: Some(NumRef { num: list, level: 0 }), ..Default::default() })),
            8 if i == 48 => d.body.push(para(&rng.words(30), ParaProps { page_break_before: Some(true), ..Default::default() })),
            _ => d.body.push(para(&rng.some_words(20, 80), ParaProps::default())),
        }
    }
    // Footnotes in a few paragraphs.
    for b in [7, 21, 44] {
        let nid = d.add_part(PartKind::Footnote, vec![para(&rng.words(12), ParaProps::default())]);
        d.insert_object(&Pos::body(b, 0), InlineObject::NoteRef { kind: NoteKind::Footnote, id: nid, custom: String::new() }, &Default::default())
            .unwrap();
    }
    // Floating shapes the text wraps around, one behind the text.
    for (b, wrap) in [(15, Wrap::Square), (34, Wrap::BehindText), (61, Wrap::TopAndBottom), (66, Wrap::Square)] {
        let float = Float { wrap, h_rel: Anchor::Column, v_rel: Anchor::Paragraph, x: 200.0, y: 10.0, dist: 9.0, ..Default::default() };
        let shape = InlineObject::Shape {
            kind: ShapeKind::Rectangle,
            w: 150.0,
            h: 120.0,
            fill: None,
            stroke: None,
            stroke_width: 0.75,
            float,
            story: None,
            freeform: None,
            effects: Default::default(),
            extra: Default::default(),
        };
        d.insert_object(&Pos::body(b, 0), shape, &Default::default()).unwrap();
    }
    // A header with "Page N of M".
    let hid = d.add_part(PartKind::Header, vec![para("Page  of ", ParaProps::default())]);
    field(&mut d, &Pos { story: StoryRef::Part(hid), path: Path::top(0), off: 5 }, "PAGE");
    let end = d.end_of(StoryRef::Part(hid));
    field(&mut d, &end, "NUMPAGES");
    d.last_section.headers.default = Some(hid);
    if sections {
        let two = wordcraft_doc::section::SectionProps {
            start: wordcraft_doc::section::SectionStart::Continuous,
            columns: wordcraft_doc::section::Columns { count: 2, ..Default::default() },
            ..Default::default()
        };
        if let Block::Para(p) = Arc::make_mut(&mut d.body[24]) {
            p.section = Some(Box::new(wordcraft_doc::section::SectionProps::default()));
        }
        if let Block::Para(p) = Arc::make_mut(&mut d.body[38]) {
            p.section = Some(Box::new(two));
        }
        let restart = wordcraft_doc::section::SectionProps { page_num_start: Some(5), ..Default::default() };
        if let Block::Para(p) = Arc::make_mut(&mut d.body[52]) {
            p.section = Some(Box::new(restart));
        }
        // Page numbers restarting at 0 (#1601).
        let zero = wordcraft_doc::section::SectionProps { page_num_start: Some(0), ..Default::default() };
        if let Block::Para(p) = Arc::make_mut(&mut d.body[64]) {
            p.section = Some(Box::new(zero));
        }
    }
    d
}

/// One edit, as the editor's commands make them: blocks that change are copied (new `Arc`s),
/// the others stay shared.
fn edit(d: &mut Document, rng: &mut Lcg, history: &mut Vec<Document>) -> &'static str {
    let n = d.body.len();
    let i = rng.below(n);
    // Mostly near the end (where typing usually is), sometimes anywhere.
    let i = if rng.below(3) == 0 { i } else { n - 1 - rng.below(n.min(12)) };
    let snapshot = d.clone();
    let what = match rng.below(10) {
        0 | 1 => {
            // Type a few characters.
            let w = rng.some_words(1, 2);
            match Arc::make_mut(&mut d.body[i]) {
                Block::Para(p) => {
                    let off = p.text.len();
                    p.insert_text(off, &format!(" {w}"), &Default::default()).unwrap();
                    p.touch();
                    "type"
                }
                Block::Table(t) => {
                    let r = rng.below(t.rows.len());
                    if let Some(Block::Para(p)) = t.rows[r].cells[1].blocks.get_mut(0).map(Arc::make_mut) {
                        p.insert_text(0, &format!("{w} "), &Default::default()).unwrap();
                        p.touch();
                    }
                    "type in a cell"
                }
            }
        }
        2 => {
            // Enter: a new paragraph after this block.
            d.body.insert(i + 1, para(&rng.some_words(1, 30), ParaProps::default()));
            "enter"
        }
        3 if n > 20 => {
            // Delete a block (not one ending a section).
            if d.body[i].as_para().is_some_and(|p| p.section.is_some()) {
                return "nothing";
            }
            d.body.remove(i);
            "delete"
        }
        4 => {
            // Paste a few paragraphs.
            for k in 0..3 {
                d.body.insert(i + k, para(&rng.some_words(10, 60), ParaProps::default()));
            }
            "paste"
        }
        5 if !history.is_empty() => {
            // Undo: the document as it was (sharing its blocks with the layouts before).
            if let Some(h) = history.pop() {
                *d = h;
            }
            return "undo";
        }
        7 => type_in_a_note(d, rng),
        8 => {
            // A footnote added in a paragraph (its notes after it numbered again).
            if d.body[i].as_para().is_none() {
                return "nothing";
            }
            let id = d.add_part(PartKind::Footnote, vec![para(&rng.some_words(3, 20), ParaProps::default())]);
            d.insert_object(&Pos::body(i, 0), InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() }, &Default::default())
                .unwrap();
            "add a footnote"
        }
        9 => remove_a_note(d, rng),
        6 => {
            // A page break: ending a paragraph (its mark stays on the old page, #1168), opening a
            // numbered one (its label goes along, #1616) or before a page break before (one
            // page, #1611); now and then an empty section mark after it (#1614).
            let numbered = ParaProps { numbering: d.numbering.nums.first().map(|n| NumRef { num: n.id, level: 0 }), ..Default::default() };
            let text = rng.some_words(1, 6);
            match rng.below(3) {
                0 => d.body.insert(i + 1, para(&format!("{text}\u{c}"), ParaProps::default())),
                1 => d.body.insert(i + 1, para(&format!("\u{c}{text}"), numbered)),
                _ => {
                    d.body.insert(i + 1, para("\u{c}", ParaProps::default()));
                    d.body.insert(i + 2, para(&text, ParaProps { page_break_before: Some(true), ..Default::default() }));
                }
            }
            "page break"
        }
        _ => {
            // Backspace at the end of a paragraph.
            if let Block::Para(p) = Arc::make_mut(&mut d.body[i])
                && p.text.len() > 3
            {
                let end = p.text.len();
                let cut = p.text.floor_char_boundary(end - 2);
                p.delete(cut, end).unwrap();
                p.touch();
            }
            "backspace"
        }
    };
    history.push(snapshot);
    what
}

/// Type in a footnote's text (a few words, now and then a paragraph more).
fn type_in_a_note(d: &mut Document, rng: &mut Lcg) -> &'static str {
    let ids: Vec<u32> = d.parts.iter().filter(|(_, p)| p.kind == PartKind::Footnote).map(|(id, _)| *id).collect();
    if ids.is_empty() {
        return "nothing";
    }
    let id = ids[rng.below(ids.len())];
    let more = rng.below(4) == 0;
    let words = rng.some_words(2, 12);
    let Some(part) = d.parts.get_mut(&id) else { return "nothing" };
    if more {
        part.blocks.push(para(&words, ParaProps::default()));
        return "a paragraph in a note";
    }
    if let Some(Block::Para(p)) = part.blocks.last_mut().map(Arc::make_mut) {
        let off = p.text.len();
        p.insert_text(off, &format!(" {words}"), &Default::default()).unwrap();
        p.touch();
    }
    "type in a note"
}

/// Delete a footnote reference (and its note) from a body paragraph.
fn remove_a_note(d: &mut Document, rng: &mut Lcg) -> &'static str {
    let with: Vec<usize> = (0..d.body.len())
        .filter(|i| d.body[*i].as_para().is_some_and(|p| p.objects.iter().any(|o| matches!(o, InlineObject::NoteRef { .. }))))
        .collect();
    if with.is_empty() {
        return "nothing";
    }
    let i = with[rng.below(with.len())];
    let Block::Para(p) = Arc::make_mut(&mut d.body[i]) else { return "nothing" };
    let Some(k) = p.objects.iter().position(|o| matches!(o, InlineObject::NoteRef { .. })) else { return "nothing" };
    let InlineObject::NoteRef { id, .. } = p.objects[k].clone() else { return "nothing" };
    let Some((off, _)) = p.text.char_indices().filter(|(_, c)| *c == wordcraft_doc::para::OBJ).nth(k) else { return "nothing" };
    p.delete(off, off + wordcraft_doc::para::OBJ.len_utf8()).unwrap();
    p.touch();
    d.parts.remove(&id);
    "remove a footnote"
}

fn dump(l: &DocLayout) -> String {
    format!("{:?}\n{}", l.pages, l.text_boxes)
}

/// Run `steps` edits on `d`, checking the resumed layout against a fresh one after each.
/// Returns how many passes resumed and how many took the old pages over.
fn check_edits(d: Document, seed: u64, steps: usize) -> (usize, usize) {
    check_edits_every(d, seed, steps, 1)
}

/// [`check_edits`], checking only every `every`th layout: the others are never read, so the
/// next pass resumes from pages whose items (table rows, pages taken over) weren't copied out.
fn check_edits_every(mut d: Document, seed: u64, steps: usize, every: usize) -> (usize, usize) {
    let opts = LayoutOptions::default();
    let mut cache = LayoutCache::new();
    let mut prev = layout(&d, &mut cache, &opts);
    let mut rng = Lcg(seed);
    let mut history = Vec::new();
    let (mut resumed, mut spliced) = (0, 0);
    let mut tables = 0;
    for step in 0..steps {
        let what = edit(&mut d, &mut rng, &mut history);
        let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
        resumed += usize::from(cache.resumed.is_some());
        spliced += usize::from(cache.spliced.is_some());
        tables += cache.tables_reused;
        if step % every != every - 1 {
            prev = inc;
            continue;
        }
        let full = layout(&d, &mut LayoutCache::new(), &opts);
        same_lengths(&inc);
        if dump(&inc) != dump(&full) {
            for (pi, (a, b)) in inc.pages.iter().zip(&full.pages).enumerate() {
                if format!("{a:?}") != format!("{b:?}") {
                    eprintln!("DBG page {pi}: first_block {} vs {}, items {} vs {}", a.first_block, b.first_block, a.items.len(), b.items.len());
                    for (ii, (x, y)) in a.items.iter().zip(b.items.iter()).enumerate() {
                        let (fx, fy) = (format!("{x:?}"), format!("{y:?}"));
                        if fx != fy {
                            eprintln!("DBG item {ii}:\n  inc  {}\n  full {}", &fx[..fx.len().min(300)], &fy[..fy.len().min(300)]);
                            break;
                        }
                    }
                    break;
                }
            }
        }
        assert!(
            dump(&inc) == dump(&full),
            "seed {seed}, step {step} ({what}): resumed from {:?}, spliced at {:?}: {} pages, {} from scratch; first different page {:?}",
            cache.resumed,
            cache.spliced,
            inc.pages.len(),
            full.pages.len(),
            inc.pages.iter().zip(&full.pages).position(|(a, b)| format!("{a:?}") != format!("{b:?}")),
        );
        // The pieces of every body paragraph are found without the index too.
        for (key, v) in full.index() {
            if key.0 == StoryRef::Body {
                assert_eq!(&inc.places(key.0, &key.1), v, "{key:?}");
            }
        }
        prev = inc;
    }
    eprintln!("TABLES REUSED {tables}");
    (resumed, spliced)
}

#[test]
fn resumed_layouts_match_layouts_from_scratch() {
    let (mut resumed, mut spliced) = (0, 0);
    for seed in 1..=4 {
        let (r, s) = check_edits(sample(seed, seed % 2 == 0), seed * 7919, 40);
        resumed += r;
        spliced += s;
    }
    eprintln!("RESUMED {resumed} SPLICED {spliced}");
    // The incremental paths were taken (most edits are near the end; Enter mid-document comes
    // back in step at a page break before or a new section).
    assert!(resumed > 100, "resumed {resumed}");
    assert!(spliced > 5, "spliced {spliced}");
}

/// The pages' item counts, without copying table rows out, are what they hold.
fn same_lengths(l: &DocLayout) {
    for (pi, p) in l.pages.iter().enumerate() {
        let n = p.items.len();
        assert_eq!(n, p.items.iter().count(), "page {pi}");
    }
}

/// The same with layouts read only now and then: a pass resumes from pages of a layout nobody
/// read, whose rows and taken-over pages were never copied out.
#[test]
fn layouts_read_now_and_then_match_layouts_from_scratch() {
    let (mut resumed, mut spliced) = (0, 0);
    for seed in 21..=24 {
        let (r, s) = check_edits_every(sample(seed, seed % 2 == 1), seed * 104_729, 60, 4);
        resumed += r;
        spliced += s;
    }
    assert!(resumed > 150 && spliced > 10, "resumed {resumed}, spliced {spliced}");
}

#[test]
fn an_edit_resumes_on_its_page_and_takes_the_rest_over() {
    let mut d = sample(11, true);
    let opts = LayoutOptions::default();
    let mut cache = LayoutCache::new();
    let prev = layout(&d, &mut cache, &opts);
    // Type in the paragraph just before the page break before (block 48).
    if let Block::Para(p) = Arc::make_mut(&mut d.body[46]) {
        p.insert_text(0, "x", &Default::default()).unwrap();
        p.touch();
    }
    let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
    let from = cache.resumed.unwrap();
    assert!(from <= 46 && from > 30, "resumed from {from}");
    // Back in step by the page break before at the latest (block 47 can start a page too).
    assert!(cache.spliced.is_some_and(|s| (47..=48).contains(&s)), "the page break before brings it back in step: {:?}", cache.spliced);
    assert_eq!(dump(&inc), dump(&layout(&d, &mut LayoutCache::new(), &opts)));
}

#[test]
fn changes_outside_the_body_lay_everything_out() {
    let mut d = sample(5, false);
    let opts = LayoutOptions::default();
    let mut cache = LayoutCache::new();
    let prev = layout(&d, &mut cache, &opts);
    // The header changes: every page's top may move.
    let hid = d.last_section.headers.default.unwrap();
    if let Some(part) = d.parts.get_mut(&hid) {
        part.blocks.push(para("A second header line", ParaProps::default()));
    }
    let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
    assert_eq!(cache.resumed, None);
    assert_eq!(dump(&inc), dump(&layout(&d, &mut LayoutCache::new(), &opts)));
    // A layout from another cache (or none) isn't resumed from.
    let other = layout(&d, &mut LayoutCache::new(), &opts);
    let _ = layout_after(&d, &mut cache, &opts, Some(&other));
    assert_eq!(cache.resumed, None);
}

#[test]
fn page_totals_in_the_body_lay_everything_out() {
    let mut d = sample(6, false);
    field(&mut d, &Pos::body(2, 0), "NUMPAGES");
    let (resumed, _) = check_edits(d, 99, 8);
    assert_eq!(resumed, 0);
}

/// A heading kept with the next paragraph moves to the next page when the two don't fit: when
/// that paragraph changes so they do, the heading moves back. The pass resumes before the
/// heading (it looked at the paragraph while placed), not after it.
#[test]
fn a_heading_kept_with_a_changed_paragraph_is_placed_again() {
    let opts = LayoutOptions::default();
    let mut moved = 0;
    for lines in 20..40 {
        let mut d = Document::new();
        d.body.clear();
        for _ in 0..lines {
            d.body.push(para("A line.", ParaProps::default()));
        }
        d.body.push(para("Heading", ParaProps { style: Some("Heading1".into()), ..Default::default() }));
        let tall = ParaProps { space_before: Some(60.0), ..Default::default() };
        d.body.push(para("The paragraph the heading is kept with.", tall));
        for _ in 0..6 {
            d.body.push(para(&WORDS.repeat(9).join(" "), ParaProps::default()));
        }
        let mut cache = LayoutCache::new();
        let prev = layout(&d, &mut cache, &opts);
        let heading_page = |l: &DocLayout| l.places(StoryRef::Body, &Path::top(lines)).first().map(|p| p.0);
        if let Block::Para(p) = Arc::make_mut(&mut d.body[lines + 1]) {
            p.props.space_before = Some(0.0);
            p.touch();
        }
        let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
        let full = layout(&d, &mut LayoutCache::new(), &opts);
        assert!(dump(&inc) == dump(&full), "{lines} lines: resumed from {:?}", cache.resumed);
        moved += usize::from(heading_page(&prev) != heading_page(&full));
    }
    assert!(moved > 0, "some heading moved back");
}

/// A header showing the page number is measured as its section starts, with the number then:
/// resuming in a later section measures it with that number too (a two-digit number can wrap
/// the header onto another line and move the body down).
#[test]
fn resuming_in_a_section_measures_its_header_with_the_page_number_then() {
    let opts = LayoutOptions::default();
    let mut differ = 0;
    for fill in (100..240).step_by(2) {
        let mut d = Document::new();
        d.body.clear();
        for i in 0..90 {
            d.body.push(para(&WORDS.repeat(10).join(" "), ParaProps::default()));
            if i == 70 {
                // Section 1 ends here, a dozen pages in.
                if let Some(Block::Para(p)) = d.body.last_mut().map(Arc::make_mut) {
                    p.section = Some(Box::default());
                }
            }
        }
        let hid = d.add_part(PartKind::Header, vec![para(&"i".repeat(fill), ParaProps::default())]);
        let end = d.end_of(StoryRef::Part(hid));
        field(&mut d, &end, "PAGE");
        d.last_section.headers.default = Some(hid);
        let mut cache = LayoutCache::new();
        let prev = layout(&d, &mut cache, &opts);
        if let Block::Para(p) = Arc::make_mut(&mut d.body[85]) {
            p.insert_text(0, "x", &Default::default()).unwrap();
            p.touch();
        }
        let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
        assert!(cache.resumed.is_some_and(|b| b > 71), "resumed in the second section: {:?}", cache.resumed);
        let full = layout(&d, &mut LayoutCache::new(), &opts);
        assert!(dump(&inc) == dump(&full), "fill {fill}");
        // The header is taller on some pages than others (the number wraps).
        let tops: Vec<u32> = full.pages.iter().map(|p| p.body.y.to_bits()).collect();
        differ += usize::from(tops.windows(2).any(|w| w[0] != w[1]));
    }
    assert!(differ > 0, "some header wrapped");
}

/// Long tables (a repeated header row, some rows numbered, one table with automatic column
/// widths) between paragraphs.
fn long_tables(seed: u64) -> Document {
    let mut rng = Lcg(seed);
    let mut d = Document::new();
    d.body.clear();
    let list = d.numbering.add_list(wordcraft_doc::ListKind::Numbered);
    for k in 0..3 {
        d.body.push(para(&rng.some_words(20, 40), ParaProps::default()));
        let rows = 80 + rng.below(80);
        let mut t = Table::new(rows, 3, 468.0);
        for (r, row) in t.rows.iter_mut().enumerate() {
            for (c, cell) in row.cells.iter_mut().enumerate() {
                // The second table numbers its first column.
                let props = if k == 1 && c == 0 {
                    ParaProps { numbering: Some(NumRef { num: list, level: 0 }), ..Default::default() }
                } else {
                    ParaProps::default()
                };
                cell.blocks = vec![para(&rng.some_words(1 + (r + c) % 3, 4), props)];
            }
        }
        t.rows[0].props.header = true;
        // The third lays its columns out to fit their contents.
        t.props.fixed = k != 2;
        d.body.push(Arc::new(Block::Table(t)));
    }
    d.body.push(para(&rng.some_words(20, 40), ParaProps::default()));
    // Footnotes referenced from a few cells of the first table.
    for r in [3, 30, 60] {
        let id = d.add_part(PartKind::Footnote, vec![para(&rng.some_words(5, 10), ParaProps::default())]);
        let pos = Pos { story: StoryRef::Body, path: Path(vec![1, r, 1, 0]), off: 0 };
        d.insert_object(&pos, InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() }, &Default::default()).unwrap();
    }
    d
}

/// Type `text` at the start of the first paragraph of cell `c` of row `r`.
fn type_in_cell(t: &mut Table, r: usize, c: usize, text: &str) {
    if let Some(Block::Para(p)) = t.rows[r].cells[c].blocks.first_mut().map(Arc::make_mut) {
        p.insert_text(0, text, &Default::default()).unwrap();
        p.touch();
    }
}

/// One edit in or around the tables: typing in a cell, Enter (a second paragraph) or joining
/// in a cell, a word too long for its column, numbering a cell, two cells far apart changed at
/// once (as a paste over several rows does), or a paragraph added or removed before a table.
fn edit_cell(d: &mut Document, rng: &mut Lcg, list: u32) -> &'static str {
    let tables: Vec<usize> = d.body.iter().enumerate().filter(|(_, b)| matches!(&***b, Block::Table(_))).map(|(i, _)| i).collect();
    let ti = tables[rng.below(tables.len())];
    let op = rng.below(10);
    if op == 9 {
        // Before the table: a new paragraph, or the one added before taken away.
        if d.body.get(ti.wrapping_sub(1)).and_then(|b| b.as_para()).is_some_and(|p| p.text.starts_with("Added")) {
            d.body.remove(ti - 1);
            return "remove a paragraph before a table";
        }
        d.body.insert(ti, para(&format!("Added {}", rng.some_words(5, 30)), ParaProps::default()));
        return "add a paragraph before a table";
    }
    let Block::Table(t) = Arc::make_mut(&mut d.body[ti]) else { return "nothing" };
    let r = rng.below(t.rows.len());
    let c = rng.below(3);
    match op {
        0 => {
            t.rows[r].cells[c].blocks.push(para(&rng.some_words(1, 6), ParaProps::default()));
            "enter in a cell"
        }
        1 if t.rows[r].cells[c].blocks.len() > 1 => {
            t.rows[r].cells[c].blocks.pop();
            "join in a cell"
        }
        2 => {
            type_in_cell(t, r, c, "Pneumonoultramicroscopicsilicovolcanoconiosis ");
            "long word in a cell"
        }
        5 if rng.below(3) == 0 => {
            // The cell's text replaced (a footnote reference in it goes).
            let nr = [3, 30, 60][rng.below(3)];
            t.rows[nr].cells[1].blocks[0] = para(&rng.some_words(1, 4), ParaProps::default());
            "replace a cell's text"
        }
        3 if rng.below(4) == 0 => {
            if let Some(Block::Para(p)) = t.rows[r].cells[c].blocks.first_mut().map(Arc::make_mut) {
                p.props.numbering = Some(NumRef { num: list, level: 0 });
                p.touch();
            }
            "number a cell"
        }
        4 => {
            let far = (r + t.rows.len() / 2) % t.rows.len();
            type_in_cell(t, r, c, "first ");
            let more = rng.some_words(10, 20);
            type_in_cell(t, far, (c + 1) % 3, &format!("{more} "));
            "two rows far apart"
        }
        _ => {
            let w = rng.words(1);
            type_in_cell(t, r, c, &format!("{w} "));
            "type in a cell"
        }
    }
}

/// Typing in long tables lays out again only the rows that changed and the page they are on,
/// and comes back in step after them, with the same result as laying the document out from
/// scratch.
#[test]
fn edits_in_long_tables_match_layouts_from_scratch() {
    let opts = LayoutOptions::default();
    let (mut rows, mut inside, mut spliced) = (0, 0, 0);
    for seed in 1..=3 {
        let mut d = long_tables(seed);
        let list = d.numbering.find_kind(wordcraft_doc::ListKind::Numbered).unwrap();
        let mut cache = LayoutCache::new();
        let mut prev = layout(&d, &mut cache, &opts);
        let mut rng = Lcg(seed * 31);
        let mut history: Vec<Document> = Vec::new();
        for step in 0..60 {
            let what = if step % 9 == 8 && !history.is_empty() {
                d = history.pop().unwrap();
                "undo"
            } else {
                history.push(d.clone());
                edit_cell(&mut d, &mut rng, list)
            };
            let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
            rows += cache.rows_reused;
            inside += usize::from(cache.resumed_row.is_some());
            spliced += usize::from(cache.spliced.is_some());
            let full = layout(&d, &mut LayoutCache::new(), &opts);
            assert!(
                dump(&inc) == dump(&full),
                "seed {seed}, step {step} ({what}): resumed from {:?} row {:?}, spliced at {:?}",
                cache.resumed,
                cache.resumed_row,
                cache.spliced
            );
            prev = inc;
        }
    }
    eprintln!("LONG TABLES rows reused {rows}, resumed inside {inside}, spliced {spliced}");
    assert!(rows > 1000 && inside > 10 && spliced > 20, "rows reused {rows}, resumed inside {inside}, spliced {spliced}");
}

/// Notes are numbered again when a changed cell of a long table loses its note reference (the
/// last pass's numbers hold only when no changed cell has one); the pass starts at the table (the
/// first block whose notes' numbers changed).
#[test]
fn a_note_reference_leaving_a_cell_renumbers_the_notes() {
    let opts = LayoutOptions::default();
    let mut d = long_tables(7);
    let mut cache = LayoutCache::new();
    let mut prev = layout(&d, &mut cache, &opts);
    for row in [3, 30] {
        if let Block::Table(t) = Arc::make_mut(&mut d.body[1]) {
            t.rows[row].cells[1].blocks[0] = para("no note here", ParaProps::default());
        }
        let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
        assert!(cache.resumed.is_some_and(|b| b <= 1), "resumed {:?}", cache.resumed);
        assert_eq!(dump(&inc), dump(&layout(&d, &mut LayoutCache::new(), &opts)));
        prev = inc;
    }
}

/// A paragraph added before a long table (Enter at the start of the document) moves every row
/// down a line: the rows are paginated again without copying their items, and the table's
/// layout is taken as it is (its rows numbered for its new block when placed). Passes resume
/// from layouts nobody read, edit after edit; read later, every layout is the one laid out from
/// scratch.
#[test]
fn a_paragraph_before_a_long_table_moves_its_rows_without_copying_them() {
    let opts = LayoutOptions::default();
    let mut d = long_tables(5);
    let mut cache = LayoutCache::new();
    let mut prev = layout(&d, &mut cache, &opts);
    let mut kept: Vec<(Document, DocLayout)> = Vec::new();
    let mut rng = Lcg(77);
    for step in 0..12 {
        match step % 4 {
            // Enter at the start, twice; then the first paragraph goes; then typing at the start.
            0 | 1 => d.body.insert(0, para(&rng.some_words(1, 12), ParaProps::default())),
            2 => {
                d.body.remove(0);
            }
            _ => {
                if let Block::Para(p) = Arc::make_mut(&mut d.body[0]) {
                    p.insert_text(0, &format!("{} ", rng.some_words(3, 10)), &Default::default()).unwrap();
                    p.touch();
                }
            }
        }
        let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
        assert!(cache.resumed.is_some(), "step {step}: resumed");
        // (The tables after where it comes back in step aren't met at all.)
        let table = d.body.iter().position(|b| matches!(**b, Block::Table(_))).unwrap();
        let met = cache.spliced.is_none_or(|s| s > table);
        assert!(
            cache.rows_reused == 0 && (cache.tables_reused >= 1 || !met),
            "step {step}: the tables were taken as they were ({}, spliced at {:?})",
            cache.tables_reused,
            cache.spliced
        );
        kept.push((d.clone(), inc.clone()));
        prev = inc;
    }
    for (step, (d, inc)) in kept.iter().enumerate() {
        same_lengths(inc);
        assert!(dump(inc) == dump(&layout(d, &mut LayoutCache::new(), &opts)), "step {step}");
    }
}

/// The first footnote `b` references.
fn note_of(b: &Block) -> Option<u32> {
    match b {
        Block::Para(p) => p.objects.iter().find_map(|o| if let InlineObject::NoteRef { id, .. } = o { Some(*id) } else { None }),
        Block::Table(t) => t.rows.iter().flat_map(|r| &r.cells).flat_map(|c| &c.blocks).find_map(|b| note_of(b)),
    }
}

/// Typing in a footnote lays out again from the page its reference is on and comes back in
/// step after it, in the body and in a long table's cells, the same as from scratch.
#[test]
fn typing_in_a_footnote_resumes_at_its_reference() {
    let opts = LayoutOptions::default();
    let (mut resumed, mut spliced, mut edits) = (0, 0, 0);
    for in_table in [false, true] {
        let mut d = if in_table { long_tables(9) } else { sample(31, false) };
        let mut cache = LayoutCache::new();
        let mut prev = layout(&d, &mut cache, &opts);
        let mut rng = Lcg(5);
        // The notes of the paragraphs, or of the long table's rows 3, 30 and 60.
        let ids: Vec<u32> = match &*d.body[1] {
            Block::Table(t) if in_table => [3, 30, 60].iter().filter_map(|r| note_of(&t.rows[*r].cells[1].blocks[0])).collect(),
            _ => [7, 21, 44].iter().filter_map(|b| note_of(&d.body[*b])).collect(),
        };
        assert_eq!(ids.len(), 3);
        for (k, id) in ids.iter().enumerate() {
            for n in 0..6 {
                let words = rng.some_words(3, 25);
                let part = d.parts.get_mut(id).unwrap();
                if n == 3 {
                    part.blocks.push(para(&words, ParaProps::default()));
                } else if let Some(Block::Para(p)) = part.blocks.last_mut().map(Arc::make_mut) {
                    let off = p.text.len();
                    p.insert_text(off, &format!(" {words}"), &Default::default()).unwrap();
                    p.touch();
                }
                let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
                resumed += usize::from(cache.resumed.is_some());
                spliced += usize::from(cache.spliced.is_some());
                edits += 1;
                let full = layout(&d, &mut LayoutCache::new(), &opts);
                assert!(dump(&inc) == dump(&full), "note {k}, edit {n}: resumed {:?}, spliced {:?}", cache.resumed, cache.spliced);
                prev = inc;
            }
        }
    }
    assert!(resumed == edits && spliced > edits / 2, "resumed {resumed}, spliced {spliced} of {edits}");
}

/// A footnote added or taken away numbers the notes after it again: the pass starts before the
/// first block whose notes' numbers changed and comes back in step only after the last.
#[test]
fn adding_a_footnote_renumbers_the_notes_after_it() {
    let opts = LayoutOptions::default();
    let mut d = sample(41, true);
    let mut cache = LayoutCache::new();
    let mut prev = layout(&d, &mut cache, &opts);
    let mut ids = Vec::new();
    for b in [62, 50, 4, 30, 12] {
        let id = d.add_part(PartKind::Footnote, vec![para("A note added.", ParaProps::default())]);
        d.insert_object(&Pos::body(b, 0), InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() }, &Default::default())
            .unwrap();
        ids.push((b, id));
        let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
        assert!(cache.resumed.is_some_and(|r| r <= b), "added at {b}: resumed {:?}", cache.resumed);
        assert!(dump(&inc) == dump(&layout(&d, &mut LayoutCache::new(), &opts)), "added at {b}");
        prev = inc;
    }
    for (b, id) in ids {
        if let Block::Para(p) = Arc::make_mut(&mut d.body[b]) {
            let k = p.objects.iter().position(|o| matches!(o, InlineObject::NoteRef { id: x, .. } if *x == id)).unwrap();
            let off = p.text.char_indices().filter(|(_, c)| *c == wordcraft_doc::para::OBJ).nth(k).unwrap().0;
            p.delete(off, off + wordcraft_doc::para::OBJ.len_utf8()).unwrap();
            p.touch();
        }
        d.parts.remove(&id);
        let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
        assert!(cache.resumed.is_some(), "removed at {b}");
        assert!(dump(&inc) == dump(&layout(&d, &mut LayoutCache::new(), &opts)), "removed at {b}");
        prev = inc;
    }
}

/// Typing in a table that numbers its cells takes the rows after the same list counters from
/// the last layout (a cell numbered or not any more lays the rows after it out again).
#[test]
fn typing_in_a_numbered_table_takes_the_other_rows_as_they_were() {
    let opts = LayoutOptions::default();
    let mut d = long_tables(3);
    let list = d.numbering.find_kind(wordcraft_doc::ListKind::Numbered).unwrap();
    let mut cache = LayoutCache::new();
    let mut prev = layout(&d, &mut cache, &opts);
    let mut rng = Lcg(19);
    for step in 0..16 {
        let Block::Table(t) = Arc::make_mut(&mut d.body[3]) else { panic!("the numbered table") };
        let r = 1 + rng.below(t.rows.len() - 1);
        match step % 4 {
            3 => {
                if let Some(Block::Para(p)) = t.rows[r].cells[2].blocks.first_mut().map(Arc::make_mut) {
                    p.props.numbering = if p.props.numbering.is_some() { None } else { Some(NumRef { num: list, level: 0 }) };
                    p.touch();
                }
            }
            // Typing in a numbered cell, and in another.
            0 | 1 => type_in_cell(t, r, 0, &format!("{} ", rng.words(1))),
            _ => type_in_cell(t, r, 1, &format!("{} ", rng.some_words(1, 9))),
        }
        let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
        let reused = cache.rows_reused;
        if step % 4 != 3 {
            assert!(reused > 50, "step {step}: rows reused {reused}");
        }
        assert!(dump(&inc) == dump(&layout(&d, &mut LayoutCache::new(), &opts)), "step {step}: rows reused {reused}");
        prev = inc;
    }
}

/// Table rows placed on a page are copied out when its items are read, and again with the rows
/// placed after a read.
#[test]
fn rows_placed_after_a_read_are_read_too() {
    let fill = |y: f32| Placed::Fill { rect: Rect::new(0.0, y, 5.0, 5.0), color: Default::default() };
    let row = |y: f32| Arc::new(table::RowLayout::built(10.0, vec![fill(y)], 0, 0.0, Vec::new()));
    let mut items = Items::from(vec![fill(1.0)]);
    items.push_row(&row(0.0), 10.0, 20.0, 0);
    assert_eq!((items.len(), items.iter().count()), (2, 2));
    items.push_row(&row(3.0), 10.0, 40.0, 0);
    assert_eq!((items.len(), items.iter().count()), (3, 3));
    let shared = items.clone();
    items.push_row(&row(6.0), 10.0, 60.0, 0);
    assert_eq!((items.len(), items.iter().count()), (4, 4));
    assert_eq!(shared.iter().count(), 3, "a copy read before keeps its items");
    let ys: Vec<f32> = items.iter().filter_map(|it| if let Placed::Fill { rect, .. } = it { Some(rect.y) } else { None }).collect();
    assert_eq!(ys, vec![1.0, 20.0, 43.0, 66.0]);
    items.push(fill(0.0));
    assert_eq!((items.len(), items.iter().count()), (5, 5));
}

/// A footnote added before a long table whose cells reference notes numbers those again: the
/// table, unchanged, isn't taken from the last layout with its old numbers.
#[test]
fn a_note_added_before_a_table_renumbers_its_cells() {
    let opts = LayoutOptions::default();
    let mut d = long_tables(13);
    let mut cache = LayoutCache::new();
    let mut prev = layout(&d, &mut cache, &opts);
    for n in 0..3 {
        let id = d.add_part(PartKind::Footnote, vec![para(&format!("Note {n} before the table."), ParaProps::default())]);
        d.insert_object(&Pos::body(0, 0), InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() }, &Default::default())
            .unwrap();
        let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
        assert_eq!(cache.resumed, Some(0), "note {n}");
        assert!(dump(&inc) == dump(&layout(&d, &mut LayoutCache::new(), &opts)), "note {n}");
        prev = inc;
    }
}

/// A style changed between two layouts with the same cache lays the paragraphs out again with
/// it (the styles' hash is kept only while they are the same).
#[test]
fn a_changed_style_lays_the_paragraphs_out_again() {
    let opts = LayoutOptions::default();
    let mut d = sample(17, false);
    let mut cache = LayoutCache::new();
    let mut prev = layout(&d, &mut cache, &opts);
    for (k, size) in [(0, 15.0), (1, 9.0), (2, 15.0)] {
        let id = if k == 1 { "Heading1" } else { "Normal" };
        d.styles.get_mut(id).unwrap().chr.size = Some(size);
        let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
        assert!(dump(&inc) == dump(&layout(&d, &mut LayoutCache::new(), &opts)), "change {k}");
        prev = inc;
    }
}

/// A footnote-heavy document: three notes in nearly every body paragraph (one in six long enough
/// to continue on the next page), a heading every tenth block and a table with a note in a cell.
fn noted(seed: u64) -> Document {
    let mut rng = Lcg(seed);
    let mut d = Document::new();
    d.body.clear();
    let list = d.numbering.add_list(wordcraft_doc::ListKind::Numbered);
    for i in 0..60 {
        match i % 10 {
            0 => d.body.push(para(&format!("Heading {i}"), ParaProps { style: Some("Heading1".into()), ..Default::default() })),
            5 => d.body.push(table(&mut rng, 4, list)),
            _ => d.body.push(para(&rng.some_words(20, 60), ParaProps::default())),
        }
    }
    let note = |d: &mut Document, rng: &mut Lcg, at: Pos| {
        let words = if rng.below(6) == 0 { rng.some_words(60, 60) } else { rng.some_words(6, 24) };
        let nid = d.add_part(PartKind::Footnote, vec![para(&words, ParaProps::default())]);
        d.insert_object(&at, InlineObject::NoteRef { kind: NoteKind::Footnote, id: nid, custom: String::new() }, &Default::default()).unwrap();
    };
    for b in 0..d.body.len() {
        let Some(len) = d.body[b].as_para().map(|p| p.text.len()).filter(|_| b % 10 != 0) else { continue };
        let mid = d.body[b].as_para().and_then(|p| p.text.get(len / 2..)).and_then(|t| t.find(' ')).map_or(0, |k| len / 2 + k);
        note(&mut d, &mut rng, Pos::body(b, len));
        note(&mut d, &mut rng, Pos::body(b, mid));
        note(&mut d, &mut rng, Pos::body(b, 0));
    }
    note(&mut d, &mut rng, Pos { story: StoryRef::Body, path: Path(vec![15, 2, 1, 0]), off: 0 });
    d
}

/// Typing in a body paragraph of a footnote-heavy document comes back in step a page or two
/// later, though nearly every page starts with notes waiting (those of a paragraph continued
/// from the page before, or continued notes): the notes waiting are compared, not required to
/// be none.
#[test]
fn typing_with_notes_waiting_takes_the_rest_over() {
    let opts = LayoutOptions::default();
    let mut d = noted(3);
    let mut cache = LayoutCache::new();
    let mut prev = layout(&d, &mut cache, &opts);
    assert!(prev.pages.len() > 12, "{} pages", prev.pages.len());
    let (mut spliced, mut edits) = (0, 0);
    let mut rng = Lcg(77);
    for b in [12, 22, 33, 41] {
        for _ in 0..4 {
            let words = format!(" {}", rng.some_words(1, 6));
            if let Block::Para(p) = Arc::make_mut(&mut d.body[b]) {
                p.insert_text(wordcraft_doc::para::OBJ.len_utf8(), &words, &Default::default()).unwrap();
                p.touch();
            }
            let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
            // Back in step within a few blocks of where it resumed (a page or two).
            let back = cache.resumed.zip(cache.spliced).map(|(r, s)| s.saturating_sub(r));
            eprintln!("NOTED block {b}: resumed {:?}, back in step {back:?} blocks later", cache.resumed);
            spliced += usize::from(back.is_some_and(|n| n <= 10));
            edits += 1;
            assert!(
                dump(&inc) == dump(&layout(&d, &mut LayoutCache::new(), &opts)),
                "block {b}: resumed {:?}, spliced {:?}",
                cache.resumed,
                cache.spliced
            );
            prev = inc;
        }
    }
    assert!(spliced * 4 >= edits * 3, "back in step soon {spliced} times of {edits}");
}

/// Random edits (typing, notes added, typed in and removed, tables, page breaks…) in the
/// footnote-heavy document: every resumed layout is the layout from scratch.
#[test]
fn edits_with_notes_waiting_match_layouts_from_scratch() {
    let (mut resumed, mut spliced) = (0, 0);
    for seed in 1..=3 {
        let (r, s) = check_edits(noted(seed), seed * 6007, 40);
        resumed += r;
        spliced += s;
    }
    eprintln!("NOTED RESUMED {resumed} SPLICED {spliced}");
    assert!(resumed > 60, "resumed {resumed}");
}

/// On a page with footnotes a paragraph's last line keeps its space after clear of the separator,
/// unless contextual spacing drops it before the next paragraph (of the same style): the next
/// paragraph's style decides where the last line goes. When that paragraph, starting the next
/// page, changes style, the pass must not resume after the paragraph that looked at it.
#[test]
fn a_last_line_kept_clear_of_the_notes_looks_at_the_next_paragraph() {
    let opts = LayoutOptions::default();
    let mut moved = 0;
    for filler in 0..50 {
        let mut d = Document::new();
        d.body.clear();
        for _ in 0..filler {
            d.body.push(para("A line.", ParaProps::default()));
        }
        // A paragraph running from the first page onto the second, its note (referenced at its
        // end) on the second: the pass records a checkpoint before the paragraph after it there.
        let kept = ParaProps { contextual_spacing: Some(true), space_after: Some(36.0), ..Default::default() };
        d.body.push(para(&WORDS.repeat(60).join(" "), kept));
        let at = d.end_of(StoryRef::Body);
        let id = d.add_part(PartKind::Footnote, vec![para("The note.", ParaProps::default())]);
        d.insert_object(&at, InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() }, &Default::default()).unwrap();
        let b = d.body.len();
        d.body.push(para(&WORDS.repeat(12).join(" "), ParaProps::default()));
        for _ in 0..4 {
            d.body.push(para(&WORDS.repeat(9).join(" "), ParaProps::default()));
        }
        let mut cache = LayoutCache::new();
        let prev = layout(&d, &mut cache, &opts);
        if let Block::Para(p) = Arc::make_mut(&mut d.body[b]) {
            p.props.style = Some("Quote".into());
            p.touch();
        }
        let inc = layout_after(&d, &mut cache, &opts, Some(&prev));
        let full = layout(&d, &mut LayoutCache::new(), &opts);
        let pieces = |l: &DocLayout| l.places(StoryRef::Body, &Path::top(b - 1));
        moved += usize::from(pieces(&prev) != pieces(&full));
        assert!(dump(&inc) == dump(&full), "{filler} lines: resumed from {:?}", cache.resumed);
    }
    assert!(moved > 0, "some last line moved");
}

/// A character's fallback face is chosen among the fonts loaded so far, and fonts load as text
/// first asks for them: the layout of a paragraph that asked for a fallback face may not hold
/// once more fonts have loaded. After fonts loaded, a pass lays such paragraphs out again and
/// doesn't take pages holding them from the last pass; a pass during which fonts loaded is made
/// again with them; text that never fell back is taken from the cache and the last pass as
/// before.
#[test]
fn fonts_loaded_since_or_during_a_pass_lay_fallback_text_out_again() {
    let set = |keys: &[(usize, u64)]| FONTS_KEYS.with(|k| *k.borrow_mut() = keys.to_vec());
    let opts = LayoutOptions::default();
    let type_at_end = |d: &mut Document| {
        let last = d.body.len() - 1;
        if let Block::Para(p) = Arc::make_mut(&mut d.body[last]) {
            p.insert_text(p.text.len(), " typed", &Default::default()).unwrap();
            p.touch();
        }
    };
    // Six paragraphs near the start with a character no font has (it asks for a fallback).
    let mut d = sample(8, false);
    let fallback = [1, 2, 4, 11, 12, 14];
    for b in fallback {
        if let Block::Para(p) = Arc::make_mut(&mut d.body[b]) {
            p.insert_text(0, "\u{10FFFD} ", &Default::default()).unwrap();
            p.touch();
        }
    }
    let mut cache = LayoutCache::new();
    set(&[(1000, 0)]);
    let prev = layout(&d, &mut cache, &opts);
    // The same fonts: a keystroke at the end resumes.
    type_at_end(&mut d);
    let prev = layout_after(&d, &mut cache, &opts, Some(&prev));
    assert!(cache.resumed.is_some(), "resumed with the same fonts");
    // More fonts loaded since: the pages with those paragraphs aren't taken over, and the
    // paragraphs are laid out again (the others come from the cache).
    set(&[(1001, 0)]);
    type_at_end(&mut d);
    let misses = cache.misses;
    let prev = layout_after(&d, &mut cache, &opts, Some(&prev));
    assert_eq!(cache.resumed, None, "fonts loaded since the last pass");
    let laid = cache.misses - misses;
    assert!(laid > fallback.len() as u64 && laid < 30, "the fallback paragraphs and the typed one laid out again: {laid}");
    // Fonts loading while the pass lays out (the pass started with the old ones): it is made
    // again with the new ones.
    set(&[(1001, 0), (1001, 0), (1002, 0)]);
    type_at_end(&mut d);
    let misses = cache.misses;
    let _ = layout_after(&d, &mut cache, &opts, Some(&prev));
    assert_eq!(cache.resumed, None, "made again after fonts loaded during the pass");
    assert!(cache.misses - misses > fallback.len() as u64, "the fallback paragraphs laid out again: {}", cache.misses - misses);
    // Text that never fell back doesn't care which fonts loaded.
    let mut d = sample(8, false);
    let mut cache = LayoutCache::new();
    set(&[(1000, 0)]);
    let prev = layout(&d, &mut cache, &opts);
    set(&[(1001, 0)]);
    type_at_end(&mut d);
    let _ = layout_after(&d, &mut cache, &opts, Some(&prev));
    assert!(cache.resumed.is_some(), "resumed: nothing fell back");
    set(&[]);
}

/// Paragraph `i` (a body paragraph) to change, copied out of its `Arc`; `None` for a table.
fn para_at(d: &mut Document, i: usize) -> Option<&mut Paragraph> {
    match d.body.get_mut(i).map(Arc::make_mut) {
        Some(Block::Para(p)) => Some(p),
        _ => None,
    }
}

/// The body paragraphs that end a section.
fn section_ends(d: &Document) -> Vec<usize> {
    (0..d.body.len()).filter(|k| d.body[*k].as_para().is_some_and(|p| p.section.is_some())).collect()
}

/// Edits the generator above doesn't make: section breaks added and taken away, a section's
/// page setup, columns and first-page header changed (before or after where the pass resumes),
/// odd and even headers, hidden text, tracked insertions and deletions, page fields, keep with
/// next and paragraph styles, with undo across all of them.
fn edit_more(d: &mut Document, rng: &mut Lcg, history: &mut Vec<Document>) -> &'static str {
    use wordcraft_doc::section::{Columns, SectionProps, SectionStart};
    let n = d.body.len();
    let i = if rng.below(2) == 0 { rng.below(n) } else { n - 1 - rng.below(n.min(15)) };
    let snapshot = d.clone();
    let what = match rng.below(13) {
        0 => {
            // A section break: next page, continuous, odd or even page; some with two columns,
            // a deeper top margin or a landscape page.
            if i + 1 >= n {
                return "nothing";
            }
            let start = [SectionStart::NextPage, SectionStart::Continuous, SectionStart::OddPage, SectionStart::EvenPage][rng.below(4)];
            let mut s = SectionProps { start, ..Default::default() };
            match rng.below(4) {
                0 => s.columns = Columns { count: 2, ..Default::default() },
                1 => s.margin_top = 144.0,
                2 => (s.page_w, s.page_h, s.landscape) = (792.0, 612.0, true),
                _ => {}
            }
            let Some(p) = para_at(d, i) else { return "nothing" };
            if p.section.is_some() {
                return "nothing";
            }
            p.section = Some(Box::new(s));
            p.touch();
            "add a section break"
        }
        1 => {
            let ends = section_ends(d);
            if ends.is_empty() {
                return "nothing";
            }
            let k = ends[rng.below(ends.len())];
            let Some(p) = para_at(d, k) else { return "nothing" };
            p.section = None;
            p.touch();
            "remove a section break"
        }
        2 | 3 => {
            // A section's page setup, the last section's (outside the body blocks) now and then.
            let ends = section_ends(d);
            let first_header = d.parts.iter().find(|(_, p)| p.kind == PartKind::Header).map(|(id, _)| *id);
            let change = rng.below(6);
            let set = |s: &mut wordcraft_doc::section::SectionProps| match change {
                0 => s.margin_left = if s.margin_left == 72.0 { 144.0 } else { 72.0 },
                1 => s.columns.count = 1 + (s.columns.count % 3),
                2 => {
                    s.title_page = !s.title_page;
                    s.headers.first = if s.title_page { first_header } else { None };
                }
                3 => s.page_num_start = if s.page_num_start.is_some() { None } else { Some(3) },
                4 => s.start = if s.start == SectionStart::Continuous { SectionStart::NextPage } else { SectionStart::Continuous },
                _ => s.page_h = if s.page_h == 792.0 { 600.0 } else { 792.0 },
            };
            if ends.is_empty() || rng.below(4) == 0 {
                set(&mut d.last_section);
                "change the last section's setup"
            } else {
                let k = ends[rng.below(ends.len())];
                let Some(p) = para_at(d, k) else { return "nothing" };
                if let Some(s) = p.section.as_mut() {
                    set(s);
                }
                p.touch();
                "change a section's setup"
            }
        }
        4 => {
            // Hidden text (now and then a whole paragraph, mark and all).
            let whole = rng.below(3) == 0;
            let Some(p) = para_at(d, i) else { return "nothing" };
            let len = p.text.len();
            if len < 4 {
                return "nothing";
            }
            let a = if whole { 0 } else { p.text.floor_char_boundary(len / 3) };
            let b = if whole { len } else { p.text.floor_char_boundary(2 * len / 3) };
            p.format(a, b, &|c| c.hidden = Some(true)).unwrap();
            if whole {
                p.mark.hidden = Some(true);
            }
            p.touch();
            "hide text"
        }
        5 | 6 => {
            // A tracked insertion typed in, or a tracked deletion.
            let kind = if rng.below(2) == 0 { wordcraft_doc::RevisionKind::Insert } else { wordcraft_doc::RevisionKind::Delete };
            let rid = d.revisions.len() as u32;
            d.revisions.push(wordcraft_doc::Revision { kind, author: "A".into(), ..Default::default() });
            let words = rng.some_words(3, 30);
            let Some(p) = para_at(d, i) else { return "nothing" };
            let len = p.text.len();
            if kind == wordcraft_doc::RevisionKind::Insert {
                let props = wordcraft_doc::props::CharProps { ins: Some(rid), ..Default::default() };
                p.insert_text(len, &format!(" {words}"), &props).unwrap();
            } else if len > 4 {
                let a = p.text.floor_char_boundary(len / 2);
                p.format(a, len, &|c| c.del = Some(rid)).unwrap();
            }
            p.touch();
            "tracked change"
        }
        7 => {
            if d.body[i].as_para().is_none() {
                return "nothing";
            }
            field(d, &Pos::body(i, 0), "PAGE");
            "page field"
        }
        8 => {
            // Different odd and even pages (the document's setting), with an even page header.
            d.settings.even_odd_headers = !d.settings.even_odd_headers;
            let even =
                if d.settings.even_odd_headers { Some(d.add_part(PartKind::Header, vec![para("Even page", ParaProps::default())])) } else { None };
            d.last_section.headers.even = even;
            "odd and even headers"
        }
        9 if !history.is_empty() => {
            if let Some(h) = history.pop() {
                *d = h;
            }
            return "undo";
        }
        10 => {
            let Some(p) = para_at(d, i) else { return "nothing" };
            p.props.keep_next = Some(!p.props.keep_next.unwrap_or(false));
            p.touch();
            "keep with next"
        }
        11 => {
            let style = ["Heading2", "Normal", "ListParagraph", "Quote"][rng.below(4)];
            let Some(p) = para_at(d, i) else { return "nothing" };
            p.props.style = Some(style.into());
            p.touch();
            "paragraph style"
        }
        _ => {
            let w = rng.some_words(1, 20);
            let Some(p) = para_at(d, i) else { return "nothing" };
            let off = p.text.len();
            p.insert_text(off, &format!(" {w}"), &Default::default()).unwrap();
            p.touch();
            "type"
        }
    };
    history.push(snapshot);
    what
}

/// After every edit of [`edit_more`]'s kinds, with the view switched now and then (markup shown
/// or hidden, hidden text shown, Draft and back), the layout resumed from the last one is the
/// one laid out from scratch.
#[test]
fn sections_page_setup_markup_and_hidden_text_relayout_like_from_scratch() {
    let views = [
        LayoutOptions::default(),
        LayoutOptions { hide_deleted: true, ..Default::default() },
        LayoutOptions { show_hidden: true, ..Default::default() },
        LayoutOptions { view: ViewMode::Draft, web_width: 600.0, ..Default::default() },
    ];
    let (mut resumed, mut edits) = (0, 0);
    for seed in 1..=6 {
        let mut d = sample(seed * 3, seed % 2 == 0);
        let mut cache = LayoutCache::new();
        let mut v = 0;
        let mut prev = layout(&d, &mut cache, &views[v]);
        let mut rng = Lcg(seed * 7_727);
        let mut history = Vec::new();
        for step in 0..45 {
            let what = edit_more(&mut d, &mut rng, &mut history);
            if step % 15 == 14 {
                v = rng.below(views.len());
            }
            let opts = &views[v];
            let inc = layout_after(&d, &mut cache, opts, Some(&prev));
            resumed += usize::from(cache.resumed.is_some());
            edits += 1;
            let full = layout(&d, &mut LayoutCache::new(), opts);
            assert!(
                dump(&inc) == dump(&full),
                "seed {seed}, step {step} ({what}, view {v}): resumed from {:?}, spliced at {:?}: {} pages, {} from scratch; first different page {:?}",
                cache.resumed,
                cache.spliced,
                inc.pages.len(),
                full.pages.len(),
                inc.pages.iter().zip(&full.pages).position(|(a, b)| format!("{a:?}") != format!("{b:?}")),
            );
            prev = inc;
        }
    }
    eprintln!("MORE EDITS resumed {resumed} of {edits}");
    assert!(resumed > edits / 4, "resumed {resumed} of {edits}");
}

/// Typing keystroke after keystroke — in a paragraph, a table cell and a footnote — leaves the
/// paragraph cache about the size it was: the layouts of the earlier versions of what was typed
/// in (one per keystroke) go. They used to stay until the cache was four
/// times what a whole pass uses (on a long document, hundreds of megabytes of typing).
#[test]
fn typing_doesnt_grow_the_paragraph_cache() {
    let opts = LayoutOptions::default();
    let mut d = sample(12, false);
    let mut cache = LayoutCache::new();
    let mut prev = layout(&d, &mut cache, &opts);
    let start = cache.len();
    let last = d.body.len() - 1;
    let note = d.parts.iter().find(|(_, p)| p.kind == PartKind::Footnote).map(|(id, _)| *id).unwrap();
    let mut most = 0;
    for k in 0..1500 {
        let p = match k % 5 {
            // A table cell.
            3 => match Arc::make_mut(&mut d.body[3]) {
                Block::Table(t) => t.rows[1].cells[1].blocks.first_mut().map(Arc::make_mut),
                Block::Para(_) => None,
            },
            // A footnote.
            4 => d.parts.get_mut(&note).and_then(|p| p.blocks.first_mut()).map(Arc::make_mut),
            _ => Some(Arc::make_mut(&mut d.body[last])),
        };
        if let Some(Block::Para(p)) = p {
            let at = p.text.len();
            p.insert_text(at, if k % 7 == 0 { " " } else { "x" }, &Default::default()).unwrap();
            p.touch();
        }
        prev = layout_after(&d, &mut cache, &opts, Some(&prev));
        most = most.max(cache.len());
    }
    assert!(most < start + 50, "the cache grew from {start} to {most} entries");
    assert!(dump(&prev) == dump(&layout(&d, &mut LayoutCache::new(), &opts)));
}

/// Undoing a change to every paragraph (Replace All, Select All and Bold) finds the paragraphs'
/// layouts in the cache, and so does redoing it: a large change's replaced versions are kept for
/// the undo (only a small change's go at once), so neither lays a paragraph out again. They used
/// to go at once, and the undo laid the whole document out again (25 ms became 570 ms on 600
/// pages of prose).
#[test]
fn undoing_and_redoing_a_change_to_every_paragraph_lays_none_out_again() {
    fn bold(b: &mut Arc<Block>) {
        match Arc::make_mut(b) {
            Block::Para(p) => {
                let n = p.len();
                p.format(0, n, &|c| c.bold = Some(true)).unwrap();
                p.touch();
            }
            Block::Table(t) => {
                for c in t.rows.iter_mut().flat_map(|r| r.cells.iter_mut()) {
                    c.blocks.iter_mut().for_each(bold);
                }
            }
        }
    }
    let opts = LayoutOptions::default();
    let mut before = sample(14, false);
    // (A header showing the page count is laid out again for the other count, which is fine.)
    before.last_section.headers.default = None;
    let mut cache = LayoutCache::new();
    let prev = layout(&before, &mut cache, &opts);
    let mut after = before.clone();
    after.body.iter_mut().for_each(bold);
    let changed = layout_after(&after, &mut cache, &opts, Some(&prev));
    let misses = cache.misses;
    let undone = layout_after(&before, &mut cache, &opts, Some(&changed));
    assert_eq!(cache.misses - misses, 0, "the undo laid paragraphs out again");
    assert!(dump(&undone) == dump(&layout(&before, &mut LayoutCache::new(), &opts)));
    let misses = cache.misses;
    let redone = layout_after(&after, &mut cache, &opts, Some(&undone));
    assert_eq!(cache.misses - misses, 0, "the redo laid paragraphs out again");
    assert!(dump(&redone) == dump(&layout(&after, &mut LayoutCache::new(), &opts)));
}

/// Pasting long tables, undoing the paste and redoing it don't walk the pasted tables' cells
/// for the revisions the change replaced (`Resumable::replaced_revisions`): their rows are
/// shared with the tables copied, still in the document, so none of their layouts goes. Walking
/// them made undoing or redoing a paste of 1,000 blocks of tables 8 ms slower (of 40).
#[test]
fn pasting_tables_and_undoing_it_walk_none_of_their_cells() {
    let opts = LayoutOptions::default();
    let before = long_tables(9);
    let mut cache = LayoutCache::new();
    let prev = layout(&before, &mut cache, &opts);
    // A paste before the last paragraph of copies of the second and third tables (the first
    // holds note references), sharing their rows with those copied.
    let mut after = before.clone();
    let copies: Vec<Arc<Block>> = [3, 5].iter().map(|&i| Arc::new((*before.body[i]).clone())).collect();
    let cells: usize = copies.iter().map(|b| if let Block::Table(t) = &**b { t.rows.len() * 3 } else { 0 }).sum();
    assert!(cells > 400, "{cells} cells pasted");
    let last = after.body.len() - 1;
    after.body.splice(last..last, copies);
    let walked = |cache: &LayoutCache, doc: &Document| {
        let (gone, kept) = cache.resume.as_ref().unwrap().replaced_revisions(doc);
        gone.len() + kept.len()
    };
    assert_eq!(walked(&cache, &after), 0, "paragraphs the paste walked");
    let pasted = layout_after(&after, &mut cache, &opts, Some(&prev));
    assert_eq!(walked(&cache, &before), 0, "paragraphs the undo walked");
    let misses = cache.misses;
    let undone = layout_after(&before, &mut cache, &opts, Some(&pasted));
    assert_eq!(cache.misses - misses, 0, "the undo laid paragraphs out again");
    assert!(dump(&undone) == dump(&layout(&before, &mut LayoutCache::new(), &opts)));
    assert_eq!(walked(&cache, &after), 0, "paragraphs the redo walked");
    let misses = cache.misses;
    let redone = layout_after(&after, &mut cache, &opts, Some(&undone));
    assert_eq!(cache.misses - misses, 0, "the redo laid paragraphs out again");
    assert!(dump(&redone) == dump(&layout(&after, &mut LayoutCache::new(), &opts)));
}
