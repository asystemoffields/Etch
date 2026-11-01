//! The limits every document is held to, in one place.
//!
//! What WordCraft lets a person make, it saves, and opens again, whole. So the same numbers bind
//! three sides:
//! - **the editor** refuses an edit that would take a document past one of them (the engine's
//!   `Session::run`, which counts only what the edit changed);
//! - **every reader** refuses a file past one of them with an error naming it, never dropping
//!   what is over (a later save would quietly destroy it);
//! - **the readers' byte limits** ([`MAX_XML_PART_BYTES`], [`MAX_PACKAGE_BYTES`], …) follow from
//!   [`SAVED_BYTES`] and [`MEDIA_BYTES`]: a document within the limits is written into parts
//!   within them, so the file it saves opens again.
//!
//! The numbers are far beyond real documents (a 1,000-page book is some 25,000 paragraphs and a
//! few hundred thousand runs) and sized so that a document at every limit at once opens on a 7 GB
//! laptop: within about 2 GB. A [`Tally`] counts a document against them; [`Tally::of_blocks`] counts part of
//! one, which is how an edit is checked without walking the whole document.

use std::sync::Arc;

use serde::ser::{self, Serialize};

use crate::{Block, Document, InlineObject, Paragraph, PartKind, Revision};

/// Paragraphs, every story counted (body, table cells, headers, footers, notes, comments, text
/// boxes). A paragraph costs about 1 KB in memory however short it is, and laying one out about
/// 3 KB more: 250,000 open in 0.7 GB and lay out in 1.3 GB (some 10,000 pages).
pub const PARAGRAPHS: usize = 250_000;
/// Runs (stretches of text in one formatting), every story counted: about 350 bytes each in
/// memory, whatever they hold; three a paragraph at the limit.
pub const RUNS: usize = 750_000;
/// Secondary stories: headers, footers, footnotes, endnotes, comments and text boxes together.
pub const STORIES: usize = 25_000;
/// Styles, built-in ones included.
pub const STYLES: usize = 10_000;
/// Lists: list definitions, and lists using them (a restart is a list of its own), each.
pub const LISTS: usize = 10_000;
/// Sections.
pub const SECTIONS: usize = 5_000;
/// Revisions: who made a tracked change, when, and what kind.
pub const REVISIONS: usize = 100_000;
/// Bibliography sources.
pub const SOURCES: usize = 10_000;
/// Pictures (distinct images; one shown many times counts once).
pub const PICTURES: usize = 5_000;
/// Bytes of pictures, and of the parts a file's objects, fonts and macros bring along, together.
pub const MEDIA_BYTES: u64 = 256 << 20;
/// The document's size: its saved XML, at most, as [`Tally::saved_bytes`] estimates it (an upper
/// bound of what the Word writer emits, tested against it).
pub const SAVED_BYTES: u64 = 256 << 20;
/// Parts a file's embedded objects, macros, fonts and unmodelled content bring along, kept and
/// written back as they are (the editor makes none).
pub const KEPT_PARTS: usize = 4096;

// What one value may hold. The editor cuts what it is given to these (or refuses more), and a
// reader refuses a file with more.

/// Characters of an author's name (tracked changes, comments).
pub const AUTHOR_CHARS: usize = 255;
/// Tab stops of one paragraph or style.
pub const TAB_STOPS: usize = 64;
/// Characters of a list level's number text (`%1.`, a bullet).
pub const LIST_LEVEL_TEXT_CHARS: usize = 64;
/// Characters of a watermark's text.
pub const WATERMARK_TEXT_CHARS: usize = 200;
/// Tables inside tables an edit may make (a paragraph is inside at most this many).
pub const TABLE_NESTING: usize = 16;
/// Tables inside tables a reader keeps (more than the editor makes; a Word file nesting deeper is
/// refused).
pub const TABLE_NESTING_READ: usize = 24;

/// Whether every table of `doc` is within what the Word reader takes ([`TABLE_NESTING_READ`] deep,
/// [`crate::table::MAX_ROWS`] rows, [`crate::table::MAX_COLS`] cells a row): what is past it,
/// named, when one isn't. A document read from another format that holds such a table would
/// save into a file that doesn't open again, so opening refuses it.
pub fn tables_over(doc: &Document) -> Option<String> {
    fn walk(blocks: &[Arc<Block>], depth: usize) -> Option<String> {
        for b in blocks {
            let Block::Table(t) = &**b else { continue };
            if depth >= TABLE_NESTING_READ {
                return Some(format!("tables nested more than {TABLE_NESTING_READ} deep"));
            }
            if t.rows.len() > crate::table::MAX_ROWS {
                return Some(format!("a table with more than {} rows", crate::table::MAX_ROWS));
            }
            for r in &t.rows {
                if r.cells.len() > crate::table::MAX_COLS {
                    return Some(format!("a table row with more than {} cells", crate::table::MAX_COLS));
                }
                for c in &r.cells {
                    if let Some(w) = walk(&c.blocks, depth + 1) {
                        return Some(w);
                    }
                }
            }
        }
        None
    }
    walk(&doc.body, 0).or_else(|| doc.parts.values().find_map(|p| walk(&p.blocks, 0)))
}

/// Most bytes one XML part of a Word package (or a Flat OPC part, or an OpenDocument entry) may
/// inflate to: every part WordCraft writes for a document within [`SAVED_BYTES`] is smaller.
pub const MAX_XML_PART_BYTES: u64 = SAVED_BYTES + (64 << 20);
/// Most bytes a package's parts may inflate to together: the document's XML, its pictures and
/// kept parts, and room for the fixed parts.
pub const MAX_PACKAGE_BYTES: u64 = SAVED_BYTES + MEDIA_BYTES + (64 << 20);
/// Most parts in a package: one a header or footer, one a picture, the kept parts, and the
/// fixed ones.
pub const MAX_PACKAGE_PARTS: usize = STORIES + PICTURES + KEPT_PARTS + 256;
/// Most relationships one part may have: a hyperlink's at most one a run, a header's, footer's
/// and picture's one each, the kept parts', and the fixed ones.
pub const MAX_PART_RELATIONSHIPS: usize = RUNS + STORIES + PICTURES + KEPT_PARTS + 256;

/// What a document is counted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Measure {
    Paragraphs,
    Runs,
    Stories,
    Styles,
    Lists,
    Sections,
    Revisions,
    Sources,
    Pictures,
    MediaBytes,
    SavedBytes,
}

impl Measure {
    pub const ALL: [Measure; 11] = [
        Measure::Paragraphs,
        Measure::Runs,
        Measure::Stories,
        Measure::Styles,
        Measure::Lists,
        Measure::Sections,
        Measure::Revisions,
        Measure::Sources,
        Measure::Pictures,
        Measure::MediaBytes,
        Measure::SavedBytes,
    ];

    /// What is counted, in words (plural).
    pub fn what(self) -> &'static str {
        match self {
            Measure::Paragraphs => "paragraphs",
            Measure::Runs => "runs of formatted text",
            Measure::Stories => "headers, footers, notes, comments and text boxes",
            Measure::Styles => "styles",
            Measure::Lists => "lists",
            Measure::Sections => "sections",
            Measure::Revisions => "revisions (authors and times of tracked changes)",
            Measure::Sources => "bibliography sources",
            Measure::Pictures => "pictures",
            Measure::MediaBytes => "of pictures and embedded files",
            Measure::SavedBytes => "of saved text and formatting",
        }
    }
}

/// The limits a document is held to (see the module's documentation). [`Limits::DOCUMENT`] are
/// everyone's; tests use smaller ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub paragraphs: u64,
    pub runs: u64,
    pub stories: u64,
    pub styles: u64,
    pub lists: u64,
    pub sections: u64,
    pub revisions: u64,
    pub sources: u64,
    pub pictures: u64,
    pub media_bytes: u64,
    pub saved_bytes: u64,
}

impl Limits {
    pub const DOCUMENT: Limits = Limits {
        paragraphs: PARAGRAPHS as u64,
        runs: RUNS as u64,
        stories: STORIES as u64,
        styles: STYLES as u64,
        lists: LISTS as u64,
        sections: SECTIONS as u64,
        revisions: REVISIONS as u64,
        sources: SOURCES as u64,
        pictures: PICTURES as u64,
        media_bytes: MEDIA_BYTES,
        saved_bytes: SAVED_BYTES,
    };

    pub fn get(&self, m: Measure) -> u64 {
        match m {
            Measure::Paragraphs => self.paragraphs,
            Measure::Runs => self.runs,
            Measure::Stories => self.stories,
            Measure::Styles => self.styles,
            Measure::Lists => self.lists,
            Measure::Sections => self.sections,
            Measure::Revisions => self.revisions,
            Measure::Sources => self.sources,
            Measure::Pictures => self.pictures,
            Measure::MediaBytes => self.media_bytes,
            Measure::SavedBytes => self.saved_bytes,
        }
    }

    pub fn set(&mut self, m: Measure, v: u64) {
        match m {
            Measure::Paragraphs => self.paragraphs = v,
            Measure::Runs => self.runs = v,
            Measure::Stories => self.stories = v,
            Measure::Styles => self.styles = v,
            Measure::Lists => self.lists = v,
            Measure::Sections => self.sections = v,
            Measure::Revisions => self.revisions = v,
            Measure::Sources => self.sources = v,
            Measure::Pictures => self.pictures = v,
            Measure::MediaBytes => self.media_bytes = v,
            Measure::SavedBytes => self.saved_bytes = v,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Limits::DOCUMENT
    }
}

/// A count that is over its limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Over {
    pub measure: Measure,
    pub count: u64,
    pub limit: u64,
}

impl std::fmt::Display for Over {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Bytes in MB, unless the two would read the same (one byte past the limit).
        let bytes = matches!(self.measure, Measure::MediaBytes | Measure::SavedBytes);
        let n = |v: u64| {
            if !bytes {
                grouped(v)
            } else if size(self.count) != size(self.limit) {
                size(v)
            } else {
                format!("{} bytes", grouped(v))
            }
        };
        write!(f, "{} {}, more than the {} a document may have", n(self.count), self.measure.what(), n(self.limit))
    }
}

/// `bytes` in MB (or KB, below one), to a tenth.
fn size(bytes: u64) -> String {
    let b = bytes as f64;
    if bytes >= 1 << 20 { format!("{:.1} MB", b / 1_048_576.0) } else { format!("{:.1} KB", b / 1024.0) }
}

/// `n` with thousands separated by commas.
fn grouped(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A document (or part of one) counted against the [`Limits`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub paragraphs: u64,
    pub runs: u64,
    pub stories: u64,
    pub styles: u64,
    pub lists: u64,
    pub sections: u64,
    pub revisions: u64,
    pub sources: u64,
    pub pictures: u64,
    pub media_bytes: u64,
    /// An upper bound of the XML the Word writer emits for what was counted.
    pub saved_bytes: u64,
}

impl Tally {
    pub fn get(&self, m: Measure) -> u64 {
        match m {
            Measure::Paragraphs => self.paragraphs,
            Measure::Runs => self.runs,
            Measure::Stories => self.stories,
            Measure::Styles => self.styles,
            Measure::Lists => self.lists,
            Measure::Sections => self.sections,
            Measure::Revisions => self.revisions,
            Measure::Sources => self.sources,
            Measure::Pictures => self.pictures,
            Measure::MediaBytes => self.media_bytes,
            Measure::SavedBytes => self.saved_bytes,
        }
    }

    fn zip(self, o: Tally, f: impl Fn(u64, u64) -> u64) -> Tally {
        Tally {
            paragraphs: f(self.paragraphs, o.paragraphs),
            runs: f(self.runs, o.runs),
            stories: f(self.stories, o.stories),
            styles: f(self.styles, o.styles),
            lists: f(self.lists, o.lists),
            sections: f(self.sections, o.sections),
            revisions: f(self.revisions, o.revisions),
            sources: f(self.sources, o.sources),
            pictures: f(self.pictures, o.pictures),
            media_bytes: f(self.media_bytes, o.media_bytes),
            saved_bytes: f(self.saved_bytes, o.saved_bytes),
        }
    }

    pub fn plus(self, o: Tally) -> Tally {
        self.zip(o, u64::saturating_add)
    }

    pub fn minus(self, o: Tally) -> Tally {
        self.zip(o, u64::saturating_sub)
    }

    /// The first count over its limit, if any.
    pub fn over(&self, limits: &Limits) -> Option<Over> {
        Measure::ALL.into_iter().map(|m| Over { measure: m, count: self.get(m), limit: limits.get(m) }).find(|o| o.count > o.limit)
    }

    /// The first count over its limit that is more than in `before` (an edit may leave a
    /// document that was already over a limit as it is, or take it back under).
    pub fn grown_over(&self, before: &Tally, limits: &Limits) -> Option<Over> {
        Measure::ALL
            .into_iter()
            .map(|m| Over { measure: m, count: self.get(m), limit: limits.get(m) })
            .find(|o| o.count > o.limit && o.count > before.get(o.measure))
    }

    /// The whole document.
    pub fn of(doc: &Document) -> Tally {
        Tally::of_with(doc, &mut Memo::default())
    }

    fn of_with<'d>(doc: &'d Document, m: &mut Memo<'d>) -> Tally {
        let mut t = blocks_tally(&doc.body, doc, m);
        for p in doc.parts.values() {
            t = t.plus(Tally::of_part_with(p, doc, m));
        }
        t.plus(Tally::doc_wide(doc))
    }

    /// One secondary story: its blocks and what its part costs on its own.
    pub fn of_part(p: &crate::Part, doc: &Document) -> Tally {
        Tally::of_part_with(p, doc, &mut Memo::default())
    }

    fn of_part_with<'d>(p: &'d crate::Part, doc: &Document, m: &mut Memo<'d>) -> Tally {
        let own = Tally { stories: 1, saved_bytes: part_bytes(p.kind), ..Tally::default() };
        own.plus(blocks_tally(&p.blocks, doc, m))
    }

    /// Blocks of a story (`doc` gives what they refer to: revisions' authors). Sections are
    /// counted by the paragraphs that end one: a story other than the body has none.
    pub fn of_blocks(blocks: &[Arc<Block>], doc: &Document) -> Tally {
        blocks_tally(blocks, doc, &mut Memo::default())
    }

    pub fn of_block(b: &Block, doc: &Document) -> Tally {
        block_tally(b, doc, 0, &mut Memo::default())
    }

    /// A table on its own, without its rows (counted with [`Tally::of_row`]).
    pub fn of_table_alone(t: &crate::Table, doc: &Document) -> Tally {
        let revs: &[Revision] = &doc.revisions;
        let own = TABLE.saturating_add(weigh(&t.props, revs)).saturating_add(weigh(&t.grid, revs)).saturating_add(weigh(&t.controls, revs));
        Tally { saved_bytes: own, ..Tally::default() }
    }

    /// One table row: its cells' blocks and what the row costs on its own.
    pub fn of_row(r: &crate::table::Row, doc: &Document) -> Tally {
        row_tally(r, doc, 1, &mut Memo::default())
    }

    /// What isn't in a story's blocks: the counts of document-wide tables (secondary stories,
    /// styles, lists, revisions, sources, pictures) and the saved size of the parts that aren't
    /// stories (styles, lists, comments' details, settings, properties, sources, the last
    /// section, note separators).
    pub fn doc_wide(doc: &Document) -> Tally {
        Tally::shared_parts(doc).plus(Tally::misc(doc)).plus(Tally::notes_parts(doc))
    }

    /// The footnotes' and the endnotes' parts on their own (namespaces, the separator notes,
    /// relationship and content type), each when the document has a note of its kind. Counted
    /// again whenever the secondary stories change.
    pub fn notes_parts(doc: &Document) -> Tally {
        let has = |kind: PartKind| doc.parts.values().any(|p| p.kind == kind);
        let n = u64::from(has(PartKind::Footnote)) + u64::from(has(PartKind::Endnote));
        Tally { saved_bytes: n.saturating_mul(NOTES_PART), ..Tally::default() }
    }

    /// The document-wide tables held as [`crate::Shared`] (an edit that leaves one as it was
    /// leaves it the same pointer): secondary stories (their count only; their blocks are counted
    /// with [`Tally::of_part`]), styles, lists, comments' details, revisions, pictures and kept
    /// parts.
    pub fn shared_parts(doc: &Document) -> Tally {
        Tally::styles_of(doc).plus(Tally::lists_of(doc)).plus(Tally::comments_of(doc)).plus(Tally::revisions_of(doc)).plus(Tally::media_of(doc))
    }

    pub fn styles_of(doc: &Document) -> Tally {
        let s = &doc.styles;
        let bytes = s.styles.iter().map(|st| STYLE.saturating_add(weigh(st, &[]))).fold(STYLES_PART, u64::saturating_add);
        let bytes = bytes
            .saturating_add(weigh(&s.default_chr, &[]))
            .saturating_add(weigh(&s.default_para, &[]))
            .saturating_add(s.latent_styles.as_ref().map_or(0, |x| x.len() as u64));
        Tally { styles: s.styles.len() as u64, saved_bytes: bytes, ..Tally::default() }
    }

    pub fn lists_of(doc: &Document) -> Tally {
        let n = &doc.numbering;
        let bytes = n.abstracts.iter().map(|a| LIST.saturating_add(weigh(a, &[]))).fold(LISTS_PART, u64::saturating_add);
        let bytes = n.nums.iter().map(|x| LIST.saturating_add(weigh(x, &[]))).fold(bytes, u64::saturating_add);
        Tally { lists: n.abstracts.len().max(n.nums.len()) as u64, saved_bytes: bytes, ..Tally::default() }
    }

    pub fn comments_of(doc: &Document) -> Tally {
        let own = if doc.comments.is_empty() { 0 } else { COMMENTS_PARTS };
        let bytes = doc.comments.values().map(|c| COMMENT.saturating_add(weigh(c, &[]))).fold(own, u64::saturating_add);
        Tally { saved_bytes: bytes, ..Tally::default() }
    }

    pub fn revisions_of(doc: &Document) -> Tally {
        Tally { revisions: doc.revisions.len() as u64, ..Tally::default() }
    }

    pub fn media_of(doc: &Document) -> Tally {
        let bytes = doc.media.values().chain(doc.passthrough.values()).map(|b| b.len() as u64).fold(0u64, u64::saturating_add);
        // Each picture's part name, content type and relationship.
        let names =
            doc.media.keys().chain(doc.passthrough.keys()).map(|k| MEDIA_ENTRY.saturating_add(12 * k.len() as u64)).fold(0u64, u64::saturating_add);
        Tally { pictures: doc.media.len() as u64, media_bytes: bytes, saved_bytes: names, ..Tally::default() }
    }

    /// The rest: sources, settings, properties, the last section, note separators and the glossary (not
    /// shared, so compared by value when an edit is checked), and the fixed parts.
    pub fn misc(doc: &Document) -> Tally {
        let mut bytes = FIXED_PARTS;
        bytes = bytes.saturating_add(weigh(&doc.sources, &[]));
        bytes = bytes.saturating_add(weigh(&doc.settings, &[]));
        // Twice: the core properties are written in `core.xml` and some again in `app.xml` (the
        // creator as the last to modify it, too).
        bytes = bytes.saturating_add(weigh(&doc.core, &[]).saturating_mul(2));
        bytes = bytes.saturating_add(weigh(&doc.custom_props, &[]));
        bytes = bytes.saturating_add(SECTION.saturating_add(weigh(&doc.last_section, &doc.revisions)));
        for seps in [&doc.footnote_separators, &doc.endnote_separators] {
            for blocks in [&seps.separator, &seps.continuation_separator, &seps.continuation_notice].into_iter().flatten() {
                bytes = bytes.saturating_add(Tally::of_blocks(blocks, doc).saved_bytes);
            }
        }
        // The glossary document (building blocks), written to its own part.
        for bb in &doc.glossary {
            let names = weigh(&(&bb.name, &bb.gallery, &bb.category, &bb.description, &bb.guid, &bb.style, &bb.types, &bb.behaviors), &[]);
            bytes = bytes.saturating_add(names).saturating_add(Tally::of_blocks(&bb.blocks, doc).saved_bytes);
        }
        Tally { sources: doc.sources.len() as u64, sections: 1, saved_bytes: bytes, ..Tally::default() }
    }
}

// What the Word writer spends, at most, on the pieces of a document, beyond what [`weigh`] counts
// for their properties. Tested against the writer: `engine::tests_limits` and the fidelity corpus
// (`crates/engine/tests/format_fidelity.rs`).

/// A paragraph: `<w:p>` with its ids, `<w:pPr>` and the mark's `<w:rPr>`.
const PARA: u64 = 48;
/// A run: `<w:r>`, `<w:rPr>`, `<w:t xml:space="preserve">`.
const RUN: u64 = 60;
/// A tab, break or special hyphen inside a run: it ends the `<w:t>` and starts another.
const SPECIAL: u64 = 64;
/// A range mark (bookmark, comment range and its reference run, editable range, content
/// control, a complex field's begin or end), besides what [`weigh`] counts of it.
const MARKER: u64 = 256;
/// A simple field, a note reference, ruby text, kept markup: an object written in runs of its
/// own (a field's begin, instruction, separator, result and end: [`OBJECT_RUNS`]).
const OBJECT: u64 = 512;
const OBJECT_RUNS: u64 = 5;
/// A shape, group, chart or embedded object: `w:drawing` with its frame, properties and fallback.
const DRAWING: u64 = 4096;
/// A picture: `w:drawing` with its frame and `pic:pic`.
const PICTURE: u64 = 1536;
/// A footnote's or endnote's reference run.
const NOTE_REF: u64 = 256;
/// A chart's own part, content type and relationship (besides its data).
const CHART: u64 = 16 << 10;
/// A SmartArt graphic's parts (data, layout, colours, style, drawing).
const SMART_ART: u64 = 192 << 10;
/// A tracked change around a run (`<w:ins w:id w:author w:date>`, a move's range marks).
const REVISION: u64 = 192;
/// A hyperlink around a run, and its relationship.
const LINK: u64 = 384;
/// A section's `w:sectPr`, its header and footer references and their relationships (the
/// section's properties are weighed besides).
const SECTION: u64 = 512;
/// A table, a row and a cell on their own.
const TABLE: u64 = 512;
const ROW: u64 = 128;
const CELL: u64 = 128;
/// A header's or footer's own part: namespaces, content type and relationships.
const STORY_PART: u64 = 4096;
/// A note, comment or text box: its element and reference mark.
const STORY: u64 = 512;
/// A style, a list definition or list, a comment's details (`comments.xml` and its extension
/// parts, the paragraph id its last paragraph gets), a picture's part name, content type and
/// relationship.
const STYLE: u64 = 256;
const LIST: u64 = 256;
const COMMENT: u64 = 512;
/// The comments' four parts on their own (`comments.xml` and its extensions), when there are any.
const COMMENTS_PARTS: u64 = 16 << 10;
/// The footnotes' (or the endnotes') part on its own, when there are any.
const NOTES_PART: u64 = 8 << 10;
const MEDIA_ENTRY: u64 = 512;
/// The styles and numbering parts on their own, and the other fixed parts (content types,
/// relationships, settings, theme, font table, properties, web settings).
const STYLES_PART: u64 = 16 << 10;
const LISTS_PART: u64 = 16 << 10;
const FIXED_PARTS: u64 = 256 << 10;

fn part_bytes(kind: PartKind) -> u64 {
    match kind {
        PartKind::Header | PartKind::Footer => STORY_PART,
        PartKind::Footnote | PartKind::Endnote | PartKind::Comment | PartKind::TextBox => STORY,
    }
}

/// Tables nest at most this deep in a count (the readers and the editor keep them far
/// shallower).
const MAX_DEPTH: usize = 64;

/// What a count has weighed: the last formatting of each kind it weighed (a row's, a cell's, a
/// paragraph's, a run's) and its weight, so the next one formatted the same isn't weighed again.
/// Most documents format long stretches alike (every cell of a column, the runs of one style),
/// and weighing serializes formatting, which costs far more than comparing it (opening a
/// document counts it whole). The count is the same: [`weigh`] depends only on the value and the
/// revisions, which are the one document's throughout a count.
#[derive(Default)]
struct Memo<'d> {
    /// Weigh everything, remembering nothing (tests compare the two).
    #[cfg(test)]
    off: bool,
    row: Option<(&'d crate::props::RowProps, &'d crate::control::ControlWrap, u64)>,
    cell: Option<(&'d crate::props::CellProps, &'d crate::control::ControlWrap, u64)>,
    para: Option<(&'d crate::props::ParaProps, u64)>,
    chr: Option<(&'d crate::props::CharProps, u64)>,
}

impl<'d> Memo<'d> {
    fn keeps(&self) -> bool {
        #[cfg(test)]
        if self.off {
            return false;
        }
        true
    }

    /// A row on its own: [`ROW`], its formatting and its content controls.
    fn row(&mut self, r: &'d crate::table::Row, revs: &[Revision]) -> u64 {
        if let Some((p, k, w)) = self.row
            && *p == r.props
            && *k == r.controls
        {
            return w;
        }
        let w = ROW.saturating_add(weigh(&r.props, revs)).saturating_add(weigh(&r.controls, revs));
        if self.keeps() {
            self.row = Some((&r.props, &r.controls, w));
        }
        w
    }

    /// A cell on its own: [`CELL`], its formatting and its content controls.
    fn cell(&mut self, c: &'d crate::table::Cell, revs: &[Revision]) -> u64 {
        if let Some((p, k, w)) = self.cell
            && *p == c.props
            && *k == c.controls
        {
            return w;
        }
        let w = CELL.saturating_add(weigh(&c.props, revs)).saturating_add(weigh(&c.controls, revs));
        if self.keeps() {
            self.cell = Some((&c.props, &c.controls, w));
        }
        w
    }

    /// [`weigh_para`].
    fn para(&mut self, p: &'d crate::props::ParaProps, revs: &[Revision]) -> u64 {
        if let Some((q, w)) = self.para
            && *q == *p
        {
            return w;
        }
        let w = weigh_para(p, revs);
        if self.keeps() {
            self.para = Some((p, w));
        }
        w
    }

    /// [`weigh_chr`].
    fn chr(&mut self, c: &'d crate::props::CharProps, revs: &[Revision]) -> u64 {
        if let Some((q, w)) = self.chr
            && *q == *c
        {
            return w;
        }
        let w = weigh_chr(c, revs);
        if self.keeps() {
            self.chr = Some((c, w));
        }
        w
    }
}

fn blocks_tally<'d>(blocks: &'d [Arc<Block>], doc: &Document, m: &mut Memo<'d>) -> Tally {
    blocks.iter().map(|b| block_tally(b, doc, 0, m)).fold(Tally::default(), Tally::plus)
}

fn block_tally<'d>(b: &'d Block, doc: &Document, depth: usize, m: &mut Memo<'d>) -> Tally {
    match b {
        Block::Para(p) => para_tally(p, doc, m),
        Block::Table(_) if depth >= MAX_DEPTH => Tally::default(),
        Block::Table(t) => t.rows.iter().map(|r| row_tally(r, doc, depth + 1, m)).fold(Tally::of_table_alone(t, doc), Tally::plus),
    }
}

fn row_tally<'d>(r: &'d crate::table::Row, doc: &Document, depth: usize, m: &mut Memo<'d>) -> Tally {
    let revs: &[Revision] = &doc.revisions;
    let mut t = Tally { saved_bytes: m.row(r, revs), ..Tally::default() };
    for c in &r.cells {
        t.saved_bytes = t.saved_bytes.saturating_add(m.cell(c, revs));
        for b in &c.blocks {
            t = t.plus(block_tally(b, doc, depth, m));
        }
    }
    t
}

/// [`char_cost`] of each ASCII character, for text that is all ASCII (no object marks or
/// special hyphens in it).
const ASCII_COST: [u8; 128] = {
    let mut t = [1u8; 128];
    let mut i = 0;
    while i < 128 {
        // Tabs, breaks and the other control characters end the `<w:t>` ([`SPECIAL`]).
        if i < 0x20 || i == 0x7F {
            t[i] = SPECIAL as u8;
        }
        i += 1;
    }
    t[b'&' as usize] = 5;
    t[b'<' as usize] = 4;
    t[b'>' as usize] = 4;
    t[b'"' as usize] = 6;
    t[b'\'' as usize] = 6;
    t
};

fn para_tally<'d>(p: &'d Paragraph, doc: &Document, m: &mut Memo<'d>) -> Tally {
    let revs: &[Revision] = &doc.revisions;
    let own = m.para(&p.props, revs).saturating_add(m.chr(&p.mark, revs));
    let mut bytes = PARA.saturating_add(own);
    if p.props.drop_cap.is_some() {
        // Written as two paragraphs: the framed first character and the rest.
        bytes = bytes.saturating_add(PARA).saturating_add(own).saturating_add(RUN);
    }
    let mut sections = 0;
    if let Some(s) = &p.section {
        sections = 1;
        bytes = bytes.saturating_add(SECTION).saturating_add(weigh(&**s, revs));
    }
    for r in &p.runs {
        bytes = bytes.saturating_add(RUN).saturating_add(m.chr(&r.props, revs));
    }
    let mut objects = p.objects.iter();
    // All ASCII (most text): no object marks in it, so its objects are all counted below.
    let ascii = p.text.is_ascii();
    if ascii {
        let text = p.text.bytes().map(|b| ASCII_COST.get(usize::from(b)).map_or(1, |c| u64::from(*c))).fold(0, u64::saturating_add);
        bytes = bytes.saturating_add(text);
    } else {
        for (off, c) in p.text.char_indices() {
            let n: u64 = match c {
                crate::para::OBJ => match objects.next() {
                    Some(o) => object_bytes(o, weigh_chr(p.props_of_char(off), revs), revs),
                    None => 0,
                },
                c => char_cost(c),
            };
            bytes = bytes.saturating_add(n);
        }
    }
    // Objects the text has no mark for (the writer leaves them out): counted anyway.
    for o in objects {
        bytes = bytes.saturating_add(object_bytes(o, 0, revs));
    }
    Tally { paragraphs: 1, runs: p.runs.len() as u64, sections, saved_bytes: bytes, ..Tally::default() }
}

/// What a character of a paragraph's text other than an object mark costs.
fn char_cost(c: char) -> u64 {
    match c {
        '\t' | '\n' | '\u{000C}' | '\u{000E}' | '\u{2011}' | '\u{00AD}' => SPECIAL,
        c if c.is_control() => SPECIAL,
        c => escaped_char(c),
    }
}

/// What object `o` costs, written in a run whose properties weigh `props`.
fn object_bytes(o: &InlineObject, props: u64, revs: &[Revision]) -> u64 {
    let run = RUN.saturating_add(props);
    match o {
        InlineObject::FieldStart { .. } => MARKER.saturating_add(weigh(o, revs).saturating_mul(2)).saturating_add(run.saturating_mul(3)),
        InlineObject::FieldEnd => MARKER.saturating_add(run),
        _ if o.is_marker() => MARKER.saturating_add(weigh(o, revs).saturating_mul(2)),
        // Each piece of an equation is a run of its own, with its own properties.
        InlineObject::Equation { .. } => OBJECT.saturating_add(weigh(o, revs).saturating_mul(4)).saturating_add(run),
        InlineObject::Graphic { alt, float, graphic, .. } => {
            let g = &**graphic;
            let mut n = DRAWING.saturating_add(run).saturating_add(weigh(alt, revs).saturating_mul(2)).saturating_add(weigh(float, revs));
            if let Some(src) = &g.source {
                n = n.saturating_add(src.xml.len() as u64).saturating_add(weigh(&src.refs, revs));
            }
            if let Some(spec) = &g.chart {
                // The categories are written with each series.
                let cats = weigh(&spec.categories, revs).saturating_mul(spec.series.len().max(1) as u64);
                n = n.saturating_add(CHART).saturating_add(weigh(&**spec, revs).saturating_mul(2)).saturating_add(cats);
            }
            if let Some(spec) = &g.smart_art {
                n = n.saturating_add(SMART_ART).saturating_add(weigh(&**spec, revs).saturating_mul(4));
            }
            n
        }
        InlineObject::Group { children, .. } => {
            // The group's own frame, then each member (without its text: a member's text box is
            // a story of its own, counted with [`Tally::of_part`]).
            let mut n = DRAWING.saturating_add(run).saturating_add(weigh(o, revs).saturating_mul(2));
            for c in children {
                n = n.saturating_add(object_bytes(&c.obj, 0, revs));
            }
            n
        }
        // A picture (not an embedded object's): `w:drawing` with its frame, `pic:pic` and the
        // relationship id.
        InlineObject::Image { ole: None, .. } => PICTURE.saturating_add(run).saturating_add(weigh(o, revs).saturating_mul(2)),
        _ if o.is_drawing() => DRAWING.saturating_add(run).saturating_add(weigh(o, revs).saturating_mul(2)),
        // A note reference: one run with the reference's style.
        InlineObject::NoteRef { .. } => NOTE_REF.saturating_add(run).saturating_add(weigh(o, revs).saturating_mul(2)),
        InlineObject::Opaque { xml, .. } => OBJECT.saturating_add(run).saturating_add(xml.len() as u64).saturating_add(weigh(o, revs)),
        _ => OBJECT.saturating_add(weigh(o, revs).saturating_mul(2)).saturating_add(run.saturating_mul(OBJECT_RUNS)),
    }
}

/// Bytes `c` takes in XML text or an attribute value: escaped (`&amp;`, `&lt;`, `&quot;`…).
fn escaped_char(c: char) -> u64 {
    match c {
        '&' => 5,
        '<' | '>' => 4,
        '"' | '\'' => 6,
        c => c.len_utf8() as u64,
    }
}

/// [`escaped_char`] of a whole string.
fn escaped(s: &str) -> u64 {
    s.chars().map(escaped_char).fold(0, u64::saturating_add)
}

/// An upper bound of the XML the Word writer emits for `v`'s properties: each field set costs an
/// element (or an attribute), each number its digits, each string its characters escaped
/// (twice: a font is written as `w:ascii` and `w:hAnsi`), each list item an element. A field
/// naming a revision (`ins`, `del`, `rev`) adds the tracked change around what it marks, with the
/// revision's author and date from `revs`; a hyperlink (`link`) its element and relationship.
pub fn weigh<T: Serialize + ?Sized>(v: &T, revs: &[Revision]) -> u64 {
    let mut w = Weigh { bytes: 0, revs };
    match v.serialize(&mut w) {
        Ok(()) => w.bytes,
        // Serializing the model doesn't fail; if it ever did, it counts as over any budget.
        Err(_) => u64::MAX,
    }
}

/// [`weigh`] of a run's or a paragraph mark's formatting, the default (most of most documents)
/// without serializing it: counting a document on opening is that much cheaper.
fn weigh_chr(c: &crate::props::CharProps, revs: &[Revision]) -> u64 {
    static DEFAULT: std::sync::LazyLock<(crate::props::CharProps, u64)> = std::sync::LazyLock::new(|| {
        let d = crate::props::CharProps::default();
        let w = weigh(&d, &[]);
        (d, w)
    });
    if *c == DEFAULT.0 { DEFAULT.1 } else { weigh(c, revs) }
}

/// [`weigh_chr`] for a paragraph's formatting.
fn weigh_para(p: &crate::props::ParaProps, revs: &[Revision]) -> u64 {
    static DEFAULT: std::sync::LazyLock<(crate::props::ParaProps, u64)> = std::sync::LazyLock::new(|| {
        let d = crate::props::ParaProps::default();
        let w = weigh(&d, &[]);
        (d, w)
    });
    if *p == DEFAULT.0 { DEFAULT.1 } else { weigh(p, revs) }
}

/// A field of a property set: an element, or an attribute of one.
const FIELD: u64 = 24;
/// A list item, a map entry.
const ITEM: u64 = 16;
/// A string's attribute names and quotes (two attributes).
const STRING: u64 = 12;

struct Weigh<'a> {
    bytes: u64,
    revs: &'a [Revision],
}

impl Weigh<'_> {
    fn add(&mut self, n: u64) {
        self.bytes = self.bytes.saturating_add(n);
    }

    /// The tracked change a field naming revision `idx` puts around what it marks.
    fn revision(&mut self, idx: u64) {
        let r = usize::try_from(idx).ok().and_then(|i| self.revs.get(i));
        let strings = match r {
            // A move's range marks repeat the name, author and date.
            Some(r) => {
                let s = escaped(&r.author).saturating_add(escaped(&r.date));
                match &r.move_name {
                    Some(m) => s.saturating_add(escaped(m)).saturating_mul(2),
                    None => s,
                }
            }
            // An index no revision has is written with the anonymous author.
            None => 64,
        };
        self.add(REVISION.saturating_add(strings.saturating_mul(2)));
    }

    fn field<T: Serialize + ?Sized>(&mut self, key: &str, v: &T) -> Result<(), WeighError> {
        if matches!(key, "ins" | "del" | "rev") {
            let mut idx = RevIndex(None);
            // Not an index (a struct holding one, say): weighed as anything else below.
            if v.serialize(&mut idx).is_ok()
                && let Some(i) = idx.0
            {
                self.revision(i);
            }
        }
        if key == "link" {
            self.add(LINK);
        }
        let before = self.bytes;
        v.serialize(&mut *self)?;
        if self.bytes > before {
            self.add(FIELD);
        }
        Ok(())
    }
}

#[derive(Debug)]
struct WeighError;

impl std::fmt::Display for WeighError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("can't weigh")
    }
}

impl std::error::Error for WeighError {}

impl ser::Error for WeighError {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        WeighError
    }
}

/// The revision index a value holds, if it is one (`Some(n)` or `n`).
struct RevIndex(Option<u64>);

macro_rules! ignored {
    ($($f:ident($t:ty)),*) => {
        $(fn $f(self, _: $t) -> Result<(), WeighError> { Ok(()) })*
    };
}

macro_rules! index {
    ($($f:ident($t:ty)),*) => {
        $(fn $f(self, v: $t) -> Result<(), WeighError> { self.0 = Some(u64::from(v)); Ok(()) })*
    };
}

impl ser::Serializer for &mut RevIndex {
    type Ok = ();
    type Error = WeighError;
    type SerializeSeq = ser::Impossible<(), WeighError>;
    type SerializeTuple = ser::Impossible<(), WeighError>;
    type SerializeTupleStruct = ser::Impossible<(), WeighError>;
    type SerializeTupleVariant = ser::Impossible<(), WeighError>;
    type SerializeMap = ser::Impossible<(), WeighError>;
    type SerializeStruct = ser::Impossible<(), WeighError>;
    type SerializeStructVariant = ser::Impossible<(), WeighError>;
    index!(serialize_u8(u8), serialize_u16(u16), serialize_u32(u32), serialize_u64(u64));
    ignored!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_f32(f32),
        serialize_f64(f64),
        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8]),
        serialize_unit_struct(&'static str)
    );
    fn serialize_none(self) -> Result<(), WeighError> {
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<(), WeighError> {
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), WeighError> {
        Ok(())
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, _: &'static str) -> Result<(), WeighError> {
        Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _: &'static str, v: &T) -> Result<(), WeighError> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(self, _: &'static str, _: u32, _: &'static str, _: &T) -> Result<(), WeighError> {
        Ok(())
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, WeighError> {
        Err(WeighError)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, WeighError> {
        Err(WeighError)
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeTupleStruct, WeighError> {
        Err(WeighError)
    }
    fn serialize_tuple_variant(self, _: &'static str, _: u32, _: &'static str, _: usize) -> Result<Self::SerializeTupleVariant, WeighError> {
        Err(WeighError)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, WeighError> {
        Err(WeighError)
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeStruct, WeighError> {
        Err(WeighError)
    }
    fn serialize_struct_variant(self, _: &'static str, _: u32, _: &'static str, _: usize) -> Result<Self::SerializeStructVariant, WeighError> {
        Err(WeighError)
    }
}

macro_rules! costs {
    ($($f:ident($t:ty) = $n:expr),*) => {
        $(fn $f(self, _: $t) -> Result<(), WeighError> { self.add($n); Ok(()) })*
    };
}

impl ser::Serializer for &mut Weigh<'_> {
    type Ok = ();
    type Error = WeighError;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    // Digits (and sign) at most; a measurement in points is written in twips or EMUs, so
    // floats take a 64-bit integer's room.
    costs!(
        serialize_bool(bool) = 4,
        serialize_i8(i8) = 4,
        serialize_i16(i16) = 6,
        serialize_i32(i32) = 11,
        serialize_i64(i64) = 20,
        serialize_u8(u8) = 3,
        serialize_u16(u16) = 5,
        serialize_u32(u32) = 10,
        serialize_u64(u64) = 20,
        serialize_f32(f32) = 20,
        serialize_f64(f64) = 24,
        serialize_char(char) = 12,
        serialize_unit_struct(&'static str) = 0
    );
    fn serialize_str(self, v: &str) -> Result<(), WeighError> {
        // Escaped, in two attributes at most (a font's `w:ascii` and `w:hAnsi`), each with its
        // name and quotes.
        self.add(escaped(v).saturating_mul(2).saturating_add(STRING));
        Ok(())
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<(), WeighError> {
        // As base64, if ever written.
        self.add((v.len() as u64).saturating_mul(2));
        Ok(())
    }
    fn serialize_none(self) -> Result<(), WeighError> {
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<(), WeighError> {
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), WeighError> {
        Ok(())
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, variant: &'static str) -> Result<(), WeighError> {
        // An enumerated value is written as a word much like its name.
        self.add(4 + 2 * variant.len() as u64);
        Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _: &'static str, v: &T) -> Result<(), WeighError> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(self, _: &'static str, _: u32, variant: &'static str, v: &T) -> Result<(), WeighError> {
        self.add(4 + 2 * variant.len() as u64);
        v.serialize(self)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self, WeighError> {
        Ok(self)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self, WeighError> {
        Ok(self)
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, WeighError> {
        Ok(self)
    }
    fn serialize_tuple_variant(self, _: &'static str, _: u32, variant: &'static str, _: usize) -> Result<Self, WeighError> {
        self.add(FIELD + 2 * variant.len() as u64);
        Ok(self)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self, WeighError> {
        Ok(self)
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, WeighError> {
        Ok(self)
    }
    fn serialize_struct_variant(self, _: &'static str, _: u32, variant: &'static str, _: usize) -> Result<Self, WeighError> {
        self.add(FIELD + 2 * variant.len() as u64);
        Ok(self)
    }
}

impl ser::SerializeSeq for &mut Weigh<'_> {
    type Ok = ();
    type Error = WeighError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), WeighError> {
        self.add(ITEM);
        v.serialize(&mut **self)
    }
    fn end(self) -> Result<(), WeighError> {
        Ok(())
    }
}

impl ser::SerializeTuple for &mut Weigh<'_> {
    type Ok = ();
    type Error = WeighError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), WeighError> {
        v.serialize(&mut **self)
    }
    fn end(self) -> Result<(), WeighError> {
        Ok(())
    }
}

impl ser::SerializeTupleStruct for &mut Weigh<'_> {
    type Ok = ();
    type Error = WeighError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), WeighError> {
        v.serialize(&mut **self)
    }
    fn end(self) -> Result<(), WeighError> {
        Ok(())
    }
}

impl ser::SerializeTupleVariant for &mut Weigh<'_> {
    type Ok = ();
    type Error = WeighError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), WeighError> {
        v.serialize(&mut **self)
    }
    fn end(self) -> Result<(), WeighError> {
        Ok(())
    }
}

impl ser::SerializeMap for &mut Weigh<'_> {
    type Ok = ();
    type Error = WeighError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, k: &T) -> Result<(), WeighError> {
        self.add(ITEM);
        k.serialize(&mut **self)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), WeighError> {
        v.serialize(&mut **self)
    }
    fn end(self) -> Result<(), WeighError> {
        Ok(())
    }
}

impl ser::SerializeStruct for &mut Weigh<'_> {
    type Ok = ();
    type Error = WeighError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, v: &T) -> Result<(), WeighError> {
        self.field(key, v)
    }
    fn end(self) -> Result<(), WeighError> {
        Ok(())
    }
}

impl ser::SerializeStructVariant for &mut Weigh<'_> {
    type Ok = ();
    type Error = WeighError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, v: &T) -> Result<(), WeighError> {
        self.field(key, v)
    }
    fn end(self) -> Result<(), WeighError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::props::CharProps;

    #[test]
    fn counts_are_grouped_in_messages() {
        let o = Over { measure: Measure::Paragraphs, count: 500_001, limit: 500_000 };
        assert_eq!(o.to_string(), "500,001 paragraphs, more than the 500,000 a document may have");
        let o = Over { measure: Measure::SavedBytes, count: 300 << 20, limit: SAVED_BYTES };
        assert_eq!(o.to_string(), "300.0 MB of saved text and formatting, more than the 256.0 MB a document may have");
        let o = Over { measure: Measure::MediaBytes, count: MEDIA_BYTES + 1, limit: MEDIA_BYTES };
        assert_eq!(o.to_string(), "268,435,457 bytes of pictures and embedded files, more than the 268,435,456 bytes a document may have");
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
    }

    #[test]
    fn a_tally_counts_every_story() {
        let mut doc = Document::new();
        doc.body = vec![crate::para_block(Paragraph::with_text("Body", CharProps::default())); 3];
        doc.add_part(PartKind::Footnote, vec![crate::para_block(Paragraph::with_text("Note", CharProps::default()))]);
        let t = Tally::of(&doc);
        assert_eq!((t.paragraphs, t.runs, t.stories, t.sections), (4, 4, 1, 1));
        assert!(t.saved_bytes > 0);
        assert_eq!(t.over(&Limits::DOCUMENT), None);
        let tight = Limits { paragraphs: 3, ..Limits::DOCUMENT };
        assert_eq!(t.over(&tight).map(|o| o.measure), Some(Measure::Paragraphs));
        // Already over and no more than before: an edit may keep it.
        assert_eq!(t.grown_over(&t, &tight), None);
    }

    #[test]
    fn revisions_add_their_authors_where_they_are_named() {
        let revs = vec![Revision::new(crate::RevisionKind::Insert, "A".repeat(100), "2026-10-17T12:00:00Z")];
        let plain = CharProps::default();
        let tracked = CharProps { ins: Some(0), ..Default::default() };
        let named = weigh(&tracked, &revs);
        assert!(named > weigh(&plain, &revs) + 2 * 100, "{named}");
        // An index no revision has costs a generous author.
        assert!(weigh(&CharProps { ins: Some(7), ..Default::default() }, &revs) > REVISION);
    }

    /// A count remembers the formatting it last weighed of each kind (`Memo`) and reads ASCII text
    /// from a table: the same count as weighing everything character by character, on stories
    /// whose rows, cells, paragraphs and runs are formatted alike, apart and alike again, with
    /// tracked changes by different authors, content controls, objects and every kind of text.
    #[test]
    fn remembering_what_was_weighed_counts_the_same() {
        for b in 0u8..128 {
            assert_eq!(u64::from(ASCII_COST[usize::from(b)]), char_cost(char::from(b)), "{b:#x}");
        }
        let mut doc = Document::new();
        doc.revisions = vec![
            Revision::new(crate::RevisionKind::Insert, "A".repeat(40), "2026-10-17T12:00:00Z"),
            Revision::new(crate::RevisionKind::Delete, "Bea", "2026-10-18T09:30:00Z"),
        ]
        .into();
        let texts =
            ["plain words", "a & b < c > d \"e\" 'f'", "tab\there\nline", "caf\u{e9} na\u{ef}ve", "non\u{2011}breaking soft\u{00AD}hyphen", ""];
        let chr = |i: usize| match i % 5 {
            0 => CharProps::default(),
            1 => CharProps { bold: Some(true), ..Default::default() },
            2 => CharProps { ins: Some(0), ..Default::default() },
            3 => CharProps { del: Some(1), font: Some("Serif".into()), ..Default::default() },
            _ => CharProps { ins: Some(7), ..Default::default() },
        };
        let para = |i: usize| {
            let mut p = Paragraph::with_text(texts[i % texts.len()], chr(i));
            p.props = crate::props::ParaProps { style: (i % 3 == 1).then(|| "Heading1".to_string()), ..Default::default() };
            if i % 4 == 2 {
                p.mark = chr(i + 2);
                let at = p.text.len();
                p.insert_text(at, " more", &chr(i + 1)).unwrap();
            }
            if i % 7 == 3 {
                p.insert_object(0, InlineObject::FieldEnd, &chr(i)).unwrap();
            }
            p
        };
        let mut table = crate::Table::new(9, 4, 400.0);
        for (r, row) in table.rows.iter_mut().enumerate() {
            let row = crate::Shared::make_mut(row);
            if r % 3 == 1 {
                row.props.cant_split = true;
            }
            // Formatted as the row before, but with content controls (the memo compares both).
            if r == 6 {
                row.controls.open.push(crate::control::ContentControl { tag: "repeating".into(), ..Default::default() });
            }
            for (c, cell) in row.cells.iter_mut().enumerate() {
                if (r + c) % 4 == 0 {
                    cell.props.width = Some(50.0 + c as f32);
                }
                if r == 7 && c == 3 {
                    cell.controls.open.push(crate::control::ContentControl { title: "A cell's control".into(), ..Default::default() });
                }
                cell.blocks = vec![crate::para_block(para(r * 4 + c))];
            }
        }
        doc.body = (0..30).map(|i| crate::para_block(para(i))).collect();
        doc.body.insert(10, Arc::new(Block::Table(table)));
        doc.add_part(PartKind::Footnote, (0..5).map(|i| crate::para_block(para(i + 3))).collect());
        let all = Tally::of_with(&doc, &mut Memo { off: true, ..Default::default() });
        assert_eq!(Tally::of(&doc), all);
        assert_eq!(Tally::of_blocks(&doc.body, &doc), blocks_tally(&doc.body, &doc, &mut Memo { off: true, ..Default::default() }));
        assert!(all.saved_bytes > 0 && all.paragraphs == 30 + 36 + 5, "{all:?}");
    }

    /// How long counting a document of one 8,000-row, 3-column table takes (what opening one
    /// spends on it), remembering what was weighed and not:
    /// `cargo test --release -p wordcraft-doc --lib tally_bench -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn tally_bench() {
        let mut t = crate::Table::new(8000, 3, 468.0);
        for (r, row) in t.rows.iter_mut().enumerate() {
            for (c, cell) in crate::Shared::make_mut(row).cells.iter_mut().enumerate() {
                cell.props.width = Some(150.0);
                cell.blocks = vec![crate::para_block(Paragraph::with_text(&format!("cell {r} {c} with a few words"), CharProps::default()))];
            }
        }
        let mut doc = Document::new();
        doc.body.push(Arc::new(Block::Table(t)));
        for round in 0..3 {
            for off in [true, false] {
                let at = std::time::Instant::now();
                let n = Tally::of_with(&doc, &mut Memo { off, ..Default::default() });
                eprintln!("TALLY round {round} memo {}: {:.2} ms ({} paragraphs)", !off, at.elapsed().as_secs_f64() * 1e3, n.paragraphs);
            }
        }
    }

    #[test]
    fn the_derived_reader_limits_hold_a_document_at_the_limits() {
        const { assert!(TABLE_NESTING <= TABLE_NESTING_READ) };
        const { assert!(MAX_XML_PART_BYTES >= SAVED_BYTES) };
        const { assert!(MAX_PACKAGE_BYTES >= SAVED_BYTES + MEDIA_BYTES) };
        const { assert!(MAX_PACKAGE_PARTS >= STORIES + PICTURES + KEPT_PARTS) };
        const { assert!(MAX_PART_RELATIONSHIPS >= RUNS + STORIES + PICTURES) };
    }
}
