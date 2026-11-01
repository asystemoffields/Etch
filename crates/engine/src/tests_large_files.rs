//! Large documents WordCraft can make open again in every XML-based format it saves: the
//! readers' limits (the XML node budget, the byte limits that follow from the saved size a
//! document may have) are sized so that anything within the limits a document is held to
//! (`wordcraft_doc::limits`) that the writers produce reads back, however heavily it is formatted.

use std::sync::Arc;

use wordcraft_doc::Document;
use wordcraft_doc::props::{CharProps, TextColor, Underline};

use crate::io::{ExportOptions, open_bytes, save_bytes_opts};

/// `paragraphs` paragraphs of `runs` runs in alternating bold, italic and size; `heavy` adds a
/// font, a colour and an underline to every run.
fn formatted(paragraphs: usize, runs: usize, heavy: bool) -> Document {
    let word = "word ";
    let mut doc = Document::new();
    doc.body.clear();
    for _ in 0..paragraphs {
        let mut p = wordcraft_doc::Paragraph::new();
        p.text = word.repeat(runs);
        p.runs = (0..runs)
            .map(|i| {
                let mut props = CharProps {
                    bold: Some(i % 2 == 0),
                    italic: Some(i % 3 == 0),
                    size: Some(if i % 2 == 0 { 12.0 } else { 11.0 }),
                    ..Default::default()
                };
                if heavy {
                    props.font = Some(if i % 2 == 0 { "Source Serif 4" } else { "Source Sans 3" }.into());
                    props.color = Some(TextColor::Rgb(wordcraft_doc::props::Rgb(10, 20, (i * 20) as u8)));
                    props.underline = Some(Underline::Single);
                }
                wordcraft_doc::Run { len: word.len(), props }
            })
            .collect();
        doc.body.push(Arc::new(wordcraft_doc::Block::Para(p)));
    }
    doc
}

/// Found in the save-safety review: a document well under the paragraph cap whose main part
/// held more XML nodes than the reader took saved as `.docx` and was then refused when opened.
/// 120,000 paragraphs of 10 runs in alternating bold, italic and size (about 70 nodes a
/// paragraph as the writer writes them, 8.4 million in all) were enough. They are past the runs
/// a document may have now (750,000): 70,000 such paragraphs, within every limit, open.
#[test]
fn a_heavily_formatted_document_under_the_paragraph_cap_opens_again() {
    let doc = formatted(70_000, 10, false);
    assert_eq!(wordcraft_doc::limits::Tally::of(&doc).over(&wordcraft_doc::limits::Limits::DOCUMENT), None);
    let bytes = crate::io::save_bytes("formatted.docx", &doc).unwrap();
    drop(doc);
    let back = open_bytes("formatted.docx", &bytes);
    assert!(back.is_ok(), "a document WordCraft wrote doesn't open: {:?}", back.err());
    assert_eq!(back.map(|d| d.body.len()).ok(), Some(70_000));
}

/// `paragraphs` paragraphs of `runs` runs of five toggles each (bold, italic, strike, caps,
/// small caps), on and off in turn: many XML nodes for the bytes they take, so a document of more
/// nodes than the reader's budget is still within the saved size a document may have.
fn toggled(paragraphs: usize, runs: usize) -> Document {
    let word = "word ";
    let mut doc = Document::new();
    doc.body.clear();
    for _ in 0..paragraphs {
        let mut p = wordcraft_doc::Paragraph::new();
        p.text = word.repeat(runs);
        p.runs = (0..runs)
            .map(|i| {
                let on = |k: usize| Some((i + k).is_multiple_of(2));
                let props = CharProps { bold: on(0), italic: on(1), strike: on(0), caps: on(1), small_caps: on(0), ..Default::default() };
                wordcraft_doc::Run { len: word.len(), props }
            })
            .collect();
        doc.body.push(Arc::new(wordcraft_doc::Block::Para(p)));
    }
    doc
}

/// XML nodes of the main part of a saved package as the reader's budget counts them, about:
/// elements, attributes and text.
fn main_part_nodes(docx: &[u8]) -> usize {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(docx)).unwrap();
    let mut xml = String::new();
    z.by_name("word/document.xml").unwrap().read_to_string(&mut xml).unwrap();
    let tags = xml.matches('<').count() - xml.matches("</").count();
    tags + xml.matches("=\"").count() + xml.matches("</w:t>").count()
}

/// No fixed count of XML nodes is what the reader holds a file to: it reads a story a block at
/// a time, so a main part of as many nodes as the file's budget (12 million) opens (the reader's
/// own tests show more than the budget opening, `wordcraft_docx`'s `tests_limits`). 70,000
/// paragraphs of ten runs of five toggles take about 126 a paragraph, 8.8 million in all, and are
/// within every limit a document is held to: the runs (750,000) and the saved size bound the
/// nodes a document's main part can have to about the budget (a font, colour and underline on
/// every run, as 08 had it, takes 120,000 such paragraphs past the saved size, and that document
/// is refused by the editor and on opening now).
#[test]
fn a_document_with_more_xml_nodes_than_the_budget_opens_again() {
    let doc = toggled(70_000, 10);
    assert_eq!(wordcraft_doc::limits::Tally::of(&doc).over(&wordcraft_doc::limits::Limits::DOCUMENT), None);
    let bytes = crate::io::save_bytes("heavy.docx", &doc).unwrap();
    let nodes = main_part_nodes(&bytes);
    assert!(nodes > 8_000_000, "{nodes} nodes");
    // Word XML (Flat OPC) holds the same parts in one file, under the same budget.
    let flat = crate::io::save_bytes("heavy.xml", &doc).unwrap();
    drop(doc);
    for (name, bytes) in [("heavy.docx", bytes), ("heavy.xml", flat)] {
        let back = open_bytes(name, &bytes);
        assert!(back.is_ok(), "{name}: a document WordCraft wrote doesn't open: {:?}", back.err());
        let back = back.unwrap_or_default();
        assert_eq!(back.body.len(), 70_000, "{name}");
        let run = back.body.first().and_then(|b| b.as_para()).and_then(|p| p.runs.get(1)).map(|r| r.props.small_caps);
        assert_eq!(run, Some(Some(false)), "{name}");
    }
}

/// A heavily formatted document (a font, a colour and an underline on every run) as large as
/// the saved size a document may have allows (70,000 paragraphs of ten runs) opens again in
/// every XML-based format WordCraft saves: Word XML (Flat OPC), Word 2003 XML and OpenDocument.
#[test]
fn heavily_formatted_documents_open_again_in_every_xml_format() {
    let doc = formatted(70_000, 10, true);
    assert_eq!(wordcraft_doc::limits::Tally::of(&doc).over(&wordcraft_doc::limits::Limits::DOCUMENT), None);
    let word2003 = ExportOptions { word2003: true, ..Default::default() };
    for (name, opts) in [("flat.xml", ExportOptions::default()), ("w2003.xml", word2003), ("formatted.odt", ExportOptions::default())] {
        let bytes = save_bytes_opts(name, &doc, &opts).unwrap();
        let back = open_bytes(name, &bytes);
        assert!(back.is_ok(), "{name}: a document WordCraft wrote doesn't open: {:?}", back.err());
        let back = back.unwrap_or_default();
        assert_eq!(back.body.len(), 70_000, "{name}");
        let run = back.body.first().and_then(|b| b.as_para()).and_then(|p| p.runs.first()).map(|r| r.props.underline);
        assert_eq!(run, Some(Some(Underline::Single)), "{name}");
    }
}
