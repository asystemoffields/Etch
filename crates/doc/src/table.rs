//! Tables: a column grid, rows and cells (cells hold block lists). Merges follow OOXML: a cell
//! spans `span` grid columns; vertical merges are `Restart` + `Continue` cells.

use serde::{Deserialize, Serialize};

use crate::control::ControlWrap;
use crate::props::{CellProps, RowProps, TableProps, VMerge};
use crate::{Blocks, Paragraph, Shared, para_block};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Cell {
    #[serde(default)]
    pub props: CellProps,
    pub blocks: Blocks,
    /// Content controls around the cell (see [`crate::control`]).
    #[serde(default, skip_serializing_if = "ControlWrap::is_empty")]
    pub controls: ControlWrap,
}

impl Cell {
    pub fn empty() -> Cell {
        Cell { props: CellProps { span: 1, ..Default::default() }, blocks: vec![para_block(Paragraph::new())], controls: ControlWrap::default() }
    }
    pub fn with_text(s: &str) -> Cell {
        Cell {
            props: CellProps { span: 1, ..Default::default() },
            blocks: vec![para_block(Paragraph::with_text(s, Default::default()))],
            controls: ControlWrap::default(),
        }
    }
    pub fn span(&self) -> usize {
        self.props.span.clamp(1, 63) as usize
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Row {
    #[serde(default)]
    pub props: RowProps,
    pub cells: Vec<Cell>,
    /// Content controls around the row (repeating sections; see [`crate::control`]).
    #[serde(default, skip_serializing_if = "ControlWrap::is_empty")]
    pub controls: ControlWrap,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Table {
    #[serde(default)]
    pub props: TableProps,
    /// Grid column widths, points.
    pub grid: Vec<f32>,
    /// Rows, each [`Shared`]: a copy of the table (an undo step after typing in one cell) shares
    /// the rows it doesn't change.
    pub rows: Vec<Shared<Row>>,
    /// Content controls whose content starts or ends with this table (see [`crate::control`]).
    #[serde(default, skip_serializing_if = "ControlWrap::is_empty")]
    pub controls: ControlWrap,
}

/// Hard limits that keep hostile input from exhausting memory.
pub const MAX_ROWS: usize = 32_767;
pub const MAX_COLS: usize = 63;

impl Table {
    /// A `rows × cols` table spanning `width` points, "Table Grid" style.
    pub fn new(rows: usize, cols: usize, width: f32) -> Table {
        let rows = rows.clamp(1, MAX_ROWS);
        let cols = cols.clamp(1, MAX_COLS);
        let w = (width / cols as f32).max(6.0);
        Table {
            props: TableProps { style: Some("TableGrid".into()), ..Default::default() },
            grid: vec![w; cols],
            rows: (0..rows)
                .map(|_| Row {
                    props: RowProps::default(),
                    cells: (0..cols)
                        .map(|_| {
                            let mut c = Cell::empty();
                            c.props.width = Some(w);
                            c
                        })
                        .collect(),
                    controls: ControlWrap::default(),
                })
                .map(Shared::new)
                .collect(),
            controls: ControlWrap::default(),
        }
    }

    pub fn cols(&self) -> usize {
        self.grid.len().max(self.rows.iter().map(|r| r.cells.iter().map(Cell::span).sum::<usize>()).max().unwrap_or(0))
    }

    /// Grid column where cell `c` of row `r` starts.
    pub fn grid_col(&self, r: usize, c: usize) -> usize {
        self.rows.get(r).map(|row| row.cells.iter().take(c).map(Cell::span).sum()).unwrap_or(0)
    }

    /// Cell index in row `r` that covers grid column `g`.
    pub fn cell_at_grid(&self, r: usize, g: usize) -> Option<usize> {
        let row = self.rows.get(r)?;
        let mut x = 0;
        for (i, c) in row.cells.iter().enumerate() {
            if g < x + c.span() {
                return Some(i);
            }
            x += c.span();
        }
        None
    }

    /// The cell of row `r` that starts at grid column `g` and spans `span` columns, if there is one.
    fn cell_starting(&self, r: usize, g: usize, span: usize) -> Option<usize> {
        let row = self.rows.get(r)?;
        let mut x = 0;
        for (i, c) in row.cells.iter().enumerate() {
            if x == g {
                return (c.span() == span).then_some(i);
            }
            if x > g {
                return None;
            }
            x += c.span();
        }
        None
    }

    /// Whether the cell of row `r` at grid column `g` (spanning `span`) is part of a vertical
    /// merge that goes on into the row below.
    fn merge_goes_down(&self, r: usize, g: usize, span: usize) -> bool {
        let here = self.cell_starting(r, g, span).and_then(|i| self.rows.get(r)?.cells.get(i)).map(|c| c.props.vmerge);
        let below = self.cell_starting(r + 1, g, span).and_then(|i| self.rows.get(r + 1)?.cells.get(i)).map(|c| c.props.vmerge);
        matches!(here, Some(VMerge::Restart | VMerge::Continue)) && below == Some(VMerge::Continue)
    }

    /// Insert a row copied (formatting, empty text) from row `like` at index `at`. A vertically
    /// merged cell the new row lands inside (merged both above and below it) takes it in;
    /// elsewhere the new row's cells stand alone.
    pub fn insert_row(&mut self, at: usize, like: usize) {
        let Some(src) = self.rows.get(like.min(self.rows.len().saturating_sub(1))) else { return };
        if self.rows.len() >= MAX_ROWS {
            return;
        }
        let at = at.min(self.rows.len());
        // Formatted like `like`, not tracked like it: the caller marks a tracked insertion.
        let mut row = Row { props: src.props.formatting(), cells: Vec::with_capacity(src.cells.len()), controls: ControlWrap::default() };
        row.props.header = false;
        let mut g = 0;
        for c in &src.cells {
            let mut nc = Cell::empty();
            nc.props = c.props.formatting();
            let inside = at > 0 && self.merge_goes_down(at - 1, g, c.span());
            nc.props.vmerge = if inside { VMerge::Continue } else { VMerge::None };
            g += c.span();
            row.cells.push(nc);
        }
        self.rows.insert(at, row.into());
    }

    /// Delete row `r`. A vertically merged cell that starts in it and goes on below keeps its
    /// text: the cell below starts the merge now and takes the text.
    pub fn delete_row(&mut self, r: usize) {
        if r >= self.rows.len() {
            return;
        }
        let mut g = 0;
        let mut moves = Vec::new();
        for (i, c) in self.rows.get(r).map(|row| row.cells.as_slice()).unwrap_or_default().iter().enumerate() {
            if c.props.vmerge == VMerge::Restart && self.merge_goes_down(r, g, c.span()) {
                moves.push((i, g, c.span()));
            }
            g += c.span();
        }
        for (i, g, span) in moves {
            let Some(below) = self.cell_starting(r + 1, g, span) else { continue };
            let blocks = self.rows.get_mut(r).and_then(|row| row.cells.get_mut(i)).map(|c| std::mem::take(&mut c.blocks)).unwrap_or_default();
            if let Some(c) = self.rows.get_mut(r + 1).and_then(|row| row.cells.get_mut(below)) {
                c.props.vmerge = VMerge::Restart;
                if !blocks.is_empty() {
                    c.blocks = blocks;
                }
            }
        }
        self.rows.remove(r);
        self.repair_vmerge();
    }

    /// Make the vertical merges consistent: a cell continuing a merge sits under a cell of the
    /// same grid columns that starts or continues it. One that doesn't (left behind by deleting
    /// or moving rows or cells) starts a merge of its own when the cell below continues it, and
    /// otherwise stands alone, so no cell's text is hidden under a merge that isn't there.
    pub fn repair_vmerge(&mut self) {
        for r in 0..self.rows.len() {
            let mut g = 0;
            let mut fix = Vec::new();
            for (i, c) in self.rows.get(r).map(|row| row.cells.as_slice()).unwrap_or_default().iter().enumerate() {
                let span = c.span();
                let below = || self.cell_starting(r + 1, g, span).and_then(|j| self.rows.get(r + 1)?.cells.get(j)).map(|b| b.props.vmerge);
                // A merge of one row is no merge.
                if c.props.vmerge == VMerge::Restart && below() != Some(VMerge::Continue) {
                    fix.push((i, VMerge::None));
                }
                if c.props.vmerge == VMerge::Continue {
                    let above = r
                        .checked_sub(1)
                        .and_then(|k| self.cell_starting(k, g, span).and_then(|j| self.rows.get(k)?.cells.get(j)))
                        .map(|a| a.props.vmerge);
                    if !matches!(above, Some(VMerge::Restart | VMerge::Continue)) {
                        fix.push((i, if below() == Some(VMerge::Continue) { VMerge::Restart } else { VMerge::None }));
                    }
                }
                g += span;
            }
            for (i, vm) in fix {
                if let Some(c) = self.rows.get_mut(r).and_then(|row| row.cells.get_mut(i)) {
                    c.props.vmerge = vm;
                }
            }
        }
    }

    /// The rows the vertical merge through cell `c` of row `r` covers (first, last): just `r`
    /// when the cell isn't merged down.
    pub fn merge_rows(&self, r: usize, c: usize) -> (usize, usize) {
        let g = self.grid_col(r, c);
        let Some(span) = self.rows.get(r).and_then(|row| row.cells.get(c)).map(Cell::span) else { return (r, r) };
        let vm = |k: usize| self.cell_starting(k, g, span).and_then(|j| self.rows.get(k)?.cells.get(j)).map(|x| x.props.vmerge);
        let mut top = r;
        while top > 0 && vm(top) == Some(VMerge::Continue) && matches!(vm(top - 1), Some(VMerge::Restart | VMerge::Continue)) {
            top -= 1;
        }
        let mut bottom = r;
        while vm(bottom).is_some_and(|v| v != VMerge::None) && vm(bottom + 1) == Some(VMerge::Continue) {
            bottom += 1;
        }
        (top, bottom)
    }

    /// Insert a grid column at grid index `g` (cells spanning it widen instead).
    pub fn insert_col(&mut self, g: usize) {
        if self.grid.len() >= MAX_COLS {
            return;
        }
        let g = g.min(self.grid.len());
        let w = self.grid.get(g.min(self.grid.len().saturating_sub(1))).copied().unwrap_or(72.0);
        self.grid.insert(g, w);
        for r in 0..self.rows.len() {
            let mut x = 0;
            let mut idx = None;
            if let Some(row) = self.rows.get(r) {
                for (i, c) in row.cells.iter().enumerate() {
                    if g <= x {
                        idx = Some((i, false));
                        break;
                    }
                    if g < x + c.span() {
                        idx = Some((i, true));
                        break;
                    }
                    x += c.span();
                }
            }
            if let Some(row) = self.rows.get_mut(r) {
                match idx {
                    Some((i, true)) => {
                        if let Some(c) = row.cells.get_mut(i) {
                            // From the clamped span: a `.json` file can carry any `u32` (#3637).
                            c.props.span = (c.span() + 1).min(MAX_COLS) as u32;
                        }
                    }
                    Some((i, false)) => {
                        let mut c = Cell::empty();
                        c.props.width = Some(w);
                        row.cells.insert(i, c);
                    }
                    None => {
                        let mut c = Cell::empty();
                        c.props.width = Some(w);
                        row.cells.push(c);
                    }
                }
            }
        }
    }

    /// Delete grid column `g` (spanning cells shrink).
    pub fn delete_col(&mut self, g: usize) {
        if g >= self.grid.len() || self.grid.len() <= 1 {
            return;
        }
        self.grid.remove(g);
        for row in &mut self.rows {
            let mut x = 0;
            let mut rm = None;
            for (i, c) in row.cells.iter_mut().enumerate() {
                if g < x + c.span() {
                    if c.span() > 1 {
                        c.props.span = (c.span() - 1) as u32;
                    } else {
                        rm = Some(i);
                    }
                    break;
                }
                x += c.span();
            }
            if let Some(i) = rm {
                row.cells.remove(i);
            }
        }
        self.rows.retain(|r| !r.cells.is_empty());
    }

    /// Merge cells `c0..=c1` of row `r` horizontally (content is concatenated).
    pub fn merge_right(&mut self, r: usize, c0: usize, c1: usize) {
        let Some(row) = self.rows.get_mut(r) else { return };
        if c1 <= c0 || c1 >= row.cells.len() {
            return;
        }
        let removed: Vec<Cell> = row.cells.drain(c0 + 1..=c1).collect();
        if let Some(first) = row.cells.get_mut(c0) {
            for c in removed {
                first.props.span = (first.span() + c.span()).min(MAX_COLS) as u32;
                // The merged cell is as wide as the cells it took in.
                first.props.width = match (first.props.width, c.props.width) {
                    (Some(a), Some(b)) => Some(a + b),
                    (a, _) => a,
                };
                let nonempty: Vec<_> = c.blocks.into_iter().filter(|b| b.as_para().is_none_or(|p| !p.is_empty())).collect();
                first.blocks.extend(nonempty);
            }
            if first.blocks.len() > 1 && first.blocks.first().and_then(|b| b.as_para()).is_some_and(Paragraph::is_empty) {
                first.blocks.remove(0);
            }
        }
    }

    /// Merge a rectangle of cells (rows r0..=r1, grid columns g0..=g1).
    pub fn merge(&mut self, r0: usize, r1: usize, g0: usize, g1: usize) {
        for r in r0..=r1.min(self.rows.len().saturating_sub(1)) {
            let (Some(a), Some(b)) = (self.cell_at_grid(r, g0), self.cell_at_grid(r, g1)) else { continue };
            self.merge_right(r, a, b);
            if let Some(c) = self.rows.get_mut(r).and_then(|row| row.cells.get_mut(a)) {
                c.props.vmerge = if r1 == r0 {
                    VMerge::None
                } else if r == r0 {
                    VMerge::Restart
                } else {
                    VMerge::Continue
                };
            }
        }
        // Move content of continued cells into the restart cell.
        if r1 > r0
            && let Some(a) = self.cell_at_grid(r0, g0)
        {
            let mut moved = Vec::new();
            for r in r0 + 1..=r1.min(self.rows.len().saturating_sub(1)) {
                if let Some(c) = self.cell_at_grid(r, g0).and_then(|ci| self.rows.get_mut(r).and_then(|row| row.cells.get_mut(ci))) {
                    let blocks = std::mem::replace(&mut c.blocks, vec![para_block(Paragraph::new())]);
                    moved.extend(blocks.into_iter().filter(|b| b.as_para().is_none_or(|p| !p.is_empty())));
                }
            }
            if let Some(c) = self.rows.get_mut(r0).and_then(|row| row.cells.get_mut(a)) {
                c.blocks.extend(moved);
            }
        }
    }

    /// Split cell `c` of row `r` into `n` cells horizontally. Where the cell's grid lines don't
    /// fall on the equal parts, the grid is refined inside the cell only: the new borders divide it
    /// into equal parts, and the cells of other rows widen over the new grid columns, so their
    /// extent and the table's width stay as they were. Returns the parts' spans, left to right.
    pub fn split_cell(&mut self, r: usize, c: usize, n: usize) -> Vec<usize> {
        let n = n.clamp(1, MAX_COLS);
        let Some(span) = self.rows.get(r).and_then(|row| row.cells.get(c)).map(Cell::span) else { return Vec::new() };
        if n <= 1 {
            return vec![span];
        }
        let g = self.grid_col(r, c);
        let mut end = g.saturating_add(span);
        // A grid narrower than the row (a damaged file) gets the columns the cell needs.
        while self.grid.len() < end && self.grid.len() < MAX_COLS {
            self.grid.push(72.0);
        }
        if self.grid.len() < end {
            return vec![span];
        }
        for w in self.grid.get_mut(g..end).into_iter().flatten() {
            if !(w.is_finite() && *w > 0.0) {
                *w = 72.0;
            }
        }
        const EPS: f32 = 0.01;
        let total: f32 = self.grid.get(g..end).map(|x| x.iter().sum()).unwrap_or(0.0);
        // Grid lines where the cell's parts start (after the first, at `g`).
        let mut lines = Vec::with_capacity(n);
        for k in 1..n {
            let x = total * k as f32 / n as f32;
            let mut acc = 0.0;
            let mut hit = None;
            for j in g..end {
                let w = self.grid.get(j).copied().unwrap_or(0.0);
                if j > g && (x - acc).abs() <= EPS {
                    hit = Some((j, None));
                    break;
                }
                if x < acc + w - EPS {
                    if x > acc + EPS {
                        hit = Some((j, Some((x - acc, w - (x - acc)))));
                    }
                    break;
                }
                acc += w;
            }
            match hit {
                // An existing grid line is where the border goes.
                Some((j, None)) => lines.push(j),
                // A new grid line inside column `j`: every cell over `j` widens over both halves.
                Some((j, Some((left, right)))) if self.grid.len() < MAX_COLS => {
                    if let Some(w) = self.grid.get_mut(j) {
                        *w = left;
                    }
                    self.grid.insert(j + 1, right);
                    for rr in 0..self.rows.len() {
                        if let Some(ci) = self.cell_at_grid(rr, j)
                            && let Some(cell) = self.rows.get_mut(rr).and_then(|row| row.cells.get_mut(ci))
                        {
                            cell.props.span = cell.props.span.max(1).saturating_add(1);
                        }
                    }
                    end += 1;
                    lines.push(j + 1);
                }
                _ => {}
            }
        }
        lines.dedup();
        // The cell now spans `g..end`: cut it at the new lines.
        let grid = self.grid.clone();
        let width = |a: usize, b: usize| grid.get(a..b).map(|x| x.iter().sum::<f32>());
        let Some(row) = self.rows.get_mut(r) else { return Vec::new() };
        let mut a = g;
        let mut parts = Vec::with_capacity(lines.len() + 1);
        for &b in lines.iter().chain(std::iter::once(&end)) {
            if b > a {
                parts.push((a, b));
                a = b;
            }
        }
        for (k, &(a, b)) in parts.iter().enumerate() {
            if k == 0 {
                if let Some(cell) = row.cells.get_mut(c) {
                    cell.props.span = (b - a) as u32;
                    cell.props.width = width(a, b);
                }
            } else {
                let mut nc = Cell::empty();
                nc.props.span = (b - a) as u32;
                nc.props.width = width(a, b);
                let n = row.cells.len();
                row.cells.insert((c + k).min(n), nc);
            }
        }
        parts.iter().map(|&(a, b)| b - a).collect()
    }

    /// The rows cell `c` of row `r` covers: (first row, count). A vertically merged cell runs from
    /// its `Restart` cell down through the `Continue` cells that start at the same grid column.
    pub fn vertical_extent(&self, r: usize, c: usize) -> Option<(usize, usize)> {
        let g = self.grid_col(r, c);
        let at = |rr: usize| -> Option<VMerge> {
            let ci = self.cell_at_grid(rr, g)?;
            if self.grid_col(rr, ci) != g {
                return None;
            }
            Some(self.rows.get(rr)?.cells.get(ci)?.props.vmerge)
        };
        let mut top = r;
        if self.rows.get(r)?.cells.get(c)?.props.vmerge == VMerge::Continue {
            while let Some(up) = top.checked_sub(1).and_then(at) {
                if up == VMerge::None {
                    break;
                }
                top -= 1;
                if up == VMerge::Restart {
                    break;
                }
            }
        }
        let mut k = 1;
        if at(top) != Some(VMerge::None) {
            while at(top + k) == Some(VMerge::Continue) {
                k += 1;
            }
        }
        Some((top, k))
    }

    /// Split Cells: cut cell `c` of row `r` into `cols` × `rows` cells. A cell one row high gets
    /// `rows - 1` new rows under it, over which the row's other cells merge down; a vertically
    /// merged cell is regrouped instead, so `rows` must divide its height. The content stays in
    /// the first part. Returns false (with the table unchanged) when the split can't be made.
    pub fn split_cell_grid(&mut self, r: usize, c: usize, cols: usize, rows: usize) -> bool {
        let (cols, rows) = (cols.clamp(1, MAX_COLS), rows.clamp(1, MAX_ROWS));
        let Some((top, k)) = self.vertical_extent(r, c) else { return false };
        let g = self.grid_col(r, c);
        let Some(ci) = self.cell_at_grid(top, g) else { return false };
        if (k > 1 && k % rows != 0) || (k == 1 && self.rows.len().saturating_add(rows - 1) > MAX_ROWS) {
            return false;
        }
        let height = if k > 1 { k } else { rows };
        if k > 1 && rows > 1 {
            let q = k / rows;
            for i in 0..k {
                if let Some(cj) = self.cell_at_grid(top + i, g)
                    && let Some(cell) = self.rows.get_mut(top + i).and_then(|row| row.cells.get_mut(cj))
                {
                    cell.props.vmerge = if q == 1 {
                        VMerge::None
                    } else if i % q == 0 {
                        VMerge::Restart
                    } else {
                        VMerge::Continue
                    };
                }
            }
        } else if k == 1 && rows > 1 {
            let Some(src) = self.rows.get_mut(top) else { return false };
            let mut like = Row { props: src.props.clone(), cells: Vec::with_capacity(src.cells.len()), controls: ControlWrap::default() };
            like.props.header = false;
            for (i, cell) in src.cells.iter_mut().enumerate() {
                let mut nc = Cell::empty();
                nc.props = cell.props.clone();
                if i == ci {
                    cell.props.vmerge = VMerge::None;
                    nc.props.vmerge = VMerge::None;
                } else {
                    // The row's other cells merge down over the new rows.
                    if cell.props.vmerge == VMerge::None {
                        cell.props.vmerge = VMerge::Restart;
                    }
                    nc.props.vmerge = VMerge::Continue;
                }
                like.cells.push(nc);
            }
            for _ in 1..rows {
                self.rows.insert(top + 1, like.clone().into());
            }
        }
        if cols > 1 {
            let spans = self.split_cell(top, ci, cols);
            let total: usize = spans.iter().sum();
            let vmerge = self.rows.get(top).and_then(|row| row.cells.get(ci)).map(|cell| cell.props.vmerge).unwrap_or_default();
            if let Some(row) = self.rows.get_mut(top) {
                for cell in row.cells.iter_mut().skip(ci + 1).take(spans.len().saturating_sub(1)) {
                    cell.props.vmerge = vmerge;
                }
            }
            // The cells under it, which the new grid lines widened, are cut the same way.
            let grid = self.grid.clone();
            let width = |a: usize, b: usize| grid.get(a..b).map(|x| x.iter().sum::<f32>());
            for rr in top + 1..top + height {
                let Some(cj) = self.cell_at_grid(rr, g).filter(|&cj| self.grid_col(rr, cj) == g) else { continue };
                let Some(row) = self.rows.get_mut(rr) else { continue };
                let Some(cell) = row.cells.get_mut(cj).filter(|cell| cell.span() == total) else { continue };
                let mut a = g;
                let mut parts = Vec::with_capacity(spans.len());
                for (n, &span) in spans.iter().enumerate() {
                    let mut part = if n == 0 { cell.clone() } else { Cell::empty() };
                    part.props.vmerge = cell.props.vmerge;
                    part.props.span = span as u32;
                    part.props.width = width(a, a + span);
                    a += span;
                    parts.push(part);
                }
                row.cells.splice(cj..=cj, parts);
            }
        }
        true
    }

    pub fn total_width(&self) -> f32 {
        self.grid.iter().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_table_shape() {
        let t = Table::new(3, 4, 468.0);
        assert_eq!(t.rows.len(), 3);
        assert_eq!(t.cols(), 4);
        assert_eq!(t.total_width(), 468.0);
        let huge = Table::new(1_000_000, 1000, 100.0);
        assert_eq!(huge.rows.len(), MAX_ROWS);
        assert_eq!(huge.cols(), MAX_COLS);
    }

    #[test]
    fn row_col_ops() {
        let mut t = Table::new(2, 2, 200.0);
        t.insert_row(1, 0);
        assert_eq!(t.rows.len(), 3);
        t.insert_col(1);
        assert_eq!(t.cols(), 3);
        assert!(t.rows.iter().all(|r| r.cells.len() == 3));
        t.delete_col(0);
        assert_eq!(t.cols(), 2);
        t.delete_row(0);
        assert_eq!(t.rows.len(), 2);
        t.delete_row(99);
        t.delete_col(99);
    }

    #[test]
    fn merge_and_split() {
        let mut t = Table::new(2, 3, 300.0);
        t.merge_right(0, 0, 1);
        assert_eq!(t.rows[0].cells.len(), 2);
        assert_eq!(t.rows[0].cells[0].span(), 2);
        assert_eq!(t.cell_at_grid(0, 1), Some(0));
        assert_eq!(t.grid_col(0, 1), 2);
        t.insert_col(1);
        assert_eq!(t.rows[0].cells[0].span(), 3);
        t.split_cell(0, 0, 3);
        assert_eq!(t.rows[0].cells.len(), 4);
        let mut v = Table::new(3, 2, 100.0);
        v.merge(0, 2, 0, 0);
        assert_eq!(v.rows[0].cells[0].props.vmerge, VMerge::Restart);
        assert_eq!(v.rows[2].cells[0].props.vmerge, VMerge::Continue);
    }

    #[test]
    fn row_edits_keep_vertical_merges_valid() {
        let vm = |t: &Table| t.rows.iter().map(|r| r.cells[0].props.vmerge).collect::<Vec<_>>();
        let mut t = Table::new(3, 2, 100.0);
        t.rows[0].cells[0] = Cell::with_text("top");
        t.merge(0, 2, 0, 0);
        // A row inserted inside the merge joins it; one above or below it doesn't.
        let mut inside = t.clone();
        inside.insert_row(1, 0);
        assert_eq!(vm(&inside), [VMerge::Restart, VMerge::Continue, VMerge::Continue, VMerge::Continue]);
        let mut above = t.clone();
        above.insert_row(0, 0);
        assert_eq!(vm(&above), [VMerge::None, VMerge::Restart, VMerge::Continue, VMerge::Continue]);
        let mut below = t.clone();
        below.insert_row(3, 2);
        assert_eq!(vm(&below), [VMerge::Restart, VMerge::Continue, VMerge::Continue, VMerge::None]);
        // Deleting the top row hands the merge and its text to the row below.
        t.delete_row(0);
        assert_eq!(vm(&t), [VMerge::Restart, VMerge::Continue]);
        assert_eq!(t.rows[0].cells[0].blocks[0].as_para().map(Paragraph::plain_text).as_deref(), Some("top"));
        // A continuation with nothing above starts a merge or stands alone.
        let mut orphan = Table::new(3, 1, 100.0);
        orphan.rows[1].cells[0].props.vmerge = VMerge::Continue;
        orphan.rows[2].cells[0].props.vmerge = VMerge::Continue;
        orphan.repair_vmerge();
        assert_eq!(vm(&orphan), [VMerge::None, VMerge::Restart, VMerge::Continue]);
    }

    /// #3637 (upstream 4e692b41): a `.json` file can give a cell any span; inserting a column
    /// through it, merging it or deleting a column under it doesn't overflow, and the span it
    /// gets is in 1..=MAX_COLS.
    #[test]
    fn a_huge_span_from_a_file_does_not_overflow() {
        let huge = || {
            let mut t = Table::new(1, 2, 200.0);
            t.rows[0].cells[0].props.span = u32::MAX;
            t
        };
        // Column 1 is inside the spanning cell, so the cell widens.
        let mut t = huge();
        t.insert_col(1);
        assert_eq!(t.rows[0].cells[0].props.span, MAX_COLS as u32);
        let mut t = huge();
        t.merge_right(0, 0, 1);
        assert_eq!(t.rows[0].cells.len(), 1);
        assert_eq!(t.rows[0].cells[0].props.span, MAX_COLS as u32);
        let mut t = huge();
        t.rows[0].cells[1].props.span = u32::MAX;
        t.merge_right(0, 0, 1);
        assert_eq!(t.rows[0].cells[0].props.span, MAX_COLS as u32);
        let mut t = huge();
        t.delete_col(0);
        assert_eq!(t.rows[0].cells[0].props.span, MAX_COLS as u32 - 1);
        // An ordinary table still adds up.
        let mut t = Table::new(1, 3, 300.0);
        t.merge_right(0, 0, 2);
        assert_eq!(t.rows[0].cells[0].props.span, 3);
        t.insert_col(1);
        assert_eq!(t.rows[0].cells[0].props.span, 4);
        t.delete_col(0);
        assert_eq!(t.rows[0].cells[0].props.span, 3);
    }
}
