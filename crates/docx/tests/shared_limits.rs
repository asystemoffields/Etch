//! The shared document limits (`wordcraft_doc::limits`) in the Word reader: a file with one more
//! style, list or secondary story (notes, comments) than a document may have is refused, naming
//! what is over, never opened without what is past the limit (a later save would destroy it);
//! at the limit it opens whole. Synthetic files only.

use std::io::Write;

use wordcraft_doc::limits::{LISTS, STORIES, STYLES};

const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// A package whose main part is `body` and whose other parts (`(relationship type, name, xml)`)
/// the main part relates to.
fn package(body: &str, parts: &[(&str, &str, String)]) -> Vec<u8> {
    let root = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/officeDocument" Target="word/document.xml"/></Relationships>"#
    );
    let rels: String =
        parts.iter().enumerate().map(|(i, (kind, name, _))| format!(r#"<Relationship Id="rP{i}" Type="{R}/{kind}" Target="{name}"/>"#)).collect();
    let rels = format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#);
    let main = format!("<w:document {W_NS}><w:body>{body}</w:body></w:document>");
    let mut files =
        vec![("_rels/.rels".to_string(), root), ("word/document.xml".to_string(), main), ("word/_rels/document.xml.rels".to_string(), rels)];
    files.extend(parts.iter().map(|(_, name, xml)| (format!("word/{name}"), xml.clone())));
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, xml) in files {
        zw.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(xml.as_bytes()).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

/// The file with `n` and `n + 1` of something opens and is refused, naming `what`.
fn at_and_past(n: usize, what: &str, make: impl Fn(usize) -> Vec<u8>) {
    let at = wordcraft_docx::read(&make(n));
    assert!(at.is_ok(), "{n} {what}: {:?}", at.err());
    let past = wordcraft_docx::read(&make(n + 1)).map(|_| ()).unwrap_err().to_string();
    assert!(past.contains("limit exceeded") && past.contains(what), "{} {what}: {past}", n + 1);
}

#[test]
fn styles_past_the_limit_refuse_the_file() {
    at_and_past(STYLES, "styles", |n| {
        let styles: String =
            (0..n).map(|i| format!(r#"<w:style w:type="paragraph" w:styleId="S{i}"><w:name w:val="Synthetic {i}"/></w:style>"#)).collect();
        package("<w:p/>", &[("styles", "styles.xml", format!("<w:styles {W_NS}>{styles}</w:styles>"))])
    });
}

#[test]
fn lists_past_the_limit_refuse_the_file() {
    at_and_past(LISTS, "lists", |n| {
        let abstracts: String = (0..n)
            .map(|i| format!(r#"<w:abstractNum w:abstractNumId="{i}"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum>"#))
            .collect();
        let nums: String = (0..n).map(|i| format!(r#"<w:num w:numId="{}"><w:abstractNumId w:val="{i}"/></w:num>"#, i + 1)).collect();
        package("<w:p/>", &[("numbering", "numbering.xml", format!("<w:numbering {W_NS}>{abstracts}{nums}</w:numbering>"))])
    });
}

#[test]
fn notes_past_the_stories_a_document_may_have_refuse_the_file() {
    at_and_past(STORIES, "headers, footers, notes, comments", |n| {
        let notes: String = (0..n).map(|i| format!(r#"<w:footnote w:id="{}"><w:p/></w:footnote>"#, i + 1)).collect();
        let refs: String = (0..n).map(|i| format!(r#"<w:r><w:footnoteReference w:id="{}"/></w:r>"#, i + 1)).collect();
        package(&format!("<w:p>{refs}</w:p>"), &[("footnotes", "footnotes.xml", format!("<w:footnotes {W_NS}>{notes}</w:footnotes>"))])
    });
}

#[test]
fn comments_past_the_stories_a_document_may_have_refuse_the_file() {
    at_and_past(STORIES, "headers, footers, notes, comments", |n| {
        let comments: String = (0..n).map(|i| format!(r#"<w:comment w:id="{i}" w:author="Reviewer"><w:p/></w:comment>"#)).collect();
        let refs: String = (0..n).map(|i| format!(r#"<w:r><w:commentReference w:id="{i}"/></w:r>"#)).collect();
        package(&format!("<w:p>{refs}</w:p>"), &[("comments", "comments.xml", format!("<w:comments {W_NS}>{comments}</w:comments>"))])
    });
}
