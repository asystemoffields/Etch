//! Sections and their headers and footers across page breaks: which section each page belongs
//! to, and so which header it shows.
use super::*;
use wordcraft_doc::section::{HeaderSet, SectionStart};
use wordcraft_doc::{PartKind, para_block};

fn lay(doc: &Document) -> DocLayout {
    layout(doc, &mut LayoutCache::new(), &LayoutOptions::default())
}

fn header(d: &mut Document, text: &str) -> u32 {
    d.add_part(PartKind::Header, vec![para_block(Paragraph::with_text(text, Default::default()))])
}

/// The header text each page shows.
fn headers(d: &Document, l: &DocLayout) -> Vec<String> {
    l.pages.iter().map(|p| p.header_story.map(|id| d.plain_text(StoryRef::Part(id))).unwrap_or_default()).collect()
}

/// Section 1's text, a paragraph holding only a page break, an empty paragraph ending section 1,
/// then section 2 (starting `start`, with a first-page header) and `filler` paragraphs.
fn break_then_section(start: SectionStart, filler: usize) -> Document {
    let mut d = Document::from_text("Section 1 text");
    let a = header(&mut d, "A");
    let (b, bf) = (header(&mut d, "B"), header(&mut d, "B first"));
    let mut brk = Paragraph::new();
    let _ = brk.insert_text(0, &wordcraft_doc::para::PAGE_BREAK.to_string(), &Default::default());
    d.body.push(para_block(brk));
    let mut mark = Paragraph::new();
    mark.section = Some(Box::new(SectionProps { headers: HeaderSet { default: Some(a), ..Default::default() }, ..SectionProps::default() }));
    d.body.push(para_block(mark));
    d.body.push(para_block(Paragraph::with_text("Section 2 text", Default::default())));
    for i in 0..filler {
        d.body.push(para_block(Paragraph::with_text(&format!("Filler {i}"), Default::default())));
    }
    d.last_section = SectionProps {
        start,
        title_page: true,
        headers: HeaderSet { default: Some(b), first: Some(bf), ..Default::default() },
        ..SectionProps::default()
    };
    d
}

/// The first body blocks on each page.
fn page_blocks(l: &DocLayout) -> Vec<Vec<usize>> {
    l.pages
        .iter()
        .map(|p| {
            let mut v: Vec<usize> = p
                .items
                .iter()
                .filter_map(|it| match it {
                    Placed::Lines { story: StoryRef::Body, path, .. } => Some(path.last()),
                    _ => None,
                })
                .collect();
            v.dedup();
            v.truncate(3);
            v
        })
        .collect()
}

/// #2109: a page break, an empty paragraph ending section 1, then a continuous section 2 with a
/// first-page header. Word starts section 2 at the top of page 2, with its first-page header
/// there; the mark took page 2 for section 1, so section 2's headers came a page late.
#[test]
fn a_continuous_section_after_a_page_break_starts_the_page_with_its_own_header() {
    let d = break_then_section(SectionStart::Continuous, 60);
    let l = lay(&d);
    assert_eq!(headers(&d, &l), ["A", "B first", "B", "B"]);
    assert_eq!(l.pages.iter().map(|p| p.section).collect::<Vec<_>>(), [0, 1, 1, 1]);
    // The mark (block 2) is laid out, at the top of the page section 2 starts with.
    assert_eq!(page_blocks(&l).get(1).map(|v| v.as_slice()), Some(&[2, 3, 4][..]));
    assert_eq!(l.pages.get(1).map(|p| p.first_block), Some(1), "the page the break started (as before)");
    // The page break and the section mark in one paragraph: the same.
    let mut d = break_then_section(SectionStart::Continuous, 60);
    let mark = d.body.remove(2);
    if let (Some(Block::Para(p)), Block::Para(m)) = (d.body.get_mut(1).map(Arc::make_mut), &*mark) {
        p.section = m.section.clone();
    }
    let l = lay(&d);
    assert_eq!(headers(&d, &l), ["A", "B first", "B", "B"]);
    // A next-page section after the same break keeps its own page, as before.
    let d = break_then_section(SectionStart::NextPage, 60);
    assert_eq!(headers(&d, &lay(&d)), ["A", "A", "B first", "B", "B"]);
}

/// Typing in and around a continuous section after a page break relays out like a layout from
/// scratch (the page the section takes over has no checkpoint of its own before the mark).
#[test]
fn edits_around_a_section_after_a_page_break_relay_out_like_from_scratch() {
    let opts = LayoutOptions::default();
    let mut d = break_then_section(SectionStart::Continuous, 60);
    let mut cache = LayoutCache::new();
    let mut prev = layout(&d, &mut cache, &opts);
    let dump = |l: &DocLayout| format!("{:?}", l.pages);
    for (step, at) in [3usize, 2, 2, 1, 0, 30, 2].into_iter().enumerate() {
        if let Some(Block::Para(p)) = d.body.get_mut(at).map(Arc::make_mut) {
            if step == 2 {
                // The mark empty again.
                let n = p.len();
                let _ = p.delete(0, n);
            } else {
                let _ = p.insert_text(0, "x", &Default::default());
            }
            p.touch();
        }
        prev = layout_after(&d, &mut cache, &opts, Some(&prev));
        let full = layout(&d, &mut LayoutCache::new(), &opts);
        assert!(dump(&prev) == dump(&full), "step {step} (block {at}): resumed {:?}", cache.resumed);
    }
}

/// A long synthetic document with a few paragraphs in each of a handful of styles: Normal, Heading
/// 1 (every 10th), Quote and a style based on it (every 50th each).
fn styled_long(n: usize) -> Document {
    let mut d = crate::tests_perf::synthetic(n, 10, 0);
    let mut sheet = (*d.styles).clone();
    sheet.styles.push(wordcraft_doc::styles::Style {
        id: "QuoteMore".into(),
        name: "Quote More".into(),
        based_on: Some("Quote".into()),
        para: wordcraft_doc::props::ParaProps { indent_left: Some(36.0), ..Default::default() },
        ..Default::default()
    });
    d.styles = wordcraft_doc::Shared::new(sheet);
    for (i, b) in d.body.iter_mut().enumerate() {
        if let Block::Para(p) = Arc::make_mut(b)
            && p.props.style.is_none()
        {
            p.props.style = match i % 50 {
                7 => Some("Quote".into()),
                33 => Some("QuoteMore".into()),
                _ => None,
            };
        }
    }
    d
}

/// A style change lays out again only the paragraphs whose formatting comes from it: those in
/// it, in a style based on it, or with runs in it (a character style); the rest come from the
/// cache. The result is the layout from scratch every time.
#[test]
fn a_style_change_lays_out_again_only_the_paragraphs_using_it() {
    let mut d = styled_long(300);
    // A few runs in Strong.
    for i in [11usize, 101, 201] {
        if let Some(Block::Para(p)) = d.body.get_mut(i).map(Arc::make_mut) {
            p.format(0, 5, &|c| c.style = Some("Strong".into())).unwrap();
        }
    }
    let using = |d: &Document, f: &dyn Fn(&Paragraph) -> bool| d.body.iter().filter(|b| b.as_para().is_some_and(f)).count();
    let opts = LayoutOptions::default();
    let mut cache = LayoutCache::new();
    let mut prev = layout(&d, &mut cache, &opts);
    let style_of = |p: &Paragraph| p.props.style.clone().unwrap_or_default();
    for (id, users) in [
        ("Quote", using(&d, &|p| matches!(style_of(p).as_str(), "Quote" | "QuoteMore"))),
        ("QuoteMore", using(&d, &|p| style_of(p) == "QuoteMore")),
        ("Strong", 3),
        ("Caption", 0),
    ] {
        if let Some(st) = wordcraft_doc::Shared::make_mut(&mut d.styles).get_mut(id) {
            st.chr.size = Some(19.0);
        }
        let before = cache.misses;
        prev = layout_after(&d, &mut cache, &opts, Some(&prev));
        let laid = cache.misses - before;
        assert!(laid >= users as u64 && laid <= users as u64 + 8, "{id}: {laid} paragraphs laid out again, {users} use it");
        let full = layout(&d, &mut LayoutCache::new(), &opts);
        assert_eq!(format!("{:?}", prev.pages), format!("{:?}", full.pages), "{id}: as laid out from scratch");
    }
    // The document defaults: every paragraph.
    wordcraft_doc::Shared::make_mut(&mut d.styles).default_chr.size = Some(13.0);
    let before = cache.misses;
    prev = layout_after(&d, &mut cache, &opts, Some(&prev));
    assert!(cache.misses - before >= 300);
    assert_eq!(format!("{:?}", prev.pages), format!("{:?}", layout(&d, &mut LayoutCache::new(), &opts).pages));
}

/// Modify Style timings on a long document: the first layout, then a change to a style few
/// paragraphs use (Quote, which QuoteMore is based on), to a style nobody uses, and to Normal
/// (every paragraph). Run with `cargo test --release -p wordcraft-layout perf_modify_style --
/// --ignored --nocapture`.
#[test]
#[ignore]
fn perf_modify_style() {
    let mut d = styled_long(3000);
    let opts = LayoutOptions::default();
    let mut cache = LayoutCache::new();
    let t = std::time::Instant::now();
    let mut prev = layout(&d, &mut cache, &opts);
    eprintln!("first layout: {} pages, {:.0} ms", prev.pages.len(), t.elapsed().as_secs_f64() * 1e3);
    for (what, id) in [("Quote", "Quote"), ("unused Caption", "Caption"), ("Normal", "Normal")] {
        let mut best = f64::MAX;
        for k in 0..3 {
            if let Some(st) = wordcraft_doc::Shared::make_mut(&mut d.styles).get_mut(id) {
                st.para.space_after = Some(6.0 + k as f32);
            }
            let t = std::time::Instant::now();
            prev = layout_after(&d, &mut cache, &opts, Some(&prev));
            best = best.min(t.elapsed().as_secs_f64() * 1e3);
        }
        let full = layout(&d, &mut LayoutCache::new(), &opts);
        assert_eq!(format!("{:?}", prev.pages), format!("{:?}", full.pages), "{what}: as laid out from scratch");
        eprintln!("modify {what}: best of 3 {best:.0} ms ({} pages)", prev.pages.len());
    }
}
