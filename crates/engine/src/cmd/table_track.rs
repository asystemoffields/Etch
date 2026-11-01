//! Tracked table structure (#1998). With Track Changes on, rows inserted into a table are row
//! insertions (`w:trPr/w:ins`, ECMA-376 Part 1 §17.13.5.17) and rows deleted from it row deletions
//! (`w:trPr/w:del`, §17.13.5.13); inserted and deleted columns and cells are cell insertions and
//! deletions (`w:cellIns` / `w:cellDel`). A deleted row or cell keeps its text, marked deleted.
//!
//! Accepting a deletion or rejecting an insertion removes the row or cell; a grid column whose
//! every cell goes, goes with them, and a table left without rows goes too. Accepting an
//! insertion or rejecting a deletion keeps the row or cell, which stops being a revision.

use std::collections::BTreeSet;
use std::sync::Arc;

use wordcraft_doc::table::Table;
use wordcraft_doc::{Block, Blocks, Document, Paragraph, Path, Pos, RevisionKind, StoryRef, para_block};

use crate::{CmdError, Session};

/// Tables nested deeper than this are left alone (hostile documents).
const MAX_DEPTH: usize = 16;

/// Which tracked changes an Accept or Reject takes: by revision index.
pub(crate) type Pick<'a> = &'a dyn Fn(u32) -> bool;

/// Whether revision `rev` is the current author's.
pub(crate) fn own(s: &Session, rev: Option<u32>) -> bool {
    rev.and_then(|i| s.doc.revisions.get(i as usize)).is_some_and(|r| r.author == s.author)
}

/// Track Changes on: mark rows `rows` of the table at `tp` inserted (Insert Above / Below, Tab in
/// the last cell). Nothing with tracking off.
pub(crate) fn rows_inserted(s: &mut Session, story: StoryRef, tp: &Path, rows: std::ops::Range<usize>) -> Result<(), CmdError> {
    if !s.doc.settings.track_changes || rows.is_empty() {
        return Ok(());
    }
    let rid = super::new_revision(s, RevisionKind::Insert);
    let t = s.doc.table_mut(story, tp)?;
    for r in rows {
        if let Some(row) = t.rows.get_mut(r) {
            row.props.ins = Some(rid);
            row.props.del = None;
        }
    }
    Ok(())
}

/// Rows `rows` of the table at `tp` were just made like row `like` (Insert Above / Below, Tab in
/// the last cell): a cell copied from a tracked cell insertion or deletion is one too, so the new
/// rows keep to the columns that are coming or going (accepting a deleted column takes their cell
/// too, rejecting an inserted one likewise).
pub(crate) fn copy_cell_marks(s: &mut Session, story: StoryRef, tp: &Path, like: usize, rows: std::ops::Range<usize>) -> Result<(), CmdError> {
    let Some(src) = s.doc.table(story, tp).and_then(|t| t.rows.get(like)) else { return Ok(()) };
    let marks: Vec<(Option<u32>, Option<u32>)> = src.cells.iter().map(|c| (c.props.ins, c.props.del)).collect();
    if marks.iter().all(|m| *m == (None, None)) {
        return Ok(());
    }
    let t = s.doc.table_mut(story, tp)?;
    for r in rows {
        let Some(row) = t.rows.get_mut(r) else { continue };
        for (cell, (ins, del)) in row.cells.iter_mut().zip(&marks) {
            (cell.props.ins, cell.props.del) = (*ins, *del);
        }
    }
    Ok(())
}

/// Track Changes on: mark the cells `cells` (row, cell) of the table at `tp` inserted (Insert
/// Left / Right). Nothing with tracking off.
pub(crate) fn cells_inserted(s: &mut Session, story: StoryRef, tp: &Path, cells: &[(usize, usize)]) -> Result<(), CmdError> {
    if !s.doc.settings.track_changes || cells.is_empty() {
        return Ok(());
    }
    let rid = super::new_revision(s, RevisionKind::Insert);
    let t = s.doc.table_mut(story, tp)?;
    for (r, c) in cells {
        if let Some(cell) = t.rows.get_mut(*r).and_then(|row| row.cells.get_mut(*c)) {
            cell.props.ins = Some(rid);
            cell.props.del = None;
        }
    }
    Ok(())
}

/// A new table going in with Track Changes on (Insert Table, paste): every row an insertion by
/// revision `rid`.
pub(crate) fn mark_table_inserted(t: &mut Table, rid: u32) {
    for row in &mut t.rows {
        row.props.ins = Some(rid);
        row.props.del = None;
    }
}

/// What a tracked delete does to a row or cell.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Fate {
    /// The author's own insertion: it goes (it never existed for the reader).
    Remove,
    /// Already marked deleted: it stays as it is.
    Keep,
    /// Marked deleted, with its text.
    Mark,
}

fn fate(s: &Session, ins: Option<u32>, del: Option<u32>) -> Fate {
    if del.is_some() {
        Fate::Keep
    } else if own(s, ins) {
        Fate::Remove
    } else {
        Fate::Mark
    }
}

/// What a tracked table delete did.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Deleted {
    /// The rows or cells are marked deleted (or were the author's own insertions and went).
    Done,
    /// Everything was already marked deleted: nothing changed.
    Nothing,
    /// Every cell is the author's own insertion: delete them as with tracking off.
    Untracked,
}

/// Mark the text of the rows or cells under `prefixes` deleted by revision `rid`.
fn delete_text(s: &mut Session, story: StoryRef, prefixes: &[Vec<u32>], rid: u32) -> Result<(), CmdError> {
    let spans = super::table::text_spans(s, story, prefixes);
    for (a, b) in spans.iter().rev() {
        super::track_delete_as(s, a, b, rid)?;
    }
    Ok(())
}

/// Track Changes on: delete rows `rows` of the table at `tp` (Delete Rows, Delete Table). A row
/// the author inserted goes; any other is marked deleted with its text, unless it already is. A
/// table left without rows goes.
pub(crate) fn delete_rows(s: &mut Session, story: StoryRef, tp: &Path, rows: &BTreeSet<usize>) -> Result<Deleted, CmdError> {
    let Some(t) = s.doc.table(story, tp) else { return Ok(Deleted::Nothing) };
    let fates: Vec<(usize, Fate)> = rows.iter().filter_map(|r| t.rows.get(*r).map(|row| (*r, fate(s, row.props.ins, row.props.del)))).collect();
    if fates.iter().all(|(_, f)| *f == Fate::Keep) {
        return Ok(Deleted::Nothing);
    }
    let marked: Vec<usize> = fates.iter().filter(|(_, f)| *f == Fate::Mark).map(|(r, _)| *r).collect();
    if !marked.is_empty() {
        let rid = super::new_revision(s, RevisionKind::Delete);
        let prefixes: Vec<Vec<u32>> = marked.iter().map(|r| tp.0.iter().copied().chain([*r as u32]).collect()).collect();
        delete_text(s, story, &prefixes, rid)?;
        let t = s.doc.table_mut(story, tp)?;
        for r in &marked {
            if let Some(row) = t.rows.get_mut(*r) {
                row.props.del = Some(rid);
            }
        }
    }
    let t = s.doc.table_mut(story, tp)?;
    for (r, f) in fates.iter().rev() {
        if *f == Fate::Remove {
            t.delete_row(*r);
        }
    }
    if t.rows.is_empty() {
        s.doc.remove_block(story, tp)?;
    }
    Ok(Deleted::Done)
}

/// Track Changes on: delete the cells `cells` (row, cell) of the table at `tp` (Delete Columns,
/// Delete Cells): each is marked deleted with its text, unless it already is. When every one is
/// the author's own insertion, nothing is marked: they go as with tracking off.
pub(crate) fn delete_cells(s: &mut Session, story: StoryRef, tp: &Path, cells: &[(usize, usize)]) -> Result<Deleted, CmdError> {
    let Some(t) = s.doc.table(story, tp) else { return Ok(Deleted::Nothing) };
    let fates: Vec<((usize, usize), Fate)> = cells
        .iter()
        .filter_map(|&(r, c)| t.rows.get(r).and_then(|row| row.cells.get(c)).map(|cell| ((r, c), fate(s, cell.props.ins, cell.props.del))))
        .collect();
    if fates.is_empty() || fates.iter().all(|(_, f)| *f == Fate::Keep) {
        return Ok(Deleted::Nothing);
    }
    if fates.iter().all(|(_, f)| *f == Fate::Remove) {
        return Ok(Deleted::Untracked);
    }
    let marked: Vec<(usize, usize)> = fates.iter().filter(|(_, f)| *f != Fate::Keep).map(|(rc, _)| *rc).collect();
    let rid = super::new_revision(s, RevisionKind::Delete);
    let prefixes: Vec<Vec<u32>> = marked.iter().map(|(r, c)| tp.0.iter().copied().chain([*r as u32, *c as u32]).collect()).collect();
    delete_text(s, story, &prefixes, rid)?;
    let t = s.doc.table_mut(story, tp)?;
    for (r, c) in marked {
        if let Some(cell) = t.rows.get_mut(r).and_then(|row| row.cells.get_mut(c)) {
            cell.props.del = Some(rid);
        }
    }
    Ok(Deleted::Done)
}

/// The tables a deletion of `a..b` removes whole with tracking off ([`Document::delete_range`]):
/// those between the ends in one container; the top-level ones between a top-level start and an
/// end in a later table; and, from the very start of a top-level table to a later top-level
/// paragraph, that table and the ones up to the paragraph. In document order.
pub(crate) fn tables_deleted_by_range(doc: &Document, a: &Pos, b: &Pos) -> Vec<Path> {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    if a.story != b.story || a.path == b.path {
        return Vec::new();
    }
    let tables_between = |parent: &[u32], from: usize, to: usize| -> Vec<Path> {
        let Some(bl) = doc.container(a.story, &Path(parent.iter().copied().chain([0]).collect())) else { return Vec::new() };
        (from..to.min(bl.len()))
            .filter(|i| matches!(bl.get(*i).map(|b| &**b), Some(Block::Table(_))))
            .map(|i| Path(parent.iter().copied().chain([i as u32]).collect()))
            .collect()
    };
    if a.path.parent() == b.path.parent() {
        return tables_between(a.path.parent(), a.path.last().saturating_add(1), b.path.last());
    }
    if a.path.depth() == 0 && b.path.depth() > 0 {
        let top_b = b.path.0.first().copied().unwrap_or(0) as usize;
        return tables_between(&[], a.path.last().saturating_add(1), top_b);
    }
    if a.path.depth() > 0 && b.path.depth() == 0 && a.off == 0 {
        let ta = a.path.0.first().copied().unwrap_or(0) as usize;
        if first_para(doc, a.story, Path::top(ta)).as_ref() == Some(&a.path) {
            return tables_between(&[], ta, b.path.last());
        }
    }
    Vec::new()
}

/// The first paragraph of the block at `path`: itself, or a table's first cell's (bounded depth).
fn first_para(doc: &Document, story: StoryRef, mut path: Path) -> Option<Path> {
    for _ in 0..=MAX_DEPTH {
        match doc.block(story, &path)? {
            Block::Para(_) => return Some(path),
            Block::Table(_) => path.0.extend([0, 0, 0]),
        }
    }
    None
}

/// A tracked deletion of `a..b` removes the table at `tp` whole: its rows the author inserted go,
/// the others are marked deleted by `rid` (their text the caller marks). A table left without
/// rows goes. Returns whether the table is still there.
pub(crate) fn range_deletes_table(s: &mut Session, story: StoryRef, tp: &Path, rid: u32) -> Result<bool, CmdError> {
    let Some(t) = s.doc.table(story, tp) else { return Ok(false) };
    let fates: Vec<Fate> = t.rows.iter().map(|row| fate(s, row.props.ins, row.props.del)).collect();
    if fates.iter().all(|f| *f == Fate::Keep) {
        return Ok(true);
    }
    let t = s.doc.table_mut(story, tp)?;
    for (r, f) in fates.iter().enumerate().rev() {
        match f {
            Fate::Remove => t.delete_row(r),
            Fate::Mark => {
                if let Some(row) = t.rows.get_mut(r) {
                    row.props.del = Some(rid);
                }
            }
            Fate::Keep => {}
        }
    }
    if t.rows.is_empty() {
        s.doc.remove_block(story, tp)?;
        return Ok(false);
    }
    Ok(true)
}

/// Whether `t` (or a table nested in it) has a tracked row or cell insertion or deletion `pick`
/// takes.
pub(crate) fn has_changes(t: &Table, pick: Pick, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    let tracked = |ins: Option<u32>, del: Option<u32>| ins.is_some_and(pick) || del.is_some_and(pick);
    t.rows.iter().any(|row| {
        tracked(row.props.ins, row.props.del)
            || row.cells.iter().any(|c| {
                tracked(c.props.ins, c.props.del) || c.blocks.iter().any(|b| matches!(&**b, Block::Table(n) if has_changes(n, pick, depth + 1)))
            })
    })
}

/// Accept or reject the tracked row and cell insertions and deletions `pick` takes, in the rows
/// `in_row` and cells `in_cell` choose of `t` (tables nested in its cells aren't touched). Those
/// that go are removed — a grid column every remaining row's cell over it goes with (a cell
/// spanning one column) goes too — and the others stop being revisions. Rejecting inserted
/// columns gives the other columns back the width inserting them took. Rows left without cells
/// go; the caller removes a table left without rows. Returns whether anything changed.
pub(crate) fn resolve_table(t: &mut Table, accept: bool, pick: Pick, in_row: &dyn Fn(usize) -> bool, in_cell: &dyn Fn(usize, usize) -> bool) -> bool {
    let goes = |ins: Option<u32>, del: Option<u32>| if accept { del.is_some_and(pick) } else { ins.is_some_and(pick) };
    let marked = |ins: Option<u32>, del: Option<u32>| ins.is_some_and(pick) || del.is_some_and(pick);
    let row_goes: Vec<bool> = t.rows.iter().enumerate().map(|(r, row)| in_row(r) && goes(row.props.ins, row.props.del)).collect();
    let cell_goes: Vec<Vec<bool>> = t
        .rows
        .iter()
        .enumerate()
        .map(|(r, row)| {
            let gone = row_goes.get(r).copied().unwrap_or(false);
            row.cells.iter().enumerate().map(|(c, cell)| !gone && in_cell(r, c) && goes(cell.props.ins, cell.props.del)).collect()
        })
        .collect();
    let goes_at = |r: usize, c: usize| cell_goes.get(r).and_then(|x| x.get(c)).copied().unwrap_or(false);
    // Grid columns whose every cell (in the rows that stay) goes.
    let mut dropped = Vec::new();
    for g in 0..t.grid.len() {
        let (mut any, mut all) = (false, true);
        for r in 0..t.rows.len() {
            if row_goes.get(r).copied().unwrap_or(false) {
                continue;
            }
            if let Some(ci) = t.cell_at_grid(r, g) {
                let one = t.rows.get(r).and_then(|row| row.cells.get(ci)).is_some_and(|c| c.span() == 1);
                if goes_at(r, ci) && one {
                    any = true;
                } else {
                    all = false;
                    break;
                }
            }
        }
        if any && all {
            dropped.push(g);
        }
    }
    let mut changed = false;
    for (r, row) in t.rows.iter_mut().enumerate() {
        if row_goes.get(r).copied().unwrap_or(false) {
            continue;
        }
        let clear_row = in_row(r) && marked(row.props.ins, row.props.del);
        let touch_cells = row.cells.iter().enumerate().any(|(c, cell)| goes_at(r, c) || (in_cell(r, c) && marked(cell.props.ins, cell.props.del)));
        if !clear_row && !touch_cells {
            continue;
        }
        changed = true;
        if clear_row {
            row.props.ins = row.props.ins.filter(|v| !pick(*v));
            row.props.del = row.props.del.filter(|v| !pick(*v));
        }
        let row = &mut **row;
        for (c, cell) in row.cells.iter_mut().enumerate() {
            if !goes_at(r, c) && in_cell(r, c) {
                cell.props.ins = cell.props.ins.filter(|v| !pick(*v));
                cell.props.del = cell.props.del.filter(|v| !pick(*v));
            }
        }
        let mut c = 0;
        row.cells.retain(|_| {
            let keep = !goes_at(r, c);
            c += 1;
            keep
        });
    }
    if !dropped.is_empty() {
        changed = true;
        let total: f32 = t.grid.iter().sum();
        for g in dropped.iter().rev() {
            if *g < t.grid.len() {
                t.grid.remove(*g);
            }
        }
        // Insert Left / Right narrowed the other columns to keep the table's width; rejecting the
        // column widens them back.
        let left: f32 = t.grid.iter().sum();
        if !accept && total.is_finite() && left > 0.0 && total > left {
            let k = total / left;
            t.grid.iter_mut().for_each(|w| *w *= k);
        }
    }
    let mut r = 0;
    let before = t.rows.len();
    t.rows.retain(|row| {
        let keep = !row_goes.get(r).copied().unwrap_or(false) && !row.cells.is_empty();
        r += 1;
        keep
    });
    changed || t.rows.len() != before
}

/// Accept or reject the tracked row and cell changes `pick` takes in every table of `blocks`,
/// nested tables first; tables left without rows go (a container never ends with a table).
/// Returns whether anything changed.
pub(crate) fn resolve_blocks(blocks: &mut Blocks, accept: bool, pick: Pick, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    let mut changed = false;
    let mut i = blocks.len();
    while i > 0 {
        i -= 1;
        let Some(b) = blocks.get_mut(i) else { continue };
        if !matches!(&**b, Block::Table(t) if has_changes(t, pick, depth)) {
            continue;
        }
        let Block::Table(t) = Arc::make_mut(b) else { continue };
        for row in &mut t.rows {
            let nested = row.cells.iter().any(|c| c.blocks.iter().any(|b| matches!(&**b, Block::Table(n) if has_changes(n, pick, depth + 1))));
            if nested {
                for cell in &mut row.cells {
                    changed |= resolve_blocks(&mut cell.blocks, accept, pick, depth + 1);
                }
            }
        }
        changed |= resolve_table(t, accept, pick, &|_| true, &|_, _| true);
        if t.rows.is_empty() {
            blocks.remove(i);
            changed = true;
        }
    }
    if changed && (blocks.is_empty() || matches!(blocks.last().map(|b| &**b), Some(Block::Table(_)))) {
        blocks.push(para_block(Paragraph::new()));
    }
    changed
}

/// Accept or reject the row and cell changes chosen in the table at `tp` (see [`resolve_table`]);
/// a table left without rows goes. Returns whether anything changed.
pub(crate) fn resolve_at(
    s: &mut Session,
    story: StoryRef,
    tp: &Path,
    accept: bool,
    pick: Pick,
    in_row: &dyn Fn(usize) -> bool,
    in_cell: &dyn Fn(usize, usize) -> bool,
) -> Result<bool, CmdError> {
    if s.doc.table(story, tp).is_none_or(|t| !has_changes(t, pick, 0)) {
        return Ok(false);
    }
    let t = s.doc.table_mut(story, tp)?;
    let changed = resolve_table(t, accept, pick, in_row, in_cell);
    if t.rows.is_empty() {
        s.doc.remove_block(story, tp)?;
    }
    Ok(changed)
}
