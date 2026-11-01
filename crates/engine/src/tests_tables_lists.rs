//! Tables and lists under random command sequences, and their edge cases: whatever the commands,
//! nothing panics, every table stays a valid grid (cells within the grid, vertical merges that
//! continue a cell above of the same width), every list paragraph names a list that exists, the
//! selection stays inside the document, and undoing everything gives back the document exactly
//! (redoing everything gives back the result).

use serde_json::{Value, json};
use wordcraft_doc::props::VMerge;
use wordcraft_doc::{Block, Blocks, Document, Pos, StoryRef, Table};

use crate::Session;

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
}

/// One random table or list command, as the keyboard, ribbon or mouse would send it.
fn random_command(r: &mut Rng) -> (&'static str, Value) {
    let ext = json!({"extend": r.below(3) == 0});
    match r.below(64) {
        0..=3 => ("text.insert", json!({"text": (["a", "1", "x y", "\t"][r.below(4)])})),
        4 => ("text.newParagraph", json!({})),
        5 => ("text.backspace", json!({})),
        6 => ("text.delete", json!({})),
        7 | 8 => ("text.tab", json!({})),
        9 => ("text.backTab", json!({})),
        10 => (["caret.left", "caret.right", "caret.up", "caret.down"][r.below(4)], ext),
        11 => (["caret.docStart", "caret.docEnd", "caret.home", "caret.end"][r.below(4)], ext),
        12 => ("edit.undo", json!({})),
        13 => ("edit.redo", json!({})),
        14 => ("edit.copy", json!({})),
        15 => ("edit.cut", json!({})),
        16 | 17 => ("edit.paste", json!({})),
        18 => ("select.all", json!({})),
        19 => ("select.paragraph", json!({})),
        20 => ("insert.table", json!({"rows": 1 + r.below(3), "cols": 1 + r.below(3)})),
        21 => ("table.insertRowAbove", json!({})),
        22 => ("table.insertRowBelow", json!({})),
        23 => ("table.insertColumnLeft", json!({})),
        24 => ("table.insertColumnRight", json!({})),
        25 => ("table.deleteRow", json!({})),
        26 => ("table.deleteColumn", json!({})),
        27 => ("table.deleteTable", json!({})),
        28 => ("table.deleteCells", json!({})),
        29 | 30 => ("table.merge", json!({})),
        31 => ("table.split", json!({"columns": 1 + r.below(3)})),
        32 => ("table.splitTable", json!({})),
        33 => ("table.sort", json!({"descending": r.below(2) == 0})),
        34 => ("table.toText", json!({"separator": (["tab", "comma", "paragraph"][r.below(3)])})),
        35 => ("table.fromText", json!({})),
        36 => ("table.autofit", json!({"mode": (["contents", "window", "fixed"][r.below(3)])})),
        37 => (["table.distributeColumns", "table.distributeRows", "table.repeatHeader"][r.below(3)], json!({})),
        38 => (["table.selectTable", "table.selectRow", "table.selectColumn", "table.selectCell"][r.below(4)], json!({})),
        39 => ("table.borders", json!({"kind": (["all", "outside", "inside", "none"][r.below(4)])})),
        40 => ("table.shading", json!({"color": "FFCC00"})),
        41 => ("table.style", json!({"style": (["Table Grid", "Plain Table 1", "Grid Table 4 - Accent 1"][r.below(3)])})),
        42 => ("table.columnWidth", json!({"width": 20 + r.below(200)})),
        43 => ("table.eraser", json!({"page": 0, "x": 72 + r.below(470), "y": 72 + r.below(300)})),
        44 => ("table.draw", json!({"page": 0, "x0": 72 + r.below(470), "y0": 72 + r.below(300), "x1": 72 + r.below(470), "y1": 72 + r.below(300)})),
        45 | 46 => ("para.bullets", json!({})),
        47 | 48 => ("para.numbering", json!({})),
        49 => ("para.multilevel", json!({})),
        50 => ("para.restartNumbering", json!({})),
        51 => ("para.setNumberingValue", json!({"value": r.below(12)})),
        52 => ("para.indent", json!({})),
        53 => ("para.outdent", json!({})),
        54 => ("para.listLevel", json!({"level": r.below(4)})),
        55 => ("list.define", json!({"levels": [{"format": "upperRoman", "text": "%1)"}]})),
        56 => ("para.moveUp", json!({})),
        57 => ("para.moveDown", json!({})),
        58 => ("table.quick", json!({})),
        59 => ("table.formula", json!({})),
        60 => ("text.newParagraph", json!({})),
        61 => ("caret.down", json!({})),
        62 => ("caret.up", json!({})),
        _ => ("table.textDirection", json!({})),
    }
}

/// How deep tables may nest (#1596).
const DEPTH_LIMIT: usize = crate::session::MAX_TABLE_NESTING;

fn check_table(t: &Table, what: &str, depth: usize, doc: &Document) {
    assert!(depth < DEPTH_LIMIT, "{what}: tables nested {depth} deep");
    assert!(!t.rows.is_empty(), "{what}: a table without rows");
    assert!(!t.grid.is_empty(), "{what}: a table without grid columns");
    // Where each row's cells start and how many columns they span, by row.
    let starts: Vec<Vec<(usize, usize, VMerge)>> = t
        .rows
        .iter()
        .map(|row| {
            let mut g = 0;
            row.cells
                .iter()
                .map(|c| {
                    let x = (g, c.span(), c.props.vmerge);
                    g += c.span();
                    x
                })
                .collect()
        })
        .collect();
    for (ri, row) in t.rows.iter().enumerate() {
        assert!(!row.cells.is_empty(), "{what}: row {ri} without cells");
        let width: usize = row.cells.iter().map(|c| c.span()).sum();
        assert!(width <= t.grid.len(), "{what}: row {ri} spans {width} columns of a {}-column grid", t.grid.len());
        for (ci, c) in row.cells.iter().enumerate() {
            assert!(c.props.span >= 1, "{what}: cell {ri},{ci} spans {}", c.props.span);
            assert!(!c.blocks.is_empty(), "{what}: cell {ri},{ci} without paragraphs");
            check_blocks(&c.blocks, what, depth + 1, doc);
        }
        // A cell continuing a vertical merge sits under a cell of the same columns that starts
        // or continues it.
        for &(g, span, vm) in starts.get(ri).into_iter().flatten() {
            if vm != VMerge::Continue {
                continue;
            }
            let above = ri.checked_sub(1).and_then(|k| starts.get(k)).and_then(|cells| cells.iter().find(|(ag, _, _)| *ag == g));
            assert!(
                above.is_some_and(|&(_, aspan, avm)| aspan == span && avm != VMerge::None),
                "{what}: row {ri} column {g} continues a vertical merge that doesn't start above it ({above:?})"
            );
        }
    }
}

/// Paragraphs well formed, list references valid, tables valid.
fn check_blocks(blocks: &Blocks, what: &str, depth: usize, doc: &Document) {
    for b in blocks.iter() {
        match &**b {
            Block::Para(p) => {
                let total: usize = p.runs.iter().map(|r| r.len).sum();
                assert_eq!(total, p.text.len(), "{what}: runs don't cover {:?}", p.text);
                if let Some(n) = p.props.numbering.filter(|n| n.num != 0) {
                    assert!(doc.numbering.num(n.num).is_some(), "{what}: paragraph {:?} in list {} that doesn't exist", p.text, n.num);
                    assert!(doc.numbering.abstract_of(n.num).is_some(), "{what}: list {} without a definition", n.num);
                    assert!(n.level <= 8, "{what}: list level {}", n.level);
                }
            }
            Block::Table(t) => check_table(t, what, depth, doc),
        }
    }
}

fn check_pos(s: &Session, p: &Pos, what: &str) {
    let para = s.doc.para_at(p).unwrap_or_else(|| panic!("{what}: {p:?} isn't in a paragraph"));
    assert!(p.off <= para.len() && para.text.is_char_boundary(p.off), "{what}: {p:?} isn't a boundary of {:?}", para.text);
    // Never in a cell a vertical merge covers: its text isn't shown, so typing there is invisible.
    let mut i = 0;
    while let (Some(_), Some(r), Some(c)) = (p.path.0.get(i), p.path.0.get(i + 1), p.path.0.get(i + 2)) {
        if p.path.0.len() <= i + 3 {
            break;
        }
        let tp = wordcraft_doc::Path(p.path.0[..=i].to_vec());
        let vm = s.doc.table(p.story, &tp).and_then(|t| t.rows.get(*r as usize)?.cells.get(*c as usize)).map(|c| c.props.vmerge);
        assert_ne!(vm, Some(VMerge::Continue), "{what}: {p:?} is in a cell covered by a vertical merge");
        i += 3;
    }
}

fn check(s: &Session, what: &str) {
    check_blocks(&s.doc.body, what, 0, &s.doc);
    // A table never ends the document: Word always keeps a paragraph after it.
    assert!(s.doc.body.last().is_some_and(|b| b.as_para().is_some()), "{what}: the document ends with a table");
    check_pos(s, &s.sel.anchor, what);
    check_pos(s, &s.sel.focus, what);
}

fn serialized(d: &Document) -> String {
    serde_json::to_string(d).unwrap()
}

fn run(s: &mut Session, id: &str, v: Value) -> Value {
    s.run(id, &v).unwrap_or_else(|e| panic!("{id} {v}: {e}"))
}

fn typed(s: &mut Session, text: &str) {
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            run(s, "text.newParagraph", json!({}));
        }
        if !line.is_empty() {
            run(s, "text.insert", json!({"text": line}));
        }
    }
}

/// A start: an empty document, a numbered list then a table, a table alone, or a table between
/// two list paragraphs.
fn start(kind: usize) -> Session {
    let mut s = Session::new(Document::new());
    match kind {
        0 => {}
        1 => {
            typed(&mut s, "one\ntwo\nthree");
            run(&mut s, "select.all", json!({}));
            run(&mut s, "para.numbering", json!({}));
            run(&mut s, "caret.docEnd", json!({}));
            run(&mut s, "text.newParagraph", json!({}));
            run(&mut s, "insert.table", json!({"rows": 3, "cols": 3}));
            typed(&mut s, "c");
        }
        2 => {
            run(&mut s, "insert.table", json!({"rows": 3, "cols": 2}));
            for t in ["b", "x", "a", "y", "c", "z"] {
                typed(&mut s, t);
                run(&mut s, "text.tab", json!({}));
            }
        }
        _ => {
            typed(&mut s, "first\n");
            run(&mut s, "insert.table", json!({"rows": 2, "cols": 2}));
            typed(&mut s, "in");
            run(&mut s, "caret.docEnd", json!({}));
            typed(&mut s, "last");
            run(&mut s, "select.all", json!({}));
            run(&mut s, "para.bullets", json!({}));
        }
    }
    run(&mut s, "caret.docStart", json!({}));
    s.reset_history();
    s
}

#[test]
fn random_table_and_list_commands_keep_the_document_valid_and_undo_redo_exact() {
    let seeds: u64 = std::env::var("WC_FUZZ_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(48);
    let steps: usize = std::env::var("WC_FUZZ_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(50);
    let first: u64 = std::env::var("WC_FUZZ_FIRST").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    for seed in first..first + seeds {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut s = start(r.below(4));
        check(&s, &format!("seed {seed} start"));
        let original = s.doc.clone();
        let mut log = Vec::new();
        for i in 0..steps {
            let (id, v) = random_command(&mut r);
            log.push(format!("{id} {v}"));
            let res = s.run(id, &v);
            if let Err(e) = &res {
                assert!(!e.to_string().contains("panicked"), "seed {seed} step {i}: {e}\n{}", log.join("\n"));
            }
            check(&s, &format!("seed {seed} step {i} after {id} {v}\n{}", log.join("\n")));
            if i % 7 == 6 {
                same_labels_as_fresh(&mut s, &format!("seed {seed} step {i}\n{}", log.join("\n")));
            }
        }
        let end = s.doc.clone();
        let mut undone = 0;
        while s.can_undo() {
            s.run("edit.undo", &json!({})).unwrap();
            check(&s, &format!("seed {seed} undo {undone}\n{}", log.join("\n")));
            undone += 1;
        }
        assert_eq!(serialized(&s.doc), serialized(&original), "seed {seed}: undoing everything\n{}", log.join("\n"));
        same_labels_as_fresh(&mut s, &format!("seed {seed} after undoing everything\n{}", log.join("\n")));
        for _ in 0..undone {
            s.run("edit.redo", &json!({})).unwrap();
            check(&s, &format!("seed {seed} redo\n{}", log.join("\n")));
        }
        assert_eq!(serialized(&s.doc), serialized(&end), "seed {seed}: redoing everything\n{}", log.join("\n"));
    }
}

/// The body's first table.
fn table(s: &Session) -> Table {
    s.doc.body.iter().find_map(|b| if let Block::Table(t) = &**b { Some(t.clone()) } else { None }).unwrap()
}

fn caret_in_cell(s: &mut Session, t: u32, r: u32, c: u32) {
    let pos = Pos { story: StoryRef::Body, path: wordcraft_doc::Path(vec![t, r, c, 0]), off: 0 };
    run(s, "caret.set", json!({"pos": crate::cmd::pos_json(&pos)}));
}

/// The list labels on the pages, in order.
fn labels(s: &mut Session) -> Vec<String> {
    use wordcraft_layout::Placed;
    let l = s.layout();
    l.pages
        .iter()
        .flat_map(|p| &p.items)
        .filter_map(|i| if let Placed::Lines { para, l0: 0, .. } = i { para.label.as_ref().map(|lb| lb.text.clone()) } else { None })
        .collect()
}

/// The session's list labels (laid out incrementally) are those of laying the document out afresh.
fn same_labels_as_fresh(s: &mut Session, what: &str) {
    let got = labels(s);
    let mut fresh = Session::new(s.doc.clone());
    assert_eq!(got, labels(&mut fresh), "{what}: list labels differ from a fresh layout");
}

/// A document holding just a `rows × cols` table (and the paragraph after it), each cell's
/// text its row and column ("00", "01", …).
fn grid_doc(rows: usize, cols: usize) -> Session {
    let mut s = Session::new(Document::new());
    run(&mut s, "insert.table", json!({"rows": rows, "cols": cols}));
    for r in 0..rows {
        for c in 0..cols {
            caret_in_cell(&mut s, 0, r as u32, c as u32);
            typed(&mut s, &format!("{r}{c}"));
        }
    }
    s.reset_history();
    s
}

fn select_cells(s: &mut Session, rows: (usize, usize), cols: (usize, usize)) {
    crate::cmd::table::select_range(s, StoryRef::Body, &wordcraft_doc::Path(vec![0]), rows, cols).unwrap();
}

fn cell_text(t: &Table, r: usize, c: usize) -> String {
    crate::cmd::table::cell_text(&t.rows[r].cells[c])
}

fn vmerges(t: &Table, c: usize) -> Vec<VMerge> {
    t.rows.iter().map(|r| r.cells.get(c).map_or(VMerge::None, |x| x.props.vmerge)).collect()
}

/// A merged cell down column 0 of a 3 × 2 table, holding "00", "10" and "20".
fn merged_column() -> Session {
    let mut s = grid_doc(3, 2);
    select_cells(&mut s, (0, 2), (0, 0));
    run(&mut s, "table.merge", json!({}));
    let t = table(&s);
    assert_eq!(vmerges(&t, 0), [VMerge::Restart, VMerge::Continue, VMerge::Continue]);
    assert_eq!(cell_text(&t, 0, 0), "00\n10\n20");
    s.reset_history();
    s
}

/// Undo takes a command back exactly, and Redo brings it back.
fn undo_redo_exact(s: &mut Session, before: &Document) {
    let after = s.doc.clone();
    run(s, "edit.undo", json!({}));
    assert_eq!(serialized(&s.doc), serialized(before));
    run(s, "edit.redo", json!({}));
    assert_eq!(serialized(&s.doc), serialized(&after));
}

/// Deleting the row a vertically merged cell starts in leaves the rest of the merge merged,
/// with its text, instead of cells that continue a merge nothing starts (which hid their text).
#[test]
fn deleting_the_top_row_of_a_merged_cell_keeps_the_rest_merged_with_its_text() {
    let mut s = merged_column();
    let before = s.doc.clone();
    caret_in_cell(&mut s, 0, 0, 1);
    run(&mut s, "table.deleteRow", json!({}));
    check(&s, "after Delete Rows");
    let t = table(&s);
    assert_eq!(t.rows.len(), 2);
    assert_eq!(vmerges(&t, 0), [VMerge::Restart, VMerge::Continue]);
    assert_eq!(cell_text(&t, 0, 0), "00\n10\n20");
    assert_eq!(cell_text(&t, 0, 1), "11");
    undo_redo_exact(&mut s, &before);
    // Down to its last row: a cell of its own again.
    caret_in_cell(&mut s, 0, 0, 1);
    run(&mut s, "table.deleteRow", json!({}));
    check(&s, "after the second Delete Rows");
    assert_eq!(vmerges(&table(&s), 0), [VMerge::None]);
}

/// Insert Below in the first row of a merged cell inserts the row inside the merge; Insert
/// Above it and Insert Below its last row insert rows of their own, leaving the merge whole.
#[test]
fn inserting_rows_around_and_inside_a_merged_cell_keeps_it_whole() {
    let mut s = merged_column();
    let before = s.doc.clone();
    caret_in_cell(&mut s, 0, 0, 1);
    run(&mut s, "table.insertRowBelow", json!({}));
    check(&s, "after Insert Below in the merge's first row");
    assert_eq!(vmerges(&table(&s), 0), [VMerge::Restart, VMerge::Continue, VMerge::Continue, VMerge::Continue]);
    undo_redo_exact(&mut s, &before);
    run(&mut s, "edit.undo", json!({}));
    caret_in_cell(&mut s, 0, 0, 1);
    run(&mut s, "table.insertRowAbove", json!({}));
    check(&s, "after Insert Above");
    assert_eq!(vmerges(&table(&s), 0), [VMerge::None, VMerge::Restart, VMerge::Continue, VMerge::Continue]);
    run(&mut s, "edit.undo", json!({}));
    caret_in_cell(&mut s, 0, 2, 1);
    run(&mut s, "table.insertRowBelow", json!({}));
    check(&s, "after Insert Below the merge");
    assert_eq!(vmerges(&table(&s), 0), [VMerge::Restart, VMerge::Continue, VMerge::Continue, VMerge::None]);
}

/// Tab and Shift+Tab pass over the cells a vertical merge covers (their text isn't shown, so
/// typing there was invisible); Tab in the last cell adds a row.
#[test]
fn tab_and_shift_tab_skip_cells_a_merge_covers() {
    let mut s = merged_column();
    caret_in_cell(&mut s, 0, 0, 1);
    run(&mut s, "text.tab", json!({}));
    assert_eq!(s.sel.focus.path.cell().map(|x| (x.1, x.2)), Some((1, 1)), "past the covered cell of row 1");
    run(&mut s, "text.tab", json!({}));
    assert_eq!(s.sel.focus.path.cell().map(|x| (x.1, x.2)), Some((2, 1)));
    run(&mut s, "text.backTab", json!({}));
    assert_eq!(s.sel.focus.path.cell().map(|x| (x.1, x.2)), Some((1, 1)));
    run(&mut s, "text.backTab", json!({}));
    assert_eq!(s.sel.focus.path.cell().map(|x| (x.1, x.2)), Some((0, 1)), "past the covered cell of row 1");
    caret_in_cell(&mut s, 0, 2, 1);
    run(&mut s, "text.tab", json!({}));
    check(&s, "after Tab in the last cell");
    let t = table(&s);
    assert_eq!(t.rows.len(), 4);
    assert_eq!(vmerges(&t, 0), [VMerge::Restart, VMerge::Continue, VMerge::Continue, VMerge::None], "the new row stands alone");
    assert_eq!(s.sel.focus.path.cell().map(|x| (x.1, x.2)), Some((3, 0)));
}

/// Table commands that leave the caret in a row (Delete Columns, Delete Cells, Split Table…)
/// never leave it in a cell a merge covers.
#[test]
fn the_caret_never_lands_in_a_covered_cell() {
    let mut s = merged_column();
    caret_in_cell(&mut s, 0, 1, 1);
    run(&mut s, "table.deleteColumn", json!({}));
    check(&s, "after Delete Columns");
    assert_eq!(s.sel.focus.path.cell().map(|x| (x.1, x.2)), Some((0, 0)));
}

/// Split Table in the middle of a merged cell: each table gets a whole cell.
#[test]
fn split_table_through_a_merged_cell_gives_each_part_a_whole_cell() {
    let mut s = merged_column();
    let before = s.doc.clone();
    caret_in_cell(&mut s, 0, 1, 1);
    run(&mut s, "table.splitTable", json!({}));
    check(&s, "after Split Table");
    let Block::Table(second) = &*s.doc.body[2] else { panic!("no second table") };
    assert_eq!(vmerges(second, 0), [VMerge::Restart, VMerge::Continue]);
    undo_redo_exact(&mut s, &before);
}

/// Sorting would move a merged cell's rows apart: refused, nothing changes.
#[test]
fn sort_refuses_a_table_with_vertically_merged_cells() {
    let mut s = merged_column();
    let before = s.doc.clone();
    caret_in_cell(&mut s, 0, 0, 1);
    assert!(s.run("table.sort", &json!({"column": 1, "descending": true})).is_err());
    assert_eq!(serialized(&s.doc), serialized(&before));
}

/// Delete Cells shifting a merged cell's neighbours left: no cell is left continuing a merge
/// that isn't above it.
#[test]
fn delete_cells_beside_a_merged_cell_keeps_the_merges_valid() {
    let mut s = grid_doc(2, 3);
    select_cells(&mut s, (0, 1), (2, 2));
    run(&mut s, "table.merge", json!({}));
    caret_in_cell(&mut s, 0, 0, 0);
    run(&mut s, "table.deleteCells", json!({}));
    check(&s, "after Delete Cells");
}

/// Merge Cells joins the rectangle of grid columns the selected cells cover, even when the rows
/// split those columns differently (a cell index means a different column in each row).
#[test]
fn merge_uses_grid_columns_not_cell_indexes() {
    let mut s = grid_doc(2, 3);
    // Row 0: "00 01" (two columns) | "02"; row 1: three cells.
    select_cells(&mut s, (0, 0), (0, 1));
    run(&mut s, "table.merge", json!({}));
    // Grid columns 1–2 of both rows: row 0's first cell reaches into column 1, so all of it.
    select_cells(&mut s, (0, 1), (1, 2));
    let before = s.doc.clone();
    run(&mut s, "table.merge", json!({}));
    check(&s, "after merging");
    let t = table(&s);
    assert_eq!((t.rows[0].cells.len(), t.rows[1].cells.len()), (1, 1), "{:?}", t.rows);
    assert_eq!(vmerges(&t, 0), [VMerge::Restart, VMerge::Continue]);
    let text = cell_text(&t, 0, 0);
    for x in ["00", "01", "02", "10", "11", "12"] {
        assert!(text.contains(x), "{x} kept in {text:?}");
    }
    undo_redo_exact(&mut s, &before);
}

/// Split Cells on a vertically merged cell splits it down its whole height.
#[test]
fn split_a_merged_cell_into_columns_splits_every_row_of_it() {
    let mut s = merged_column();
    let before = s.doc.clone();
    caret_in_cell(&mut s, 0, 0, 0);
    run(&mut s, "table.split", json!({"columns": 2}));
    check(&s, "after Split Cells");
    let t = table(&s);
    for r in 0..3 {
        assert_eq!(t.rows[r].cells.len(), 3, "row {r}: {:?}", t.rows[r]);
    }
    assert_eq!(vmerges(&t, 0), [VMerge::Restart, VMerge::Continue, VMerge::Continue]);
    assert_eq!(vmerges(&t, 1), [VMerge::Restart, VMerge::Continue, VMerge::Continue]);
    assert_eq!(cell_text(&t, 0, 0), "00\n10\n20");
    undo_redo_exact(&mut s, &before);
}

/// Copy with whole cells selected copies a table (#1744): the whole table, or the rows and
/// columns selected; pasting it below the table adds a table, pasting it into a cell nests it.
#[test]
fn copying_cells_copies_a_table() {
    let mut s = grid_doc(3, 3);
    caret_in_cell(&mut s, 0, 0, 0);
    run(&mut s, "table.selectTable", json!({}));
    run(&mut s, "edit.copy", json!({}));
    run(&mut s, "caret.docEnd", json!({}));
    let before = s.doc.clone();
    run(&mut s, "edit.paste", json!({}));
    check(&s, "after pasting the table");
    let tables: Vec<&Table> = s.doc.body.iter().filter_map(|b| if let Block::Table(t) = &**b { Some(t) } else { None }).collect();
    assert_eq!(tables.len(), 2, "the copy is a table");
    assert_eq!(serde_json::to_value(tables[1]).unwrap(), serde_json::to_value(tables[0]).unwrap());
    undo_redo_exact(&mut s, &before);

    // Rows 1–2 of columns 1–2.
    select_cells(&mut s, (1, 2), (1, 2));
    run(&mut s, "edit.copy", json!({}));
    assert_eq!(s.clipboard_text.trim_end(), "11\t12\n21\t22");
    caret_in_cell(&mut s, 0, 0, 0);
    run(&mut s, "edit.paste", json!({}));
    check(&s, "after pasting cells into a cell");
    let t = table(&s);
    let Some(Block::Table(nested)) = t.rows[0].cells[0].blocks.iter().map(|b| &**b).find(|b| matches!(b, Block::Table(_))) else {
        panic!("no nested table")
    };
    assert_eq!((nested.rows.len(), nested.grid.len()), (2, 2));
    assert_eq!(cell_text(nested, 1, 1), "22");
}

/// #1844: Backspace on a selected table deletes it, on selected rows or columns deletes them;
/// Delete empties them; some cells of a row are only emptied by either.
#[test]
fn backspace_deletes_a_selected_table_rows_or_columns_and_delete_empties_them() {
    let tables = |s: &Session| s.doc.body.iter().filter(|b| matches!(&***b, Block::Table(_))).count();
    let mut s = grid_doc(3, 3);
    caret_in_cell(&mut s, 0, 0, 0);
    run(&mut s, "table.selectTable", json!({}));
    run(&mut s, "text.delete", json!({}));
    check(&s, "after Delete");
    assert_eq!(tables(&s), 1, "Delete keeps the table");
    assert!(s.doc.plain_text(StoryRef::Body).trim().is_empty(), "and empties it");
    run(&mut s, "edit.undo", json!({}));
    let before = s.doc.clone();
    caret_in_cell(&mut s, 0, 0, 0);
    run(&mut s, "table.selectTable", json!({}));
    run(&mut s, "text.backspace", json!({}));
    check(&s, "after Backspace on the table");
    assert_eq!(tables(&s), 0, "Backspace deletes the table");
    undo_redo_exact(&mut s, &before);
    run(&mut s, "edit.undo", json!({}));
    // Rows 1–2.
    select_cells(&mut s, (1, 2), (0, 2));
    run(&mut s, "text.backspace", json!({}));
    check(&s, "after Backspace on rows");
    assert_eq!(table(&s).rows.len(), 1);
    run(&mut s, "edit.undo", json!({}));
    // Column 1.
    select_cells(&mut s, (0, 2), (1, 1));
    run(&mut s, "text.backspace", json!({}));
    check(&s, "after Backspace on a column");
    assert_eq!(table(&s).grid.len(), 2);
    assert_eq!(cell_text(&table(&s), 0, 1), "02");
    run(&mut s, "edit.undo", json!({}));
    // Two cells of a row: emptied, the table keeps its shape.
    select_cells(&mut s, (0, 0), (0, 1));
    run(&mut s, "text.backspace", json!({}));
    check(&s, "after Backspace on two cells");
    let t = table(&s);
    assert_eq!((t.rows.len(), t.grid.len()), (3, 3));
    assert_eq!(cell_text(&t, 0, 2), "02");
}

/// A copy of rows starting inside a merged cell is still a valid table.
#[test]
fn copying_rows_from_inside_a_merged_cell_makes_a_valid_table() {
    let mut s = merged_column();
    select_cells(&mut s, (1, 2), (0, 1));
    run(&mut s, "edit.copy", json!({}));
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "edit.paste", json!({}));
    check(&s, "after pasting");
}

/// Convert to Text keeps the cells' formatting and a nested table (it used to keep plain
/// text only, dropping both).
#[test]
fn convert_to_text_keeps_formatting_and_nested_tables() {
    let mut s = grid_doc(2, 2);
    run(&mut s, "select.text", json!({"text": "01"}));
    run(&mut s, "format.bold", json!({"value": true}));
    caret_in_cell(&mut s, 0, 1, 1);
    run(&mut s, "caret.end", json!({}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "insert.table", json!({"rows": 1, "cols": 2}));
    typed(&mut s, "deep");
    caret_in_cell(&mut s, 0, 0, 0);
    let before = s.doc.clone();
    run(&mut s, "table.toText", json!({}));
    check(&s, "after Convert to Text");
    let texts: Vec<String> = s.doc.body.iter().filter_map(|b| b.as_para().map(|p| p.plain_text())).collect();
    assert_eq!(texts[..2], ["00\t01".to_string(), "10\t11".to_string()]);
    let p = s.doc.body[0].as_para().unwrap();
    assert_eq!(p.props_of_char(3).bold, Some(true), "01 stays bold");
    assert_eq!(p.props_of_char(0).bold, None);
    assert!(s.doc.body.iter().any(|b| matches!(&**b, Block::Table(t) if crate::cmd::table::cell_text(&t.rows[0].cells[0]) == "deep")));
    undo_redo_exact(&mut s, &before);
}

/// Convert Text to Table keeps the text's formatting in the cells.
#[test]
fn convert_text_to_table_keeps_formatting() {
    let mut s = Session::new(Document::new());
    typed(&mut s, "plain\tbold\nx\ty");
    run(&mut s, "select.text", json!({"text": "bold"}));
    run(&mut s, "format.bold", json!({"value": true}));
    run(&mut s, "select.all", json!({}));
    run(&mut s, "table.fromText", json!({}));
    check(&s, "after Convert Text to Table");
    let t = table(&s);
    assert_eq!((cell_text(&t, 0, 0), cell_text(&t, 0, 1), cell_text(&t, 1, 1)), ("plain".into(), "bold".into(), "y".into()));
    let p = t.rows[0].cells[1].blocks[0].as_para().unwrap();
    assert_eq!(p.props_of_char(0).bold, Some(true));
}

/// A table as the whole document: commands at its edges keep a paragraph after it, and deleting
/// it leaves one empty paragraph.
#[test]
fn a_table_as_the_only_content() {
    let mut s = grid_doc(1, 1);
    caret_in_cell(&mut s, 0, 0, 0);
    for id in [
        "text.backspace",
        "caret.docStart",
        "text.backTab",
        "table.insertRowAbove",
        "table.deleteRow",
        "table.insertColumnLeft",
        "table.deleteColumn",
    ] {
        run(&mut s, id, json!({}));
        check(&s, id);
    }
    run(&mut s, "table.deleteTable", json!({}));
    check(&s, "after Delete Table");
    assert_eq!(s.doc.body.len(), 1);
    run(&mut s, "insert.table", json!({"rows": 1, "cols": 1}));
    run(&mut s, "table.toText", json!({}));
    check(&s, "after Convert to Text");
}

/// A numbered list goes on after a table between its items (and inside the table's cells).
#[test]
fn numbering_continues_across_a_table() {
    let mut s = Session::new(Document::new());
    typed(&mut s, "one\ntwo\nthree");
    run(&mut s, "select.all", json!({}));
    run(&mut s, "para.numbering", json!({}));
    // A table between "two" and "three".
    run(&mut s, "caret.set", json!({"pos": crate::cmd::pos_json(&Pos::body(1, 3))}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "para.numbering", json!({"off": true}));
    run(&mut s, "insert.table", json!({"rows": 1, "cols": 1}));
    // The empty paragraph the table was inserted in follows it: "three" comes right after.
    s.doc.remove_block(StoryRef::Body, &wordcraft_doc::Path::top(3)).unwrap();
    let blocks: Vec<bool> = s.doc.body.iter().map(|b| b.as_para().is_some()).collect();
    assert_eq!(blocks, [true, true, false, true], "one, two, the table, three");
    assert_eq!(labels(&mut s), ["1.", "2.", "3."]);
    // Numbering "three" again (off, then on) goes on from the list before the table.
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "para.numbering", json!({}));
    assert_eq!(labels(&mut s), ["1.", "2."]);
    run(&mut s, "para.numbering", json!({}));
    assert_eq!(labels(&mut s), ["1.", "2.", "3."]);
    // An item in the table counts in its place.
    caret_in_cell(&mut s, 2, 0, 0);
    typed(&mut s, "cell");
    run(&mut s, "para.numbering", json!({}));
    assert_eq!(labels(&mut s), ["1.", "2.", "3.", "4."]);
    same_labels_as_fresh(&mut s, "after numbering the cell");
}

/// Labels follow Undo and Redo: taking an item out of the list renumbers the rest, and Undo
/// numbers them back.
#[test]
fn list_numbering_after_undo() {
    let mut s = Session::new(Document::new());
    typed(&mut s, "a\nb\nc\nd");
    run(&mut s, "select.all", json!({}));
    run(&mut s, "para.numbering", json!({}));
    run(&mut s, "caret.docStart", json!({}));
    assert_eq!(labels(&mut s), ["1.", "2.", "3.", "4."]);
    run(&mut s, "para.numbering", json!({"off": true}));
    assert_eq!(labels(&mut s), ["1.", "2.", "3."]);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(labels(&mut s), ["1.", "2.", "3.", "4."]);
    // Restart at item 3, then undo.
    run(&mut s, "caret.set", json!({"pos": crate::cmd::pos_json(&Pos::body(2, 0))}));
    run(&mut s, "para.restartNumbering", json!({}));
    assert_eq!(labels(&mut s), ["1.", "2.", "1.", "2."]);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(labels(&mut s), ["1.", "2.", "3.", "4."]);
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(labels(&mut s), ["1.", "2.", "1.", "2."]);
    // Delete the first item's paragraph: the rest renumber; Undo brings it back.
    run(&mut s, "caret.docStart", json!({}));
    run(&mut s, "select.paragraph", json!({}));
    run(&mut s, "text.delete", json!({}));
    same_labels_as_fresh(&mut s, "after deleting an item");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(labels(&mut s), ["1.", "2.", "1.", "2."]);
    same_labels_as_fresh(&mut s, "after undo");
}

/// #2143: Enter on an empty nested item moves it up a level, numbered on from the level above.
#[test]
fn enter_on_an_empty_nested_item_moves_it_up_and_numbers_on() {
    let mut s = Session::new(Document::new());
    // Typed key by key, so "1. " starts the list.
    for c in "1. Lorem".chars() {
        run(&mut s, "text.insert", json!({"text": c.to_string()}));
    }
    typed(&mut s, "\nIpsum\n");
    run(&mut s, "text.tab", json!({}));
    typed(&mut s, "Latin\nNonsense\n");
    assert_eq!(labels(&mut s), ["1.", "2.", "a.", "b.", "c."]);
    run(&mut s, "text.newParagraph", json!({}));
    assert_eq!(labels(&mut s), ["1.", "2.", "a.", "b.", "3."]);
    typed(&mut s, "Dolor");
    assert_eq!(s.doc.para_at(&s.sel.focus).unwrap().props.numbering.map(|n| n.level), Some(0));
}

/// #2015: copied list items carry their numbers or bullets into the plain text, as Word does
/// (`1.<tab>Item`); an item copied from part-way through doesn't.
#[test]
fn copied_list_items_keep_their_numbers_in_plain_text() {
    let mut s = Session::new(Document::new());
    typed(&mut s, "Item 1\nItem 2");
    run(&mut s, "select.all", json!({}));
    run(&mut s, "para.numbering", json!({}));
    run(&mut s, "edit.copy", json!({}));
    assert_eq!(s.clipboard_text, "1.\tItem 1\n2.\tItem 2");
    run(&mut s, "para.bullets", json!({}));
    run(&mut s, "edit.copy", json!({}));
    assert_eq!(s.clipboard_text, "•\tItem 1\n•\tItem 2");
    run(&mut s, "select.range", json!({"anchor": crate::cmd::pos_json(&Pos::body(0, 2)), "focus": crate::cmd::pos_json(&Pos::body(1, 6))}));
    run(&mut s, "edit.copy", json!({}));
    assert_eq!(s.clipboard_text, "em 1\n•\tItem 2");
    // Pasting our own copy still pastes the list, not the text with its numbers.
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "edit.paste", json!({}));
    assert!(!s.doc.plain_text(StoryRef::Body).contains('•'));
    // Plain paragraphs copy as before.
    let mut p = Session::new(Document::new());
    typed(&mut p, "Plain\ntext");
    run(&mut p, "select.all", json!({}));
    run(&mut p, "edit.copy", json!({}));
    assert_eq!(p.clipboard_text, "Plain\ntext");
}

/// Set Numbering Value and Restart at 1 on a list only change that list.
#[test]
fn set_numbering_value_renumbers_from_the_item_on() {
    let mut s = Session::new(Document::new());
    typed(&mut s, "a\nb\nc");
    run(&mut s, "select.all", json!({}));
    run(&mut s, "para.numbering", json!({}));
    run(&mut s, "caret.set", json!({"pos": crate::cmd::pos_json(&Pos::body(1, 0))}));
    let before = s.doc.clone();
    run(&mut s, "para.setNumberingValue", json!({"value": 7}));
    assert_eq!(labels(&mut s), ["1.", "7.", "8."]);
    undo_redo_exact(&mut s, &before);
    check(&s, "after Set Numbering Value");
}
