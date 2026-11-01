//! Finding a paragraph's pieces inside a table that runs over many pages (`cell_pages`).

use super::*;
use std::sync::Arc;
use wordcraft_doc::props::ParaProps;
use wordcraft_doc::{Block, Paragraph, Table, para_block};

fn para(text: &str) -> Arc<Block> {
    let mut p = Paragraph::with_text(text, Default::default());
    p.props = ParaProps::default();
    para_block(p)
}

fn words(seed: usize, n: usize) -> String {
    const W: &[&str] = &["alder", "brook", "cinder", "dune", "ember", "fjord", "grove", "heron", "inlet", "juniper"];
    (0..n).map(|i| W[(seed * 7 + i * 3 + i / 5) % W.len()]).collect::<Vec<_>>().join(" ")
}

/// The pieces of paragraphs inside a long table are found from the pages that can hold them,
/// not by scanning every page of the table: the same pieces as the index for every body
/// paragraph, with a repeated header row (on every page), cell text split over a page break,
/// and a table nested in a cell.
#[test]
fn pieces_in_a_long_table_match_the_index() {
    let mut d = Document::new();
    d.body = vec![para("Before the table")];
    let mut t = Table::new(300, 3, 468.0);
    for (ri, row) in t.rows.iter_mut().enumerate() {
        for (ci, c) in row.cells.iter_mut().enumerate() {
            let n = if ri % 37 == 5 { 400 } else { 2 + (ri + ci) % 5 };
            c.blocks = vec![para(&words(ri * 3 + ci, n)), para(&words(ri + ci, 2))];
        }
    }
    t.rows[0].props.header = true;
    let mut inner = Table::new(2, 2, 200.0);
    for row in &mut inner.rows {
        for c in row.cells.iter_mut() {
            c.blocks = vec![para(&words(9, 3))];
        }
    }
    t.rows[120].cells[1].blocks.push(Arc::new(Block::Table(inner)));
    d.body.push(Arc::new(Block::Table(t)));
    d.body.push(para("After the table"));
    let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
    assert!(l.pages.len() > 10, "the table runs over many pages: {}", l.pages.len());
    let mut checked = 0;
    for (key, v) in l.index() {
        if key.0 == StoryRef::Body {
            assert_eq!(&l.places(key.0, &key.1), v, "{key:?}");
            checked += 1;
        }
    }
    assert!(checked > 1800, "{checked}");
    // The header row's text is on every page of the table.
    let header = l.places(StoryRef::Body, &Path(vec![1, 0, 0, 0]));
    assert!(header.len() + 1 >= l.pages.len(), "{} of {}", header.len(), l.pages.len());
    // A paragraph that isn't there has no pieces.
    assert!(l.places(StoryRef::Body, &Path(vec![1, 299, 2, 7])).is_empty());
}
