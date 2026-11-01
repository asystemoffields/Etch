//! Where Etch is made to behave as Word's researched behaviour says
//! (`docs/word-behaviour-research.md`): Insert Field computes only the new
//! field, SEQ fields in text boxes count with the body's, PRINTDATE and the record of printing,
//! table-of-figures page numbers in the section's format, the Print hidden text option, Replace
//! All over every story, a section break at the end of a list item, and field updates under
//! formatting-only protection pinned. (Research item 27, the blank page after a page break and
//! a Next Page break: Etch gives two pages, the research expects three at low confidence; left
//! as it is, see the task's report.)

use serde_json::{Value, json};
use wordcraft_doc::para::InlineObject;
use wordcraft_doc::{Document, PartKind, Pos, StoryRef};

use crate::{Selection, Session};

fn run(s: &mut Session, id: &str, v: Value) -> Value {
    s.run(id, &v).unwrap_or_else(|e| panic!("{id} {v}: {e}"))
}

/// Every unlocked simple field of `story` as (code, result), in document order.
fn fields(s: &Session, story: StoryRef) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for path in s.doc.para_paths(story) {
        let Some(p) = s.doc.para(story, &path) else { continue };
        for o in &p.objects {
            if let InlineObject::Field { instr, result, .. } = o {
                out.push((instr.clone(), result.clone()));
            }
        }
    }
    out
}

/// The results of the body's fields whose code starts with `name`.
fn results(s: &Session, name: &str) -> Vec<String> {
    fields(s, StoryRef::Body).into_iter().filter(|(i, _)| i.trim_start().starts_with(name)).map(|(_, r)| r).collect()
}

/// The entries of the first generated list (`ListEntry` paragraphs), as text.
fn list_entries(s: &Session) -> Vec<String> {
    s.doc.body.iter().filter_map(|b| b.as_para()).filter(|p| p.props.style.as_deref() == Some("ListEntry")).map(|p| p.plain_text()).collect()
}

/// The table of contents' entries (`TOC1`…`TOC9` paragraphs; not its title), as text.
fn toc_entries(s: &Session) -> Vec<String> {
    s.doc
        .body
        .iter()
        .filter_map(|b| b.as_para())
        .filter(|p| p.props.style.as_deref().is_some_and(|st| st.strip_prefix("TOC").is_some_and(|l| l.parse::<u8>().is_ok())))
        .map(|p| p.plain_text())
        .collect()
}

/// The text the pages of a layout of `doc` show: each line's text without what is left out
/// (hidden text not shown) and with the objects' text (field results), lines joined with spaces.
fn layout_text(doc: &Document, l: &wordcraft_layout::DocLayout) -> String {
    let mut out = String::new();
    for page in &l.pages {
        for it in page.header.iter().chain(&page.items).chain(&page.footer) {
            let wordcraft_layout::Placed::Lines { story, path, para, l0, l1, .. } = it else { continue };
            let Some(p) = doc.para(*story, path) else { continue };
            for line in para.lines.iter().take(*l1).skip(*l0) {
                for (i, ch) in p.text.char_indices() {
                    if i >= line.start && i < line.stop && !para.left_out.iter().any(|r| r.contains(&i)) {
                        out.push(ch);
                    }
                }
            }
            for (_, t, _) in &para.shown {
                out.push_str(t);
            }
            out.push(' ');
        }
    }
    out
}

/// Research item 11: Insert Field computes the new field and nothing else. The table of
/// contents keeps its entries and the other fields their results until Update Fields, which
/// then settles everything (a second update changes nothing).
#[test]
fn insert_field_computes_only_the_new_field() {
    let mut s = Session::new(Document::from_text("One\ntext\nTwo\nmore"));
    s.prefs.autocorrect.enabled = false;
    for t in ["One", "Two"] {
        run(&mut s, "select.text", json!({"text": t}));
        run(&mut s, "styles.apply", json!({"style": "Heading 1"}));
    }
    // A SEQ field in "text", then the table of contents at the top.
    s.sel = Selection::caret(Pos::body(1, 4));
    run(&mut s, "insert.field", json!({"instr": "SEQ Figure \\* ARABIC"}));
    assert_eq!(results(&s, "SEQ"), ["1"]);
    s.sel = Selection::caret(Pos::body(0, 0));
    run(&mut s, "text.newParagraph", json!({}));
    s.sel = Selection::caret(Pos::body(0, 0));
    run(&mut s, "references.toc", json!({}));
    let toc = toc_entries(&s);
    assert_eq!(toc.len(), 2, "{toc:?}");
    // A third heading, and a SEQ field put in before the first one, with stale results around.
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "text.insert", json!({"text": "Three"}));
    run(&mut s, "styles.apply", json!({"style": "Heading 1"}));
    let one = s.doc.body.iter().position(|b| b.as_para().is_some_and(|p| p.plain_text() == "One")).unwrap();
    s.sel = Selection::caret(Pos::body(one, 0));
    run(&mut s, "insert.field", json!({"instr": "SEQ Figure \\* ARABIC"}));
    // The new field is first in the sequence: 1. The older one keeps its 1 (stale until an
    // update), and the table of contents still lists two headings.
    assert_eq!(results(&s, "SEQ"), ["1", "1"], "only the inserted field was computed");
    assert_eq!(toc_entries(&s).len(), 2, "Insert Field doesn't rebuild the table of contents");
    // A date field is computed as it goes in, and leaves the rest alone too.
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "insert.field", json!({"instr": "DATE \\@ \"yyyy\""}));
    let year = results(&s, "DATE");
    assert_eq!(year.len(), 1);
    assert!(year[0].len() == 4 && year[0].bytes().all(|b| b.is_ascii_digit()), "{year:?}");
    assert_eq!(results(&s, "SEQ"), ["1", "1"]);
    assert_eq!(toc_entries(&s).len(), 2);
    // A PAGEREF to a bookmark is computed from the layout.
    let texts: Vec<String> = s.doc.body.iter().filter_map(|b| b.as_para()).map(|p| p.plain_text()).collect();
    let more = texts.iter().position(|t| t == "more").unwrap_or_else(|| panic!("no `more` paragraph: {texts:?}"));
    s.sel = Selection { anchor: Pos::body(more, 0), focus: Pos::body(more, 4) };
    run(&mut s, "insert.bookmark", json!({"name": "spot"}));
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "insert.field", json!({"instr": "PAGEREF spot \\h"}));
    assert_eq!(results(&s, "PAGEREF"), ["1"]);
    // Update Fields settles the document, and a second update changes nothing.
    run(&mut s, "references.updateFields", json!({}));
    assert_eq!(results(&s, "SEQ"), ["1", "2"]);
    assert_eq!(toc_entries(&s).len(), 3);
    let settled = serde_json::to_string(&s.doc.body).unwrap();
    run(&mut s, "references.updateFields", json!({}));
    assert_eq!(serde_json::to_string(&s.doc.body).unwrap(), settled, "a second update changes nothing");
}

/// Research item 12: SEQ fields in a text box count in the body's sequence, at the place the
/// box is anchored; SEQ fields in headers, footers and notes don't affect it and are left alone.
#[test]
fn seq_fields_in_text_boxes_count_at_their_anchor() {
    let mut s = Session::new(Document::new());
    run(&mut s, "text.insert", json!({"text": "first "}));
    run(&mut s, "insert.field", json!({"instr": "SEQ fig \\* ARABIC"}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "text.insert", json!({"text": "anchor "}));
    let text_box = run(&mut s, "insert.textBox", json!({"text": "in box "}))["story"].as_u64().unwrap() as u32;
    assert_eq!(s.sel.focus.story, StoryRef::Part(text_box));
    run(&mut s, "insert.field", json!({"instr": "SEQ fig \\* ARABIC"}));
    assert_eq!(fields(&s, StoryRef::Part(text_box)).into_iter().map(|f| f.1).collect::<Vec<_>>(), ["2"], "computed at its anchor");
    s.sel = Selection::caret(s.doc.end_of(StoryRef::Body));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "text.insert", json!({"text": "third "}));
    run(&mut s, "insert.field", json!({"instr": "SEQ fig \\* ARABIC"}));
    assert_eq!(results(&s, "SEQ"), ["1", "3"]);
    // A SEQ field in the header: not counted, not renumbered.
    s.sel = Selection::caret(Pos::body(0, 0));
    run(&mut s, "insert.header", json!({"text": "head "}));
    let header = s.sel.focus.story;
    assert!(matches!(header, StoryRef::Part(id) if s.doc.parts.get(&id).is_some_and(|p| p.kind == PartKind::Header)));
    run(&mut s, "insert.field", json!({"instr": "SEQ fig \\* ARABIC", "result": "kept"}));
    run(&mut s, "insert.closeHeader", json!({}));
    run(&mut s, "references.updateFields", json!({"all": true}));
    assert_eq!(results(&s, "SEQ"), ["1", "3"]);
    assert_eq!(fields(&s, StoryRef::Part(text_box)).into_iter().map(|f| f.1).collect::<Vec<_>>(), ["2"]);
    assert_eq!(fields(&s, header).into_iter().map(|f| f.1).collect::<Vec<_>>(), ["kept"], "a header's SEQ field is left alone");
    // Move the box's anchor to the end: its field now counts last.
    let anchor = s.doc.text_box_anchor(text_box).unwrap();
    let obj = Pos { off: anchor.off - wordcraft_doc::para::OBJ.len_utf8(), ..anchor.clone() };
    s.sel = Selection { anchor: obj, focus: anchor };
    run(&mut s, "edit.cut", json!({}));
    s.sel = Selection::caret(s.doc.end_of(StoryRef::Body));
    run(&mut s, "edit.paste", json!({}));
    run(&mut s, "references.updateFields", json!({}));
    assert_eq!(results(&s, "SEQ"), ["1", "2"]);
    let moved = s.doc.parts.iter().find(|(_, p)| p.kind == PartKind::TextBox).map(|(id, _)| *id).unwrap();
    assert_eq!(fields(&s, StoryRef::Part(moved)).into_iter().map(|f| f.1).collect::<Vec<_>>(), ["3"]);
    // Undo and redo through it all restore the document exactly.
    let end = serde_json::to_string(&s.doc).unwrap();
    while s.can_undo() {
        run(&mut s, "edit.undo", json!({}));
    }
    assert_eq!(s.doc.plain_text(StoryRef::Body), "");
    while s.can_redo() {
        run(&mut s, "edit.redo", json!({}));
    }
    assert_eq!(serde_json::to_string(&s.doc).unwrap(), end);
}

/// Research item 14: PRINTDATE shows Word's all-zero date in a document never printed; printing
/// (File › Print, Print to PDF) records the date in the document's properties and the field
/// shows it; Save As or Export to PDF doesn't print. The record round-trips through DOCX
/// (`cp:lastPrinted`).
#[test]
fn printdate_is_the_zero_date_until_a_print_records_it() {
    let dir = std::env::temp_dir().join(format!("wc-printdate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::new(Document::from_text("Printed on "));
    // 2026-03-04 10:00 UTC, seen from Auckland (UTC+13 in March): 23:00 on the 4th.
    s.clock = crate::clock::Clock::fixed(1_772_618_400, wordcraft_tz::Zone::from_posix("NZST-12NZDT,M9.5.0,M4.1.0/3").unwrap());
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "insert.field", json!({"instr": "PRINTDATE"}));
    run(&mut s, "insert.field", json!({"instr": "PRINTDATE \\@ \"yyyy-MM-dd h:mm:ss am/pm\""}));
    assert_eq!(results(&s, "PRINTDATE"), ["0/0/0000", "0000-00-00 0:00:00 AM"]);
    run(&mut s, "references.updateFields", json!({}));
    assert_eq!(results(&s, "PRINTDATE"), ["0/0/0000", "0000-00-00 0:00:00 AM"], "an update of a document never printed");
    // Export and Save As PDF don't print.
    run(&mut s, "file.exportPdf", json!({"path": dir.join("export.pdf")}));
    run(&mut s, "file.saveAs", json!({"path": dir.join("saved.pdf")}));
    assert_eq!(s.doc.core.last_printed, "");
    assert_eq!(results(&s, "PRINTDATE"), ["0/0/0000", "0000-00-00 0:00:00 AM"]);
    // Print to PDF is a print: the properties record it (UTC in the file) and the fields show
    // it (local time), on the pages printed too.
    let r = run(&mut s, "file.printPdf", json!({"path": dir.join("printed.pdf")}));
    assert!(r["pages"].as_u64().unwrap() >= 1);
    assert_eq!(s.doc.core.last_printed, "2026-03-04T10:00:00Z");
    assert_eq!(results(&s, "PRINTDATE"), ["3/4/2026", "2026-03-04 11:00:00 PM"]);
    let lay = s.export_layout();
    assert!(layout_text(&s.doc, &lay).contains("3/4/2026"), "{}", layout_text(&s.doc, &lay));
    // The record survives a DOCX round trip and a Word 2003 XML one; the field shows it again.
    for name in ["printed.docx", "printed.xml"] {
        let opts = crate::io::ExportOptions { word2003: name.ends_with(".xml"), ..Default::default() };
        let bytes = crate::io::save_bytes_with(name, &s.doc, None, &opts).unwrap();
        let mut again = Session::new(crate::io::open_bytes(name, &bytes).unwrap());
        again.clock = s.clock.clone();
        assert_eq!(again.doc.core.last_printed, "2026-03-04T10:00:00Z", "{name}");
        run(&mut again, "references.updateFields", json!({}));
        assert_eq!(results(&again, "PRINTDATE"), ["3/4/2026", "2026-03-04 11:00:00 PM"], "{name}");
    }
    // A document never printed writes no record.
    let fresh = crate::io::save_bytes("fresh.docx", &Document::from_text("x")).unwrap();
    assert_eq!(crate::io::open_bytes("fresh.docx", &fresh).unwrap().core.last_printed, "");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Research item 15: a table of figures shows each caption's page in its section's page-number
/// format, as the table of contents does (roman front matter stays roman).
#[test]
fn table_of_figures_pages_follow_the_section_number_format() {
    let mut s = Session::new(Document::new());
    run(&mut s, "text.insert", json!({"text": "Front plate"}));
    run(&mut s, "references.caption", json!({"label": "Figure", "text": "front"}));
    s.sel = Selection::caret(s.doc.end_of(StoryRef::Body));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "styles.apply", json!({"style": "Normal"}));
    run(&mut s, "layout.break", json!({"kind": "nextPage"}));
    run(&mut s, "text.insert", json!({"text": "Body plate"}));
    run(&mut s, "references.caption", json!({"label": "Figure", "text": "body"}));
    // The first section's pages are numbered in lower-case roman from iv.
    s.sel = Selection::caret(Pos::body(0, 0));
    run(&mut s, "layout.pageNumberFormat", json!({"format": "lowerRoman", "start": 4}));
    run(&mut s, "text.newParagraph", json!({}));
    s.sel = Selection::caret(Pos::body(0, 0));
    run(&mut s, "references.tableOfFigures", json!({}));
    let entries = list_entries(&s);
    assert_eq!(entries.len(), 2, "{entries:?}");
    assert_eq!(entries[0], "Figure 1: front\tiv");
    assert!(entries[1].starts_with("Figure 2: body\t") && entries[1].ends_with(|c: char| c.is_ascii_digit()), "{entries:?}");
    run(&mut s, "references.updateFields", json!({}));
    assert_eq!(list_entries(&s), entries, "an update keeps them");
}

/// Research item 28: "Print hidden text" (File › Options › Display, off by default) decides
/// whether hidden text prints and goes into PDFs; what the screen shows (¶, the formatting-marks
/// option) doesn't.
#[test]
fn print_hidden_text_decides_prints_and_pdfs_not_the_screen() {
    let mut s = Session::new(Document::from_text("Shown SECRET end"));
    run(&mut s, "select.text", json!({"text": "SECRET"}));
    run(&mut s, "format.hidden", json!({}));
    s.sel = Selection::caret(Pos::body(0, 0));
    // The text the print layout holds, and the size of an uncompressed PDF made with the same
    // options (the text is glyphs there: with the hidden word written, more of them).
    let printed = |s: &mut Session| {
        let opts = crate::cmd::file::export_options(s, &json!({}));
        let lo = wordcraft_pdf::layout_options(&crate::io_ext::pdf_options(&opts));
        let lay = s.print_layout(&lo);
        let text = layout_text(&s.doc, &lay);
        let pdf = wordcraft_pdf::export(&s.doc, &wordcraft_pdf::PdfOptions { compress: false, ..crate::io_ext::pdf_options(&opts) }).unwrap();
        (text, pdf.len())
    };
    // Off (Word's default): hidden text stays out, even with ¶ showing it on screen.
    assert!(!s.prefs.print_hidden_text);
    run(&mut s, "view.marks", json!({"value": true}));
    let screen = s.layout();
    assert!(layout_text(&s.doc, &screen).contains("SECRET"), "the screen shows it with ¶ on");
    let (text, pdf_without) = printed(&mut s);
    assert!(text.contains("Shown") && !text.contains("SECRET"), "{text}");
    let (off, _) = s.print_pdf(&json!({})).unwrap();
    // On: it prints and goes into the PDF, with ¶ off too.
    let r = run(&mut s, "file.options", json!({"printHiddenText": true}));
    assert_eq!(r["printHiddenText"], true);
    run(&mut s, "view.marks", json!({"value": false}));
    let screen = s.layout();
    assert!(!layout_text(&s.doc, &screen).contains("SECRET"), "the screen hides it with ¶ off");
    let (text, pdf_with) = printed(&mut s);
    assert!(text.contains("SECRET"), "{text}");
    assert!(pdf_with > pdf_without, "the PDF holds the hidden word's glyphs: {pdf_with} > {pdf_without}");
    let (on, _) = s.print_pdf(&json!({})).unwrap();
    assert_ne!(off, on, "the print job differs");
    // The setting is a preference, kept between runs, not part of the document.
    let prefs: crate::Prefs = serde_json::from_str(&serde_json::to_string(&s.prefs).unwrap()).unwrap();
    assert!(prefs.print_hidden_text);
    let older: crate::Prefs = serde_json::from_str("{}").unwrap();
    assert!(!older.print_hidden_text);
}

/// Replace All scope (medium-high): Replace All covers every story, as Word's dialog with
/// Search: All does: the body, headers and footers, footnotes and endnotes, text boxes and
/// comments, but not a text box inside a header or footer. One undo step; tracked under Track
/// Changes; the caret stays in its story. `in: story` keeps to the caret's story.
#[test]
fn replace_all_covers_every_story_but_text_boxes_in_headers() {
    let mut s = Session::new(Document::new());
    s.prefs.autocorrect.enabled = false;
    run(&mut s, "text.insert", json!({"text": "body zzz here"}));
    run(&mut s, "review.newComment", json!({"text": "comment zzz"}));
    s.sel = Selection::caret(Pos::body(0, 0));
    run(&mut s, "insert.textBox", json!({"text": "box zzz"}));
    let body_box = s.sel.focus.story;
    s.sel = Selection::caret(s.doc.end_of(StoryRef::Body));
    run(&mut s, "references.footnote", json!({"text": "note zzz"}));
    s.sel = Selection::caret(s.doc.end_of(StoryRef::Body));
    run(&mut s, "references.endnote", json!({"text": "end zzz"}));
    s.sel = Selection::caret(Pos::body(0, 0));
    run(&mut s, "insert.footer", json!({"text": "footer zzz"}));
    s.sel = Selection::caret(Pos::body(0, 0));
    run(&mut s, "insert.header", json!({"text": "header zzz "}));
    let header = s.sel.focus.story;
    run(&mut s, "insert.textBox", json!({"text": "header box zzz"}));
    let header_box = s.sel.focus.story;
    assert_ne!(header_box, header);
    run(&mut s, "insert.closeHeader", json!({}));
    s.sel = Selection::caret(Pos::body(0, 0));
    let stories: Vec<StoryRef> = std::iter::once(StoryRef::Body).chain(s.doc.parts.keys().map(|k| StoryRef::Part(*k))).collect();
    let texts = |s: &Session| -> Vec<String> { stories.iter().map(|st| s.doc.plain_text(*st)).collect() };
    let before = texts(&s);
    assert_eq!(before.iter().filter(|t| t.contains("zzz")).count(), 8, "{before:?}");
    let plain = |text: &str, with: &str| json!({"text": text, "with": with, "matchCase": false, "wholeWord": false, "wildcards": false});
    // Every story but the header's text box: 7 replacements.
    let r = run(&mut s, "edit.replaceAll", plain("zzz", "yyy"));
    assert_eq!(r["replaced"], 7);
    let after = texts(&s);
    for (st, (was, now)) in stories.iter().zip(before.iter().zip(&after)) {
        if *st == header_box {
            assert_eq!(now, was, "a text box in a header is out of reach, as in Word");
        } else {
            assert_eq!(*now, was.replace("zzz", "yyy"), "{st:?}");
        }
    }
    assert_eq!(s.sel.focus.story, StoryRef::Body, "the caret stays in its story");
    assert_eq!(s.undo_label(), Some("Replace All"));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(texts(&s), before, "one undo step gives every story back");
    // The caret's story alone, on request (a recorded macro's scope).
    let r = run(&mut s, "edit.replaceAll", json!({"text": "zzz", "with": "yyy", "in": "story"}));
    assert_eq!(r["replaced"], 1);
    assert_eq!(s.doc.plain_text(StoryRef::Body), "body yyy here");
    assert_eq!(s.doc.plain_text(body_box), "box zzz");
    assert!(s.run("edit.replaceAll", &json!({"text": "zzz", "with": "yyy", "in": "page"})).is_err());
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(texts(&s), before);
    // Under Track Changes: every replacement a tracked change, in every story; Reject All gives
    // the stories back, Accept All the replaced text.
    run(&mut s, "review.trackChanges", json!({"value": true}));
    let r = run(&mut s, "edit.replaceAll", plain("zzz", "yyy"));
    assert_eq!(r["replaced"], 7);
    assert_eq!(s.sel.focus.story, StoryRef::Body);
    let mut rejected = Session::new(s.doc.clone());
    run(&mut rejected, "review.rejectAll", json!({}));
    assert_eq!(texts(&rejected), before);
    let mut accepted = Session::new(s.doc.clone());
    run(&mut accepted, "review.acceptAll", json!({}));
    assert_eq!(texts(&accepted), after);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(texts(&s), before, "one undo step");
}

/// A section break at a list item's end (high on storage, ECMA-376 §17.6.17): a Next Page break at the end of
/// a numbered item is held by the item's own paragraph (its `w:pPr/w:sectPr`); no empty numbered
/// paragraph is made to hold it. The paragraph after the break is a new, empty item (low
/// confidence for Word; pinned as it is).
#[test]
fn a_section_break_at_the_end_of_a_list_item_is_held_by_the_item() {
    let mut s = Session::new(Document::new());
    s.prefs.autocorrect.enabled = false;
    run(&mut s, "para.numbering", json!({}));
    for (i, t) in ["one", "two", "three"].iter().enumerate() {
        if i > 0 {
            run(&mut s, "text.newParagraph", json!({}));
        }
        run(&mut s, "text.insert", json!({"text": t}));
    }
    run(&mut s, "layout.break", json!({"kind": "nextPage"}));
    let paras: Vec<(String, bool, bool)> =
        s.doc.body.iter().filter_map(|b| b.as_para()).map(|p| (p.plain_text(), p.section.is_some(), p.props.numbering.is_some())).collect();
    assert_eq!(
        paras,
        [("one".into(), false, true), ("two".into(), false, true), ("three".into(), true, true), (String::new(), false, true)],
        "the item ends the section; an empty item follows on the next page"
    );
    assert_eq!(s.doc.sections().len(), 2);
    assert_eq!(s.layout().pages.len(), 2);
    // Saved as DOCX and read back: the break is in the item's paragraph.
    let bytes = crate::io::save_bytes("list.docx", &s.doc).unwrap();
    let xml = {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let mut f = z.by_name("word/document.xml").unwrap();
        let mut t = String::new();
        std::io::Read::read_to_string(&mut f, &mut t).unwrap();
        t
    };
    let three = xml.find("three</w:t>").unwrap();
    let p_start = xml[..three].rfind("<w:p>").or_else(|| xml[..three].rfind("<w:p ")).unwrap();
    let ppr = &xml[p_start..three];
    assert!(ppr.contains("<w:numPr>") && ppr.contains("<w:sectPr"), "{ppr}");
    let back = crate::io::open_bytes("list.docx", &bytes).unwrap();
    let again: Vec<(String, bool, bool)> =
        back.body.iter().filter_map(|b| b.as_para()).map(|p| (p.plain_text(), p.section.is_some(), p.props.numbering.is_some())).collect();
    assert_eq!(again, paras);
}

/// Research item 5 (high; Etch agrees): formatting-only protection blocks direct formatting and
/// locked styles, not text or field edits: fields insert and update, with the direct formatting
/// their results carry.
#[test]
fn formatting_only_protection_lets_fields_insert_and_update() {
    let mut s = Session::new(Document::from_text("Intro\nbody"));
    run(&mut s, "select.text", json!({"text": "Intro"}));
    run(&mut s, "styles.apply", json!({"style": "Heading 1"}));
    run(&mut s, "review.restrict", json!({"mode": "none", "formatting": true}));
    assert!(s.doc.settings.protect_formatting);
    run(&mut s, "select.text", json!({"text": "body"}));
    let e = s.run("format.bold", &json!({})).unwrap_err().to_string();
    assert!(e.contains("limited to the document's styles"), "{e}");
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "text.insert", json!({"text": " typed"}));
    run(&mut s, "insert.field", json!({"instr": "NUMWORDS"}));
    assert_eq!(results(&s, "NUMWORDS"), ["3"]);
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "references.toc", json!({}));
    assert_eq!(toc_entries(&s).len(), 1, "a table of contents, with its entries' tabs");
    run(&mut s, "text.insert", json!({"text": " more"}));
    run(&mut s, "references.updateFields", json!({}));
    let words = s.doc.word_count();
    assert!(words > 3);
    assert_eq!(results(&s, "NUMWORDS"), [words.to_string()]);
}

/// Research item 2 (high for the highlighting): the markup views fill the cells inserted and
/// deleted with Track Changes on in Word's default colours (light blue, pink), under their text;
/// No Markup shows plain cells; Track Changes Options take other colours (`cellInsertColor`,
/// `cellDeleteColor`, `auto` for the defaults). Merge Cells and Split Cells aren't tracked and
/// say so (`untracked`), as Word warns (item 3).
#[test]
fn tracked_cells_are_highlighted_in_the_markup_views() {
    use wordcraft_layout::display::{CELL_DELETED, CELL_INSERTED, DisplayOptions, Draw, page_display};
    let mut s = Session::new(Document::new());
    run(&mut s, "insert.table", json!({"rows": 2, "cols": 2}));
    run(&mut s, "review.trackChanges", json!({"value": true}));
    run(&mut s, "table.insertColumnRight", json!({}));
    s.sel = Selection::caret(Pos { story: StoryRef::Body, path: wordcraft_doc::Path(vec![0, 1, 0, 0]), off: 0 });
    run(&mut s, "table.deleteRow", json!({}));
    let fills = |s: &mut Session, markup: bool| -> Vec<(wordcraft_doc::Rgb, wordcraft_geom::Rect)> {
        let l = s.layout();
        let opts = DisplayOptions { markup, revisions: s.prefs.markup.clone(), ..Default::default() };
        let (ins, del) = (s.prefs.markup.cell_insert(), s.prefs.markup.cell_delete());
        page_display(&s.doc, &l.pages[0], &opts)
            .into_iter()
            .filter_map(|d| match d {
                Draw::Fill { rect, color, text_bg: false, .. } if color == ins || color == del => Some((color, rect)),
                _ => None,
            })
            .collect()
    };
    let cells = |s: &mut Session| -> Vec<(usize, usize, wordcraft_geom::Rect)> {
        s.layout().pages[0]
            .items
            .iter()
            .filter_map(|it| match it {
                wordcraft_layout::Placed::Cell { rect, row, cell, .. } => Some((*row, *cell, *rect)),
                _ => None,
            })
            .collect()
    };
    let all = cells(&mut s);
    assert_eq!(all.len(), 6, "{all:?}");
    let marked = fills(&mut s, true);
    // The inserted column (right of the caret's cell: column 1) is light blue in the kept row;
    // the deleted row's cells are pink, its inserted cell too.
    let colour_of = |marked: &[(wordcraft_doc::Rgb, wordcraft_geom::Rect)], r: usize, c: usize| {
        let rect = all.iter().find(|(x, y, _)| (*x, *y) == (r, c)).map(|x| x.2).unwrap();
        marked.iter().find(|(_, f)| (f.x - rect.x).abs() < 0.01 && (f.y - rect.y).abs() < 0.01).map(|(c, _)| *c)
    };
    assert_eq!((colour_of(&marked, 0, 0), colour_of(&marked, 0, 2)), (None, None));
    assert_eq!(colour_of(&marked, 0, 1), Some(CELL_INSERTED));
    assert_eq!(
        (colour_of(&marked, 1, 0), colour_of(&marked, 1, 1), colour_of(&marked, 1, 2)),
        (Some(CELL_DELETED), Some(CELL_DELETED), Some(CELL_DELETED))
    );
    assert!(fills(&mut s, false).is_empty(), "No Markup shows plain cells");
    // Other colours from Track Changes Options; `auto` is the default again.
    let r = run(&mut s, "review.trackingOptions", json!({"cellInsertColor": "00FF00", "cellDeleteColor": "auto"}));
    assert_eq!((r["cellInsertColor"].as_str(), r["cellDeleteColor"].as_str()), (Some("00FF00"), Some("auto")));
    let marked = fills(&mut s, true);
    assert_eq!(colour_of(&marked, 0, 1), Some(wordcraft_doc::Rgb(0, 0xFF, 0)));
    assert!(s.run("review.trackingOptions", &json!({"cellInsertColor": "pinkish"})).is_err());
    run(&mut s, "review.trackingOptions", json!({"cellInsertColor": "auto"}));
    assert_eq!(s.prefs.markup.cell_insert(), CELL_INSERTED);
    // Accepted, the fills go with the revisions.
    run(&mut s, "review.acceptAll", json!({}));
    assert!(fills(&mut s, true).is_empty());
    // Merge Cells and Split Cells with tracking on: done, untracked, and the result says so.
    s.sel = Selection::caret(Pos { story: StoryRef::Body, path: wordcraft_doc::Path(vec![0, 0, 0, 0]), off: 0 });
    let r = run(&mut s, "table.split", json!({"columns": 2}));
    assert_eq!(r["untracked"], true);
    assert!(s.doc.revisions.is_empty(), "not tracked");
    run(&mut s, "review.trackChanges", json!({"value": false}));
    let r = run(&mut s, "table.split", json!({"columns": 2}));
    assert!(r.get("untracked").is_none(), "{r}");
}
