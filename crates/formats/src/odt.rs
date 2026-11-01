//! OpenDocument Text (`.odt`, ODF 1.3).
//!
//! Export writes `mimetype` (stored, first), `META-INF/manifest.xml`, `content.xml` (paragraphs,
//! `text:h` headings with outline levels, spans with automatic text styles, links, bookmarks,
//! line breaks / tabs / `text:s`, nested `text:list`s with generated list styles, tables with
//! header rows, column/row spans and cell shading, pictures as `draw:frame`/`draw:image` with the
//! bytes under `Pictures/`, page breaks), `styles.xml` (Standard, Heading 1–6, Title,
//! Quotations, Preformatted Text, page layout) and `meta.xml`.
//!
//! Import reads the same subset back from `content.xml` / `styles.xml` (style inheritance
//! resolved, both automatic and common styles), skipping drawing shapes.
//!
//! Both ways: footnotes and endnotes (`text:note`), comments (`office:annotation` and its
//! `office:annotation-end`), text boxes (`draw:frame`/`draw:text-box`), page number and count
//! fields, the headers and footers of the master pages (`style:header`, `-left` for even pages,
//! `-first`), and on export one master page per section.
//!
//! Tracked changes (ODF 1.3 Part 3 §5.5) go both ways: `text:tracked-changes` holds one
//! `text:changed-region` per change; an insertion is the body text between
//! `text:change-start`/`text:change-end`, a deletion keeps the deleted paragraphs in its region
//! and leaves a `text:change` point in the body. They map to `Document::revisions` and the
//! `ins`/`del` marks of runs and paragraph marks; a deleted paragraph mark is a region of
//! several paragraphs. Format changes are read as no change.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use wordcraft_doc::section::{Columns, SectionStart};
use wordcraft_doc::{Align, Document, RevisionKind, Rgb, SectionProps};

use crate::model::{self, Cell, FBlock, FTable, Flow, Fmt, Inline, Kind, ListInfo, Meta, Para, RevMark, make_img, mime_of};

/// Largest zip entry we inflate: as large as an XML part of a document within the limits it is
/// held to may be ([`wordcraft_doc::limits::MAX_XML_PART_BYTES`]).
const MAX_ENTRY: u64 = wordcraft_doc::limits::MAX_XML_PART_BYTES;
/// Most bytes inflated from `content.xml`, `styles.xml` and `meta.xml` together (a deflated
/// entry can be a thousand times smaller than what it inflates to). The pictures have their own
/// budget, [`MAX_IMAGES`].
const MAX_XML_ENTRIES: u64 = MAX_ENTRY;
/// Most bytes read of the `mimetype` entry (a media type).
const MAX_MIMETYPE: u64 = 256;
/// Most list styles kept from one XML part (as many as styles, see `Styles::read`). The writer
/// writes a list style for every list, but more lists than this take more XML than the XML
/// budget allows ([`MAX_XML_ENTRIES`]), so a document WordCraft saves never reaches the cap
/// (`the_list_style_cap_is_past_what_the_xml_budget_holds`).
const MAX_LIST_STYLES: usize = 100_000;
/// Deepest element nesting handled in content.xml.
const MAX_XML_DEPTH: usize = 512;
/// Most text (bytes) read from content.xml: `<text:s text:c="1000"/>` amplifies ~45×, so the expanded
/// text gets its own budget, as large as the XML budget: text written out can't pass it (it takes
/// at least its own bytes of XML), so a document WordCraft saves always reads back whole, and a
/// file whose spaces expand past it is refused rather than read in part.
const MAX_TEXT: usize = MAX_XML_ENTRIES as usize;
/// Most image bytes inflated from the package, across all `draw:image`s: the pictures a document
/// may have ([`wordcraft_doc::limits::MEDIA_BYTES`]).
const MAX_IMAGES: u64 = wordcraft_doc::limits::MEDIA_BYTES;
/// Most sections a file may make (page-style changes and column sections), and master pages and
/// page layouts it may have: the sections a document may have.
const MAX_SECTIONS: usize = wordcraft_doc::limits::SECTIONS;
/// Most `text:changed-region`s a file may have: one a run at most (tests use fewer, so a file
/// past the cap stays small).
const MAX_REGIONS: usize = if cfg!(test) { 20_000 } else { wordcraft_doc::limits::RUNS };
/// Most insertions open at once (`text:change-start` without its end yet).
const MAX_OPEN_INSERTIONS: usize = 64;
/// Longest change or comment author, date or initials, in characters.
const MAX_INFO_CHARS: usize = wordcraft_doc::limits::AUTHOR_CHARS;

thread_local! {
    /// The first limit the file being read was found past: it is refused with it (see
    /// [`refuse`]), rather than read without what is over.
    static REFUSED: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The file being read is past a limit (`what` says which): it is refused once read.
fn refuse(what: impl FnOnce() -> String) {
    REFUSED.with(|r| {
        let mut r = r.borrow_mut();
        if r.is_none() {
            *r = Some(what());
        }
    });
}

fn take_refused() -> Option<String> {
    REFUSED.with(|r| r.borrow_mut().take())
}

/// `s` with `t` added, at most `max` characters: more has the file refused.
fn extend_capped(s: &mut String, t: &str, max: usize, what: &str) {
    let room = max.saturating_sub(s.chars().count());
    if t.chars().nth(room).is_some() {
        refuse(|| format!("{what} longer than {max} characters"));
    }
    s.extend(t.chars().take(room));
}

// ---------------------------------------------------------------------------------------------
// Export

fn x(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            // Not characters in XML 1.0.
            c if c.is_control() && !matches!(c, '\t' | '\n') => {}
            '\u{FFFE}' | '\u{FFFF}' => {}
            _ => o.push(c),
        }
    }
    o
}

const NS: &str = concat!(
    "xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" ",
    "xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\" ",
    "xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" ",
    "xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" ",
    "xmlns:draw=\"urn:oasis:names:tc:opendocument:xmlns:drawing:1.0\" ",
    "xmlns:fo=\"urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0\" ",
    "xmlns:xlink=\"http://www.w3.org/1999/xlink\" ",
    "xmlns:dc=\"http://purl.org/dc/elements/1.1/\" ",
    "xmlns:meta=\"urn:oasis:names:tc:opendocument:xmlns:meta:1.0\" ",
    "xmlns:svg=\"urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0\" ",
    "office:version=\"1.3\""
);

fn pt(v: f32) -> String {
    let s = format!("{:.2}", v);
    format!("{}pt", s.trim_end_matches('0').trim_end_matches('.'))
}

/// A section that starts a page: the master page it uses and where page numbering restarts.
type SecStart = (String, Option<u32>);

#[derive(Default)]
struct Writer {
    /// Automatic text styles: properties XML → name.
    text_styles: Vec<(String, String)>,
    /// Automatic paragraph styles: (parent, properties XML, master page) → name.
    para_styles: Vec<((String, String, String), String)>,
    /// List styles: level kinds.
    list_styles: Vec<[bool; 9]>,
    /// Table styles: (name, column widths).
    tables: Vec<Vec<f32>>,
    /// The master page (and page-number restart) each table in `tables` starts.
    table_starts: Vec<Option<SecStart>>,
    /// Automatic styles of the columned `text:section`s written.
    section_styles: Vec<String>,
    /// The page a section starts, for the next top-level block.
    pending_start: Option<SecStart>,
    /// The next top-level list continues the one that ended the previous section.
    continue_list: bool,
    /// `xml:id` of the last top-level list written, for `text:continue-list`.
    last_list: Option<String>,
    /// The page the block being written starts.
    master: Option<SecStart>,
    /// Cell styles by shading.
    cell_styles: Vec<Option<Rgb>>,
    pictures: Vec<(String, std::sync::Arc<Vec<u8>>, String)>,
    /// `text:changed-region` elements written so far.
    regions: Vec<String>,
    /// The insertion open in the body, and its region id.
    open_ins: Option<(RevMark, String)>,
    /// Style of the paragraph being written.
    cur_style: String,
    /// Styles of the paragraphs joined into the one being written (deleted paragraph marks),
    /// and which of them the text being written belongs to.
    chain: Vec<String>,
    part: usize,
    /// Prefix of the automatic style and picture names (the header writer's go in styles.xml).
    prefix: &'static str,
    /// The document's comments, and those whose annotation is written (by id).
    comments: std::collections::BTreeMap<u32, model::FComment>,
    annotated: std::collections::HashSet<u32>,
    /// Footnotes and endnotes written so far (their numbers), and text boxes.
    notes: [u32; 2],
    boxes: u32,
}

fn text_props(f: &Fmt) -> String {
    let mut p = String::new();
    if f.bold {
        p.push_str(" fo:font-weight=\"bold\" style:font-weight-asian=\"bold\" style:font-weight-complex=\"bold\"");
    }
    if f.italic {
        p.push_str(" fo:font-style=\"italic\" style:font-style-asian=\"italic\" style:font-style-complex=\"italic\"");
    }
    if f.underline {
        p.push_str(" style:text-underline-style=\"solid\" style:text-underline-width=\"auto\" style:text-underline-color=\"font-color\"");
    }
    if f.strike {
        p.push_str(" style:text-line-through-style=\"solid\"");
    }
    if f.sup {
        p.push_str(" style:text-position=\"super 58%\"");
    } else if f.sub {
        p.push_str(" style:text-position=\"sub 58%\"");
    }
    if let Some(c) = f.color {
        p.push_str(&format!(" fo:color=\"#{}\"", c.hex().to_ascii_lowercase()));
    }
    if let Some(c) = f.background {
        p.push_str(&format!(" fo:background-color=\"#{}\"", c.hex().to_ascii_lowercase()));
    }
    if let Some(s) = f.size {
        p.push_str(&format!(" fo:font-size=\"{}\"", pt(s)));
    }
    if f.code {
        p.push_str(&format!(" fo:font-family=\"'{}'\" style:font-family-generic=\"modern\" style:font-pitch=\"fixed\"", model::MONO_FONT));
    } else if let Some(fam) = &f.font {
        p.push_str(&format!(" fo:font-family=\"'{}'\"", x(&fam.replace('\'', ""))));
    }
    p
}

fn parent_style(k: Kind) -> &'static str {
    match k {
        Kind::Heading(1) => "Heading_20_1",
        Kind::Heading(2) => "Heading_20_2",
        Kind::Heading(3) => "Heading_20_3",
        Kind::Heading(4) => "Heading_20_4",
        Kind::Heading(5) => "Heading_20_5",
        Kind::Heading(_) => "Heading_20_6",
        Kind::Title => "Title",
        Kind::Quote => "Quotations",
        Kind::Code => "Preformatted_20_Text",
        Kind::Normal | Kind::Rule => "Standard",
    }
}

impl Writer {
    fn text_style(&mut self, f: &Fmt) -> Option<String> {
        let props = text_props(f);
        if props.is_empty() {
            return None;
        }
        if let Some((_, n)) = self.text_styles.iter().find(|(p, _)| *p == props) {
            return Some(n.clone());
        }
        let n = format!("{}T{}", self.prefix, self.text_styles.len() + 1);
        self.text_styles.push((props, n.clone()));
        Some(n)
    }

    fn para_style(&mut self, p: &Para) -> String {
        self.para_style_at(p, None)
    }

    /// The paragraph style of `p`; `start` makes it begin a page of that master page (ODF 1.3
    /// §19.471 `style:master-page-name`), restarting page numbering where asked (§20.322).
    fn para_style_at(&mut self, p: &Para, start: Option<&SecStart>) -> String {
        let parent = parent_style(p.kind).to_string();
        let mut props = String::new();
        match p.align {
            Some(Align::Center) => props.push_str(" fo:text-align=\"center\""),
            Some(Align::Right) => props.push_str(" fo:text-align=\"end\""),
            Some(Align::Justify) | Some(Align::Distribute) => props.push_str(" fo:text-align=\"justify\""),
            Some(Align::Left) => props.push_str(" fo:text-align=\"start\""),
            None => {}
        }
        // The master page starts the page itself: no page break besides.
        if p.page_break && start.is_none() {
            props.push_str(" fo:break-before=\"page\"");
        }
        if let Some(n) = start.and_then(|(_, n)| *n) {
            props.push_str(&format!(" style:page-number=\"{n}\""));
        }

        if p.keep_next {
            props.push_str(" fo:keep-with-next=\"always\"");
        }
        if p.rtl {
            props.push_str(" style:writing-mode=\"rl-tb\"");
        }
        let sp = &p.spacing;
        for (k, v) in [
            ("fo:margin-top", sp.before),
            ("fo:margin-bottom", sp.after),
            ("fo:margin-left", sp.left),
            ("fo:margin-right", sp.right),
            ("fo:text-indent", sp.first),
        ] {
            if let Some(v) = v.filter(|v| v.is_finite()) {
                props.push_str(&format!(" {k}=\"{}\"", pt(v)));
            }
        }
        match sp.line {
            Some(wordcraft_doc::props::LineSpacing::Multiple(m)) if m.is_finite() => {
                props.push_str(&format!(" fo:line-height=\"{}%\"", (m * 100.0).round()));
            }
            Some(wordcraft_doc::props::LineSpacing::Exactly(v)) if v.is_finite() => props.push_str(&format!(" fo:line-height=\"{}\"", pt(v))),
            Some(wordcraft_doc::props::LineSpacing::AtLeast(v)) if v.is_finite() => {
                props.push_str(&format!(" style:line-height-at-least=\"{}\"", pt(v)))
            }
            _ => {}
        }
        if p.kind == Kind::Rule {
            props.push_str(" fo:border-bottom=\"0.75pt solid #a0a0a0\" fo:padding-bottom=\"1pt\"");
        }
        let master = start.map(|(m, _)| m.clone()).unwrap_or_default();
        if props.is_empty() && master.is_empty() {
            return parent;
        }
        let key = (parent, props, master);
        if let Some((_, n)) = self.para_styles.iter().find(|(k, _)| *k == key) {
            return n.clone();
        }
        let n = format!("{}P{}", self.prefix, self.para_styles.len() + 1);
        self.para_styles.push((key, n.clone()));
        n
    }

    fn picture(&mut self, img: &model::Img) -> String {
        let ext = if img.ext == "jpeg" { "jpg".to_string() } else { img.ext.clone() };
        let name = format!("Pictures/{}image{}.{}", self.prefix, self.pictures.len() + 1, ext);
        self.pictures.push((name.clone(), img.data.clone(), mime_of(&img.ext).to_string()));
        name
    }

    /// A new `text:changed-region` of `kind` by `m` with `body` after its change info; its id.
    fn region(&mut self, kind: &str, m: &RevMark, body: &str) -> String {
        let id = format!("ct{}", self.regions.len() + 1);
        // `dc:date` is required: a change without a date is dated now.
        let date = if m.date.trim().is_empty() { now_iso() } else { m.date.clone() };
        self.regions.push(format!(
            "<text:changed-region text:id=\"{id}\"><text:{kind}><office:change-info><dc:creator>{}</dc:creator><dc:date>{}</dc:date></office:change-info>{body}</text:{kind}></text:changed-region>",
            x(&m.author),
            x(&date)
        ));
        id
    }

    /// Make `m` the insertion open in `st` (closing another one first).
    fn ins_open(&mut self, st: &mut Option<(RevMark, String)>, m: &RevMark, out: &mut String) {
        if st.as_ref().is_some_and(|(o, _)| o == m) {
            return;
        }
        ins_close(st, out);
        let ct = self.region("insertion", m, "");
        out.push_str(&format!("<text:change-start text:change-id=\"{ct}\"/>"));
        *st = Some((m.clone(), ct));
    }

    fn open_ins(&mut self, m: &RevMark, out: &mut String) {
        let mut st = self.open_ins.take();
        self.ins_open(&mut st, m, out);
        self.open_ins = st;
    }

    fn close_ins(&mut self, out: &mut String) {
        ins_close(&mut self.open_ins, out);
    }

    /// Inlines with their insertions as change ranges kept in `st`.
    fn ins_inlines(&mut self, items: &[Item<'_>], st: &mut Option<(RevMark, String)>, out: &mut String) {
        let mut k = 0;
        while k < items.len() {
            let ins = items.get(k).and_then(|i| i.marks().0);
            let mut j = k + 1;
            while j < items.len() && items.get(j).and_then(|i| i.marks().0) == ins {
                j += 1;
            }
            match ins {
                Some(m) => self.ins_open(st, m, out),
                None => ins_close(st, out),
            }
            let seg: Vec<Inline> = items.get(k..j).unwrap_or(&[]).iter().filter_map(|i| i.inline().cloned()).collect();
            self.plain_inlines(&seg, out);
            k = j;
        }
    }

    /// Deleted content: a region holding it (one paragraph per deleted paragraph mark plus
    /// one; text inserted before it was deleted as change ranges inside) and a `text:change`
    /// point in the body.
    fn deletion(&mut self, seg: &[Item<'_>], m: &RevMark, out: &mut String) {
        let mut body = String::new();
        let mut st: Option<(RevMark, String)> = None;
        let mut start = 0;
        let mut first = true;
        loop {
            let end = seg.iter().skip(start).position(|i| matches!(i, Item::Mark(_))).map(|e| start + e);
            if !first {
                self.part += 1;
            }
            first = false;
            let style = self.chain.get(self.part).cloned().unwrap_or_else(|| self.cur_style.clone());
            body.push_str(&format!("<text:p text:style-name=\"{style}\">"));
            self.ins_inlines(seg.get(start..end.unwrap_or(seg.len())).unwrap_or(&[]), &mut st, &mut body);
            // A deleted paragraph mark that had been inserted: the range runs on past it.
            match end.and_then(|e| seg.get(e)).and_then(|i| i.marks().0) {
                Some(ins) => self.ins_open(&mut st, ins, &mut body),
                None => ins_close(&mut st, &mut body),
            }
            body.push_str("</text:p>");
            match end {
                Some(e) => start = e + 1,
                None => break,
            }
        }
        let ct = self.region("deletion", m, &body);
        out.push_str(&format!("<text:change text:change-id=\"{ct}\"/>"));
    }

    /// Paragraph content, with tracked insertions as change ranges and deletions as change points.
    fn items(&mut self, items: &[Item<'_>], out: &mut String) {
        let mut k = 0;
        while k < items.len() {
            let key = items.get(k).map(Item::segment).unwrap_or_default();
            let mut j = k + 1;
            while j < items.len() && items.get(j).map(Item::segment).unwrap_or_default() == key {
                j += 1;
            }
            let seg = items.get(k..j).unwrap_or(&[]);
            match key {
                (_, Some(del)) => self.deletion(seg, del, out),
                (Some(ins), None) => {
                    self.open_ins(ins, out);
                    let inl: Vec<Inline> = seg.iter().filter_map(|i| i.inline().cloned()).collect();
                    self.plain_inlines(&inl, out);
                }
                (None, None) => {
                    self.close_ins(out);
                    let inl: Vec<Inline> = seg.iter().filter_map(|i| i.inline().cloned()).collect();
                    self.plain_inlines(&inl, out);
                }
            }
            k = j;
        }
    }

    fn plain_inlines(&mut self, inl: &[Inline], out: &mut String) {
        let mut prev_space = true;
        let inl = model::equations_as_text(inl);
        for i in inl.iter() {
            match i {
                Inline::Text(t, f) => {
                    let mut body = String::new();
                    let mut spaces = 0usize;
                    let flush = |body: &mut String, spaces: &mut usize, prev_space: bool| {
                        if *spaces == 0 {
                            return;
                        }
                        let (lit, extra) = if prev_space { (0, *spaces) } else { (1, *spaces - 1) };
                        if lit == 1 {
                            body.push(' ');
                        }
                        if extra > 0 {
                            body.push_str(&format!("<text:s text:c=\"{extra}\"/>"));
                        }
                        *spaces = 0;
                    };
                    for c in t.chars() {
                        if c == ' ' {
                            spaces += 1;
                            continue;
                        }
                        flush(&mut body, &mut spaces, prev_space);
                        prev_space = false;
                        match c {
                            '\t' => {
                                // The reader collapses a literal space after a tab, so spaces
                                // that follow are written as `<text:s/>`.
                                body.push_str("<text:tab/>");
                                prev_space = true;
                            }
                            '\n' => {
                                body.push_str("<text:line-break/>");
                                prev_space = true;
                            }
                            '\u{000C}' | '\u{000E}' => {}
                            c => body.push_str(&x(&c.to_string())),
                        }
                    }
                    if spaces > 0 {
                        flush(&mut body, &mut spaces, prev_space);
                        prev_space = true;
                    }
                    // A script, `file:`, UNC or other non-web target is written as plain text, as
                    // the HTML and Markdown exporters do (LibreOffice follows `file:`/UNC hrefs).
                    let link = f.link.as_deref().filter(|l| wordcraft_doc::link::is_safe_href(l));
                    if let Some(l) = link {
                        out.push_str(&format!("<text:a xlink:type=\"simple\" xlink:href=\"{}\">", x(l)));
                    }
                    let fs = Fmt { link: None, ..f.clone() };
                    match self.text_style(&fs) {
                        Some(n) => out.push_str(&format!("<text:span text:style-name=\"{n}\">{body}</text:span>")),
                        None => out.push_str(&body),
                    }
                    if link.is_some() {
                        out.push_str("</text:a>");
                    }
                }
                Inline::Image(img) => {
                    let href = self.picture(img);
                    let n = self.pictures.len();
                    out.push_str(&format!(
                        "<draw:frame draw:name=\"{}Image{n}\" text:anchor-type=\"as-char\" svg:width=\"{}\" svg:height=\"{}\" draw:z-index=\"0\"><draw:image xlink:href=\"{href}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\"/>",
                        self.prefix,
                        pt(img.w),
                        pt(img.h)
                    ));
                    if !img.alt.is_empty() {
                        out.push_str(&format!("<svg:desc>{}</svg:desc>", x(&img.alt)));
                    }
                    out.push_str("</draw:frame>");
                    prev_space = false;
                }
                Inline::Anchor(a) => out.push_str(&format!("<text:bookmark text:name=\"{}\"/>", x(a))),
                Inline::Figure(_) | Inline::Equation { .. } => {}
                Inline::Note(n) => {
                    let k = usize::from(n.endnote);
                    let num = self.notes.get(k).copied().unwrap_or(0) + 1;
                    if let Some(c) = self.notes.get_mut(k) {
                        *c = num;
                    }
                    let (class, id) =
                        if n.endnote { ("endnote", format!("{}edn{num}", self.prefix)) } else { ("footnote", format!("{}ftn{num}", self.prefix)) };
                    let custom = n.custom.trim();
                    let (label, cite) =
                        if custom.is_empty() { (String::new(), num.to_string()) } else { (format!(" text:label=\"{}\"", x(custom)), x(custom)) };
                    out.push_str(&format!("<text:note text:id=\"{id}\" text:note-class=\"{class}\"><text:note-citation{label}>{cite}</text:note-citation><text:note-body>"));
                    self.nested(&n.blocks, out);
                    out.push_str("</text:note-body></text:note>");
                    prev_space = false;
                }
                Inline::CommentStart(id) => {
                    if self.annotated.insert(*id) {
                        self.annotation(*id, true, out);
                    }
                }
                Inline::CommentEnd(id) => {
                    if self.annotated.contains(id) {
                        out.push_str(&format!("<office:annotation-end office:name=\"{}\"/>", annotation_name(*id)));
                    } else if self.annotated.insert(*id) {
                        // No start written: a comment on this point.
                        self.annotation(*id, false, out);
                    }
                }
                Inline::TextBox(tb) => {
                    self.boxes += 1;
                    out.push_str(&format!(
                        "<draw:frame draw:name=\"{}Text Box {}\" text:anchor-type=\"as-char\" svg:width=\"{}\" svg:height=\"{}\" draw:z-index=\"0\"><draw:text-box>",
                        self.prefix,
                        self.boxes,
                        pt(tb.w.max(1.0)),
                        pt(tb.h.max(1.0))
                    ));
                    self.nested(&tb.blocks, out);
                    out.push_str("</draw:text-box></draw:frame>");
                    prev_space = false;
                }
                Inline::Field { instr, result } => {
                    let name = instr.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
                    match name.as_str() {
                        "PAGE" => out.push_str(&format!("<text:page-number text:select-page=\"current\">{}</text:page-number>", x(result))),
                        "NUMPAGES" => out.push_str(&format!("<text:page-count>{}</text:page-count>", x(result))),
                        "DATE" => out.push_str(&format!("<text:date>{}</text:date>", x(result))),
                        "TIME" => out.push_str(&format!("<text:time>{}</text:time>", x(result))),
                        "AUTHOR" => out.push_str(&format!("<text:author-name>{}</text:author-name>", x(result))),
                        "TITLE" => out.push_str(&format!("<text:title>{}</text:title>", x(result))),
                        // Other fields keep their last result ([`KEPT_FIELDS`]).
                        _ => out.push_str(&x(result)),
                    }
                    prev_space = result.ends_with(' ');
                }
            }
        }
    }

    /// The blocks of a note, text box or comment, written inside the paragraph being written:
    /// the paragraph's own state is kept aside meanwhile.
    fn nested(&mut self, blocks: &[FBlock], out: &mut String) {
        let saved = (self.open_ins.take(), std::mem::take(&mut self.chain), self.part, std::mem::take(&mut self.cur_style));
        if blocks.is_empty() {
            out.push_str("<text:p/>");
        }
        self.blocks(blocks, out, model::MAX_DEPTH / 2);
        self.close_ins(out);
        (self.open_ins, self.chain, self.part, self.cur_style) = saved;
    }

    /// Comment `id` as an `office:annotation` (ODF 1.3 §14.1), on a range ending at its
    /// `office:annotation-end` when `ranged`.
    fn annotation(&mut self, id: u32, ranged: bool, out: &mut String) {
        let Some(c) = self.comments.get(&id).cloned() else { return };
        let name = if ranged { format!(" office:name=\"{}\"", annotation_name(id)) } else { String::new() };
        out.push_str(&format!("<office:annotation{name}>"));
        if !c.author.is_empty() {
            out.push_str(&format!("<dc:creator>{}</dc:creator>", x(&c.author)));
        }
        if !c.date.is_empty() {
            out.push_str(&format!("<dc:date>{}</dc:date>", x(c.date.trim_end_matches('Z'))));
        }
        if !c.initials.is_empty() {
            out.push_str(&format!("<meta:creator-initials>{}</meta:creator-initials>", x(&c.initials)));
        }
        self.nested(&c.blocks, out);
        out.push_str("</office:annotation>");
    }

    /// Paragraph `p`, or with `chain` the paragraphs joined at deleted paragraph marks (`p`
    /// is the first).
    fn para_in(&mut self, p: &Para, chain: Option<&Vec<Para>>, out: &mut String) {
        let paras: &[Para] = chain.map(|c| c.as_slice()).unwrap_or(std::slice::from_ref(p));
        let start = self.master.take();
        self.chain = chain.map(|c| c.iter().map(|q| self.para_style(q)).collect()).unwrap_or_default();
        self.part = 0;
        let style = self.para_style_at(p, start.as_ref());
        self.cur_style = style.clone();
        let tag = match p.kind {
            Kind::Heading(n) => {
                out.push_str(&format!("<text:h text:style-name=\"{style}\" text:outline-level=\"{}\">", n.clamp(1, 6)));
                "</text:h>"
            }
            _ => {
                out.push_str(&format!("<text:p text:style-name=\"{style}\">"));
                "</text:p>"
            }
        };
        let tracked = paras.len() > 1 || p.inlines.iter().any(|i| matches!(i, Inline::Text(_, f) if f.ins.is_some() || f.del.is_some()));
        if tracked {
            let mut items = Vec::new();
            for (k, q) in paras.iter().enumerate() {
                items.extend(q.inlines.iter().map(Item::Inl));
                if k + 1 < paras.len() {
                    items.push(Item::Mark(q));
                }
            }
            self.items(&items, out);
        } else {
            self.close_ins(out);
            self.plain_inlines(&p.inlines, out);
        }
        // An inserted paragraph mark: the insertion runs on past the paragraph's end.
        match paras.last().and_then(|q| q.mark_ins.as_deref()) {
            Some(m) => self.open_ins(m, out),
            None => self.close_ins(out),
        }
        out.push_str(tag);
        self.chain.clear();
    }

    fn blocks(&mut self, blocks: &[FBlock], out: &mut String, depth: usize) {
        let joined = join_deleted_marks(blocks);
        let (blocks, chains) = match &joined {
            Some((b, c)) => (b.as_slice(), Some(c)),
            None => (blocks, None),
        };
        let chain = |i: usize| chains.and_then(|c| c.get(&i));
        let mut i = 0;
        while let Some(b) = blocks.get(i) {
            // Only the first top-level block of a section starts its page.
            if depth == 0 {
                self.master = self.pending_start.take();
            }
            match b {
                FBlock::Table(t) => {
                    if depth < model::MAX_DEPTH {
                        self.table(t, out, depth);
                    }
                    i += 1;
                }
                FBlock::Para(p) if p.list.is_some() => {
                    let start = i;
                    let mut kinds = [p.list.map(|l| l.ordered).unwrap_or(false); 9];
                    let mut seen = [false; 9];
                    let first_list = p.list.and_then(|l| l.list);
                    while let Some(FBlock::Para(q)) = blocks.get(i) {
                        let Some(li) = q.list else { break };
                        let l = li.level.min(8) as usize;
                        // A top-level item of the other kind, of another list or restarting at
                        // a number of its own starts a new list.
                        if l == 0 && seen[0] && (kinds[0] != li.ordered || li.start.is_some() || li.list != first_list) {
                            break;
                        }
                        if let (Some(s), Some(k)) = (seen.get_mut(l), kinds.get_mut(l))
                            && !*s
                        {
                            *s = true;
                            *k = li.ordered;
                        }
                        i += 1;
                    }
                    self.list_styles.push(kinds);
                    let lname = format!("{}L{}", self.prefix, self.list_styles.len());
                    // Open lists: per level, whether a list-item is open.
                    let mut stack: Vec<bool> = Vec::new();
                    for (qi, q) in blocks.get(start..i).unwrap_or(&[]).iter().enumerate() {
                        let FBlock::Para(q) = q else { continue };
                        let lv = q.list.map(|l| l.level.min(8) as usize).unwrap_or(0);
                        while stack.len() > lv + 1 {
                            if stack.pop() == Some(true) {
                                out.push_str("</text:list-item>");
                            }
                            out.push_str("</text:list>");
                        }
                        while stack.len() < lv + 1 {
                            if let Some(top) = stack.last_mut()
                                && !*top
                            {
                                out.push_str("<text:list-item>");
                                *top = true;
                            }
                            if stack.is_empty() {
                                let id = format!("lst{}", self.list_styles.len());
                                // A list cut by a section boundary goes on numbering: continue-list names the
                                // preceding list by its xml:id, whatever its style name (ODF 1.3 §19.743).
                                let prev = self.last_list.clone();
                                let cont = match prev {
                                    Some(prev) if depth == 0 && start == 0 && std::mem::take(&mut self.continue_list) => {
                                        format!(" text:continue-list=\"{prev}\"")
                                    }
                                    _ => String::new(),
                                };
                                if depth == 0 {
                                    self.last_list = Some(id.clone());
                                }
                                out.push_str(&format!("<text:list xml:id=\"{id}\" text:style-name=\"{lname}\"{cont}>"));
                            } else {
                                out.push_str("<text:list>");
                            }
                            stack.push(false);
                        }
                        if let Some(top) = stack.last_mut() {
                            if *top {
                                out.push_str("</text:list-item>");
                            }
                            *top = true;
                        }
                        match q.list.filter(|l| l.ordered).and_then(|l| l.start) {
                            Some(s) => out.push_str(&format!("<text:list-item text:start-value=\"{s}\">")),
                            None => out.push_str("<text:list-item>"),
                        }
                        self.para_in(q, chain(start + qi), out);
                    }
                    while let Some(open) = stack.pop() {
                        if open {
                            out.push_str("</text:list-item>");
                        }
                        out.push_str("</text:list>");
                    }
                    out.push('\n');
                }
                FBlock::Para(p) => {
                    self.para_in(p, chain(i), out);
                    out.push('\n');
                    i += 1;
                }
            }
        }
    }

    fn cell_style(&mut self, shading: Option<Rgb>) -> String {
        let k = match self.cell_styles.iter().position(|s| *s == shading) {
            Some(k) => k,
            None => {
                self.cell_styles.push(shading);
                self.cell_styles.len() - 1
            }
        };
        format!("{}Cell{}", self.prefix, k + 1)
    }

    fn table(&mut self, t: &FTable, out: &mut String, depth: usize) {
        self.close_ins(out);
        let cols = t.cols().max(1);
        let widths: Vec<f32> = if t.widths.len() == cols && t.widths.iter().all(|w| w.is_finite() && *w > 0.0) {
            t.widths.clone()
        } else {
            vec![468.0 / cols as f32; cols]
        };
        self.tables.push(widths);
        self.table_starts.push(self.master.take());
        let tn = format!("{}{}", self.prefix, self.tables.len());
        out.push_str(&format!("<table:table table:name=\"Table{tn}\" table:style-name=\"Table{tn}\">"));
        for c in 0..cols {
            out.push_str(&format!("<table:table-column table:style-name=\"Table{tn}.C{}\"/>", c + 1));
        }
        let header_rows = t.rows.iter().take_while(|r| !r.is_empty() && r.iter().all(|c| c.header)).count();
        for (ri, row) in t.rows.iter().enumerate() {
            if ri == 0 && header_rows > 0 {
                out.push_str("<table:table-header-rows>");
            }
            out.push_str("<table:table-row>");
            let mut g = 0usize;
            for c in row {
                let span = c.colspan.clamp(1, 63) as usize;
                g += span;
                if c.covered {
                    for _ in 0..span {
                        out.push_str("<table:covered-table-cell/>");
                    }
                    continue;
                }
                let st = self.cell_style(c.shading);
                let mut attrs = format!(" table:style-name=\"{st}\" office:value-type=\"string\"");
                if span > 1 {
                    attrs.push_str(&format!(" table:number-columns-spanned=\"{span}\""));
                }
                if c.rowspan > 1 {
                    attrs.push_str(&format!(" table:number-rows-spanned=\"{}\"", c.rowspan));
                }
                out.push_str(&format!("<table:table-cell{attrs}>"));
                if c.blocks.is_empty() {
                    out.push_str("<text:p text:style-name=\"Standard\"/>");
                }
                self.blocks(&c.blocks, out, depth + 1);
                self.close_ins(out);
                out.push_str("</table:table-cell>");
                for _ in 1..span {
                    out.push_str("<table:covered-table-cell/>");
                }
            }
            for _ in g..cols {
                out.push_str("<table:table-cell office:value-type=\"string\"><text:p/></table:table-cell>");
            }
            out.push_str("</table:table-row>");
            if ri + 1 == header_rows {
                out.push_str("</table:table-header-rows>");
            }
        }
        out.push_str("</table:table>\n");
    }

    fn automatic_styles(&self) -> String {
        let mut s = String::from("<office:automatic-styles>");
        for (props, n) in &self.text_styles {
            s.push_str(&format!("<style:style style:name=\"{n}\" style:family=\"text\"><style:text-properties{props}/></style:style>"));
        }
        for ((parent, props, master), n) in &self.para_styles {
            let master = if master.is_empty() { String::new() } else { format!(" style:master-page-name=\"{}\"", x(master)) };
            s.push_str(&format!(
                "<style:style style:name=\"{n}\" style:family=\"paragraph\" style:parent-style-name=\"{parent}\"{master}><style:paragraph-properties{props}/></style:style>"
            ));
        }
        for (i, kinds) in self.list_styles.iter().enumerate() {
            s.push_str(&format!("<text:list-style style:name=\"{}L{}\">", self.prefix, i + 1));
            for (l, ordered) in kinds.iter().enumerate() {
                let lv = l + 1;
                let props = format!(
                    "<style:list-level-properties text:list-level-position-and-space-mode=\"label-alignment\"><style:list-level-label-alignment text:label-followed-by=\"listtab\" text:list-tab-stop-position=\"{}\" fo:text-indent=\"-18pt\" fo:margin-left=\"{}\"/></style:list-level-properties>",
                    pt(36.0 * lv as f32),
                    pt(36.0 * lv as f32)
                );
                if *ordered {
                    let fmt = ["1", "a", "i"].get(l % 3).copied().unwrap_or("1");
                    s.push_str(&format!("<text:list-level-style-number text:level=\"{lv}\" style:num-suffix=\".\" style:num-format=\"{fmt}\">{props}</text:list-level-style-number>"));
                } else {
                    let ch = ["•", "◦", "▪"].get(l % 3).copied().unwrap_or("•");
                    s.push_str(&format!(
                        "<text:list-level-style-bullet text:level=\"{lv}\" text:bullet-char=\"{ch}\">{props}</text:list-level-style-bullet>"
                    ));
                }
            }
            s.push_str("</text:list-style>");
        }
        for (i, widths) in self.tables.iter().enumerate() {
            let tn = format!("{}{}", self.prefix, i + 1);
            let total: f32 = widths.iter().sum();
            let (master, number) = match self.table_starts.get(i).and_then(Option::as_ref) {
                Some((m, n)) => {
                    (format!(" style:master-page-name=\"{}\"", x(m)), n.map(|n| format!(" style:page-number=\"{n}\"")).unwrap_or_default())
                }
                None => (String::new(), String::new()),
            };
            s.push_str(&format!(
                "<style:style style:name=\"Table{tn}\" style:family=\"table\"{master}><style:table-properties style:width=\"{}\" table:align=\"margins\"{number}/></style:style>",
                pt(total)
            ));
            for (c, w) in widths.iter().enumerate() {
                s.push_str(&format!(
                    "<style:style style:name=\"Table{tn}.C{}\" style:family=\"table-column\"><style:table-column-properties style:column-width=\"{}\"/></style:style>",
                    c + 1,
                    pt(*w)
                ));
            }
        }
        for (i, sh) in self.cell_styles.iter().enumerate() {
            let bg = sh.map(|c| format!(" fo:background-color=\"#{}\"", c.hex().to_ascii_lowercase())).unwrap_or_default();
            s.push_str(&format!(
                "<style:style style:name=\"{}Cell{}\" style:family=\"table-cell\"><style:table-cell-properties fo:padding=\"4pt\" fo:border=\"0.5pt solid #000000\"{bg}/></style:style>",
                self.prefix,
                i + 1
            ));
        }
        for st in &self.section_styles {
            s.push_str(st);
        }
        s.push_str("</office:automatic-styles>");
        s
    }
}

/// One piece of a paragraph being written: an inline, or the deleted mark of a paragraph
/// joined to the next one.
enum Item<'a> {
    Inl(&'a Inline),
    Mark(&'a Para),
}

impl Item<'_> {
    /// Its tracked insertion and deletion.
    fn marks(&self) -> (Option<&RevMark>, Option<&RevMark>) {
        match self {
            Item::Inl(Inline::Text(_, f)) => (f.ins.as_ref(), f.del.as_ref()),
            Item::Inl(_) => (None, None),
            Item::Mark(p) => (p.mark_ins.as_deref(), p.mark_del.as_deref()),
        }
    }

    /// What a run of content is written as: deleted content is one region whatever was
    /// inserted inside it, so only its deletion counts.
    fn segment(&self) -> (Option<&RevMark>, Option<&RevMark>) {
        match self.marks() {
            (_, Some(del)) => (None, Some(del)),
            k => k,
        }
    }

    fn inline(&self) -> Option<&Inline> {
        match self {
            Item::Inl(i) => Some(i),
            Item::Mark(_) => None,
        }
    }
}

/// The `office:name` of comment `id`'s annotation.
fn annotation_name(id: u32) -> String {
    format!("__Annotation__{id}")
}

/// Close the insertion open in `st`.
fn ins_close(st: &mut Option<(RevMark, String)>, out: &mut String) {
    if let Some((_, ct)) = st.take() {
        out.push_str(&format!("<text:change-end text:change-id=\"{ct}\"/>"));
    }
}

/// `blocks` with each run of paragraphs joined at tracked deleted paragraph marks (ODF keeps
/// such a deletion inside one body paragraph): the first paragraph stands for the run, and the
/// whole run is returned by its index. `None` when nothing is joined. A deleted mark with no
/// paragraph after it (before a table, at the end) is written as a plain mark.
#[allow(clippy::type_complexity)]
fn join_deleted_marks(blocks: &[FBlock]) -> Option<(Vec<FBlock>, HashMap<usize, Vec<Para>>)> {
    if !blocks.iter().any(|b| matches!(b, FBlock::Para(p) if p.mark_del.is_some())) {
        return None;
    }
    let mut out = Vec::with_capacity(blocks.len());
    let mut chains = HashMap::new();
    let mut i = 0;
    while let Some(b) = blocks.get(i) {
        i += 1;
        let FBlock::Para(p) = b else {
            out.push(b.clone());
            continue;
        };
        let mut chain = vec![p.clone()];
        while chain.last().is_some_and(|q| q.mark_del.is_some())
            && let Some(FBlock::Para(q)) = blocks.get(i)
        {
            chain.push(q.clone());
            i += 1;
        }
        if chain.len() > 1 {
            chains.insert(out.len(), chain);
        }
        out.push(FBlock::Para(p.clone()));
    }
    Some((out, chains))
}

/// Now, as an `xsd:dateTime` (UTC).
fn now_iso() -> String {
    let t = now_unix();
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    // Days since 1970-01-01 → civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

/// The fields ODT keeps as fields (ODF 1.3 §7.3–7.5: `text:page-number`, `text:page-count`,
/// `text:date`, `text:time`, `text:author-name`, `text:title`); any other is written as its
/// last result, which a save names as a loss.
pub const KEPT_FIELDS: &[&str] = &["PAGE", "NUMPAGES", "DATE", "TIME", "AUTHOR", "TITLE"];

/// Seconds since the Unix epoch: the system clock, or the browser's on the web.
fn now_unix() -> i64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).ok().and_then(|d| i64::try_from(d.as_secs()).ok()).unwrap_or(0)
    }
    // `SystemTime::now()` panics on wasm32-unknown-unknown, so ask the browser's clock.
    #[cfg(target_arch = "wasm32")]
    {
        let ms = js_sys::Date::now();
        if ms.is_finite() && ms > 0.0 { (ms / 1000.0) as i64 } else { 0 }
    }
}

/// The content of the master page: the headers and footers, in the order ODF 1.3 §16.9 gives
/// (`style:header`, `style:header-left`, `style:header-first`, then the footers). Even pages are
/// ODF's left pages. A first-page or even-page one without a default gets an empty default.
fn master_content(w: &mut Writer, headers: &[model::FHeader]) -> String {
    let mut out = String::new();
    for footer in [false, true] {
        let find = |pages: model::HfPages| headers.iter().find(|h| h.footer == footer && h.pages == pages);
        let parts = [(model::HfPages::Default, ""), (model::HfPages::Even, "-left"), (model::HfPages::First, "-first")];
        if parts.iter().all(|(p, _)| find(*p).is_none()) {
            continue;
        }
        let base = if footer { "style:footer" } else { "style:header" };
        for (pages, suffix) in parts {
            let h = find(pages);
            if h.is_none() && pages != model::HfPages::Default {
                continue;
            }
            out.push_str(&format!("<{base}{suffix}>"));
            match h {
                Some(h) if !h.blocks.is_empty() => {
                    w.blocks(&h.blocks, &mut out, 0);
                    w.close_ins(&mut out);
                }
                _ => out.push_str("<text:p/>"),
            }
            out.push_str(&format!("</{base}{suffix}>"));
        }
    }
    out
}

/// `style:columns` (ODF 1.3 §17.12) of equal columns with their gap and, when on, the line between.
fn columns_xml(c: &Columns) -> String {
    let sep = if c.separator { "<style:column-sep style:width=\"0.5pt\" style:style=\"solid\" style:color=\"#000000\"/>" } else { "" };
    format!("<style:columns fo:column-count=\"{}\" fo:column-gap=\"{}\">{sep}</style:columns>", c.count.clamp(1, 45), pt(c.space))
}

/// One master page per entry of `pages` (name, setup, content: its headers and footers), each
/// with its own page layout `pm1`, `pm2`… (§16.5, §16.9); the first is the default one,
/// "Standard". `auto` holds the header writer's automatic styles.
fn styles_xml(pages: &[(String, wordcraft_doc::SectionProps, String)], body_font: &str, auto: &str) -> String {
    let mut s = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-styles {NS}><office:styles>");
    s.push_str(&format!(
        "<style:default-style style:family=\"paragraph\"><style:paragraph-properties fo:margin-bottom=\"8pt\"/><style:text-properties fo:font-family=\"'{}'\" fo:font-size=\"12pt\"/></style:default-style>",
        x(body_font)
    ));
    s.push_str("<style:style style:name=\"Standard\" style:family=\"paragraph\" style:class=\"text\"/>");
    s.push_str("<style:style style:name=\"Heading\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:class=\"text\"><style:paragraph-properties fo:margin-top=\"12pt\" fo:margin-bottom=\"4pt\" fo:keep-with-next=\"always\"/><style:text-properties fo:color=\"#0f4761\"/></style:style>");
    let sizes = [20.0, 16.0, 14.0, 12.0, 12.0, 11.0];
    for (i, sz) in sizes.iter().enumerate() {
        let n = i + 1;
        s.push_str(&format!(
            "<style:style style:name=\"Heading_20_{n}\" style:display-name=\"Heading {n}\" style:family=\"paragraph\" style:parent-style-name=\"Heading\" style:next-style-name=\"Standard\" style:default-outline-level=\"{n}\" style:class=\"text\"><style:text-properties fo:font-size=\"{}\"/></style:style>",
            pt(*sz)
        ));
    }
    s.push_str("<style:style style:name=\"Title\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:class=\"chapter\"><style:text-properties fo:font-size=\"28pt\"/></style:style>");
    s.push_str("<style:style style:name=\"Quotations\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:class=\"html\"><style:paragraph-properties fo:margin-left=\"36pt\" fo:margin-right=\"36pt\"/><style:text-properties fo:font-style=\"italic\" fo:color=\"#404040\"/></style:style>");
    s.push_str(&format!("<style:style style:name=\"Preformatted_20_Text\" style:display-name=\"Preformatted Text\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:class=\"html\"><style:paragraph-properties fo:margin-top=\"0pt\" fo:margin-bottom=\"0pt\"/><style:text-properties fo:font-family=\"'{}'\" style:font-family-generic=\"modern\" style:font-pitch=\"fixed\" fo:font-size=\"10pt\"/></style:style>", model::MONO_FONT));
    s.push_str("</office:styles><office:automatic-styles>");
    for (i, (_, sec, _)) in pages.iter().enumerate() {
        // A page of several columns carries them in its layout.
        let cols = if sec.columns.count > 1 { columns_xml(&sec.columns) } else { String::new() };
        s.push_str(&format!(
            "<style:page-layout style:name=\"pm{}\"><style:page-layout-properties fo:page-width=\"{}\" fo:page-height=\"{}\" fo:margin-top=\"{}\" fo:margin-bottom=\"{}\" fo:margin-left=\"{}\" fo:margin-right=\"{}\" style:print-orientation=\"{}\">{cols}</style:page-layout-properties></style:page-layout>",
            i + 1,
            pt(sec.page_w),
            pt(sec.page_h),
            pt(sec.margin_top),
            pt(sec.margin_bottom),
            pt(sec.margin_left),
            pt(sec.margin_right),
            if sec.page_w > sec.page_h { "landscape" } else { "portrait" }
        ));
    }
    // The header writer's automatic styles (without their own wrapper).
    s.push_str(auto.trim_start_matches("<office:automatic-styles>").trim_end_matches("</office:automatic-styles>"));
    s.push_str("</office:automatic-styles><office:master-styles>");
    for (i, (name, _, master)) in pages.iter().enumerate() {
        s.push_str(&format!("<style:master-page style:name=\"{}\" style:page-layout-name=\"pm{}\">{master}</style:master-page>", x(name), i + 1));
    }
    s.push_str("</office:master-styles></office:document-styles>\n");
    s
}

fn meta_xml(m: &Meta) -> String {
    let mut s = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-meta {NS}><office:meta><meta:generator>Etch</meta:generator>");
    if !m.title.is_empty() {
        s.push_str(&format!("<dc:title>{}</dc:title>", x(&m.title)));
    }
    if !m.subject.is_empty() {
        s.push_str(&format!("<dc:subject>{}</dc:subject>", x(&m.subject)));
    }
    if !m.description.is_empty() {
        s.push_str(&format!("<dc:description>{}</dc:description>", x(&m.description)));
    }
    if !m.author.is_empty() {
        s.push_str(&format!("<meta:initial-creator>{}</meta:initial-creator><dc:creator>{}</dc:creator>", x(&m.author), x(&m.author)));
    }
    for k in m.keywords.split([',', ';']).map(str::trim).filter(|k| !k.is_empty()) {
        s.push_str(&format!("<meta:keyword>{}</meta:keyword>", x(k)));
    }
    if !m.created.is_empty() {
        s.push_str(&format!("<meta:creation-date>{}</meta:creation-date>", x(m.created.trim_end_matches('Z'))));
    }
    s.push_str("</office:meta></office:document-meta>\n");
    s
}

/// The sections of a flow: each one's setup and the top-level blocks it holds. A paragraph that
/// carries a section's properties ends it; the document's last section follows the last such
/// paragraph (and is left out when nothing follows it, unless it is the only one).
fn flow_sections(blocks: &[FBlock], last: &SectionProps) -> Vec<(SectionProps, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let mut from = 0;
    for (i, b) in blocks.iter().enumerate() {
        if let FBlock::Para(p) = b
            && let Some(s) = &p.section
        {
            out.push(((**s).clone(), from..i + 1));
            from = i + 1;
        }
    }
    if from < blocks.len() || out.is_empty() {
        out.push((last.clone(), from..blocks.len()));
    }
    out
}

/// Write an `.odt` package.
pub fn export(doc: &Document) -> Result<Vec<u8>, String> {
    let flow = model::from_doc_full(doc);
    let mut w = Writer { comments: flow.comments.clone(), ..Default::default() };
    let mut body = String::new();
    let sections = flow_sections(&flow.blocks, &doc.last_section);
    // One master page per section that starts a page; the first one is "Standard". Each keeps
    // the index of its section, for its headers and footers.
    let mut pages: Vec<(String, SectionProps)> = Vec::new();
    let mut page_sections: Vec<usize> = Vec::new();
    // The columns of the page in force: a continuous section with others is a `text:section`.
    let mut page_cols = Columns::default();
    for (k, (sec, range)) in sections.iter().enumerate() {
        let new_page = k == 0 || sec.start != SectionStart::Continuous;
        if new_page {
            let name = if k == 0 { "Standard".to_string() } else { format!("Page{}", pages.len() + 1) };
            pages.push((name.clone(), sec.clone()));
            page_sections.push(k);
            page_cols = sec.columns.clone();
            // The first section needs its paragraph to name the page only to restart numbering.
            if k > 0 || sec.page_num_start.is_some() {
                w.pending_start = Some((name, sec.page_num_start));
            }
        }
        // A continuous section whose columns differ from the page's is a `text:section` (§5.4)
        // with them (one column included). A restart of page numbering isn't written for it: ODF
        // applies `style:page-number` only together with a master page.
        let wrapped = k > 0
            && sec.start == SectionStart::Continuous
            && (sec.columns.count != page_cols.count || (sec.columns.count > 1 && sec.columns != page_cols));
        // A list cut at the boundary goes on numbering in the new section.
        let list_at = |b: Option<&FBlock>| matches!(b, Some(FBlock::Para(p)) if p.list.is_some());
        w.continue_list = k > 0 && list_at(flow.blocks.get(range.start)) && list_at(range.start.checked_sub(1).and_then(|i| flow.blocks.get(i)));
        if wrapped {
            w.close_ins(&mut body);
            let n = w.section_styles.len() + 1;
            w.section_styles.push(format!(
                "<style:style style:name=\"Sect{n}\" style:family=\"section\"><style:section-properties>{}</style:section-properties></style:style>",
                columns_xml(&sec.columns)
            ));
            body.push_str(&format!("<text:section text:style-name=\"Sect{n}\" text:name=\"Section{n}\">"));
        }
        w.blocks(flow.blocks.get(range.clone()).unwrap_or(&[]), &mut body, 0);
        w.pending_start = None;
        w.continue_list = false;
        if wrapped {
            w.close_ins(&mut body);
            body.push_str("</text:section>\n");
        }
    }
    w.close_ins(&mut body);
    // Headers and footers go in the master pages, their styles in styles.xml.
    // A header's section: its index among the sections the paragraphs end, or the last section.
    let mut hw = Writer { prefix: "M", ..Default::default() };
    let total = flow.blocks.iter().filter(|b| matches!(b, FBlock::Para(p) if p.section.is_some())).count() + 1;
    let of = |h: &model::FHeader| h.section.map_or(total - 1, |s| s as usize);
    let last_flow = sections.len().saturating_sub(1);
    let pages: Vec<(String, SectionProps, String)> = pages
        .into_iter()
        .zip(page_sections)
        .map(|((name, sec), k)| {
            // The section's own, then (for the last section written, when the document's last
            // section has nothing in it and isn't written) the last section's.
            let mut mine: Vec<model::FHeader> = flow.headers.iter().filter(|h| of(h) == k).cloned().collect();
            if k == last_flow {
                mine.extend(flow.headers.iter().filter(|h| of(h) > k).cloned());
            }
            let content = master_content(&mut hw, &mine);
            (name, sec, content)
        })
        .collect();
    let tracked = if w.regions.is_empty() && !doc.settings.track_changes {
        String::new()
    } else {
        format!("<text:tracked-changes text:track-changes=\"{}\">{}</text:tracked-changes>\n", doc.settings.track_changes, w.regions.concat())
    };
    let content = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-content {NS}>{}<office:body><office:text>\n{tracked}{body}</office:text></office:body></office:document-content>\n",
        w.automatic_styles()
    );
    let body_font = doc.styles.default_chr.font.clone().unwrap_or_else(|| wordcraft_doc::styles::BODY_FONT.to_string());
    let styles = styles_xml(&pages, &body_font, &hw.automatic_styles());
    let meta = meta_xml(&flow.meta);
    let mut manifest = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\" manifest:version=\"1.3\">\n<manifest:file-entry manifest:full-path=\"/\" manifest:version=\"1.3\" manifest:media-type=\"application/vnd.oasis.opendocument.text\"/>\n<manifest:file-entry manifest:full-path=\"content.xml\" manifest:media-type=\"text/xml\"/>\n<manifest:file-entry manifest:full-path=\"styles.xml\" manifest:media-type=\"text/xml\"/>\n<manifest:file-entry manifest:full-path=\"meta.xml\" manifest:media-type=\"text/xml\"/>\n",
    );
    for (name, _, mime) in w.pictures.iter().chain(&hw.pictures) {
        manifest.push_str(&format!("<manifest:file-entry manifest:full-path=\"{}\" manifest:media-type=\"{mime}\"/>\n", x(name)));
    }
    manifest.push_str("</manifest:manifest>\n");

    let err = |e: zip::result::ZipError| format!("ODT: {e}");
    let ioerr = |e: std::io::Error| format!("ODT: {e}");
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    z.start_file("mimetype", stored).map_err(err)?;
    z.write_all(b"application/vnd.oasis.opendocument.text").map_err(ioerr)?;
    for (name, data) in [
        ("META-INF/manifest.xml", manifest.as_bytes()),
        ("content.xml", content.as_bytes()),
        ("styles.xml", styles.as_bytes()),
        ("meta.xml", meta.as_bytes()),
    ] {
        z.start_file(name, deflated).map_err(err)?;
        z.write_all(data).map_err(ioerr)?;
    }
    for (name, data, _) in w.pictures.iter().chain(&hw.pictures) {
        z.start_file(name.as_str(), stored).map_err(err)?;
        z.write_all(data).map_err(ioerr)?;
    }
    let cur = z.finish().map_err(err)?;
    Ok(cur.into_inner())
}

// ---------------------------------------------------------------------------------------------
// Import

#[derive(Clone, Debug, Default)]
struct FmtDelta {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    sup: Option<bool>,
    sub: Option<bool>,
    color: Option<Rgb>,
    background: Option<Rgb>,
    size: Option<f32>,
    font: Option<String>,
    code: Option<bool>,
}

impl FmtDelta {
    fn apply(&self, f: &mut Fmt) {
        macro_rules! set {
            ($($k:ident),*) => { $( if let Some(v) = self.$k.clone() { f.$k = v; } )* };
        }
        set!(bold, italic, underline, strike, sup, sub, code);
        if self.color.is_some() {
            f.color = self.color;
        }
        if self.background.is_some() {
            f.background = self.background;
        }
        if self.size.is_some() {
            f.size = self.size;
        }
        if let Some(fam) = &self.font {
            f.font = Some(fam.clone());
        }
    }
}

#[derive(Clone, Debug, Default)]
struct StyleInfo {
    parent: Option<String>,
    display: Option<String>,
    auto: bool,
    fmt: FmtDelta,
    align: Option<Align>,
    page_break: bool,
    /// `fo:keep-with-next`: `Some(true)` for "always", `Some(false)` for "auto".
    keep_next: Option<bool>,
    outline: Option<u8>,
    border_bottom: bool,
    /// `style:writing-mode` of the paragraph properties: right to left (`rl-tb`) or not.
    rtl: Option<bool>,
    /// Spacing and indents of the paragraph properties (`fo:margin-top`, `fo:text-indent`…).
    spacing: model::Spacing,
    /// `style:column-width` of a table-column style.
    column_width: Option<f32>,
    /// `fo:background-color` of a table-cell style's `style:table-cell-properties`.
    cell_background: Option<Rgb>,
    /// `style:master-page-name` of a paragraph or table style: its paragraphs (tables) start a
    /// page of that master page. Empty: explicitly none, which stops the lookup up the chain.
    master_page: Option<String>,
    /// `style:page-number` of a paragraph or table style (applied only with a master page): `Some(Some(n))` restarts page numbering at n,
    /// `Some(None)` ("auto") continues it.
    page_number: Option<Option<u32>>,
    /// `style:columns` of a section style.
    columns: Option<Columns>,
}

/// Columns of a `style:columns` element (ODF 1.3 §17.12): count and gap, clamped as the RTF and
/// DOCX readers do; equal widths only (`style:column` widths are not read).
fn columns_of(a: &Attrs) -> Columns {
    let mut c = Columns::default();
    if let Some(n) = get(a, "fo:column-count").and_then(|v| v.trim().parse::<u64>().ok()) {
        c.count = n.clamp(1, 45) as u32;
    }
    if let Some(g) = get(a, "fo:column-gap").and_then(length).filter(|v| *v >= 0.0) {
        c.space = g.min(1584.0);
    }
    c
}

/// A `style:column-sep` (ODF 1.3 §17.13) draws a line between columns unless its style is "none".
fn column_sep(a: &Attrs) -> bool {
    get(a, "style:style") != Some("none")
}

/// Length → points.
fn length(v: &str) -> Option<f32> {
    let v = v.trim();
    let num = |s: &str| s.trim().parse::<f32>().ok().filter(|x| x.is_finite());
    if let Some(n) = v.strip_suffix("pt") {
        num(n)
    } else if let Some(n) = v.strip_suffix("in") {
        num(n).map(|x| x * 72.0)
    } else if let Some(n) = v.strip_suffix("cm") {
        num(n).map(|x| x * 72.0 / 2.54)
    } else if let Some(n) = v.strip_suffix("mm") {
        num(n).map(|x| x * 72.0 / 25.4)
    } else if let Some(n) = v.strip_suffix("pc") {
        num(n).map(|x| x * 12.0)
    } else if let Some(n) = v.strip_suffix("px") {
        num(n).map(|x| x * 0.75)
    } else {
        num(v)
    }
}

type Attrs = Vec<(String, String)>;

fn attrs_of(e: &BytesStart<'_>) -> Attrs {
    let mut v = Vec::new();
    for a in e.attributes().with_checks(false).flatten().take(128) {
        let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let val =
            a.normalized_value(XmlVersion::Explicit1_0).map(|c| c.into_owned()).unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned());
        v.push((k, val));
    }
    v
}

fn get<'a>(a: &'a Attrs, k: &str) -> Option<&'a str> {
    a.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

fn local(q: &str) -> &str {
    q.rsplit(':').next().unwrap_or(q)
}

enum Ev<'a> {
    Start(String, Attrs),
    End(String),
    /// Text as it is in the input where it can be (most text needs no copy, and text the reader
    /// skips is never copied).
    Text(Cow<'a, str>),
}

/// Stream XML events (empty elements become start + end). Malformed XML ends the stream.
fn events<'a>(xml: &'a [u8], mut f: impl FnMut(Ev<'a>)) {
    let mut r = quick_xml::Reader::from_reader(xml);
    let cfg = r.config_mut();
    cfg.trim_text(false);
    cfg.check_end_names = false;
    cfg.allow_unmatched_ends = true;
    loop {
        // Events borrow from the input rather than being copied into a buffer first.
        match r.read_event() {
            Ok(Event::Start(e)) => f(Ev::Start(String::from_utf8_lossy(e.name().as_ref()).into_owned(), attrs_of(&e))),
            Ok(Event::Empty(e)) => {
                let n = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                f(Ev::Start(n.clone(), attrs_of(&e)));
                f(Ev::End(n));
            }
            Ok(Event::End(e)) => f(Ev::End(String::from_utf8_lossy(e.name().as_ref()).into_owned())),
            Ok(Event::Text(t)) => f(Ev::Text(t.decode().unwrap_or_default())),
            Ok(Event::CData(t)) => f(Ev::Text(Cow::Owned(String::from_utf8_lossy(&t).into_owned()))),
            Ok(Event::GeneralRef(r)) => {
                let s: Cow<'a, str> = match &*r {
                    b"amp" => "&".into(),
                    b"lt" => "<".into(),
                    b"gt" => ">".into(),
                    b"quot" => "\"".into(),
                    b"apos" => "'".into(),
                    _ => r.resolve_char_ref().ok().flatten().map(|c| c.to_string()).unwrap_or_default().into(),
                };
                f(Ev::Text(s));
            }
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

#[derive(Default)]
struct Styles {
    map: HashMap<String, StyleInfo>,
    lists: HashMap<String, Vec<bool>>,
    /// Font of `style:default-style` for paragraphs (the document default, ODF 1.3 §16.4).
    default_font: Option<String>,
}

impl Styles {
    fn read(&mut self, xml: &[u8], auto_section_is_auto: bool) {
        let mut in_auto = false;
        let mut cur: Option<(String, StyleInfo)> = None;
        let mut in_default = false;
        let mut list: Option<(String, Vec<bool>)> = None;
        events(xml, |ev| match ev {
            Ev::Start(q, a) => match local(&q) {
                "automatic-styles" => in_auto = true,
                "style" if q.starts_with("style:") => {
                    if let Some(name) = get(&a, "style:name") {
                        let info = StyleInfo {
                            parent: get(&a, "style:parent-style-name").map(String::from),
                            display: get(&a, "style:display-name").map(String::from),
                            auto: in_auto && auto_section_is_auto,
                            outline: get(&a, "style:default-outline-level").and_then(|v| v.trim().parse::<u8>().ok()),
                            master_page: get(&a, "style:master-page-name").map(String::from),
                            ..Default::default()
                        };
                        cur = Some((name.to_string(), info));
                    }
                }
                "default-style" if q.starts_with("style:") => in_default = get(&a, "style:family") == Some("paragraph"),
                "text-properties" if in_default && cur.is_none() => {
                    if let Some(fam) = get(&a, "fo:font-family").or_else(|| get(&a, "style:font-name")) {
                        let fam = fam.split(',').next().unwrap_or("").trim().trim_matches(['\'', '"']);
                        if !fam.is_empty() && !model::is_mono(fam) {
                            self.default_font = Some(fam.to_string());
                        }
                    }
                }
                "text-properties" => {
                    if let Some((_, s)) = &mut cur {
                        let d = &mut s.fmt;
                        if let Some(w) = get(&a, "fo:font-weight") {
                            d.bold = Some(w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600));
                        }
                        if let Some(v) = get(&a, "fo:font-style") {
                            d.italic = Some(v == "italic" || v == "oblique");
                        }
                        if let Some(v) = get(&a, "style:text-underline-style") {
                            d.underline = Some(v != "none");
                        }
                        if let Some(v) = get(&a, "style:text-line-through-style") {
                            d.strike = Some(v != "none");
                        }
                        if let Some(v) = get(&a, "style:text-position") {
                            let v = v.trim();
                            let first = v.split_whitespace().next().unwrap_or("");
                            let pct = first.trim_end_matches('%').parse::<f32>().unwrap_or(0.0);
                            d.sup = Some(first == "super" || pct > 0.0);
                            d.sub = Some(first == "sub" || pct < 0.0);
                        }
                        if let Some(c) = get(&a, "fo:color").and_then(model::parse_color) {
                            d.color = Some(c);
                        }
                        if let Some(c) = get(&a, "fo:background-color").and_then(model::parse_color) {
                            d.background = Some(c);
                        }
                        if let Some(sz) = get(&a, "fo:font-size").and_then(length).filter(|v| *v >= 1.0 && *v <= 1638.0) {
                            d.size = Some(sz);
                        }
                        if let Some(fam) = get(&a, "fo:font-family").or_else(|| get(&a, "style:font-name")) {
                            let fam = fam.split(',').next().unwrap_or("").trim().trim_matches(['\'', '"']).to_string();
                            if model::is_mono(&fam) || get(&a, "style:font-pitch") == Some("fixed") {
                                d.code = Some(true);
                            } else if !fam.is_empty() {
                                d.font = Some(fam);
                            }
                        }
                    }
                }
                "table-properties" => {
                    if let Some((_, s)) = &mut cur
                        && let Some(v) = get(&a, "style:page-number")
                    {
                        s.page_number = Some(v.trim().parse::<u32>().ok().map(|n| n.min(32_767)));
                    }
                }
                "table-cell-properties" => {
                    if let Some((_, s)) = &mut cur
                        && let Some(c) = get(&a, "fo:background-color").and_then(model::parse_color)
                    {
                        s.cell_background = Some(c);
                    }
                }
                "table-column-properties" => {
                    if let Some((_, s)) = &mut cur {
                        s.column_width = get(&a, "style:column-width").and_then(length).filter(|w| *w > 0.0);
                    }
                }
                "paragraph-properties" => {
                    if let Some((_, s)) = &mut cur {
                        s.align = match get(&a, "fo:text-align") {
                            Some("center") => Some(Align::Center),
                            Some("end") | Some("right") => Some(Align::Right),
                            Some("justify") => Some(Align::Justify),
                            Some("start") | Some("left") => Some(Align::Left),
                            _ => s.align,
                        };
                        if get(&a, "fo:break-before") == Some("page") {
                            s.page_break = true;
                        }
                        match get(&a, "fo:keep-with-next") {
                            Some("always") => s.keep_next = Some(true),
                            Some("auto") => s.keep_next = Some(false),
                            _ => {}
                        }
                        match get(&a, "style:writing-mode").map(str::trim) {
                            Some("rl-tb") | Some("rl") | Some("tb-rl") => s.rtl = Some(true),
                            Some("lr-tb") | Some("lr") | Some("tb-lr") => s.rtl = Some(false),
                            _ => {}
                        }
                        let len = |k: &str| get(&a, k).and_then(length).filter(|v| v.is_finite()).map(|v| v.clamp(-1584.0, 1584.0));
                        s.spacing.before = len("fo:margin-top").map(|v| v.max(0.0));
                        s.spacing.after = len("fo:margin-bottom").map(|v| v.max(0.0));
                        s.spacing.left = len("fo:margin-left");
                        s.spacing.right = len("fo:margin-right");
                        s.spacing.first = len("fo:text-indent");
                        s.spacing.line = match get(&a, "fo:line-height").map(str::trim) {
                            Some(v) if v.ends_with('%') => v
                                .trim_end_matches('%')
                                .trim()
                                .parse::<f32>()
                                .ok()
                                .filter(|p| p.is_finite() && *p > 0.0)
                                .map(|p| wordcraft_doc::props::LineSpacing::Multiple((p / 100.0).clamp(0.06, 132.0))),
                            Some("normal") | None => get(&a, "style:line-height-at-least")
                                .and_then(length)
                                .filter(|v| *v > 0.0)
                                .map(|v| wordcraft_doc::props::LineSpacing::AtLeast(v.clamp(1.0, 1584.0))),
                            Some(v) => length(v).filter(|v| *v > 0.0).map(|v| wordcraft_doc::props::LineSpacing::Exactly(v.clamp(1.0, 1584.0))),
                        };
                        if let Some(v) = get(&a, "style:page-number") {
                            s.page_number = Some(v.trim().parse::<u32>().ok().map(|n| n.min(32_767)));
                        }
                        if get(&a, "fo:border-bottom").is_some_and(|b| b != "none") {
                            s.border_bottom = true;
                        }
                    }
                }
                "columns" if q.starts_with("style:") => {
                    if let Some((_, s)) = &mut cur {
                        s.columns = Some(columns_of(&a));
                    }
                }
                "column-sep" => {
                    if let Some((_, s)) = &mut cur
                        && let Some(c) = &mut s.columns
                    {
                        c.separator = column_sep(&a);
                    }
                }
                "list-style" => list = get(&a, "style:name").map(|n| (n.to_string(), vec![false; 10])),
                "list-level-style-number" | "list-level-style-bullet" | "list-level-style-image" => {
                    if let Some((_, v)) = &mut list {
                        let lv = get(&a, "text:level").and_then(|l| l.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 10);
                        let ordered = local(&q) == "list-level-style-number" && get(&a, "style:num-format").is_some_and(|f| !f.is_empty());
                        if let Some(slot) = v.get_mut(lv - 1) {
                            *slot = ordered;
                        }
                    }
                }
                _ => {}
            },
            Ev::End(q) => match local(&q) {
                "automatic-styles" => in_auto = false,
                "default-style" if q.starts_with("style:") => in_default = false,
                "style" if q.starts_with("style:") => {
                    if let Some((n, s)) = cur.take() {
                        if self.map.len() < 100_000 || self.map.contains_key(&n) {
                            self.map.insert(n, s);
                        } else {
                            refuse(|| "more than 100,000 styles".to_string());
                        }
                    }
                }
                "list-style" => {
                    if let Some((n, v)) = list.take() {
                        if self.lists.len() < MAX_LIST_STYLES || self.lists.contains_key(&n) {
                            self.lists.insert(n, v);
                        } else {
                            refuse(|| format!("more than {MAX_LIST_STYLES} list styles"));
                        }
                    }
                }
                _ => {}
            },
            Ev::Text(_) => {}
        });
    }

    /// The document's default font: the Standard paragraph style's, else the default style's.
    fn doc_font(&self) -> Option<String> {
        self.map.get("Standard").filter(|s| !s.auto).and_then(|s| s.fmt.font.clone()).or_else(|| self.default_font.clone())
    }

    fn chain<'a>(&'a self, name: &'a str) -> Vec<(&'a str, &'a StyleInfo)> {
        let mut out = Vec::new();
        let mut cur = Some(name);
        while let Some(n) = cur {
            if out.len() >= 16 {
                break;
            }
            let Some(s) = self.map.get(n) else { break };
            if out.iter().any(|(m, _): &(&str, &StyleInfo)| *m == n) {
                break;
            }
            out.push((n, s));
            cur = s.parent.as_deref();
        }
        out
    }

    /// Text formatting of a text style (whole chain).
    fn text_fmt(&self, name: &str, base: &Fmt) -> Fmt {
        let mut f = base.clone();
        for (_, s) in self.chain(name).iter().rev() {
            s.fmt.apply(&mut f);
        }
        f
    }

    /// The page a paragraph style starts (`style:master-page-name` and `style:page-number`,
    /// ODF 1.3 §16.2 and §17.6): the nearest master page and page-number setting in its chain.
    fn page_start<'a>(&'a self, name: &'a str) -> (Option<&'a str>, Option<Option<u32>>) {
        let chain = self.chain(name);
        let master = chain.iter().find_map(|(_, s)| s.master_page.as_deref()).filter(|m| !m.is_empty());
        (master, chain.iter().find_map(|(_, s)| s.page_number))
    }

    /// Columns of a section style (whole chain).
    fn columns(&self, name: &str) -> Option<Columns> {
        self.chain(name).iter().find_map(|(_, s)| s.columns.clone())
    }

    /// Background colour of a table-cell style (whole chain).
    fn cell_background(&self, name: &str) -> Option<Rgb> {
        self.chain(name).iter().find_map(|(_, s)| s.cell_background)
    }

    /// Paragraph kind, alignment, page break, rule, keep-with-next and base formatting. The text formatting of
    /// every style in the chain applies (automatic and common, ODF 1.3 §16.2 inheritance), except
    /// the style that gives the paragraph its kind and that style's ancestors: their formatting
    /// is the kind's own look (a heading's size and colour), not direct formatting. `Standard`,
    /// the default paragraph style, holds document defaults and is left out too.
    /// The width of table-column style `name`, when it has one.
    fn column_width(&self, name: &str) -> Option<f32> {
        self.map.get(name)?.column_width
    }

    fn para(&self, name: &str) -> (Kind, Option<Align>, bool, bool, bool, Fmt, bool, model::Spacing) {
        let chain = self.chain(name);
        let mut kind = Kind::Normal;
        let mut kind_at = chain.len();
        for (i, (n, s)) in chain.iter().enumerate() {
            kind_at = i;
            let dn = s.display.as_deref().unwrap_or(n).to_ascii_lowercase().replace("_20_", " ");
            let nn = n.to_ascii_lowercase().replace("_20_", " ");
            for cand in [dn.as_str(), nn.as_str()] {
                if let Some(d) = cand.strip_prefix("heading ").and_then(|r| r.trim().parse::<u8>().ok()) {
                    kind = Kind::Heading(d.clamp(1, 6));
                } else if cand == "title" {
                    kind = Kind::Title;
                } else if cand == "quotations" || cand == "quote" {
                    kind = Kind::Quote;
                } else if cand == "preformatted text" || cand == "source text" || cand == "code" {
                    kind = Kind::Code;
                }
            }
            if kind != Kind::Normal {
                break;
            }
            if let Some(o) = s.outline.filter(|o| (1..=10).contains(o)) {
                kind = Kind::Heading(o.min(6));
                break;
            }
            kind_at = chain.len();
        }
        let align = chain.iter().find_map(|(_, s)| s.align);
        let pb = chain.iter().take_while(|(_, s)| s.auto).any(|(_, s)| s.page_break);
        let rule = chain.iter().any(|(_, s)| s.border_bottom);
        let rtl = chain.iter().find_map(|(_, s)| s.rtl).unwrap_or(false);
        let mut f = Fmt::default();
        for (_, s) in chain.iter().take(kind_at).rev().filter(|(n, s)| s.auto || *n != "Standard") {
            s.fmt.apply(&mut f);
        }
        // Like the text formatting, the kind style's own `fo:keep-with-next` (a heading's) is the kind's look, not direct formatting.
        let keep_next = chain.iter().take(kind_at).find_map(|(_, s)| s.keep_next).unwrap_or(false);
        // Spacing and indents of the automatic styles (direct formatting); a named style's are
        // its own look.
        let mut spacing = model::Spacing::default();
        // Innermost first: the first value found wins.
        for (_, s) in chain.iter().take_while(|(_, s)| s.auto) {
            let sp = &s.spacing;
            spacing.before = spacing.before.or(sp.before);
            spacing.after = spacing.after.or(sp.after);
            spacing.left = spacing.left.or(sp.left);
            spacing.right = spacing.right.or(sp.right);
            spacing.first = spacing.first.or(sp.first);
            spacing.line = spacing.line.or(sp.line);
        }
        (kind, align, pb, rule, keep_next, f, rtl, spacing)
    }
}

struct TableB {
    rows: Vec<Vec<Cell>>,
    row: Vec<Cell>,
    /// Covered cells still owed to the previous cell's column span.
    hcover: usize,
    /// Column widths from `table:table-column` elements (points).
    widths: Vec<f32>,
}

/// A `text:changed-region` read from `text:tracked-changes`.
#[derive(Default)]
struct Region {
    kind: Option<RevisionKind>,
    /// Sequence number: tells regions by the same author at the same time apart.
    id: u32,
    author: String,
    date: String,
    /// A deletion's deleted paragraphs.
    blocks: Vec<FBlock>,
}

impl Region {
    fn mark(&self) -> RevMark {
        RevMark { author: self.author.trim().to_string(), date: self.date.trim().to_string(), id: Some(self.id) }
    }
}

/// Reader state saved while a deletion's content is read into its own container.
struct DelState {
    containers: usize,
    tables: usize,
    cells: usize,
    lists_saved: usize,
    fmt: usize,
    lists: Vec<(Option<String>, bool)>,
    item_used: Vec<bool>,
    /// Insertions open inside the deleted content (text inserted, then deleted).
    ins: Vec<(String, RevMark)>,
}

struct Body<'a> {
    styles: &'a Styles,
    zip: &'a mut dyn FnMut(&str) -> Option<Vec<u8>>,
    containers: Vec<Vec<FBlock>>,
    fmt: Vec<Fmt>,
    para: Option<Para>,
    /// The text read last ended in a space collapsed from XML whitespace (not a `text:s`).
    xml_space_end: bool,
    para_cont: bool,
    /// Open lists: (style name, item open, item used).
    lists: Vec<(Option<String>, bool)>,
    item_used: Vec<bool>,
    /// The id of each open list ([`ListInfo::list`]), with `lists`.
    list_ids: Vec<u32>,
    /// Lists read so far (their ids); the last top-level list closed (`text:continue-numbering`
    /// goes on with it); `text:start-value` of the open item, for its first paragraph.
    lists_seen: u32,
    last_list: Option<u32>,
    item_start: Option<u32>,
    /// Lists outside the table cells being read.
    lists_saved: Vec<(Vec<(Option<String>, bool)>, Vec<bool>, Vec<u32>)>,
    tables: Vec<TableB>,
    cells: Vec<Cell>,
    in_text: bool,
    skip: usize,
    depth: usize,
    frame: Option<(Option<f32>, Option<f32>, Option<Vec<u8>>, String)>,
    in_desc: bool,
    /// The base64 of a frame's inline picture (`office:binary-data`, flat documents) being read,
    /// and the bytes still allowed to inline pictures together ([`MAX_IMAGES`]).
    binary: Option<String>,
    inline_left: u64,
    /// Text bytes still allowed into the flow ([`MAX_TEXT`]); once more comes the file is
    /// refused (`too_much_text`).
    text_left: usize,
    too_much_text: bool,
    /// Paragraphs (and empty table cells, which get one) still allowed
    /// ([`wordcraft_doc::MAX_PARAGRAPHS`]); once spent no more are made and the file is refused.
    paras_left: usize,
    too_many_paras: bool,
    /// The master pages' page setups.
    pages: &'a Pages,
    /// The current master page's setup (a column section's page, without its columns).
    page: SectionProps,
    /// The open section; where its blocks start in the body; how many sections have ended.
    sec: SectionProps,
    sec_from: usize,
    sections: usize,
    /// Element depth of the open columned `text:section`.
    cols_at: Option<usize>,
    /// Inside `text:tracked-changes`.
    in_tc: bool,
    /// `text:track-changes` of the document's tracked changes (default on), when it has them.
    track: Option<bool>,
    /// Changed regions read, by id.
    regions: HashMap<String, Region>,
    /// Regions seen (capped at [`MAX_REGIONS`]).
    region_count: usize,
    /// The region being read.
    region: Option<(String, Region)>,
    /// Depth of the `office:change-info` being read, and the field its text goes to.
    info_at: Option<usize>,
    info_field: Option<bool>,
    /// Reading a deletion's content.
    del: Option<DelState>,
    /// Insertions open in the body: (region id, revision).
    active_ins: Vec<(String, RevMark)>,
    /// Notes, annotations and text boxes being read, innermost last.
    stories: Vec<StorySave>,
    /// Comments read (annotations), by id; annotation names → ids, for `office:annotation-end`.
    comments: std::collections::BTreeMap<u32, model::FComment>,
    ann_names: HashMap<String, u32>,
    /// Reading an annotation's author (0), date (1) or initials (2).
    ann_field: Option<u8>,
    /// A page number or page count field being read: its instruction and shown text.
    field: Option<(&'static str, String)>,
    /// The master page of the open section, of the one a paragraph is about to start, and of
    /// each section ended so far.
    cur_master: Option<String>,
    next_master: Option<String>,
    section_masters: Vec<Option<String>>,
}

/// A note, annotation or text box being read inside a paragraph.
enum StoryKind {
    Note { endnote: bool, custom: String },
    Comment(u32),
    TextBox { w: f32, h: f32 },
}

/// What a note, annotation or text box sets aside of the paragraph around it.
struct StorySave {
    kind: StoryKind,
    /// Element depth of the story's element.
    depth: usize,
    para: Option<Para>,
    para_cont: bool,
    fmt: Vec<Fmt>,
    lists: Vec<(Option<String>, bool)>,
    item_used: Vec<bool>,
    list_ids: Vec<u32>,
    frame: Option<(Option<f32>, Option<f32>, Option<Vec<u8>>, String)>,
}

/// Most notes, annotations and text boxes inside one another.
const MAX_STORY_DEPTH: usize = 2;

impl Body<'_> {
    /// Start reading a note, annotation or text box: the paragraph around it waits.
    fn begin_story(&mut self, kind: StoryKind) {
        let save = StorySave {
            kind,
            depth: self.depth,
            para: self.para.take(),
            para_cont: std::mem::take(&mut self.para_cont),
            fmt: std::mem::take(&mut self.fmt),
            lists: std::mem::take(&mut self.lists),
            item_used: std::mem::take(&mut self.item_used),
            list_ids: std::mem::take(&mut self.list_ids),
            frame: self.frame.take(),
        };
        self.containers.push(Vec::new());
        self.stories.push(save);
    }

    /// The note, annotation or text box ends: it goes into the paragraph around it.
    fn end_story(&mut self) {
        self.end_para();
        let Some(save) = self.stories.pop() else { return };
        let blocks = if self.containers.len() > 1 { self.containers.pop().unwrap_or_default() } else { Vec::new() };
        self.para = save.para;
        self.para_cont = save.para_cont;
        self.fmt = save.fmt;
        self.lists = save.lists;
        self.item_used = save.item_used;
        self.list_ids = save.list_ids;
        self.frame = save.frame;
        let inline = match save.kind {
            StoryKind::Note { endnote, custom } => Inline::Note(Box::new(model::Note { endnote, custom, blocks })),
            StoryKind::Comment(id) => {
                if let Some(c) = self.comments.get_mut(&id) {
                    c.blocks = blocks;
                }
                Inline::CommentStart(id)
            }
            StoryKind::TextBox { w, h } => {
                if blocks.iter().all(|b| matches!(b, FBlock::Para(p) if p.is_empty())) {
                    return;
                }
                Inline::TextBox(Box::new(model::TextBox { w, h, blocks }))
            }
        };
        self.push_inline(inline);
    }

    /// An inline object at the end of the paragraph being read (one starts if needed).
    fn push_inline(&mut self, i: Inline) {
        if self.para.is_none() {
            self.start_para(None, None);
        }
        if let Some(p) = &mut self.para {
            p.inlines.push(i);
        }
    }

    fn container(&mut self) -> &mut Vec<FBlock> {
        if self.containers.is_empty() {
            self.containers.push(Vec::new());
        }
        let n = self.containers.len() - 1;
        &mut self.containers[n]
    }

    fn cur_fmt(&self) -> Fmt {
        self.fmt.last().cloned().unwrap_or_default()
    }

    /// Reading the body itself, not a table cell or a frame.
    fn in_body(&self) -> bool {
        self.containers.len() == 1 && self.tables.is_empty() && self.frame.is_none()
    }

    /// End the open section before what comes next, which starts section `next`. An open section
    /// with nothing in it yet takes `next`'s place instead. Only in the body, and at most
    /// [`MAX_SECTIONS`]. True when what comes next follows a section break.
    fn break_section(&mut self, next: SectionProps) -> bool {
        let next_master = self.next_master.take();
        if !self.in_body() {
            return false;
        }
        let Some(body) = self.containers.first_mut() else { return false };
        if body.len() <= self.sec_from {
            let restart = self.sec.page_num_start;
            if next_master.is_some() {
                self.cur_master = next_master;
            }
            self.sec = next;
            self.sec.page_num_start = self.sec.page_num_start.or(restart);
            return self.sections > 0;
        }
        if self.sections + 1 >= MAX_SECTIONS {
            refuse(|| format!("more than {MAX_SECTIONS} sections"));
            return false;
        }
        // A section ends on a paragraph: after a table, an empty one (as Word writes it).
        if !matches!(body.last(), Some(FBlock::Para(p)) if p.section.is_none()) {
            body.push(FBlock::Para(Para::default()));
        }
        if let Some(FBlock::Para(p)) = body.last_mut() {
            p.section = Some(Box::new(std::mem::replace(&mut self.sec, next)));
        }
        self.sec_from = body.len();
        self.sections += 1;
        self.section_masters.push(self.cur_master.clone());
        if next_master.is_some() {
            self.cur_master = next_master;
        }
        true
    }

    /// A body paragraph or table of style `name` that names a master page starts a new page with
    /// that page's setup and, when the style says so, page numbering restarted (ODF 1.3 applies
    /// `style:page-number` only with a master page). The section that starts there, if any.
    ///
    /// Rare cases left simple: a master-page paragraph first inside a columned `text:section`
    /// replaces that (still empty) column section, so the columns are lost; a columned section
    /// nested in another is read as part of the outer one.
    fn page_section(&mut self, name: &str) -> Option<SectionProps> {
        if !self.in_body() {
            return None;
        }
        let (master, number) = self.styles.page_start(name);
        let restart = number.flatten();
        let page = master.and_then(|m| self.pages.masters.get(m))?.clone();
        self.next_master = master.map(String::from);
        self.page = page.clone();
        Some(SectionProps { start: SectionStart::NextPage, page_num_start: restart, ..page })
    }

    /// Count one more paragraph against the budget; false once it is spent.
    fn take_para(&mut self) -> bool {
        match self.paras_left.checked_sub(1) {
            Some(left) => {
                self.paras_left = left;
                true
            }
            None => {
                self.too_many_paras = true;
                false
            }
        }
    }

    fn start_para(&mut self, style: Option<&str>, heading: Option<u8>) {
        self.end_para();
        if !self.take_para() {
            return;
        }
        let (mut kind, align, mut pb, rule, keep_next, base, rtl, spacing) =
            style.map(|s| self.styles.para(s)).unwrap_or((Kind::Normal, None, false, false, false, Fmt::default(), false, Default::default()));
        // The section break starts the new page; no page break besides.
        if let Some(next) = style.and_then(|s| self.page_section(s))
            && self.break_section(next)
        {
            pb = false;
        }
        if let Some(h) = heading {
            kind = Kind::Heading(h.clamp(1, 6));
        }
        let mut p = Para { kind, align, page_break: pb, keep_next, rtl, spacing, ..Default::default() };
        if rule && kind == Kind::Normal {
            p.kind = Kind::Rule;
        }
        if let Some(lv) = self.lists.len().checked_sub(1)
            && self.lists.last().is_some_and(|l| l.1)
        {
            let style = self.lists.iter().rev().find_map(|l| l.0.clone());
            let ordered = style.and_then(|s| self.styles.lists.get(&s)).and_then(|v| v.get(lv.min(9)).copied()).unwrap_or(false);
            // The item's own start goes to its first paragraph.
            let start = self.item_start.take().filter(|_| ordered);
            p.list = Some(ListInfo { ordered, level: lv.min(8) as u8, list: self.list_ids.first().copied(), start });
            self.para_cont = self.item_used.last().copied().unwrap_or(false);
            if let Some(u) = self.item_used.last_mut() {
                *u = true;
            }
        } else {
            self.para_cont = false;
        }
        self.fmt.push(base);
        self.para = Some(p);
    }

    fn end_para(&mut self) {
        let Some(mut p) = self.para.take() else { return };
        self.fmt.pop();
        // A paragraph ending inside an open insertion has an inserted mark. (Paragraphs that
        // `place_deletion` splits off keep only the marks read inside their region.)
        if p.mark_ins.is_none() {
            p.mark_ins = self.open_insertion().map(Box::new);
        }
        self.finish_para(p);
    }

    /// The innermost insertion open where text is being read (the body, or deleted content).
    fn open_insertion(&self) -> Option<RevMark> {
        let open = match &self.del {
            Some(d) => &d.ins,
            None => &self.active_ins,
        };
        open.last().map(|(_, m)| m.clone())
    }

    /// Add a finished paragraph to the current container.
    fn finish_para(&mut self, mut p: Para) {
        // Spaces next to tracked text are part of a change.
        let tracked = self.del.is_some()
            || p.mark_ins.is_some()
            || p.mark_del.is_some()
            || p.inlines.iter().any(|i| matches!(i, Inline::Text(_, f) if f.ins.is_some() || f.del.is_some()));
        // Leading XML whitespace is never read (see `text`); a trailing one is trimmed here.
        // Explicit `text:s` spaces and tabs stay.
        let xml_space_end = std::mem::take(&mut self.xml_space_end);
        if p.kind != Kind::Code
            && !tracked
            && xml_space_end
            && let Some(Inline::Text(t, _)) = p.inlines.last_mut()
            && t.ends_with(' ')
        {
            t.pop();
            if t.is_empty() {
                p.inlines.pop();
            }
        }
        if p.kind == Kind::Rule && !p.is_empty() {
            p.kind = Kind::Normal;
        }
        if matches!(p.kind, Kind::Code | Kind::Heading(_) | Kind::Title) {
            for i in &mut p.inlines {
                if let Inline::Text(_, f) = i {
                    if p.kind == Kind::Code {
                        f.code = false;
                        f.font = None;
                    }
                    f.size = None;
                }
            }
        }
        let cont = std::mem::take(&mut self.para_cont);
        let level = p.list.map(|l| l.level);
        let c = self.container();
        if cont
            && let Some(FBlock::Para(prev)) = c.last_mut()
            && prev.list.is_some()
            && prev.list.map(|l| l.level) == level
        {
            prev.push_text("\n", &Fmt::default());
            prev.inlines.extend(p.inlines);
            return;
        }
        c.push(FBlock::Para(p));
    }

    fn push_text(&mut self, t: &str) {
        if t.len() > self.text_left {
            self.text_left = 0;
            self.too_much_text = true;
            return;
        }
        self.text_left -= t.len();
        let mut f = self.cur_fmt();
        f.ins = self.open_insertion();
        if self.para.is_none() {
            if t.trim().is_empty() {
                return;
            }
            self.start_para(None, None);
        }
        self.xml_space_end = false;
        if let Some(p) = &mut self.para {
            p.push_text(t, &f);
        }
    }

    fn text(&mut self, t: &str) {
        if self.skip > 0 || !self.in_text {
            return;
        }
        if let Some(b) = &mut self.binary {
            // Base64 is a third longer than the bytes it holds; past the budget nothing more is
            // kept (the picture is then left out).
            let cap = usize::try_from(self.inline_left.saturating_mul(4) / 3 + 4).unwrap_or(usize::MAX);
            let t = t.trim();
            if b.len().saturating_add(t.len()) <= cap {
                b.push_str(t);
            } else {
                b.clear();
                self.binary = None;
                self.inline_left = 0;
            }
            return;
        }
        if self.info_at.is_some() {
            if let (Some(field), Some((_, r))) = (self.info_field, &mut self.region) {
                let s = if field { &mut r.author } else { &mut r.date };
                extend_capped(s, t, MAX_INFO_CHARS, "a tracked change's author or date");
            }
            return;
        }
        if self.in_tc && self.del.is_none() {
            return;
        }
        if let Some(k) = self.ann_field {
            if let Some(c) = self
                .stories
                .last()
                .and_then(|st| if let StoryKind::Comment(id) = st.kind { Some(id) } else { None })
                .and_then(|id| self.comments.get_mut(&id))
            {
                let s = match k {
                    0 => &mut c.author,
                    1 => &mut c.date,
                    _ => &mut c.initials,
                };
                extend_capped(s, t, MAX_INFO_CHARS, "a comment's author, date or initials");
            }
            return;
        }
        if let Some((_, r)) = &mut self.field {
            extend_capped(r, t, MAX_INFO_CHARS, "a field's result");
            return;
        }
        if self.in_desc {
            if let Some(fr) = &mut self.frame {
                fr.3.push_str(t);
            }
            return;
        }
        let Some(p) = &self.para else {
            return;
        };
        // Collapse XML whitespace.
        let mut prev_space = match p.inlines.last() {
            Some(Inline::Text(x, _)) => x.ends_with([' ', '\n', '\t']),
            Some(_) => false,
            None => true,
        };
        let mut s = String::with_capacity(t.len());
        for c in t.chars() {
            if matches!(c, ' ' | '\t' | '\n' | '\r') {
                if !prev_space {
                    s.push(' ');
                    prev_space = true;
                }
            } else {
                s.push(c);
                prev_space = false;
            }
        }
        if !s.is_empty() {
            self.push_text(&s);
            self.xml_space_end = s.ends_with(' ');
        }
    }

    fn start(&mut self, q: &str, a: &Attrs) {
        self.depth += 1;
        if q == "office:text" {
            self.in_text = true;
            return;
        }
        if self.skip > 0 {
            self.skip += 1;
            return;
        }
        if !self.in_text {
            return;
        }
        if self.info_at.is_some() {
            self.info_field = match local(q) {
                "creator" => Some(true),
                "date" => Some(false),
                _ => None,
            };
            return;
        }
        if self.in_tc {
            if self.del.is_none() {
                self.tracked_start(q, a);
                return;
            }
            // Inside deleted content: only insertions (text inserted, then deleted) nest.
            if matches!(local(q), "change-start" | "change-end") {
                let regions = &self.regions;
                if let Some(d) = &mut self.del {
                    change_mark(regions, &mut d.ins, local(q) == "change-start", a);
                }
                return;
            }
            if matches!(local(q), "tracked-changes" | "changed-region" | "insertion" | "deletion" | "format-change" | "change") {
                self.skip = 1;
                return;
            }
            if local(q) == "change-info" && self.region.is_some() {
                self.info_at = Some(self.depth);
                return;
            }
        }
        match local(q) {
            "tracked-changes" => {
                if self.para.is_none() && self.tables.is_empty() && self.cells.is_empty() && self.frame.is_none() {
                    self.in_tc = true;
                    self.track = Some(get(a, "text:track-changes") != Some("false"));
                } else {
                    self.skip = 1;
                }
            }
            "change" => {
                if let Some(id) = get(a, "text:change-id")
                    && self.regions.get(id).is_some_and(|r| r.kind == Some(RevisionKind::Delete))
                    && let Some(r) = self.regions.remove(id)
                {
                    let mark = r.mark();
                    self.place_deletion(r.blocks, mark);
                }
            }
            "change-start" | "change-end" => change_mark(&self.regions, &mut self.active_ins, local(q) == "change-start", a),
            // Notes (ODF 1.3 §6.3), annotations (§14.1) and text boxes (§10.4.4), read into
            // their own blocks; not inside one another past MAX_STORY_DEPTH (the file is refused),
            // nor in deletions.
            "note" | "annotation" | "text-box"
                if (q.starts_with("text:") || q.starts_with("office:") || q.starts_with("draw:"))
                    && self.stories.len() >= MAX_STORY_DEPTH
                    && self.del.is_none()
                    && !self.in_tc =>
            {
                refuse(|| format!("notes, comments or text boxes inside one another more than {MAX_STORY_DEPTH} deep"));
            }
            "annotation" if q.starts_with("office:") && self.comments.len() >= model::MAX_COMMENTS && self.del.is_none() && !self.in_tc => {
                refuse(|| format!("more than {} comments", model::MAX_COMMENTS));
            }
            "note" if q.starts_with("text:") && self.stories.len() < MAX_STORY_DEPTH && self.del.is_none() && !self.in_tc => {
                let endnote = get(a, "text:note-class") == Some("endnote");
                self.begin_story(StoryKind::Note { endnote, custom: String::new() });
            }
            "note-citation" => {
                if let Some(label) = get(a, "text:label").map(str::trim).filter(|l| !l.is_empty())
                    && let Some(StorySave { kind: StoryKind::Note { custom, .. }, .. }) = self.stories.last_mut()
                {
                    *custom = String::new();
                    extend_capped(custom, label, model::MAX_NOTE_MARK, "a note's own mark");
                }
                self.skip = 1;
            }
            "annotation"
                if q.starts_with("office:")
                    && self.stories.len() < MAX_STORY_DEPTH
                    && self.del.is_none()
                    && !self.in_tc
                    && self.comments.len() < model::MAX_COMMENTS =>
            {
                let id = u32::try_from(self.comments.len()).unwrap_or(u32::MAX);
                if let Some(n) = get(a, "office:name") {
                    self.ann_names.entry(n.to_string()).or_insert(id);
                }
                self.comments.insert(id, model::FComment::default());
                self.begin_story(StoryKind::Comment(id));
            }
            "creator" | "date" | "creator-initials" if matches!(self.stories.last(), Some(StorySave { kind: StoryKind::Comment(_), depth, .. }) if *depth + 1 == self.depth) =>
            {
                self.ann_field = Some(match local(q) {
                    "creator" => 0,
                    "date" => 1,
                    _ => 2,
                });
            }
            "annotation-end" if q.starts_with("office:") => {
                if let Some(id) = get(a, "office:name").and_then(|n| self.ann_names.get(n)).copied() {
                    self.push_inline(Inline::CommentEnd(id));
                }
            }
            "text-box" if q.starts_with("draw:") && self.stories.len() < MAX_STORY_DEPTH && self.del.is_none() && !self.in_tc => {
                let (w, h) = self.frame.as_ref().map(|f| (f.0.unwrap_or(144.0), f.1.unwrap_or(72.0))).unwrap_or((144.0, 72.0));
                self.begin_story(StoryKind::TextBox { w, h });
            }
            // A rectangle or custom shape holding text (as another program writes a text box):
            // its text is read as a text box; one without text is left out, as other shapes are.
            "custom-shape" | "rect" if q.starts_with("draw:") && self.stories.len() < MAX_STORY_DEPTH && self.del.is_none() && !self.in_tc => {
                let w = get(a, "svg:width").and_then(length).unwrap_or(144.0);
                let h = get(a, "svg:height").and_then(length).unwrap_or(72.0);
                self.begin_story(StoryKind::TextBox { w, h });
            }
            "page-number" | "page-count" | "date" | "time" | "author-name" | "title" if q.starts_with("text:") && self.field.is_none() => {
                let instr = match local(q) {
                    "page-number" => "PAGE",
                    "page-count" => "NUMPAGES",
                    "date" => "DATE",
                    "time" => "TIME",
                    "author-name" => "AUTHOR",
                    _ => "TITLE",
                };
                self.field = Some((instr, String::new()));
            }
            "note"
            | "annotation"
            | "text-box"
            | "sequence-decls"
            | "forms"
            | "variable-decls"
            | "user-field-decls"
            | "index-title-template"
            | "table-of-content-source" => {
                self.skip = 1;
            }
            // Drawing shapes (ODF 1.3 §10.3), with their text, until shapes are read (#1654).
            // Groups are walked, so the pictures in them still are.
            "rect" | "line" | "polyline" | "polygon" | "regular-polygon" | "path" | "circle" | "ellipse" | "connector" | "caption" | "measure"
            | "custom-shape"
                if q.starts_with("draw:") =>
            {
                self.skip = 1;
            }
            "p" | "h" if !self.in_desc => {
                let h =
                    if local(q) == "h" { Some(get(a, "text:outline-level").and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(1)) } else { None };
                self.start_para(get(a, "text:style-name"), h);
            }
            "span" => {
                let base = self.cur_fmt();
                let f = match get(a, "text:style-name") {
                    Some(s) => self.styles.text_fmt(s, &base),
                    None => base,
                };
                self.fmt.push(f);
            }
            "a" => {
                let mut f = self.cur_fmt();
                // Scripts, `file:` and other schemes keep their text but lose the link.
                if let Some(h) = get(a, "xlink:href").filter(|h| !h.is_empty() && wordcraft_doc::link::is_safe_href(h)) {
                    f.link = Some(h.to_string());
                }
                if let Some(s) = get(a, "text:style-name") {
                    f = self.styles.text_fmt(s, &f);
                }
                self.fmt.push(f);
            }
            "s" => {
                // As many spaces as the file says (the text budget bounds them).
                let n = get(a, "text:c").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).max(1);
                if n > self.text_left {
                    self.text_left = 0;
                    self.too_much_text = true;
                } else {
                    self.push_text(&" ".repeat(n));
                }
            }
            "tab" => self.push_text("\t"),
            "line-break" => self.push_text("\n"),
            "bookmark" | "bookmark-start" => {
                if let Some(n) = get(a, "text:name") {
                    if self.para.is_none() {
                        self.start_para(None, None);
                    }
                    if let Some(p) = &mut self.para {
                        p.inlines.push(Inline::Anchor(n.to_string()));
                    }
                }
            }
            "list" => {
                self.end_para();
                if self.lists.len() < 64 {
                    self.lists_seen = self.lists_seen.saturating_add(1);
                    // A top-level list that continues the numbering of the one before is the
                    // same list (ODF 1.3 §19.790).
                    let continued = self.lists.is_empty() && get(a, "text:continue-numbering").is_some_and(|v| v.trim() == "true");
                    let id = continued.then_some(self.last_list).flatten().unwrap_or(self.lists_seen);
                    self.lists.push((get(a, "text:style-name").map(String::from), false));
                    self.item_used.push(false);
                    self.list_ids.push(id);
                } else {
                    self.skip = 1;
                }
            }
            "list-item" | "list-header" => {
                if let Some(l) = self.lists.last_mut() {
                    l.1 = true;
                }
                if let Some(u) = self.item_used.last_mut() {
                    *u = false;
                }
                self.item_start = get(a, "text:start-value").and_then(|v| v.trim().parse::<u32>().ok()).map(|v| v.min(32_767));
            }
            "table" if q.starts_with("table:") => {
                self.end_para();
                // A body table whose style names a master page starts that page (ODF 1.3 §19.509).
                if let Some(next) = get(a, "table:style-name").and_then(|s| self.page_section(s)) {
                    self.break_section(next);
                }
                if self.tables.len() >= model::MAX_DEPTH {
                    self.skip = 1;
                    return;
                }
                self.tables.push(TableB { rows: Vec::new(), row: Vec::new(), hcover: 0, widths: Vec::new() });
            }
            "table-column" => {
                let w = get(a, "table:style-name").and_then(|s| self.styles.column_width(s)).unwrap_or(0.0);
                let n = get(a, "table:number-columns-repeated").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 63);
                if let Some(t) = self.tables.last_mut() {
                    for _ in 0..n {
                        if t.widths.len() < wordcraft_doc::table::MAX_COLS {
                            t.widths.push(w);
                        }
                    }
                }
            }
            "table-row" => {
                if let Some(t) = self.tables.last_mut() {
                    t.row.clear();
                    t.hcover = 0;
                }
            }
            "table-cell" => {
                self.end_para();
                let span = get(a, "table:number-columns-spanned").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(1).clamp(1, 63);
                let rows = get(a, "table:number-rows-spanned").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(1).clamp(1, 1000);
                let shading = get(a, "table:style-name").and_then(|s| self.styles.cell_background(s));
                if let Some(t) = self.tables.last_mut() {
                    t.hcover = span as usize - 1;
                }
                self.cells.push(Cell { colspan: span, rowspan: rows, shading, ..Default::default() });
                self.containers.push(Vec::new());
                self.lists_saved.push((std::mem::take(&mut self.lists), std::mem::take(&mut self.item_used), std::mem::take(&mut self.list_ids)));
            }
            "covered-table-cell" => {
                if let Some(t) = self.tables.last_mut() {
                    if t.hcover > 0 {
                        t.hcover -= 1;
                    } else if t.row.len() < 63 {
                        t.row.push(Cell { covered: true, ..Default::default() });
                    }
                }
            }
            "frame" => {
                let w = get(a, "svg:width").and_then(length);
                let h = get(a, "svg:height").and_then(length);
                self.frame = Some((w, h, None, String::new()));
            }
            "image" => {
                if let Some(href) = get(a, "xlink:href")
                    && let Some(fr) = &mut self.frame
                    && fr.2.is_none()
                {
                    let path = href.trim_start_matches("./");
                    fr.2 = (self.zip)(path);
                }
            }
            // A picture written into the document itself (flat documents).
            "binary-data" if self.frame.as_ref().is_some_and(|fr| fr.2.is_none()) && self.inline_left > 0 => self.binary = Some(String::new()),
            "desc" | "title" if self.frame.is_some() => self.in_desc = true,
            // A columned section (ODF 1.3 §5.4) is a continuous section with its columns.
            "section" if q.starts_with("text:") => {
                self.end_para();
                if self.cols_at.is_none()
                    && self.in_body()
                    && let Some(columns) = get(a, "text:style-name")
                        .and_then(|s| self.styles.columns(s))
                        // A section whose columns match the open section's (a plain one-column
                        // section in a one-column page) is no new section.
                        .filter(|c| c.count != self.sec.columns.count || (c.count > 1 && *c != self.sec.columns))
                {
                    let next = SectionProps { start: SectionStart::Continuous, page_num_start: None, columns, ..self.page.clone() };
                    self.break_section(next);
                    self.cols_at = Some(self.depth);
                }
            }
            _ => {}
        }
    }

    /// An element inside `text:tracked-changes`, outside deleted content.
    fn tracked_start(&mut self, q: &str, a: &Attrs) {
        match local(q) {
            "changed-region" if self.region.is_none() && self.region_count >= MAX_REGIONS => {
                refuse(|| format!("more than {MAX_REGIONS} tracked-change regions"));
            }
            "changed-region" if self.region.is_none() => {
                let id = get(a, "text:id").unwrap_or_default().to_string();
                let r = Region { id: self.region_count as u32, ..Default::default() };
                self.region_count += 1;
                self.region = Some((id, r));
            }
            "insertion" | "deletion" | "format-change" if self.region.as_ref().is_some_and(|(_, r)| r.kind.is_none()) => {
                let kind = match local(q) {
                    "insertion" => RevisionKind::Insert,
                    "deletion" => RevisionKind::Delete,
                    _ => RevisionKind::Format,
                };
                if let Some((_, r)) = &mut self.region {
                    r.kind = Some(kind);
                }
                if kind == RevisionKind::Delete {
                    self.containers.push(Vec::new());
                    self.del = Some(DelState {
                        containers: self.containers.len(),
                        tables: self.tables.len(),
                        cells: self.cells.len(),
                        lists_saved: self.lists_saved.len(),
                        fmt: self.fmt.len(),
                        lists: std::mem::take(&mut self.lists),
                        item_used: std::mem::take(&mut self.item_used),
                        ins: Vec::new(),
                    });
                }
            }
            "change-info" if self.region.is_some() => self.info_at = Some(self.depth),
            _ => self.skip = 1,
        }
    }

    /// End a deletion's content: its paragraphs go to the region being read.
    fn close_deletion(&mut self) {
        if self.del.is_none() {
            return;
        }
        self.end_para();
        let Some(d) = self.del.take() else { return };
        let base = d.containers.saturating_sub(1).min(self.containers.len());
        let blocks: Vec<FBlock> = self.containers.split_off(base).into_iter().flatten().filter(|b| matches!(b, FBlock::Para(_))).collect();
        if self.containers.is_empty() {
            self.containers.push(Vec::new());
        }
        self.tables.truncate(d.tables);
        self.cells.truncate(d.cells);
        self.lists_saved.truncate(d.lists_saved);
        self.fmt.truncate(d.fmt);
        self.lists = d.lists;
        self.item_used = d.item_used;
        self.frame = None;
        self.in_desc = false;
        self.para_cont = false;
        if let Some((_, r)) = &mut self.region {
            r.blocks = blocks;
        }
    }

    /// Put a deletion's paragraphs back at a `text:change` point, as deleted text. Inside a
    /// paragraph, each deleted paragraph mark splits it; between paragraphs, each deleted
    /// paragraph stands alone with its mark deleted.
    fn place_deletion(&mut self, blocks: Vec<FBlock>, mark: RevMark) {
        let mut paras: Vec<Para> = Vec::new();
        for b in blocks {
            let FBlock::Para(mut p) = b else { continue };
            // Pictures and other objects carry no revision mark: leave them out.
            p.inlines.retain(|i| matches!(i, Inline::Text(..) | Inline::Anchor(_)));
            for i in &mut p.inlines {
                if let Inline::Text(_, f) = i {
                    f.del = Some(mark.clone());
                }
            }
            paras.push(p);
        }
        let append = |dst: &mut Para, src: Vec<Inline>| {
            for i in src {
                match i {
                    Inline::Text(t, f) => dst.push_text(&t, &f),
                    other => dst.inlines.push(other),
                }
            }
        };
        let Some(mut cur) = self.para.take() else {
            self.para_cont = false;
            for mut p in paras {
                p.mark_del = Some(Box::new(mark.clone()));
                self.finish_para(p);
            }
            return;
        };
        let n = paras.len();
        let mut it = paras.into_iter();
        let mut first_mark_ins = None;
        if let Some(first) = it.next() {
            first_mark_ins = first.mark_ins;
            append(&mut cur, first.inlines);
        }
        if n < 2 {
            self.para = Some(cur);
            return;
        }
        let mut rest = Para { kind: cur.kind, list: cur.list, align: cur.align, ..Default::default() };
        // The host paragraph now ends at the first deleted paragraph's mark.
        cur.mark_del = Some(Box::new(mark.clone()));
        cur.mark_ins = first_mark_ins;
        self.finish_para(cur);
        for (k, mut p) in it.enumerate() {
            if k + 2 == n {
                // The last deleted paragraph runs on into the rest of the body paragraph.
                rest.kind = p.kind;
                rest.align = p.align.or(rest.align);
                append(&mut rest, p.inlines);
            } else {
                p.mark_del = Some(Box::new(mark.clone()));
                self.finish_para(p);
            }
        }
        self.para = Some(rest);
    }

    fn end(&mut self, q: &str) {
        self.depth = self.depth.saturating_sub(1);
        if q == "office:text" {
            if self.del.is_some() {
                // Unclosed deleted content (hostile): drop it.
                self.close_deletion();
            }
            self.in_tc = false;
            self.region = None;
            self.info_at = None;
            self.end_para();
            self.in_text = false;
            return;
        }
        if self.skip > 0 {
            self.skip -= 1;
            return;
        }
        if !self.in_text {
            return;
        }
        if let Some(d) = self.info_at {
            if self.depth < d {
                self.info_at = None;
            }
            self.info_field = None;
            return;
        }
        if self.in_tc {
            match local(q) {
                "deletion" if self.del.is_some() => {
                    self.close_deletion();
                    return;
                }
                "changed-region" => {
                    // A deletion left open (hostile) ends with its region.
                    self.close_deletion();
                    if let Some((id, r)) = self.region.take()
                        && r.kind.is_some()
                    {
                        // The first region with an id wins.
                        self.regions.entry(id).or_insert(r);
                    }
                    return;
                }
                "tracked-changes" => {
                    self.close_deletion();
                    self.in_tc = false;
                    self.region = None;
                    return;
                }
                _ if self.del.is_some() => {}
                _ => return,
            }
        }
        if self.stories.last().is_some_and(|st| st.depth == self.depth + 1)
            && matches!(local(q), "note" | "annotation" | "text-box" | "custom-shape" | "rect")
        {
            self.end_story();
            return;
        }
        match local(q) {
            "creator" | "date" | "creator-initials" if self.ann_field.is_some() => self.ann_field = None,
            "page-number" | "page-count" | "date" | "time" | "author-name" | "title" if q.starts_with("text:") => {
                if let Some((instr, result)) = self.field.take() {
                    self.push_inline(Inline::Field { instr: instr.to_string(), result });
                }
            }
            "p" | "h" if !self.in_desc => self.end_para(),
            "span" | "a" if self.fmt.len() > usize::from(self.para.is_some()) => {
                self.fmt.pop();
            }
            "list" => {
                self.end_para();
                self.lists.pop();
                self.item_used.pop();
                let id = self.list_ids.pop();
                if self.lists.is_empty() {
                    self.last_list = id;
                }
            }
            "list-item" | "list-header" => {
                self.end_para();
                if let Some(l) = self.lists.last_mut() {
                    l.1 = false;
                }
            }
            "table-cell" => {
                self.end_para();
                let blocks = if self.containers.len() > 1 { self.containers.pop().unwrap_or_default() } else { Vec::new() };
                (self.lists, self.item_used, self.list_ids) = self.lists_saved.pop().unwrap_or_default();
                let mut cell = self.cells.pop().unwrap_or_default();
                // An empty cell gets a paragraph of its own: it counts as one.
                if blocks.is_empty() && !self.take_para() {
                    return;
                }
                cell.blocks = blocks;
                if let Some(t) = self.tables.last_mut()
                    && t.row.len() < 63
                {
                    t.row.push(cell);
                }
            }
            "table-row" => {
                if let Some(t) = self.tables.last_mut() {
                    let r = std::mem::take(&mut t.row);
                    if !r.is_empty() && t.rows.len() < wordcraft_doc::table::MAX_ROWS {
                        t.rows.push(r);
                    }
                }
            }
            "table-header-rows" => {
                if let Some(t) = self.tables.last_mut() {
                    for r in &mut t.rows {
                        for c in r {
                            c.header = true;
                        }
                    }
                }
            }
            "table" if q.starts_with("table:") => {
                self.end_para();
                if let Some(t) = self.tables.pop()
                    && !t.rows.is_empty()
                {
                    // Column widths count only when every column has one.
                    let widths = if t.widths.iter().all(|w| w.is_finite() && *w > 1.0) { t.widths } else { Vec::new() };
                    self.container().push(FBlock::Table(FTable { rows: t.rows, widths, borderless: false }));
                }
            }
            "desc" | "title" => self.in_desc = false,
            "binary-data" => {
                if let Some(b) = self.binary.take()
                    && let Some(fr) = &mut self.frame
                    && fr.2.is_none()
                    && let Some(data) = model::base64_decode(&b).filter(|d| d.len() as u64 <= self.inline_left)
                {
                    self.inline_left = self.inline_left.saturating_sub(data.len() as u64);
                    fr.2 = Some(data);
                }
            }
            "section" if q.starts_with("text:") && self.cols_at == Some(self.depth + 1) => {
                self.end_para();
                self.cols_at = None;
                let next = SectionProps { start: SectionStart::Continuous, page_num_start: None, ..self.page.clone() };
                self.break_section(next);
            }
            "frame" => {
                if let Some((w, h, Some(data), alt)) = self.frame.take()
                    && let Some(img) = make_img(data, w, h, alt.trim())
                {
                    if self.para.is_none() {
                        self.start_para(None, None);
                    }
                    if let Some(p) = &mut self.para {
                        p.inlines.push(Inline::Image(img));
                    }
                }
                self.frame = None;
            }
            _ => {}
        }
    }
}

/// A `text:change-start` (`start`) or `text:change-end` (attributes `a`) among the insertions
/// `open`: unknown ids, ends without a start and starts past [`MAX_OPEN_INSERTIONS`] are ignored.
fn change_mark(regions: &HashMap<String, Region>, open: &mut Vec<(String, RevMark)>, start: bool, a: &Attrs) {
    let Some(id) = get(a, "text:change-id") else { return };
    if !start {
        if let Some(k) = open.iter().position(|(o, _)| o == id) {
            open.remove(k);
        }
        return;
    }
    if let Some(r) = regions.get(id).filter(|r| r.kind == Some(RevisionKind::Insert))
        && !open.iter().any(|(o, _)| o == id)
    {
        if open.len() < MAX_OPEN_INSERTIONS {
            open.push((id.to_string(), r.mark()));
        } else {
            refuse(|| format!("more than {MAX_OPEN_INSERTIONS} tracked insertions open inside one another"));
        }
    }
}

/// Inflate the XML entry `name` (`None` when missing or unreadable), taking what it inflates to
/// from `left`: an entry bigger than what's left, whatever size it declares, refuses the file.
fn read_entry(z: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str, left: &mut u64) -> Result<Option<Vec<u8>>, String> {
    let Ok(f) = z.by_name(name) else { return Ok(None) };
    let room = (*left).min(MAX_ENTRY);
    let too_big = || format!("ODT: {name} inflates past the size limit ({} MB of XML)", MAX_XML_ENTRIES >> 20);
    if f.size() > room {
        return Err(too_big());
    }
    let mut out = Vec::with_capacity(usize::try_from(f.size().min(16 << 20)).unwrap_or(0));
    if f.take(room.saturating_add(1)).read_to_end(&mut out).is_err() {
        return Ok(None);
    }
    if out.len() as u64 > room {
        return Err(too_big());
    }
    *left = left.saturating_sub(out.len() as u64);
    Ok(Some(out))
}

/// The `mimetype` entry: a short media type, so no more than a few hundred bytes are read.
fn read_mimetype(z: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Option<Vec<u8>> {
    let f = z.by_name("mimetype").ok()?;
    let mut out = Vec::new();
    f.take(MAX_MIMETYPE).read_to_end(&mut out).ok()?;
    Some(out)
}

/// Inflate image entry `name`, charging every inflated byte (even of a failed read) to `total`, so
/// the work across all images stays within `budget`: nothing is inflated once it is spent, and an
/// entry is read at most up to what is left.
fn fetch_image(z: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str, total: &mut u64, budget: u64) -> Option<Vec<u8>> {
    let left = budget.checked_sub(*total).filter(|l| *l > 0)?;
    let f = z.by_name(name).ok()?;
    if f.size() > left {
        return None;
    }
    let mut out = Vec::new();
    let r = f.take(left.saturating_add(1)).read_to_end(&mut out);
    *total = total.saturating_add(out.len() as u64);
    (r.is_ok() && out.len() as u64 <= left).then_some(out)
}

/// The pictures a package's frames point at, within one byte budget. Each entry is inflated
/// at most once, and only when the size it declares fits in what is left; the bytes inflated
/// (even from an entry bigger than it declares) and every later copy are charged, so repeated
/// references and lying entries can't go past the budget, and nothing is inflated after it.
struct Pictures {
    /// Entry name → its bytes (`None`: missing, unreadable or over the budget; not retried).
    seen: HashMap<String, Option<Vec<u8>>>,
    /// Bytes still allowed.
    room: u64,
}

impl Pictures {
    fn new(budget: u64) -> Self {
        Pictures { seen: HashMap::new(), room: budget }
    }

    /// The bytes of entry `name`, or `None` when it can't be read or doesn't fit.
    fn get(&mut self, z: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
        if let Some(hit) = self.seen.get(name) {
            let d = hit.as_ref().filter(|d| d.len() as u64 <= self.room)?;
            self.room = self.room.saturating_sub(d.len() as u64);
            return Some(d.clone());
        }
        let d = self.inflate(z, name);
        self.seen.insert(name.to_string(), d.clone());
        d
    }

    /// Inflate entry `name`, charging the bytes inflated; `None` when it doesn't fit or fails.
    fn inflate(&mut self, z: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
        let mut spent = 0u64;
        let d = fetch_image(z, name, &mut spent, self.room);
        self.room = self.room.saturating_sub(spent);
        d
    }
}

/// The document properties of `office:meta` (in `meta.xml`, or a flat document's own, whose
/// annotations' `dc:creator`s are not the document's).
fn read_meta(xml: &[u8]) -> Meta {
    let mut m = Meta::default();
    let mut cur: Option<String> = None;
    let mut keywords: Vec<String> = Vec::new();
    let mut in_meta = 0usize;
    events(xml, |ev| match ev {
        Ev::Start(q, _) => {
            if q == "office:meta" || in_meta > 0 {
                in_meta = in_meta.saturating_add(1);
            }
            cur = Some(q);
        }
        Ev::End(_) => {
            in_meta = in_meta.saturating_sub(1);
            cur = None;
        }
        Ev::Text(_) if in_meta == 0 => {}
        Ev::Text(t) => match cur.as_deref() {
            Some("dc:title") => m.title.push_str(&t),
            Some("dc:subject") => m.subject.push_str(&t),
            Some("dc:description") => m.description.push_str(&t),
            Some("meta:initial-creator") => m.author.push_str(&t),
            Some("dc:creator") if m.author.is_empty() => m.author.push_str(&t),
            Some("meta:keyword") => keywords.push(t.into_owned()),
            Some("meta:creation-date") => m.created.push_str(&t),
            _ => {}
        },
    });
    m.keywords = keywords.join(", ");
    if !m.created.is_empty() && !m.created.ends_with('Z') && !m.created.contains('+') {
        let base = m.created.split('.').next().unwrap_or("").to_string();
        m.created = format!("{base}Z");
    }
    m
}

/// Is `bytes` a flat XML document (`.fodt`: one `office:document` file) rather than a package?
/// Anything that starts with `<` after an optional byte order mark and white space.
fn is_flat_xml(bytes: &[u8]) -> bool {
    let b = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    b.iter().find(|c| !c.is_ascii_whitespace()) == Some(&b'<')
}

/// Parse an `.odt` package, or a flat `.fodt` document, into the flow model.
pub fn parse(bytes: &[u8]) -> Result<Flow, String> {
    parse_full(bytes).map(|(flow, _)| flow)
}

/// [`parse`], with the last section's properties (the others are on the paragraphs that end them).
fn parse_full(bytes: &[u8]) -> Result<(Flow, SectionProps), String> {
    parse_tracked(bytes).map(|(f, last, _, _)| (f, last))
}

/// A read package: its flow, last section, whether change tracking is on (`None`: the document
/// has no tracked changes), and the document's default font (its Standard or default style's).
type Parsed = (Flow, SectionProps, Option<bool>, Option<String>);

/// [`parse_full`], plus whether change tracking is on and the document's font.
fn parse_tracked(bytes: &[u8]) -> Result<Parsed, String> {
    parse_budgeted(bytes, MAX_TEXT, MAX_IMAGES, wordcraft_doc::MAX_PARAGRAPHS)
}

/// [`parse_tracked`] with explicit text, image and paragraph budgets (tests use small ones).
fn parse_budgeted(bytes: &[u8], text_budget: usize, image_budget: u64, para_budget: usize) -> Result<Parsed, String> {
    parse_bounded(bytes, text_budget, image_budget, para_budget, MAX_XML_ENTRIES)
}

/// [`parse_budgeted`] inflating at most `xml_budget` bytes of `content.xml`, `styles.xml` and
/// `meta.xml` together.
fn parse_bounded(bytes: &[u8], text_budget: usize, image_budget: u64, para_budget: usize, xml_budget: u64) -> Result<Parsed, String> {
    // Whatever an earlier read on this thread left (it may have ended early).
    let _ = take_refused();
    let read = parse_bounded_inner(bytes, text_budget, image_budget, para_budget, xml_budget);
    match take_refused() {
        Some(what) if read.is_ok() => Err(format!("ODT: limit exceeded: {what}")),
        _ => read,
    }
}

fn parse_bounded_inner(bytes: &[u8], text_budget: usize, image_budget: u64, para_budget: usize, xml_budget: u64) -> Result<Parsed, String> {
    // A flat document (`.fodt`, ODF 1.3 §3.1.2): meta, styles, master pages and the body in one
    // file; its pictures come inline (`office:binary-data`), so nothing is fetched.
    if is_flat_xml(bytes) {
        if bytes.len() as u64 > xml_budget.min(MAX_ENTRY) {
            return Err(format!("ODT: the document is past the size limit ({} MB of XML)", MAX_XML_ENTRIES >> 20));
        }
        let meta = read_meta(bytes);
        let mut fetch = |_: &str| -> Option<Vec<u8>> { None };
        return parse_parts(bytes, Some(bytes), meta, &mut fetch, text_budget, image_budget, para_budget);
    }
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("ODT: not a valid package ({e})"))?;
    if let Some(mt) = read_mimetype(&mut z) {
        let mt = String::from_utf8_lossy(&mt);
        if !mt.trim().starts_with("application/vnd.oasis.opendocument.text") {
            return Err(format!("ODT: not a text document ({})", mt.trim()));
        }
    }
    let mut xml_left = xml_budget;
    let content = read_entry(&mut z, "content.xml", &mut xml_left)?.ok_or("ODT: content.xml is missing")?;
    // Read once: its styles, page layouts, and the master pages' headers and footers below.
    let styles_xml = read_entry(&mut z, "styles.xml", &mut xml_left)?;
    let meta = read_entry(&mut z, "meta.xml", &mut xml_left)?.map(|m| read_meta(&m)).unwrap_or_default();
    let mut pictures = Pictures::new(image_budget);
    let mut fetch = |path: &str| -> Option<Vec<u8>> {
        // A frame may point at any entry, as often as it likes: `Pictures` keeps that bounded.
        pictures.get(&mut z, path)
    };
    parse_parts(&content, styles_xml.as_deref(), meta, &mut fetch, text_budget, image_budget, para_budget)
}

/// The document from its parts: `content` (the body and automatic styles), `styles_xml` (the
/// common styles, page layouts and master pages) and `meta`, with `fetch` giving the pictures
/// frames point at. A flat document passes itself as both.
fn parse_parts(
    content: &[u8],
    styles_xml: Option<&[u8]>,
    meta: Meta,
    fetch: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    text_budget: usize,
    image_budget: u64,
    para_budget: usize,
) -> Result<Parsed, String> {
    let mut styles = Styles::default();
    let mut pages = Pages::default();
    if let Some(s) = styles_xml {
        styles.read(s, false);
        pages = read_pages(s);
    }
    let page = pages.first.clone().unwrap_or_default();
    styles.read(content, true);
    // The master pages' headers and footers, read with styles.xml's own styles.
    let mut master_hf = HashMap::new();
    let mut paras_left = para_budget;
    if let Some(s) = styles_xml {
        let mut hstyles = Styles::default();
        hstyles.read(s, false);
        let left;
        (master_hf, left) = read_master_headers(s, &hstyles, fetch, &pages, text_budget / 4, para_budget);
        paras_left = left?;
    }
    let mut body = Body {
        styles: &styles,
        zip: fetch,
        containers: vec![Vec::new()],
        fmt: Vec::new(),
        para: None,
        xml_space_end: false,
        para_cont: false,
        lists: Vec::new(),
        lists_saved: Vec::new(),
        item_used: Vec::new(),
        list_ids: Vec::new(),
        lists_seen: 0,
        last_list: None,
        item_start: None,
        tables: Vec::new(),
        cells: Vec::new(),
        in_text: false,
        skip: 0,
        depth: 0,
        frame: None,
        in_desc: false,
        binary: None,
        inline_left: image_budget,
        text_left: text_budget,
        too_much_text: false,
        paras_left,
        too_many_paras: false,
        pages: &pages,
        sec: page.clone(),
        page,
        sec_from: 0,
        sections: 0,
        cols_at: None,
        in_tc: false,
        track: None,
        regions: HashMap::new(),
        region_count: 0,
        region: None,
        info_at: None,
        info_field: None,
        del: None,
        active_ins: Vec::new(),
        stories: Vec::new(),
        comments: Default::default(),
        ann_names: HashMap::new(),
        ann_field: None,
        field: None,
        cur_master: pages.first_name.clone(),
        next_master: None,
        section_masters: Vec::new(),
    };
    events(content, |ev| match ev {
        Ev::Start(q, a) => {
            if body.depth < MAX_XML_DEPTH {
                body.start(&q, &a);
            } else {
                refuse(|| format!("elements nested more than {MAX_XML_DEPTH} deep"));
                body.depth += 1;
            }
        }
        Ev::End(q) => {
            if body.depth <= MAX_XML_DEPTH {
                body.end(&q);
            } else {
                body.depth -= 1;
            }
        }
        Ev::Text(t) => body.text(&t),
    });
    while !body.stories.is_empty() {
        body.end_story();
    }
    body.end_para();
    if body.too_many_paras {
        return Err(too_many_paragraphs(para_budget));
    }
    if body.too_much_text {
        return Err(too_much_text(text_budget));
    }
    while body.containers.len() > 1 {
        let c = body.containers.pop().unwrap_or_default();
        body.container().extend(c);
    }
    let mut blocks = body.containers.pop().unwrap_or_default();
    for t in body.tables.drain(..) {
        if !t.rows.is_empty() {
            blocks.push(FBlock::Table(FTable { rows: t.rows, widths: Vec::new(), borderless: false }));
        }
    }
    // An open section with nothing in it (after a column section at the end) isn't one: the
    // section before it ends the document.
    let mut last = body.sec;
    let mut last_master = body.cur_master.clone();
    if blocks.len() <= body.sec_from
        && let Some(FBlock::Para(p)) = blocks.last_mut()
        && let Some(s) = p.section.take()
    {
        last = *s;
        last_master = body.section_masters.pop().flatten();
    }
    // Each section's headers: those of its master page, when it differs from the section
    // before's (else the section links to the one before). A master page without a header
    // after one with gets an empty one.
    let mut headers = Vec::new();
    let mut prev: Option<&Option<String>> = None;
    let masters: Vec<(Option<u32>, &Option<String>)> = body
        .section_masters
        .iter()
        .enumerate()
        .map(|(i, m)| (Some(u32::try_from(i).unwrap_or(u32::MAX)), m))
        .chain(std::iter::once((None, &last_master)))
        .collect();
    for (section, m) in masters {
        if prev == Some(m) {
            continue;
        }
        let mine = m.as_ref().and_then(|m| master_hf.get(m)).cloned().unwrap_or_default();
        let had = prev.and_then(|p| p.as_ref()).and_then(|p| master_hf.get(p)).cloned().unwrap_or_default();
        for h in &mine {
            headers.push(model::FHeader { section, ..h.clone() });
        }
        for footer in [false, true] {
            let has = |l: &[model::FHeader]| l.iter().any(|h| h.footer == footer && h.pages == model::HfPages::Default);
            if has(&had) && !has(&mine) {
                headers.push(model::FHeader { footer, section, ..Default::default() });
            }
        }
        prev = Some(m);
    }
    if blocks.is_empty() {
        blocks.push(FBlock::Para(Para::default()));
    }
    let track = body.track;
    let comments = std::mem::take(&mut body.comments);
    Ok((Flow { blocks, meta, comments, headers }, last, track, styles.doc_font()))
}

/// The headers and footers of each master page in `styles.xml` (ODF 1.3 §16.9–16.12), by
/// name: `style:header`/`style:footer` for every page, `-left` for left (even) pages, `-first`
/// for the first page. One with `style:display="false"` is off.
fn read_master_headers(
    xml: &[u8],
    styles: &Styles,
    zip: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    pages: &Pages,
    budget: usize,
    para_budget: usize,
) -> (HashMap<String, Vec<model::FHeader>>, Result<usize, String>) {
    let page = pages.first.clone().unwrap_or_default();
    let mut body = Body {
        styles,
        zip,
        containers: vec![Vec::new()],
        fmt: Vec::new(),
        para: None,
        xml_space_end: false,
        para_cont: false,
        lists: Vec::new(),
        lists_saved: Vec::new(),
        item_used: Vec::new(),
        list_ids: Vec::new(),
        lists_seen: 0,
        last_list: None,
        item_start: None,
        tables: Vec::new(),
        cells: Vec::new(),
        in_text: false,
        skip: 0,
        depth: 0,
        frame: None,
        in_desc: false,
        binary: None,
        inline_left: wordcraft_doc::limits::MEDIA_BYTES,
        text_left: budget,
        too_much_text: false,
        paras_left: para_budget,
        too_many_paras: false,
        pages,
        sec: page.clone(),
        page,
        sec_from: 0,
        sections: 0,
        cols_at: None,
        in_tc: false,
        track: None,
        regions: HashMap::new(),
        region_count: 0,
        region: None,
        info_at: None,
        info_field: None,
        del: None,
        active_ins: Vec::new(),
        stories: Vec::new(),
        comments: Default::default(),
        ann_names: HashMap::new(),
        ann_field: None,
        field: None,
        cur_master: None,
        next_master: None,
        section_masters: Vec::new(),
    };
    let mut out: HashMap<String, Vec<model::FHeader>> = HashMap::new();
    // The master page being read.
    let mut master: Option<String> = None;
    let mut cur: Option<(bool, model::HfPages, usize)> = None;
    let mut depth = 0usize;
    events(xml, |ev| match ev {
        Ev::Start(q, a) => {
            depth += 1;
            if cur.is_some() {
                if body.depth < MAX_XML_DEPTH {
                    body.start(&q, &a);
                } else {
                    body.depth += 1;
                }
                return;
            }
            match local(&q) {
                "master-page" if q.starts_with("style:") && out.len() >= MAX_SECTIONS => refuse(too_many_masters),
                "master-page" if q.starts_with("style:") => {
                    master = get(&a, "style:name").map(String::from);
                }
                n if master.is_some() && q.starts_with("style:") && get(&a, "style:display") != Some("false") => {
                    let which = match n {
                        "header" => Some((false, model::HfPages::Default)),
                        "header-left" => Some((false, model::HfPages::Even)),
                        "header-first" => Some((false, model::HfPages::First)),
                        "footer" => Some((true, model::HfPages::Default)),
                        "footer-left" => Some((true, model::HfPages::Even)),
                        "footer-first" => Some((true, model::HfPages::First)),
                        _ => None,
                    };
                    if let Some((footer, pages)) = which {
                        cur = Some((footer, pages, depth));
                        body.in_text = true;
                        body.containers = vec![Vec::new()];
                    }
                }
                _ => {}
            }
        }
        Ev::End(q) => {
            match cur {
                Some((footer, pages, d)) if d == depth => {
                    while !body.stories.is_empty() {
                        body.end_story();
                    }
                    body.end_para();
                    while body.containers.len() > 1 {
                        let c = body.containers.pop().unwrap_or_default();
                        body.container().extend(c);
                    }
                    let blocks = body.containers.pop().unwrap_or_default();
                    if let Some(m) = &master {
                        let list = out.entry(m.clone()).or_default();
                        if !list.iter().any(|h| h.footer == footer && h.pages == pages) {
                            list.push(model::FHeader { footer, pages, blocks, section: None });
                        }
                    }
                    body.in_text = false;
                    body.skip = 0;
                    body.depth = 0;
                    cur = None;
                }
                Some(_) => {
                    if body.depth <= MAX_XML_DEPTH {
                        body.end(&q);
                    } else {
                        body.depth -= 1;
                    }
                }
                None if local(&q) == "master-page" => master = None,
                None => {}
            }
            depth = depth.saturating_sub(1);
        }
        Ev::Text(t) => {
            if cur.is_some() {
                body.text(&t);
            }
        }
    });
    let left = if body.too_many_paras {
        Err(too_many_paragraphs(para_budget))
    } else if body.too_much_text {
        Err(too_much_text(budget))
    } else {
        Ok(body.paras_left)
    };
    (out, left)
}

/// Why a file with more master pages or page layouts than a document may have sections is
/// refused.
fn too_many_masters() -> String {
    format!("more than {MAX_SECTIONS} master pages or page layouts")
}

/// The error for a file whose text expands past `budget` bytes.
fn too_much_text(budget: usize) -> String {
    format!("ODT: more than {} MB of text", budget >> 20)
}

/// The error for a file making more than `budget` paragraphs.
fn too_many_paragraphs(budget: usize) -> String {
    format!("ODT: more than {budget} paragraphs")
}

/// Parse an `.odt` package into a document.
pub fn import(bytes: &[u8]) -> Result<Document, String> {
    let (flow, last, track, font) = parse_tracked(bytes)?;
    let mut doc = model::to_doc(&flow);
    if font.is_some() {
        doc.styles.default_chr.font = font;
    }
    // The headers and footers `to_doc` gave the last section stay with it.
    let s = &doc.last_section;
    let (headers, footers, title_page) = (s.headers, s.footers, s.title_page);
    doc.last_section = SectionProps { headers, footers, title_page, ..last };
    if let Some(t) = track {
        doc.settings.track_changes = t;
    }
    Ok(doc)
}

/// The page setup of each master page in `styles.xml` (ODF 1.3 §16.9), and of the first one,
/// which pages use until a paragraph names another.
#[derive(Default)]
struct Pages {
    masters: HashMap<String, SectionProps>,
    first: Option<SectionProps>,
    /// The first master page's name.
    first_name: Option<String>,
}

fn read_pages(xml: &[u8]) -> Pages {
    let mut layouts: HashMap<String, (Attrs, Option<Columns>)> = HashMap::new();
    let mut cur: Option<String> = None;
    let mut masters: Vec<(String, String)> = Vec::new();
    events(xml, |ev| match ev {
        Ev::Start(q, a) => match local(&q) {
            "page-layout" if q.starts_with("style:") => cur = get(&a, "style:name").map(String::from),
            "page-layout-properties" => {
                if let Some(n) = &cur {
                    if layouts.len() < MAX_SECTIONS || layouts.contains_key(n) {
                        layouts.insert(n.clone(), (a, None));
                    } else {
                        refuse(too_many_masters);
                    }
                }
            }
            "columns" if q.starts_with("style:") => {
                if let Some(l) = cur.as_ref().and_then(|n| layouts.get_mut(n)) {
                    l.1 = Some(columns_of(&a));
                }
            }
            "column-sep" => {
                if let Some(c) = cur.as_ref().and_then(|n| layouts.get_mut(n)).and_then(|l| l.1.as_mut()) {
                    c.separator = column_sep(&a);
                }
            }
            "master-page" if masters.len() >= MAX_SECTIONS => refuse(too_many_masters),
            "master-page" => {
                if let (Some(n), Some(l)) = (get(&a, "style:name"), get(&a, "style:page-layout-name")) {
                    masters.push((n.to_string(), l.to_string()));
                }
            }
            _ => {}
        },
        Ev::End(q) => {
            if local(&q) == "page-layout" {
                cur = None;
            }
        }
        Ev::Text(_) => {}
    });
    let mut pages = Pages::default();
    for (name, layout) in masters {
        let Some((a, cols)) = layouts.get(&layout) else { continue };
        let mut sec = SectionProps::default();
        apply_layout(a, &mut sec);
        // A one-column layout keeps the default columns (and their gap).
        if let Some(c) = cols.as_ref().filter(|c| c.count > 1) {
            sec.columns = c.clone();
        }
        if pages.first.is_none() {
            pages.first = Some(sec.clone());
            pages.first_name = Some(name.clone());
        }
        pages.masters.entry(name).or_insert(sec);
    }
    pages
}

/// Replace `sec` with the page setup of the first master page in `styles.xml`, if any.
#[cfg(test)]
fn apply_page_layout(xml: &[u8], sec: &mut SectionProps) {
    if let Some(first) = read_pages(xml).first {
        *sec = first;
    }
}

/// A `style:page-layout-properties` element's page size, orientation and margins into `sec`.
fn apply_layout(a: &Attrs, sec: &mut SectionProps) {
    let len = |k: &str| get(a, k).and_then(length).filter(|v| *v >= 0.0);
    // Like the DOCX reader: a page is at least half an inch; at most 22 inches (Word's limit), so a hostile
    // `1e30pt` can't reach layout.
    let page = |k: &str| len(k).filter(|v| *v >= 36.0).map(|v| v.min(1584.0));
    if let (Some(w), Some(h)) = (page("fo:page-width"), page("fo:page-height")) {
        sec.page_w = w;
        sec.page_h = h;
    }
    sec.landscape = match get(a, "style:print-orientation") {
        Some("landscape") => true,
        Some("portrait") => false,
        _ => sec.page_w > sec.page_h,
    };
    for (k, slot) in [
        ("fo:margin-top", &mut sec.margin_top),
        ("fo:margin-bottom", &mut sec.margin_bottom),
        ("fo:margin-left", &mut sec.margin_left),
        ("fo:margin-right", &mut sec.margin_right),
    ] {
        if let Some(v) = len(k) {
            // No margin is larger than the page's longer side (as `w:pgMar` in the DOCX reader).
            *slot = v.min(sec.page_w.max(sec.page_h));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths() {
        assert_eq!(length("1in"), Some(72.0));
        assert_eq!(length("12pt"), Some(12.0));
        assert!((length("2.54cm").unwrap() - 72.0).abs() < 0.01);
        assert_eq!(length("x"), None);
    }

    #[test]
    fn default_paragraph_style_font_becomes_document_font() {
        // LibreOffice keeps the font in the Standard style or in `style:default-style`; the import
        // used to skip both and the text fell back to the built-in body font.
        let content = format!(
            r#"<office:document-content {NS_T}><office:body><office:text><text:p>Hi</text:p></office:text></office:body></office:document-content>"#
        );
        let standard = format!(
            r#"<office:document-styles {NS_T}><office:styles><style:default-style style:family="paragraph"><style:text-properties style:font-name="DejaVu Serif"/></style:default-style><style:style style:name="Standard" style:family="paragraph"><style:text-properties style:font-name="Times New Roman"/></style:style></office:styles></office:document-styles>"#
        );
        let doc = import(&package(&content, Some(&standard))).unwrap();
        assert_eq!(doc.styles.default_chr.font.as_deref(), Some("Times New Roman"));
        let only_default = format!(
            r#"<office:document-styles {NS_T}><office:styles><style:default-style style:family="paragraph"><style:text-properties style:font-name="DejaVu Serif"/></style:default-style></office:styles></office:document-styles>"#
        );
        let doc = import(&package(&content, Some(&only_default))).unwrap();
        assert_eq!(doc.styles.default_chr.font.as_deref(), Some("DejaVu Serif"));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(b"not a zip").is_err());
        assert!(parse(b"<not xml").is_err() || parse(b"<not xml").is_ok(), "a lone tag is a flat document with nothing in it, or refused");
        assert!(parse(b"  <x/>").is_ok(), "an empty flat document");
    }

    const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    /// A flat document (`.fodt`, ODF 1.3 §3.1.2) with its meta, styles, master pages and body in
    /// one file.
    fn flat(body: &str, auto: &str, picture: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><office:document {NS_T} xmlns:xlink="http://www.w3.org/1999/xlink" office:mimetype="application/vnd.oasis.opendocument.text"><office:meta><dc:title>Flat title</dc:title><meta:initial-creator>Flat author</meta:initial-creator></office:meta><office:styles><style:style style:name="Standard" style:family="paragraph"/></office:styles><office:automatic-styles><style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="10in" fo:page-height="5in" fo:margin-left="1in"/></style:page-layout>{auto}</office:automatic-styles><office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pm1"><style:header><text:p>Flat header</text:p></style:header></style:master-page></office:master-styles><office:body><office:text>{body}<text:p>pic <draw:frame svg:width="1in" svg:height="1in"><draw:image>{picture}</draw:image></draw:frame></text:p></office:text></office:body></office:document>"#
        )
    }

    /// A flat document opens as the package would: properties (an annotation's `dc:creator`
    /// isn't the author), page setup, the master page's header, styles, and pictures from
    /// `office:binary-data`; one whose picture is past the picture budget opens without it.
    #[test]
    fn flat_documents_open_with_their_pictures_styles_and_headers() {
        let body = r#"<text:p>Body <office:annotation office:name="a1"><dc:creator>Ann Otator</dc:creator><dc:date>2026-01-01T00:00:00</dc:date><text:p>Remark</text:p></office:annotation>words<office:annotation-end office:name="a1"/>.</text:p>"#;
        let xml = flat(body, "", &format!("<office:binary-data>\n  {PNG_1X1}\n</office:binary-data>"));
        let d = import(xml.as_bytes()).unwrap();
        assert_eq!((d.core.title.as_str(), d.core.creator.as_str()), ("Flat title", "Flat author"));
        assert_eq!(d.last_section.page_w, 720.0);
        assert!(d.parts.values().any(|p| p.kind == wordcraft_doc::PartKind::Header && p.blocks.len() == 1), "the header");
        assert_eq!(d.comments.len(), 1);
        assert_eq!(d.media.len(), 1, "the inline picture");
        assert_eq!(d.plain_text(wordcraft_doc::StoryRef::Body).replace(wordcraft_doc::para::OBJ, ""), "Body words.\npic ");
        // The same text, saved as a package, reads the same.
        let again = import(&export(&d).unwrap()).unwrap();
        assert_eq!(again.plain_text(wordcraft_doc::StoryRef::Body), d.plain_text(wordcraft_doc::StoryRef::Body));
        assert_eq!(again.media.len(), 1);
        // Past the picture budget: no picture, no error, the text whole.
        let (flow, _, _, _) = parse_bounded(xml.as_bytes(), MAX_TEXT, 16, wordcraft_doc::MAX_PARAGRAPHS, MAX_XML_ENTRIES).unwrap();
        let pictures = flow.blocks.iter().filter(|b| matches!(b, FBlock::Para(p) if p.inlines.iter().any(|i| matches!(i, Inline::Image(_))))).count();
        assert_eq!(pictures, 0);
        assert!(flow.blocks.len() >= 2);
        // Base64 that isn't: no picture.
        let d = import(flat("", "", "<office:binary-data>!!not base64!!</office:binary-data>").as_bytes()).unwrap();
        assert_eq!(d.media.len(), 0);
        // A flat document past the size limit is refused, not read.
        let big = flat(&"<text:p>x</text:p>".repeat(10), "", "");
        assert!(parse_bounded(big.as_bytes(), MAX_TEXT, MAX_IMAGES, wordcraft_doc::MAX_PARAGRAPHS, 64).unwrap_err().contains("size limit"));
    }

    /// Spacing, indents and line spacing of a paragraph's automatic style are the paragraph's
    /// (#2542), a right-to-left `style:writing-mode` is a right-to-left paragraph (#2693), and
    /// both are written back.
    #[test]
    fn paragraph_spacing_and_direction_round_trip() {
        let content = format!(
            r#"<office:document-content {NS_T}><office:automatic-styles><style:style style:name="P1" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:margin-top="12pt" fo:margin-bottom="6pt" fo:margin-left="1cm" fo:text-indent="-0.5cm" fo:line-height="150%" style:writing-mode="rl-tb"/></style:style></office:automatic-styles><office:body><office:text><text:p text:style-name="P1">עברית</text:p><text:p>plain</text:p></office:text></office:body></office:document-content>"#
        );
        let d = import(&zip_of(&[("content.xml", content.as_bytes())])).unwrap();
        let p = d.body.first().and_then(|b| b.as_para()).unwrap();
        assert_eq!(p.props.bidi, Some(true));
        assert_eq!((p.props.space_before, p.props.space_after), (Some(12.0), Some(6.0)));
        assert!((p.props.indent_left.unwrap() - 28.35).abs() < 0.01 && (p.props.indent_first.unwrap() + 14.17).abs() < 0.01, "{:?}", p.props);
        assert_eq!(p.props.line_spacing, Some(wordcraft_doc::props::LineSpacing::Multiple(1.5)));
        let q = d.body.get(1).and_then(|b| b.as_para()).unwrap();
        assert_eq!((q.props.bidi, q.props.space_before, q.props.indent_left), (None, None, None));
        let out = export(&d).unwrap();
        let again = import(&out).unwrap();
        let p2 = again.body.first().and_then(|b| b.as_para()).unwrap();
        assert_eq!(
            (p2.props.bidi, p2.props.space_before, p2.props.space_after, p2.props.line_spacing),
            (Some(true), Some(12.0), Some(6.0), p.props.line_spacing)
        );
        assert!((p2.props.indent_left.unwrap() - 28.35).abs() < 0.01 && (p2.props.indent_first.unwrap() + 14.17).abs() < 0.01);
    }

    /// Column widths (`style:column-width` of the table-column styles) are the table's grid,
    /// written and read back.
    #[test]
    fn column_widths_round_trip() {
        let content = format!(
            r#"<office:document-content {NS_T}><office:automatic-styles><style:style style:name="T1.A" style:family="table-column"><style:table-column-properties style:column-width="1in"/></style:style><style:style style:name="T1.B" style:family="table-column"><style:table-column-properties style:column-width="2in"/></style:style></office:automatic-styles><office:body><office:text><table:table><table:table-column table:style-name="T1.A"/><table:table-column table:style-name="T1.B" table:number-columns-repeated="2"/><table:table-row><table:table-cell><text:p>a</text:p></table:table-cell><table:table-cell><text:p>b</text:p></table:table-cell><table:table-cell><text:p>c</text:p></table:table-cell></table:table-row></table:table></office:text></office:body></office:document-content>"#
        );
        let d = import(&zip_of(&[("content.xml", content.as_bytes())])).unwrap();
        let grid =
            |d: &Document| d.body.iter().find_map(|b| if let wordcraft_doc::Block::Table(t) = &**b { Some(t.grid.clone()) } else { None }).unwrap();
        assert_eq!(grid(&d), vec![72.0, 144.0, 144.0]);
        let again = import(&export(&d).unwrap()).unwrap();
        assert_eq!(grid(&again), vec![72.0, 144.0, 144.0]);
    }

    /// `text:start-value` starts a list item's number, `text:continue-numbering` goes on from
    /// the list before, and two lists in a row are two lists; all written back as read.
    #[test]
    fn list_start_values_and_continued_numbering_round_trip() {
        let content = format!(
            r#"<office:document-content {NS_T}><office:automatic-styles><text:list-style style:name="L1"><text:list-level-style-number text:level="1" style:num-format="1"/></text:list-style></office:automatic-styles><office:body><office:text><text:list text:style-name="L1"><text:list-item text:start-value="3"><text:p>three</text:p></text:list-item><text:list-item><text:p>four</text:p></text:list-item></text:list><text:list text:style-name="L1"><text:list-item><text:p>one</text:p></text:list-item></text:list><text:p>text</text:p><text:list text:style-name="L1" text:continue-numbering="true"><text:list-item><text:p>two</text:p></text:list-item></text:list></office:text></office:body></office:document-content>"#
        );
        let labels = |d: &Document| {
            let mut c = wordcraft_doc::numbering::Counters::default();
            d.body
                .iter()
                .filter_map(|b| b.as_para())
                .filter_map(|p| {
                    let n = p.props.numbering?;
                    let (l, _) = c.next_label(&d.numbering, n.num, n.level)?;
                    Some(format!("{l}{}", p.plain_text()))
                })
                .collect::<Vec<_>>()
        };
        let d = import(&zip_of(&[("content.xml", content.as_bytes())])).unwrap();
        let want = vec!["3.three", "4.four", "1.one", "2.two"];
        assert_eq!(labels(&d), want);
        let out = export(&d).unwrap();
        assert_eq!(labels(&import(&out).unwrap()), want);
    }

    /// A package holding `entries`, deflated.
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            z.start_file(*name, o).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    /// `bytes` (a package) with the size its central directory declares for `name` set to `size`.
    fn declare_size(mut bytes: Vec<u8>, name: &str, size: u32) -> Vec<u8> {
        let at = (0..bytes.len())
            .find(|&i| bytes[i..].starts_with(b"PK\x01\x02") && bytes.get(i + 46..).is_some_and(|r| r.starts_with(name.as_bytes())))
            .unwrap();
        bytes[at + 24..at + 28].copy_from_slice(&size.to_le_bytes());
        bytes
    }

    fn archive(bytes: &[u8]) -> zip::ZipArchive<Cursor<&[u8]>> {
        zip::ZipArchive::new(Cursor::new(bytes)).unwrap()
    }

    #[test]
    fn pictures_are_inflated_once_and_every_use_is_charged() {
        // Every frame on a picture inflated it again, even once the budget was used up.
        let a = zip_of(&[("p.png", &[1u8; 300])]);
        let b = zip_of(&[("p.png", &[2u8; 300]), ("q.png", &[3u8; 10])]);
        let (mut za, mut zb) = (archive(&a), archive(&b));
        let mut pics = Pictures::new(1000);
        assert_eq!(pics.get(&mut za, "p.png"), Some(vec![1u8; 300]));
        // Kept from the first read: the entry isn't read again (another package's would differ).
        assert_eq!(pics.get(&mut zb, "p.png"), Some(vec![1u8; 300]));
        assert_eq!(pics.get(&mut zb, "p.png"), Some(vec![1u8; 300]));
        assert_eq!(pics.room, 100);
        // A fourth copy doesn't fit; an entry that couldn't be read isn't looked up again.
        assert_eq!(pics.get(&mut za, "p.png"), None);
        assert_eq!(pics.get(&mut za, "q.png"), None);
        assert_eq!(pics.get(&mut zb, "q.png"), None);
        assert_eq!(pics.room, 100);
    }

    #[test]
    fn pictures_over_the_budget_are_not_inflated() {
        // An entry declaring more than is left is refused before inflating, and costs nothing.
        let pkg = zip_of(&[("big.png", &[0u8; 1200]), ("ok.png", &[5u8; 300])]);
        let mut z = archive(&pkg);
        let mut pics = Pictures::new(1000);
        assert_eq!(pics.get(&mut z, "big.png"), None);
        assert_eq!(pics.room, 1000);
        assert_eq!(pics.get(&mut z, "ok.png").map(|d| d.len()), Some(300));
        // One inflating to more than it declares stops a byte past the budget and uses it up:
        // nothing is inflated after it.
        let liar = declare_size(zip_of(&[("liar.png", &[0u8; 5000]), ("ok.png", &[5u8; 10])]), "liar.png", 10);
        let mut z = archive(&liar);
        let mut pics = Pictures::new(1000);
        assert_eq!(pics.get(&mut z, "liar.png"), None);
        assert_eq!(pics.room, 0);
        assert_eq!(pics.get(&mut z, "ok.png"), None);
    }

    #[test]
    fn frames_sharing_a_picture_all_show_it() {
        let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[7u8; 300]].concat();
        let frames = r#"<draw:frame svg:width="1in" svg:height="1in"><draw:image xlink:href="Pictures/a.png"/></draw:frame>"#.repeat(3);
        let content = format!(
            r#"<office:document-content {NS_T}><office:body><office:text><text:p>{frames}</text:p></office:text></office:body></office:document-content>"#
        );
        let pkg = zip_of(&[("content.xml", content.as_bytes()), ("Pictures/a.png", &png)]);
        let flow = parse(&pkg).unwrap();
        let imgs: Vec<&model::Img> =
            paras(&flow).into_iter().flat_map(|p| &p.inlines).filter_map(|i| if let Inline::Image(img) = i { Some(img) } else { None }).collect();
        assert_eq!(imgs.len(), 3);
        assert!(imgs.iter().all(|i| i.data.as_slice() == png.as_slice()));
        assert_eq!(import(&pkg).unwrap().media.len(), 1);
    }

    /// `content.xml`, `styles.xml` and `meta.xml` share one inflate budget: each entry could
    /// inflate to 256 MB, and `styles.xml` was inflated twice, so a package of a few hundred KB
    /// took a gigabyte (two 250 MB entries of white space: 998 MB). Past it the file is refused,
    /// whatever size the entries declare.
    #[test]
    fn the_xml_entries_together_have_one_inflate_budget() {
        let content = format!(
            r#"<office:document-content {NS_T}><office:body><office:text><text:p>x</text:p></office:text></office:body></office:document-content>"#
        );
        let styles = format!(r#"<office:document-styles {NS_T}>{}</office:document-styles>"#, " ".repeat(1000));
        let pkg = zip_of(&[("content.xml", content.as_bytes()), ("styles.xml", styles.as_bytes()), ("meta.xml", b"<office:document-meta/>")]);
        let total = (content.len() + styles.len() + "<office:document-meta/>".len()) as u64;
        let read = |pkg: &[u8], budget: u64| parse_bounded(pkg, MAX_TEXT, MAX_IMAGES, wordcraft_doc::MAX_PARAGRAPHS, budget);
        assert!(read(&pkg, total).is_ok());
        assert!(read(&pkg, total - 1).unwrap_err().contains("size limit"));
        // An entry that inflates to more than it declares is stopped there too.
        let liar = declare_size(pkg.clone(), "styles.xml", 10);
        assert!(read(&liar, content.len() as u64 + 500).unwrap_err().contains("size limit"));
        // The mimetype entry is read only as far as a media type goes.
        let long = [b"application/vnd.oasis.opendocument.text".as_slice(), &[b' '; 100_000]].concat();
        let pkg = zip_of(&[("mimetype", &long), ("content.xml", content.as_bytes())]);
        assert!(read_mimetype(&mut archive(&pkg)).is_some_and(|m| m.len() as u64 == MAX_MIMETYPE));
        assert!(parse(&pkg).is_ok());
    }

    /// List styles are capped as styles are: millions of them took a gigabyte (4,000,000
    /// list styles in a 10 MB file: 985 MB).
    #[test]
    fn list_styles_are_capped() {
        let lists: String = (0..MAX_LIST_STYLES + 10).map(|i| format!(r#"<text:list-style style:name="L{i}"/>"#)).collect();
        let xml = format!(r#"<office:document-content {NS_T}><office:automatic-styles>{lists}</office:automatic-styles></office:document-content>"#);
        let mut s = Styles::default();
        s.read(xml.as_bytes(), true);
        assert_eq!(s.lists.len(), MAX_LIST_STYLES);
        // One already kept is still updated by a later definition.
        let again = format!(
            r#"<office:document-content {NS_T}><office:automatic-styles><text:list-style style:name="L0"><text:list-level-style-number text:level="1" style:num-format="1"/></text:list-style></office:automatic-styles></office:document-content>"#
        );
        s.read(again.as_bytes(), true);
        assert_eq!(s.lists.get("L0").and_then(|l| l.first()).copied(), Some(true));
    }

    /// Text is handed on as it is in the input where it can be: text the reader skips (white
    /// space between elements, however long) is never copied.
    #[test]
    fn text_events_borrow_from_the_input() {
        let xml = b"<a>plain<b/>  \n  </a><c>x&amp;y</c><d><![CDATA[z]]></d>";
        let mut seen = Vec::new();
        events(xml, |ev| {
            if let Ev::Text(t) = ev {
                seen.push((t.to_string(), matches!(t, Cow::Borrowed(_))));
            }
        });
        let want = [("plain", true), ("  \n  ", true), ("x", true), ("&", true), ("y", true), ("z", false)];
        assert_eq!(seen, want.map(|(t, b)| (t.to_string(), b)));
    }

    const NS_T: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0""#;

    /// A package with `content.xml` and, when given, `styles.xml`.
    fn package(content: &str, styles: Option<&str>) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, data) in
            [("mimetype", "application/vnd.oasis.opendocument.text"), ("content.xml", content)].into_iter().chain(styles.map(|s| ("styles.xml", s)))
        {
            z.start_file(name, o).unwrap();
            z.write_all(data.as_bytes()).unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    fn paras(f: &Flow) -> Vec<&Para> {
        f.blocks.iter().filter_map(|b| if let FBlock::Para(p) = b { Some(p) } else { None }).collect()
    }

    #[test]
    fn common_paragraph_styles_give_text_formatting() {
        // #436: a common paragraph style's text properties reach its paragraphs, directly,
        // through a derived common style and through an automatic style's parent.
        let props = r##"<style:text-properties fo:font-weight="bold" fo:color="#ff0000" fo:font-size="24pt"/>"##;
        let styles = format!(
            r#"<office:document-styles {NS_T}><office:styles><style:style style:name="Standard" style:family="paragraph"/><style:style style:name="Emphasis" style:family="paragraph" style:parent-style-name="Standard">{props}</style:style><style:style style:name="Derived" style:family="paragraph" style:parent-style-name="Emphasis"/></office:styles></office:document-styles>"#
        );
        let content = format!(
            r#"<office:document-content {NS_T}><office:automatic-styles><style:style style:name="Bridge" style:family="paragraph" style:parent-style-name="Emphasis"/></office:automatic-styles><office:body><office:text><text:p>Plain</text:p><text:p text:style-name="Emphasis">Common</text:p><text:p text:style-name="Derived">Derived</text:p><text:p text:style-name="Bridge">Bridge</text:p></office:text></office:body></office:document-content>"#
        );
        let f = parse(&package(&content, Some(&styles))).unwrap();
        let ps = paras(&f);
        assert_eq!(ps.len(), 4);
        let fmt = |p: &Para| p.inlines.iter().find_map(|i| if let Inline::Text(_, f) = i { Some(f.clone()) } else { None }).unwrap();
        assert_eq!(fmt(ps[0]), Fmt::default());
        for p in &ps[1..] {
            let f = fmt(p);
            assert!(f.bold && f.color == Some(Rgb(255, 0, 0)) && f.size == Some(24.0), "{}: {f:?}", p.text());
        }
    }

    #[test]
    fn text_after_a_drawing_shape_stays_in_its_paragraph() {
        // #1654: a shape's own text:p closed the paragraph it is anchored in, and the text after
        // the shape was dropped. Shapes are skipped like text boxes; the paragraph keeps its text.
        let draw =
            r#"xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0""#;
        let content = format!(
            r#"<office:document-content {NS_T} {draw}><office:body><office:text><text:p text:style-name="Heading"><draw:custom-shape svg:width="20mm" svg:height="20mm"><text:p>In the shape</text:p><draw:enhanced-geometry draw:type="rectangle"/></draw:custom-shape>Before <draw:rect><text:p/></draw:rect>middle <draw:g><draw:ellipse><text:p>In the group</text:p></draw:ellipse></draw:g>after</text:p><text:p>Next</text:p></office:text></office:body></office:document-content>"#
        );
        let styles = format!(
            r#"<office:document-styles {NS_T}><office:styles><style:style style:name="Heading" style:family="paragraph"><style:text-properties fo:font-weight="bold"/></style:style></office:styles></office:document-styles>"#
        );
        let f = parse(&package(&content, Some(&styles))).unwrap();
        let ps = paras(&f);
        assert_eq!(ps.iter().map(|p| p.text()).collect::<Vec<_>>(), ["Before middle after", "Next"]);
        assert!(ps[0].inlines.iter().all(|i| !matches!(i, Inline::Text(_, f) if !f.bold)), "{:?}", ps[0].inlines);
    }

    #[test]
    fn page_size_orientation_and_margins_round_trip() {
        // #810: the page setup written on export is read back on import.
        let mut doc = Document::new();
        doc.last_section.page_w = 841.0;
        doc.last_section.page_h = 595.0;
        doc.last_section.landscape = true;
        doc.last_section.margin_left = 36.0;
        doc.last_section.margin_top = 54.0;
        let back = import(&export(&doc).unwrap()).unwrap();
        let s = &back.last_section;
        assert!(s.landscape);
        assert_eq!((s.page_w, s.page_h), (841.0, 595.0));
        assert_eq!((s.margin_left, s.margin_top), (36.0, 54.0));
    }

    #[test]
    fn page_layout_follows_the_first_master_page() {
        let xml = br#"<office:document-styles><office:automatic-styles>
            <style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm"/></style:page-layout>
            <style:page-layout style:name="pm2">
            <style:page-layout-properties fo:page-width="10in" fo:page-height="5in" fo:margin-left="1in" style:print-orientation="landscape"/>
            </style:page-layout>
            </office:automatic-styles><office:master-styles>
            <style:master-page style:name="Standard" style:page-layout-name="pm2"/>
            <style:master-page style:name="Other" style:page-layout-name="pm1"/>
            </office:master-styles></office:document-styles>"#;
        let mut sec = wordcraft_doc::SectionProps::default();
        apply_page_layout(xml, &mut sec);
        assert!(sec.landscape);
        assert_eq!((sec.page_w, sec.page_h, sec.margin_left, sec.margin_top), (720.0, 360.0, 72.0, 72.0));
    }

    #[test]
    fn hostile_page_layout_values_are_clamped() {
        let styles = |w: &str, h: &str, m: &str| {
            format!(
                r#"<office:document-styles><office:automatic-styles><style:page-layout style:name="pm1">
                <style:page-layout-properties fo:page-width="{w}" fo:page-height="{h}" fo:margin-top="{m}"
                fo:margin-bottom="{m}" fo:margin-left="{m}" fo:margin-right="{m}"/></style:page-layout>
                </office:automatic-styles><office:master-styles>
                <style:master-page style:name="Standard" style:page-layout-name="pm1"/></office:master-styles>
                </office:document-styles>"#
            )
        };
        // Huge values: the page is capped at 22 inches and margins at the page's longer side.
        let mut sec = wordcraft_doc::SectionProps::default();
        apply_page_layout(styles("1e30pt", "1e30in", "1e30pt").as_bytes(), &mut sec);
        assert_eq!((sec.page_w, sec.page_h), (1584.0, 1584.0));
        assert_eq!((sec.margin_top, sec.margin_left), (1584.0, 1584.0));
        // Negative, zero, tiny and non-numeric values leave the defaults alone.
        let def = wordcraft_doc::SectionProps::default();
        for (w, h, m) in [("-10in", "-5in", "-1in"), ("0pt", "0pt", "NaNpt"), ("1pt", "2pt", "infpt"), ("NaN", "inf", "x")] {
            let mut sec = wordcraft_doc::SectionProps::default();
            apply_page_layout(styles(w, h, m).as_bytes(), &mut sec);
            assert_eq!((sec.page_w, sec.page_h), (def.page_w, def.page_h), "{w} {h}");
            assert_eq!((sec.margin_top, sec.margin_right), (def.margin_top, def.margin_right), "{m}");
            assert!(sec.page_w.is_finite() && sec.margin_left.is_finite());
        }
    }

    #[test]
    fn table_cell_style_background_is_shading() {
        // #437: `fo:background-color` on `style:table-cell-properties` is the cell's shading.
        let content = format!(
            r##"<office:document-content {NS_T}><office:automatic-styles><style:style style:name="RedCell" style:family="table-cell"><style:table-cell-properties fo:background-color="#ff0000"/></style:style></office:automatic-styles><office:body><office:text><table:table><table:table-column table:number-columns-repeated="2"/><table:table-row><table:table-cell table:style-name="RedCell"><text:p>Red</text:p></table:table-cell><table:table-cell><text:p>Plain</text:p></table:table-cell></table:table-row></table:table></office:text></office:body></office:document-content>"##
        );
        let f = parse(&package(&content, None)).unwrap();
        let Some(FBlock::Table(t)) = f.blocks.iter().find(|b| matches!(b, FBlock::Table(_))) else { panic!("no table") };
        let row: Vec<_> = t.rows[0].iter().map(|c| (c.text(), c.shading)).collect();
        assert_eq!(row, vec![("Red".to_string(), Some(Rgb(255, 0, 0))), ("Plain".to_string(), None)]);
    }

    fn flow_text_len(f: &Flow) -> usize {
        paras(f).iter().flat_map(|p| p.inlines.iter()).map(|i| if let Inline::Text(t, _) = i { t.len() } else { 0 }).sum()
    }

    #[test]
    fn text_s_expansion_is_budgeted() {
        // `<text:s text:c="1000"/>` amplifies ~45×; text expanded past the text budget refuses
        // the file (nothing is left out silently).
        let spaces = "x<text:s text:c=\"1000\"/>".repeat(200);
        let content = format!(
            "<office:document-content {NS_T}><office:body><office:text><text:p>{spaces}x</text:p><text:p>after</text:p></office:text></office:body></office:document-content>"
        );
        let pkg = package(&content, None);
        let budgeted =
            |pkg: &[u8], budget: usize| parse_budgeted(pkg, budget, MAX_IMAGES, wordcraft_doc::MAX_PARAGRAPHS).map(|f| flow_text_len(&f.0));
        assert_eq!(budgeted(&pkg, 5000), Err(too_much_text(5000)));
        // At the budget everything comes through (the real budget is far larger).
        assert_eq!(budgeted(&pkg, 200_206), Ok(200_206));
        assert!(budgeted(&pkg, 200_205).is_err());
        assert_eq!(flow_text_len(&parse(&pkg).unwrap()), 200_206);
        // Text past the budget, however it ends, is refused too.
        let content = format!(
            "<office:document-content {NS_T}><office:body><office:text><text:p>ééé</text:p></office:text></office:body></office:document-content>"
        );
        assert!(budgeted(&package(&content, None), 5).is_err());
        assert_eq!(budgeted(&package(&content, None), 6), Ok(6));
    }

    /// Each list the writer writes has a list style of its own: the reader's cap on list styles
    /// is past what the XML budget lets content.xml hold, so the budget (a clear refusal) comes
    /// first and the cap never drops the lists of a document WordCraft saved.
    #[test]
    fn the_list_style_cap_is_past_what_the_xml_budget_holds() {
        let content_len = |lists: usize| {
            let html = "<ol><li>item</li></ol><p>between</p>".repeat(lists);
            let bytes = export(&crate::html::import(&html)).unwrap();
            archive(&bytes).by_name("content.xml").unwrap().size()
        };
        let per_list = content_len(21) - content_len(20);
        assert!(per_list > 1_000, "{per_list}");
        assert!(per_list * MAX_LIST_STYLES as u64 > MAX_XML_ENTRIES, "{per_list} bytes a list");
    }

    /// Before, a run of spaces read back as 1,001 at most (`text:c` was capped at 1000): a
    /// document with a longer one (which the writer writes as one `text:s`) lost spaces.
    #[test]
    fn long_runs_of_spaces_read_back_whole() {
        let mut doc = Document::new();
        let text = format!("a{}b", " ".repeat(5000));
        doc.body = vec![wordcraft_doc::para_block(wordcraft_doc::Paragraph::with_text(&text, Default::default()))];
        let back = import(&export(&doc).unwrap()).unwrap();
        assert_eq!(back.plain_text(wordcraft_doc::StoryRef::Body), text);
    }

    #[test]
    fn paragraphs_past_the_budget_refuse_the_file() {
        // Empty paragraphs zip to almost nothing, and each is about 1 KB in the document.
        let doc = |body: &str| {
            package(
                &format!("<office:document-content {NS_T}><office:body><office:text>{body}</office:text></office:body></office:document-content>"),
                None,
            )
        };
        let pkg = doc(&"<text:p>x</text:p>".repeat(51));
        assert!(parse_budgeted(&pkg, MAX_TEXT, MAX_IMAGES, 50).is_err());
        assert_eq!(paras(&parse_budgeted(&pkg, MAX_TEXT, MAX_IMAGES, 51).unwrap().0).len(), 51);
        // An empty table cell counts: it gets a paragraph.
        let pkg = doc(
            "<table:table><table:table-row><table:table-cell/><table:table-cell><text:p>x</text:p></table:table-cell></table:table-row></table:table>",
        );
        assert!(parse_budgeted(&pkg, MAX_TEXT, MAX_IMAGES, 1).is_err());
        assert!(parse_budgeted(&pkg, MAX_TEXT, MAX_IMAGES, 2).is_ok());
        // Header paragraphs (styles.xml) come out of the same budget.
        let styles = format!(
            "<office:document-styles {NS_T}><office:master-styles><style:master-page style:name=\"Standard\"><style:header>{}</style:header></style:master-page></office:master-styles></office:document-styles>",
            "<text:p>h</text:p>".repeat(3)
        );
        let pkg = package(
            &format!(
                "<office:document-content {NS_T}><office:body><office:text><text:p>x</text:p></office:text></office:body></office:document-content>"
            ),
            Some(&styles),
        );
        assert!(parse_budgeted(&pkg, MAX_TEXT, MAX_IMAGES, 3).is_err());
        assert!(parse_budgeted(&pkg, MAX_TEXT, MAX_IMAGES, 4).is_ok());
        assert!(parse_budgeted(&pkg, MAX_TEXT, MAX_IMAGES, 2).is_err());
    }

    #[test]
    fn image_fetch_is_budgeted() {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, len) in [("a.png", 60), ("b.png", 60), ("c.png", 10), ("big.png", 100_000)] {
            z.start_file(name, o).unwrap();
            z.write_all(&vec![0u8; len]).unwrap();
        }
        let bytes = z.finish().unwrap().into_inner();
        let mut z = zip::ZipArchive::new(Cursor::new(&bytes[..])).unwrap();
        let mut total = 0;
        // An entry larger than what is left is never inflated.
        assert!(fetch_image(&mut z, "big.png", &mut total, 100).is_none());
        assert_eq!(total, 0);
        assert_eq!(fetch_image(&mut z, "a.png", &mut total, 100).map(|d| d.len()), Some(60));
        assert!(fetch_image(&mut z, "b.png", &mut total, 100).is_none());
        assert_eq!(fetch_image(&mut z, "c.png", &mut total, 100).map(|d| d.len()), Some(10));
        assert_eq!(total, 70);
        // Once spent, nothing more is inflated.
        let mut total = 100;
        assert!(fetch_image(&mut z, "c.png", &mut total, 100).is_none());
        assert_eq!(total, 100);
    }

    #[test]
    fn unsafe_link_targets_are_dropped() {
        let content = format!(
            r#"<office:document-content {NS_T} xmlns:xlink="http://www.w3.org/1999/xlink"><office:body><office:text><text:p><text:a xlink:href="javascript:alert(1)">js</text:a> <text:a xlink:href="https://e.test/">web</text:a> <text:a xlink:href="file:///etc/passwd">file</text:a></text:p></office:text></office:body></office:document-content>"#
        );
        let f = parse(&package(&content, None)).unwrap();
        let l: Vec<(String, Option<String>)> = paras(&f)
            .iter()
            .flat_map(|p| p.inlines.iter())
            .filter_map(|i| if let Inline::Text(t, f) = i { Some((t.clone(), f.link.clone())) } else { None })
            .collect();
        assert_eq!(paras(&f)[0].text(), "js web file");
        assert!(l.iter().all(|(t, link)| (t == "web") == link.is_some()), "{l:?}");
        assert!(l.iter().any(|(_, link)| link.as_deref() == Some("https://e.test/")), "{l:?}");
    }

    #[test]
    fn export_drops_non_xml_characters() {
        assert_eq!(x("a\u{FFFE}b\u{FFFF}c\u{1}d\te"), "abcd\te");
    }

    /// #1379: three master pages (A4 upright; A4 wide with page numbering restarting at 1; A4
    /// upright again with numbering continuing) and a two-column `text:section` in between.
    fn sections_package() -> Vec<u8> {
        let styles = format!(
            r#"<office:document-styles {NS_T}><office:automatic-styles>
            <style:page-layout style:name="pU"><style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm" style:print-orientation="portrait" fo:margin-top="2cm" fo:margin-bottom="2cm" fo:margin-left="2.5cm" fo:margin-right="2.5cm"/></style:page-layout>
            <style:page-layout style:name="pW"><style:page-layout-properties fo:page-width="29.7cm" fo:page-height="21cm" style:print-orientation="landscape" fo:margin-top="1.5cm" fo:margin-bottom="1.5cm" fo:margin-left="1.5cm" fo:margin-right="1.5cm"/></style:page-layout>
            </office:automatic-styles><office:master-styles>
            <style:master-page style:name="Upright" style:page-layout-name="pU"/>
            <style:master-page style:name="Wide" style:page-layout-name="pW"/>
            <style:master-page style:name="Closing" style:page-layout-name="pU"/>
            </office:master-styles></office:document-styles>"#
        );
        let content = format!(
            r#"<office:document-content {NS_T}><office:automatic-styles>
            <style:style style:name="PStart" style:family="paragraph" style:master-page-name="Upright"/>
            <style:style style:name="PWide" style:family="paragraph" style:master-page-name="Wide"><style:paragraph-properties fo:break-before="page" style:page-number="1"/></style:style>
            <style:style style:name="PClose" style:family="paragraph" style:master-page-name="Closing"><style:paragraph-properties fo:break-before="page" style:page-number="auto"/></style:style>
            <style:style style:name="Two" style:family="section"><style:section-properties><style:columns fo:column-count="2" fo:column-gap="1cm"><style:column-sep style:width="0.02cm" style:style="solid"/></style:columns></style:section-properties></style:style>
            </office:automatic-styles><office:body><office:text>
            <text:p text:style-name="PStart">Opening words</text:p><text:p>More opening words</text:p>
            <text:h text:style-name="PWide" text:outline-level="1">Wide part</text:h>
            <table:table><table:table-column table:number-columns-repeated="2"/><table:table-row><table:table-cell><text:p>A</text:p></table:table-cell><table:table-cell><text:p>B</text:p></table:table-cell></table:table-row></table:table>
            <text:section text:name="Cols" text:style-name="Two"><text:p>Left column text</text:p><text:p>Right column text</text:p></text:section>
            <text:p text:style-name="PClose">Closing words</text:p><text:p>Last line</text:p>
            </office:text></office:body></office:document-content>"#
        );
        package(&content, Some(&styles))
    }

    #[test]
    fn master_pages_and_column_sections_become_sections() {
        use wordcraft_doc::section::SectionStart;
        let doc = import(&sections_package()).unwrap();
        let secs = doc.sections();
        let summary: Vec<_> =
            secs.iter().map(|(_, s)| (s.start, s.landscape, s.page_w.round(), s.page_h.round(), s.columns.count, s.page_num_start)).collect();
        assert_eq!(
            summary,
            [
                (SectionStart::NextPage, false, 595.0, 842.0, 1, None),
                (SectionStart::NextPage, true, 842.0, 595.0, 1, Some(1)),
                (SectionStart::Continuous, true, 842.0, 595.0, 2, None),
                (SectionStart::NextPage, false, 595.0, 842.0, 1, None),
            ]
        );
        // Margins come from each section's page layout; the columns from the section style.
        assert!((secs[0].1.margin_left - 70.87).abs() < 0.01 && (secs[1].1.margin_left - 42.52).abs() < 0.01);
        let cols = &secs[2].1.columns;
        assert!((cols.space - 28.35).abs() < 0.01 && cols.separator, "{cols:?}");
        // Each section ends on its own last paragraph; the wide one ends after its table.
        let text = |i: usize| match doc.body.get(i).map(|b| &**b) {
            Some(wordcraft_doc::Block::Para(p)) => p.plain_text().to_string(),
            _ => String::from("<not a paragraph>"),
        };
        assert_eq!(text(secs[0].0), "More opening words");
        assert!(matches!(doc.body.get(secs[1].0 - 1).map(|b| &**b), Some(wordcraft_doc::Block::Table(_))));
        assert_eq!(text(secs[2].0), "Right column text");
        assert_eq!(text(secs[3].0), "Last line");
    }

    /// (start, landscape, width, height, columns, restart, margins) of each section, and the
    /// text of the paragraph ending it.
    fn section_summary(doc: &Document) -> Vec<(wordcraft_doc::section::SectionStart, bool, f32, f32, u32, bool, Option<u32>, [f32; 4], String)> {
        doc.sections()
            .iter()
            .map(|(end, s)| {
                let text = match doc.body.get(*end).map(|b| &**b) {
                    Some(wordcraft_doc::Block::Para(p)) => p.plain_text().to_string(),
                    _ => String::new(),
                };
                (
                    s.start,
                    s.landscape,
                    s.page_w.round(),
                    s.page_h.round(),
                    s.columns.count,
                    s.columns.separator,
                    s.page_num_start,
                    [s.margin_top, s.margin_bottom, s.margin_left, s.margin_right].map(f32::round),
                    text,
                )
            })
            .collect()
    }

    #[test]
    fn export_writes_sections_that_import_back() {
        // Portrait; landscape on a new page restarting at 1; continuous two columns with a
        // separator; portrait again. Export, import: the same sections.
        let doc = import(&sections_package()).unwrap();
        let before = section_summary(&doc);
        assert_eq!(before.len(), 4, "{before:?}");
        let back = import(&export(&doc).unwrap()).unwrap();
        let after = section_summary(&back);
        assert_eq!(after, before);
        let (a, b) = (&doc.sections(), &back.sections());
        for (x, y) in a.iter().zip(b.iter()) {
            assert!((x.1.columns.space - y.1.columns.space).abs() < 0.1, "{:?} vs {:?}", x.1.columns, y.1.columns);
            assert!((x.1.margin_top - y.1.margin_top).abs() < 0.1 && (x.1.margin_right - y.1.margin_right).abs() < 0.1);
        }
    }

    #[test]
    fn export_starts_a_section_at_a_table_and_keeps_its_restart() {
        // A section that starts with a table names its master page on the table's style.
        let styles = format!(
            r#"<office:document-styles {NS_T}><office:automatic-styles><style:page-layout style:name="pU"><style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm"/></style:page-layout><style:page-layout style:name="pW"><style:page-layout-properties fo:page-width="29.7cm" fo:page-height="21cm" style:print-orientation="landscape"/></style:page-layout></office:automatic-styles><office:master-styles><style:master-page style:name="Upright" style:page-layout-name="pU"/><style:master-page style:name="Wide" style:page-layout-name="pW"/></office:master-styles></office:document-styles>"#
        );
        let content = format!(
            r#"<office:document-content {NS_T}><office:automatic-styles><style:style style:name="WideTable" style:family="table" style:master-page-name="Wide"><style:table-properties style:page-number="5"/></style:style></office:automatic-styles><office:body><office:text><text:p>Intro</text:p><table:table table:style-name="WideTable"><table:table-column/><table:table-row><table:table-cell><text:p>Cell</text:p></table:table-cell></table:table-row></table:table><text:p>After</text:p></office:text></office:body></office:document-content>"#
        );
        let doc = import(&package(&content, Some(&styles))).unwrap();
        let before = section_summary(&doc);
        assert_eq!(before.len(), 2, "{before:?}");
        let back = import(&export(&doc).unwrap()).unwrap();
        assert_eq!(section_summary(&back), before);
        assert_eq!(back.sections()[1].1.page_num_start, Some(5));
    }

    /// Master pages `Standard` (portrait) and `Cols` (landscape, two columns with a line) for the
    /// round-trip tests below.
    fn cols_styles() -> String {
        format!(
            r#"<office:document-styles {NS_T}><office:automatic-styles><style:page-layout style:name="pU"><style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm" fo:margin-top="2cm" fo:margin-bottom="2.2cm" fo:margin-left="2.5cm" fo:margin-right="2.6cm"/></style:page-layout><style:page-layout style:name="pC"><style:page-layout-properties fo:page-width="29.7cm" fo:page-height="21cm" style:print-orientation="landscape" fo:margin-top="1cm" fo:margin-bottom="1.1cm" fo:margin-left="1.2cm" fo:margin-right="1.3cm"><style:columns fo:column-count="2" fo:column-gap="1cm"><style:column-sep style:width="0.02cm" style:style="solid"/></style:columns></style:page-layout-properties></style:page-layout></office:automatic-styles><office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pU"/><style:master-page style:name="Cols" style:page-layout-name="pC"/></office:master-styles></office:document-styles>"#
        )
    }

    /// Import `body` (with automatic `styles`) over [`cols_styles`], export, import: both summaries.
    fn round_trip(styles: &str, body: &str) -> (Vec<String>, Vec<String>) {
        let content = format!(
            r#"<office:document-content {NS_T}><office:automatic-styles>{styles}</office:automatic-styles><office:body><office:text>{body}</office:text></office:body></office:document-content>"#
        );
        let doc = import(&package(&content, Some(&cols_styles()))).unwrap();
        let back = import(&export(&doc).unwrap()).unwrap();
        let show = |d: &Document| section_summary(d).iter().map(|s| format!("{s:?}")).collect::<Vec<_>>();
        (show(&doc), show(&back))
    }

    #[test]
    fn export_keeps_a_columned_page_and_the_one_column_part_after_it() {
        let styles = r#"<style:style style:name="PC" style:family="paragraph" style:master-page-name="Cols"/><style:style style:name="One" style:family="section"><style:section-properties><style:columns fo:column-count="1" fo:column-gap="0cm"/></style:section-properties></style:style>"#;
        let body = r#"<text:p>Intro</text:p><text:p text:style-name="PC">Two columns</text:p><text:section text:name="S" text:style-name="One"><text:p>One column</text:p></text:section>"#;
        let (before, after) = round_trip(styles, body);
        assert_eq!(before.len(), 3, "{before:?}");
        assert!(before[1].contains(", 2, true,") && before[2].contains("Continuous") && before[2].contains(", 1, false,"), "{before:?}");
        assert_eq!(after, before);
    }

    #[test]
    fn plain_one_column_text_sections_do_not_make_sections() {
        let styles = r#"<style:style style:name="One" style:family="section"><style:section-properties><style:columns fo:column-count="1" fo:column-gap="0cm"/></style:section-properties></style:style>"#;
        let body = r#"<text:p>A</text:p><text:section text:name="S" text:style-name="One"><text:p>B</text:p></text:section><text:section text:name="T"><text:p>C</text:p></text:section>"#;
        let (before, after) = round_trip(styles, body);
        assert_eq!(before.len(), 1, "{before:?}");
        assert_eq!(after, before);
    }

    #[test]
    fn export_keeps_a_section_that_starts_on_a_list_item() {
        let styles = r#"<style:style style:name="PC" style:family="paragraph" style:master-page-name="Cols"/>"#;
        let body = r#"<text:p>Intro</text:p><text:list><text:list-item><text:p text:style-name="PC">First</text:p></text:list-item><text:list-item><text:p>Second</text:p></text:list-item></text:list><text:p>After</text:p>"#;
        let (before, after) = round_trip(styles, body);
        assert_eq!(before.len(), 2, "{before:?}");
        assert_eq!(after, before);
    }

    #[test]
    fn a_list_cut_at_a_section_boundary_goes_on_numbering() {
        let styles = r#"<style:style style:name="PC" style:family="paragraph" style:master-page-name="Cols"/>"#;
        let body = r#"<text:list><text:list-item><text:p>One</text:p></text:list-item><text:list-item><text:p text:style-name="PC">Two</text:p></text:list-item></text:list>"#;
        let content = format!(
            r#"<office:document-content {NS_T}><office:automatic-styles>{styles}</office:automatic-styles><office:body><office:text>{body}</office:text></office:body></office:document-content>"#
        );
        let doc = import(&package(&content, Some(&cols_styles()))).unwrap();
        let bytes = export(&doc).unwrap();
        let xml = String::from_utf8(read_entry(&mut archive(&bytes), "content.xml", &mut u64::MAX.clone()).unwrap().unwrap()).unwrap();
        assert_eq!(xml.matches("text:continue-list=\"").count(), 1, "{xml}");
        // The continuation points at the first list's xml:id.
        assert!(xml.contains("xml:id=\"lst1\"") && xml.contains("text:continue-list=\"lst1\""), "{xml}");
        assert_eq!(section_summary(&import(&bytes).unwrap()), section_summary(&doc));
    }

    #[test]
    fn export_keeps_a_page_number_restart_on_the_first_section() {
        let styles = r#"<style:style style:name="PS" style:family="paragraph" style:master-page-name="Standard"><style:paragraph-properties style:page-number="7"/></style:style>"#;
        let body = r#"<text:p text:style-name="PS">First</text:p><text:p>Second</text:p>"#;
        let (before, after) = round_trip(styles, body);
        assert_eq!(before.len(), 1, "{before:?}");
        assert!(before[0].contains("Some(7)"), "{before:?}");
        assert_eq!(after, before);
    }

    #[test]
    fn export_of_a_one_section_document_stays_simple() {
        let doc = import(&sections_package()).unwrap();
        let one = import(&export(&doc).unwrap()).unwrap();
        let mut single = one.clone();
        for b in single.body.iter_mut() {
            if let wordcraft_doc::Block::Para(p) = std::sync::Arc::make_mut(b) {
                p.section = None;
            }
        }
        let bytes = export(&single).unwrap();
        let mut z = archive(&bytes);
        let styles = String::from_utf8(read_entry(&mut z, "styles.xml", &mut u64::MAX.clone()).unwrap().unwrap()).unwrap();
        let content = String::from_utf8(read_entry(&mut z, "content.xml", &mut u64::MAX.clone()).unwrap().unwrap()).unwrap();
        assert_eq!(styles.matches("<style:master-page ").count(), 1);
        assert!(styles.contains("style:name=\"Standard\" style:page-layout-name=\"pm1\""));
        assert!(!content.contains("text:section") && !content.contains("master-page-name"), "{content}");
    }

    #[test]
    fn one_master_page_stays_one_section() {
        let styles = format!(
            r#"<office:document-styles {NS_T}><office:automatic-styles><style:page-layout style:name="pU"><style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm"/></style:page-layout></office:automatic-styles><office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pU"/></office:master-styles></office:document-styles>"#
        );
        let content = format!(
            r#"<office:document-content {NS_T}><office:automatic-styles><style:style style:name="P1" style:family="paragraph" style:master-page-name="Standard"/></office:automatic-styles><office:body><office:text><text:p text:style-name="P1">One</text:p><text:p>Two</text:p></office:text></office:body></office:document-content>"#
        );
        let doc = import(&package(&content, Some(&styles))).unwrap();
        assert_eq!(doc.sections().len(), 1);
        assert_eq!(doc.last_section.page_w.round(), 595.0);
    }

    #[test]
    fn hostile_sections_are_capped_and_clamped() {
        // Master-page paragraphs past the sections a document may have: refused (they used to
        // give the first 10,000 sections); just under, read whole. Absurd column counts and gaps
        // are clamped; a column section at the very end leaves no empty section after it.
        let file = |n: usize| {
            let mut body = String::new();
            for _ in 0..n {
                body.push_str(r#"<text:p text:style-name="PM">x</text:p>"#);
            }
            body.push_str(r#"<text:section text:style-name="Many"><text:p>cols</text:p></text:section>"#);
            let content = format!(
                r#"<office:document-content {NS_T}><office:automatic-styles><style:style style:name="PM" style:family="paragraph" style:master-page-name="Standard"/><style:style style:name="Many" style:family="section"><style:section-properties><style:columns fo:column-count="99999999999" fo:column-gap="1e30in"/></style:section-properties></style:style></office:automatic-styles><office:body><office:text>{body}</office:text></office:body></office:document-content>"#
            );
            let styles = format!(
                r#"<office:document-styles {NS_T}><office:automatic-styles><style:page-layout style:name="pU"><style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm"/></style:page-layout></office:automatic-styles><office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pU"/></office:master-styles></office:document-styles>"#
            );
            package(&content, Some(&styles))
        };
        assert!(import(&file(20_000)).is_err());
        let doc = import(&file(MAX_SECTIONS - 2)).unwrap();
        assert!(doc.sections().len() <= MAX_SECTIONS && doc.sections().len() > MAX_SECTIONS - 4, "{}", doc.sections().len());
        let content = format!(
            r#"<office:document-content {NS_T}><office:automatic-styles><style:style style:name="Many" style:family="section"><style:section-properties><style:columns fo:column-count="1000" fo:column-gap="1e30in"/></style:section-properties></style:style></office:automatic-styles><office:body><office:text><text:p>before</text:p><text:section text:style-name="Many"><text:p>cols</text:p></text:section></office:text></office:body></office:document-content>"#
        );
        let doc = import(&package(&content, None)).unwrap();
        let secs = doc.sections();
        assert_eq!(secs.len(), 2);
        assert_eq!(secs[1].1.columns.count, 45);
        assert!(secs[1].1.columns.space <= 1584.0);
    }

    #[test]
    fn master_page_on_a_table_style_starts_a_section() {
        // Review of #1379: a table style can name a master page too (ODF 1.3 §19.509), with its
        // page-number restart on `style:table-properties`. An empty name on a child style means
        // "no page style here", and a page-number restart without a master page starts nothing.
        use wordcraft_doc::section::SectionStart;
        let styles = format!(
            r#"<office:document-styles {NS_T}><office:styles><style:style style:name="WideParent" style:family="paragraph" style:master-page-name="Wide"/></office:styles><office:automatic-styles>
            <style:page-layout style:name="pU"><style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm"><style:columns fo:column-count="1" fo:column-gap="0cm"/></style:page-layout-properties></style:page-layout>
            <style:page-layout style:name="pW"><style:page-layout-properties fo:page-width="29.7cm" fo:page-height="21cm" style:print-orientation="landscape"/></style:page-layout>
            </office:automatic-styles><office:master-styles>
            <style:master-page style:name="Upright" style:page-layout-name="pU"/><style:master-page style:name="Wide" style:page-layout-name="pW"/>
            </office:master-styles></office:document-styles>"#
        );
        let content = format!(
            r#"<office:document-content {NS_T}><office:automatic-styles>
            <style:style style:name="NoPage" style:family="paragraph" style:parent-style-name="WideParent" style:master-page-name=""/>
            <style:style style:name="Restart" style:family="paragraph"><style:paragraph-properties fo:break-before="page" style:page-number="3"/></style:style>
            <style:style style:name="WideTable" style:family="table" style:master-page-name="Wide"><style:table-properties style:page-number="1"/></style:style>
            </office:automatic-styles><office:body><office:text>
            <text:p>Intro</text:p><text:p text:style-name="NoPage">Not a new page style</text:p><text:p text:style-name="Restart">Page break only</text:p>
            <table:table table:style-name="WideTable"><table:table-column/><table:table-row><table:table-cell><text:p>Cell</text:p></table:table-cell></table:table-row></table:table>
            <text:p>After</text:p>
            </office:text></office:body></office:document-content>"#
        );
        let doc = import(&package(&content, Some(&styles))).unwrap();
        let secs = doc.sections();
        let summary: Vec<_> = secs.iter().map(|(i, s)| (*i, s.start, s.landscape, s.page_num_start)).collect();
        assert_eq!(summary, [(2, SectionStart::NextPage, false, None), (4, SectionStart::NextPage, true, Some(1))]);
        assert!(matches!(doc.body.get(3).map(|b| &**b), Some(wordcraft_doc::Block::Table(_))));
        // A one-column page layout keeps the default column gap.
        assert_eq!(secs[0].1.columns, wordcraft_doc::section::Columns::default());
    }

    #[test]
    fn keep_with_next_survives_import_and_export() {
        let content = format!(
            r#"<office:document-content {NS_T}><office:automatic-styles>
            <style:style style:name="Keep" style:family="paragraph"><style:paragraph-properties fo:keep-with-next="always"/></style:style>
            <style:style style:name="Loose" style:family="paragraph" style:parent-style-name="Keep"><style:paragraph-properties fo:keep-with-next="auto"/></style:style>
            </office:automatic-styles><office:body><office:text>
            <text:p text:style-name="Keep">Kept</text:p><text:p text:style-name="Loose">Loose</text:p><text:p>Plain</text:p>
            </office:text></office:body></office:document-content>"#
        );
        let doc = import(&package(&content, None)).unwrap();
        let flags = |d: &Document| d.body.iter().filter_map(|b| b.as_para()).map(|p| p.props.keep_next).collect::<Vec<_>>();
        assert_eq!(flags(&doc), [Some(true), None, None]);
        let again = import(&export(&doc).unwrap()).unwrap();
        assert_eq!(flags(&again), [Some(true), None, None]);
    }

    // --- Tracked changes (ODF 1.3 Part 3 §5.5) -------------------------------------------------

    const NS_TC: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/""#;

    /// An `.odt` whose body holds `regions` in `text:tracked-changes` and then `body`.
    fn tracked(regions: &str, body: &str) -> Vec<u8> {
        let content = format!(
            r#"<office:document-content {NS_TC}><office:body><office:text><text:tracked-changes text:track-changes="true">{regions}</text:tracked-changes>{body}</office:text></office:body></office:document-content>"#
        );
        package(&content, None)
    }

    fn region(id: &str, kind: &str, author: &str, date: &str, content: &str) -> String {
        format!(
            r#"<text:changed-region text:id="{id}"><text:{kind}><office:change-info><dc:creator>{author}</dc:creator><dc:date>{date}</dc:date><text:p>a remark on the change</text:p></office:change-info>{content}</text:{kind}></text:changed-region>"#
        )
    }

    /// Each body paragraph as (text, revision kind) runs, plus its mark's revision kind.
    type RevRuns = Vec<(Vec<(String, Option<wordcraft_doc::RevisionKind>)>, Option<wordcraft_doc::RevisionKind>)>;

    fn rev_runs(doc: &Document) -> RevRuns {
        let kind = |i: Option<u32>| i.and_then(|i| doc.revisions.get(i as usize)).map(|r| r.kind);
        doc.body
            .iter()
            .filter_map(|b| if let wordcraft_doc::Block::Para(p) = &**b { Some(p) } else { None })
            .map(|p| {
                let runs = p.run_ranges().map(|(r, cp)| (p.text[r].to_string(), kind(cp.del).or(kind(cp.ins)))).collect();
                (runs, kind(p.mark.del).or(kind(p.mark.ins)))
            })
            .collect()
    }

    fn insertion_and_deletion() -> Vec<u8> {
        let regions = region("ct1", "insertion", "Alice", "2024-03-01T10:00:00", "")
            + &region("ct2", "deletion", "Bob", "2024-03-02T11:30:00", "<text:p>old<text:s/></text:p>");
        tracked(
            &regions,
            r#"<text:p>Keep <text:change-start text:change-id="ct1"/>new <text:change-end text:change-id="ct1"/><text:change text:change-id="ct2"/>words.</text:p>"#,
        )
    }

    #[test]
    fn tracked_insertion_and_deletion_import_with_authors_and_dates() {
        use wordcraft_doc::RevisionKind::{Delete, Insert};
        let doc = import(&insertion_and_deletion()).unwrap();
        let s = |v: &str| v.to_string();
        assert_eq!(rev_runs(&doc), vec![(vec![(s("Keep "), None), (s("new "), Some(Insert)), (s("old "), Some(Delete)), (s("words."), None)], None)]);
        let revs: Vec<_> = doc.revisions.iter().map(|r| (r.kind, r.author.as_str(), r.date.as_str())).collect();
        assert_eq!(revs, vec![(Insert, "Alice", "2024-03-01T10:00:00"), (Delete, "Bob", "2024-03-02T11:30:00")]);
        assert!(doc.settings.track_changes, "text:track-changes=\"true\" turns tracking on");
    }

    #[test]
    fn tracked_deletion_across_paragraphs_keeps_the_deleted_paragraph_mark() {
        use wordcraft_doc::RevisionKind::Delete;
        // "First tail¶head rest." with "tail¶head " deleted: the body holds the merged paragraph.
        let regions = region("d1", "deletion", "Bob", "2024-03-02T11:30:00", "<text:p>tail</text:p><text:p>head<text:s/></text:p>");
        let doc = import(&tracked(&regions, r#"<text:p>First <text:change text:change-id="d1"/>rest.</text:p>"#)).unwrap();
        let s = |v: &str| v.to_string();
        assert_eq!(
            rev_runs(&doc),
            vec![(vec![(s("First "), None), (s("tail"), Some(Delete))], Some(Delete)), (vec![(s("head "), Some(Delete)), (s("rest."), None)], None),]
        );
    }

    #[test]
    fn tracked_changes_round_trip_through_export() {
        let regions = region("ct1", "insertion", "Alice", "2024-03-01T10:00:00", "")
            + &region("d1", "deletion", "Bob", "2024-03-02T11:30:00", "<text:p>tail</text:p><text:p></text:p><text:p>head<text:s/></text:p>");
        let body = r#"<text:p>First <text:change text:change-id="d1"/>rest <text:change-start text:change-id="ct1"/>added</text:p><text:p>whole new<text:change-end text:change-id="ct1"/></text:p><text:p>End.</text:p>"#;
        let doc = import(&tracked(&regions, body)).unwrap();
        let back = import(&export(&doc).unwrap()).unwrap();
        assert_eq!(rev_runs(&back), rev_runs(&doc));
        let revs = |d: &Document| d.revisions.iter().map(|r| (r.kind, r.author.clone(), r.date.clone())).collect::<Vec<_>>();
        assert_eq!(revs(&back), revs(&doc));
        assert_eq!(back.settings.track_changes, doc.settings.track_changes);
        // The inserted paragraph mark survives: "added¶" is part of the insertion.
        assert_eq!(rev_runs(&doc)[2].1, Some(wordcraft_doc::RevisionKind::Insert));
    }

    #[test]
    fn hostile_tracked_changes_are_bounded() {
        // Unknown and duplicate ids, a change-end without a start, nested regions, a deletion
        // inside a deletion, a change mark inside the deleted text and tracked changes inside a paragraph.
        let regions = region("d1", "deletion", "B", "x", r#"<text:p>gone<text:change text:change-id="d1"/></text:p>"#)
            + r#"<text:changed-region text:id="n"><text:changed-region text:id="m"><text:deletion><text:p>inner</text:p></text:deletion></text:changed-region></text:changed-region>"#
            + &region("dd", "deletion", "B", "x", "<text:deletion><text:p>deeper</text:p></text:deletion><text:p>ok</text:p>")
            + &region("d1", "insertion", "C", "y", "");
        let body = r#"<text:p>a<text:change-end text:change-id="zz"/><text:change text:change-id="d1"/><text:change text:change-id="d1"/><text:change-start text:change-id="nope"/>b<text:change text:change-id="m"/><text:change text:change-id="dd"/><text:tracked-changes><text:changed-region text:id="q"/></text:tracked-changes>c</text:p>"#;
        let doc = import(&tracked(&regions, body)).unwrap();
        let text: String = rev_runs(&doc).iter().flat_map(|(r, _)| r.iter().map(|(t, _)| t.clone())).collect();
        assert_eq!(text.matches("gone").count(), 1, "a deletion is placed once: {text}");
        assert!(!text.contains("deeper"), "{text}");
        assert!(text.starts_with('a') && text.ends_with('c'), "{text}");

        // Many regions are read whole (a region marks a run at most); past the cap the file is
        // refused, not read without the rest (tests cap regions lower, see `MAX_REGIONS`).
        let regions = |n: usize| {
            let many: String = (0..n).map(|i| format!(r#"<text:changed-region text:id="r{i}"><text:insertion/></text:changed-region>"#)).collect();
            let starts: String =
                (0..n).map(|i| format!(r#"<text:change-start text:change-id="r{i}"/>x<text:change-end text:change-id="r{i}"/>"#)).collect();
            tracked(&many, &format!("<text:p>{starts}</text:p>"))
        };
        assert!(import(&regions(MAX_REGIONS + 1)).is_err());
        let doc = import(&regions(MAX_REGIONS)).unwrap();
        assert!(doc.revisions.len() <= MAX_REGIONS, "{}", doc.revisions.len());
    }

    /// Every paragraph (body and table cells, in order) as (text, inserted, deleted) runs plus
    /// its mark's (inserted, deleted).
    type Marks = Vec<(Vec<(String, bool, bool)>, (bool, bool))>;

    fn marks(doc: &Document) -> Marks {
        fn walk(bl: &wordcraft_doc::Blocks, out: &mut Marks) {
            for b in bl {
                match &**b {
                    wordcraft_doc::Block::Para(p) => out.push((
                        p.run_ranges().map(|(r, cp)| (p.text[r].to_string(), cp.ins.is_some(), cp.del.is_some())).collect(),
                        (p.mark.ins.is_some(), p.mark.del.is_some()),
                    )),
                    wordcraft_doc::Block::Table(t) => {
                        for r in &t.rows {
                            for c in &r.cells {
                                walk(&c.blocks, out);
                            }
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&doc.body, &mut out);
        out
    }

    #[test]
    fn inserted_then_deleted_text_keeps_both_marks() {
        // "new" was inserted by Alice, then Bob deleted "old new": the deletion's content holds
        // the insertion as a change range.
        let regions = region("i1", "insertion", "Alice", "2024-03-01T10:00:00", "")
            + &region(
                "d1",
                "deletion",
                "Bob",
                "2024-03-02T11:30:00",
                r#"<text:p>old <text:change-start text:change-id="i1"/>new<text:change-end text:change-id="i1"/></text:p>"#,
            );
        let doc = import(&tracked(&regions, r#"<text:p>Keep <text:change text:change-id="d1"/>end.</text:p>"#)).unwrap();
        let s = |v: &str| v.to_string();
        let want =
            vec![(vec![(s("Keep "), false, false), (s("old "), false, true), (s("new"), true, true), (s("end."), false, false)], (false, false))];
        assert_eq!(marks(&doc), want);
        let back = import(&export(&doc).unwrap()).unwrap();
        assert_eq!(marks(&back), want, "both marks survive a save");
        assert_eq!(back.revisions.len(), doc.revisions.len(), "one deletion stays one region and one revision: {:?}", back.revisions);
    }

    #[test]
    fn a_deleted_mark_inside_an_insertion_is_not_inserted() {
        // "A" inserted by Alice, then a paragraph break Bob deleted, then "C" inserted by Alice:
        // the insertion range runs across the deletion point, but the deleted mark wasn't inserted.
        let regions = region("i1", "insertion", "Alice", "2024-03-01T10:00:00", "")
            + &region("d1", "deletion", "Bob", "2024-03-02T11:30:00", "<text:p></text:p><text:p></text:p>");
        let body =
            r#"<text:p><text:change-start text:change-id="i1"/>A<text:change text:change-id="d1"/>C<text:change-end text:change-id="i1"/></text:p>"#;
        let doc = import(&tracked(&regions, body)).unwrap();
        let s = |v: &str| v.to_string();
        let want = vec![(vec![(s("A"), true, false)], (false, true)), (vec![(s("C"), true, false)], (false, false))];
        assert_eq!(marks(&doc), want);
        let back = import(&export(&doc).unwrap()).unwrap();
        assert_eq!(marks(&back), want, "after a save");
    }

    #[test]
    fn insertions_end_before_a_table_and_inside_a_cell() {
        let regions = region("i1", "insertion", "Alice", "2024-03-01T10:00:00", "") + &region("i2", "insertion", "Bob", "2024-03-02T10:00:00", "");
        let table = r#"<table:table><table:table-column/><table:table-row><table:table-cell><text:p>Cell <text:change-start text:change-id="i2"/>added</text:p><text:change-end text:change-id="i2"/></table:table-cell></table:table-row></table:table>"#;
        let body = format!(
            r#"<text:p>Start</text:p><text:p><text:change-start text:change-id="i1"/>New paragraph</text:p><text:change-end text:change-id="i1"/>{table}<text:p>End.</text:p>"#
        );
        let content = format!(
            r#"<office:document-content {NS_TC} xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"><office:body><office:text><text:tracked-changes>{regions}</text:tracked-changes>{body}</office:text></office:body></office:document-content>"#
        );
        let doc = import(&package(&content, None)).unwrap();
        let s = |v: &str| v.to_string();
        let want = vec![
            (vec![(s("Start"), false, false)], (false, false)),
            (vec![(s("New paragraph"), true, false)], (true, false)),
            (vec![(s("Cell "), false, false), (s("added"), true, false)], (true, false)),
            (vec![(s("End."), false, false)], (false, false)),
        ];
        assert_eq!(marks(&doc), want);
        let back = import(&export(&doc).unwrap()).unwrap();
        assert_eq!(marks(&back), want);
        let authors = |d: &Document| d.revisions.iter().map(|r| r.author.clone()).collect::<Vec<_>>();
        assert_eq!(authors(&back), authors(&doc));
    }

    #[test]
    fn exported_changes_always_have_a_date_and_keep_paragraph_separators() {
        // A change without a date gets one on save (`dc:date` is required); a U+2029 in deleted
        // text is text, not a paragraph break.
        let regions = r#"<text:changed-region text:id="d1"><text:deletion><office:change-info><dc:creator>Bob</dc:creator></office:change-info><text:p>a&#x2029;b</text:p></text:deletion></text:changed-region>"#;
        let doc = import(&tracked(regions, r#"<text:p>x<text:change text:change-id="d1"/>y</text:p>"#)).unwrap();
        assert_eq!(doc.revisions.first().map(|r| r.date.as_str()), Some(""));
        let back = import(&export(&doc).unwrap()).unwrap();
        assert!(back.revisions.first().is_some_and(|r| r.date.len() >= 19 && r.date.starts_with("20")), "{:?}", back.revisions);
        assert_eq!(marks(&back), marks(&doc));
        assert_eq!(marks(&back).len(), 1);
    }

    #[test]
    fn spaces_after_a_tab_survive_a_round_trip() {
        for src in ["a\t b", "a\t  b", "a\t   éa", "é\t 😀a", "a\t\t b"] {
            let doc = Document::from_text(src);
            let back = import(&export(&doc).unwrap()).unwrap();
            assert_eq!(back.plain_text(wordcraft_doc::StoryRef::Body).trim_end(), src, "{src:?}");
        }
    }

    #[test]
    fn whitespace_in_attributes_is_trimmed() {
        let content = format!(
            r#"<office:document-content {NS_T}><office:body><office:text>
            <text:h text:outline-level=" 2 ">Heading</text:h>
            <text:p>A<text:s text:c=" 3 "/>B</text:p>
            <table:table>
                <table:table-column/>
                <table:table-row>
                    <table:table-cell table:number-columns-spanned=" 2 " table:number-rows-spanned=" 1 ">
                        <text:p>Cell</text:p>
                    </table:table-cell>
                </table:table-row>
            </table:table>
            </office:text></office:body></office:document-content>"#
        );
        let flow = parse(&package(&content, None)).unwrap();
        let h = match &flow.blocks[0] {
            FBlock::Para(p) => p.kind,
            _ => panic!("expected heading para"),
        };
        assert_eq!(h, Kind::Heading(2));

        let p_text = match &flow.blocks[1] {
            FBlock::Para(p) => p.text(),
            _ => panic!("expected paragraph"),
        };
        assert_eq!(p_text, "A   B");

        let colspan = match &flow.blocks[2] {
            FBlock::Table(t) => t.rows[0][0].colspan,
            _ => panic!("expected table"),
        };
        assert_eq!(colspan, 2);
    }

    #[test]
    fn explicit_spaces_at_paragraph_ends_are_kept() {
        // `text:s` spaces are content; only collapsible XML whitespace is trimmed at the ends.
        let content = format!(
            r#"<office:document-content {NS_T}><office:body><office:text><text:p><text:s text:c="4"/>lead</text:p><text:p>trail<text:s text:c="2"/></text:p><text:p>  plain  </text:p><text:p>  <text:s/>x <text:s/> </text:p></office:text></office:body></office:document-content>"#
        );
        let f = parse(&package(&content, None)).unwrap();
        let texts: Vec<String> = paras(&f).iter().map(|p| p.text()).collect();
        assert_eq!(texts, vec!["    lead", "trail  ", "plain", " x  "]);
        let d = Document::from_text("    indented code  ");
        let back = import(&export(&d).unwrap()).unwrap();
        assert_eq!(back.plain_text(wordcraft_doc::StoryRef::Body).trim_end_matches('\n'), "    indented code  ");
    }
}
