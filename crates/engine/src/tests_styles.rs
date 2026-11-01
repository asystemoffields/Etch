//! Styles, sections and headers and footers: Word's rules for style precedence and toggle
//! properties, deleting a section break, link to previous, and random sequences of style and
//! section commands mixed with typing.

use serde_json::{Value, json};
use wordcraft_doc::section::{HeaderSet, SectionStart};
use wordcraft_doc::{Block, Blocks, Document, PartKind, Pos, SectionProps, StoryRef};

use crate::Session;

fn run(s: &mut Session, id: &str, v: Value) -> Value {
    s.run(id, &v).unwrap_or_else(|e| panic!("{id} {v}: {e}"))
}

fn session(text: &str) -> Session {
    let mut s = Session::new(Document::from_text(text));
    s.reset_history();
    s
}

fn serialized(d: &Document) -> String {
    serde_json::to_string(d).unwrap()
}

/// (Last block, landscape, columns) of each section.
fn sections(s: &Session) -> Vec<(usize, bool, u32)> {
    s.doc.sections().iter().map(|(e, x)| (*e, x.landscape, x.columns.count)).collect()
}

fn texts(s: &Session) -> Vec<String> {
    s.doc.body.iter().filter_map(|b| b.as_para().map(|p| p.text.clone())).collect()
}

/// "Alpha" | section break | "Beta", "Gamma": section 2 landscape in two columns.
fn two_sections() -> Session {
    let mut s = session("Alpha\nBeta\nGamma");
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 5}}));
    run(&mut s, "layout.break", json!({"kind": "nextPage"}));
    // The break made an empty paragraph starting section 2: take it out again.
    run(&mut s, "text.delete", json!({}));
    assert_eq!(texts(&s), ["Alpha", "Beta", "Gamma"]);
    run(&mut s, "caret.set", json!({"pos": {"block": 1, "off": 0}}));
    run(&mut s, "layout.orientation", json!({"value": "landscape"}));
    run(&mut s, "layout.columns", json!({"count": 2}));
    assert_eq!(sections(&s), [(0, false, 1), (2, true, 2)]);
    s.reset_history();
    s
}

/// Deleting a section break joins its section to the next one, which keeps its own setup: the
/// text before the break takes the formatting of the section after it, as in Word (#2127: the
/// joined paragraph kept the first paragraph's section, so the break only moved down).
#[test]
fn deleting_a_section_break_joins_the_section_after_it() {
    for how in ["backspace", "delete", "selection"] {
        let mut s = two_sections();
        let before = serialized(&s.doc);
        match how {
            "backspace" => {
                run(&mut s, "caret.set", json!({"pos": {"block": 1, "off": 0}}));
                run(&mut s, "text.backspace", json!({}));
            }
            "delete" => {
                run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 5}}));
                run(&mut s, "text.delete", json!({}));
            }
            _ => {
                run(&mut s, "select.range", json!({"anchor": {"block": 0, "off": 3}, "focus": {"block": 1, "off": 2}}));
                run(&mut s, "text.backspace", json!({}));
            }
        }
        assert_eq!(sections(&s), [(1, true, 2)], "{how}: one section, the second's setup");
        assert_eq!(s.doc.body.len(), 2, "{how}");
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(serialized(&s.doc), before, "{how}: undo brings the break back");
    }
}

/// A section break deleted across several: the section after them keeps its setup and the
/// headers and footers it linked to (showed from) the deleted ones.
#[test]
fn deleting_section_breaks_keeps_the_headers_the_next_section_showed() {
    let mut s = session("One\nTwo\nThree\nFour");
    let mut d = s.doc.clone();
    let (h1, h2, f2, h3) = (
        d.add_part(PartKind::Header, Default::default()),
        d.add_part(PartKind::Header, Default::default()),
        d.add_part(PartKind::Footer, Default::default()),
        d.add_part(PartKind::Header, Default::default()),
    );
    let sect = |headers: HeaderSet, footers: HeaderSet| Box::new(SectionProps { headers, footers, ..SectionProps::default() });
    let only = |id: u32| HeaderSet { default: Some(id), ..Default::default() };
    // Sections: [One] header h1, [Two] header h2 + first-page footer f2, [Three, Four] header h3
    // first-page only (links the default header and the footers).
    d.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap().section = Some(sect(only(h1), HeaderSet::default()));
    d.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(1)).unwrap().section =
        Some(sect(only(h2), HeaderSet { first: Some(f2), ..Default::default() }));
    d.last_section =
        SectionProps { headers: HeaderSet { first: Some(h3), ..Default::default() }, start: SectionStart::OddPage, ..SectionProps::default() };
    s = Session::new(d);
    // From inside "One" to inside "Three": both breaks go.
    run(&mut s, "select.range", json!({"anchor": {"block": 0, "off": 1}, "focus": {"block": 2, "off": 1}}));
    run(&mut s, "text.delete", json!({}));
    let all = s.doc.sections();
    assert_eq!(all.len(), 1);
    let last = all[0].1;
    assert_eq!(last.start, SectionStart::OddPage, "the last section's own setup");
    assert_eq!(last.headers, HeaderSet { default: Some(h2), first: Some(h3), even: None }, "the nearest deleted section's header");
    assert_eq!(last.footers.first, Some(f2));
    assert_eq!(texts(&s), ["Ohree", "Four"]);
}

/// A section break at the very end (a file's last paragraph ending a section): Delete there
/// takes the break away and keeps the text (#2127's sample).
#[test]
fn delete_at_the_end_removes_a_last_paragraph_section_break() {
    let mut d = Document::from_text("Only paragraph");
    let h = d.add_part(PartKind::Header, Default::default());
    d.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap().section =
        Some(Box::new(SectionProps { headers: HeaderSet { default: Some(h), ..Default::default() }, landscape: true, ..SectionProps::default() }));
    let mut s = Session::new(d);
    let before = serialized(&s.doc);
    assert_eq!(s.doc.sections().len(), 2);
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "text.delete", json!({}));
    assert_eq!(s.doc.sections().len(), 1);
    assert!(!s.doc.last_section.landscape, "the last section's setup");
    assert_eq!(s.doc.last_section.headers.default, Some(h), "the header it showed stays");
    assert_eq!(texts(&s), ["Only paragraph"]);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(serialized(&s.doc), before);
}

/// Link to Previous works on the header being edited, in the section of the page it is edited
/// on, for that kind of header only (first page, even pages or the default one), and switches:
/// unlinking gives the section a copy of what it showed.
#[test]
fn link_to_previous_switches_the_edited_headers_section_and_kind() {
    let mut s = session("One\nTwo");
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 3}}));
    run(&mut s, "layout.break", json!({"kind": "nextPage"}));
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 0}}));
    run(&mut s, "insert.header", json!({"text": "First section"}));
    let h1 = s.doc.sections()[0].1.headers.default.unwrap();
    // Section 2 links to it: page 2 shows it.
    let layout = s.layout();
    assert_eq!(layout.pages.iter().map(|p| p.header_story).collect::<Vec<_>>(), [Some(h1), Some(h1)]);
    // Editing that header on page 2 and unlinking: section 2 gets a copy, the caret in it.
    s.page_hint = 1;
    s.sel = crate::Selection::caret(s.doc.end_of(StoryRef::Part(h1)));
    let out = run(&mut s, "layout.linkToPrevious", json!({}));
    assert_eq!((out["linked"].as_bool(), out["section"].as_u64(), out["slot"].as_str()), (Some(false), Some(1), Some("default")), "{out}");
    let h2 = s.doc.sections()[1].1.headers.default.expect("section 2 has its own now");
    assert_ne!(h2, h1);
    assert_eq!(s.doc.sections()[0].1.headers.default, Some(h1), "section 1 keeps its own");
    assert_eq!(s.sel.focus.story, StoryRef::Part(h2), "the caret is in the copy");
    assert_eq!(s.doc.plain_text(StoryRef::Part(h2)), "First section");
    run(&mut s, "text.insert", json!({"text": " (2)"}));
    assert_eq!(s.doc.plain_text(StoryRef::Part(h1)), "First section", "editing the copy leaves section 1's");
    // Linking again drops the copy and shows section 1's; the footers were left alone.
    let out = run(&mut s, "layout.linkToPrevious", json!({}));
    assert_eq!(out["linked"].as_bool(), Some(true));
    assert_eq!(s.doc.sections()[1].1.headers.default, None);
    assert!(!s.doc.parts.contains_key(&h2), "the dropped copy is gone");
    assert_eq!(s.sel.focus.story, StoryRef::Part(h1));
    // Linking what is linked changes nothing; the first section has nothing to link to.
    let undo = s.undo_label().map(str::to_string);
    run(&mut s, "layout.linkToPrevious", json!({"value": true}));
    assert_eq!(s.undo_label().map(str::to_string), undo, "no undo step for no change");
    s.page_hint = 0;
    assert!(s.run("layout.linkToPrevious", &json!({})).is_err());
    // A first-page header of section 2 links on its own: the default one stays its own.
    let mut d = s.doc.clone();
    let first = d.add_part(PartKind::Header, Default::default());
    let own = d.add_part(PartKind::Header, Default::default());
    let sect = d.section_mut(1);
    sect.headers = HeaderSet { default: Some(own), first: Some(first), even: None };
    sect.title_page = true;
    let mut s = Session::new(d);
    s.sel = crate::Selection::caret(s.doc.start_of(StoryRef::Part(first)));
    s.page_hint = 1;
    let out = run(&mut s, "layout.linkToPrevious", json!({}));
    assert_eq!(out["slot"].as_str(), Some("first"), "{out}");
    assert_eq!(s.doc.sections()[1].1.headers, HeaderSet { default: Some(own), first: None, even: None });
}

/// Text pasted into a document without the styles it uses brings them along, as in Word: cut a
/// paragraph in a custom style, delete the style, paste (found by the random test below: the
/// pasted paragraph named a style that didn't exist). A style the document has keeps its own
/// definition.
#[test]
fn pasted_text_brings_the_styles_it_uses() {
    let mut s = session("One\nTwo\nThree\nPlain");
    run(&mut s, "caret.set", json!({"pos": {"block": 1, "off": 0}}));
    run(&mut s, "styles.create", json!({"name": "Mine", "basedOn": "Quote"}));
    run(&mut s, "styles.modify", json!({"style": "Quote", "chr": {"size": 20}}));
    run(&mut s, "select.range", json!({"anchor": {"block": 0, "off": 0}, "focus": {"block": 3, "off": 0}}));
    run(&mut s, "edit.cut", json!({}));
    run(&mut s, "styles.delete", json!({"style": "Mine"}));
    assert!(s.doc.styles.get("Mine").is_none());
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "edit.paste", json!({}));
    let mine = s.doc.styles.get("Mine").expect("the style came back with the text");
    assert_eq!(mine.based_on.as_deref(), Some("Quote"));
    assert_eq!(texts(&s), ["PlainOne", "Two", "Three", ""]);
    assert_eq!(s.doc.para_at(&Pos::body(1, 0)).and_then(|p| p.props.style.as_deref()), Some("Mine"));
    // Into another document: what it lacks comes along (Mine), what it has stays its own (Quote).
    let frag = s.doc.copy_range(&Pos::body(1, 0), &Pos::body(2, 0));
    let mut other = Document::from_text("Elsewhere");
    other.insert_fragment(&Pos::body(0, 0), &frag).unwrap();
    assert!(other.styles.get("Mine").is_some());
    assert_eq!(other.styles.get("Quote").and_then(|q| q.chr.size), None, "the destination's Quote is kept");
    assert_eq!(other.styles.resolve_char(Some("Mine"), &Default::default()).size, 12.0);
}

/// Enter at the end of a paragraph whose style names a "style for following paragraph" the
/// document doesn't have (a file can) keeps the paragraph's style instead of naming a style that
/// isn't there.
#[test]
fn enter_keeps_the_style_when_the_next_style_is_missing() {
    let mut d = Document::from_text("Heading text");
    if let Some(h) = wordcraft_doc::Shared::make_mut(&mut d.styles).get_mut("Heading1") {
        h.next = Some("NoSuchStyle".into());
    }
    d.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap().props.style = Some("Heading1".into());
    let mut s = Session::new(d);
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "text.newParagraph", json!({}));
    assert_eq!(s.doc.para_at(&Pos::body(1, 0)).and_then(|p| p.props.style.as_deref()), Some("Heading1"));
}

/// Columns with text selected put the selection in a section of its own, as in Word (#2024: the
/// whole section got the columns): a continuous section break before and after it, the text
/// around keeping its columns; a selection that starts and ends sections already needs none.
#[test]
fn columns_on_selected_text_go_in_a_section_of_their_own() {
    let mut s = session("Before\nMiddle one\nMiddle two\nAfter");
    let before = serialized(&s.doc);
    run(&mut s, "select.range", json!({"anchor": {"block": 1, "off": 0}, "focus": {"block": 3, "off": 0}}));
    run(&mut s, "layout.columns", json!({"count": 2}));
    let starts: Vec<(usize, u32, SectionStart)> = s.doc.sections().iter().map(|(e, x)| (*e, x.columns.count, x.start)).collect();
    assert_eq!(starts, [(1, 1, SectionStart::NextPage), (4, 2, SectionStart::Continuous), (5, 1, SectionStart::Continuous)]);
    assert_eq!(texts(&s), ["Before", "", "Middle one", "Middle two", "", "After"], "each break is a paragraph mark of its own");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(serialized(&s.doc), before, "one undo step");
    // The whole document selected: no breaks, the one section in columns.
    run(&mut s, "select.all", json!({}));
    run(&mut s, "layout.columns", json!({"count": 3}));
    assert_eq!(sections(&s), [(3, false, 3)]);
    // A caret: the caret's section, as before.
    run(&mut s, "caret.set", json!({"pos": {"block": 1, "off": 2}}));
    run(&mut s, "layout.columns", json!({"count": 1}));
    assert_eq!(sections(&s), [(3, false, 1)]);
}

/// Update Style to Match for a character style follows the toggle rule: in an italic Quote, a
/// run in the style made upright directly makes the style turn italic on, so it flips the
/// paragraph's italic off (setting it off would leave the paragraph's italic, and the other runs
/// in the style would stay italic).
#[test]
fn updating_a_character_style_to_match_turns_toggles_on_where_the_look_differs() {
    let mut d = Document::from_text("First second");
    wordcraft_doc::Shared::make_mut(&mut d.styles).styles.push(wordcraft_doc::styles::Style {
        id: "Mine".into(),
        name: "Mine".into(),
        kind: wordcraft_doc::StyleKind::Character,
        ..Default::default()
    });
    let p = d.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap();
    p.props.style = Some("Quote".into());
    p.format(0, 12, &|c| c.style = Some("Mine".into())).unwrap();
    p.format(0, 5, &|c| c.italic = Some(false)).unwrap();
    let mut s = Session::new(d);
    let looks = |s: &Session, off: usize| {
        let p = s.doc.para_at(&Pos::body(0, off)).unwrap();
        s.doc.styles.resolve_char(p.props.style.as_deref(), p.props_of_char(off)).italic
    };
    assert!(!looks(&s, 0) && looks(&s, 7), "the first word upright, the second italic");
    run(&mut s, "select.range", json!({"anchor": {"block": 0, "off": 0}, "focus": {"block": 0, "off": 5}}));
    run(&mut s, "styles.updateToMatch", json!({"style": "Mine"}));
    assert_eq!(s.doc.styles.get("Mine").and_then(|m| m.chr.italic), Some(true), "the style turns italic on");
    assert!(!looks(&s, 7), "and every run in it in the Quote is upright");
    assert!(s.doc.styles.resolve_char(Some("Normal"), &wordcraft_doc::CharProps { style: Some("Mine".into()), ..Default::default() }).italic);
}

// ---- Random style and section commands mixed with typing ----

/// A small deterministic generator (xorshift), so a failing seed can be replayed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, v: &[&'a str]) -> &'a str {
        v[self.below(v.len())]
    }
}

const PARA_STYLES: &[&str] = &["Normal", "Heading1", "Heading2", "Heading3", "Quote", "Title", "ListParagraph", "NoSpacing", "Mine", "Mine2"];
const CHAR_STYLES: &[&str] = &["Strong", "Emphasis", "BookTitle", "SubtleReference", "IntenseEmphasis"];

fn random_chr(r: &mut Rng) -> Value {
    match r.below(7) {
        0 => json!({"bold": r.below(2) == 0}),
        1 => json!({"italic": r.below(2) == 0}),
        2 => json!({"size": 8 + r.below(30)}),
        3 => json!({"color": "C00000"}),
        4 => json!({"smallCaps": true}),
        5 => json!({"font": "Liberation Serif"}),
        _ => json!({"hidden": r.below(4) == 0}),
    }
}

fn random_para(r: &mut Rng) -> Value {
    match r.below(7) {
        0 => json!({"spaceAfter": r.below(30)}),
        1 => json!({"spaceBefore": r.below(30)}),
        2 => json!({"keepNext": r.below(2) == 0}),
        3 => json!({"keepLines": r.below(2) == 0}),
        4 => json!({"pageBreakBefore": r.below(3) == 0}),
        5 => json!({"align": r.pick(&["left", "center", "right", "justify"])}),
        _ => json!({"indentLeft": r.below(72)}),
    }
}

/// One random style, section, header or typing command.
fn random_command(r: &mut Rng) -> (&'static str, Value) {
    let ext = json!({"extend": r.below(3) == 0});
    match r.below(48) {
        // Latin text only: the faces other scripts fall back to depend on the fonts loaded so far
        // (a layout cache matter of its own).
        0..=5 => ("text.insert", json!({"text": r.pick(&["a", "word ", "Text. ", "Fine. "])})),
        6 | 7 => ("text.newParagraph", json!({})),
        8 => ("text.backspace", json!({})),
        9 => ("text.delete", json!({})),
        10 => (["caret.left", "caret.right", "caret.up", "caret.down"][r.below(4)], ext),
        11 => (["caret.home", "caret.end", "caret.docStart", "caret.docEnd", "caret.paraUp", "caret.paraDown"][r.below(6)], ext),
        12 => (["select.word", "select.paragraph", "select.all"][r.below(3)], json!({})),
        13 | 14 => ("para.style", json!({"style": r.pick(PARA_STYLES)})),
        15 => ("format.charStyle", json!({"style": r.pick(CHAR_STYLES)})),
        16 => ("styles.modify", json!({"style": r.pick(PARA_STYLES), "chr": random_chr(r)})),
        17 => ("styles.modify", json!({"style": r.pick(PARA_STYLES), "para": random_para(r)})),
        18 => ("styles.modify", json!({"style": r.pick(CHAR_STYLES), "chr": random_chr(r)})),
        19 => ("styles.create", json!({"name": r.pick(&["Mine", "Mine2"]), "basedOn": r.pick(PARA_STYLES)})),
        20 => ("styles.updateToMatch", json!({})),
        21 => ("styles.delete", json!({"style": r.pick(&["Mine", "Mine2"])})),
        22 => (["format.clear", "format.resetChar", "para.reset"][r.below(3)], json!({})),
        23 => (["format.bold", "format.italic", "format.smallCaps"][r.below(3)], json!({})),
        24 => ("format.size", json!({"size": 8 + r.below(30)})),
        25 | 26 => ("layout.break", json!({"kind": r.pick(&["nextPage", "continuous", "evenPage", "oddPage", "page", "column"])})),
        27 => ("layout.orientation", json!({"value": r.pick(&["landscape", "portrait"])})),
        28 => ("layout.margins", json!({"preset": r.pick(&["normal", "narrow", "wide", "moderate"])})),
        29 => ("layout.columns", json!({"count": 1 + r.below(3)})),
        30 => ("layout.differentFirstPage", json!({})),
        31 => ("layout.differentOddEven", json!({})),
        32 => (if r.below(2) == 0 { "insert.header" } else { "insert.footer" }, json!({"text": r.pick(&["Head", "Foot", "Page"])})),
        33 => ("layout.linkToPrevious", json!({"footer": r.below(2) == 0})),
        34 => ("insert.pageNumber", json!({"position": r.pick(&["top", "bottom"])})),
        35 => (["para.keepNext", "para.keepLines", "para.widowControl", "para.pageBreakBefore"][r.below(4)], json!({})),
        36 => ("layout.lineNumbers", json!({"value": r.pick(&["continuous", "restartPage", "restartSection", "none"])})),
        37 => ("layout.size", json!({"name": r.pick(&["Letter", "A4", "A5"])})),
        38 | 39 => ("edit.undo", json!({})),
        40 => ("edit.redo", json!({})),
        41 => ("edit.cut", json!({})),
        42 => ("edit.paste", json!({})),
        43 => ("styles.setVisibility", json!({"style": r.pick(PARA_STYLES), "gallery": r.below(2) == 0})),
        44 => ("hf.next", json!({})),
        45 => ("design.styleSet", json!({"name": r.pick(&["default", "basic", "lines", "shaded", "casual", "centered", "minimalist", "title"])})),
        46 => ("para.heading1", json!({})),
        _ => ("text.pageBreak", json!({})),
    }
}

/// Every paragraph and character style a story names exists.
fn check_style_ids(d: &Document, blocks: &Blocks, what: &str) {
    for b in blocks {
        match &**b {
            Block::Para(p) => {
                if let Some(st) = &p.props.style {
                    assert!(d.styles.get(st).is_some(), "{what}: paragraph style {st:?} doesn't exist ({:?})", p.text);
                }
                for c in p.runs.iter().map(|r| &r.props).chain([&p.mark]) {
                    if let Some(st) = &c.style {
                        assert!(d.styles.get(st).is_some(), "{what}: character style {st:?} doesn't exist ({:?})", p.text);
                    }
                }
            }
            Block::Table(t) => {
                for c in t.rows.iter().flat_map(|r| &r.cells) {
                    check_style_ids(d, &c.blocks, what);
                }
            }
        }
    }
}

/// Every section's setup is usable: finite sizes in Word's range, margins that leave room for
/// text, a column count Word allows, headers and footers that exist and are of their kind; and
/// only top-level body paragraphs end sections.
fn check_sections(d: &Document, what: &str) {
    for (i, (end, x)) in d.sections().iter().enumerate() {
        let sizes = [x.page_w, x.page_h, x.margin_top, x.margin_bottom, x.margin_left, x.margin_right, x.header, x.footer, x.gutter];
        assert!(sizes.iter().all(|v| v.is_finite() && *v >= 0.0), "{what}: section {i} sizes {sizes:?}");
        assert!((7.2..=1584.0).contains(&x.page_w) && (7.2..=1584.0).contains(&x.page_h), "{what}: section {i} page {}x{}", x.page_w, x.page_h);
        assert!(x.margin_left + x.margin_right + x.gutter < x.page_w && x.margin_top + x.margin_bottom < x.page_h, "{what}: section {i} margins");
        assert!((1..=45).contains(&x.columns.count), "{what}: section {i} columns {}", x.columns.count);
        assert!(*end < d.body.len().max(1), "{what}: section {i} ends at {end}");
        for (set, kind) in [(&x.headers, PartKind::Header), (&x.footers, PartKind::Footer)] {
            for id in [set.default, set.first, set.even].into_iter().flatten() {
                assert!(d.parts.get(&id).is_some_and(|p| p.kind == kind), "{what}: section {i} names {kind:?} {id}, which isn't one");
            }
        }
    }
    let nested =
        d.para_paths(StoryRef::Body).into_iter().filter(|p| p.depth() > 0).any(|p| d.para(StoryRef::Body, &p).is_some_and(|q| q.section.is_some()));
    assert!(!nested, "{what}: a paragraph in a table ends a section");
    for part in d.parts.values() {
        assert!(part.blocks.iter().all(|b| b.as_para().is_none_or(|p| p.section.is_none())), "{what}: a header or note ends a section");
    }
}

fn check(s: &Session, what: &str) {
    check_style_ids(&s.doc, &s.doc.body, what);
    for part in s.doc.parts.values() {
        check_style_ids(&s.doc, &part.blocks, what);
    }
    check_sections(&s.doc, what);
    for p in [&s.sel.anchor, &s.sel.focus] {
        let para = s.doc.para_at(p).unwrap_or_else(|| panic!("{what}: {p:?} isn't in a paragraph"));
        assert!(p.off <= para.len() && para.text.is_char_boundary(p.off), "{what}: {p:?} isn't a boundary");
    }
}

/// A few pages in two sections with headers and footers, styled paragraphs and a custom style.
fn start(r: &mut Rng) -> Session {
    let mut s = Session::new(crate::sample::sample_document());
    let _ = s.run("caret.docEnd", &json!({}));
    for i in 0..(10 + r.below(20)) {
        let _ = s.run("text.newParagraph", &json!({}));
        let _ = s.run("text.insert", &json!({"text": format!("Paragraph {i} with enough words to wrap onto a second line of the page here.")}));
        if i % 4 == 0 {
            let _ = s.run("para.style", &json!({"style": PARA_STYLES[i % PARA_STYLES.len()]}));
        }
    }
    let _ = s.run("styles.create", &json!({"name": "Mine", "basedOn": "Quote"}));
    let _ = s.run("caret.set", &json!({"pos": {"block": 3, "off": 0}}));
    let _ = s.run("layout.break", &json!({"kind": "continuous"}));
    let _ = s.run("insert.header", &json!({"text": "Running head"}));
    s.sel = crate::Selection::caret(Pos::body(r.below(4), 0));
    s.reset_history();
    s
}

/// Random style and section commands mixed with typing: after every step nothing panicked, every
/// style the text names exists, every section is usable, and the session's layout (resumed from
/// the last one where it can) is the one laid out from scratch; undoing everything gives the
/// document back exactly, and redoing everything gives the result.
#[test]
fn random_style_and_section_edits_keep_the_document_valid_and_relayout_like_from_scratch() {
    let seeds: u64 = std::env::var("WC_FUZZ_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
    let steps: usize = std::env::var("WC_FUZZ_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    let dump = |l: &wordcraft_layout::DocLayout| format!("{:?}\n{}", l.pages, l.text_boxes);
    for seed in 1..=seeds {
        let mut r = Rng(seed.wrapping_mul(0xA076_1D64_78BD_642F) | 1);
        let mut s = start(&mut r);
        let _ = s.layout();
        let original = serialized(&s.doc);
        let mut log: Vec<String> = Vec::new();
        for i in 0..steps {
            let (id, v) = random_command(&mut r);
            log.push(format!("{id} {v}"));
            let res = s.run(id, &v);
            if let Err(e) = &res {
                assert!(!e.to_string().contains("panicked"), "seed {seed} step {i}: {e}\n{}", log.join("\n"));
            }
            let what = format!("seed {seed} step {i} after {id} {v}\n{}", log.join("\n"));
            check(&s, &what);
            let inc = s.layout();
            let full = wordcraft_layout::layout(&s.doc, &mut wordcraft_layout::LayoutCache::new(), &s.layout_options());
            if dump(&inc) != dump(&full) {
                // Which part differs, and whether the session's paragraph cache is to blame.
                let resumed = s.cache_resumed();
                s.relayout();
                let again = s.layout();
                let page = inc.pages.iter().zip(&full.pages).position(|(a, b)| format!("{a:?}") != format!("{b:?}"));
                let item = page.and_then(|k| {
                    let (a, b) = (inc.pages.get(k)?, full.pages.get(k)?);
                    let (ia, ib): (Vec<_>, Vec<_>) =
                        (a.items.iter().map(|x| format!("{x:?}")).collect(), b.items.iter().map(|x| format!("{x:?}")).collect());
                    let at = ia.iter().zip(&ib).position(|(x, y)| x != y)?;
                    let (x, y) = (ia.get(at)?, ib.get(at)?);
                    let c = x.bytes().zip(y.bytes()).take_while(|(p, q)| p == q).count();
                    let cut = |t: &str| t.get(c.saturating_sub(200).min(t.len())..(c + 200).min(t.len())).unwrap_or("").to_string();
                    Some(format!("item {at}:\n  inc  …{}\n  full …{}", cut(x), cut(y)))
                });
                panic!(
                    "{what}: the layout differs from one from scratch ({} pages, {} from scratch; resumed {resumed:?}; with the cache cleared {}), page {page:?} {}",
                    inc.pages.len(),
                    full.pages.len(),
                    if dump(&again) == dump(&full) { "the same" } else { "still different" },
                    item.unwrap_or_default()
                );
            }
        }
        let end = serialized(&s.doc);
        let mut undone = 0;
        while s.can_undo() {
            s.run("edit.undo", &json!({})).unwrap();
            check(&s, &format!("seed {seed} undo {undone}"));
            undone += 1;
        }
        assert!(serialized(&s.doc) == original, "seed {seed}: undoing everything doesn't give the document back\n{}", log.join("\n"));
        for _ in 0..undone {
            s.run("edit.redo", &json!({})).unwrap();
        }
        assert!(serialized(&s.doc) == end, "seed {seed}: redoing everything doesn't give the result back\n{}", log.join("\n"));
    }
    let _ = Pos::body(0, 0);
}

/// Where the session's layout after `cmds` (from seed `seed`'s start) first differs from one from
/// scratch: `None` when it never does.
fn replay_fails(seed: u64, cmds: &[(String, Value)]) -> Option<String> {
    let dump = |l: &wordcraft_layout::DocLayout| format!("{:?}\n{}", l.pages, l.text_boxes);
    let mut r = Rng(seed.wrapping_mul(0xA076_1D64_78BD_642F) | 1);
    let mut s = start(&mut r);
    let _ = s.layout();
    for (i, (id, v)) in cmds.iter().enumerate() {
        let _ = s.run(id, v);
        let inc = s.layout();
        let full = wordcraft_layout::layout(&s.doc, &mut wordcraft_layout::LayoutCache::new(), &s.layout_options());
        if dump(&inc) != dump(&full) {
            let mut out = format!("step {i} {id} {v}: resumed {:?}", s.cache_resumed());
            for (k, (a, b)) in inc.pages.iter().zip(&full.pages).enumerate() {
                for (part, x, y) in [
                    ("items", format!("{:?}", a.items), format!("{:?}", b.items)),
                    ("header", format!("{:?}", a.header), format!("{:?}", b.header)),
                    ("footer", format!("{:?}", a.footer), format!("{:?}", b.footer)),
                    (
                        "page",
                        format!("{:?}", (a.w, a.h, a.section, a.number, a.body, a.header_story, a.footer_story, a.hf_slot, a.first_block)),
                        format!("{:?}", (b.w, b.h, b.section, b.number, b.body, b.header_story, b.footer_story, b.hf_slot, b.first_block)),
                    ),
                ] {
                    if x != y {
                        let c = x.bytes().zip(y.bytes()).take_while(|(p, q)| p == q).count();
                        let cut = |t: &str| {
                            t.get(t.floor_char_boundary(c.saturating_sub(300))..t.floor_char_boundary((c + 300).min(t.len())))
                                .unwrap_or("")
                                .to_string()
                        };
                        out += &format!("\npage {k} {part} at {c}:\n  inc  …{}\n  full …{}", cut(&x), cut(&y));
                    }
                }
            }
            return Some(out);
        }
    }
    None
}

/// Replays `WC_REPLAY` (lines `id json`, from the random test's failure message) from seed
/// `WC_SEED`'s start, shrinks the sequence while it still fails and prints the differences.
#[test]
#[ignore]
fn replay_style_edits() {
    let (Some(path), Some(seed)) = (std::env::var("WC_REPLAY").ok(), std::env::var("WC_SEED").ok().and_then(|v| v.parse().ok())) else { return };
    let text = std::fs::read_to_string(path).unwrap();
    let mut cmds: Vec<(String, Value)> =
        text.lines().filter_map(|l| l.split_once(' ')).map(|(id, v)| (id.to_string(), serde_json::from_str(v).unwrap_or(json!({})))).collect();
    let Some(first) = replay_fails(seed, &cmds) else {
        eprintln!("REPLAY: no difference");
        return;
    };
    eprintln!("REPLAY: {first}");
    let mut i = 0;
    while i < cmds.len() {
        let mut fewer = cmds.clone();
        fewer.remove(i);
        if replay_fails(seed, &fewer).is_some() {
            cmds = fewer;
        } else {
            i += 1;
        }
    }
    eprintln!("REPLAY shrunk to:\n{}", cmds.iter().map(|(id, v)| format!("{id} {v}")).collect::<Vec<_>>().join("\n"));
    eprintln!("REPLAY: {}", replay_fails(seed, &cmds).unwrap_or_default());
}
