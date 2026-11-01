//! Hostile input: every case must return `Err` or a document, never panic.

use std::io::Write;

use proptest::prelude::*;
use wordcraft_docx::{DocxError, read, write};

const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
const ROOT_RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        zw.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(bytes).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

fn doc_zip(document_xml: &[u8]) -> Vec<u8> {
    zip(&[("_rels/.rels", ROOT_RELS.as_bytes()), ("word/document.xml", document_xml)])
}

fn sample() -> Vec<u8> {
    let mut d = wordcraft_doc::Document::from_text("Hello world\nSecond paragraph with more text\nThird");
    let t = wordcraft_doc::Table::new(2, 2, 200.0);
    d.body.insert(1, std::sync::Arc::new(wordcraft_doc::Block::Table(t)));
    write(&d).unwrap()
}

#[test]
fn random_bytes_and_empty_input() {
    assert!(matches!(read(&[]), Err(DocxError::Zip(_))));
    assert!(read(b"PK\x03\x04garbage").is_err());
    assert!(read(&[0u8; 4096]).is_err());
    let noise: Vec<u8> = (0..10_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    assert!(read(&noise).is_err());
}

#[test]
fn truncated_zip_never_panics() {
    let good = sample();
    assert!(read(&good).is_ok());
    for cut in (0..good.len()).step_by(37) {
        let _ = read(&good[..cut]);
    }
    // Flipped bytes throughout.
    for i in (0..good.len()).step_by(53) {
        let mut b = good.clone();
        b[i] ^= 0xA5;
        let _ = read(&b);
    }
}

#[test]
fn missing_or_wrong_main_part() {
    let z = zip(&[("_rels/.rels", ROOT_RELS.as_bytes()), ("word/other.xml", b"<x/>")]);
    assert!(matches!(read(&z), Err(DocxError::MissingPart(_))));
    let z = doc_zip(b"<notword/>");
    assert!(matches!(read(&z), Err(DocxError::NotWord(_))));
    let z = doc_zip(b"<w:document xmlns:w=\"x\"><unclosed");
    let _ = read(&z);
    let z = doc_zip(b"\xff\xfe<\0w\0");
    assert!(read(&z).is_err());
    // No root rels at all: falls back to word/document.xml.
    let z = zip(&[("word/document.xml", format!("<w:document {W_NS}><w:body><w:p><w:r><w:t>ok</w:t></w:r></w:p></w:body></w:document>").as_bytes())]);
    let d = read(&z).unwrap();
    assert_eq!(d.plain_text(wordcraft_doc::StoryRef::Body), "ok");
}

fn body_zip(body: &str) -> Vec<u8> {
    doc_zip(format!("<w:document {W_NS}><w:body>{body}</w:body></w:document>").as_bytes())
}

/// #3599 (upstream 7dbb79ad): a malformed or cut-off main document part is an open error, never a
/// document that is silently shorter than the file. Before the fix the lenient reader closed the
/// open elements at the end of the data and the user saw a truncated document with no warning.
#[test]
fn a_malformed_main_part_is_an_error_not_a_truncated_document() {
    let cases = [
        ("a less-than sign in a run", "<w:p><w:r><w:t>before a <- b after</w:t></w:r></w:p><w:p><w:r><w:t>last</w:t></w:r></w:p>"),
        ("a paragraph left open", "<w:p><w:r><w:t>first</w:t></w:r></w:p><w:p><w:r><w:t>second</w:t></w:r><w:p><w:r><w:t>third</w:t></w:r></w:p>"),
        ("a mismatched end tag", "<w:p><w:r><w:t>one</w:t></w:p></w:r><w:p><w:r><w:t>two</w:t></w:r></w:p>"),
    ];
    for (what, body) in cases {
        let err = read(&body_zip(body)).err();
        assert!(matches!(err, Some(DocxError::Xml(_))), "{what}: {err:?}");
    }
    for body in ["<w:p><w:r><w:t>kept&unknown;lost</w:t></w:r></w:p>", "<w:p w:rsidR=\"1\" w:rsidR=\"2\"><w:r><w:t>text</w:t></w:r></w:p>"] {
        assert!(matches!(read(&body_zip(body)), Err(DocxError::Xml(_))), "invalid XML must not silently drop content or attributes");
    }
    // A file cut off in the middle (a copy that stopped, a bad sector): the part ends with two
    // paragraphs open. Lenient reading made "one" the whole document.
    let whole = format!(
        "<w:document {W_NS}><w:body><w:p><w:r><w:t>one</w:t></w:r></w:p><w:p><w:r><w:t>two</w:t></w:r></w:p><w:p><w:r><w:t>three</w:t></w:r></w:p></w:body></w:document>"
    );
    let cut = whole.find("two</w:t>").unwrap() + 1;
    let err = read(&doc_zip(&whole.as_bytes()[..cut])).err();
    assert!(matches!(err, Some(DocxError::Xml(_))), "cut-off part: {err:?}");
    let msg = err.map(|e| e.to_string()).unwrap_or_default();
    assert!(msg.contains("unclosed") && msg.contains("w:t"), "the error names the open element: {msg}");
    // Well-formed text still reads whole, escapes included.
    let d = read(&body_zip("<w:p><w:r><w:t>a &lt;- b</w:t></w:r></w:p><w:p><w:r><w:t>last</w:t></w:r></w:p>")).unwrap();
    assert_eq!(d.plain_text(wordcraft_doc::StoryRef::Body), "a <- b\nlast");
    let d = read(&doc_zip(whole.as_bytes())).unwrap();
    assert_eq!(d.plain_text(wordcraft_doc::StoryRef::Body), "one\ntwo\nthree");
}

/// Leniency stays where it is deliberate: a secondary part left open (here the styles part) costs
/// at most formatting, so the body still opens whole.
#[test]
fn an_unclosed_secondary_part_still_opens_the_body() {
    let main_rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;
    let styles = format!("<w:styles {W_NS}><w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:rPr><w:b/>");
    let document = format!(
        "<w:document {W_NS}><w:body><w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p><w:p><w:r><w:t>Body text</w:t></w:r></w:p></w:body></w:document>"
    );
    let z = zip(&[
        ("_rels/.rels", ROOT_RELS.as_bytes()),
        ("word/_rels/document.xml.rels", main_rels.as_bytes()),
        ("word/document.xml", document.as_bytes()),
        ("word/styles.xml", styles.as_bytes()),
    ]);
    let d = read(&z).unwrap();
    assert_eq!(d.plain_text(wordcraft_doc::StoryRef::Body), "Title\nBody text");
}

#[test]
fn deep_nesting_is_rejected() {
    let mut x = format!("<w:document {W_NS}><w:body>");
    for _ in 0..5_000 {
        x.push_str("<w:sdt><w:sdtContent>");
    }
    x.push_str("</w:body></w:document>");
    assert!(matches!(read(&doc_zip(x.as_bytes())), Err(DocxError::Limit(_))));
    // Tables nested 60 deep (under the XML depth limit, past the 24 the reader keeps): refused
    // without blowing the stack, rather than read with the deeper ones flattened.
    let nested = |n: usize| {
        let mut y = format!("<w:document {W_NS}><w:body>");
        for _ in 0..n {
            y.push_str("<w:tbl><w:tr><w:tc>");
        }
        y.push_str("<w:p><w:r><w:t>deep</w:t></w:r></w:p>");
        for _ in 0..n {
            y.push_str("</w:tc></w:tr></w:tbl>");
        }
        y.push_str("</w:body></w:document>");
        y
    };
    assert!(matches!(read(&doc_zip(nested(60).as_bytes())), Err(DocxError::Limit(_))));
    // As deep as the reader keeps, they build and write.
    let d = read(&doc_zip(nested(24).as_bytes())).unwrap();
    fn all_text(bl: &wordcraft_doc::Blocks, out: &mut String) {
        for b in bl {
            match &**b {
                wordcraft_doc::Block::Para(p) => out.push_str(&p.plain_text()),
                wordcraft_doc::Block::Table(t) => t.rows.iter().flat_map(|r| &r.cells).for_each(|c| all_text(&c.blocks, out)),
            }
        }
    }
    let mut s = String::new();
    all_text(&d.body, &mut s);
    assert!(s.contains("deep"), "{s}");
    write(&d).unwrap();
}

#[test]
fn broken_secondary_parts_are_ignored() {
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="a" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="b" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/><Relationship Id="c" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="comments.xml"/></Relationships>"#;
    let doc = format!("<w:document {W_NS}><w:body><w:p><w:r><w:t>body</w:t></w:r></w:p></w:body></w:document>");
    let z = zip(&[
        ("_rels/.rels", ROOT_RELS.as_bytes()),
        ("word/document.xml", doc.as_bytes()),
        ("word/_rels/document.xml.rels", rels.as_bytes()),
        ("word/styles.xml", b"\xff\xfe"),
        ("word/numbering.xml", b"<w:numbering><w:num w:numId=\"1\"><w:abstractNumId w:val=\"999\"/></w:num></w:numbering>"),
        ("word/comments.xml", b"<<<"),
    ]);
    let d = read(&z).unwrap();
    assert_eq!(d.plain_text(wordcraft_doc::StoryRef::Body), "body");
    // The unresolvable list still writes.
    write(&d).unwrap();
}

#[test]
fn many_fields_and_huge_attribute_values() {
    let file = |begins: usize| {
        let mut x = format!("<w:document {W_NS}><w:body><w:p>");
        for _ in 0..begins {
            x.push_str(r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#);
        }
        x.push_str(&format!(r#"<w:r><w:rPr><w:rFonts w:ascii="{}"/></w:rPr><w:t>x</w:t></w:r>"#, "F".repeat(100_000)));
        x.push_str("</w:p><w:p><w:r><w:t>later</w:t></w:r></w:p></w:body></w:document>");
        doc_zip(x.as_bytes())
    };
    // 200 fields open inside one another: more than the 32 the reader keeps, so refused.
    assert!(matches!(read(&file(200)), Err(DocxError::Limit(_))));
    // 30 (unfinished) and a 100,000-character font name read.
    let d = read(&file(30)).unwrap();
    assert!(d.plain_text(wordcraft_doc::StoryRef::Body).contains("later"));
}

/// A small valid document.xml to mutate.
fn base_xml() -> String {
    format!(
        r#"<w:document {W_NS}><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:rPr><w:b/><w:sz w:val="24"/></w:rPr><w:t>Hi</w:t></w:r><w:hyperlink w:anchor="x"><w:r><w:t>link</w:t></w:r></w:hyperlink><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:bookmarkStart w:id="0" w:name="x"/><w:bookmarkEnd w:id="0"/></w:p><w:tbl><w:tblPr><w:tblLook w:val="04A0"/></w:tblPr><w:tblGrid><w:gridCol w:w="100"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/><w:vMerge/></w:tcPr><w:p/></w:tc></w:tr></w:tbl><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:cols w:num="2"/></w:sectPr></w:body></w:document>"#
    )
}

const SNIPPETS: &[&str] = &[
    "<w:p>",
    "</w:p>",
    "<w:tbl>",
    "<w:tr>",
    "<w:tc>",
    "</w:tc>",
    r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
    r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
    r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>ADDIN ZOTERO_ITEM {}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
    r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
    r#"<w:br w:type="page"/>"#,
    r#"<w:sz w:val="-99999999999"/>"#,
    r#"<w:gridSpan w:val="0"/>"#,
    "&amp;",
    "&#0;",
    "&#xFFFF;",
    "<![CDATA[x]]>",
    "\u{FFFC}",
    r#"<w:footnoteReference w:id="1"/>"#,
    r#"<w:drawing><wp:inline/></w:drawing>"#,
    r#"<mc:AlternateContent><mc:Choice/></mc:AlternateContent>"#,
    "<w:sectPr/>",
];

#[test]
fn malformed_or_hostile_bibliography_never_panics() {
    let doc = format!("<w:document {W_NS}><w:body><w:p><w:r><w:t>body</w:t></w:r></w:p></w:body></w:document>");
    let cases: &[&[u8]] = &[
        b"<not_sources/>",
        b"<b:Sources xmlns:b=\"http://schemas.openxmlformats.org/officeDocument/2006/bibliography\"><unclosed",
        b"<b:Sources xmlns:b=\"http://schemas.openxmlformats.org/officeDocument/2006/bibliography\"><b:Source/></b:Sources>",
        b"<b:Sources xmlns:b=\"http://schemas.openxmlformats.org/officeDocument/2006/bibliography\"><b:Source><b:Author><b:NameList><b:Person/></b:NameList></b:Author></b:Source></b:Sources>",
        b"<b:Sources xmlns:b=\"http://schemas.openxmlformats.org/officeDocument/2006/bibliography\"><b:Source><b:Tag></b:Tag><b:SourceType></b:SourceType><b:Author><b:NameList><b:Person><b:Last>,,,,</b:Last><b:First></b:First></b:Person></b:NameList></b:Author></b:Source></b:Sources>",
        b"<b:Sources xmlns:b=\"http://schemas.openxmlformats.org/officeDocument/2006/bibliography\"><b:Source><b:Author><b:Editor><b:NameList><b:Person><b:Last>Ed</b:Last></b:Person></b:NameList></b:Editor></b:Author><b:Edition>1st</b:Edition><b:Issue>99</b:Issue><b:Month>13</b:Month><b:Day>32</b:Day><b:YearAccessed>2025</b:YearAccessed><b:MonthAccessed>02</b:MonthAccessed><b:DayAccessed>30</b:DayAccessed><b:StandardNumber>doi:10.0/0</b:StandardNumber></b:Source></b:Sources>",
        b"<b:Sources xmlns:b=\"http://schemas.openxmlformats.org/officeDocument/2006/bibliography\"><b:Source><b:Tag>Hostile1</b:Tag><b:Editor><b:Person/></b:Editor><b:Accessed>---///---</b:Accessed></b:Source></b:Sources>",
        b"\xff\xfe<\0b\0:\0S\0",
    ];
    let root_rels_bib = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml" Target="customXml/item1.xml"/></Relationships>"#;
    for case in cases {
        let z = zip(&[("_rels/.rels", root_rels_bib.as_bytes()), ("word/document.xml", doc.as_bytes()), ("customXml/item1.xml", case)]);
        let res = read(&z);
        if let Ok(d) = res {
            let _ = write(&d);
        }
    }
}

/// A file with a huge number of bibliography sources (duplicates and distinct ones) reads in
/// linear time: up to the 10,000 a document may have it opens whole, past them it is refused
/// (it used to open with the first 10,000).
#[test]
fn hostile_bibliography_source_count_is_capped() {
    let doc = format!("<w:document {W_NS}><w:body><w:p><w:r><w:t>body</w:t></w:r></w:p></w:body></w:document>");
    let file = |tagged: usize| {
        let mut sources = String::from(r#"<b:Sources xmlns:b="http://schemas.openxmlformats.org/officeDocument/2006/bibliography" StyleName="MLA">"#);
        for i in 0..tagged {
            sources.push_str(&format!("<b:Source><b:Tag>T{i}</b:Tag><b:Title>Title {i}</b:Title></b:Source>"));
            // Untagged duplicates all collapse into one source.
            sources.push_str("<b:Source><b:Title>Same</b:Title></b:Source>");
        }
        sources.push_str("</b:Sources>");
        let root_rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml" Target="customXml/item1.xml"/></Relationships>"#;
        zip(&[("_rels/.rels", root_rels.as_bytes()), ("word/document.xml", doc.as_bytes()), ("customXml/item1.xml", sources.as_bytes())])
    };
    let start = std::time::Instant::now();
    assert!(matches!(read(&file(60_000)), Err(DocxError::Limit(_))));
    let d = read(&file(wordcraft_doc::limits::SOURCES - 1)).unwrap();
    assert_eq!(d.sources.len(), wordcraft_doc::limits::SOURCES);
    assert_eq!(d.sources.iter().filter(|s| s.tag.is_empty()).count(), 1, "untagged duplicates are kept once");
    assert_eq!(d.settings.bib_style.as_deref(), Some("MLA"));
    assert!(start.elapsed() < std::time::Duration::from_secs(30), "took {:?}", start.elapsed());
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    #[test]
    fn mutated_document_xml_never_panics(ops in proptest::collection::vec((0u8..4, any::<prop::sample::Index>(), any::<u8>(), 0usize..SNIPPETS.len()), 0..12)) {
        let mut x = base_xml().into_bytes();
        for (k, idx, byte, snip) in ops {
            let at = idx.index(x.len() + 1);
            match k {
                0 => { if at < x.len() { x[at] = byte; } }
                1 => { x.truncate(at); }
                2 => { let s = SNIPPETS[snip].as_bytes(); let at = at.min(x.len()); x.splice(at..at, s.iter().copied()); }
                _ => { if at < x.len() { x.remove(at); } }
            }
        }
        if let Ok(d) = read(&doc_zip(&x)) {
            // Whatever we managed to read must write back and read again.
            let out = write(&d).unwrap();
            read(&out).unwrap();
        }
    }

    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = read(&bytes);
        let mut z = b"PK\x03\x04".to_vec();
        z.extend(&bytes);
        let _ = read(&z);
    }
}

/// The streams of a Standard-encrypted package (gap 9), and a compound file holding them.
fn standard_streams() -> (Vec<u8>, Vec<u8>) {
    use std::io::Read;
    let file = wordcraft_docx::encrypt_standard_for_tests(&sample(), "pw", 128).unwrap();
    let mut comp = cfb::CompoundFile::open(std::io::Cursor::new(file)).unwrap();
    let mut info = Vec::new();
    comp.open_stream("/EncryptionInfo").unwrap().read_to_end(&mut info).unwrap();
    let mut package = Vec::new();
    comp.open_stream("/EncryptedPackage").unwrap().read_to_end(&mut package).unwrap();
    (info, package)
}

fn compound(info: &[u8], package: &[u8]) -> Vec<u8> {
    let mut comp = cfb::CompoundFile::create(std::io::Cursor::new(Vec::new())).unwrap();
    comp.create_stream("/EncryptionInfo").unwrap().write_all(info).unwrap();
    comp.create_stream("/EncryptedPackage").unwrap().write_all(package).unwrap();
    comp.flush().unwrap();
    comp.into_inner().into_inner()
}

proptest! {
    // Each case that parses runs the 50,000-spin key derivation, so fewer cases.
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn mutated_standard_encryption_never_panics(ops in proptest::collection::vec((0u8..3, any::<prop::sample::Index>(), any::<u8>(), any::<bool>()), 1..8)) {
        let (mut info, mut package) = standard_streams();
        for (k, idx, byte, in_package) in ops {
            let x = if in_package { &mut package } else { &mut info };
            let at = idx.index(x.len() + 1);
            match k {
                0 => { if at < x.len() { x[at] = byte; } }
                1 => { x.truncate(at); }
                _ => { x.splice(at..at, std::iter::repeat_n(byte, usize::from(byte % 9))); }
            }
        }
        let file = compound(&info, &package);
        let _ = wordcraft_docx::decrypt(&file, Some("pw"));
        let _ = wordcraft_docx::read_with_password(&file, Some("pw"));
        prop_assert!(read(&file).is_err());
    }
}

#[test]
fn long_header_reference_chain_is_cut_off() {
    const HDR_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
    const RELS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
    let rels_for = |next: usize| {
        let rel = format!(r#"<Relationship Id="rId1" Type="{HDR_REL}" Target="header{next}.xml"/>"#);
        format!(r#"<Relationships xmlns="{RELS_NS}">{rel}</Relationships>"#)
    };
    let sect = r#"<w:sectPr><w:headerReference w:type="default" r:id="rId1"/></w:sectPr>"#;
    let doc = format!("<w:document {W_NS}><w:body><w:p><w:r><w:t>body</w:t></w:r></w:p>{sect}</w:body></w:document>");
    // header0 -> header1 -> ... -> header59: every header's last paragraph holds a section that points at the next one.
    let mut entries: Vec<(String, Vec<u8>)> = vec![
        ("_rels/.rels".into(), ROOT_RELS.as_bytes().to_vec()),
        ("word/document.xml".into(), doc.into_bytes()),
        ("word/_rels/document.xml.rels".into(), rels_for(0).into_bytes()),
    ];
    for i in 0..60 {
        let hdr = format!("<w:hdr {W_NS}><w:p><w:pPr>{sect}</w:pPr><w:r><w:t>h{i}</w:t></w:r></w:p></w:hdr>");
        entries.push((format!("word/header{i}.xml"), hdr.into_bytes()));
        entries.push((format!("word/_rels/header{i}.xml.rels"), rels_for(i + 1).into_bytes()));
    }
    let refs: Vec<(&str, &[u8])> = entries.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    // Not followed past a few, and the file is refused rather than read without the rest.
    assert!(matches!(read(&zip(&refs)), Err(DocxError::Limit(_))));
}

/// A valid first root must not hide a second document or a broken tail (#3599).
#[test]
fn strict_main_part_checks_after_the_root_closes() {
    let main = format!("<w:document {W_NS}><w:body><w:p><w:r><w:t>kept</w:t></w:r></w:p></w:body></w:document>");
    for suffix in ["<w:document>", "<another/>", "lost text", "<![CDATA[lost text]]>", "&amp;"] {
        let err = read(&doc_zip(format!("{main}{suffix}").as_bytes())).err();
        assert!(matches!(err, Some(DocxError::Xml(_))), "tail {suffix:?} must not be silently dropped: {err:?}");
    }
    // Whitespace and XML comments outside the root are legal.
    let doc = read(&doc_zip(format!(" \n{main} \n<!--end-->").as_bytes())).unwrap();
    assert_eq!(doc.plain_text(wordcraft_doc::StoryRef::Body), "kept");
}

#[test]
fn strict_main_part_requires_bound_namespace_prefixes() {
    for main in [
        format!(
            "<w:document {W_NS}><w:body><w:p><w:r><w:t>kept</w:t></w:r></w:p><missing:p><w:r><w:t>must not disappear</w:t></w:r></missing:p></w:body></w:document>"
        ),
        format!("<w:document {W_NS}><w:body><w:p missing:attribute=\"x\"><w:r><w:t>must not disappear</w:t></w:r></w:p></w:body></w:document>"),
    ] {
        let error = read(&doc_zip(main.as_bytes())).err();
        assert!(matches!(error, Some(DocxError::Xml(_))), "unbound prefixes cannot silently shorten the document: {error:?}");
    }
    let main = format!(
        "<w:document {W_NS} xmlns:ext=\"urn:synthetic-extension\"><w:body><w:p ext:attribute=\"x\"><w:r><w:t>kept</w:t></w:r></w:p></w:body></w:document>"
    );
    let doc = read(&doc_zip(main.as_bytes())).unwrap();
    assert_eq!(doc.plain_text(wordcraft_doc::StoryRef::Body), "kept", "a bound extension namespace is valid");
}
