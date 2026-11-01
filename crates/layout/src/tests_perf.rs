//! Relayout timings on large synthetic documents (`#[ignore]`d; run with
//! `cargo test --release -p wordcraft-layout perf_ -- --ignored --nocapture`).

use super::*;
use std::sync::Arc;
use wordcraft_doc::props::ParaProps;
use wordcraft_doc::{Block, Paragraph, Table};

const LINE: &str = "The quick brown fox jumps over the lazy dog while the band plays on. ";

fn para(text: &str, props: ParaProps) -> Arc<Block> {
    let mut p = Paragraph::with_text(text, Default::default());
    p.props = props;
    wordcraft_doc::para_block(p)
}

/// `n` paragraphs of plain text, a Heading 1 before every `heading_every`th (0: none), and a
/// 4×4 table after every `table_every`th (0: none).
pub(crate) fn synthetic(n: usize, heading_every: usize, table_every: usize) -> Document {
    let mut d = Document::new();
    d.body.clear();
    for i in 0..n {
        if heading_every > 0 && i % heading_every == 0 {
            d.body.push(para(&format!("Heading {i}"), ParaProps { style: Some("Heading1".into()), ..Default::default() }));
        }
        d.body.push(para(&LINE.repeat(4), ParaProps::default()));
        if table_every > 0 && i % table_every == 0 {
            let mut t = Table::new(4, 4, 468.0);
            for row in &mut t.rows {
                for c in &mut row.cells {
                    c.blocks = vec![para("cell text here", ParaProps::default())];
                }
            }
            d.body.push(Arc::new(Block::Table(t)));
        }
    }
    d
}

/// Type a character into body block `at` (a table: into its middle row's first cell).
fn edit(d: &mut Document, at: usize) {
    let Some(b) = d.body.get_mut(at) else { return };
    let p = match Arc::make_mut(b) {
        Block::Para(p) => p,
        Block::Table(t) => {
            let mid = t.rows.len() / 2;
            let Some(Block::Para(p)) = t.rows.get_mut(mid).and_then(|r| r.cells.first_mut()).and_then(|c| c.blocks.first_mut()).map(Arc::make_mut)
            else {
                return;
            };
            p
        }
    };
    let _ = p.insert_text(0, "a", &Default::default());
    p.touch();
}

/// Best of 5 relayouts after typing into block `at`: from scratch (`layout`, with the
/// paragraph cache) or incremental (`layout_after` the last layout).
fn best_relayout(d: &mut Document, cache: &mut LayoutCache, at: usize, incremental: bool) -> f64 {
    let opts = LayoutOptions::default();
    let mut prev = layout(d, cache, &opts);
    let mut best = std::time::Duration::MAX;
    // The versions before each edit stay alive, as the undo history keeps them in the editor.
    let mut history = Vec::new();
    for _ in 0..5 {
        history.push(d.clone());
        edit(d, at);
        let t = std::time::Instant::now();
        prev = if incremental { layout_after(d, cache, &opts, Some(&prev)) } else { layout(d, cache, &opts) };
        best = best.min(t.elapsed());
    }
    best.as_secs_f64() * 1e3
}

fn time_relayout(name: &str, mut d: Document) {
    let opts = LayoutOptions::default();
    let mut cache = LayoutCache::new();
    let t0 = std::time::Instant::now();
    let l = layout(&d, &mut cache, &opts);
    let cold = t0.elapsed();
    let end = d.body.len() - 1;
    let whole = best_relayout(&mut d, &mut cache, end, false);
    let at_end = best_relayout(&mut d, &mut cache, end, true);
    let middle = best_relayout(&mut d, &mut cache, end / 2, true);
    eprintln!(
        "PERF {name}: {} pages, cold {:.1} ms; relayout after typing: whole {whole:.1} ms, incremental at the end {at_end:.1} ms, in the middle {middle:.1} ms",
        l.pages.len(),
        cold.as_secs_f64() * 1e3,
    );
}

/// One table of `rows` 4-cell rows between two paragraphs.
fn big_table(rows: usize) -> Document {
    let mut d = Document::new();
    d.body.clear();
    d.body.push(para("Intro", ParaProps::default()));
    let mut t = Table::new(rows, 4, 468.0);
    for (r, row) in t.rows.iter_mut().enumerate() {
        for c in &mut row.cells {
            c.blocks = vec![para(&format!("cell {r} text"), ParaProps::default())];
        }
    }
    d.body.push(Arc::new(Block::Table(t)));
    d.body.push(para("After", ParaProps::default()));
    d
}

#[test]
#[ignore]
fn perf_relayout() {
    time_relayout("plain", synthetic(2000, 0, 0));
    time_relayout("headings", synthetic(2000, 2, 0));
    time_relayout("tables", synthetic(600, 0, 2));
    time_relayout("big table", big_table(8000));
}

/// What a cluster cost before it was slimmed (offsets, a `usize` object index, object height and
/// depth on every cluster), for the memory comparison below.
#[allow(dead_code)]
struct OldCluster {
    start: usize,
    end: usize,
    adv: f32,
    kind: (u8, usize),
    style: u16,
    g0: u32,
    g1: u32,
    break_after: bool,
    obj_h: f32,
    obj_d: f32,
    dot: bool,
}

/// A cluster is laid out for every character of every paragraph: keep it small.
#[test]
fn clusters_stay_small() {
    assert!(std::mem::size_of::<para::ClKind>() <= 8);
    // Byte offsets are u32 (48 bytes with usize offsets).
    assert!(std::mem::size_of::<para::Cluster>() <= 36, "{}", std::mem::size_of::<para::Cluster>());
}

/// Cluster offsets are u32: a paragraph longer than `MAX_LAID_OUT` bytes is laid out up to the
/// last character boundary within it (shown here with a smaller limit; 4 GiB of text won't fit a
/// test), formatting kept.
#[test]
fn paragraphs_past_the_offset_limit_are_cut_at_a_character() {
    use wordcraft_doc::{CharProps, Paragraph};
    let bold = CharProps { bold: Some(true), ..Default::default() };
    let mut p = Paragraph::with_text("ab", CharProps::default());
    p.insert_text(2, "\u{e9}cd", &bold).unwrap();
    assert!(para::laid_out_part(&p, 6).is_none(), "fits");
    let head = para::laid_out_part(&p, 3).unwrap();
    assert_eq!(head.text, "ab", "not inside the two-byte e-acute");
    let head = para::laid_out_part(&p, 4).unwrap();
    assert_eq!(head.text, "ab\u{e9}");
    assert_eq!(head.runs.iter().map(|r| r.len).sum::<usize>(), 4);
    assert_eq!(head.runs.last().map(|r| r.props.bold), Some(Some(true)));
    assert_eq!(para::cl_off(para::MAX_LAID_OUT), u32::MAX);
    assert_eq!(para::cl_off(usize::MAX), u32::MAX, "saturates (never reached)");
}

/// Layout memory of a large document (clusters, glyphs, line positions), before/after numbers.
#[test]
#[ignore]
fn perf_layout_memory() {
    let d = synthetic(6000, 0, 0);
    let mut cache = LayoutCache::new();
    let l = layout(&d, &mut cache, &LayoutOptions::default());
    let (mut cl, mut gl, mut xs, mut side) = (0usize, 0usize, 0usize, 0usize);
    for (pl, _) in cache.paras.values() {
        cl += pl.clusters.len();
        gl += pl.glyphs.len();
        xs += pl.lines.iter().map(|x| x.xs.len()).sum::<usize>();
        side += pl.obj_hd.len();
    }
    let (old, new) = (std::mem::size_of::<OldCluster>(), std::mem::size_of::<para::Cluster>());
    let mb = |b: usize| b as f64 / 1e6;
    let rest = gl * std::mem::size_of::<para::Glyph>() + xs * 4;
    eprintln!(
        "MEM {} pages, {cl} clusters: cluster {old} -> {new} bytes; clusters {:.1} -> {:.1} MB (+{side} side entries); with glyphs and xs {:.1} -> {:.1} MB",
        l.pages.len(),
        mb(cl * old),
        mb(cl * new + side * 12),
        mb(cl * old + rest),
        mb(cl * new + side * 12 + rest)
    );
    // What the cache holds: the lists' capacities. (Before the layouts were shrunk, and with
    // 48-byte clusters, 2026-10-14: 3,072,000 clusters and glyphs and 2,496,000 xs held for
    // 1,656,000 and 1,680,000 used, 194.3 MB; now 86.2 MB.)
    let held: usize = cache
        .paras
        .values()
        .map(|(pl, _)| {
            pl.clusters.capacity() * new
                + pl.glyphs.capacity() * std::mem::size_of::<para::Glyph>()
                + pl.lines.iter().map(|x| x.xs.capacity() * 4).sum::<usize>()
        })
        .sum();
    eprintln!("MEM held: clusters, glyphs and xs {:.1} MB", mb(held));
}

/// The layout cache keeps every paragraph's layout: its clusters, glyphs and line positions
/// hold no spare capacity (growing them while laying out left nearly as much again).
#[test]
fn paragraph_layouts_hold_no_spare_room() {
    let d = synthetic(20, 2, 1);
    let mut cache = LayoutCache::new();
    let _ = layout(&d, &mut cache, &LayoutOptions::default());
    assert!(!cache.is_empty());
    for (pl, _) in cache.paras.values() {
        assert_eq!(pl.clusters.capacity(), pl.clusters.len());
        assert_eq!(pl.glyphs.capacity(), pl.glyphs.len());
        assert!(pl.lines.iter().all(|l| l.xs.capacity() == l.xs.len()));
    }
    // A revision laid out one way holds room for that one layout only (a list allocated for four
    // made the cache of a document of tables 80 MB bigger), and the pass sorted out no
    // revision's layouts (only revisions laid out more than one way need it).
    for v in cache.paras.map.values() {
        assert!(v.list.len() > 1 || v.list.capacity() == 1, "room for {} layouts", v.list.capacity());
    }
    assert_eq!(cache.paras.sorted, 0, "revisions sorted out");
}

/// Revisions counted up hash apart in the bits a hash table picks a slot with (the low ones) and
/// in those it tells keys in a group apart with (the top seven), so the fast revision hasher costs
/// no extra probing.
#[test]
fn revision_hashes_spread() {
    let hash = |n: u64| {
        let mut h = RevHasher::default();
        h.write_u64(n);
        h.finish()
    };
    let start = 1_000_000u64;
    let slots: HashSet<u64> = (start..start + 4096).map(|n| hash(n) & 4095).collect();
    assert!(slots.len() > 2400, "{} of 4096 slots", slots.len());
    let tags: HashSet<u64> = (start..start + 4096).map(|n| hash(n) >> 57).collect();
    assert!(tags.len() > 120, "{} of 128 tags", tags.len());
}

/// Keep with next looks at the next paragraph through the paragraph cache: relaying out an
/// unchanged document with headings lays nothing out again (the lookahead used to bypass the
/// cache, so every heading laid its next paragraph out afresh on every pass).
#[test]
fn keep_with_next_lookahead_hits_the_cache() {
    let d = synthetic(40, 2, 0);
    let opts = LayoutOptions::default();
    let mut cache = LayoutCache::new();
    let _ = layout(&d, &mut cache, &opts);
    let (h0, m0) = (cache.hits, cache.misses);
    let _ = layout(&d, &mut cache, &opts);
    let headings = 20;
    assert_eq!(cache.misses, m0, "nothing laid out again");
    assert!(cache.hits - h0 >= (d.body.len() + headings) as u64, "the lookaheads hit the cache too: {}", cache.hits - h0);
}

/// Cache keys hash table-cell formatting structurally: a cell paragraph laid out under a table
/// style is found again, and a change to the style's formatting lays it out again.
#[test]
fn table_cell_keys_follow_the_table_style() {
    let mut d = synthetic(3, 0, 1);
    let opts = LayoutOptions::default();
    let mut cache = LayoutCache::new();
    let _ = layout(&d, &mut cache, &opts);
    let m0 = cache.misses;
    let _ = layout(&d, &mut cache, &opts);
    assert_eq!(cache.misses, m0);
    if let Some(s) = d.styles.get_mut("TableGrid") {
        s.chr.size = Some(31.0);
    }
    let l = layout(&d, &mut cache, &opts);
    assert!(cache.misses > m0, "a style change lays cells out again");
    let tall = l
        .pages
        .iter()
        .flat_map(|p| &p.items)
        .any(|it| matches!(it, Placed::Lines { para, .. } if para.lines.first().is_some_and(|x| x.height > 30.0)));
    assert!(tall, "the new size shows");
}

/// Proofing text marks deleted runs and objects by one walk over the runs; it agrees with
/// looking each character's run up (empty runs, runs past the text, the mark).
#[test]
fn proof_text_follows_the_runs() {
    use wordcraft_doc::Run;
    let del = wordcraft_doc::CharProps { del: Some(0), ..Default::default() };
    let mut p = Paragraph::with_text("ab\u{e9}cd\u{fffc}ef", Default::default());
    p.runs = vec![
        Run { len: 1, props: Default::default() },
        Run { len: 0, props: del.clone() },
        Run { len: 3, props: del.clone() },
        Run { len: 2, props: Default::default() },
    ];
    let t = para::proof_text(&p);
    assert_eq!(t.len(), p.text.len());
    let want: String = p
        .text
        .char_indices()
        .flat_map(|(i, c)| {
            let gone = c == wordcraft_doc::para::OBJ || p.props_of_char(i).del.is_some();
            let s: String = if gone { "\u{1}".repeat(c.len_utf8()) } else { c.to_string() };
            s.chars().collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(t, want);
    assert!(t.starts_with("a\u{1}\u{1}\u{1}cd"), "{t:?}");
}

/// Past 65,535 distinct formats in one paragraph (`Cluster::style` used to be a u16 capped at
/// its maximum), every character still points at its own format.
#[test]
fn a_paragraph_with_more_formats_than_u16_keeps_each_one() {
    use wordcraft_doc::{Rgb, Run, TextColor};
    let n = usize::from(u16::MAX) + 300;
    let color = |i: usize| TextColor::Rgb(Rgb((i >> 16) as u8, (i >> 8) as u8, i as u8));
    let mut p = Paragraph::with_text(&"x".repeat(n), Default::default());
    p.runs = (0..n).map(|i| Run { len: 1, props: wordcraft_doc::CharProps { color: Some(color(i)), ..Default::default() } }).collect();
    let mut d = Document::new();
    d.body = vec![wordcraft_doc::para_block(p)];
    let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions { view: ViewMode::Web, ..Default::default() });
    let pl =
        l.pages.iter().flat_map(|pg| &pg.items).find_map(|it| if let Placed::Lines { para, .. } = it { Some(para.clone()) } else { None }).unwrap();
    assert!(pl.styles.len() > usize::from(u16::MAX));
    for k in [0, 1000, n - 1] {
        let c = &pl.clusters[k];
        assert_eq!(pl.styles[c.style as usize].rc.color, color(c.start()), "cluster {k}");
    }
}

/// TITLE and AUTHOR fields without a result show the document's properties: changing them lays
/// those paragraphs out again (they weren't in the cache key, so the old text stayed).
#[test]
fn title_and_author_fields_follow_the_document_properties() {
    let mut d = Document::from_text("x");
    for instr in ["AUTHOR", "TITLE"] {
        d.insert_object(
            &wordcraft_doc::Pos::body(0, 0),
            wordcraft_doc::para::InlineObject::Field {
                instr: instr.into(),
                result: String::new(),
                locked: false,
                dirty: false,
                code: false,
                form: String::new(),
                nested: Vec::new(),
            },
            &Default::default(),
        )
        .unwrap();
    }
    let shown = |l: &DocLayout| -> String {
        l.pages[0]
            .items
            .iter()
            .filter_map(|it| if let Placed::Lines { para, .. } = it { Some(para.clone()) } else { None })
            .flat_map(|p| p.shown.iter().map(|s| s.1.clone()).collect::<Vec<_>>())
            .collect()
    };
    let opts = LayoutOptions::default();
    let mut cache = LayoutCache::new();
    d.core.title = "Old".into();
    d.core.creator = "Ann".into();
    assert_eq!(shown(&layout(&d, &mut cache, &opts)), "OldAnn");
    d.core.title = "New".into();
    assert_eq!(shown(&layout(&d, &mut cache, &opts)), "NewAnn");
    d.core.creator = "Bob".into();
    assert_eq!(shown(&layout(&d, &mut cache, &opts)), "NewBob");
}

/// Revision authors are numbered (for their colours) in order of first appearance, from one pass
/// built per page drawn rather than a scan of every revision per tracked run.
#[test]
fn revision_authors_number_in_order_of_first_appearance() {
    assert_eq!(display::authors_in_order(["b", "a", "b", "c", "a"].into_iter()), [0, 1, 0, 2, 1]);
    let mut d = Document::from_text("x");
    for a in ["Ann", "Bob", "Ann"] {
        d.revisions.push(wordcraft_doc::Revision::new(wordcraft_doc::RevisionKind::Insert, a, ""));
    }
    assert_eq!(display::revision_authors(&d), [0, 1, 0]);
}

/// The ASCII fast paths of the bidi checks agree with the Unicode tables.
#[test]
fn ascii_bidi_fast_paths_match_the_tables() {
    use unicode_bidi::BidiClass::{AL, BN, NSM, R};
    for c in (0u8..=127).map(char::from) {
        assert_eq!(wordcraft_fonts::is_rtl(c), matches!(unicode_bidi::bidi_class(c), R | AL), "{c:?}");
        assert_eq!(para::is_mark(c), matches!(unicode_bidi::bidi_class(c), NSM | BN) && !c.is_control(), "{c:?}");
    }
    assert!(wordcraft_fonts::is_rtl('\u{5d0}') && para::is_mark('\u{64b}') && para::is_mark('\u{200d}'));
}

/// A placed item is kept for every line run, rule and fill of every page (and again in table
/// rows' layouts): the big shape fields are boxed so they don't set every item's size (176 bytes
/// before).
#[test]
fn placed_items_stay_small() {
    assert!(std::mem::size_of::<crate::Placed>() <= 104, "{}", std::mem::size_of::<crate::Placed>());
}

/// Finished pages give back the room their item lists grew into (a long table's pages held
/// nearly twice their items).
#[test]
fn pages_hold_no_spare_room() {
    let d = big_table(300);
    let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
    assert!(l.pages.len() > 2);
    assert!(l.pages.iter().all(|p| p.items.capacity() == p.items.len()));
}
