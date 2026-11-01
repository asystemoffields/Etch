//! Incremental pagination: lay out again only from the page an edit is on.
//!
//! A pass records a [`Checkpoint`] before the first body block placed on each new page: the
//! page builder's state, the list counters and the other walk state. The next pass, given the
//! layout that pass returned, compares the document's body block by block (blocks are shared
//! `Arc`s: an unchanged block is the same allocation) and starts again from the last checkpoint
//! before the first changed block, with that layout's pages up to it. Everything a block's
//! placement reads from before it is in the checkpoint; what it reads from after it (keep with
//! next looking at the next paragraphs) is kept out by resuming before such a chain.
//!
//! Further on, once a checkpoint of the new pass finds the same state the old pass had before
//! the same (unchanged) blocks, the old pass's pages from there on are what the new pass would
//! make: they are taken over, their block numbers moved by the blocks added or removed (when
//! their items are read, see [`crate::Items`]: a paragraph added near the start costs the pages
//! up to where the pagination comes back in step, not the rest).
//!
//! A long table is one block, so tables get more: a checkpoint before and after each, and one
//! before the first row placed on each new page. A table that is the same `Arc` as last time
//! reuses its layout ([`TableMemo`]); a changed one reuses the rows that didn't change when its
//! columns come out the same ([`crate::table::RowReuse`]), in a table numbering its cells those
//! after the same list counters. When the first changed block is such a table, the pass jumps
//! to the old pass's last row checkpoint before its first changed row, and comes back in step
//! at a row checkpoint after its last one. Rows are placed by reference (copied out when a
//! page's items are read), so a table moved down by a line is paginated again from its rows'
//! heights.
//!
//! Footnotes: a note whose text changed (typing in a note), or whose number did (a note added
//! or deleted before it), counts as a change of the body blocks referencing it: the pass starts
//! before the first and comes back in step after the last. Paragraph and table layouts depend
//! on the marks of their own notes only.
//!
//! Anything the checkpoints don't cover falls back to laying the whole document out: a change
//! anywhere outside the body blocks and the footnotes (styles, numbering, settings, headers,
//! footers, endnotes and their numbers, text boxes, sections' setup, media), a changed note
//! referenced from a text box, other views than Print Layout, page totals in the body,
//! footnotes numbered per page, mirrored margins, vertical text, vertical alignment of a
//! section, linked text boxes and Print Markup.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use wordcraft_doc::numbering::Counters;
use wordcraft_doc::para::InlineObject;
use wordcraft_doc::{Block, Document, PartKind};

use crate::fields::NoteNum;
use crate::{LayoutOptions, NoteBox, Page, PageBuilder, Placed, ViewMode, WrapArea, hashser};

/// Page builder state at a checkpoint, without its pages, section and footnote separators
/// (which the pass sets up again for the section).
#[derive(Clone)]
pub(crate) struct PbSaved {
    sect_idx: usize,
    col: usize,
    cols: Vec<(f32, f32)>,
    y: f32,
    top: f32,
    bottom: f32,
    number: u32,
    web: bool,
    notes: Vec<NoteBox>,
    carry: Vec<NoteBox>,
    orig_bottom: f32,
    prev: Option<(String, bool, f32, bool, bool)>,
    after_break_mark: bool,
    next_starts_page: bool,
    restart_at: Option<u32>,
    excl: Vec<WrapArea>,
    float_tables: Vec<wordcraft_geom::Rect>,
    line_no: u32,
    page_items_start: usize,
    block_marks: Vec<usize>,
    page_valign: wordcraft_doc::section::PageVAlign,
    max_y: f32,
    content_bottom: f32,
    balance: Option<(usize, f32)>,
    col_break_page: Option<usize>,
    sect_beneath: bool,
    beneath: bool,
    foot_top: f32,
    header_repeats: usize,
}

impl PbSaved {
    pub(crate) fn take(pb: &PageBuilder) -> Self {
        // Every field, so a new one doesn't build until someone decides whether it is saved.
        let PageBuilder {
            pages: _,
            sect: _,
            sect_idx,
            col,
            cols,
            y,
            top,
            bottom,
            number,
            web,
            notes,
            carry,
            orig_bottom,
            prev,
            after_break_mark,
            next_starts_page,
            restart_at,
            excl,
            float_tables,
            line_no,
            page_items_start,
            block_marks,
            page_valign,
            seps: _,
            behind: _,
            max_y,
            content_bottom,
            balance,
            col_break_page,
            sect_beneath,
            beneath,
            foot_top,
            header_repeats,
        } = pb;
        PbSaved {
            sect_idx: *sect_idx,
            col: *col,
            cols: cols.clone(),
            y: *y,
            top: *top,
            bottom: *bottom,
            number: *number,
            web: *web,
            notes: notes.clone(),
            carry: carry.clone(),
            orig_bottom: *orig_bottom,
            prev: prev.clone(),
            after_break_mark: *after_break_mark,
            next_starts_page: *next_starts_page,
            restart_at: *restart_at,
            excl: excl.clone(),
            float_tables: float_tables.clone(),
            line_no: *line_no,
            page_items_start: *page_items_start,
            block_marks: block_marks.clone(),
            page_valign: *page_valign,
            max_y: *max_y,
            content_bottom: *content_bottom,
            balance: *balance,
            col_break_page: *col_break_page,
            sect_beneath: *sect_beneath,
            beneath: *beneath,
            foot_top: *foot_top,
            header_repeats: *header_repeats,
        }
    }

    pub(crate) fn restore(&self, pb: &mut PageBuilder) {
        let s = self.clone();
        pb.sect_idx = s.sect_idx;
        pb.col = s.col;
        pb.cols = s.cols;
        pb.y = s.y;
        pb.top = s.top;
        pb.bottom = s.bottom;
        pb.number = s.number;
        pb.web = s.web;
        pb.notes = s.notes;
        pb.carry = s.carry;
        pb.orig_bottom = s.orig_bottom;
        pb.prev = s.prev;
        pb.after_break_mark = s.after_break_mark;
        pb.next_starts_page = s.next_starts_page;
        pb.restart_at = s.restart_at;
        pb.excl = s.excl;
        pb.float_tables = s.float_tables;
        pb.line_no = s.line_no;
        pb.page_items_start = s.page_items_start;
        pb.block_marks = s.block_marks;
        pb.page_valign = s.page_valign;
        pb.max_y = s.max_y;
        pb.content_bottom = s.content_bottom;
        pb.balance = s.balance;
        pb.col_break_page = s.col_break_page;
        pb.sect_beneath = s.sect_beneath;
        pb.beneath = s.beneath;
        pb.foot_top = s.foot_top;
        pb.header_repeats = s.header_repeats;
    }

    /// The same state, for taking the old pass's pages over: only plain states compare (no
    /// notes to continue on the next page, nothing floating on the page), field by field. The
    /// footnotes waiting for the page's note area must be the same notes laid out the same
    /// ([`NoteBox::same_as`]): in a footnote-heavy document nearly every page starts with some
    /// (the notes of a paragraph continued from the page before, or continued notes), and
    /// requiring none made typing lay out the rest of the document every time.
    fn same_plain(&self, o: &PbSaved) -> bool {
        let plain = |s: &PbSaved| s.carry.is_empty() && s.excl.is_empty() && s.float_tables.is_empty();
        let same_notes = self.notes.len() == o.notes.len() && self.notes.iter().zip(&o.notes).all(|(a, b)| a.same_as(b));
        let bits = |v: &[(f32, f32)]| v.iter().map(|(a, b)| (a.to_bits(), b.to_bits())).collect::<Vec<_>>();
        let prev = |p: &Option<(String, bool, f32, bool, bool)>| p.as_ref().map(|(s, a, f, b, c)| (s.clone(), *a, f.to_bits(), *b, *c));
        plain(self)
            && plain(o)
            && same_notes
            && self.sect_idx == o.sect_idx
            && self.col == o.col
            && bits(&self.cols) == bits(&o.cols)
            && self.y.to_bits() == o.y.to_bits()
            && self.top.to_bits() == o.top.to_bits()
            && self.bottom.to_bits() == o.bottom.to_bits()
            && self.number == o.number
            && self.web == o.web
            && self.orig_bottom.to_bits() == o.orig_bottom.to_bits()
            && prev(&self.prev) == prev(&o.prev)
            && self.after_break_mark == o.after_break_mark
            && self.next_starts_page == o.next_starts_page
            && self.restart_at == o.restart_at
            && self.line_no == o.line_no
            && self.page_items_start == o.page_items_start
            && self.block_marks == o.block_marks
            && self.page_valign == o.page_valign
            && self.max_y.to_bits() == o.max_y.to_bits()
            && self.content_bottom.to_bits() == o.content_bottom.to_bits()
            && self.balance.is_none()
            && o.balance.is_none()
            && self.col_break_page == o.col_break_page
            && self.sect_beneath == o.sect_beneath
            && self.beneath == o.beneath
            && self.foot_top.to_bits() == o.foot_top.to_bits()
            && same_header_repeats(self.header_repeats, o.header_repeats)
    }
}

/// Whether a table placed from a checkpoint with `a` header rows repeated so far repeats the
/// same header rows as the old pass did from one with `b`. Only the cap
/// ([`crate::MAX_HEADER_REPEATS`]) makes the count matter: a pass that went past half of it
/// isn't resumed from, so the old pass repeats at most half the cap from `b` on, and from `a`,
/// at most half of it too, the new one stays within the cap as well.
fn same_header_repeats(a: usize, b: usize) -> bool {
    let half = crate::MAX_HEADER_REPEATS / 2;
    a == b || (a <= half && b <= half)
}

/// The walk state before a body block, to lay out from there again.
#[derive(Clone)]
pub(crate) struct Checkpoint {
    /// The body block about to be placed.
    pub block: usize,
    /// The row of it (a table) about to be placed; 0 before the block.
    pub row: usize,
    /// Its section (index into `Document::sections`).
    pub si: usize,
    /// Pages so far, items on the last of them, and how many of those were put in front of the
    /// page's items (behind the text).
    pub pages: usize,
    pub items: usize,
    pub behind: usize,
    pub pb: PbSaved,
    pub counters: Counters,
    pub eq_count: u32,
    pub page_field: u32,
    /// The page field as its section started.
    pub sect_page: u32,
    /// Text boxes laid out so far (net of balancing attempts thrown away).
    pub boxes: usize,
    /// Endnotes not placed yet.
    pub endnotes_left: usize,
}

impl Checkpoint {
    /// The same walk state as `o`, for taking the old pass's pages over from here.
    pub(crate) fn same_state(&self, o: &Checkpoint) -> bool {
        self.si == o.si
            && self.row == o.row
            && self.pages == o.pages
            && self.items == o.items
            && self.behind == o.behind
            && self.eq_count == o.eq_count
            && self.page_field == o.page_field
            && self.sect_page == o.sect_page
            && self.endnotes_left == o.endnotes_left
            && self.counters == o.counters
            && self.pb.same_plain(&o.pb)
    }
}

/// What a pass leaves for the next one to resume from.
pub(crate) struct Resumable {
    /// The layout this pass returned ([`crate::DocLayout::generation`]).
    pub generation: u64,
    /// Everything besides the body blocks the pass depended on ([`frame_hash`]).
    pub frame: u64,
    /// Per section, its setup and the next one's ([`section_hashes`]).
    pub sects: Vec<u64>,
    /// The body blocks it laid out.
    pub blocks: Vec<Arc<Block>>,
    pub cps: Vec<Checkpoint>,
    /// Per page, the items put in front of its list (behind the text) by the end of the pass.
    pub behind: Vec<usize>,
    /// Text boxes laid out in the whole pass (net), for a pass that takes its pages over.
    pub boxes: usize,
    /// The notes' numbers it counted.
    pub notes: Arc<HashMap<u32, NoteNum>>,
    /// The footnotes it laid out, by id ([`footnotes`]; not in `frame`).
    pub foot: Vec<Foot>,
}

/// Where a pass starts again: the checkpoint, and the pages of the old layout to start with.
pub(crate) struct Start {
    pub cp: Checkpoint,
    pub pages: Vec<Page>,
    pub behind: Vec<usize>,
    /// Checkpoints before it (still valid).
    pub cps: Vec<Checkpoint>,
    /// For taking the old pages over later on: the old pass's checkpoints after it, its blocks,
    /// its boxes and the number of blocks the body grew by (negative: shrank). Blocks at and
    /// after `same_from` (new numbering) are the old ones.
    pub tail: Tail,
}

pub(crate) struct Tail {
    /// The first body block that changed.
    pub first: usize,
    /// Whether a footnote's text changed (the pass doesn't start inside a table then).
    pub notes: bool,
    pub cps: Vec<Checkpoint>,
    pub behind: Vec<usize>,
    pub boxes: usize,
    pub delta: isize,
    pub same_from: usize,
}

static GEN: AtomicU64 = AtomicU64::new(1);

/// A new layout generation (never 0, which marks a layout no pass can resume from).
pub(crate) fn next_gen() -> u64 {
    GEN.fetch_add(1, Ordering::Relaxed)
}

/// Can a pass with these options and this document resume at all?
pub(crate) fn eligible(doc: &Document, opts: &LayoutOptions, per_page_notes: bool) -> bool {
    opts.view == ViewMode::Print
        && !opts.comment_balloons
        && !per_page_notes
        && !doc.settings.mirror_margins
        && !doc.parts.values().any(|p| p.body.next.is_some())
        && doc.sections().iter().all(|(_, s)| !s.text_direction.is_turned() && s.valign == wordcraft_doc::section::PageVAlign::Top)
}

/// A hash of everything outside the body blocks that the layout reads: `env` (the document's
/// styles and numbering and the options that change paragraphs, see `env_hash`), its settings,
/// parts (headers, footers, notes, text boxes), the last section, the note separators, the
/// title and author, media, the options and the page totals assumed.
pub(crate) fn frame_hash(doc: &Document, opts: &LayoutOptions, env: u64, totals: &crate::Totals) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    env.hash(&mut h);
    hashser::feed(&mut h, &(&doc.last_section, &doc.settings, &doc.core.title, &doc.core.creator, &doc.footnote_separators, &doc.endnote_separators));
    // The parts but the footnotes (compared one by one, see `footnotes`).
    for (id, p) in doc.parts.iter().filter(|(_, p)| p.kind != PartKind::Footnote) {
        id.hash(&mut h);
        hashser::feed(&mut h, p);
    }
    for (k, v) in &doc.media {
        k.hash(&mut h);
        (Arc::as_ptr(v) as usize).hash(&mut h);
        v.len().hash(&mut h);
    }
    // The word being typed changes with every keystroke and only marks its own paragraph (whose
    // layouts are keyed by it): `layout_pass` handles it (`typing_moved_before`), not the frame.
    format!("{:?}", LayoutOptions { typing_word: None, ..opts.clone() }).hash(&mut h);
    totals.pages.hash(&mut h);
    totals.sections.hash(&mut h);
    h.finish()
}

/// Where the word being typed was in the last pass (`old`), when it has moved since (`new`) and
/// that paragraph is still as the last pass laid it out: its squiggles change although nothing in
/// it did. `Some(Ok(block))` for a body block, `Some(Err(id))` for a part (a note, a header…).
/// (Typing on in the same word changes the paragraph, so its old revision is gone; a new typed
/// word's paragraph is the one just edited, which is laid out again anyway.)
fn typing_left(doc: &Document, old: Option<crate::TypingWord>, new: Option<crate::TypingWord>) -> Option<Result<usize, u32>> {
    let old = old.filter(|o| Some(*o) != new)?;
    if let Some(i) = doc.body.iter().position(|b| holds_rev(b, old.rev, 0)) {
        return Some(Ok(i));
    }
    doc.parts.iter().find(|(_, p)| p.blocks.iter().any(|b| holds_rev(b, old.rev, 0))).map(|(id, _)| Err(*id))
}

fn holds_rev(b: &Block, rev: u64, depth: usize) -> bool {
    match b {
        Block::Para(p) => p.rev == rev,
        Block::Table(t) if depth < 64 => t.rows.iter().any(|r| r.cells.iter().any(|c| c.blocks.iter().any(|b| holds_rev(b, rev, depth + 1)))),
        _ => false,
    }
}

/// A footnote: its id, text and text setup.
pub(crate) type Foot = (u32, wordcraft_doc::Blocks, wordcraft_doc::TextBody);

/// The footnotes, by id (their blocks are shared: a note typed in has new ones).
pub(crate) fn footnotes(doc: &Document) -> Vec<Foot> {
    doc.parts.iter().filter(|(_, p)| p.kind == PartKind::Footnote).map(|(id, p)| (*id, p.blocks.clone(), p.body)).collect()
}

/// The footnotes referenced in `new_nums` whose text changed (or that are new) between `old`
/// and `new`, or whose number changed (`old_nums` to `new_nums`), in id order; `None` when an
/// endnote's number changed (endnotes go at the end: their pages would have to be compared).
fn changed_notes_or_numbers(old: &[Foot], new: &[Foot], old_nums: &HashMap<u32, NoteNum>, new_nums: &HashMap<u32, NoteNum>) -> Option<Vec<u32>> {
    let same_text = |id: u32| {
        let find = |v: &[Foot]| v.binary_search_by_key(&id, |f| f.0).ok().and_then(|i| v.get(i).cloned());
        match (find(old), find(new)) {
            (Some((_, x, xb)), Some((_, y, yb))) => xb == yb && x.len() == y.len() && x.iter().zip(&y).all(|(p, q)| Arc::ptr_eq(p, q)),
            (None, None) => true,
            _ => false,
        }
    };
    let foot = |id: u32| new.binary_search_by_key(&id, |f| f.0).is_ok() || old.binary_search_by_key(&id, |f| f.0).is_ok();
    let mut ids = Vec::new();
    for (id, n) in new_nums {
        if old_nums.get(id) != Some(n) {
            if !foot(*id) {
                return None;
            }
            ids.push(*id);
        } else if !same_text(*id) {
            ids.push(*id);
        }
    }
    // A removed note: its reference went with a changed block. An endnote's numbering going
    // with it changes the numbers after it (seen above).
    if old_nums.keys().any(|id| !new_nums.contains_key(id) && !foot(*id)) {
        return None;
    }
    ids.sort_unstable();
    Some(ids)
}

/// Whether `b` references one of the footnotes `ids` (sorted) in its text or its cells, and
/// whether it has text boxes.
fn refs(b: &Block, ids: &[u32]) -> (bool, bool) {
    match b {
        Block::Para(p) => (
            p.objects.iter().any(|o| matches!(o, InlineObject::NoteRef { id, .. } if ids.binary_search(id).is_ok())),
            p.objects.iter().any(|o| !o.text_boxes().is_empty()),
        ),
        Block::Table(t) => t.rows.iter().flat_map(|r| &r.cells).flat_map(|c| &c.blocks).fold((false, false), |(r, x), b| {
            let (r2, x2) = refs(b, ids);
            (r || r2, x || x2)
        }),
    }
}

/// The first and last body blocks referencing the footnotes `ids` (sorted), when that is all
/// that does:
/// every one is referenced in the body text or its tables, none from a text box, and neither
/// their text nor (old or new) has text boxes.
fn note_refs(doc: &Document, ids: &[u32], texts: &[&[Foot]]) -> Option<(usize, usize)> {
    let boxes = |blocks: &wordcraft_doc::Blocks| blocks.iter().any(|b| refs(b, &[]).1);
    if texts.iter().flat_map(|t| t.iter()).filter(|(id, ..)| ids.binary_search(id).is_ok()).any(|(_, b, _)| boxes(b)) {
        return None;
    }
    let mut found: Vec<u32> = Vec::new();
    let mut span: Option<(usize, usize)> = None;
    for (i, b) in doc.body.iter().enumerate() {
        let (hit, has_boxes) = refs(b, ids);
        if has_boxes {
            // A text box's story may reference one (and is placed with the block).
            if doc.parts.values().any(|p| p.kind == PartKind::TextBox && p.blocks.iter().any(|b| refs(b, ids).0)) {
                return None;
            }
        }
        if hit {
            span = Some(span.map_or((i, i), |(a, _)| (a, i)));
            collect_refs(b, Some(ids), &mut found);
        }
    }
    found.sort_unstable();
    found.dedup();
    ids.iter().all(|id| found.binary_search(id).is_ok()).then_some(span).flatten()
}

/// The notes `b` references (those of `ids`, sorted, only), in order, some maybe more than
/// once.
fn collect_refs(b: &Block, ids: Option<&[u32]>, out: &mut Vec<u32>) {
    match b {
        Block::Para(p) => {
            for o in &p.objects {
                if let InlineObject::NoteRef { id, .. } = o
                    && ids.is_none_or(|ids| ids.binary_search(id).is_ok())
                {
                    out.push(*id);
                }
            }
        }
        Block::Table(t) => {
            for b in t.rows.iter().flat_map(|r| &r.cells).flat_map(|c| &c.blocks) {
                collect_refs(b, ids, out);
            }
        }
    }
}

/// Per section: a hash of its setup and the next section's (which decides whether its columns
/// are balanced and whether its last empty paragraph takes room).
pub(crate) fn section_hashes(doc: &Document) -> Vec<u64> {
    let sections = doc.sections();
    let one: Vec<u64> = sections.iter().map(|(_, s)| hashser::hash_ser(s)).collect();
    (0..one.len()).map(|i| crate::hash_of(&(one.get(i), one.get(i + 1)))).collect()
}

/// Whether `b` has note references, or text boxes (whose stories may have them).
fn has_notes(b: &Block) -> bool {
    match b {
        Block::Para(p) => p.objects.iter().any(|o| matches!(o, InlineObject::NoteRef { .. }) || !o.text_boxes().is_empty()),
        Block::Table(t) => t.rows.iter().flat_map(|r| &r.cells).flat_map(|c| &c.blocks).any(|b| has_notes(b)),
    }
}

/// Whether old block `a` or its new version `b` may have note references the other hasn't: for
/// two tables of the same shape, any of the cells that differ has some (or a text box); else
/// either has any.
fn changed_notes(a: &Block, b: &Block) -> bool {
    let (Block::Table(x), Block::Table(y)) = (a, b) else { return has_notes(a) || has_notes(b) };
    // (Rows the two share are the same rows: only the others are looked at.)
    let changed_rows = || x.rows.iter().zip(&y.rows).filter(|(r, s)| !wordcraft_doc::Shared::ptr_eq(r, s));
    let same_shape = x.rows.len() == y.rows.len() && changed_rows().all(|(r, s)| r.cells.len() == s.cells.len());
    if !same_shape {
        return has_notes(a) || has_notes(b);
    }
    changed_rows().flat_map(|(r, s)| r.cells.iter().zip(&s.cells)).any(|(c, d)| {
        let same = c.blocks.len() == d.blocks.len() && c.blocks.iter().zip(&d.blocks).all(|(p, q)| Arc::ptr_eq(p, q));
        !same && c.blocks.iter().chain(&d.blocks).any(|b| has_notes(b))
    })
}

/// The body blocks that differ between `old` and `new`: the first, and where the blocks that
/// are the same to the end start in each.
fn changed(old: &[Arc<Block>], new: &[Arc<Block>]) -> (usize, usize, usize) {
    let first = old.iter().zip(new).take_while(|(a, b)| Arc::ptr_eq(a, b)).count();
    let most = old.len().min(new.len()) - first;
    let suffix = old.iter().rev().zip(new.iter().rev()).take(most).take_while(|(a, b)| Arc::ptr_eq(a, b)).count();
    (first, old.len() - suffix, new.len() - suffix)
}

impl Resumable {
    /// The notes' numbers this pass counted, if they hold for `doc`: nothing else changed but
    /// body blocks, none of the old or new ones has note references (or text boxes), and the
    /// sections are the same.
    pub(crate) fn notes_if_unchanged(&self, doc: &Document, frame: u64, sects: &[u64]) -> Option<Arc<HashMap<u32, NoteNum>>> {
        if self.frame != frame || self.sects != sects {
            return None;
        }
        let (first, old_end, new_end) = changed(&self.blocks, &doc.body);
        let old = self.blocks.get(first..old_end)?;
        let new = doc.body.get(first..new_end)?;
        // A table changed in place: only its changed cells (typing in a long table).
        let in_place = old.len() == new.len() && old.iter().zip(new).all(|(a, b)| !changed_notes(a, b));
        if !in_place && old.iter().chain(new).any(|b| has_notes(b)) {
            return None;
        }
        Some(self.notes.clone())
    }

    /// Where the pass for `doc` can start from, given the layout `prev` this one belongs to.
    /// `typing`: the word being typed in the last pass and in this one
    /// ([`crate::LayoutOptions::typing_word`]).
    pub(crate) fn start(
        self,
        doc: &Document,
        prev: &crate::DocLayout,
        frame: u64,
        sects: &[u64],
        notes: &HashMap<u32, NoteNum>,
        typing: (Option<crate::TypingWord>, Option<crate::TypingWord>),
    ) -> Option<Start> {
        if prev.generation != self.generation || self.frame != frame {
            return None;
        }
        let (old, new) = (&self.blocks, &doc.body);
        // The first block that differs, and where the blocks that are the same to the end start.
        let (mut first, _, mut same_from) = changed(old, new);
        // A footnote's text changed: as if the blocks referencing it had (and after a table
        // referencing one, the block after it too: the table's rows aren't compared).
        let foot = footnotes(doc);
        let mut notes = changed_notes_or_numbers(&self.foot, &foot, &self.notes, notes)?;
        // The paragraph the typed word left counts as changed (its squiggles are): a body block,
        // as a changed block; a footnote's, as its note; any other part's, no resuming.
        match typing_left(doc, typing.0, typing.1) {
            Some(Ok(i)) => {
                let after = i + 1 + usize::from(new.get(i).is_some_and(|x| matches!(**x, Block::Table(_))));
                same_from = if first == new.len() && old.len() == new.len() { after } else { same_from.max(after) };
                first = first.min(i);
            }
            Some(Err(id)) if foot.iter().any(|f| f.0 == id) => {
                if !notes.contains(&id) {
                    notes.push(id);
                }
            }
            Some(Err(_)) => return None,
            None => {}
        }
        if !notes.is_empty() {
            let (a, b) = note_refs(doc, &notes, &[&self.foot, &foot])?;
            let after = b + 1 + usize::from(new.get(b).is_some_and(|x| matches!(**x, Block::Table(_))));
            // (With the body the same, every block is an old one from `after` on.)
            same_from = if first == new.len() && old.len() == new.len() { after } else { same_from.max(after) };
            first = first.min(a);
        }
        // Paragraphs kept with the next one look at it while placed: start before the chain of
        // them that leads up to the change. So does a paragraph whose space after the next one
        // may drop (contextual spacing, automatic spacing between list items): on a page with
        // footnotes its last line keeps that space clear of them (`space_after_kept`).
        let mut start = first;
        if let Some(k) = start.checked_sub(1)
            && new.get(k).and_then(|b| b.as_para()).is_some_and(|p| {
                let rp = doc.styles.resolve_para(&p.props);
                rp.space_after > 0.0 && (rp.contextual_spacing || rp.after_auto)
            })
        {
            start = k;
        }
        while start > 0
            && new
                .get(start - 1)
                .and_then(|b| b.as_para())
                .is_some_and(|p| p.props.keep_next.unwrap_or(false) || doc.styles.resolve_para(&p.props).keep_next)
        {
            start -= 1;
        }
        // (Checkpoints inside a table's rows hold the state after the table was laid out: the
        // pass resumes before a block, and inside the first changed table only once it has laid
        // that out, see `Tail::row_start`.)
        let k = self.cps.iter().rposition(|cp| {
            cp.row == 0
                && cp.block <= start
                && cp.pages <= prev.pages.len()
                && sects.get(..=cp.si).is_some_and(|s| self.sects.get(..=cp.si) == Some(s))
        })?;
        let mut cps = self.cps;
        let tail_cps = cps.split_off(k + 1);
        let cp = cps.pop()?;
        let (pages, behind) = pages_upto(prev, &self.behind, &cp)?;
        let delta = new.len() as isize - old.len() as isize;
        let notes = !notes.is_empty();
        Some(Start { cp, pages, behind, cps, tail: Tail { first, notes, cps: tail_cps, behind: self.behind, boxes: self.boxes, delta, same_from } })
    }
}

/// The revisions of every paragraph in `b` (tables to `MAX_DEPTH` deep).
fn revisions(b: &Block, out: &mut crate::RevSet, depth: usize) {
    match b {
        Block::Para(p) => {
            out.insert(p.rev);
        }
        Block::Table(t) if depth < 64 => {
            for b in t.rows.iter().flat_map(|r| &r.cells).flat_map(|c| &c.blocks) {
                revisions(b, out, depth + 1);
            }
        }
        Block::Table(_) => {}
    }
}

/// The revision of a body block replaced whole, if it is a paragraph. A table replaced whole
/// (pasted, its paste undone, a row added or removed) isn't walked: its rows are mostly still in
/// the document (shared with the table it was copied from, or with its version before the edit),
/// walking every cell of long tables cost undoing or redoing a paste of them a fifth of its time,
/// and the layouts of rows that did go are trimmed after a whole pass like any unused ones.
fn whole(b: &Block, out: &mut crate::RevSet) {
    if let Block::Para(p) = b {
        out.insert(p.rev);
    }
}

impl Resumable {
    /// The revisions the paragraphs of the body blocks and footnotes that changed since this pass
    /// had then and don't have now (an edit gave them new ones: the layouts cached for them are
    /// found again only if an undo brings them back), and those they have now. Rows of a table
    /// changed in place that it still shares with this pass's version are skipped, and so are
    /// tables replaced whole ([`whole`]).
    pub(crate) fn replaced_revisions(&self, doc: &Document) -> (crate::RevSet, crate::RevSet) {
        let (mut gone, mut kept) = (crate::RevSet::default(), crate::RevSet::default());
        let (first, old_end, new_end) = changed(&self.blocks, &doc.body);
        let old = self.blocks.get(first..old_end).unwrap_or(&[]);
        let new = doc.body.get(first..new_end).unwrap_or(&[]);
        if old.len() == new.len() {
            for (a, b) in old.iter().zip(new) {
                match (&**a, &**b) {
                    (Block::Table(x), Block::Table(y)) if x.rows.len() == y.rows.len() => {
                        for (r, s) in x.rows.iter().zip(&y.rows) {
                            if wordcraft_doc::Shared::ptr_eq(r, s) {
                                continue;
                            }
                            for b in r.cells.iter().flat_map(|c| &c.blocks) {
                                revisions(b, &mut gone, 1);
                            }
                            for b in s.cells.iter().flat_map(|c| &c.blocks) {
                                revisions(b, &mut kept, 1);
                            }
                        }
                    }
                    _ => {
                        whole(a, &mut gone);
                        whole(b, &mut kept);
                    }
                }
            }
        } else {
            for a in old {
                whole(a, &mut gone);
            }
            for b in new {
                whole(b, &mut kept);
            }
        }
        // Footnotes typed in, or gone.
        let foot = footnotes(doc);
        for (id, blocks, _) in &self.foot {
            let now = foot.binary_search_by_key(id, |f| f.0).ok().and_then(|i| foot.get(i));
            if now.is_some_and(|(_, b, _)| b.len() == blocks.len() && b.iter().zip(blocks).all(|(x, y)| Arc::ptr_eq(x, y))) {
                continue;
            }
            for b in blocks {
                revisions(b, &mut gone, 0);
            }
            for b in now.map(|(_, b, _)| b.as_slice()).unwrap_or(&[]) {
                revisions(b, &mut kept, 0);
            }
        }
        gone.retain(|r| !kept.contains(r));
        (gone, kept)
    }
}

/// Pages taken over from the old layout: their items shared with it (with their body block
/// numbers moved by `delta` when read, see [`crate::Items`]), without headers and footers (the
/// pass adds them again).
pub(crate) fn take_over(pages: &[Page], delta: isize) -> Vec<Page> {
    pages
        .iter()
        .map(|p| {
            let Page { w, h, section, number, items, header: _, footer: _, body, header_story: _, footer_story: _, first_block, markup, hf_slot: _ } =
                p;
            let items = items.shifted(delta);
            Page {
                w: *w,
                h: *h,
                section: *section,
                number: *number,
                items,
                header: Vec::new(),
                footer: Vec::new(),
                body: *body,
                header_story: None,
                footer_story: None,
                first_block: first_block.saturating_add_signed(delta),
                markup: *markup,
                hf_slot: crate::HfSlot::default(),
            }
        })
        .collect()
}

/// Move the body block numbers of `items` by `delta`.
pub(crate) fn shift_items(items: &mut crate::Items, delta: isize) {
    if delta == 0 {
        return;
    }
    for it in items {
        shift_item(it, delta);
    }
}

fn shift_path(story: wordcraft_doc::StoryRef, path: &mut wordcraft_doc::Path, delta: isize) {
    if story != wordcraft_doc::StoryRef::Body {
        return;
    }
    if let Some(b) = path.0.first_mut() {
        *b = u32::try_from(i64::from(*b).saturating_add(delta as i64).max(0)).unwrap_or(u32::MAX);
    }
}

pub(crate) fn shift_item(it: &mut Placed, delta: isize) {
    match it {
        Placed::Lines { story, path, .. }
        | Placed::Image { story, path, .. }
        | Placed::Graphic { story, path, .. }
        | Placed::Object { story, path, .. } => shift_path(*story, path, delta),
        Placed::Cell { story, table, .. } => shift_path(*story, table, delta),
        Placed::Clip { items, .. } => {
            for it in items {
                shift_item(it, delta);
            }
        }
        Placed::Shape { object: Some(pos), .. } => shift_path(pos.story, &mut pos.path, delta),
        Placed::Fill { .. } | Placed::Rule { .. } | Placed::Shape { object: None, .. } => {}
    }
}

/// A body table's layout, for a later pass that meets the same table (the same `Arc`) in the
/// same circumstances: after the same list counters and equation count, with the same width,
/// document grid and note numbers, and on a page with the same numbers if its cells show them.
/// Kept only for tables whose layout lays out no text boxes and shows no page totals.
#[derive(Clone)]
pub(crate) struct TableMemo {
    block: Arc<Block>,
    /// The body block it was at (its items' paths start with it).
    block_idx: usize,
    sig: u64,
    /// The notes referenced in its cells, and a hash of the marks they showed.
    note_ids: Arc<[u32]>,
    /// Known to have no merged or turned cells (its rows were taken from an earlier version's
    /// layout, which checks): the next version is checked only where its rows differ.
    plain: bool,
    marks: u64,
    counters: Counters,
    eq_count: u32,
    /// The page numbers it was laid out with, if its cells show them.
    page: Option<(u32, u32, u32, u32, u64, wordcraft_doc::section::NumFormat)>,
    counters_after: Counters,
    eq_after: u32,
    /// For a table that numbers its cells: the list counters before each row and after the last
    /// ([`crate::table::RowReuse::before`]).
    rows_counters: Option<Arc<Vec<Arc<Counters>>>>,
    layout: Arc<crate::table::TableLayout>,
    /// The generation of the pass that placed it last, and the body block it is at on that
    /// pass's pages (`block_idx` until blocks before it come or go).
    stamp: u64,
    placed: usize,
}

/// A table layout the last pass remembered that this one didn't place: if the last pass placed
/// it on pages this one took over (before block `from`, where it resumed, or from the old pass's
/// block where it came back in step, moved by the blocks added there), it is placed by this one
/// too (`generation`).
pub(crate) fn carried(m: &mut TableMemo, prev_gen: u64, generation: u64, from: usize, splice: Option<(usize, isize)>) {
    if m.stamp != prev_gen {
        return;
    }
    if m.placed < from {
        m.stamp = generation;
    } else if let Some((old_block, delta)) = splice
        && m.placed >= old_block
        && let Some(at) = m.placed.checked_add_signed(delta)
    {
        m.placed = at;
        m.stamp = generation;
    }
}

/// A hash of the marks the notes `ids` show now.
fn marks(ctx: &crate::Ctx, ids: &[u32]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for id in ids {
        (id, ctx.fields.notes.get(id).map(|n| (n.n, n.fmt))).hash(&mut h);
    }
    h.finish()
}

/// Is `b` the block holding table `t`?
fn holds(b: &Block, t: &wordcraft_doc::Table) -> bool {
    matches!(b, Block::Table(x) if std::ptr::eq(x, t))
}

/// Table `t`, body block `block`, laid out `width` wide: the last pass's layout when it holds
/// ([`TableMemo`]), else laid out (and remembered for the next pass), with the rows that didn't
/// change taken from the last pass's layout of the table at that block. Also returns the layout
/// rows were taken from (the same one when the table didn't change) and the body block the last
/// pass placed it at, when the last pass's pages show it.
pub(crate) fn body_table(
    ctx: &mut crate::Ctx,
    t: &wordcraft_doc::Table,
    block: usize,
    width: f32,
) -> (Arc<crate::table::TableLayout>, Option<(Arc<crate::table::TableLayout>, usize)>) {
    let arc = ctx.doc.body.get(block).filter(|b| holds(b, t)).cloned();
    let Some(frame) = ctx.memo else {
        return (Arc::new(crate::table::layout_table(ctx, wordcraft_doc::StoryRef::Body, t, &[block as u32], width, 1)), None);
    };
    let path = [block as u32];
    let sig = crate::hash_of(&(frame, ctx.grid.key(), ctx.hide_deleted, ctx.boxes.inside(), width.to_bits()));
    let key = arc.as_ref().map_or(0, |a| Arc::as_ptr(a) as usize);
    if ctx.memo_lookup
        && let Some(arc) = &arc
        && ctx.typing_left.is_none_or(|r| !holds_rev(arc, r, 0))
        && let Some(m) = ctx.cache.tables.get(&key)
        && Arc::ptr_eq(&m.block, arc)
        && m.sig == sig
        && m.marks == marks(ctx, &m.note_ids)
        && m.counters == ctx.counters
        && m.eq_count == ctx.eq_count
        && m.page.is_none_or(|k| k == ctx.fields.page_key())
    {
        // (Its rows are numbered as where they were laid out: placing them moves them here.)
        let layout = m.layout.clone();
        let placed_at = (m.stamp == ctx.prev_gen).then_some(m.placed);
        let memo = TableMemo { block_idx: block, layout: layout.clone(), stamp: ctx.memo_gen, placed: block, ..m.clone() };
        ctx.counters = memo.counters_after.clone();
        ctx.eq_count = memo.eq_after;
        ctx.cache.tables_next.insert(key, memo);
        ctx.cache.tables_reused += 1;
        return (layout.clone(), placed_at.map(|at| (layout, at)));
    }
    let (counters, eq_count, boxes, page_keyed, uses_totals) =
        (ctx.counters.clone(), ctx.eq_count, ctx.boxes.total(), ctx.page_keyed, ctx.uses_totals);
    // The table changed: its rows that didn't may come from the last pass's layout of it.
    let reuse = if ctx.memo_lookup {
        let (reuse, scanned) = row_reuse(ctx, t, block, sig);
        ctx.cache.rows_scanned += scanned;
        reuse
    } else {
        None
    };
    let mut rows_counters = Vec::new();
    let mut layout =
        crate::table::layout_table_reusing(ctx, wordcraft_doc::StoryRef::Body, t, &path, width, 1, reuse.as_ref(), Some(&mut rows_counters));
    let mut old = reuse.as_ref().and_then(|r| r.placed_at.map(|at| (r.old.clone(), at)));
    if let Some(r) = &reuse {
        ctx.cache.rows_reused += r.keep.iter().filter(|k| **k).count();
        // The rows laid out again were checked to count nothing (but list items, in a table
        // whose rows' counters are known); if they did after all, lay the whole table out as a
        // pass from scratch would.
        let counted = ctx.counters != counters && r.before.is_none();
        if counted || ctx.eq_count != eq_count || ctx.boxes.total() != boxes || ctx.page_keyed != page_keyed {
            (ctx.counters, ctx.eq_count) = (counters.clone(), eq_count);
            rows_counters.clear();
            layout = crate::table::layout_table_reusing(ctx, wordcraft_doc::StoryRef::Body, t, &path, width, 1, None, Some(&mut rows_counters));
            old = None;
        }
    }
    let layout = Arc::new(layout);
    if let Some(block_arc) = arc
        && ctx.boxes.total() == boxes
        && ctx.uses_totals == uses_totals
    {
        let page = (ctx.page_keyed != page_keyed).then(|| ctx.fields.page_key());
        // (Only kept for a table that counts list items.)
        let rows_counters = (ctx.counters != counters).then(|| Arc::new(rows_counters));
        let mut ids = Vec::new();
        match (&reuse, &*block_arc) {
            // The earlier version referenced no notes: only the rows laid out again can.
            (Some(r), Block::Table(tt)) if r.no_old_notes => {
                for row in tt.rows.iter().zip(&r.keep).filter(|(_, k)| !**k).map(|(row, _)| row) {
                    ctx.cache.rows_scanned += 1;
                    for b in row.cells.iter().flat_map(|c| &c.blocks) {
                        collect_refs(b, None, &mut ids);
                    }
                }
            }
            _ => {
                ctx.cache.rows_scanned += t.rows.len();
                collect_refs(&block_arc, None, &mut ids)
            }
        }
        ids.sort_unstable();
        ids.dedup();
        let memo = TableMemo {
            marks: marks(ctx, &ids),
            note_ids: ids.into(),
            plain: reuse.is_some(),
            block: block_arc,
            block_idx: block,
            sig,
            counters,
            eq_count,
            page,
            counters_after: ctx.counters.clone(),
            eq_after: ctx.eq_count,
            rows_counters,
            layout: layout.clone(),
            stamp: ctx.memo_gen,
            placed: block,
        };
        ctx.cache.tables_next.insert(key, memo);
    }
    (layout, old)
}

/// The rows of the last pass's layout of the table at body block `block` (an earlier version of
/// `t`) that `t` has unchanged, when they can be taken as they are: the table was laid out in
/// the same circumstances (`sig`, list counters, equation count), counted nothing (list items,
/// equations, page numbers) and has the same setup and number of rows; nothing in `t` merges
/// cells vertically or turns text; and the rows that changed have nothing that counts either.
/// Also returns how many of the table's rows it looked through for merged or turned cells.
fn row_reuse(ctx: &crate::Ctx, t: &wordcraft_doc::Table, block: usize, sig: u64) -> (Option<crate::table::RowReuse>, usize) {
    // (The most recent if several were at this block.)
    let Some(m) = ctx.cache.tables.values().filter(|m| m.block_idx == block).max_by_key(|m| m.stamp) else { return (None, 0) };
    let Block::Table(old) = &*m.block else { return (None, 0) };
    // A table numbering its cells needs its rows' counters (to keep a row only after the same).
    let before = m.rows_counters.clone().filter(|b| b.len() == m.layout.rows.len() + 1);
    let same_context = m.sig == sig
        && m.marks == marks(ctx, &m.note_ids)
        && m.counters == ctx.counters
        && m.eq_count == ctx.eq_count
        && m.page.is_none()
        && (m.counters_after == m.counters || before.is_some())
        && m.eq_after == m.eq_count;
    if !same_context || old.props != t.props || old.grid != t.grid || old.controls != t.controls || old.rows.len() != t.rows.len() {
        return (None, 0);
    }
    // (Not a row holding the paragraph the typed word left: its squiggles changed.)
    let left = |r: &wordcraft_doc::Shared<wordcraft_doc::table::Row>| {
        ctx.typing_left.is_some_and(|rev| r.cells.iter().any(|c| c.blocks.iter().any(|b| holds_rev(b, rev, 0))))
    };
    let keep: Vec<bool> = t.rows.iter().zip(&old.rows).map(|(a, b)| same_row(a, b) && !left(a)).collect();
    // No merged or turned cells. A kept row has the cells of the old one (the same formatting):
    // when the old version is known to have none, only the other rows are looked at.
    let plain = |r: &wordcraft_doc::Shared<wordcraft_doc::table::Row>| {
        r.cells.iter().all(|c| c.props.vmerge == wordcraft_doc::props::VMerge::None && !c.props.text_direction.is_turned())
    };
    let mut scanned = 0;
    let mut plain = |r: &wordcraft_doc::Shared<wordcraft_doc::table::Row>| {
        scanned += 1;
        plain(r)
    };
    let plain_cells = if m.plain { t.rows.iter().zip(&keep).filter(|(_, k)| !**k).all(|(r, _)| plain(r)) } else { t.rows.iter().all(plain) };
    if !plain_cells {
        return (None, scanned);
    }
    let lists = before.is_some();
    let changed_plain =
        t.rows.iter().zip(&keep).filter(|(_, k)| !**k).flat_map(|(r, _)| &r.cells).flat_map(|c| &c.blocks).all(|b| counts_nothing(ctx.doc, b, lists));
    let placed_at = (m.stamp == ctx.prev_gen).then_some(m.placed);
    let no_old_notes = m.note_ids.is_empty();
    let reuse =
        (changed_plain && keep.iter().any(|k| *k)).then(|| crate::table::RowReuse { old: m.layout.clone(), keep, before, placed_at, no_old_notes });
    (reuse, scanned)
}

/// The same row: the same formatting and cells, each with the same blocks (shared). A row
/// shared with the last version of the table (not written to since) is the same.
fn same_row(a: &wordcraft_doc::Shared<wordcraft_doc::table::Row>, b: &wordcraft_doc::Shared<wordcraft_doc::table::Row>) -> bool {
    wordcraft_doc::Shared::ptr_eq(a, b)
        || a.props == b.props
            && a.controls == b.controls
            && a.cells.len() == b.cells.len()
            && a.cells.iter().zip(&b.cells).all(|(x, y)| {
                x.props == y.props
                    && x.controls == y.controls
                    && x.blocks.len() == y.blocks.len()
                    && x.blocks.iter().zip(&y.blocks).all(|(p, q)| Arc::ptr_eq(p, q))
            })
}

/// A paragraph whose layout counts nothing and depends on no count: no list label (unless
/// `lists`), no automatic equation number, no page fields, no text boxes; not a nested table.
fn counts_nothing(doc: &Document, b: &Block, lists: bool) -> bool {
    let Block::Para(p) = b else { return false };
    let listed = p.props.numbering.or_else(|| doc.styles.resolve_para(&p.props).numbering).is_some_and(|n| n.num != 0);
    (lists || !listed)
        && !crate::has_page_fields(p)
        && !crate::has_total_fields(p)
        && !p.objects.iter().any(|o| matches!(o, InlineObject::Equation { .. }) || !o.text_boxes().is_empty())
}

/// The old layout's pages up to checkpoint `cp`, the last as it was then, with the items each
/// had put in front of its list (`behind`, as the old pass ended).
pub(crate) fn pages_upto(prev: &crate::DocLayout, behind: &[usize], cp: &Checkpoint) -> Option<(Vec<Page>, Vec<usize>)> {
    let n = cp.pages;
    let mut pages = take_over(prev.pages.get(..n)?, 0);
    let mut behind: Vec<usize> = behind.get(..n)?.to_vec();
    if let Some(pg) = pages.last_mut() {
        let k = behind.last().copied().unwrap_or(0).saturating_sub(cp.behind).min(pg.items.len());
        pg.items.drain(..k);
        pg.items.truncate(cp.items);
    }
    if let Some(b) = behind.last_mut() {
        *b = cp.behind;
    }
    Some((pages, behind))
}

/// The rest of the old layout from checkpoint `cp` of the new pass, which has the same state as
/// the old pass's: the rest of the page it is on and the pages after it, block numbers moved.
pub(crate) fn take_rest(pb: &mut PageBuilder, cp: &Checkpoint, prev: &crate::DocLayout, t: &Tail) {
    let n = cp.pages;
    if let (Some(pg), Some(old_pg)) = (pb.pages.last_mut(), prev.pages.get(n.wrapping_sub(1))) {
        let mut rest: crate::Items = old_pg.items.get(cp.items..).unwrap_or(&[]).iter().cloned().collect();
        shift_items(&mut rest, t.delta);
        pg.items.extend(rest);
    }
    pb.pages.extend(take_over(prev.pages.get(n..).unwrap_or(&[]), t.delta));
    pb.behind.extend(t.behind.get(n..).unwrap_or(&[]).iter().copied());
}

impl Tail {
    /// The old pass's checkpoint where the new one's `cp` is (its block numbered as it was), by
    /// its index in `cps`.
    pub(crate) fn old_cp(&self, cp: &Checkpoint) -> Option<usize> {
        let ob = cp.block.checked_add_signed(-self.delta)?;
        self.cps.binary_search_by_key(&(ob, cp.row), |c| (c.block, c.row)).ok()
    }

    /// The old pass's checkpoints after checkpoint `i`, block numbers moved (taken: the pass
    /// ends with them).
    pub(crate) fn take_cps_after(&mut self, i: usize) -> Vec<Checkpoint> {
        let delta = self.delta;
        let cps = std::mem::take(&mut self.cps);
        let Some(key) = cps.get(i).map(|c| (c.block, c.row)) else { return Vec::new() };
        cps.into_iter()
            .skip(i + 1)
            .filter(|c| (c.block, c.row) > key)
            .map(|mut c| {
                c.block = c.block.saturating_add_signed(delta);
                c
            })
            .collect()
    }

    /// Where to start again inside table `block` (the first changed block, just laid out): the
    /// old pass's last checkpoint inside it before row `same_until` (the rows before it, header
    /// rows among them, are the old ones) with the walk state the pass has now.
    pub(crate) fn row_start(&self, block: usize, same_until: usize, counters: &Counters, eq_count: u32, page_field: u32) -> Option<&Checkpoint> {
        self.cps.iter().rev().find(|c| {
            c.block == block && c.row > 0 && c.row <= same_until && c.counters == *counters && c.eq_count == eq_count && c.page_field == page_field
        })
    }
}
