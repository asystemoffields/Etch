//! Seeded mutation fuzzing of the DOCX reader and writer, from a package that uses what the
//! reader now keeps (OLE objects without a picture, custom relationships of headers, moves,
//! `w:specVanish`, smart tags and custom XML, position tabs) besides notes, comments and
//! tables. Never-crash: each mutant opens or is refused without a panic, in bounded time, and
//! what opens saves and opens again. The mutator is a fixed-seed xorshift.

use std::io::Write;
use std::time::{Duration, Instant};

const BUDGET: Duration = Duration::from_secs(5);
const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office""#;
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

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
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

const TOKENS: &[&str] = &[
    "<w:p>",
    "</w:p>",
    "<w:r>",
    "</w:r>",
    "<w:t>x</w:t>",
    "<w:smartTag w:element=\"a\">",
    "</w:smartTag>",
    "<w:customXml w:element=\"b\">",
    "</w:customXml>",
    "<w:ptab w:relativeTo=\"margin\" w:alignment=\"center\" w:leader=\"dot\"/>",
    "<w:ptab/>",
    "<w:specVanish/>",
    "<w:moveFrom w:id=\"9\" w:author=\"A\">",
    "</w:moveFrom>",
    "<w:moveTo w:id=\"9\">",
    "</w:moveTo>",
    "<w:moveFromRangeStart w:id=\"5\" w:name=\"m\"/>",
    "<w:moveToRangeEnd w:id=\"5\"/>",
    "<w:object w:dxaOrig=\"-1\" w:dyaOrig=\"99999999\">",
    "<o:OLEObject r:id=\"rId9\"/>",
    "</w:object>",
    "r:id=\"rIdX\"",
    "<w:tbl><w:tr><w:tc>",
    "</w:tc></w:tr></w:tbl>",
    "\"",
    "<",
    ">",
];

fn mutate(seed: &[u8], tokens: &[&str], rng: &mut Rng) -> Vec<u8> {
    let mut b = seed.to_vec();
    for _ in 0..1 + rng.below(6) {
        let at = rng.below(b.len() + 1);
        match rng.below(5) {
            0 => {
                if let Some(x) = b.get_mut(at) {
                    *x = rng.next() as u8;
                }
            }
            1 => {
                let end = (at + rng.below(64)).min(b.len());
                b.drain(at..end);
            }
            2 => {
                let end = (at + rng.below(256)).min(b.len());
                let chunk = b.get(at..end).unwrap_or(&[]).to_vec();
                let to = rng.below(b.len() + 1);
                b.splice(to..to, chunk);
            }
            _ => {
                let t = tokens[rng.below(tokens.len())];
                let long = rng.below(8) == 0;
                let n = 1 + rng.below(if long { 200 } else { 3 });
                b.splice(at..at, t.repeat(n).into_bytes());
            }
        }
    }
    b
}

fn rels(list: &[(&str, &str, &str)]) -> String {
    let mut s =
        String::from(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
    for (id, ty, target) in list {
        s.push_str(&format!(r#"<Relationship Id="{id}" Type="{ty}" Target="{target}"/>"#));
    }
    s.push_str("</Relationships>");
    s
}

const BODY: &str = r#"<w:p><w:r><w:t>Before</w:t></w:r><w:r><w:object w:dxaOrig="2880" w:dyaOrig="1440"><o:OLEObject Type="Embed" ProgID="Package" r:id="rId9"/></w:object></w:r><w:smartTag w:uri="urn:x" w:element="place"><w:smartTagPr><w:attr w:name="k" w:val="v"/></w:smartTagPr><w:r><w:t>Town</w:t></w:r></w:smartTag><w:customXml w:element="d"><w:r><w:rPr><w:specVanish/></w:rPr><w:t>Day</w:t></w:r></w:customXml><w:r><w:ptab w:relativeTo="margin" w:alignment="right" w:leader="dot"/><w:t>Right</w:t></w:r></w:p><w:p><w:moveFrom w:id="1" w:author="Ann"><w:r><w:t>Moved</w:t></w:r></w:moveFrom></w:p><w:p><w:moveTo w:id="2" w:author="Ann"><w:r><w:t>Moved</w:t></w:r></w:moveTo></w:p><w:tbl><w:tr><w:tc><w:tbl><w:tr><w:tc><w:p><w:r><w:t>In</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/></w:tc></w:tr></w:tbl><w:sectPr><w:headerReference w:type="default" r:id="rIdH"/></w:sectPr>"#;

fn package(document: &[u8], header_rels: &[u8]) -> Vec<u8> {
    let root = rels(&[("rId1", &format!("{REL}/officeDocument"), "word/document.xml")]);
    let doc_rels = rels(&[("rId9", &format!("{REL}/oleObject"), "embeddings/oleObject1.bin"), ("rIdH", &format!("{REL}/header"), "header1.xml")]);
    let hdr = format!(r#"<w:hdr {NS}><w:p><w:r><w:t>Head</w:t></w:r></w:p></w:hdr>"#);
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    for (name, data) in [
        ("_rels/.rels", root.as_bytes()),
        ("word/document.xml", document),
        ("word/_rels/document.xml.rels", doc_rels.as_bytes()),
        ("word/header1.xml", hdr.as_bytes()),
        ("word/_rels/header1.xml.rels", header_rels),
        ("word/extra/custom1.xml", b"<x/>"),
        ("word/embeddings/oleObject1.bin", b"ole"),
    ] {
        z.start_file(name, o).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap().into_inner()
}

fn exercise(bytes: &[u8], what: &str) {
    let t = Instant::now();
    if let Ok(doc) = wordcraft_docx::read(bytes) {
        let saved = wordcraft_docx::write(&doc).unwrap_or_else(|e| panic!("{what}: doesn't save: {e}"));
        let again = wordcraft_docx::read(&saved).unwrap_or_else(|e| panic!("{what}: our save doesn't open: {e}"));
        wordcraft_docx::write(&again).unwrap_or_else(|e| panic!("{what}: doesn't save twice: {e}"));
    }
    assert!(t.elapsed() < BUDGET, "{what}: took {:?}", t.elapsed());
}

#[test]
fn mutated_packages_never_panic() {
    let mut rng = Rng(0xd0c5_eed5_1234_5678);
    let document = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NS}><w:body>{BODY}</w:body></w:document>"#);
    let hrels = rels(&[("rIdX", "urn:example:custom", "extra/custom1.xml")]);
    exercise(&package(document.as_bytes(), hrels.as_bytes()), "seed");
    for i in 0..1200 {
        let (d, h) = if i % 4 == 0 {
            (
                document.as_bytes().to_vec(),
                mutate(
                    hrels.as_bytes(),
                    &["\"", "../", "Target=\"\"", "<Relationship Id=\"rIdX\" Type=\"t\" Target=\"word/document.xml\"/>"],
                    &mut rng,
                ),
            )
        } else {
            (mutate(document.as_bytes(), TOKENS, &mut rng), hrels.as_bytes().to_vec())
        };
        exercise(&package(&d, &h), &format!("mutant {i}"));
    }
}
