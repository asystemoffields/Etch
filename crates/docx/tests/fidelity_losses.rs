//! Markup an earlier audit found lost on open → save: OLE objects without a preview picture,
//! parts reached by custom relationship types, move tracking, `w:specVanish`, smart tag and
//! custom XML wrappers, and absolute position tabs. Each test reads a hand-written package
//! (from ECMA-376, no files produced by Word), saves it, and checks the saved markup and a
//! second open.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use wordcraft_doc::Document;

const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office""#;
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn zip(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        zw.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(bytes).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

fn unzip(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let mut b = Vec::new();
        f.read_to_end(&mut b).unwrap();
        out.insert(f.name().to_string(), b);
    }
    out
}

fn rels(list: &[(&str, &str, &str)]) -> Vec<u8> {
    let mut s =
        String::from(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
    for (id, ty, target) in list {
        s.push_str(&format!(r#"<Relationship Id="{id}" Type="{ty}" Target="{target}"/>"#));
    }
    s.push_str("</Relationships>");
    s.into_bytes()
}

/// A package with `body`, the document's relationships and extra parts.
fn package(body: &str, doc_rels: &[(&str, &str, &str)], extra: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let doc = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NS}><w:body>{body}</w:body></w:document>"#);
    let root = rels(&[("rId1", &format!("{REL}/officeDocument"), "word/document.xml")]);
    let mut entries = vec![("_rels/.rels", root), ("word/document.xml", doc.into_bytes()), ("word/_rels/document.xml.rels", rels(doc_rels))];
    for (p, c) in extra {
        entries.push((p, c.clone()));
    }
    zip(&entries)
}

/// Open → save: the saved package's files.
fn resave(bytes: &[u8]) -> (Document, BTreeMap<String, Vec<u8>>) {
    let doc = wordcraft_docx::read(bytes).unwrap();
    let out = wordcraft_docx::write(&doc).unwrap();
    (doc, unzip(&out))
}

fn file(files: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
    String::from_utf8_lossy(files.get(name).unwrap_or_else(|| panic!("no {name} in {:?}", files.keys()))).into_owned()
}

/// Save → open → save again, for markup that must survive more than one save.
fn twice(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let (doc, _) = resave(bytes);
    let again = wordcraft_docx::read(&wordcraft_docx::write(&doc).unwrap()).unwrap();
    unzip(&wordcraft_docx::write(&again).unwrap())
}

#[test]
fn ole_object_without_a_preview_picture_is_kept_with_its_embedding() {
    let body = r#"<w:p><w:r><w:t>Before</w:t></w:r><w:r><w:object w:dxaOrig="2880" w:dyaOrig="1440"><o:OLEObject Type="Embed" ProgID="Package" ShapeID="_x0000_i1025" DrawAspect="Icon" ObjectID="_1234" r:id="rId9"/></w:object></w:r><w:r><w:t>After</w:t></w:r></w:p>"#;
    let bin = b"\xd0\xcf\x11\xe0synthetic ole bytes".to_vec();
    let bytes =
        package(body, &[("rId9", &format!("{REL}/oleObject"), "embeddings/oleObject1.bin")], &[("word/embeddings/oleObject1.bin", bin.clone())]);
    for files in [resave(&bytes).1, twice(&bytes)] {
        let doc = file(&files, "word/document.xml");
        assert!(doc.contains("o:OLEObject") && doc.contains("ProgID=\"Package\""), "{doc}");
        let kept = files.iter().find(|(k, _)| k.starts_with("word/embeddings/")).map(|(_, v)| v.clone());
        assert_eq!(kept, Some(bin.clone()), "embedding not carried: {:?}", files.keys());
        let doc_rels = file(&files, "word/_rels/document.xml.rels");
        assert!(doc_rels.contains("/oleObject\""), "{doc_rels}");
        assert!(doc.find("Before") < doc.find("OLEObject") && doc.find("OLEObject") < doc.find("After"), "{doc}");
    }
}

#[test]
fn custom_relationships_of_headers_and_notes_are_kept() {
    let hdr = format!(r#"<w:hdr {NS}><w:p><w:r><w:t>Head</w:t></w:r></w:p></w:hdr>"#);
    let body = r#"<w:p><w:r><w:t>Body</w:t></w:r></w:p><w:sectPr><w:headerReference w:type="default" r:id="rIdH"/></w:sectPr>"#;
    let bytes = package(
        body,
        &[("rIdH", &format!("{REL}/header"), "header1.xml")],
        &[
            ("word/header1.xml", hdr.into_bytes()),
            ("word/_rels/header1.xml.rels", rels(&[("rIdX", "urn:example:custom-relationship", "extra/custom1.xml")])),
            ("word/extra/custom1.xml", b"<x:data xmlns:x=\"urn:example\">kept</x:data>".to_vec()),
        ],
    );
    for files in [resave(&bytes).1, twice(&bytes)] {
        let part = files.iter().find(|(_, v)| v.as_slice() == b"<x:data xmlns:x=\"urn:example\">kept</x:data>");
        assert!(part.is_some(), "custom part dropped: {:?}", files.keys());
        let all_rels: String = files.iter().filter(|(k, _)| k.ends_with(".rels")).map(|(_, v)| String::from_utf8_lossy(v).into_owned()).collect();
        assert!(all_rels.contains("urn:example:custom-relationship"), "{all_rels}");
    }
}

#[test]
fn moves_without_range_markers_stay_moves() {
    let body = r#"<w:p><w:moveFrom w:id="1" w:author="Ann" w:date="2026-01-01T00:00:00Z"><w:r><w:t>Moved text</w:t></w:r></w:moveFrom><w:r><w:t> stays</w:t></w:r></w:p><w:p><w:r><w:t>Here: </w:t></w:r><w:moveTo w:id="2" w:author="Ann" w:date="2026-01-01T00:00:00Z"><w:r><w:t>Moved text</w:t></w:r></w:moveTo></w:p>"#;
    let bytes = package(body, &[], &[]);
    let (doc, files) = resave(&bytes);
    assert!(doc.revisions.iter().all(|r| r.move_name.is_some()), "{:?}", doc.revisions);
    let xml = file(&files, "word/document.xml");
    assert!(xml.contains("<w:moveFrom ") && xml.contains("<w:moveTo "), "{xml}");
    let xml = file(&twice(&bytes), "word/document.xml");
    assert!(xml.contains("<w:moveFrom ") && xml.contains("<w:moveTo "), "{xml}");
}

#[test]
fn ranged_moves_stay_moves() {
    let body = r#"<w:p><w:moveFromRangeStart w:id="10" w:name="move1" w:author="Ann" w:date="2026-01-01T00:00:00Z"/><w:moveFrom w:id="1" w:author="Ann" w:date="2026-01-01T00:00:00Z"><w:r><w:t>Moved text</w:t></w:r></w:moveFrom><w:moveFromRangeEnd w:id="10"/><w:r><w:t> stays</w:t></w:r></w:p><w:p><w:r><w:t>Here: </w:t></w:r><w:moveToRangeStart w:id="11" w:name="move1" w:author="Ann" w:date="2026-01-01T00:00:00Z"/><w:moveTo w:id="2" w:author="Ann" w:date="2026-01-01T00:00:00Z"><w:r><w:t>Moved text</w:t></w:r></w:moveTo><w:moveToRangeEnd w:id="11"/></w:p>"#;
    let xml = file(&twice(&package(body, &[], &[])), "word/document.xml");
    assert!(xml.contains("<w:moveFrom ") && xml.contains("<w:moveTo ") && xml.contains("w:moveFromRangeStart"), "{xml}");
}

#[test]
fn spec_vanish_is_kept() {
    let body = r#"<w:p><w:pPr><w:rPr><w:vanish/><w:specVanish/></w:rPr></w:pPr><w:r><w:t>Heading part</w:t></w:r></w:p><w:p><w:r><w:rPr><w:specVanish/></w:rPr><w:t>Run</w:t></w:r></w:p>"#;
    let xml = file(&twice(&package(body, &[], &[])), "word/document.xml");
    assert_eq!(xml.matches("<w:specVanish/>").count(), 2, "{xml}");
}

#[test]
fn smart_tag_and_custom_xml_wrappers_are_kept() {
    let body = r#"<w:p><w:r><w:t>Meet in </w:t></w:r><w:smartTag w:uri="urn:schemas-microsoft-com:office:smarttags" w:element="place"><w:smartTagPr><w:attr w:name="kind" w:val="city"/></w:smartTagPr><w:r><w:t>Springfield</w:t></w:r></w:smartTag><w:r><w:t> on </w:t></w:r><w:customXml w:uri="urn:example:schema" w:element="date"><w:customXmlPr><w:attr w:name="iso" w:val="2026-01-01"/></w:customXmlPr><w:r><w:t>New Year</w:t></w:r></w:customXml><w:r><w:t>.</w:t></w:r></w:p>"#;
    let bytes = package(body, &[], &[]);
    let (doc, _) = resave(&bytes);
    assert_eq!(doc.body.first().and_then(|b| b.as_para()).unwrap().plain_text(), "Meet in Springfield on New Year.");
    let xml = file(&twice(&bytes), "word/document.xml");
    for needle in [
        r#"w:element="place""#,
        r#"<w:attr w:name="kind" w:val="city"/>"#,
        r#"w:uri="urn:example:schema""#,
        r#"w:element="date""#,
        r#"<w:attr w:name="iso" w:val="2026-01-01"/>"#,
    ] {
        assert!(xml.contains(needle), "{needle} missing: {xml}");
    }
    // The wrapper holds just its own text.
    let tag = xml.split("<w:smartTag ").nth(1).and_then(|s| s.split("</w:smartTag>").next()).unwrap();
    assert!(tag.contains("Springfield") && !tag.contains("Meet") && !tag.contains(" on "), "{tag}");
    let cx = xml.split("<w:customXml ").nth(1).and_then(|s| s.split("</w:customXml>").next()).unwrap();
    assert!(cx.contains("New Year") && !cx.contains("Springfield"), "{cx}");
}

#[test]
fn absolute_position_tabs_are_kept() {
    let body = r#"<w:p><w:r><w:t>Left</w:t></w:r><w:r><w:ptab w:relativeTo="margin" w:alignment="center" w:leader="dot"/><w:t>Centre</w:t></w:r><w:r><w:ptab w:relativeTo="indent" w:alignment="right" w:leader="none"/><w:t>Right</w:t></w:r></w:p>"#;
    let bytes = package(body, &[], &[]);
    let (doc, _) = resave(&bytes);
    assert_eq!(doc.body.first().and_then(|b| b.as_para()).unwrap().plain_text(), "Left\tCentre\tRight");
    let xml = file(&twice(&bytes), "word/document.xml");
    assert!(xml.contains(r#"<w:ptab w:relativeTo="margin" w:alignment="center" w:leader="dot"/>"#), "{xml}");
    assert!(xml.contains(r#"<w:ptab w:relativeTo="indent" w:alignment="right" w:leader="none"/>"#), "{xml}");
    assert!(!xml.contains("<w:tab/>"), "{xml}");
}
