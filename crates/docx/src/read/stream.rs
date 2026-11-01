//! Stories read a block at a time while their part is parsed ([`crate::xml::Chunks`]).
//!
//! A block (a paragraph, a table row, a block of a cell) is read as soon as the parser has read
//! it, and its tree let go: a story's tree is never held whole, however long the story is, so
//! what a part holds while it is read is about one block (and what the file's node budget
//! allows at most), not the part. Every list of blocks is read this way: a story's, a table
//! cell's, a block content control's, a note's or comment's. What only groups blocks (custom
//! XML, smart tags, insertions) or rows and cells (content controls, custom XML) is looked
//! through as the reader of whole trees looks through it, so a part reads the same either way
//! (`tests_stream::streamed_stories_read_as_whole_trees_do` compares them).

use std::sync::Arc;

use wordcraft_doc::control::{ContentControl, ControlWrap};
use wordcraft_doc::props::CellProps;
use wordcraft_doc::table::{MAX_COLS, Row};
use wordcraft_doc::{Block, Blocks};

use super::story::{MAX_TABLE_DEPTH, StoryCtx, TableRows, block_control, merge_cell, wrap_in_control};
use super::{CommentRead, CommentsRead, NoteRead, Reader};
use crate::package::Rels;
use crate::xml::{El, MAX_DEPTH, Node};

#[cfg(test)]
thread_local! {
    /// Read stories from whole trees instead (as before they were read a block at a time), to
    /// compare the two.
    pub(super) static WHOLE_TREES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Are stories read from whole trees on this thread (tests compare)?
#[cfg(test)]
pub(super) fn whole_trees() -> bool {
    WHOLE_TREES.with(std::cell::Cell::get)
}

/// Deepest content controls and custom XML looked through around rows or cells (as `collect`).
const MAX_AROUND_DEPTH: usize = 8;

/// Why a file with content controls or custom XML around rows or cells nested deeper than
/// [`MAX_AROUND_DEPTH`] is refused (the rows and cells inside them would be left out).
fn too_deep_around() -> String {
    format!("content controls or custom XML around table rows or cells nested more than {MAX_AROUND_DEPTH} deep")
}

/// What a part's stories are.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Kind {
    /// The main part: the blocks of the first `w:document/w:body`, and its page background.
    Body,
    /// A part whose root holds blocks (a header or footer).
    Root,
    /// Footnotes (`foot`) or endnotes: each note, and each separator, a story of its own.
    Notes { foot: bool },
    /// Comments: each a story of its own.
    Comments,
}

/// An element whose children are read as they come.
struct Frame {
    /// How many elements were open when it started (it is `open[at]` while it is open).
    at: usize,
    /// For a content control: how many `w:sdtContent` it has had (the children of the first are
    /// what is read).
    contents: Option<u8>,
    what: What,
}

impl Frame {
    /// Are `open` (the root first) this frame's element, and inside it what its children are in?
    fn holds(&self, open: &[El]) -> bool {
        match self.contents {
            None => open.len() == self.at + 1,
            Some(n) => n == 1 && open.len() == self.at + 2 && open.last().is_some_and(|e| e.name == "w:sdtContent"),
        }
    }
}

enum What {
    /// A list of blocks, read at `depth`.
    Blocks { blocks: Blocks, depth: usize, of: Of },
    /// Custom XML, a smart tag or an insertion around blocks: they go to the list around it.
    Group { depth: usize },
    /// A table among blocks read at `depth`: `found` rows so far (read or not), and whether the
    /// last of them was added (`last_kept`).
    Table { rows: TableRows, depth: usize, found: usize, last_kept: bool },
    /// A row of a table at `depth`, made once its properties have been read (when its first cell
    /// starts), with the content controls around it (`wrap`, until then); `found` cells so far,
    /// whether the last was added, and whether the paragraph budget stopped its cells (`done`).
    Row { row: Option<Row>, wrap: Option<ControlWrap>, depth: usize, found: usize, last_kept: bool, done: bool },
    /// A content control (`control`) or custom XML around rows, or around cells (`cells`): the
    /// rows or cells found before it started, whether its control has gone to one of them, and
    /// how deep it is among such wrappers.
    Around { control: bool, cells: bool, found: usize, given: bool, depth: usize },
}

impl What {
    fn is_owner(&self) -> bool {
        matches!(self, What::Table { .. } | What::Row { .. })
    }
}

/// What a list of blocks is.
enum Of {
    /// A story's own (the main part's body, a header's or footer's root).
    Story,
    /// A block content control's content.
    Control,
    /// A cell's: its properties once read, the content controls around it, and whether its
    /// content isn't read (the cell continues the one before it, a horizontal merge).
    Cell { props: Option<CellProps>, wrap: ControlWrap, skip: bool },
    /// A footnote, endnote or separator.
    Note(NoteRead),
    /// A comment.
    Comment(CommentRead),
}

/// A part's stories read a block at a time while it is parsed.
pub(super) struct StoryReader<'s, 'p> {
    reader: &'s mut Reader<'p>,
    rels: &'s Rels,
    sc: StoryCtx,
    kind: Kind,
    /// The elements open whose children are read as they come (innermost last).
    frames: Vec<Frame>,
    /// The story's blocks (main part, header or footer), once read.
    out: Blocks,
    /// The body's first `w:sectPr` (the last section's properties).
    sect: Option<El>,
    /// Bodies started (only the first is read), and whether a page background was read.
    bodies: usize,
    background: bool,
    /// No more notes or comments are read (no more parts may be made).
    stopped: bool,
    comments: CommentsRead,
}

impl<'s, 'p> StoryReader<'s, 'p> {
    fn new(reader: &'s mut Reader<'p>, rels: &'s Rels, sc: StoryCtx, kind: Kind) -> Self {
        StoryReader {
            reader,
            rels,
            sc,
            kind,
            frames: Vec::new(),
            out: Blocks::new(),
            sect: None,
            bodies: 0,
            background: false,
            stopped: false,
            comments: CommentsRead::default(),
        }
    }

    /// The main part's story (`w:document/w:body`) and page background.
    pub fn body(reader: &'s mut Reader<'p>, rels: &'s Rels) -> Self {
        Self::new(reader, rels, StoryCtx::default(), Kind::Body)
    }

    /// The story of a part whose root holds its blocks (a header or footer), read with `sc`.
    pub fn root(reader: &'s mut Reader<'p>, rels: &'s Rels, sc: StoryCtx) -> Self {
        Self::new(reader, rels, sc, Kind::Root)
    }

    /// The notes of a footnotes (`foot`) or endnotes part.
    pub fn notes(reader: &'s mut Reader<'p>, rels: &'s Rels, foot: bool) -> Self {
        Self::new(reader, rels, StoryCtx::default(), Kind::Notes { foot })
    }

    /// The comments of a comments part, with what is known of the file's comments so far.
    pub fn comments(reader: &'s mut Reader<'p>, rels: &'s Rels, comments: CommentsRead) -> Self {
        StoryReader { comments, ..Self::new(reader, rels, StoryCtx::default(), Kind::Comments) }
    }

    /// What was read of a story: its context, its blocks, the body's last section properties,
    /// and whether there was a body (the main part).
    pub fn finish(mut self) -> (StoryCtx, Blocks, Option<El>, bool) {
        // A header's root is never handed on: its blocks are those of the frame left.
        while let Some(f) = self.frames.pop() {
            if let What::Blocks { blocks, of: Of::Story, .. } = f.what {
                self.out = blocks;
            }
        }
        (self.sc, self.out, self.sect, self.bodies > 0)
    }

    /// What was read of a comments part.
    pub fn finish_comments(self) -> CommentsRead {
        self.comments
    }

    /// The innermost list of blocks open.
    fn list(&mut self) -> Option<&mut Blocks> {
        self.frames.iter_mut().rev().find_map(|f| match &mut f.what {
            What::Blocks { blocks, .. } => Some(blocks),
            _ => None,
        })
    }

    /// The innermost table or row open (what rows or cells found now belong to).
    fn owner(&self) -> Option<usize> {
        self.frames.iter().rposition(|f| f.what.is_owner())
    }

    fn push(&mut self, at: usize, contents: Option<u8>, what: What) {
        self.frames.push(Frame { at, contents, what });
    }

    /// The content controls opening before a row or cell just found, outermost first: those of
    /// the content controls around it that haven't gone to a row or cell before it.
    fn opening(&mut self, owner: usize, open: &[El]) -> Vec<ContentControl> {
        let mut out = Vec::new();
        for f in self.frames.iter_mut().skip(owner + 1) {
            if let What::Around { control: true, given, .. } = &mut f.what
                && !*given
            {
                *given = true;
                if let Some(sdt) = open.get(f.at) {
                    out.push(around_control(sdt));
                }
            }
        }
        out
    }

    // ---- elements starting ----

    /// A child of the part's root (or the root itself) starts; `true`: it is handed on.
    fn top_starts(&mut self, open: &[El], el: &El) -> bool {
        let n = open.len();
        match self.kind {
            Kind::Root => {
                if n == 0 {
                    self.push(0, None, What::Blocks { blocks: Blocks::new(), depth: 0, of: Of::Story });
                }
                false
            }
            Kind::Body => {
                if n == 1 && el.name == "w:body" {
                    self.bodies += 1;
                    if self.bodies == 1 {
                        self.push(1, None, What::Blocks { blocks: Blocks::new(), depth: 0, of: Of::Story });
                    }
                }
                // The root's other children (a page background, a second body) are handed on too.
                n == 1
            }
            Kind::Notes { foot } => {
                if n == 1
                    && el.name == if foot { "w:footnote" } else { "w:endnote" }
                    && let Some(note) = self.reader.begin_note(el, foot, &mut self.stopped)
                {
                    self.sc = StoryCtx::default();
                    self.push(1, None, What::Blocks { blocks: Blocks::new(), depth: 0, of: Of::Note(note) });
                }
                n == 1
            }
            Kind::Comments => {
                if n == 1
                    && el.name == "w:comment"
                    && let Some(c) = self.reader.begin_comment(el, &mut self.comments, &mut self.stopped)
                {
                    self.sc = StoryCtx::default();
                    self.push(1, None, What::Blocks { blocks: Blocks::new(), depth: 0, of: Of::Comment(c) });
                }
                n == 1
            }
        }
    }

    /// A child of a list of blocks (or of what groups blocks) starts.
    fn block_starts(&mut self, open: &[El], el: &El) -> bool {
        let n = open.len();
        let (depth, cell) = match self.frames.last().map(|f| &f.what) {
            Some(What::Blocks { depth, of: Of::Cell { .. }, .. }) => (*depth, true),
            Some(What::Blocks { depth, .. } | What::Group { depth }) => (*depth, false),
            _ => return true,
        };
        if cell {
            // A cell's properties stay with it; its first other child reads them (they come
            // first), and a merged cell's content isn't read.
            if el.name == "w:tcPr" {
                return false;
            }
            if self.cell_props(open) {
                return true;
            }
        }
        match el.name.as_str() {
            "w:tbl" if depth / 4 < MAX_TABLE_DEPTH => {
                self.push(n, None, What::Table { rows: TableRows::default(), depth, found: 0, last_kept: false });
            }
            "w:sdt" if depth < MAX_DEPTH => {
                self.push(n, Some(0), What::Blocks { blocks: Blocks::new(), depth: depth + 1, of: Of::Control });
            }
            "w:customXml" | "w:smartTag" | "w:ins" | "w:moveTo" if depth < MAX_DEPTH => {
                self.push(n, None, What::Group { depth: depth + 1 });
            }
            _ => {}
        }
        true
    }

    /// The properties of the cell whose list is innermost (its element `open[at]`), read once,
    /// merging it into the cell before when it continues it; true when it is merged (skipped).
    fn cell_props(&mut self, open: &[El]) -> bool {
        let Some(f) = self.frames.last() else { return false };
        match &f.what {
            What::Blocks { of: Of::Cell { props: Some(_), skip, .. }, .. } => return *skip,
            What::Blocks { of: Of::Cell { props: None, .. }, .. } => {}
            _ => return false,
        }
        let Some(tc) = open.get(f.at) else { return false };
        let (props, hcont) = self.reader.cell_props(tc);
        let merged = self.row().is_some_and(|row| merge_cell(row, &props, hcont));
        if let Some(Frame { what: What::Blocks { of: Of::Cell { props: slot, skip, .. }, .. }, .. }) = self.frames.last_mut() {
            *slot = Some(props);
            *skip = merged;
        }
        merged
    }

    /// The innermost row open, as made so far.
    fn row(&mut self) -> Option<&mut Row> {
        self.frames.iter_mut().rev().find_map(|f| match &mut f.what {
            What::Row { row, .. } => row.as_mut(),
            _ => None,
        })
    }

    /// A child of a table, or of what is around rows, starts.
    fn row_level_starts(&mut self, open: &[El], el: &El, parent_depth: usize, in_table: bool) -> bool {
        let n = open.len();
        match el.name.as_str() {
            "w:tblPr" | "w:tblGrid" if in_table => return false,
            "w:tr" => self.row_found(open),
            "w:sdt" if parent_depth < MAX_AROUND_DEPTH => self.around(n, true, false, parent_depth + 1),
            "w:customXml" if parent_depth < MAX_AROUND_DEPTH => self.around(n, false, false, parent_depth + 1),
            "w:sdt" | "w:customXml" => self.reader.refuse(too_deep_around),
            _ => {}
        }
        true
    }

    /// A child of a row, or of what is around cells, starts.
    fn cell_level_starts(&mut self, open: &[El], el: &El, parent_depth: usize, in_row: bool) -> bool {
        let n = open.len();
        if in_row {
            if matches!(el.name.as_str(), "w:trPr" | "w:tblPrEx") {
                return false;
            }
            self.make_row(open);
        }
        match el.name.as_str() {
            "w:tc" => self.cell_found(open),
            "w:sdt" if parent_depth < MAX_AROUND_DEPTH => self.around(n, true, true, parent_depth + 1),
            "w:customXml" if parent_depth < MAX_AROUND_DEPTH => self.around(n, false, true, parent_depth + 1),
            "w:sdt" | "w:customXml" => self.reader.refuse(too_deep_around),
            _ => {}
        }
        true
    }

    fn around(&mut self, n: usize, control: bool, cells: bool, depth: usize) {
        let found = self.owner().and_then(|i| self.frames.get(i)).map_or(0, |f| match &f.what {
            What::Table { found, .. } | What::Row { found, .. } => *found,
            _ => 0,
        });
        self.push(n, control.then_some(0), What::Around { control, cells, found, given: false, depth });
    }

    /// A row starts: it is read (a frame of its own) unless the table has all the rows it may.
    fn row_found(&mut self, open: &[El]) {
        let Some(owner) = self.owner() else { return };
        let opening = self.opening(owner, open);
        let Some(f) = self.frames.get_mut(owner) else { return };
        let table_el = open.get(f.at);
        let What::Table { rows, depth, found, last_kept } = &mut f.what else { return };
        *found += 1;
        *last_kept = false;
        if !rows.take_row() {
            self.reader.refuse(super::story::too_many_rows);
            return;
        }
        let depth = *depth;
        if let Some(t) = table_el {
            self.reader.table_props(rows, t);
        }
        let wrap = ControlWrap { open: opening, close: 0 };
        self.push(open.len(), None, What::Row { row: None, wrap: Some(wrap), depth, found: 0, last_kept: false, done: false });
    }

    /// The innermost row, made from its element (`open[at]`, its properties read) if it isn't yet.
    fn make_row(&mut self, open: &[El]) {
        let Some(f) = self.frames.iter_mut().rev().find(|f| matches!(f.what, What::Row { .. })) else { return };
        let tr = open.get(f.at);
        if let What::Row { row: row @ None, wrap, .. } = &mut f.what
            && let Some(tr) = tr
        {
            *row = Some(self.reader.new_row(tr, wrap.take().unwrap_or_default()));
        }
    }

    /// A cell starts: it is read (a list of its own) unless its row has all the cells it may, or
    /// the paragraph budget stopped the row.
    fn cell_found(&mut self, open: &[El]) {
        let Some(owner) = self.owner() else { return };
        let opening = self.opening(owner, open);
        let Some(What::Row { depth, found, last_kept, done, .. }) = self.frames.get_mut(owner).map(|f| &mut f.what) else { return };
        *found += 1;
        *last_kept = false;
        if *found > MAX_COLS {
            self.reader.refuse(super::story::too_many_columns);
            return;
        }
        if *done {
            return;
        }
        let depth = *depth + 4;
        let wrap = ControlWrap { open: opening, close: 0 };
        self.push(open.len(), None, What::Blocks { blocks: Blocks::new(), depth, of: Of::Cell { props: None, wrap, skip: false } });
    }

    // ---- elements ending ----

    /// The element of frame `f` ended (`el`, what is left of it).
    fn close(&mut self, f: Frame, el: El) {
        match f.what {
            What::Blocks { blocks, of: Of::Story, .. } => self.out = blocks,
            What::Blocks { blocks, of: Of::Control, .. } => {
                let ctl = block_control(&el);
                if let Some(list) = self.list() {
                    wrap_in_control(ctl, blocks, list);
                }
            }
            What::Blocks { blocks, of: Of::Cell { props, wrap, skip }, .. } => self.close_cell(&el, props, wrap, skip, blocks),
            What::Blocks { blocks, of: Of::Note(note), .. } => {
                let foot = matches!(self.kind, Kind::Notes { foot: true });
                self.reader.end_note(note, &mut self.sc, blocks, foot);
            }
            What::Blocks { blocks, of: Of::Comment(c), .. } => self.reader.end_comment(c, &mut self.comments, &mut self.sc, blocks),
            What::Group { .. } => {}
            What::Table { rows, .. } => {
                if let Some(t) = self.reader.finish_table(rows, &el)
                    && let Some(list) = self.list()
                {
                    list.push(Arc::new(Block::Table(t)));
                }
            }
            What::Row { row, wrap, .. } => {
                let row = match row {
                    Some(r) => r,
                    None => self.reader.new_row(&el, wrap.unwrap_or_default()),
                };
                if let Some(What::Table { rows, last_kept, .. }) = self.owner().and_then(|i| self.frames.get_mut(i)).map(|f| &mut f.what) {
                    *last_kept = rows.add_row(row);
                }
            }
            What::Around { control, found, .. } => {
                if !control {
                    return;
                }
                // One more control closes after the last row or cell found in it, if that was kept.
                match self.owner().and_then(|i| self.frames.get_mut(i)).map(|f| &mut f.what) {
                    Some(What::Table { rows, found: now, last_kept: true, .. }) if *now > found => rows.close_after_last(),
                    Some(What::Row { row: Some(row), found: now, last_kept: true, .. }) if *now > found => {
                        if let Some(c) = row.cells.last_mut() {
                            c.controls.close = c.controls.close.saturating_add(1);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn close_cell(&mut self, tc: &El, props: Option<CellProps>, wrap: ControlWrap, skip: bool, blocks: Blocks) {
        let props = match props {
            Some(p) => p,
            None => {
                // No child but its properties: read them now.
                let (props, hcont) = self.reader.cell_props(tc);
                if self.row().is_some_and(|row| merge_cell(row, &props, hcont)) {
                    return;
                }
                props
            }
        };
        if skip {
            return;
        }
        let Some(owner) = self.owner() else { return };
        let Some(What::Row { row: Some(row), last_kept, done, .. }) = self.frames.get_mut(owner).map(|f| &mut f.what) else { return };
        if self.reader.add_cell(&mut self.sc, row, props, blocks, wrap) {
            *last_kept = true;
        } else {
            *done = true;
        }
    }

    /// A child of the innermost frame ended: read it if it is a block.
    fn child(&mut self, el: El) {
        let body = self.kind == Kind::Body;
        let Some(f) = self.frames.last_mut() else { return };
        let depth = match &mut f.what {
            What::Blocks { of: Of::Cell { skip: true, .. }, .. } => return,
            What::Blocks { of: Of::Story, .. } if body && el.name == "w:sectPr" => {
                if self.sect.is_none() {
                    self.sect = Some(el);
                }
                return;
            }
            What::Blocks { depth, of: Of::Comment(c), .. } => {
                if el.name == "w:p" {
                    c.last_para = el.attr("w14:paraId").map(str::to_string);
                }
                *depth
            }
            What::Blocks { depth, .. } | What::Group { depth } => *depth,
            // Rows and cells have frames of their own; what else is in tables and rows is left.
            _ => return,
        };
        let node = Node::El(el);
        let (reader, sc, rels) = (&mut *self.reader, &mut self.sc, self.rels);
        let Some(list) = self.frames.iter_mut().rev().find_map(|f| match &mut f.what {
            What::Blocks { blocks, .. } => Some(blocks),
            _ => None,
        }) else {
            return;
        };
        reader.read_block_nodes(sc, std::slice::from_ref(&node), rels, list, depth);
    }

    /// A child of the root ended that no story holds: the page background is read.
    fn top_level(&mut self, open: &[El], el: El) {
        if self.kind == Kind::Body && open.len() == 1 && el.name == "w:background" && !self.background {
            self.background = true;
            self.reader.read_background(&el, self.rels);
        }
    }
}

/// The content control `sdt` around rows or cells is (as `collect` reads it).
fn around_control(sdt: &El) -> ContentControl {
    let mut ctl = super::sdt::sdt_pr(sdt.child("w:sdtPr"), true);
    ctl.end_pr_xml = super::sdt::sdt_end_pr(sdt.child("w:sdtEndPr"));
    ctl
}

impl crate::xml::Chunks for StoryReader<'_, '_> {
    fn is_chunk(&mut self, open: &[El], el: &El) -> bool {
        let Some(f) = self.frames.last_mut() else { return self.top_starts(open, el) };
        if !f.holds(open) {
            // A content control's content starts (or anything else inside a frame's element
            // that isn't one of its children, or inside a child).
            if open.len() == f.at + 1
                && el.name == "w:sdtContent"
                && let Some(n) = &mut f.contents
            {
                *n = n.saturating_add(1);
            }
            return false;
        }
        match &f.what {
            What::Blocks { .. } | What::Group { .. } => self.block_starts(open, el),
            What::Table { .. } => self.row_level_starts(open, el, 0, true),
            What::Row { .. } => self.cell_level_starts(open, el, 0, true),
            What::Around { cells: false, depth, .. } => {
                let d = *depth;
                self.row_level_starts(open, el, d, false)
            }
            What::Around { cells: true, depth, .. } => {
                let d = *depth;
                self.cell_level_starts(open, el, d, false)
            }
        }
    }

    fn holds_chunks(&self, open: &[El]) -> bool {
        self.frames.last().is_some_and(|f| f.holds(open))
    }

    fn chunk(&mut self, open: &[El], el: El) -> Result<(), crate::DocxError> {
        match self.frames.last() {
            Some(f) if f.at == open.len() => {
                if let Some(f) = self.frames.pop() {
                    self.close(f, el);
                }
            }
            Some(f) if f.holds(open) => self.child(el),
            _ => self.top_level(open, el),
        }
        Ok(())
    }
}
