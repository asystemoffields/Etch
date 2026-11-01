//! Stories read a block at a time as their parts are parsed read exactly as whole trees do: the
//! same document, block for block, from every structure the reader looks through (content
//! controls around blocks, rows and cells, custom XML, merged cells, nested tables, notes,
//! comments, headers), while holding a small part of what the whole trees hold.

use std::io::Write;

use super::read_opened_budgeted;
use super::stream::WHOLE_TREES;
use crate::package::Package;

const NS: &str = concat!(
    r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" "#,
    r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" "#,
    r#"xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" "#,
    r#"xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml" "#,
    r#"xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#
);
const ROOT_RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn zip(files: &[(String, String)]) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in files {
        zw.start_file(name.as_str(), zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(bytes.as_bytes()).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

fn p(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

fn cell(inner: &str) -> String {
    format!("<w:tc><w:tcPr><w:tcW w:w=\"2000\" w:type=\"dxa\"/></w:tcPr>{inner}</w:tc>")
}

fn table(rows: &str) -> String {
    format!(
        "<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblPrChange w:id=\"90\" w:author=\"A\"><w:tblPr/></w:tblPrChange></w:tblPr><w:tblGrid><w:gridCol w:w=\"2000\"/><w:gridCol w:w=\"2000\"/></w:tblGrid>{rows}</w:tbl>"
    )
}

fn row(cells: &str) -> String {
    format!(
        "<w:tr><w:trPr><w:trPrChange w:id=\"91\" w:author=\"B\"><w:trPr/></w:trPrChange><w:ins w:id=\"92\" w:author=\"C\"/></w:trPr>{cells}</w:tr>"
    )
}

fn sdt(alias: &str, content: &str) -> String {
    format!(
        "<w:sdt><w:sdtPr><w:alias w:val=\"{alias}\"/><w:tag w:val=\"{alias}\"/><w:id w:val=\"7\"/></w:sdtPr><w:sdtEndPr><w:rPr><w:b/></w:rPr></w:sdtEndPr><w:sdtContent>{content}</w:sdtContent></w:sdt>"
    )
}

/// Every structure a story's blocks, rows and cells come in, nested in one another.
fn body() -> String {
    let simple = table(&row(&(cell(&p("a")) + &cell(&p("b")))).repeat(2));
    // Rows in repeating sections (content controls around rows), nested, in custom XML, one
    // section empty and one around a row without cells (dropped, its end too).
    let item = |t: &str| sdt("item", &row(&(cell(&p(t)) + &cell(""))));
    let sections = table(&format!(
        "{}{}{}{}<w:customXml w:element=\"x\">{}</w:customXml>{}",
        row(&cell(&p("first"))),
        sdt("section", &format!("{}{}", item("one"), item("two"))),
        sdt("empty", ""),
        sdt("no cells", "<w:tr><w:trPr/></w:tr>"),
        row(&cell(&p("in custom XML"))),
        sdt("after", &row(&cell(&p("last")))),
    ));
    // Cells in content controls and custom XML, merged cells, an empty cell, one with only its
    // properties, more cells than a row may have, and a nested table in a cell.
    let merged = "<w:tc><w:tcPr><w:hMerge/></w:tcPr><w:p><w:r><w:t>merged away</w:t></w:r></w:p></w:tc>";
    let in_custom = format!("<w:customXml w:element=\"c\">{}</w:customXml>", cell(&p("in custom XML")));
    let first = format!(
        "{}{}{}{}<w:tc><w:tcPr><w:cellIns w:id=\"93\" w:author=\"D\"/></w:tcPr></w:tc><w:tc/>",
        cell(&p("x")),
        merged,
        sdt("cell", &(cell(&p("in control")) + merged)) + &sdt("cells", &(cell(&p("ctl a")) + &cell(&p("ctl b")))),
        in_custom,
    );
    let cells = table(&format!(
        "{}{}{}",
        row(&first),
        // As many cells as a row may have (more: the file is refused, `tests_limits`).
        row(&(0..wordcraft_doc::table::MAX_COLS).map(|i| cell(&p(&format!("c{i}")))).collect::<String>()),
        row(&(cell(&table(&row(&cell(&p("nested"))))) + &cell(&p("beside")))),
    ));
    // Tables nested deep (deeper than a reader takes: the file is refused, `tests_limits`).
    let mut deep = p("core");
    for _ in 0..20 {
        deep = table(&row(&cell(&deep)));
    }
    // Content controls around blocks: starting and ending with tables, nested, without content,
    // with two contents (the first is read), in custom XML, smart tags and insertions.
    let outer = format!("{}{}{}", simple, sdt("inner", &(p("in inner") + &p("more"))), simple);
    let grouped = format!(
        "<w:customXml w:element=\"g\"><w:customXmlPr/>{}<w:smartTag w:element=\"s\">{}</w:smartTag></w:customXml>",
        sdt("grouped", &p("g")),
        p("tagged")
    );
    let changed = format!(
        "<w:ins w:id=\"94\" w:author=\"E\">{}</w:ins><w:moveTo w:id=\"95\" w:author=\"E\">{}</w:moveTo>",
        p("inserted"),
        table(&row(&cell(&p("moved"))))
    );
    let controls = format!(
        "{}{}{}{}{}",
        sdt("outer", &outer),
        "<w:sdt><w:sdtPr><w:alias w:val=\"none\"/></w:sdtPr></w:sdt>",
        "<w:sdt><w:sdtPr/><w:sdtContent><w:p><w:r><w:t>first content</w:t></w:r></w:p></w:sdtContent><w:sdtContent><w:p><w:r><w:t>second</w:t></w:r></w:p></w:sdtContent></w:sdt>",
        grouped,
        changed,
    );
    // Markers between blocks (a bookmark around a table: its end goes to the table's last
    // paragraph), a drop cap, text and a second body the reader leaves, alternate content.
    let markers = format!(
        "<w:bookmarkStart w:id=\"1\" w:name=\"around\"/>{simple}<w:bookmarkEnd w:id=\"1\"/>{}{}",
        "<w:p><w:pPr><w:framePr w:dropCap=\"drop\" w:lines=\"3\"/></w:pPr><w:r><w:t>D</w:t></w:r></w:p>",
        p("rop cap")
    );
    let alternate = format!(
        "<mc:AlternateContent><mc:Choice Requires=\"w14\">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>",
        p("choice"),
        p("fallback")
    );
    let notes = "<w:p><w:r><w:t>noted</w:t></w:r><w:r><w:footnoteReference w:id=\"2\"/></w:r><w:r><w:endnoteReference w:id=\"3\"/></w:r></w:p>";
    let commented = "<w:p><w:commentRangeStart w:id=\"5\"/><w:r><w:t>commented</w:t></w:r><w:commentRangeEnd w:id=\"5\"/><w:r><w:commentReference w:id=\"5\"/></w:r></w:p><w:p><w:commentRangeStart w:id=\"6\"/><w:r><w:t>reply</w:t></w:r><w:commentRangeEnd w:id=\"6\"/><w:r><w:commentReference w:id=\"6\"/></w:r></w:p>";
    let section_break = "<w:p><w:pPr><w:sectPr><w:headerReference w:type=\"default\" r:id=\"rIdH1\"/><w:pgSz w:w=\"11906\" w:h=\"16838\"/></w:sectPr></w:pPr><w:r><w:t>end of section one</w:t></w:r></w:p>";
    format!(
        "{}text the reader leaves{simple}{sections}{cells}{deep}{controls}{markers}{alternate}{notes}{commented}{section_break}{}<w:sectPr><w:headerReference w:type=\"default\" r:id=\"rIdH2\"/><w:footerReference w:type=\"default\" r:id=\"rIdF1\"/><w:pgSz w:w=\"12240\" w:h=\"15840\"/></w:sectPr><w:sectPr><w:pgSz w:w=\"100\" w:h=\"100\"/></w:sectPr>",
        p("start"),
        p("last paragraph"),
    )
}

/// A package with `body` and every other story there is.
fn package(body: &str) -> Vec<u8> {
    let doc = format!("<w:document {NS}><w:background w:color=\"FFEEDD\"/><w:body>{body}</w:body><w:body>{}</w:body></w:document>", p("second body"));
    let story = |root: &str| format!("<w:{root} {NS}>{}{}{}</w:{root}>", p("story"), table(&row(&cell(&p("in table")))), sdt("hf", &p("in control")));
    let notes = |kind: &str, id: &str| {
        format!(
            "<w:{kind}s {NS}><w:{kind} w:type=\"separator\" w:id=\"-1\"><w:p><w:r><w:separator/></w:r></w:p></w:{kind}><w:{kind} w:type=\"continuationSeparator\" w:id=\"0\"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:{kind}><w:{kind} w:type=\"separator\" w:id=\"1\">{}</w:{kind}><w:{kind} w:id=\"{id}\">{}{}</w:{kind}><w:{kind} w:id=\"7\"><w:p><w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r></w:p></w:{kind}><w:{kind} w:id=\"8\">{}</w:{kind}><w:{kind} w:type=\"other\" w:id=\"9\">{}</w:{kind}></w:{kind}s>",
            p("second separator"),
            p("note"),
            table(&row(&cell(&p("note table")))),
            p("after a field that doesn't end"),
            p("other"),
        )
    };
    let comments = format!(
        "<w:comments {NS}><w:comment w:id=\"5\" w:author=\"A\" w:initials=\"a\" w:date=\"2024-01-01T00:00:00Z\"><w:p w14:paraId=\"11111111\"><w:r><w:t>first</w:t></w:r></w:p>{}<w:p w14:paraId=\"22222222\"><w:r><w:t>last</w:t></w:r></w:p></w:comment><w:comment w:id=\"6\" w:author=\"B\"><w:customXml w:element=\"x\"><w:p w14:paraId=\"33333333\"><w:r><w:t>grouped</w:t></w:r></w:p></w:customXml><w:p w14:paraId=\"44444444\"><w:r><w:t>reply</w:t></w:r></w:p></w:comment><w:comment w:id=\"5\" w:author=\"again\">{}</w:comment><w:comment w:id=\"x\" w:author=\"C\">{}</w:comment></w:comments>",
        table(&row(&cell(&p("comment table")))),
        p("same id"),
        p("no number"),
    );
    let ext = r#"<w15:commentsEx xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml"><w15:commentEx w15:paraId="22222222"/><w15:commentEx w15:paraId="44444444" w15:paraIdParent="22222222" w15:done="1"/></w15:commentsEx>"#;
    let rel = |id: &str, kind: &str, target: &str| format!(r#"<Relationship Id="{id}" Type="{REL}/{kind}" Target="{target}"/>"#);
    let rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{}{}{}{}{}{}<Relationship Id="rIdX" Type="http://schemas.microsoft.com/office/2011/relationships/commentsExtended" Target="commentsExtended.xml"/></Relationships>"#,
        rel("rIdH1", "header", "header1.xml"),
        rel("rIdH2", "header", "header2.xml"),
        rel("rIdF1", "footer", "footer1.xml"),
        rel("rIdN1", "footnotes", "footnotes.xml"),
        rel("rIdN2", "endnotes", "endnotes.xml"),
        rel("rIdC", "comments", "comments.xml"),
    );
    let files: Vec<(String, String)> = vec![
        ("_rels/.rels".into(), ROOT_RELS.into()),
        ("word/document.xml".into(), doc),
        ("word/_rels/document.xml.rels".into(), rels),
        ("word/header1.xml".into(), story("hdr")),
        ("word/header2.xml".into(), story("hdr")),
        ("word/footer1.xml".into(), story("ftr")),
        ("word/footnotes.xml".into(), notes("footnote", "2")),
        ("word/endnotes.xml".into(), notes("endnote", "3")),
        ("word/comments.xml".into(), comments),
        ("word/commentsExtended.xml".into(), ext.into()),
    ];
    zip(&files)
}

/// The document read from whole trees, and read a block at a time with `budget` nodes held at
/// most (`None`: the file's budget).
fn both(file: &[u8], budget: Option<usize>) -> (wordcraft_doc::Document, wordcraft_doc::Document) {
    WHOLE_TREES.with(|w| w.set(true));
    let whole = read_opened_budgeted(&Package::open(file).unwrap(), 1_000_000);
    WHOLE_TREES.with(|w| w.set(false));
    let mut pkg = Package::open(file).unwrap();
    if let Some(n) = budget {
        pkg = pkg.with_node_budget(n);
    }
    let streamed = read_opened_budgeted(&pkg, 1_000_000);
    (whole.expect("reads whole"), streamed.expect("reads a block at a time"))
}

#[test]
fn streamed_stories_read_as_whole_trees_do() {
    let file = package(&body());
    let (whole, streamed) = both(&file, None);
    // What the comparison covers is there.
    let mut text = whole.plain_text(wordcraft_doc::StoryRef::Body);
    for id in whole.parts.keys() {
        text.push_str(&whole.plain_text(wordcraft_doc::StoryRef::Part(*id)));
    }
    for t in [
        "in inner",
        "one",
        "two",
        "last",
        "in control",
        "c62",
        "nested",
        "first content",
        "tagged",
        "moved",
        "choice",
        "rop cap",
        "end of section one",
    ] {
        assert!(text.contains(t), "{t} missing from {text}");
    }
    for t in ["merged away", "second", "c63", "second body", "text the reader leaves"] {
        assert!(!text.contains(t), "{t} read");
    }
    // The nested tables are read as deep as they go.
    let deepest = whole.body.iter().find_map(|b| {
        let mut depth = 0;
        let mut b = b.clone();
        while let wordcraft_doc::Block::Table(t) = &*b {
            depth += 1;
            b = t.rows.first()?.cells.first()?.blocks.first()?.clone();
        }
        (depth >= 20).then(|| (depth, b.as_para().map(|p| p.text.clone())))
    });
    assert_eq!(deepest, Some((20, Some("core".to_string()))));
    assert_eq!(whole.parts.len(), 12, "three headers and footers, six notes and three comments");
    assert_eq!(whole.comments.len(), 3);
    assert!(whole.comments.values().any(|c| c.resolved && c.parent.is_some()));
    assert!(whole.footnote_separators.separator.is_some() && whole.footnote_separators.continuation_separator.is_some());
    assert_eq!(whole.last_section.page_w, 612.0);
    assert!(whole.settings.page_color.is_some());
    assert_eq!(whole, streamed);
}

#[test]
fn a_story_holds_what_one_block_takes() {
    // The whole document held a tree at a time takes far more than the largest block, also
    // when its blocks are deep in what groups them, a content control and a table's cell.
    let many = p("x").repeat(20_000);
    let wrapped = format!(
        "<w:customXml w:element=\"g\"><w:smartTag w:element=\"s\">{}</w:smartTag></w:customXml>",
        sdt("all", &table(&sdt("rows", &row(&sdt("cells", &cell(&many))))))
    );
    let blocks = format!("{}{many}{wrapped}", body());
    let file = package(&blocks);
    let pkg = Package::open(&file).unwrap().with_node_budget(60_000);
    WHOLE_TREES.with(|w| w.set(true));
    assert!(read_opened_budgeted(&pkg, 1_000_000).is_err(), "whole trees don't fit");
    WHOLE_TREES.with(|w| w.set(false));
    let (whole, streamed) = both(&file, Some(60_000));
    assert_eq!(whole, streamed);
}

#[test]
fn writer_output_reads_the_same_either_way() {
    let doc = super::tests_limits::formatted_document(200, 10);
    let file = crate::write(&doc).unwrap();
    let (whole, streamed) = both(&file, Some(20_000));
    assert_eq!(whole, streamed);
    assert_eq!(whole.body.len(), doc.body.len());
}
