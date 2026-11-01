//! The synthetic corpus the snapshots pin: about three dozen small documents built in code (or
//! driven through the command registry with a fixed clock and author), each covering one area
//! of the layout, the writers and the command layer, and two scripted command sequences.
//!
//! Every document uses the bundled fonts only (`pin_fonts`), so its layout is the same on any
//! machine: system fonts differ between machines and the goldens must not.

use std::sync::Arc;

use serde_json::{Value, json};
use wordcraft_doc::para::{Anchor, Float, FloatAlign, InlineObject, NoteKind, ShapeKind, Wrap, WrapText};
use wordcraft_doc::props::{
    Align, Border, BorderStyle, Borders, CharProps, HeightRule, Highlight, LineSpacing, NumRef, ParaProps, Rgb, TabAlign, TabLeader, TabStop,
    TableFloat, TextColor, TextDirection, TextVert, Underline, VAlign, VMerge, VertAlign,
};
use wordcraft_doc::section::{Columns, DocGrid, DocGridType, HeaderSet, LineNumberRestart, LineNumbering, NumFormat, PageVAlign, SectionStart};
use wordcraft_doc::{Block, Blocks, Document, ListKind, Paragraph, PartKind, Pos, SectionProps, StoryRef, Style, StyleKind, Table, para_block};
use wordcraft_engine::clock::Clock;
use wordcraft_engine::io::ExportOptions;
use wordcraft_engine::{Selection, Session, sample};
use wordcraft_layout::LayoutOptions;
use wordcraft_tz::Zone;

/// The instant every session's clock is stopped at: 2023-11-14T22:13:20Z.
pub const FIXED_NOW: i64 = 1_700_000_000;

/// The families of the bundled fonts: the only ones a snapshot layout may use.
pub const BUNDLED: &[&str] = &["Source Serif 4", "Source Sans 3", "Inter", "JetBrains Mono"];

/// One document of the corpus.
pub struct Doc {
    pub name: &'static str,
    pub doc: Document,
    /// Layouts to snapshot: a label and the options.
    pub variants: Vec<(&'static str, LayoutOptions)>,
    /// Pages (0-based) rendered as images, in light and dark mode.
    pub render: &'static [usize],
    /// How the save and PDF snapshots export it.
    pub export: ExportOptions,
}

/// A scripted command sequence, snapshotted after every step.
pub struct Script {
    pub name: &'static str,
    pub start: Document,
    pub steps: Vec<(&'static str, Value)>,
}

// ---------------------------------------------------------------------------------------------
// Building blocks

const WORDS: &[&str] = &[
    "the", "quick", "brown", "fox", "jumps", "over", "lazy", "dog", "while", "band", "plays", "river", "stone", "garden", "window", "morning",
    "evening", "letter", "number", "people", "history", "between", "through", "because", "without", "another", "village", "harbour", "journey",
    "question", "silver", "orchard", "lantern", "meadow", "pattern", "chapter", "measure", "market", "winter", "summer",
];

/// `n` deterministic words from `seed`, as a sentence.
pub fn words(seed: usize, n: usize) -> String {
    let mut x = (seed as u64).wrapping_mul(2_654_435_761).wrapping_add(1);
    let mut out = String::with_capacity(n * 7);
    for i in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        if i > 0 {
            out.push(' ');
        }
        let w = WORDS[(x % WORDS.len() as u64) as usize];
        if i == 0 {
            let mut c = w.chars();
            if let Some(f) = c.next() {
                out.push(f.to_ascii_uppercase());
                out.push_str(c.as_str());
            }
        } else {
            out.push_str(w);
        }
    }
    out.push('.');
    out
}

fn para(text: &str) -> Paragraph {
    Paragraph::with_text(text, CharProps::default())
}

fn styled(text: &str, style: &str) -> Paragraph {
    para(text).styled(style)
}

/// Append formatted text to a paragraph.
fn add(p: &mut Paragraph, text: &str, props: CharProps) {
    let at = p.len();
    p.insert_text(at, text, &props).unwrap();
}

fn chr() -> CharProps {
    CharProps::default()
}

fn bold() -> CharProps {
    CharProps { bold: Some(true), ..chr() }
}

fn italic() -> CharProps {
    CharProps { italic: Some(true), ..chr() }
}

fn font(name: &str) -> CharProps {
    CharProps { font: Some(name.into()), ..chr() }
}

fn color(r: u8, g: u8, b: u8) -> Option<TextColor> {
    Some(TextColor::Rgb(Rgb(r, g, b)))
}

fn border(style: BorderStyle, width: f32, c: Option<Rgb>) -> Border {
    Border { style, width, color: c, space: 0.0 }
}

/// A document with these body blocks.
fn doc_of(blocks: Vec<Block>) -> Document {
    let mut d = Document::new();
    d.body = blocks.into_iter().map(Arc::new).collect();
    d
}

fn text_doc(paras: &[&str]) -> Document {
    doc_of(paras.iter().map(|t| Block::Para(para(t))).collect())
}

/// A table of `rows`×`cols` filled with short deterministic text.
fn table(rows: usize, cols: usize, width: f32, seed: usize) -> Table {
    let mut t = Table::new(rows, cols, width);
    for r in 0..rows {
        for c in 0..cols {
            cell_text(&mut t, r, c, &words(seed + r * cols + c, 3));
        }
    }
    t
}

fn cell_text(t: &mut Table, r: usize, c: usize, text: &str) {
    if let Some(cell) = t.rows.get_mut(r).and_then(|row| row.cells.get_mut(c)) {
        cell.blocks = vec![para_block(para(text))];
    }
}

fn cell_blocks(t: &mut Table, r: usize, c: usize, blocks: Blocks) {
    if let Some(cell) = t.rows.get_mut(r).and_then(|row| row.cells.get_mut(c)) {
        cell.blocks = blocks;
    }
}

/// A small distinct PNG (`w`×`h`), its pixels a function of `i`.
pub fn png(i: usize, w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        let k = (x / 8 + y / 8) % 2 == 0;
        let base = (i as u32 * 37) as u8;
        if k {
            image::Rgb([base.wrapping_add(40), 90, 200u8.wrapping_sub(base)])
        } else {
            image::Rgb([240, 200u8.wrapping_sub(base), base.wrapping_add(60)])
        }
    });
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

fn image(media: String, w: f32, h: f32, alt: &str, float: Float) -> InlineObject {
    InlineObject::Image { media, w, h, alt: alt.into(), float, crop: [0.0; 4], ole: None }
}

fn shape(kind: ShapeKind, w: f32, h: f32, fill: Option<Rgb>, stroke: Option<Rgb>, float: Float) -> InlineObject {
    InlineObject::Shape {
        kind,
        w,
        h,
        fill,
        stroke,
        stroke_width: 1.5,
        float,
        story: None,
        effects: Default::default(),
        freeform: None,
        extra: Default::default(),
    }
}

fn field(instr: &str, result: &str) -> InlineObject {
    InlineObject::Field {
        instr: instr.into(),
        result: result.into(),
        locked: false,
        dirty: false,
        code: false,
        form: String::new(),
        nested: Vec::new(),
    }
}

/// A header or footer part with one paragraph, styled `style`.
fn hf_part(d: &mut Document, kind: PartKind, p: Paragraph) -> u32 {
    d.add_part(kind, vec![para_block(p)])
}

/// A session on `doc` with a stopped clock and a fixed author.
pub fn session(doc: Document) -> Session {
    let mut s = Session::new(doc);
    s.clock = Clock::fixed(FIXED_NOW, Zone::UTC);
    s.author = "Snapshot Author".into();
    s.initials = "SA".into();
    let end = s.doc.end_of(StoryRef::Body);
    s.sel = Selection::caret(end);
    s
}

pub fn run(s: &mut Session, id: &str, v: Value) {
    if let Err(e) = s.run(id, &v) {
        panic!("corpus: {id} {v}: {e}");
    }
}

fn select(s: &mut Session, text: &str) {
    run(s, "select.text", json!({"text": text}));
}

fn caret(s: &mut Session, block: usize, off: usize) {
    s.sel = Selection::caret(Pos::body(block, off));
}

fn opts() -> LayoutOptions {
    LayoutOptions { proofing: false, ..Default::default() }
}

fn default_variants() -> Vec<(&'static str, LayoutOptions)> {
    vec![("default", opts())]
}

/// The bundled family a font name is pinned to.
fn bundled_name(f: &str) -> String {
    if BUNDLED.iter().any(|b| b.eq_ignore_ascii_case(f)) {
        return f.to_string();
    }
    let l = f.to_ascii_lowercase();
    if l.contains("mono") || l.contains("code") || l.contains("consolas") || l.contains("courier") {
        "JetBrains Mono".into()
    } else if l.contains("display")
        || l.contains("sans")
        || l.contains("arial")
        || l.contains("segoe")
        || l.contains("symbol")
        || l.contains("helvetica")
    {
        "Source Sans 3".into()
    } else {
        "Source Serif 4".into()
    }
}

fn pin_chr(c: &mut CharProps) {
    if let Some(f) = &c.font {
        c.font = Some(bundled_name(f));
    }
    if let Some(f) = &c.font_ea {
        c.font_ea = Some(bundled_name(f));
    }
    if let Some(f) = &c.font_cs {
        c.font_cs = Some(bundled_name(f));
    }
}

fn pin_blocks(blocks: &mut Blocks, depth: usize) {
    for b in blocks.iter_mut() {
        match Arc::make_mut(b) {
            Block::Para(p) => {
                pin_chr(&mut p.mark);
                for r in p.runs.iter_mut() {
                    pin_chr(&mut r.props);
                }
            }
            Block::Table(t) if depth < 16 => {
                for row in t.rows.iter_mut() {
                    for cell in row.cells.iter_mut() {
                        pin_blocks(&mut cell.blocks, depth + 1);
                    }
                }
            }
            Block::Table(_) => {}
        }
    }
}

/// Every font the document names becomes a bundled one, so the layout never reaches for a
/// system font (which differs between machines).
pub fn pin_fonts(doc: &mut Document) {
    let default = doc.styles.default_chr.font.clone().unwrap_or_else(|| "Aptos".into());
    doc.styles.default_chr.font = Some(bundled_name(&default));
    pin_chr(&mut doc.styles.default_chr);
    for st in doc.styles.styles.iter_mut() {
        pin_chr(&mut st.chr);
    }
    doc.settings.major_font = "Source Sans 3".into();
    doc.settings.minor_font = "Source Serif 4".into();
    for a in doc.numbering.abstracts.iter_mut() {
        for l in a.levels.iter_mut() {
            pin_chr(&mut l.chr);
        }
    }
    for n in doc.numbering.nums.iter_mut() {
        for (_, l) in n.level_overrides.iter_mut() {
            pin_chr(&mut l.chr);
        }
    }
    pin_blocks(&mut doc.body, 0);
    let ids: Vec<u32> = doc.parts.keys().copied().collect();
    for id in ids {
        if let Some(p) = doc.parts.get_mut(&id) {
            pin_blocks(&mut p.blocks, 0);
        }
    }
}

/// Fixed document properties, so saves and PDFs don't carry the build's clock.
fn stamp(doc: &mut Document, title: &str) {
    doc.core.title = title.into();
    doc.core.creator = "Snapshot Author".into();
    doc.core.created = "2023-11-14T22:13:20Z".into();
    doc.core.modified = "2023-11-14T22:13:20Z".into();
}

fn finish(name: &'static str, mut doc: Document) -> Doc {
    pin_fonts(&mut doc);
    stamp(&mut doc, &format!("Snapshot: {name}"));
    Doc { name, doc, variants: default_variants(), render: &[], export: ExportOptions::default() }
}

// ---------------------------------------------------------------------------------------------
// Documents

fn blank() -> Doc {
    finish("blank", Document::new())
}

fn styles_text() -> Doc {
    let mut b = vec![
        Block::Para(styled("Styles and Inline Formatting", "Title")),
        Block::Para(styled("Every built-in paragraph style and most run properties", "Subtitle")),
        Block::Para(styled("Heading One", "Heading1")),
    ];
    let mut p = para("Plain text, then ");
    add(&mut p, "bold", bold());
    add(&mut p, ", ", chr());
    add(&mut p, "italic", italic());
    add(&mut p, ", ", chr());
    add(&mut p, "underlined", CharProps { underline: Some(Underline::Single), ..chr() });
    add(&mut p, ", ", chr());
    add(&mut p, "double underlined", CharProps { underline: Some(Underline::Double), ..chr() });
    add(&mut p, ", ", chr());
    add(&mut p, "struck", CharProps { strike: Some(true), ..chr() });
    add(&mut p, ", ", chr());
    add(&mut p, "double struck", CharProps { double_strike: Some(true), ..chr() });
    add(&mut p, ", x", chr());
    add(&mut p, "2", CharProps { vert_align: Some(VertAlign::Superscript), ..chr() });
    add(&mut p, " and H", chr());
    add(&mut p, "2", CharProps { vert_align: Some(VertAlign::Subscript), ..chr() });
    add(&mut p, "O, ", chr());
    add(&mut p, "coloured", CharProps { color: color(0xC0, 0x00, 0x00), ..chr() });
    add(&mut p, ", ", chr());
    add(&mut p, "highlighted", CharProps { highlight: Some(Highlight::Yellow), ..chr() });
    add(&mut p, ", ", chr());
    add(&mut p, "shaded", CharProps { shading: Some(Rgb(0xDD, 0xEE, 0xFF)), ..chr() });
    add(&mut p, ", ", chr());
    add(&mut p, "Small Caps", CharProps { small_caps: Some(true), ..chr() });
    add(&mut p, ", ", chr());
    add(&mut p, "all caps", CharProps { caps: Some(true), ..chr() });
    add(&mut p, ", ", chr());
    add(&mut p, "spaced", CharProps { spacing: Some(2.0), ..chr() });
    add(&mut p, ", ", chr());
    add(&mut p, "wide", CharProps { scale: Some(150.0), ..chr() });
    add(&mut p, ", ", chr());
    add(&mut p, "raised", CharProps { position: Some(3.0), ..chr() });
    add(&mut p, ", ", chr());
    add(&mut p, "monospace", font("JetBrains Mono"));
    add(&mut p, ", ", chr());
    add(&mut p, "sans", font("Source Sans 3"));
    add(&mut p, " and ", chr());
    add(&mut p, "large", CharProps { size: Some(18.0), ..chr() });
    add(&mut p, " and ", chr());
    add(&mut p, "tiny", CharProps { size: Some(7.0), ..chr() });
    add(&mut p, " text, with a ", chr());
    add(&mut p, "link", CharProps { style: Some("Hyperlink".into()), link: Some("https://example.org/snapshot".into()), ..chr() });
    add(&mut p, ".", chr());
    b.push(Block::Para(p));
    b.push(Block::Para(styled("Heading Two", "Heading2")));
    let mut q = para("Character styles: ");
    add(&mut q, "subtle emphasis", CharProps { style: Some("SubtleEmphasis".into()), ..chr() });
    add(&mut q, ", ", chr());
    add(&mut q, "emphasis", CharProps { style: Some("Emphasis".into()), ..chr() });
    add(&mut q, ", ", chr());
    add(&mut q, "intense emphasis", CharProps { style: Some("IntenseEmphasis".into()), ..chr() });
    add(&mut q, ", ", chr());
    add(&mut q, "strong", CharProps { style: Some("Strong".into()), ..chr() });
    add(&mut q, ", ", chr());
    add(&mut q, "subtle reference", CharProps { style: Some("SubtleReference".into()), ..chr() });
    add(&mut q, ", ", chr());
    add(&mut q, "intense reference", CharProps { style: Some("IntenseReference".into()), ..chr() });
    add(&mut q, " and ", chr());
    add(&mut q, "a book title", CharProps { style: Some("BookTitle".into()), ..chr() });
    add(&mut q, ".", chr());
    b.push(Block::Para(q));
    b.push(Block::Para(styled("Heading Three", "Heading3")));
    b.push(Block::Para(styled("A quotation set in the Quote style, centred and italic.", "Quote")));
    b.push(Block::Para(styled("An intense quotation with rules above and below.", "IntenseQuote")));
    b.push(Block::Para(styled("No Spacing line one.", "NoSpacing")));
    b.push(Block::Para(styled("No Spacing line two.", "NoSpacing")));
    b.push(Block::Para(styled("Heading Four", "Heading4")));
    let mut j = para(&words(1, 60));
    j.props.align = Some(Align::Justify);
    b.push(Block::Para(j));
    let mut r = para("Right aligned line.");
    r.props.align = Some(Align::Right);
    b.push(Block::Para(r));
    let mut c = para("Centred line.");
    c.props.align = Some(Align::Center);
    b.push(Block::Para(c));
    let mut dist = para("Distributed alignment spreads the letters of a short line.");
    dist.props.align = Some(Align::Distribute);
    b.push(Block::Para(dist));
    let mut ind = para(&words(2, 50));
    ind.props = ParaProps { indent_left: Some(36.0), indent_right: Some(36.0), indent_first: Some(18.0), ..Default::default() };
    b.push(Block::Para(ind));
    let mut hang = para(&words(3, 50));
    hang.props = ParaProps { indent_left: Some(36.0), indent_first: Some(-36.0), ..Default::default() };
    b.push(Block::Para(hang));
    let mut exact = para(&words(4, 40));
    exact.props =
        ParaProps { line_spacing: Some(LineSpacing::Exactly(20.0)), space_before: Some(12.0), space_after: Some(12.0), ..Default::default() };
    b.push(Block::Para(exact));
    let mut atleast = para(&words(5, 40));
    atleast.props = ParaProps { line_spacing: Some(LineSpacing::AtLeast(18.0)), ..Default::default() };
    b.push(Block::Para(atleast));
    let mut dbl = para(&words(6, 40));
    dbl.props = ParaProps { line_spacing: Some(LineSpacing::Multiple(2.0)), ..Default::default() };
    b.push(Block::Para(dbl));
    for i in 0..2 {
        let mut ctx = para(&format!("Contextual spacing paragraph {}.", i + 1));
        ctx.props = ParaProps { contextual_spacing: Some(true), space_after: Some(24.0), ..Default::default() };
        b.push(Block::Para(ctx));
    }
    b.push(Block::Para(styled("Heading Five", "Heading5")));
    b.push(Block::Para(styled("Heading Six", "Heading6")));
    b.push(Block::Para(para("The end.")));
    finish("styles_text", doc_of(b))
}

fn lists() -> Doc {
    let mut d = Document::new();
    let bullets = d.numbering.add_list(ListKind::Bullet);
    let numbers = d.numbering.add_list(ListKind::Numbered);
    let letters = d.numbering.add_list(ListKind::LowerLetter);
    let outline = d.numbering.add_list(ListKind::Outline);
    let restarted = d.numbering.restart(numbers).unwrap_or(numbers);
    let mut b = vec![Block::Para(styled("Lists", "Heading1")), Block::Para(para("Bulleted, three levels:"))];
    for (lvl, text) in
        [(0, "First bullet"), (0, "Second bullet"), (1, "Nested bullet"), (2, "Deeper bullet"), (1, "Back one level"), (0, "Last bullet")]
    {
        let mut p = styled(text, "ListParagraph");
        p.props.numbering = Some(NumRef { num: bullets, level: lvl });
        b.push(Block::Para(p));
    }
    b.push(Block::Para(para("Numbered, three levels:")));
    for (lvl, text) in [(0, "Step one"), (1, "Sub-step a"), (1, "Sub-step b"), (2, "Detail i"), (2, "Detail ii"), (0, "Step two"), (0, "Step three")]
    {
        let mut p = styled(text, "ListParagraph");
        p.props.numbering = Some(NumRef { num: numbers, level: lvl });
        b.push(Block::Para(p));
    }
    b.push(Block::Para(para("A plain paragraph between two lists.")));
    for text in ["Continues at four", "Then five"] {
        let mut p = styled(text, "ListParagraph");
        p.props.numbering = Some(NumRef { num: numbers, level: 0 });
        b.push(Block::Para(p));
    }
    b.push(Block::Para(para("Restarted:")));
    for text in ["Starts again at one", "Then two"] {
        let mut p = styled(text, "ListParagraph");
        p.props.numbering = Some(NumRef { num: restarted, level: 0 });
        b.push(Block::Para(p));
    }
    b.push(Block::Para(para("Lettered:")));
    for text in ["Apples", "Pears", "Plums"] {
        let mut p = styled(text, "ListParagraph");
        p.props.numbering = Some(NumRef { num: letters, level: 0 });
        b.push(Block::Para(p));
    }
    b.push(Block::Para(para("Outline (multilevel):")));
    for (lvl, text) in [(0, "Part"), (1, "Chapter"), (2, "Section"), (2, "Section"), (1, "Chapter"), (0, "Part")] {
        let mut p = styled(text, "ListParagraph");
        p.props.numbering = Some(NumRef { num: outline, level: lvl });
        b.push(Block::Para(p));
    }
    let mut long = styled(&words(9, 45), "ListParagraph");
    long.props.numbering = Some(NumRef { num: bullets, level: 0 });
    b.push(Block::Para(long));
    d.body = b.into_iter().map(Arc::new).collect();
    let mut doc = finish("lists", d);
    doc.render = &[0];
    doc
}

fn table_fixed() -> Doc {
    let mut t = table(5, 4, 468.0, 100);
    t.props.fixed = true;
    t.props.borders = Some(Borders {
        top: Some(border(BorderStyle::Double, 1.5, Some(Rgb(0x33, 0x33, 0x33)))),
        bottom: Some(border(BorderStyle::Double, 1.5, Some(Rgb(0x33, 0x33, 0x33)))),
        left: Some(border(BorderStyle::Single, 0.5, None)),
        right: Some(border(BorderStyle::Single, 0.5, None)),
        between: Some(border(BorderStyle::Dotted, 0.5, Some(Rgb(0x88, 0x88, 0x88)))),
        inside_v: Some(border(BorderStyle::Dashed, 0.5, Some(Rgb(0x88, 0x88, 0x88)))),
    });
    t.props.cell_margins = Some([3.0, 6.0, 3.0, 6.0]);
    t.props.caption = Some("Fixed widths".into());
    for (c, w) in [60.0, 180.0, 108.0, 120.0].iter().enumerate() {
        for r in 0..5 {
            if let Some(cell) = t.rows.get_mut(r).and_then(|row| row.cells.get_mut(c)) {
                cell.props.width = Some(*w);
            }
        }
    }
    if let Some(row) = t.rows.first_mut() {
        row.props.header = true;
        for cell in row.cells.iter_mut() {
            cell.props.shading = Some(Rgb(0xDE, 0xE8, 0xF4));
        }
    }
    if let Some(row) = t.rows.get_mut(1) {
        row.props.height = Some(40.0);
        row.props.height_rule = HeightRule::Exact;
        for (c, cell) in row.cells.iter_mut().enumerate() {
            cell.props.valign = Some([VAlign::Top, VAlign::Center, VAlign::Bottom, VAlign::Center][c]);
        }
    }
    if let Some(row) = t.rows.get_mut(2) {
        row.props.height = Some(48.0);
        row.props.height_rule = HeightRule::AtLeast;
    }
    cell_text(&mut t, 3, 1, &words(123, 30));
    let mut right = para("Right-aligned cell text");
    right.props.align = Some(Align::Right);
    cell_blocks(&mut t, 4, 1, vec![para_block(right)]);
    let mut d = text_doc(&["Before the fixed table.", "A second table, aligned right with an indent, follows."]);
    let mut t2 = table(2, 2, 240.0, 200);
    t2.props.fixed = true;
    t2.props.align = Some(Align::Right);
    t2.props.indent = Some(36.0);
    t2.props.shading = Some(Rgb(0xF4, 0xF4, 0xE8));
    d.body.insert(1, Arc::new(Block::Table(t)));
    d.body.push(Arc::new(Block::Table(t2)));
    d.body.push(para_block(para("After the tables.")));
    finish("table_fixed", d)
}

fn table_autofit() -> Doc {
    let mut t = Table::new(6, 3, 468.0);
    t.props.fixed = false;
    for r in 0..6 {
        for c in 0..3 {
            if let Some(cell) = t.rows.get_mut(r).and_then(|row| row.cells.get_mut(c)) {
                cell.props.width = None;
            }
        }
    }
    cell_text(&mut t, 0, 0, "Item");
    cell_text(&mut t, 0, 1, "Description");
    cell_text(&mut t, 0, 2, "Qty");
    cell_text(&mut t, 1, 0, "Press");
    cell_text(&mut t, 1, 1, &words(300, 24));
    cell_text(&mut t, 1, 2, "1");
    cell_text(&mut t, 2, 0, "Kiln");
    cell_text(&mut t, 2, 1, "Averyveryverylongunbreakablewordthatwidensitscolumn");
    cell_text(&mut t, 2, 2, "12");
    cell_text(&mut t, 3, 0, "Loom");
    cell_blocks(&mut t, 3, 1, vec![para_block(para("Two paragraphs")), para_block(para(&words(301, 10)))]);
    cell_text(&mut t, 3, 2, "3");
    cell_text(&mut t, 4, 0, "");
    cell_text(&mut t, 4, 1, "An empty first cell on this row");
    cell_text(&mut t, 4, 2, "0");
    cell_text(&mut t, 5, 0, "Light table");
    cell_text(&mut t, 5, 1, "Short");
    cell_text(&mut t, 5, 2, "100");
    let mut d = text_doc(&["AutoFit to contents: the columns take what their text needs."]);
    d.body.push(Arc::new(Block::Table(t)));
    let mut pct = table(3, 3, 468.0, 400);
    pct.props.width_pct = Some(60.0);
    pct.props.align = Some(Align::Center);
    d.body.push(para_block(para("A table 60% of the text width, centred:")));
    d.body.push(Arc::new(Block::Table(pct)));
    d.body.push(para_block(para("After the tables.")));
    finish("table_autofit", d)
}

fn table_merged() -> Doc {
    let mut t = table(5, 4, 468.0, 500);
    // Row 0: cells 0-1 merged across; row 1: cells 1-3 merged across.
    if let Some(row) = t.rows.get_mut(0) {
        row.cells.remove(1);
        if let Some(c0) = row.cells.get_mut(0) {
            c0.props.span = 2;
            c0.props.width = Some(234.0);
            c0.props.shading = Some(Rgb(0xFF, 0xE0, 0xB0));
            c0.blocks = vec![para_block(para("Spans two columns"))];
        }
    }
    if let Some(row) = t.rows.get_mut(1) {
        row.cells.truncate(2);
        if let Some(c1) = row.cells.get_mut(1) {
            c1.props.span = 3;
            c1.props.width = Some(351.0);
            c1.blocks = vec![para_block(para("Spans three columns"))];
        }
    }
    // Column 0: rows 2-4 merged down; column 3: rows 2-3 merged down.
    for (r, vm) in [(2, VMerge::Restart), (3, VMerge::Continue), (4, VMerge::Continue)] {
        if let Some(c) = t.rows.get_mut(r).and_then(|row| row.cells.get_mut(0)) {
            c.props.vmerge = vm;
            c.props.valign = Some(VAlign::Center);
            if vm == VMerge::Continue {
                c.blocks = vec![para_block(Paragraph::new())];
            } else {
                c.blocks = vec![para_block(para("Merged down three rows"))];
            }
        }
    }
    for (r, vm) in [(2, VMerge::Restart), (3, VMerge::Continue)] {
        if let Some(c) = t.rows.get_mut(r).and_then(|row| row.cells.get_mut(3)) {
            c.props.vmerge = vm;
            c.props.shading = Some(Rgb(0xD0, 0xF0, 0xD0));
            if vm == VMerge::Continue {
                c.blocks = vec![para_block(Paragraph::new())];
            }
        }
    }
    let mut d = text_doc(&["Merged cells: spans across and merges down."]);
    d.body.push(Arc::new(Block::Table(t)));
    d.body.push(para_block(para("After the table.")));
    let mut doc = finish("table_merged", d);
    doc.render = &[0];
    doc
}

fn table_nested() -> Doc {
    let mut inner = table(2, 2, 200.0, 600);
    inner.props.borders = Some(Borders::all(border(BorderStyle::Single, 1.0, Some(Rgb(0x1F, 0x4E, 0x79)))));
    let mut inner2 = table(1, 3, 150.0, 650);
    inner2.props.shading = Some(Rgb(0xEE, 0xEE, 0xEE));
    let mut outer = table(2, 2, 468.0, 700);
    cell_blocks(
        &mut outer,
        0,
        1,
        vec![para_block(para("Outer cell with an inner table:")), Arc::new(Block::Table(inner)), para_block(para("after the inner table"))],
    );
    cell_blocks(&mut outer, 1, 0, vec![Arc::new(Block::Table(inner2)), para_block(Paragraph::new())]);
    let mut d = text_doc(&["Nested tables."]);
    d.body.push(Arc::new(Block::Table(outer)));
    d.body.push(para_block(para("After the tables.")));
    finish("table_nested", d)
}

fn table_split() -> Doc {
    let mut t = table(48, 3, 468.0, 800);
    if let Some(row) = t.rows.first_mut() {
        row.props.header = true;
        for (c, cell) in row.cells.iter_mut().enumerate() {
            cell.blocks = vec![para_block(para(["Column A", "Column B", "Column C"][c]))];
            cell.props.shading = Some(Rgb(0xE0, 0xE0, 0xE0));
        }
    }
    cell_text(&mut t, 20, 1, &words(820, 70));
    if let Some(row) = t.rows.get_mut(20) {
        row.props.cant_split = true;
    }
    cell_text(&mut t, 33, 2, &words(833, 90));
    if let Some(row) = t.rows.get_mut(40) {
        row.props.height = Some(60.0);
        row.props.height_rule = HeightRule::Exact;
    }
    let mut d = text_doc(&["A table that runs over several pages, with a repeated header row, a row that can't split and one with an exact height."]);
    d.body.push(Arc::new(Block::Table(t)));
    d.body.push(para_block(para("After the long table.")));
    finish("table_split", d)
}

fn section_props(start: SectionStart) -> SectionProps {
    SectionProps { start, ..SectionProps::default() }
}

fn sections() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = Vec::new();
    b.push(styled("Section one: Letter, portrait", "Heading1"));
    for i in 0..6 {
        b.push(para(&words(900 + i, 55)));
    }
    let mut s1 = section_props(SectionStart::NextPage);
    s1.page_num_format = NumFormat::LowerRoman;
    s1.page_num_start = Some(1);
    s1.page_num_set = true;
    if let Some(last) = b.last_mut() {
        last.section = Some(Box::new(s1));
    }
    b.push(styled("Section two: A4 landscape, wide margins", "Heading1"));
    for i in 0..5 {
        b.push(para(&words(920 + i, 55)));
    }
    let mut s2 = SectionProps { page_w: 595.3, page_h: 841.9, ..section_props(SectionStart::NextPage) };
    s2.set_landscape(true);
    s2.margin_left = 90.0;
    s2.margin_right = 90.0;
    s2.margin_top = 54.0;
    s2.margin_bottom = 54.0;
    s2.page_num_format = NumFormat::Decimal;
    s2.page_num_start = Some(1);
    s2.page_num_set = true;
    if let Some(last) = b.last_mut() {
        last.section = Some(Box::new(s2));
    }
    b.push(styled("Section three: continuous, two columns", "Heading1"));
    for i in 0..8 {
        b.push(para(&words(940 + i, 40)));
    }
    let mut s3 = section_props(SectionStart::Continuous);
    s3.columns = Columns { count: 2, space: 24.0, separator: false, widths: Vec::new() };
    if let Some(last) = b.last_mut() {
        last.section = Some(Box::new(s3));
    }
    b.push(styled("Section four: odd page, Letter again", "Heading1"));
    for i in 0..3 {
        b.push(para(&words(960 + i, 55)));
    }
    d.last_section = section_props(SectionStart::OddPage);
    d.body = b.into_iter().map(para_block).collect();
    finish("sections", d)
}

fn columns() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = Vec::new();
    b.push(styled("Columns", "Heading1"));
    for i in 0..14 {
        b.push(para(&words(1000 + i, 28)));
    }
    let mut broke = para(&words(1020, 10));
    broke.text.push(wordcraft_doc::para::COLUMN_BREAK);
    broke.runs.push(wordcraft_doc::Run { len: 1, props: chr() });
    b.push(broke);
    b.push(para("After a column break."));
    for i in 0..4 {
        b.push(para(&words(1030 + i, 28)));
    }
    let mut s1 = section_props(SectionStart::Continuous);
    s1.columns = Columns { count: 2, space: 36.0, separator: true, widths: Vec::new() };
    if let Some(last) = b.last_mut() {
        last.section = Some(Box::new(s1));
    }
    b.push(styled("Three unequal columns", "Heading2"));
    for i in 0..9 {
        b.push(para(&words(1040 + i, 24)));
    }
    let mut s2 = section_props(SectionStart::Continuous);
    s2.columns = Columns { count: 3, space: 18.0, separator: false, widths: vec![(216.0, 18.0), (108.0, 18.0), (108.0, 0.0)] };
    if let Some(last) = b.last_mut() {
        last.section = Some(Box::new(s2));
    }
    b.push(para("Back to one column, which balances the columns above."));
    d.last_section = section_props(SectionStart::Continuous);
    d.body = b.into_iter().map(para_block).collect();
    let mut doc = finish("columns", d);
    doc.render = &[0];
    doc
}

fn headers_footers() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = vec![styled("Headers and Footers", "Title")];
    for i in 0..22 {
        b.push(para(&words(1100 + i, 60)));
    }
    d.body = b.into_iter().map(para_block).collect();
    let mut hd = styled("Running head\tChapter One\t", "Header");
    hd.insert_object(hd.len(), field("PAGE", "1"), &chr()).unwrap();
    hd.props.borders = Some(Borders { bottom: Some(border(BorderStyle::Single, 0.75, Some(Rgb(0x99, 0x99, 0x99)))), ..Default::default() });
    let h_default = hf_part(&mut d, PartKind::Header, hd);
    let h_first = hf_part(&mut d, PartKind::Header, styled("Title page header (different first page)", "Header"));
    let mut he = styled("Even page header", "Header");
    he.props.align = Some(Align::Right);
    let h_even = hf_part(&mut d, PartKind::Header, he);
    let mut ft = styled("Page ", "Footer");
    ft.props.align = Some(Align::Center);
    ft.insert_object(ft.len(), field("PAGE", "1"), &chr()).unwrap();
    add(&mut ft, " of ", chr());
    ft.insert_object(ft.len(), field("NUMPAGES", "3"), &chr()).unwrap();
    let f_default = hf_part(&mut d, PartKind::Footer, ft);
    let mut ff = styled("First page footer\tright tab", "Footer");
    ff.props.tabs = Some(vec![TabStop { pos: 468.0, align: TabAlign::Right, leader: TabLeader::None }]);
    let f_first = hf_part(&mut d, PartKind::Footer, ff);
    d.last_section.headers = HeaderSet { default: Some(h_default), first: Some(h_first), even: Some(h_even) };
    d.last_section.footers = HeaderSet { default: Some(f_default), first: Some(f_first), even: None };
    d.last_section.title_page = true;
    d.last_section.header = 30.0;
    d.last_section.footer = 30.0;
    d.settings.even_odd_headers = true;
    finish("headers_footers", d)
}

fn footnotes() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = vec![styled("Footnotes", "Heading1")];
    for i in 0..14 {
        b.push(para(&words(1200 + i, 60)));
    }
    let mut t = table(2, 2, 468.0, 1300);
    cell_text(&mut t, 0, 0, "A note in a table cell");
    d.body = b.into_iter().map(para_block).collect();
    d.body.insert(4, Arc::new(Block::Table(t)));
    d.body.push(para_block(para("The last paragraph.")));
    let note = |d: &mut Document, pos: Pos, text: &str, custom: &str| {
        let n = Paragraph::with_text(text, chr()).styled("FootnoteText");
        let id = d.add_part(PartKind::Footnote, vec![para_block(n)]);
        d.insert_object(&pos, InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: custom.into() }, &chr()).unwrap();
    };
    note(&mut d, Pos::body(1, 30), "The first footnote, short.", "");
    note(&mut d, Pos::body(2, 10), &words(1400, 120), "");
    note(&mut d, Pos::body(3, 5), "A custom mark instead of a number.", "*");
    note(&mut d, Pos::new(StoryRef::Body, wordcraft_doc::Path(vec![4, 0, 0, 0]), 6), "A footnote referenced from inside a table.", "");
    note(&mut d, Pos::body(6, 20), &words(1401, 160), "");
    note(&mut d, Pos::body(7, 20), &words(1402, 150), "");
    note(&mut d, Pos::body(8, 20), "Three notes on one page.", "");
    note(&mut d, Pos::body(12, 20), "Near the end.", "");
    finish("footnotes", d)
}

fn endnotes() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = vec![styled("Endnotes", "Heading1")];
    for i in 0..8 {
        b.push(para(&words(1500 + i, 50)));
    }
    d.body = b.into_iter().map(para_block).collect();
    for (i, (block, off)) in [(1usize, 12usize), (2, 40), (5, 8), (8, 30)].into_iter().enumerate() {
        let n = Paragraph::with_text(&format!("Endnote {} says: {}", i + 1, words(1600 + i, 20)), chr()).styled("EndnoteText");
        let id = d.add_part(PartKind::Endnote, vec![para_block(n)]);
        d.insert_object(&Pos::body(block, off), InlineObject::NoteRef { kind: NoteKind::Endnote, id, custom: String::new() }, &chr()).unwrap();
    }
    d.settings.endnote_format = NumFormat::LowerRoman;
    finish("endnotes", d)
}

fn text_box_part(d: &mut Document, blocks: Blocks, vert: TextVert, anchor: VAlign) -> u32 {
    let id = d.add_part(PartKind::TextBox, blocks);
    if let Some(p) = d.parts.get_mut(&id) {
        p.body.vert = vert;
        p.body.anchor = anchor;
    }
    id
}

fn text_box_obj(story: u32, w: f32, h: f32, float: Float) -> InlineObject {
    InlineObject::Shape {
        kind: ShapeKind::TextBox,
        w,
        h,
        fill: Some(Rgb(0xFF, 0xFF, 0xF0)),
        stroke: Some(Rgb(0x40, 0x40, 0x40)),
        stroke_width: 1.0,
        float,
        story: Some(story),
        effects: Default::default(),
        freeform: None,
        extra: Default::default(),
    }
}

fn text_boxes() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = vec![styled("Text Boxes", "Heading1")];
    for i in 0..8 {
        b.push(para(&words(1700 + i, 60)));
    }
    d.body = b.into_iter().map(para_block).collect();
    // Inline box at the end of paragraph 1.
    let inline = text_box_part(&mut d, vec![para_block(para("An inline text box, two lines of text inside it."))], TextVert::Horz, VAlign::Top);
    let end1 = d.para(StoryRef::Body, &wordcraft_doc::Path::top(1)).map_or(0, |p| p.len());
    d.insert_object(&Pos::body(1, end1), text_box_obj(inline, 150.0, 48.0, Float::default()), &chr()).unwrap();
    // Floating box, square wrap, right of the margin, anchored to paragraph 3.
    let mut lb = d.numbering.clone();
    let bullets = lb.add_list(ListKind::Bullet);
    d.numbering = lb;
    let mut item = styled("A bullet inside the box", "ListParagraph");
    item.props.numbering = Some(NumRef { num: bullets, level: 0 });
    let floating = text_box_part(
        &mut d,
        vec![para_block(styled("Floating box", "Heading3")), para_block(para(&words(1800, 30))), para_block(item)],
        TextVert::Horz,
        VAlign::Top,
    );
    let sq = Float {
        wrap: Wrap::Square,
        wrap_text: WrapText::BothSides,
        h_rel: Anchor::Margin,
        v_rel: Anchor::Paragraph,
        x: 288.0,
        y: 0.0,
        dist: 9.0,
        ..Default::default()
    };
    d.insert_object(&Pos::body(3, 0), text_box_obj(floating, 180.0, 120.0, sq), &chr()).unwrap();
    // A box with vertical text, anchored to paragraph 6, text wrapping top and bottom.
    let vertical = text_box_part(&mut d, vec![para_block(para("Rotated text runs down"))], TextVert::Vert, VAlign::Center);
    let tb = Float {
        wrap: Wrap::TopAndBottom,
        h_rel: Anchor::Margin,
        v_rel: Anchor::Paragraph,
        x: 0.0,
        y: 0.0,
        h_align: Some(FloatAlign::Center),
        ..Default::default()
    };
    d.insert_object(&Pos::body(6, 0), text_box_obj(vertical, 40.0, 150.0, tb), &chr()).unwrap();
    finish("text_boxes", d)
}

fn floats() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = vec![styled("Floating Pictures and Shapes", "Heading1")];
    for i in 0..10 {
        b.push(para(&words(1900 + i, 55)));
    }
    d.body = b.into_iter().map(para_block).collect();
    let m1 = d.add_media(png(1, 64, 48), "png");
    let m2 = d.add_media(png(2, 96, 32), "png");
    let m3 = d.add_media(png(3, 48, 48), "png");
    let square_left = Float {
        wrap: Wrap::Square,
        wrap_text: WrapText::Right,
        h_rel: Anchor::Margin,
        v_rel: Anchor::Paragraph,
        x: 0.0,
        y: 6.0,
        dist: 7.2,
        ..Default::default()
    };
    d.insert_object(&Pos::body(1, 0), image(m1, 128.0, 96.0, "Square wrapped picture on the left", square_left), &chr()).unwrap();
    let top_bottom = Float {
        wrap: Wrap::TopAndBottom,
        h_rel: Anchor::Margin,
        v_rel: Anchor::Paragraph,
        h_align: Some(FloatAlign::Center),
        y: 0.0,
        dist_top: 6.0,
        dist_bottom: 6.0,
        ..Default::default()
    };
    d.insert_object(&Pos::body(3, 0), image(m2, 192.0, 64.0, "Top and bottom wrapped", top_bottom), &chr()).unwrap();
    let behind = Float { wrap: Wrap::BehindText, h_rel: Anchor::Page, v_rel: Anchor::Page, x: 400.0, y: 500.0, ..Default::default() };
    d.insert_object(&Pos::body(5, 0), image(m3, 144.0, 144.0, "Behind the text", behind), &chr()).unwrap();
    let in_front =
        Float { wrap: Wrap::InFrontOfText, h_rel: Anchor::Margin, v_rel: Anchor::Paragraph, x: 300.0, y: 10.0, rot: 20.0, ..Default::default() };
    d.insert_object(
        &Pos::body(7, 0),
        shape(ShapeKind::Ellipse, 90.0, 60.0, Some(Rgb(0xFF, 0xD7, 0x00)), Some(Rgb(0x80, 0x60, 0x00)), in_front),
        &chr(),
    )
    .unwrap();
    let tight = Float {
        wrap: Wrap::Tight,
        wrap_text: WrapText::BothSides,
        h_rel: Anchor::Margin,
        v_rel: Anchor::Paragraph,
        x: 150.0,
        y: 0.0,
        dist: 4.0,
        ..Default::default()
    };
    d.insert_object(&Pos::body(8, 0), shape(ShapeKind::Triangle, 80.0, 80.0, Some(Rgb(0x66, 0xAA, 0xDD)), None, tight), &chr()).unwrap();
    // An inline picture with a crop, and an inline arrow.
    let m4 = d.add_media(png(4, 80, 60), "png");
    let end2 = d.para(StoryRef::Body, &wordcraft_doc::Path::top(2)).map_or(0, |p| p.len());
    d.insert_object(
        &Pos::body(2, end2),
        InlineObject::Image {
            media: m4,
            w: 60.0,
            h: 45.0,
            alt: "Cropped inline picture".into(),
            float: Float::default(),
            crop: [0.1, 0.2, 0.1, 0.0],
            ole: None,
        },
        &chr(),
    )
    .unwrap();
    d.insert_object(&Pos::body(9, 0), shape(ShapeKind::Arrow, 72.0, 18.0, Some(Rgb(0x22, 0x88, 0x22)), None, Float::default()), &chr()).unwrap();
    let mut doc = finish("floats", d);
    doc.render = &[0];
    doc
}

fn fields_toc() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = vec![styled("Fields and a Table of Contents", "Title"), para("The table of contents goes here:"), para("")];
    for ch in 1..=3 {
        b.push(styled(&format!("Chapter {ch}"), "Heading1"));
        b.push(para(&words(2000 + ch * 10, 60)));
        for sec in 1..=2 {
            b.push(styled(&format!("Section {ch}.{sec}"), "Heading2"));
            b.push(para(&words(2000 + ch * 10 + sec, 50)));
            if sec == 1 {
                b.push(styled(&format!("Topic {ch}.{sec}.1"), "Heading3"));
                b.push(para(&words(2100 + ch, 40)));
            }
        }
    }
    b.push(para("A bookmarked phrase: the quick brown fox."));
    let mut fields = para("Fields: page ");
    fields.insert_object(fields.len(), field("PAGE", ""), &chr()).unwrap();
    add(&mut fields, " of ", chr());
    fields.insert_object(fields.len(), field("NUMPAGES", ""), &chr()).unwrap();
    add(&mut fields, "; date ", chr());
    fields.insert_object(fields.len(), field("DATE \\@ \"yyyy-MM-dd\"", ""), &chr()).unwrap();
    add(&mut fields, "; time ", chr());
    fields.insert_object(fields.len(), field("TIME \\@ \"HH:mm\"", ""), &chr()).unwrap();
    add(&mut fields, "; author ", chr());
    fields.insert_object(fields.len(), field("AUTHOR", ""), &chr()).unwrap();
    add(&mut fields, "; title ", chr());
    fields.insert_object(fields.len(), field("TITLE", ""), &chr()).unwrap();
    add(&mut fields, "; words ", chr());
    fields.insert_object(fields.len(), field("NUMWORDS", ""), &chr()).unwrap();
    add(&mut fields, ".", chr());
    b.push(fields);
    for n in 1..=3 {
        let mut cap = para("Figure ");
        cap.insert_object(cap.len(), field("SEQ Figure \\* ARABIC", ""), &chr()).unwrap();
        add(&mut cap, &format!(": caption {n}"), chr());
        b.push(cap);
    }
    let mut refs = para("See ");
    refs.insert_object(refs.len(), field("REF fox \\h", ""), &chr()).unwrap();
    add(&mut refs, " on page ", chr());
    refs.insert_object(refs.len(), field("PAGEREF fox \\h", ""), &chr()).unwrap();
    add(&mut refs, ". Nested: ", chr());
    let inner = wordcraft_doc::para::NestedField { at: 3, instr: "REF fox".into(), result: "fox".into(), ..Default::default() };
    refs.insert_object(
        refs.len(),
        InlineObject::Field {
            instr: "IF \"x\" = \"x\" \"yes\" \"no\"".into(),
            result: "yes".into(),
            locked: false,
            dirty: false,
            code: false,
            form: String::new(),
            nested: vec![inner],
        },
        &chr(),
    )
    .unwrap();
    add(&mut refs, ".", chr());
    b.push(refs);
    d.body = b.into_iter().map(para_block).collect();
    stamp(&mut d, "Snapshot: fields_toc");
    let mut s = session(d);
    select(&mut s, "quick brown fox");
    run(&mut s, "insert.bookmark", json!({"name": "fox"}));
    select(&mut s, "bookmarked phrase");
    run(&mut s, "insert.link", json!({"url": "https://example.org/fox"}));
    caret(&mut s, 2, 0);
    run(&mut s, "references.toc", json!({"levels": 3, "tabLeader": "dot"}));
    run(&mut s, "references.updateFields", json!({}));
    let mut fld = Paragraph::new().styled("Footer");
    fld.props.align = Some(Align::Center);
    fld.insert_object(0, field("PAGE", "1"), &chr()).unwrap();
    let fid = hf_part(&mut s.doc, PartKind::Footer, fld);
    s.doc.last_section.footers.default = Some(fid);
    run(&mut s, "references.updateFields", json!({}));
    let mut doc = finish("fields_toc", s.doc);
    doc.variants.push(("field_codes", LayoutOptions { field_codes: true, ..opts() }));
    doc
}

fn tracked_changes() -> Doc {
    let mut d = text_doc(&[
        "Tracked changes: words inserted, words deleted, formatting changed, a paragraph split and one joined.",
        &words(2200, 45),
        &words(2201, 45),
        "This whole paragraph is deleted by the second reviewer.",
        &words(2202, 45),
        "The last paragraph gets a tracked table after it.",
    ]);
    stamp(&mut d, "Snapshot: tracked_changes");
    let mut s = session(d);
    run(&mut s, "review.trackChanges", json!({"value": true}));
    select(&mut s, "words inserted");
    run(&mut s, "select.collapse", json!({"end": true}));
    run(&mut s, "text.insert", json!({"text": " (and these words were added)"}));
    select(&mut s, "words deleted");
    run(&mut s, "text.delete", json!({}));
    select(&mut s, "formatting changed");
    run(&mut s, "format.bold", json!({}));
    run(&mut s, "format.color", json!({"color": "C00000"}));
    caret(&mut s, 1, 40);
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "text.insert", json!({"text": "New paragraph typed with tracking on."}));
    s.author = "Second Reviewer".into();
    s.initials = "SR".into();
    select(&mut s, "This whole paragraph is deleted by the second reviewer.");
    run(&mut s, "text.delete", json!({}));
    let end = s.doc.body.len() - 1;
    caret(&mut s, end, 0);
    run(&mut s, "caret.end", json!({}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "insert.table", json!({"rows": 2, "cols": 2}));
    run(&mut s, "text.insert", json!({"text": "tracked cell"}));
    run(&mut s, "table.insertRowBelow", json!({}));
    run(&mut s, "text.insert", json!({"text": "inserted row"}));
    s.author = "Snapshot Author".into();
    select(&mut s, "paragraph split");
    run(&mut s, "select.collapse", json!({"end": true}));
    run(&mut s, "text.insert", json!({"text": " [moved words]"}));
    let mut doc = finish("tracked_changes", s.doc);
    doc.variants.push(("no_markup", LayoutOptions { hide_deleted: true, ..opts() }));
    doc.render = &[0];
    doc
}

fn comments() -> Doc {
    let mut d = text_doc(&[
        "Comments: a remark on a phrase, a reply to it, a resolved thread and a comment on a whole paragraph.",
        &words(2300, 50),
        &words(2301, 50),
        "A short last paragraph.",
    ]);
    stamp(&mut d, "Snapshot: comments");
    let mut s = session(d);
    select(&mut s, "remark on a phrase");
    run(&mut s, "review.newComment", json!({"text": "The first remark, on a phrase."}));
    run(&mut s, "review.reply", json!({"id": 0, "text": "A reply from the same author."}));
    s.author = "Second Reviewer".into();
    s.initials = "SR".into();
    select(&mut s, "resolved thread");
    run(&mut s, "review.newComment", json!({"text": "This one is resolved."}));
    run(&mut s, "review.resolveComment", json!({"id": 2}));
    run(&mut s, "select.paragraph", json!({}));
    caret(&mut s, 2, 0);
    run(&mut s, "select.paragraph", json!({}));
    run(
        &mut s,
        "review.newComment",
        json!({"text": "A longer comment on the whole paragraph, so the balloon wraps onto several lines in the markup area."}),
    );
    let mut doc = finish("comments", s.doc);
    doc.variants.push(("print_markup", LayoutOptions { comment_balloons: true, ..opts() }));
    doc.export = ExportOptions { markup: true, comments: true, hidden_text: false, word2003: false };
    doc
}

fn rtl() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = vec![styled("Right-to-Left Text", "Heading1")];
    let mut latin_rtl = para(&words(2400, 40));
    latin_rtl.props.bidi = Some(true);
    b.push(latin_rtl);
    let mut hebrew = para("שלום עולם, זהו משפט בעברית עם מספרים 123 ו-456 בתוכו.");
    hebrew.props.bidi = Some(true);
    b.push(hebrew);
    let mut arabic = para("مرحبا بالعالم، هذه جملة عربية فيها كلمات english وأرقام 789.");
    arabic.props.bidi = Some(true);
    arabic.props.align = Some(Align::Justify);
    b.push(arabic);
    b.push(para("Mixed in a left-to-right paragraph: Hello שלום 42 world مرحبا again."));
    let mut right = para("Right-aligned right-to-left paragraph.");
    right.props.bidi = Some(true);
    right.props.align = Some(Align::Right);
    b.push(right);
    let mut t = table(2, 3, 468.0, 2450);
    t.props.bidi_visual = true;
    cell_text(&mut t, 0, 0, "ימין");
    cell_text(&mut t, 0, 2, "שמאל");
    let mut s1 = section_props(SectionStart::NextPage);
    s1.rtl = false;
    if let Some(last) = b.last_mut() {
        last.section = Some(Box::new(s1));
    }
    b.push(styled("A right-to-left section", "Heading2"));
    b.push(para(&words(2460, 50)));
    let mut num = d.numbering.clone();
    let list = num.add_list(ListKind::Numbered);
    d.numbering = num;
    for text in ["פריט ראשון", "פריט שני"] {
        let mut p = styled(text, "ListParagraph");
        p.props.numbering = Some(NumRef { num: list, level: 0 });
        p.props.bidi = Some(true);
        b.push(p);
    }
    d.last_section = SectionProps { rtl: true, ..section_props(SectionStart::NextPage) };
    d.body = b.into_iter().map(para_block).collect();
    d.body.insert(6, Arc::new(Block::Table(t)));
    finish("rtl", d)
}

fn vertical() -> Doc {
    let text = "Vertical sections lay their lines out down the page, one after another from right to left. ".repeat(3);
    let mut d = Document::from_text(&format!("{text}\n日本語のテキストと英語 mixed text."));
    let mut t = table(2, 3, 300.0, 2500);
    if let Some(c) = t.rows.get_mut(0).and_then(|r| r.cells.get_mut(0)) {
        c.props.text_direction = TextDirection::Down;
        c.props.text_direction_set = true;
        c.blocks = vec![para_block(para("Turned down"))];
    }
    if let Some(c) = t.rows.get_mut(1).and_then(|r| r.cells.get_mut(2)) {
        c.props.text_direction = TextDirection::Up;
        c.props.text_direction_set = true;
        c.blocks = vec![para_block(para("Turned up, two words more"))];
    }
    if let Some(row) = t.rows.get_mut(0) {
        row.props.height = Some(90.0);
        row.props.height_rule = HeightRule::AtLeast;
    }
    d.body.push(Arc::new(Block::Table(t)));
    d.body.push(para_block(para("After the table in the vertical section.")));
    d.last_section.text_direction = TextDirection::Down;
    d.last_section.text_direction_set = true;
    let vb = text_box_part(&mut d, vec![para_block(para("A box with text turned 270°"))], TextVert::Vert270, VAlign::Top);
    d.insert_object(&Pos::body(0, 0), text_box_obj(vb, 36.0, 140.0, Float::default()), &chr()).unwrap();
    finish("vertical", d)
}

fn east_asian() -> Doc {
    let ja = "吾輩は猫である。名前はまだ無い。どこで生れたかとんと見当がつかぬ。何でも薄暗いじめじめした所でニャーニャー泣いていた事だけは記憶している。吾輩はここで始めて人間というものを見た。";
    let zh = "北京的秋天是最好的季节，天高云淡，风和日丽。人们走在街上，感受着阳光的温暖。这是一段用于测试中文换行的文字，包含标点符号、数字123和English words。";
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = vec![styled("East Asian Line Breaking", "Heading1")];
    let mut p1 = para(ja);
    p1.props.kinsoku = Some(true);
    b.push(p1);
    let mut p2 = para(ja);
    p2.props.kinsoku = Some(false);
    b.push(p2);
    let mut p3 = para(zh);
    p3.props = ParaProps { align: Some(Align::Justify), auto_space_de: Some(true), auto_space_dn: Some(true), ..Default::default() };
    b.push(p3);
    let mut p4 = para(zh);
    p4.props = ParaProps { overflow_punct: Some(true), word_wrap: Some(false), ..Default::default() };
    b.push(p4);
    let mut p5 = para("Latin and 日本語 mixed on one line, with 全角 characters and punctuation、like this。");
    p5.props.snap_to_grid = Some(true);
    b.push(p5);
    d.body = b.into_iter().map(para_block).collect();
    d.last_section.doc_grid = Some(DocGrid { kind: DocGridType::Lines, line_pitch: 18.0, char_space: 0 });
    finish("east_asian", d)
}

fn hyphenation() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = vec![styled("Hyphenation", "Heading1")];
    let long = "Extraordinarily uncharacteristic internationalization responsibilities overwhelmingly counterproductive misunderstandings notwithstanding incomprehensibility. ";
    b.push(para(&long.repeat(3)));
    let mut no = para(&long.repeat(2));
    no.props.suppress_hyphens = Some(true);
    b.push(no);
    b.push(para("Soft hyphens: ex\u{AD}tra\u{AD}or\u{AD}di\u{AD}nar\u{AD}i\u{AD}ly un\u{AD}char\u{AD}ac\u{AD}ter\u{AD}is\u{AD}tic in\u{AD}ter\u{AD}na\u{AD}tion\u{AD}al\u{AD}i\u{AD}za\u{AD}tion re\u{AD}spon\u{AD}si\u{AD}bil\u{AD}i\u{AD}ties over\u{AD}whelm\u{AD}ing\u{AD}ly."));
    b.push(para("Non-breaking hyphens keep well\u{2011}known\u{2011}compound\u{2011}words\u{2011}together on the line and an ordinary long-hyphenated-compound-word may break at its hyphens."));
    b.push(para("ALLCAPS INTERNATIONALIZATION RESPONSIBILITIES OVERWHELMINGLY COUNTERPRODUCTIVE MISUNDERSTANDINGS."));
    d.body = b.into_iter().map(para_block).collect();
    d.last_section.columns = Columns { count: 3, space: 18.0, separator: false, widths: Vec::new() };
    d.settings.auto_hyphenation = true;
    d.settings.hyphenation_zone = 18.0;
    d.settings.consecutive_hyphen_limit = 2;
    d.settings.hyphenate_caps = false;
    finish("hyphenation", d)
}

fn breaks_keeps() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = vec![styled("Breaks, Widows and Keeps", "Heading1")];
    for i in 0..9 {
        b.push(para(&words(2600 + i, 60)));
    }
    let mut pb = para("A paragraph ending in a page break.");
    pb.text.push(wordcraft_doc::para::PAGE_BREAK);
    pb.runs.push(wordcraft_doc::Run { len: 1, props: chr() });
    b.push(pb);
    b.push(para("First paragraph on the next page."));
    for i in 0..9 {
        b.push(para(&words(2620 + i, 60)));
    }
    let mut keep_head = styled("A heading kept with the next paragraph", "Heading2");
    keep_head.props.keep_next = Some(true);
    b.push(keep_head);
    let mut kept = para(&words(2640, 120));
    kept.props.keep_lines = Some(true);
    b.push(kept);
    let mut widow = para(&words(2641, 50));
    widow.props.widow_control = Some(true);
    b.push(widow);
    let mut orphan = para(&words(2642, 50));
    orphan.props.widow_control = Some(false);
    b.push(orphan);
    for i in 0..6 {
        b.push(para(&words(2650 + i, 60)));
    }
    let mut before = para("Page break before this paragraph.");
    before.props.page_break_before = Some(true);
    b.push(before);
    let mut spaced = para("Space before at the top of a page is suppressed.");
    spaced.props.space_before = Some(48.0);
    b.push(spaced);
    d.body = b.into_iter().map(para_block).collect();
    finish("breaks_keeps", d)
}

fn drop_cap_tabs() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = vec![styled("Drop Caps, Tabs, Borders and Line Numbers", "Heading1")];
    let mut dc = para(&words(2700, 70));
    dc.props.drop_cap = Some(3);
    b.push(dc);
    let mut tabs = para("Left\tCentre\tRight\t12.5\tdone");
    tabs.props.tabs = Some(vec![
        TabStop { pos: 72.0, align: TabAlign::Left, leader: TabLeader::None },
        TabStop { pos: 216.0, align: TabAlign::Center, leader: TabLeader::Dot },
        TabStop { pos: 360.0, align: TabAlign::Right, leader: TabLeader::Hyphen },
        TabStop { pos: 420.0, align: TabAlign::Decimal, leader: TabLeader::Underscore },
    ]);
    b.push(tabs);
    let mut tabs2 = para("Item\t1.25\nSecond item\t12.5\nThird\t100.125");
    tabs2.props.tabs = Some(vec![TabStop { pos: 300.0, align: TabAlign::Decimal, leader: TabLeader::Dot }]);
    b.push(tabs2);
    let mut boxed = para(&words(2701, 40));
    boxed.props.borders = Some(Borders::all(border(BorderStyle::Single, 1.0, Some(Rgb(0x1F, 0x4E, 0x79)))));
    boxed.props.shading = Some(Rgb(0xF0, 0xF6, 0xFC));
    boxed.props.indent_left = Some(18.0);
    boxed.props.indent_right = Some(18.0);
    b.push(boxed);
    let mut ruled = para(&words(2702, 30));
    ruled.props.borders = Some(Borders {
        top: Some(border(BorderStyle::Thick, 2.0, None)),
        bottom: Some(border(BorderStyle::Wave, 1.0, Some(Rgb(0xC0, 0, 0)))),
        ..Default::default()
    });
    b.push(ruled);
    for i in 0..3 {
        b.push(para(&words(2710 + i, 60)));
    }
    let mut no_num = para("This paragraph suppresses its line numbers.");
    no_num.props.suppress_line_numbers = Some(true);
    b.push(no_num);
    b.push(para(&words(2720, 60)));
    d.body = b.into_iter().map(para_block).collect();
    d.last_section.line_numbers = Some(LineNumbering { count_by: 1, start: 1, distance: 18.0, restart: LineNumberRestart::Page });
    finish("drop_cap_tabs", d)
}

fn page_setup() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = vec![styled("Page Setup", "Title")];
    for i in 0..16 {
        b.push(para(&words(2800 + i, 60)));
    }
    d.body = b.into_iter().map(para_block).collect();
    d.last_section.page_w = 612.0;
    d.last_section.page_h = 1008.0;
    d.last_section.set_landscape(true);
    d.last_section.margin_top = 54.0;
    d.last_section.margin_bottom = 54.0;
    d.last_section.margin_left = 72.0;
    d.last_section.margin_right = 54.0;
    d.last_section.gutter = 36.0;
    d.last_section.header = 24.0;
    d.last_section.footer = 24.0;
    d.last_section.valign = PageVAlign::Center;
    d.last_section.page_borders = Some(Borders::all(border(BorderStyle::Double, 1.5, Some(Rgb(0x55, 0x33, 0x11)))));
    d.settings.mirror_margins = true;
    d.settings.page_color = Some(Rgb(0xFF, 0xF8, 0xE1));
    d.settings.default_tab = 72.0;
    let mut doc = finish("page_setup", d);
    doc.render = &[0];
    doc
}

fn custom_styles() -> Doc {
    let mut d = Document::new();
    d.styles.upsert(Style {
        id: "BodyNote".into(),
        name: "Body Note".into(),
        based_on: Some("Normal".into()),
        next: Some("Normal".into()),
        para: ParaProps { indent_left: Some(36.0), space_after: Some(4.0), ..Default::default() },
        chr: CharProps { size: Some(10.0), italic: Some(true), color: color(0x40, 0x40, 0x40), ..chr() },
        quick: true,
        ..Default::default()
    });
    d.styles.upsert(Style {
        id: "CodeChar".into(),
        name: "Code".into(),
        kind: StyleKind::Character,
        based_on: Some("DefaultParagraphFont".into()),
        chr: CharProps { font: Some("JetBrains Mono".into()), size: Some(10.0), shading: Some(Rgb(0xEE, 0xEE, 0xEE)), ..chr() },
        ..Default::default()
    });
    d.styles.upsert(Style {
        id: "Boxed".into(),
        name: "Boxed".into(),
        based_on: Some("BodyNote".into()),
        para: ParaProps { borders: Some(Borders::all(border(BorderStyle::Single, 0.5, None))), ..Default::default() },
        chr: CharProps { italic: Some(false), bold: Some(true), ..chr() },
        ..Default::default()
    });
    // Outline numbering linked to the heading styles.
    let levels: Vec<wordcraft_doc::numbering::Level> = (0..3)
        .map(|i| wordcraft_doc::numbering::Level {
            format: NumFormat::Decimal,
            text: match i {
                0 => "%1".into(),
                1 => "%1.%2".into(),
                _ => "%1.%2.%3".into(),
            },
            indent: 36.0 * (i as f32 + 1.0),
            hanging: 36.0,
            style: Some(format!("Heading{}", i + 1)),
            ..Default::default()
        })
        .collect();
    let num = d.numbering.try_add_levels(Some("Headings".into()), levels).unwrap();
    for (i, id) in ["Heading1", "Heading2", "Heading3"].iter().enumerate() {
        if let Some(st) = d.styles.styles.iter().find(|s| s.id == *id).cloned() {
            let mut st = st;
            st.para.numbering = Some(NumRef { num, level: i as u8 });
            d.styles.upsert(st);
        }
    }
    let mut b: Vec<Paragraph> = vec![
        styled("Numbered heading", "Heading1"),
        para(&words(2900, 40)),
        styled("Body Note style paragraph with a smaller italic face.", "BodyNote"),
    ];
    let mut code = para("Inline code: ");
    add(&mut code, "let x = 42;", CharProps { style: Some("CodeChar".into()), ..chr() });
    add(&mut code, " in a sentence.", chr());
    b.push(code);
    b.push(styled("Second-level numbered heading", "Heading2"));
    b.push(styled("Boxed, based on Body Note: bold, not italic.", "Boxed"));
    b.push(styled("Third level", "Heading3"));
    b.push(styled("Another second level", "Heading2"));
    b.push(styled("Another first level", "Heading1"));
    b.push(para(&words(2901, 40)));
    d.body = b.into_iter().map(para_block).collect();
    finish("custom_styles", d)
}

fn controls_hidden() -> Doc {
    let mut d = text_doc(&[
        "Content controls: a plain-text control, a rich-text control, a check box and a drop-down.",
        "Hidden text: this sentence is shown, this one is hidden, and this one is shown again.",
        &words(3000, 40),
    ]);
    stamp(&mut d, "Snapshot: controls_hidden");
    let mut s = session(d);
    select(&mut s, "plain-text control");
    run(&mut s, "developer.plainText", json!({"tag": "plain", "title": "Plain"}));
    select(&mut s, "rich-text control");
    run(&mut s, "developer.richText", json!({"tag": "rich"}));
    select(&mut s, "check box");
    run(&mut s, "select.collapse", json!({"end": true}));
    run(&mut s, "developer.checkBox", json!({}));
    select(&mut s, "drop-down");
    run(&mut s, "developer.dropDown", json!({"items": ["One", "Two", "Three"]}));
    select(&mut s, "this one is hidden,");
    run(&mut s, "format.hidden", json!({}));
    let mut doc = finish("controls_hidden", s.doc);
    doc.variants.push(("show_hidden", LayoutOptions { show_hidden: true, ..opts() }));
    doc
}

fn images_inline() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = vec![
        styled("Inline Pictures", "Heading1"),
        para("Small pictures in the text flow:"),
        para(&words(3100, 30)),
        para("A picture wider than the column:"),
        para(""),
    ];
    let mut lb = d.numbering.clone();
    let bullets = lb.add_list(ListKind::Bullet);
    d.numbering = lb;
    let mut li = styled("A list item with a picture ", "ListParagraph");
    li.props.numbering = Some(NumRef { num: bullets, level: 0 });
    b.push(li);
    b.push(para("A table with a picture in a cell:"));
    d.body = b.into_iter().map(para_block).collect();
    let m: Vec<String> = (0..5).map(|i| d.add_media(png(10 + i, 40 + 8 * i as u32, 30), "png")).collect();
    let p1 = d.para(StoryRef::Body, &wordcraft_doc::Path::top(1)).map_or(0, |p| p.len());
    d.insert_object(&Pos::body(1, p1), image(m[0].clone(), 40.0, 30.0, "Tiny", Float::default()), &chr()).unwrap();
    d.insert_object(&Pos::body(2, 20), image(m[1].clone(), 96.0, 60.0, "In the middle of a sentence", Float::default()), &chr()).unwrap();
    d.insert_object(&Pos::body(4, 0), image(m[2].clone(), 600.0, 300.0, "Wider than the column", Float::default()), &chr()).unwrap();
    let p5 = d.para(StoryRef::Body, &wordcraft_doc::Path::top(5)).map_or(0, |p| p.len());
    d.insert_object(&Pos::body(5, p5), image(m[3].clone(), 48.0, 36.0, "In a list", Float::default()), &chr()).unwrap();
    let mut t = table(2, 2, 468.0, 3150);
    let mut pic = para("");
    pic.insert_object(0, image(m[4].clone(), 120.0, 90.0, "In a cell", Float::default()), &chr()).unwrap();
    cell_blocks(&mut t, 0, 1, vec![para_block(pic)]);
    d.body.push(Arc::new(Block::Table(t)));
    let mut hd = styled("Header with a picture ", "Header");
    hd.insert_object(hd.len(), image(m[0].clone(), 24.0, 18.0, "Header picture", Float::default()), &chr()).unwrap();
    let hid = hf_part(&mut d, PartKind::Header, hd);
    d.last_section.headers.default = Some(hid);
    finish("images_inline", d)
}

fn shapes() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> =
        vec![styled("Shapes", "Heading1"), para("Inline shapes: "), para(&words(3200, 60)), para(&words(3201, 60)), para(&words(3202, 60))];
    d.body = b.drain(..).map(para_block).collect();
    let kinds = [
        (ShapeKind::Rectangle, Some(Rgb(0x44, 0x72, 0xC4)), Some(Rgb(0x2F, 0x52, 0x8F))),
        (ShapeKind::RoundedRectangle, Some(Rgb(0xED, 0x7D, 0x31)), None),
        (ShapeKind::Ellipse, None, Some(Rgb(0x70, 0xAD, 0x47))),
        (ShapeKind::Diamond, Some(Rgb(0xFF, 0xC0, 0x00)), Some(Rgb(0x7F, 0x60, 0x00))),
        (ShapeKind::Star, Some(Rgb(0xA5, 0xA5, 0xA5)), None),
        (ShapeKind::Heart, Some(Rgb(0xC0, 0x00, 0x00)), None),
        (ShapeKind::RightTriangle, Some(Rgb(0x5B, 0x9B, 0xD5)), None),
        (ShapeKind::Line, None, Some(Rgb(0, 0, 0))),
    ];
    for (k, f, st) in kinds {
        let at = d.para(StoryRef::Body, &wordcraft_doc::Path::top(1)).map_or(0, |p| p.len());
        d.insert_object(&Pos::body(1, at), shape(k, 36.0, 28.0, f, st, Float::default()), &chr()).unwrap();
        d.insert_text(&Pos::body(1, at + wordcraft_doc::para::OBJ.len_utf8()), " ", &chr()).unwrap();
    }
    let rot =
        Float { wrap: Wrap::Square, h_rel: Anchor::Margin, v_rel: Anchor::Paragraph, x: 320.0, y: 0.0, rot: 45.0, dist: 6.0, ..Default::default() };
    d.insert_object(
        &Pos::body(2, 0),
        shape(ShapeKind::Rectangle, 100.0, 50.0, Some(Rgb(0xDD, 0xEB, 0xF7)), Some(Rgb(0x1F, 0x4E, 0x79)), rot),
        &chr(),
    )
    .unwrap();
    let flipped =
        Float { wrap: Wrap::Square, h_rel: Anchor::Margin, v_rel: Anchor::Paragraph, x: 0.0, y: 0.0, flip_h: true, dist: 6.0, ..Default::default() };
    d.insert_object(&Pos::body(3, 0), shape(ShapeKind::RightTriangle, 80.0, 80.0, Some(Rgb(0xF4, 0xB1, 0x83)), None, flipped), &chr()).unwrap();
    let arrow = Float { wrap: Wrap::InFrontOfText, h_rel: Anchor::Margin, v_rel: Anchor::Paragraph, x: 200.0, y: 20.0, ..Default::default() };
    d.insert_object(&Pos::body(4, 0), shape(ShapeKind::Arrow, 120.0, 24.0, Some(Rgb(0x70, 0xAD, 0x47)), Some(Rgb(0x38, 0x56, 0x23)), arrow), &chr())
        .unwrap();
    finish("shapes", d)
}

fn table_floating() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = vec![styled("Floating and Styled Tables", "Heading1")];
    for i in 0..8 {
        b.push(para(&words(3300 + i, 60)));
    }
    d.body = b.into_iter().map(para_block).collect();
    let mut ft = table(3, 2, 200.0, 3400);
    ft.props.float = Some(TableFloat {
        h_rel: Anchor::Margin,
        v_rel: Anchor::Paragraph,
        x: 268.0,
        y: 0.0,
        h_align: None,
        v_align: None,
        dist: [6.0, 6.0, 6.0, 6.0],
        overlap: false,
    });
    ft.props.borders = Some(Borders::all(border(BorderStyle::Single, 0.5, None)));
    d.body.insert(2, Arc::new(Block::Table(ft)));
    let mut styled_t = table(4, 4, 468.0, 3500);
    styled_t.props.style = Some("GridTable4AccentBlue".into());
    styled_t.props.look.header_row = true;
    styled_t.props.look.banded_rows = true;
    styled_t.props.look.first_column = true;
    if let Some(row) = styled_t.rows.first_mut() {
        row.props.header = true;
    }
    d.body.insert(6, Arc::new(Block::Table(styled_t)));
    let mut rtl_t = table(2, 3, 468.0, 3600);
    rtl_t.props.bidi_visual = true;
    rtl_t.props.align = Some(Align::Center);
    d.body.push(Arc::new(Block::Table(rtl_t)));
    d.body.push(para_block(para("After the tables.")));
    finish("table_floating", d)
}

fn long_prose() -> Doc {
    let mut d = Document::new();
    let mut b: Vec<Paragraph> = Vec::new();
    for i in 0..60 {
        if i % 20 == 0 {
            b.push(styled(&format!("Chapter {}", i / 20 + 1), "Heading1"));
        } else if i % 5 == 0 {
            b.push(styled(&format!("Section {}.{}", i / 20 + 1, i % 20 / 5), "Heading2"));
        }
        b.push(para(&words(4000 + i, 70)));
    }
    d.body = b.into_iter().map(para_block).collect();
    let mut f = Paragraph::new().styled("Footer");
    f.props.align = Some(Align::Center);
    f.insert_object(0, field("PAGE", "1"), &chr()).unwrap();
    let fid = hf_part(&mut d, PartKind::Footer, f);
    d.last_section.footers.default = Some(fid);
    finish("long_prose", d)
}

fn kitchen_sink() -> Doc {
    let mut d = text_doc(&[
        "Kitchen sink: a bit of everything the sweep document has.",
        &words(5000, 50),
        "Body text with a bookmark, a comment, hidden words and tracked words.",
        &words(5001, 50),
        &words(5002, 50),
        "Last paragraph of the first section.",
        "Second section, two columns, with a footnote and a content control here.",
        &words(5003, 50),
        &words(5004, 50),
        &words(5005, 50),
        "The end of the second section.",
    ]);
    stamp(&mut d, "Snapshot: kitchen_sink");
    let mut hd = styled("Header: page ", "Header");
    hd.insert_object(hd.len(), field("PAGE", "1"), &chr()).unwrap();
    add(&mut hd, " of ", chr());
    hd.insert_object(hd.len(), field("NUMPAGES", "1"), &chr()).unwrap();
    let hid = hf_part(&mut d, PartKind::Header, hd);
    d.last_section.headers.default = Some(hid);
    let mut s1 = section_props(SectionStart::NextPage);
    s1.headers.default = Some(hid);
    if let Some(Block::Para(p)) = d.body.get_mut(5).map(Arc::make_mut) {
        p.section = Some(Box::new(s1));
    }
    d.last_section.columns = Columns { count: 2, space: 24.0, separator: false, widths: Vec::new() };
    d.last_section.start = SectionStart::Continuous;
    let mut t = table(3, 3, 468.0, 5100);
    if let Some(row) = t.rows.first_mut() {
        row.props.header = true;
    }
    d.body.insert(4, Arc::new(Block::Table(t)));
    let mut s = session(d);
    select(&mut s, "a bookmark");
    run(&mut s, "insert.bookmark", json!({"name": "sink"}));
    select(&mut s, "a comment");
    run(&mut s, "review.newComment", json!({"text": "A remark in the kitchen sink."}));
    select(&mut s, "hidden words");
    run(&mut s, "format.hidden", json!({}));
    run(&mut s, "review.trackChanges", json!({"value": true}));
    select(&mut s, "tracked words");
    run(&mut s, "text.delete", json!({}));
    run(&mut s, "text.insert", json!({"text": "words tracked"}));
    run(&mut s, "review.trackChanges", json!({"value": false}));
    select(&mut s, "a footnote");
    run(&mut s, "select.collapse", json!({"end": true}));
    run(&mut s, "references.footnote", json!({"text": "The kitchen sink's footnote."}));
    // (The caret is in the new footnote; `select.text` searches the caret's story.)
    caret(&mut s, 0, 0);
    select(&mut s, "a content control");
    run(&mut s, "developer.plainText", json!({"tag": "sink"}));
    select(&mut s, "Last paragraph of the first section.");
    run(&mut s, "select.collapse", json!({"end": true}));
    run(&mut s, "text.insert", json!({"text": " See "}));
    run(&mut s, "insert.field", json!({"instr": "REF sink \\h"}));
    run(&mut s, "text.insert", json!({"text": " on page "}));
    run(&mut s, "insert.field", json!({"instr": "PAGEREF sink \\h"}));
    run(&mut s, "text.insert", json!({"text": "."}));
    run(&mut s, "references.updateFields", json!({}));
    let mut doc = finish("kitchen_sink", s.doc);
    doc.variants.push(("no_markup_hidden", LayoutOptions { hide_deleted: true, show_hidden: true, ..opts() }));
    doc
}

/// The letter template with its date paragraph fixed: the template dates itself from the wall
/// clock (`sample::letter`, block 3), which a golden can't hold.
fn letter_dated() -> Document {
    let mut d = sample::letter();
    if let Some(b) = d.body.get_mut(3) {
        *b = para_block(para("November 14, 2023"));
    }
    d
}

fn builtin(name: &'static str, d: Document, render: &'static [usize]) -> Doc {
    let mut doc = finish(name, d);
    doc.render = render;
    doc
}

/// The corpus, in a fixed order.
pub fn corpus() -> Vec<Doc> {
    vec![
        blank(),
        styles_text(),
        lists(),
        table_fixed(),
        table_autofit(),
        table_merged(),
        table_nested(),
        table_split(),
        table_floating(),
        sections(),
        columns(),
        headers_footers(),
        footnotes(),
        endnotes(),
        text_boxes(),
        floats(),
        images_inline(),
        shapes(),
        fields_toc(),
        tracked_changes(),
        comments(),
        rtl(),
        vertical(),
        east_asian(),
        hyphenation(),
        breaks_keeps(),
        drop_cap_tabs(),
        page_setup(),
        custom_styles(),
        controls_hidden(),
        long_prose(),
        kitchen_sink(),
        builtin("sample", sample::sample_document(), &[0]),
        builtin("letter", letter_dated(), &[]),
        builtin("resume", sample::resume(), &[]),
        builtin("report", sample::report(), &[]),
    ]
}

// ---------------------------------------------------------------------------------------------
// Command scripts

/// About fifty editing commands on the sample document: text, formatting, paragraphs, lists,
/// clipboard, find and replace, a table, notes, a bookmark, tracked changes, a comment, a
/// style change, a table of contents, page setup, undo and redo.
fn handbook_script() -> Script {
    let mut start = sample::sample_document();
    pin_fonts(&mut start);
    stamp(&mut start, "Snapshot script: handbook");
    let steps = vec![
        ("caret.docStart", json!({})),
        ("caret.down", json!({})),
        ("caret.end", json!({})),
        ("text.insert", json!({"text": " Typed at the end of the subtitle."})),
        ("text.newParagraph", json!({})),
        ("text.insert", json!({"text": "A new paragraph, then a tab\tand a line break"})),
        ("text.lineBreak", json!({})),
        ("text.insert", json!({"text": "after it."})),
        ("text.backspace", json!({})),
        ("text.deleteWordBack", json!({})),
        ("select.text", json!({"text": "Painters"})),
        ("format.italic", json!({})),
        ("format.underline", json!({})),
        ("format.size", json!({"size": 14})),
        ("format.color", json!({"color": "1F4E79"})),
        ("format.highlight", json!({"color": "yellow"})),
        ("select.text", json!({"text": "Rent is the obvious reason"})),
        ("select.paragraph", json!({})),
        ("para.justify", json!({})),
        ("para.lineSpacing", json!({"value": 1.5})),
        ("para.spacing", json!({"before": 6, "after": 12})),
        ("para.indent", json!({})),
        ("select.text", json!({"text": "Clean as you go."})),
        ("para.numbering", json!({})),
        ("para.listLevel", json!({"level": 1})),
        ("select.text", json!({"text": "Switch on the main breaker"})),
        ("select.paragraph", json!({})),
        ("edit.copy", json!({})),
        ("caret.docEnd", json!({})),
        ("text.newParagraph", json!({})),
        ("edit.paste", json!({})),
        ("edit.undo", json!({})),
        ("edit.redo", json!({})),
        ("edit.replaceAll", json!({"text": "studio", "with": "workshop"})),
        ("edit.find", json!({"text": "kiln"})),
        ("caret.docEnd", json!({})),
        ("text.newParagraph", json!({})),
        ("insert.table", json!({"rows": 2, "cols": 3})),
        ("text.insert", json!({"text": "cell one"})),
        ("text.tab", json!({})),
        ("text.insert", json!({"text": "cell two"})),
        ("table.insertRowBelow", json!({})),
        ("table.insertColumnRight", json!({})),
        ("table.selectRow", json!({})),
        ("table.merge", json!({})),
        ("table.deleteRow", json!({})),
        ("select.text", json!({"text": "glaze chemistry"})),
        ("references.footnote", json!({"text": "A footnote on glaze chemistry."})),
        ("text.insert", json!({"text": " Typed into the footnote."})),
        ("caret.set", json!({"pos": {"story": "body", "path": [0], "off": 0}})),
        ("select.text", json!({"text": "cross-talk"})),
        ("insert.bookmark", json!({"name": "crosstalk"})),
        ("insert.link", json!({"url": "https://example.org/crosstalk"})),
        ("review.trackChanges", json!({"value": true})),
        ("select.text", json!({"text": "bread"})),
        ("text.insert", json!({"text": "cake"})),
        ("select.text", json!({"text": "Thursday"})),
        ("format.bold", json!({})),
        ("review.trackChanges", json!({"value": false})),
        ("select.text", json!({"text": "Quiet hours"})),
        ("review.newComment", json!({"text": "Are the hours still right?"})),
        ("styles.modify", json!({"style": "Heading1", "chr": {"size": 22, "color": "C00000"}})),
        ("caret.docStart", json!({})),
        ("references.toc", json!({"levels": 2})),
        ("layout.margins", json!({"top": 54, "bottom": 54, "left": 54, "right": 54})),
        ("layout.columns", json!({"count": 2})),
        ("edit.undo", json!({})),
        ("edit.undo", json!({})),
        ("edit.undo", json!({})),
        ("edit.redo", json!({})),
        ("select.all", json!({})),
        ("format.size", json!({"size": 11})),
        ("edit.undo", json!({})),
    ];
    Script { name: "handbook", start, steps }
}

/// About fifty commands centred on tables and lists, on a document built for them: typing in
/// cells, row and column edits, widths and heights, alignment, shading, borders, splitting,
/// text to table and back, list levels and restarts, case changes, a text box and a shape.
fn tables_lists_script() -> Script {
    let mut d = Document::new();
    let num = d.numbering.add_list(ListKind::Numbered);
    let mut b: Vec<Paragraph> = vec![styled("Tables and Lists", "Heading1"), para("Before the table.")];
    let mut t = table(3, 3, 468.0, 6000);
    if let Some(row) = t.rows.first_mut() {
        row.props.header = true;
    }
    for text in ["First item", "Second item", "Third item"] {
        let mut p = styled(text, "ListParagraph");
        p.props.numbering = Some(NumRef { num, level: 0 });
        b.push(p);
    }
    b.push(para("alpha, beta, gamma"));
    b.push(para("one, two, three"));
    b.push(para("The end."));
    d.body = b.into_iter().map(para_block).collect();
    d.body.insert(2, Arc::new(Block::Table(t)));
    pin_fonts(&mut d);
    stamp(&mut d, "Snapshot script: tables_lists");
    let steps = vec![
        ("caret.set", json!({"pos": {"story": "body", "path": [2, 0, 0, 0], "off": 0}})),
        ("text.insert", json!({"text": "Typed in the header cell. "})),
        ("text.tab", json!({})),
        ("text.insert", json!({"text": "Averyveryverylongwordinthesecondcell"})),
        ("text.tab", json!({})),
        ("text.tab", json!({})),
        ("text.insert", json!({"text": "second row"})),
        ("table.insertRowAbove", json!({})),
        ("table.insertColumnLeft", json!({})),
        ("table.columnWidth", json!({"width": 60})),
        ("table.rowHeight", json!({"height": 36, "rule": "exact"})),
        ("table.cellAlign", json!({"value": "centerRight"})),
        ("table.shading", json!({"color": "DDEBF7"})),
        ("table.borders", json!({"kind": "all"})),
        ("table.selectRow", json!({})),
        ("format.bold", json!({})),
        ("table.selectColumn", json!({})),
        ("format.italic", json!({})),
        ("table.selectTable", json!({})),
        ("format.size", json!({"size": 10})),
        ("caret.set", json!({"pos": {"story": "body", "path": [2, 1, 1, 0], "off": 0}})),
        ("table.split", json!({})),
        ("edit.undo", json!({})),
        ("table.splitTable", json!({})),
        ("edit.undo", json!({})),
        ("table.autofit", json!({"mode": "contents"})),
        ("table.autofit", json!({"mode": "window"})),
        ("table.distributeColumns", json!({})),
        ("table.textDirection", json!({})),
        ("table.deleteColumn", json!({})),
        ("table.repeatHeader", json!({})),
        ("select.text", json!({"text": "alpha, beta, gamma"})),
        ("table.fromText", json!({"separator": "comma"})),
        ("select.text", json!({"text": "one, two, three"})),
        ("table.fromText", json!({"separator": "comma"})),
        ("table.toText", json!({"separator": "tab"})),
        ("select.text", json!({"text": "Second item"})),
        ("para.demote", json!({})),
        ("select.text", json!({"text": "Third item"})),
        ("para.listLevel", json!({"level": 2})),
        ("para.promote", json!({})),
        ("para.restartNumbering", json!({})),
        ("select.text", json!({"text": "First item"})),
        ("para.bullets", json!({})),
        ("para.bullets", json!({})),
        ("select.text", json!({"text": "The end."})),
        ("format.changeCase", json!({"mode": "upper"})),
        ("format.changeCase", json!({"mode": "title"})),
        ("select.collapse", json!({"end": true})),
        ("text.newParagraph", json!({})),
        ("insert.textBox", json!({"text": "A box typed by the script", "width": 120, "height": 40})),
        ("text.insert", json!({"text": " and typed into"})),
        ("caret.set", json!({"pos": {"story": "body", "path": [9], "off": 0}})),
        ("caret.end", json!({})),
        ("insert.shape", json!({"kind": "ellipse", "width": 50, "height": 30, "fill": "FFC000"})),
        ("text.insert", json!({"text": "teh "})),
        ("text.insert", json!({"text": "(c) "})),
        ("select.all", json!({})),
        ("edit.copy", json!({})),
        ("caret.docEnd", json!({})),
        ("edit.paste", json!({})),
        ("edit.undo", json!({})),
        ("edit.undo", json!({})),
        ("edit.redo", json!({})),
        ("caret.docEnd", json!({})),
        ("edit.pasteText", json!({})),
        ("caret.set", json!({"pos": {"story": "body", "path": [2, 0, 0, 0], "off": 0}})),
        ("table.deleteTable", json!({})),
        ("edit.undo", json!({})),
    ];
    Script { name: "tables_lists", start: d, steps }
}

pub fn scripts() -> Vec<Script> {
    vec![handbook_script(), tables_lists_script()]
}
