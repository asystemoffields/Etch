//! The paragraph budget: a file can't make more paragraphs than the cap, however few bytes it
//! takes (empty `<w:p/>` zip to almost nothing).

use std::io::Write;

use super::read_opened_budgeted;
use crate::DocxError;
use crate::package::Package;

const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
const ROOT_RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

fn package(body: &str) -> Vec<u8> {
    let xml = format!("<w:document {W_NS}><w:body>{body}</w:body></w:document>");
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in [("_rels/.rels", ROOT_RELS.as_bytes()), ("word/document.xml", xml.as_bytes())] {
        zw.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(bytes).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

fn read(bytes: &[u8], max: usize) -> Result<wordcraft_doc::Document, DocxError> {
    read_opened_budgeted(&Package::open(bytes).unwrap(), max)
}

#[test]
fn paragraphs_past_the_budget_refuse_the_file() {
    let file = package(&"<w:p/>".repeat(51));
    assert!(matches!(read(&file, 50), Err(DocxError::Limit(_))));
    let doc = read(&file, 51).expect("at the budget it opens");
    assert_eq!(doc.body.len(), 51);
    // The public reader uses the shared cap.
    assert_eq!(crate::read(&file).expect("opens").body.len(), 51);
}

#[test]
fn table_cells_and_their_paragraphs_count() {
    // Three cells: an empty one (it gets a paragraph) and two with one paragraph each, plus a
    // paragraph after the table.
    let file = package("<w:tbl><w:tr><w:tc/><w:tc><w:p/></w:tc><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>");
    assert!(matches!(read(&file, 3), Err(DocxError::Limit(_))));
    assert!(read(&file, 4).is_ok());
}

/// A paragraph read keeps no room to spare in its text and runs.
#[test]
fn paragraphs_read_hold_no_spare_room() {
    let file =
        package(r#"<w:p><w:r><w:t>one run</w:t></w:r></w:p><w:p><w:r><w:t>plain </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r></w:p>"#);
    let doc = read(&file, 100).expect("opens");
    for b in &doc.body {
        let p = b.as_para().expect("a paragraph");
        assert_eq!(p.runs.capacity(), p.runs.len(), "{:?}", p.text);
        assert_eq!(p.text.capacity(), p.text.len(), "{:?}", p.text);
    }
}

#[test]
fn the_main_part_is_counted_as_the_reader_reads_it() {
    // One branch of an alternate content counts (the reader reads one), and paragraphs in
    // table cells count.
    let mc = r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><mc:Choice Requires="wps"><w:p/></mc:Choice><mc:Fallback><w:p/></mc:Fallback></mc:AlternateContent>"#;
    let body = format!("{mc}<w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>");
    let count = |body: &str| super::story::paragraph_elements(format!("<w:document {W_NS}><w:body>{body}</w:body></w:document>").as_bytes()).unwrap();
    assert_eq!(count(&body), (crate::xml::Name::from("w:document"), 3));
    // The branch the reader reads: a choice it understands; else the fallback; else the first
    // choice. Anything else in an alternate content, and the branches not read, don't count.
    let ac = |inner: &str| {
        format!(
            r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:x="urn:x" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape">{inner}</mc:AlternateContent>"#
        )
    };
    let choice = |req: &str, n: usize| format!(r#"<mc:Choice Requires="{req}">{}</mc:Choice>"#, "<w:p/>".repeat(n));
    let fallback = |n: usize| format!("<mc:Fallback>{}</mc:Fallback>", "<w:p/>".repeat(n));
    for (inner, want) in [
        (format!("{}{}", choice("x", 1), fallback(2)), 2),
        (format!("{}{}{}", choice("x", 1), choice("wps", 3), fallback(2)), 3),
        (format!("{}{}", choice("x", 4), choice("x", 1)), 4),
        (format!("<w:p/>{}", fallback(1)), 1),
        (fallback(0), 0),
        (format!("<mc:Fallback><w:p/>{}</mc:Fallback>", ac(&fallback(2))), 3),
    ] {
        assert_eq!(count(&ac(&inner)).1, want, "{inner}");
    }
    let file = package(&body);
    assert!(read(&file, 3).is_ok());
    assert!(matches!(read(&file, 2), Err(DocxError::Limit(_))));
}

/// A package of `files` (name, XML).
fn zip(files: &[(&str, &str)]) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in files {
        zw.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(bytes.as_bytes()).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

/// A document showing `charts` (part names; repeats show one part twice), with core properties
/// of `core` elements.
fn with_charts(charts: &[&str], core: usize) -> Vec<u8> {
    let c_ns = "http://schemas.openxmlformats.org/drawingml/2006/chart";
    let drawing = |i: usize| {
        format!(
            r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="2743200" cy="1371600"/><wp:docPr id="{i}" name="C"/><a:graphic><a:graphicData uri="{c_ns}"><c:chart xmlns:c="{c_ns}" r:id="rId{i}"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
        )
    };
    let body: String = (0..charts.len()).map(drawing).collect();
    let ns = r#"xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
    let doc = format!("<w:document {W_NS} {ns}><w:body>{body}</w:body></w:document>");
    let rels: String = charts
        .iter()
        .enumerate()
        .map(|(i, c)| {
            format!(
                r#"<Relationship Id="rId{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="charts/{c}"/>"#
            )
        })
        .collect();
    let rels = format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#);
    // A chart of 2,000 points: about 10,000 nodes.
    let points = r#"<c:pt idx="0"><c:v>1</c:v></c:pt>"#.repeat(2_000);
    let chart = format!(
        r#"<c:chartSpace xmlns:c="{c_ns}"><c:chart><c:plotArea><c:barChart><c:ser><c:val><c:numLit><c:ptCount val="1"/>{points}</c:numLit></c:val></c:ser></c:barChart></c:plotArea></c:chart></c:chartSpace>"#
    );
    let core = format!(
        r#"<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties">{}</cp:coreProperties>"#,
        "<x/>".repeat(core)
    );
    let mut files = vec![
        ("_rels/.rels".to_string(), ROOT_RELS.to_string()),
        ("word/document.xml".into(), doc),
        ("word/_rels/document.xml.rels".into(), rels),
        ("docProps/core.xml".into(), core),
    ];
    for c in charts {
        let name = format!("word/charts/{c}");
        if !files.iter().any(|(n, _)| *n == name) {
            files.push((name, chart.clone()));
        }
    }
    let files: Vec<(&str, &str)> = files.iter().map(|(n, x)| (n.as_str(), x.as_str())).collect();
    zip(&files)
}

/// The budget bounds the XML nodes a file holds at once. A part read and let go gives its nodes
/// back, but parts held for the rest of the read (charts and diagrams, each parsed once however
/// often it is shown) count together: a file whose charts together pass the budget is refused
/// even though each fits alone, and so is one where what doesn't fit is a part the reader would
/// skip if it were broken (nothing is left out silently). Before, each part had a budget of its
/// own: six chart parts of four million elements each, all held, took 974 MB.
#[test]
fn parts_held_together_count_together() {
    let budget = 15_000;
    let open = |file: &[u8]| {
        let pkg = Package::open(file).unwrap().with_node_budget(budget);
        let read = read_opened_budgeted(&pkg, 100);
        (read.map(|d| d.body.len()), pkg.nodes_spent())
    };
    // One chart (10,000 nodes) fits, shown twice too (one tree), and so do 2,000 nodes of core
    // properties read after it: the main part's are given back by then.
    assert_eq!(open(&with_charts(&["a.xml"], 0)), (Ok(1), false));
    assert_eq!(open(&with_charts(&["a.xml", "a.xml"], 0)), (Ok(2), false));
    assert_eq!(open(&with_charts(&["a.xml"], 2_000)), (Ok(1), false));
    // Two charts held together don't.
    let (read, spent) = open(&with_charts(&["a.xml", "b.xml"], 0));
    assert!(matches!(read, Err(DocxError::Limit(_))) && spent);
    // Nor do core properties (a part the reader skips when broken) that only fit without the chart.
    let (read, spent) = open(&with_charts(&["a.xml"], 8_000));
    assert!(matches!(read, Err(DocxError::Limit(_))) && spent);
    let (read, spent) = open(&with_charts(&[], 8_000));
    assert_eq!((read.ok(), spent), (Some(1), false));
}

/// The main part is read a block at a time as it is parsed, so the budget bounds the nodes held
/// at once, not the part: a document of any length opens (as long as each block fits). Before,
/// the main part's whole tree was held, so a document WordCraft saved with more than the cap's
/// nodes (120,000 paragraphs of ten formatted runs) couldn't be opened again.
#[test]
fn the_main_part_is_held_a_block_at_a_time() {
    let para = "<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>x</w:t></w:r></w:p>";
    let table = format!(
        "<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"100\"/></w:tblGrid>{}</w:tbl>",
        format!("<w:tr><w:tc>{para}</w:tc></w:tr>").repeat(2_000)
    );
    let body = format!("{}{table}<w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/></w:sectPr>", para.repeat(2_000));
    let file = zip(&[("_rels/.rels", ROOT_RELS), ("word/document.xml", &format!("<w:document {W_NS}><w:body>{body}</w:body></w:document>"))]);
    // About 30,000 nodes in all; a few hundred are held at once.
    let pkg = Package::open(&file).unwrap().with_node_budget(500);
    let doc = read_opened_budgeted(&pkg, 10_000).expect("opens");
    // 2,000 paragraphs, the table, and the paragraph the model keeps after a final table.
    assert_eq!(doc.body.len(), 2_002);
    let rows = doc.body.get(2_000).map(|b| match &**b {
        wordcraft_doc::Block::Table(t) => t.rows.len(),
        _ => 0,
    });
    assert_eq!(rows, Some(2_000), "the table is read a row at a time");
    assert_eq!(doc.last_section.page_w, 612.0);
    // A block that doesn't fit is still refused.
    let big = format!("<w:p>{}</w:p>", "<w:r><w:t>x</w:t></w:r>".repeat(200));
    let file = zip(&[("_rels/.rels", ROOT_RELS), ("word/document.xml", &format!("<w:document {W_NS}><w:body>{big}</w:body></w:document>"))]);
    assert!(matches!(read_opened_budgeted(&Package::open(&file).unwrap().with_node_budget(500), 10), Err(DocxError::Limit(_))));
}

/// A document of `paras` paragraphs as heavily formatted as the writer writes: every paragraph
/// property it has, `runs` runs a paragraph with every character property, each run a tracked
/// insertion and every other one a tracked formatting change, a link, a bookmark around the text.
pub(super) fn formatted_document(paras: usize, runs: usize) -> wordcraft_doc::Document {
    use wordcraft_doc::props::*;
    use wordcraft_doc::{Block, Document, InlineObject, Paragraph, Revision, RevisionKind, Run};
    let border = Border { style: BorderStyle::Single, width: 0.5, color: Some(Rgb(1, 2, 3)), space: 1.0 };
    let mut doc = Document::new();
    doc.body.clear();
    let revs = wordcraft_doc::Shared::make_mut(&mut doc.revisions);
    revs.push(Revision { kind: RevisionKind::Insert, author: "Synthetic Author".into(), date: "2026-01-02T03:04:05Z".into(), move_name: None });
    revs.push(Revision { kind: RevisionKind::Format, author: "Synthetic Author".into(), date: "2026-01-02T03:04:05Z".into(), move_name: None });
    let word = "words ";
    for i in 0..paras {
        let mut p = Paragraph::new();
        p.props = ParaProps {
            align: Some(wordcraft_doc::Align::Justify),
            indent_left: Some(10.0),
            indent_right: Some(11.0),
            indent_first: Some(12.0),
            space_before: Some(3.0),
            space_after: Some(4.0),
            line_spacing: Some(LineSpacing::Multiple(1.15)),
            contextual_spacing: Some(true),
            keep_next: Some(true),
            keep_lines: Some(true),
            widow_control: Some(false),
            outline_level: Some(3),
            tabs: Some([36.0, 72.0, 108.0].map(|pos| TabStop { pos, align: TabAlign::Right, leader: TabLeader::Dot }).to_vec()),
            shading: Some(Rgb(250, 240, 230)),
            borders: Some(Borders {
                top: Some(border),
                left: Some(border),
                bottom: Some(border),
                right: Some(border),
                between: Some(border),
                inside_v: None,
            }),
            suppress_hyphens: Some(true),
            suppress_line_numbers: Some(true),
            ..Default::default()
        };
        let props = |k: usize| CharProps {
            font: Some("Source Serif 4".into()),
            font_cs: Some("Source Sans 3".into()),
            font_ea: Some("Inter".into()),
            size: Some(11.0 + (k % 3) as f32),
            size_cs: Some(12.0),
            bold: Some(k.is_multiple_of(2)),
            bold_cs: Some(true),
            italic: Some(k.is_multiple_of(3)),
            italic_cs: Some(true),
            underline: Some(Underline::Single),
            underline_color: Some(Rgb(9, 8, 7)),
            strike: Some(k.is_multiple_of(5)),
            color: Some(TextColor::Rgb(Rgb(10, 20, 30))),
            highlight: Some(Highlight::Yellow),
            shading: Some(Rgb(200, 210, 220)),
            border: Some(border),
            vert_align: Some(if k.is_multiple_of(4) { VertAlign::Superscript } else { VertAlign::Baseline }),
            caps: Some(k.is_multiple_of(6)),
            spacing: Some(0.5),
            scale: Some(110.0),
            position: Some(1.0),
            kern: Some(8.0),
            lang: Some("en-GB".into()),
            no_proof: Some(true),
            link: (k == 1).then(|| "https://example.com/synthetic".to_string()),
            ins: Some(0),
            fmt_change: if k % 2 == 1 { PropChange::boxed(1, CharProps { bold: Some(true), ..Default::default() }) } else { None },
            ..Default::default()
        };
        p.text = word.repeat(runs);
        p.runs = (0..runs).map(|k| Run { len: word.len(), props: props(k) }).collect();
        p.mark = props(0);
        let name = format!("b{i}");
        let _ = p.insert_object(0, InlineObject::BookmarkStart { name: name.clone() }, &props(0));
        let end = p.text.len();
        let _ = p.insert_object(end, InlineObject::BookmarkEnd { name }, &props(0));
        doc.body.push(std::sync::Arc::new(Block::Para(p)));
    }
    doc
}

/// XML nodes of the main part of `file` as the budget counts them, and the paragraphs read.
fn nodes_of(file: &[u8]) -> (usize, usize) {
    let pkg = Package::open(file).unwrap();
    let (_, nodes) = crate::xml::parse_limited(pkg.get("word/document.xml").unwrap(), &[], usize::MAX).unwrap();
    let doc = crate::read::read_opened(&pkg).unwrap();
    (nodes, doc.paragraph_count())
}

#[test]
#[ignore]
fn probe_writer_nodes() {
    for (paras, runs) in [(200, 1), (200, 10), (200, 30)] {
        let (n, p) = nodes_of(&crate::write(&formatted_document(paras, runs)).unwrap());
        eprintln!("{runs} runs: {n} nodes for {p} paragraphs: {:.1} a paragraph", n as f64 / p as f64);
    }
}

/// A quarter of what a Windows main thread has (1 MiB), where the app reads a file it opens.
const SMALL_STACK: usize = 256 * 1024;

/// Files nesting each element the reader reads the children of as deep as the parser allows
/// ([`crate::xml::MAX_DEPTH`]): (what, file, a Word 2003 XML file rather than a package).
fn deepest_nesting() -> Vec<(&'static str, Vec<u8>, bool)> {
    let n = 250;
    let wrap = |open: &str, close: &str, inner: &str, k: usize| format!("{}{inner}{}", open.repeat(k), close.repeat(k));
    let run = "<w:r><w:t>x</w:t></w:r>";
    let para = format!("<w:p>{run}</w:p>");
    let in_para = |s: String| format!("<w:p>{s}</w:p>");
    let ac = (
        r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Fallback>"#,
        "</mc:Fallback></mc:AlternateContent>",
    );
    let docx = [
        ("custom XML around blocks", wrap("<w:customXml>", "</w:customXml>", &para, n)),
        ("smart tags", in_para(wrap("<w:smartTag>", "</w:smartTag>", run, n))),
        ("hyperlinks", in_para(wrap("<w:hyperlink>", "</w:hyperlink>", run, n))),
        ("insertions", in_para(wrap("<w:ins>", "</w:ins>", run, n))),
        ("simple fields", in_para(wrap(r#"<w:fldSimple w:instr="PAGE">"#, "</w:fldSimple>", run, n))),
        ("inline content controls", in_para(wrap("<w:sdt><w:sdtContent>", "</w:sdtContent></w:sdt>", run, n / 2))),
        ("block content controls", wrap("<w:sdt><w:sdtContent>", "</w:sdtContent></w:sdt>", &para, n / 2)),
        ("bidirectional embeddings", in_para(wrap(r#"<w:dir w:val="rtl">"#, "</w:dir>", run, n))),
        ("alternate content around blocks", wrap(ac.0, ac.1, &para, n / 2)),
        ("alternate content in a paragraph", in_para(wrap(ac.0, ac.1, run, n / 2))),
        ("alternate content in a run", in_para(format!("<w:r>{}</w:r>", wrap(ac.0, ac.1, "<w:t>x</w:t>", n / 2 - 2)))),
        ("tables", wrap("<w:tbl><w:tr><w:tc>", "</w:tc></w:tr></w:tbl>", &para, 80)),
    ];
    let w2003 = [
        ("Word 2003 sub-sections", wrap("<wx:sub-section>", "</wx:sub-section>", &para, n)),
        ("Word 2003 hyperlinks", in_para(wrap(r#"<w:hlink w:bookmark="a">"#, "</w:hlink>", run, n))),
        (
            "Word 2003 insertions",
            in_para(wrap(r#"<aml:annotation aml:id="1" w:type="Word.Insertion"><aml:content>"#, "</aml:content></aml:annotation>", run, n / 2)),
        ),
    ];
    let mut out: Vec<(&'static str, Vec<u8>, bool)> = docx.into_iter().map(|(what, body)| (what, package(&body), false)).collect();
    for (what, body) in w2003 {
        let x = format!(
            r#"<w:wordDocument xmlns:w="http://schemas.microsoft.com/office/word/2003/wordml" xmlns:wx="http://schemas.microsoft.com/office/word/2003/auxHint" xmlns:aml="http://schemas.microsoft.com/aml/2001/core"><w:body>{body}</w:body></w:wordDocument>"#
        );
        out.push((what, x.into_bytes(), true));
    }
    out
}

/// Nesting as deep as the parser allows reads in a small stack: wrappers are walked with a list
/// and the calls that remain take little stack each. Before, 250 levels of custom XML around a
/// paragraph took more than a whole Windows main thread's stack (release build, 0.8–1 MiB), so
/// the app crashed on such a file there. Nesting deeper than the model keeps (tables more than 24
/// deep, more than 8 smart tags around a run, fields more than 32 deep) has the file refused, naming it, rather than read
/// with the deeper levels left out; the rest is read whole.
#[test]
fn the_deepest_nesting_reads_in_a_small_stack() {
    let refused = ["tables", "smart tags", "simple fields"];
    for (what, file, w2003) in deepest_nesting() {
        let read = std::thread::Builder::new()
            .stack_size(SMALL_STACK)
            .spawn(move || if w2003 { crate::read_xml(&file) } else { crate::read(&file) })
            .unwrap()
            .join()
            .unwrap_or_else(|_| panic!("{what}: panicked"));
        if refused.contains(&what) {
            assert!(matches!(read, Err(DocxError::Limit(_))), "{what}: {:?}", read.map(|d| d.paragraph_count()));
            continue;
        }
        let doc = read.unwrap_or_else(|e| panic!("{what}: {e}"));
        assert!(doc.plain_text(wordcraft_doc::StoryRef::Body).contains('x'), "{what}: the text is read");
    }
}

/// 09: the reader counts paragraphs as the editor does (`wordcraft_doc::limits::Tally`), so a
/// document at the limit opens again: what WordCraft writes reads back within a budget of exactly
/// the document's own count, drop caps (written as two paragraphs) and the note separators (the
/// writer's own, not the document's) included; a budget of one less refuses it.
#[test]
fn the_reader_counts_paragraphs_as_the_editor_does() {
    use std::sync::Arc;
    use wordcraft_doc::para::{InlineObject, NoteKind};
    use wordcraft_doc::props::CharProps;
    use wordcraft_doc::{Block, Document, Paragraph, PartKind, para_block};
    let mut doc = Document::from_text("Synthetic opening line\nSynthetic second line\nSynthetic third line");
    if let Some(Block::Para(p)) = doc.body.first_mut().map(Arc::make_mut) {
        p.props.drop_cap = Some(3);
    }
    // The body alone: the main part's paragraphs, counted before it is read, are as many.
    let count = usize::try_from(wordcraft_doc::limits::Tally::of(&doc).paragraphs).unwrap();
    let bytes = crate::write(&doc).unwrap();
    assert!(read(&bytes, count).is_ok(), "the body read within a budget of its own count");
    assert!(matches!(read(&bytes, count - 1), Err(DocxError::Limit(_))));
    for (pk, kind) in [(PartKind::Footnote, NoteKind::Footnote), (PartKind::Endnote, NoteKind::Endnote)] {
        let id = doc.add_part(pk, vec![para_block(Paragraph::with_text("Synthetic note.", CharProps::default()))]);
        if let Some(Block::Para(p)) = doc.body.get_mut(1).map(Arc::make_mut) {
            let end = p.len();
            let _ = p.insert_object(end, InlineObject::NoteRef { kind, id, custom: String::new() }, &CharProps::default());
        }
    }
    let count = usize::try_from(wordcraft_doc::limits::Tally::of(&doc).paragraphs).unwrap();
    let bytes = crate::write(&doc).unwrap();
    let back = read(&bytes, count).expect("a document read within a budget of its own count");
    assert_eq!(wordcraft_doc::limits::Tally::of(&back).paragraphs, count as u64);
    assert!(back.body.first().and_then(|b| b.as_para()).is_some_and(|p| p.props.drop_cap == Some(3)), "the drop cap is read back");
    assert!(back.footnote_separators.separator.is_some(), "the separators are read");
    assert!(matches!(read(&bytes, count - 1), Err(DocxError::Limit(_))));
}

/// 09: the note separators have a budget of their own: a file whose separators have more
/// paragraphs than that is refused, naming them.
#[test]
fn note_separators_past_their_budget_refuse_the_file() {
    let sep = |n: usize| format!(r#"<w:footnote w:type="separator" w:id="-1">{}</w:footnote>"#, "<w:p/>".repeat(n));
    let notes = |n: usize| format!("<w:footnotes {W_NS}>{}</w:footnotes>", sep(n));
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdF" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes" Target="footnotes.xml"/></Relationships>"#;
    let doc = format!("<w:document {W_NS}><w:body><w:p/></w:body></w:document>");
    for (n, ok) in [(super::MAX_SEPARATOR_PARAGRAPHS, true), (super::MAX_SEPARATOR_PARAGRAPHS + 1, false)] {
        let file = zip(&[
            ("_rels/.rels", ROOT_RELS),
            ("word/document.xml", &doc),
            ("word/_rels/document.xml.rels", rels),
            ("word/footnotes.xml", &notes(n)),
        ]);
        match crate::read(&file) {
            Ok(d) => assert!(ok, "{n} separator paragraphs opened: {:?}", d.footnote_separators.separator.map(|b| b.len())),
            Err(e) => assert!(!ok && e.to_string().contains("note separators"), "{n}: {e}"),
        }
    }
}
