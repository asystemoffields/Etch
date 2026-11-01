//! Everyday editing, checked the way the keyboard and ribbon drive it: the clipboard within a
//! document and between two documents (two sessions sharing what Copy puts on the system
//! clipboard), Paste Special, other programs' payloads, Find & Replace, lists and numbering, and
//! the undo step each leaves.

use serde_json::{Value, json};
use wordcraft_doc::para::InlineObject;
use wordcraft_doc::{Block, Document, Path, StoryRef};

use crate::Session;

fn run(s: &mut Session, id: &str, v: Value) -> Value {
    s.run(id, &v).unwrap_or_else(|e| panic!("{id} {v}: {e}"))
}

/// Type like a person: `\n` is Enter, everything else one `text.insert` a line.
fn typed(s: &mut Session, text: &str) {
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            run(s, "text.newParagraph", json!({}));
        }
        if !line.is_empty() {
            run(s, "text.insert", json!({"text": line, "raw": true}));
        }
    }
}

fn new_session() -> Session {
    let mut s = Session::new(Document::new());
    s.prefs.autocorrect.enabled = false;
    s
}

/// The body's paragraph texts, a table as `[table]`.
fn texts(d: &Document) -> Vec<String> {
    d.body
        .iter()
        .map(|b| match &**b {
            Block::Para(p) => p.text.clone(),
            Block::Table(_) => "[table]".into(),
        })
        .collect()
}

/// The list labels on the pages, in order.
fn labels(s: &mut Session) -> Vec<String> {
    use wordcraft_layout::Placed;
    let l = s.layout();
    l.pages
        .iter()
        .flat_map(|p| &p.items)
        .filter_map(|i| if let Placed::Lines { para, l0: 0, .. } = i { para.label.as_ref().map(|lb| lb.text.clone()) } else { None })
        .collect()
}

/// The footnote and endnote references in a story, in order: (part id, the note's text).
fn note_refs(d: &Document, story: StoryRef) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for path in d.para_paths(story) {
        let Some(p) = d.para(story, &path) else { continue };
        for o in &p.objects {
            if let InlineObject::NoteRef { id, .. } = o {
                out.push((*id, d.plain_text(StoryRef::Part(*id)).trim().to_string()));
            }
        }
    }
    out
}

fn tiny_png() -> Vec<u8> {
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(2, 2, image::Rgba([30, 30, 200, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    png
}

/// Session `b` pastes what session `a` just copied or cut, the way two WordCraft windows share
/// it: through the system clipboard's text and HTML (what `edit.copy` returns).
fn paste_between(a: &mut Session, b: &mut Session, take: &str, paste: &str) -> Value {
    let r = run(a, take, json!({}));
    run(b, paste, json!({"text": r["text"], "html": r["html"]}))
}

fn docx_round_trip(d: &Document) -> Document {
    wordcraft_docx::read(&wordcraft_docx::write(d).unwrap()).unwrap()
}

/// A document with a bit of everything: a bold word, a numbered list, a footnote, a picture
/// and a table.
fn rich_doc() -> Session {
    let mut s = new_session();
    typed(&mut s, "Alpha beta\nOne\nTwo\nThree\nLast words");
    run(&mut s, "select.text", json!({"text": "beta"}));
    run(&mut s, "format.bold", json!({}));
    run(&mut s, "select.range", json!({"anchor": {"block": 1, "off": 0}, "focus": {"block": 3, "off": 1}}));
    run(&mut s, "para.numbering", json!({}));
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 5}}));
    run(&mut s, "references.footnote", json!({"text": "A note"}));
    // (Inserting a note puts the caret in it, as in Word.)
    run(&mut s, "caret.set", json!({"pos": {"block": 4, "off": 4}}));
    run(&mut s, "insert.picture", json!({"data": crate::cmd::insert::base64_encode(&tiny_png()), "width": 36}));
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "insert.table", json!({"rows": 1, "cols": 2}));
    run(&mut s, "text.insert", json!({"text": "cell", "raw": true}));
    run(&mut s, "caret.docStart", json!({}));
    s.reset_history();
    s
}

/// Copy in one document, paste in another (through the system clipboard's HTML, as two windows
/// do): the text, bold, the numbered list with its numbers, and the picture arrive; so does the
/// table. Saved as DOCX and opened again they are still there.
#[test]
fn copy_to_another_document_keeps_text_formatting_lists_pictures_and_tables() {
    let mut a = rich_doc();
    run(&mut a, "select.all", json!({}));
    let mut b = new_session();
    paste_between(&mut a, &mut b, "edit.copy", "edit.paste");
    for d in [b.doc.clone(), docx_round_trip(&b.doc)] {
        let mut s = Session::new(d);
        let t = texts(&s.doc);
        // ("Alpha" is followed by the footnote's reference, which the HTML carries too.)
        assert!(t.iter().any(|x| x.starts_with("Alpha") && x.ends_with(" beta")), "the text is there: {t:?}");
        assert!(t.contains(&"[table]".to_string()), "the table is there: {t:?}");
        let p = s.doc.para(StoryRef::Body, &Path::top(0)).unwrap();
        let at = p.text.find("beta").unwrap();
        assert_eq!(p.props_of_char(at).bold, Some(true), "beta stays bold");
        assert_eq!(labels(&mut s), ["1.", "2.", "3."], "the list keeps its numbers");
        let pictures =
            s.doc.body.iter().filter_map(|b| b.as_para()).flat_map(|p| &p.objects).filter(|o| matches!(o, InlineObject::Image { .. })).count();
        assert_eq!(pictures, 1, "the picture is there");
    }
}

/// A footnote crosses the HTML clipboard between documents: the copy's HTML carries the note
/// (plain HTML with DPUB-ARIA note roles), and the paste makes a note of its own from it, with
/// the reference where it was; the pasted document saves it as a footnote.
#[test]
fn copy_to_another_document_keeps_footnotes() {
    let mut a = rich_doc();
    run(&mut a, "select.all", json!({}));
    let mut b = new_session();
    paste_between(&mut a, &mut b, "edit.copy", "edit.paste");
    let notes = note_refs(&b.doc, StoryRef::Body);
    assert_eq!(notes.len(), 1, "one footnote: {notes:?}");
    assert_eq!(notes[0].1, "A note");
    let p = b.doc.para(StoryRef::Body, &Path::top(0)).unwrap();
    assert_eq!(p.plain_text(), "Alpha beta", "the reference sits in the first paragraph");
    assert_eq!(p.text.find(wordcraft_doc::para::OBJ), Some(5), "after \"Alpha\": {:?}", p.text);
    let saved = note_refs(&docx_round_trip(&b.doc), StoryRef::Body);
    assert_eq!(saved.len(), 1, "saved as a footnote: {saved:?}");
    assert_eq!(saved[0].1, "A note");
}

/// Cut in one document, paste in the other, both ways: the source loses the text and the target
/// gains it, with its list numbers and bold; each session's own undo takes its half back.
#[test]
fn cut_and_paste_between_two_documents_both_ways() {
    let mut a = rich_doc();
    let mut b = new_session();
    typed(&mut b, "B one\nB two");
    run(&mut b, "select.all", json!({}));
    run(&mut b, "para.bullets", json!({}));
    // A's list goes to B.
    run(&mut a, "select.range", json!({"anchor": {"block": 1, "off": 0}, "focus": {"block": 4, "off": 0}}));
    let before_a = texts(&a.doc);
    run(&mut b, "caret.docEnd", json!({}));
    run(&mut b, "text.newParagraph", json!({}));
    run(&mut b, "text.newParagraph", json!({}));
    paste_between(&mut a, &mut b, "edit.cut", "edit.paste");
    assert!(!texts(&a.doc).contains(&"One".to_string()), "A lost the items: {:?}", texts(&a.doc));
    let tb = texts(&b.doc);
    assert!(tb.contains(&"One".to_string()) && tb.contains(&"Three".to_string()), "B got them: {tb:?}");
    assert_eq!(labels(&mut b), ["•", "•", "1.", "2.", "3."], "B shows its bullets and A's numbers");
    // B's first bullet goes to A's blank line after the table (the cut of the three items left
    // "Last words", the paragraph after them, with its own formatting).
    assert!(labels(&mut a).is_empty(), "A has no list left: {:?}", labels(&mut a));
    assert_eq!(texts(&a.doc), ["Alpha\u{FFFC} beta", "Last\u{FFFC} words", "[table]", ""]);
    run(&mut b, "select.range", json!({"anchor": {"block": 0, "off": 0}, "focus": {"block": 1, "off": 0}}));
    run(&mut a, "caret.set", json!({"pos": {"block": 3, "off": 0}}));
    paste_between(&mut b, &mut a, "edit.cut", "edit.paste");
    assert!(texts(&a.doc).iter().any(|t| t.starts_with("B one")), "A got the bullet: {:?}", texts(&a.doc));
    assert!(!texts(&b.doc).iter().any(|t| t.starts_with("B one")), "B lost it: {:?}", texts(&b.doc));
    let a_lists: Vec<_> = a.doc.body.iter().filter_map(|b| b.as_para()).map(|p| (p.text.clone(), p.props.numbering)).collect();
    assert!(
        labels(&mut a).contains(&"•".to_string()),
        "it is still a bullet in A: {:?}; B's HTML {:?}; A {a_lists:?}",
        labels(&mut a),
        b.clipboard_html
    );
    // Each side undoes its own edits.
    while a.can_undo() {
        run(&mut a, "edit.undo", json!({}));
    }
    assert_eq!(texts(&a.doc), before_a);
}

/// Copy text holding a footnote reference and paste it: the pasted reference is a footnote of its
/// own with the same text (as in Word), not a second reference to the same note. Pasting into
/// another document (through its own copy) makes a note there too.
#[test]
fn pasting_a_copied_footnote_makes_a_note_of_its_own() {
    let mut s = new_session();
    typed(&mut s, "Alpha beta");
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 5}}));
    run(&mut s, "references.footnote", json!({"text": "A note"}));
    // (The caret is in the note, as in Word: back to the body.)
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 0}}));
    run(&mut s, "select.all", json!({}));
    run(&mut s, "edit.copy", json!({}));
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "edit.paste", json!({}));
    run(&mut s, "edit.paste", json!({}));
    let notes = note_refs(&s.doc, StoryRef::Body);
    assert_eq!(notes.len(), 3, "three references: {notes:?}");
    let mut ids: Vec<u32> = notes.iter().map(|n| n.0).collect();
    ids.dedup();
    assert_eq!(ids.len(), 3, "each its own note: {notes:?}");
    assert!(notes.iter().all(|n| n.1 == "A note"), "{notes:?}");
    // Editing one note leaves the others alone.
    let id = notes[1].0;
    run(&mut s, "caret.set", json!({"pos": {"story": {"part": id}, "path": [0], "off": 2}}));
    run(&mut s, "text.insert", json!({"text": "changed ", "raw": true}));
    let notes = note_refs(&s.doc, StoryRef::Body);
    assert_eq!(notes.iter().filter(|n| n.1 == "A note").count(), 2, "{notes:?}");
    // Saved and opened again: three footnotes.
    let back = docx_round_trip(&s.doc);
    assert_eq!(note_refs(&back, StoryRef::Body).len(), 3);
    // Undo takes the pastes back and the notes with them.
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(note_refs(&s.doc, StoryRef::Body).len(), 1);
}

/// A copied range whose middle paragraph ends a section carries the break; pasted into a table
/// cell, a header or a footnote the break can't live there (Word drops it), so it goes. In the
/// body it stays a section break.
#[test]
fn pasting_a_section_break_into_a_cell_header_or_note_drops_it() {
    let mut s = new_session();
    typed(&mut s, "Alpha\nBeta\nGamma");
    run(&mut s, "caret.set", json!({"pos": {"block": 1, "off": 4}}));
    run(&mut s, "layout.break", json!({"kind": "nextPage"}));
    let breaks = |d: &Document, story: StoryRef| -> usize {
        d.para_paths(story).iter().filter(|p| d.para(story, p).is_some_and(|x| x.section.is_some())).count()
    };
    assert!(breaks(&s.doc, StoryRef::Body) >= 1, "a section break: {:?}", texts(&s.doc));
    let sections_before = breaks(&s.doc, StoryRef::Body);
    run(&mut s, "select.all", json!({}));
    run(&mut s, "edit.copy", json!({}));
    // Into a cell.
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "insert.table", json!({"rows": 1, "cols": 1}));
    run(&mut s, "edit.paste", json!({}));
    let cell_breaks = s
        .doc
        .para_paths(StoryRef::Body)
        .iter()
        .filter(|p| p.depth() > 0 && s.doc.para(StoryRef::Body, p).is_some_and(|x| x.section.is_some()))
        .count();
    assert_eq!(cell_breaks, 0, "no section break inside a cell");
    assert_eq!(breaks(&s.doc, StoryRef::Body), sections_before, "the body's sections are as they were");
    // Into a header and a footnote.
    let header = run(&mut s, "insert.header", json!({"text": "H"}))["story"].as_u64().unwrap() as u32;
    run(&mut s, "edit.paste", json!({}));
    assert_eq!(breaks(&s.doc, StoryRef::Part(header)), 0, "no section break in the header");
    run(&mut s, "insert.closeHeader", json!({}));
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 2}}));
    let note = run(&mut s, "references.footnote", json!({"text": "N"}))["id"].as_u64().unwrap() as u32;
    run(&mut s, "edit.paste", json!({}));
    assert_eq!(breaks(&s.doc, StoryRef::Part(note)), 0, "no section break in the note");
    // Into the body: the break comes along.
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 0}}));
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "edit.paste", json!({}));
    assert!(breaks(&s.doc, StoryRef::Body) > sections_before, "pasted in the body the break stays");
    let back = docx_round_trip(&s.doc);
    assert_eq!(breaks(&back, StoryRef::Body), breaks(&s.doc, StoryRef::Body));
}

/// Paste over a selection that spans cells, paragraphs or a field: the table stays valid, the
/// document stays well formed and Undo gives everything back.
#[test]
fn paste_over_selections_spanning_cells_paragraphs_and_fields_stays_valid() {
    let mut s = new_session();
    typed(&mut s, "Head\n");
    run(&mut s, "insert.table", json!({"rows": 2, "cols": 2}));
    for (i, w) in ["aa", "bb", "cc", "dd"].iter().enumerate() {
        if i > 0 {
            run(&mut s, "text.tab", json!({}));
        }
        run(&mut s, "text.insert", json!({"text": w, "raw": true}));
    }
    run(&mut s, "caret.docEnd", json!({}));
    typed(&mut s, "Tail ");
    run(&mut s, "insert.field", json!({"instr": "PAGE"}));
    typed(&mut s, " end");
    let original = serde_json::to_string(&s.doc).unwrap();
    s.reset_history();
    let cell = |r: u32, c: u32, off: usize| json!({"story": "body", "path": [1, r, c, 0], "off": off});
    let cases = [
        ("cells", json!({"anchor": cell(0, 0, 0), "focus": cell(0, 1, 1)})),
        ("rows", json!({"anchor": cell(0, 0, 1), "focus": cell(1, 1, 2)})),
        ("paragraphs", json!({"anchor": {"block": 0, "off": 1}, "focus": {"block": 2, "off": 2}})),
        ("into the table", json!({"anchor": {"block": 0, "off": 2}, "focus": cell(0, 1, 1)})),
        ("out of the table", json!({"anchor": cell(1, 0, 1), "focus": {"block": 2, "off": 3}})),
        ("over the field", json!({"anchor": {"block": 2, "off": 3}, "focus": {"block": 2, "off": 9}})),
    ];
    for (what, sel) in cases {
        for (id, v) in [("edit.pasteText", json!({"text": "X\nY"})), ("edit.paste", json!({"text": "<b>Q</b>", "html": "<p><b>Q</b></p><p>R</p>"}))] {
            run(&mut s, "select.range", sel.clone());
            let res = s.run(id, &v);
            assert!(res.as_ref().err().is_none_or(|e| !e.to_string().contains("panicked")), "{what} {id}: {res:?}");
            crate::tests_editing::check(&s, &format!("{what} {id}"));
            while s.can_undo() {
                run(&mut s, "edit.undo", json!({}));
            }
            assert_eq!(serde_json::to_string(&s.doc).unwrap(), original, "{what} {id}: undo gives the document back");
        }
    }
}

/// Undo grouping: a run of typing is one step, Replace All one step, a paste one step, an
/// AutoCorrect its own step right after the typing it corrected, and Paste Special one step.
#[test]
fn undo_steps_group_typing_runs_replace_all_and_pastes() {
    let mut s = new_session();
    for c in "one two".chars() {
        run(&mut s, "text.insert", json!({"text": c.to_string()}));
    }
    assert_eq!(s.undo_history(), ["Typing"], "a typing run is one step");
    run(&mut s, "text.newParagraph", json!({}));
    for c in "two three two".chars() {
        run(&mut s, "text.insert", json!({"text": c.to_string()}));
    }
    assert_eq!(s.undo_history(), ["Typing", "New Paragraph", "Typing"]);
    run(&mut s, "edit.replaceAll", json!({"text": "two", "with": "2"}));
    assert_eq!(texts(&s.doc), ["one 2", "2 three 2"]);
    assert_eq!(s.undo_label(), Some("Replace All"));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(texts(&s.doc), ["one two", "two three two"], "Replace All is one step");
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(texts(&s.doc), ["one 2", "2 three 2"]);
    run(&mut s, "select.all", json!({}));
    run(&mut s, "edit.copy", json!({}));
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "edit.paste", json!({}));
    assert_eq!(s.undo_label(), Some("Paste"));
    let n = texts(&s.doc).len();
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(texts(&s.doc).len(), n - 1, "a paste of two paragraphs is one step");
    run(&mut s, "edit.pasteSpecial", json!({"as": "text"}));
    assert_eq!(s.undo_label(), Some("Paste Special"));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(texts(&s.doc).len(), n - 1);
    // AutoCorrect right after typing: its own step, then the typing.
    s.prefs.autocorrect.enabled = true;
    run(&mut s, "text.newParagraph", json!({}));
    for c in "teh ".chars() {
        run(&mut s, "text.insert", json!({"text": c.to_string()}));
    }
    assert!(texts(&s.doc).last().unwrap().starts_with("The "), "{:?}", texts(&s.doc));
    run(&mut s, "edit.undo", json!({}));
    assert!(texts(&s.doc).last().unwrap().starts_with("teh "), "Undo takes back only the correction: {:?}", texts(&s.doc));
}

/// Undo keeps working across a save: the steps before the save are still there, and undoing
/// past the save marks the document edited again (it no longer matches the file).
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn undo_works_across_a_save() {
    let dir = std::env::temp_dir().join(format!("wordcraft-undo-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("undo.docx");
    let mut s = new_session();
    typed(&mut s, "First");
    run(&mut s, "file.save", json!({"path": path.to_string_lossy()}));
    assert!(!s.dirty, "saved");
    run(&mut s, "text.newParagraph", json!({}));
    typed(&mut s, "Second");
    assert!(s.dirty);
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(texts(&s.doc), ["First"], "back to the saved text");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(texts(&s.doc), [""], "and past the save");
    assert!(s.dirty, "the document no longer matches the file");
    run(&mut s, "edit.redo", json!({}));
    run(&mut s, "edit.redo", json!({}));
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(texts(&s.doc), ["First", "Second"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Find & Replace: match case, whole words, wildcards, the special codes (`^p`, `^t`, `^m`,
/// `^l`, `^&`), replacing across a paragraph mark, inside a table and inside a note, and
/// Replace All under Track Changes (one revision set: Reject All gives the text back, Accept All
/// the replaced text).
#[test]
fn find_and_replace_options_special_characters_and_stories() {
    let mut s = new_session();
    typed(&mut s, "Cat cat catalog\ta\nb");
    run(&mut s, "caret.docStart", json!({}));
    let r = run(&mut s, "edit.replaceAll", json!({"text": "cat", "with": "X", "matchCase": true, "wholeWord": true}));
    assert_eq!(r["replaced"], 1);
    assert_eq!(texts(&s.doc), ["Cat X catalog\ta", "b"]);
    // (The options stay set between searches, as in Word's dialog: each call names its own.)
    let plain = |text: &str, with: &str| json!({"text": text, "with": with, "matchCase": false, "wholeWord": false, "wildcards": false});
    run(&mut s, "edit.replaceAll", plain("^t", "^p"));
    assert_eq!(texts(&s.doc), ["Cat X catalog", "a", "b"], "^t becomes a paragraph mark");
    run(&mut s, "edit.replaceAll", plain("a^pb", "a^lb"));
    assert_eq!(texts(&s.doc), ["Cat X catalog", "a\nb"], "a match across the mark joins the paragraphs; ^l is a line break");
    run(&mut s, "edit.replaceAll", json!({"text": "c?t", "with": "[^&]", "wildcards": true, "wholeWord": false}));
    assert_eq!(texts(&s.doc)[0], "Cat X [cat]alog", "wildcards match case; ^& is the match");
    run(&mut s, "edit.replaceAll", plain("[cat]", ""));
    assert_eq!(texts(&s.doc)[0], "Cat X alog");
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "text.pageBreak", json!({}));
    typed(&mut s, "after");
    let r = run(&mut s, "edit.replaceAll", plain("^m", " "));
    assert_eq!(r["replaced"], 1, "^m finds the manual page break");
    assert_eq!(texts(&s.doc)[1], "a\nb after");
    // Inside a table cell and inside a footnote.
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "insert.table", json!({"rows": 1, "cols": 1}));
    typed(&mut s, "alog in cell");
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 3}}));
    let note = run(&mut s, "references.footnote", json!({"text": "alog in note"}))["id"].as_u64().unwrap() as u32;
    // (The caret is in the note now. Replace All covers every story by default, as Word's
    // dialog does — `tests_word_parity::replace_all_covers_every_story_but_text_boxes_in_headers`;
    // `in: story` keeps to the caret's story, as a recorded macro does.)
    let in_story =
        |text: &str, with: &str| json!({"text": text, "with": with, "matchCase": false, "wholeWord": false, "wildcards": false, "in": "story"});
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 0}}));
    let r = run(&mut s, "edit.replaceAll", in_story("alog", "Z"));
    assert_eq!(r["replaced"], 2, "the body and its table");
    assert_eq!(texts(&s.doc)[0].replace(wordcraft_doc::para::OBJ, ""), "Cat X Z");
    assert!(s.doc.plain_text(StoryRef::Body).contains("Z in cell"));
    assert!(s.doc.plain_text(StoryRef::Part(note)).contains("alog in note"), "the caret's story alone");
    run(&mut s, "caret.set", json!({"pos": {"story": {"part": note}, "path": [0], "off": 1}}));
    let r = run(&mut s, "edit.replaceAll", in_story("alog", "Z"));
    assert_eq!(r["replaced"], 1, "with the caret in the note, the note");
    assert!(s.doc.plain_text(StoryRef::Part(note)).contains("Z in note"));
    // Under Track Changes.
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 0}}));
    let before = s.doc.plain_text(StoryRef::Body);
    run(&mut s, "review.trackChanges", json!({"value": true}));
    run(&mut s, "edit.replaceAll", plain("X", "Why"));
    assert_eq!(s.undo_label(), Some("Replace All"));
    let mut rejected = Session::new(s.doc.clone());
    run(&mut rejected, "review.rejectAll", json!({}));
    assert_eq!(rejected.doc.plain_text(StoryRef::Body), before);
    let mut accepted = Session::new(s.doc.clone());
    run(&mut accepted, "review.acceptAll", json!({}));
    assert_eq!(accepted.doc.plain_text(StoryRef::Body), before.replace('X', "Why"));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc.plain_text(StoryRef::Body), before, "one undo step");
}

/// Copy any selection, paste it into an empty document, save as DOCX and open again: the text,
/// its formatting, the list numbering and the notes survive (within one process, where the
/// paste is WordCraft's own fragment).
#[test]
fn a_copied_selection_pasted_into_an_empty_document_round_trips_through_docx() {
    let mut a = rich_doc();
    let n = a.doc.body.len();
    let selections = [
        json!({"anchor": {"block": 0, "off": 0}, "focus": {"block": n - 1, "off": 0}}),
        json!({"anchor": {"block": 0, "off": 2}, "focus": {"block": 2, "off": 2}}),
        json!({"anchor": {"block": 1, "off": 0}, "focus": {"block": 4, "off": 0}}),
    ];
    for sel in selections {
        run(&mut a, "select.range", sel.clone());
        run(&mut a, "edit.copy", json!({}));
        let want_text = a.clipboard_text.clone();
        let mut b = new_session();
        b.clipboard = a.clipboard.clone();
        run(&mut b, "edit.paste", json!({}));
        let back = docx_round_trip(&b.doc);
        let mut c = Session::new(back);
        assert_eq!(c.doc.plain_text(StoryRef::Body).trim_end(), b.doc.plain_text(StoryRef::Body).trim_end(), "{sel}: text");
        let got: String = c
            .doc
            .plain_text(StoryRef::Body)
            .lines()
            .map(|l| l.trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == '.' || ch == '\t'))
            .collect::<Vec<_>>()
            .join("\n");
        let want: String =
            want_text.lines().map(|l| l.trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == '.' || ch == '\t')).collect::<Vec<_>>().join("\n");
        assert!(
            got.trim_end().replace(wordcraft_doc::para::OBJ, "").contains(want.trim_end().replace(wordcraft_doc::para::OBJ, "").trim()),
            "{sel}: {got:?} vs {want:?}"
        );
        assert_eq!(labels(&mut c), labels(&mut b), "{sel}: list numbering");
        assert_eq!(note_refs(&c.doc, StoryRef::Body), note_refs(&b.doc, StoryRef::Body), "{sel}: notes");
        if sel["anchor"]["block"] == 0 && sel["anchor"]["off"] == 0 {
            let p = c.doc.para(StoryRef::Body, &Path::top(0)).unwrap();
            let at = p.text.find("beta").unwrap();
            assert_eq!(p.props_of_char(at).bold, Some(true), "bold");
            assert_eq!(note_refs(&c.doc, StoryRef::Body).len(), 1, "the note came along");
        }
    }
}

/// Plain text from another program (the system clipboard's text, no HTML or RTF) pastes as
/// Keep Text Only does: each line after the first splits the paragraph as Enter does, so every
/// pasted paragraph keeps the paragraph's alignment, indents and list, as in Word.
#[test]
fn plain_text_pasted_from_another_program_keeps_the_paragraphs_formatting() {
    let mut s = new_session();
    typed(&mut s, "Alpha");
    run(&mut s, "para.alignCenter", json!({}));
    run(&mut s, "para.indent", json!({}));
    run(&mut s, "caret.end", json!({}));
    run(&mut s, "edit.paste", json!({"text": " one\ntwo\nthree"}));
    assert_eq!(texts(&s.doc), ["Alpha one", "two", "three"]);
    for i in 0..3 {
        let p = s.doc.para(StoryRef::Body, &Path::top(i)).unwrap();
        let r = s.doc.styles.resolve_para(&p.props);
        assert_eq!(r.align, wordcraft_doc::props::Align::Center, "paragraph {i} keeps the alignment");
        assert!(r.indent_left > 0.0, "paragraph {i} keeps the indent");
    }
}

/// Restart at 1, Set Numbering Value and Continue Numbering, on a list split by a table and by a
/// section break: the labels follow, as a fresh layout numbers them, and Undo and Redo give the
/// same labels back.
#[test]
fn restart_set_value_and_continue_numbering_across_a_table_and_a_section_break() {
    let mut s = new_session();
    typed(&mut s, "One\nTwo\nThree\nFour\nFive\nSix");
    run(&mut s, "select.all", json!({}));
    run(&mut s, "para.numbering", json!({}));
    // A table in a plain paragraph after Two, and a section break in one after Four.
    run(&mut s, "caret.set", json!({"pos": {"block": 1, "off": 3}}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "para.numbering", json!({"off": true}));
    run(&mut s, "insert.table", json!({"rows": 1, "cols": 1}));
    run(&mut s, "text.insert", json!({"text": "cell", "raw": true}));
    let four = texts(&s.doc).iter().position(|t| t == "Four").unwrap();
    run(&mut s, "caret.set", json!({"pos": {"block": four, "off": 4}}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "para.numbering", json!({"off": true}));
    run(&mut s, "layout.break", json!({"kind": "nextPage"}));
    let fresh = |s: &mut Session| {
        let got = labels(s);
        let mut f = Session::new(s.doc.clone());
        assert_eq!(got, labels(&mut f), "the labels are those of a fresh layout");
        got
    };
    assert_eq!(fresh(&mut s), ["1.", "2.", "3.", "4.", "5.", "6."], "a table and a section break don't end the list");
    // Restart at Three.
    let at = |s: &mut Session, text: &str| {
        let i = texts(&s.doc).iter().position(|t| t == text).unwrap();
        run(s, "caret.set", json!({"pos": {"block": i, "off": 0}}));
    };
    at(&mut s, "Three");
    run(&mut s, "para.restartNumbering", json!({}));
    assert_eq!(fresh(&mut s), ["1.", "2.", "1.", "2.", "3.", "4."]);
    // Set Five to 7.
    at(&mut s, "Five");
    run(&mut s, "para.setNumberingValue", json!({"value": 7}));
    assert_eq!(fresh(&mut s), ["1.", "2.", "1.", "2.", "7.", "8."]);
    // Continue Numbering at Five rejoins the list before it; at Three the first list.
    at(&mut s, "Five");
    run(&mut s, "para.continueNumbering", json!({}));
    assert_eq!(fresh(&mut s), ["1.", "2.", "1.", "2.", "3.", "4."]);
    at(&mut s, "Three");
    run(&mut s, "para.continueNumbering", json!({}));
    assert_eq!(fresh(&mut s), ["1.", "2.", "3.", "4.", "5.", "6."]);
    // Already continuing: refused, nothing changes.
    at(&mut s, "Four");
    assert!(s.run("para.continueNumbering", &json!({})).is_err());
    at(&mut s, "One");
    assert!(s.run("para.continueNumbering", &json!({})).is_err(), "nothing before the first item");
    assert_eq!(fresh(&mut s), ["1.", "2.", "3.", "4.", "5.", "6."]);
    // Undo back through every change (the refused ones left no step), and Redo.
    let states = [
        vec!["1.", "2.", "3.", "4.", "5.", "6."],
        vec!["1.", "2.", "1.", "2.", "3.", "4."],
        vec!["1.", "2.", "1.", "2.", "7.", "8."],
        vec!["1.", "2.", "1.", "2.", "3.", "4."],
        vec!["1.", "2.", "3.", "4.", "5.", "6."],
    ];
    for w in states.iter().rev().skip(1) {
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(fresh(&mut s), *w, "undo");
    }
    for w in states.iter().skip(1) {
        run(&mut s, "edit.redo", json!({}));
        assert_eq!(fresh(&mut s), *w, "redo");
    }
    let back = Session::new(docx_round_trip(&s.doc));
    let mut back = back;
    assert_eq!(labels(&mut back), ["1.", "2.", "3.", "4.", "5.", "6."], "saved and opened again");
}

/// Enter and Backspace on list items, as docs/typing-parity.md records: Enter on an empty item
/// ends the list (a nested one moves up first); Backspace at an item's start takes the number,
/// then the indent, and never joins the paragraph above; Tab and Shift+Tab change the level. Each
/// key is one undo step and Undo gives the labels back.
#[test]
fn enter_backspace_tab_on_list_items_follow_word_and_undo() {
    let mut s = new_session();
    typed(&mut s, "One\nTwo");
    run(&mut s, "select.all", json!({}));
    run(&mut s, "para.numbering", json!({}));
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "text.tab", json!({}));
    typed(&mut s, "Nested");
    assert_eq!(labels(&mut s), ["1.", "2.", "a."]);
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "text.newParagraph", json!({}));
    assert_eq!(labels(&mut s), ["1.", "2.", "a.", "3."], "Enter on the empty nested item moves it up a level");
    run(&mut s, "text.newParagraph", json!({}));
    assert_eq!(labels(&mut s), ["1.", "2.", "a."], "Enter on the empty top-level item ends the list");
    assert_eq!(texts(&s.doc), ["One", "Two", "Nested", ""]);
    typed(&mut s, "Plain");
    run(&mut s, "caret.set", json!({"pos": {"block": 2, "off": 0}}));
    run(&mut s, "text.backTab", json!({}));
    assert_eq!(labels(&mut s), ["1.", "2.", "3."], "Shift+Tab promotes");
    run(&mut s, "text.backspace", json!({}));
    assert_eq!(labels(&mut s), ["1.", "2."], "Backspace at the start takes the number");
    assert_eq!(texts(&s.doc).len(), 4, "not joined to the paragraph above");
    run(&mut s, "text.backspace", json!({}));
    assert_eq!(texts(&s.doc).len(), 4, "the second Backspace takes the indent, still not joined");
    let p = s.doc.para(StoryRef::Body, &Path::top(2)).unwrap();
    assert_eq!(s.doc.styles.resolve_para(&p.props).indent_left, 0.0);
    run(&mut s, "text.backspace", json!({}));
    assert_eq!(texts(&s.doc), ["One", "TwoNested", "Plain"], "the third joins");
    // The run of Backspaces is one undo step (typing-parity, Undo), then Shift+Tab, then typing.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(texts(&s.doc), ["One", "Two", "Nested", "Plain"]);
    assert_eq!(labels(&mut s), ["1.", "2.", "3."], "undone back to the promoted item");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(labels(&mut s), ["1.", "2.", "a."], "and to the nested one");
    run(&mut s, "edit.redo", json!({}));
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(texts(&s.doc), ["One", "TwoNested", "Plain"]);
}

/// Deleting (or cutting) whole paragraphs, from the first one's start to the start of the
/// paragraph after them, takes them with their marks: the paragraph after keeps its own
/// formatting, as in Word (three list items deleted don't make the next paragraph an item).
/// A deletion from inside the first paragraph joins as Backspace does, keeping its formatting.
#[test]
fn deleting_whole_paragraphs_leaves_the_next_paragraphs_formatting_alone() {
    for id in ["text.delete", "edit.cut", "text.backspace"] {
        let mut s = new_session();
        typed(&mut s, "Head\nOne\nTwo\nTail");
        run(&mut s, "select.range", json!({"anchor": {"block": 1, "off": 0}, "focus": {"block": 2, "off": 3}}));
        run(&mut s, "para.numbering", json!({}));
        run(&mut s, "select.text", json!({"text": "Tail"}));
        run(&mut s, "format.bold", json!({}));
        assert_eq!(labels(&mut s), ["1.", "2."]);
        run(&mut s, "select.range", json!({"anchor": {"block": 1, "off": 0}, "focus": {"block": 3, "off": 0}}));
        run(&mut s, id, json!({}));
        assert_eq!(texts(&s.doc), ["Head", "Tail"], "{id}");
        assert!(labels(&mut s).is_empty(), "{id}: Tail isn't an item: {:?}", labels(&mut s));
        let p = s.doc.para(StoryRef::Body, &Path::top(1)).unwrap();
        assert_eq!(p.props_of_char(0).bold, Some(true), "{id}: Tail keeps its text formatting");
        // From inside the first paragraph: joined, keeping the first one's formatting.
        run(&mut s, "edit.undo", json!({}));
        run(&mut s, "select.range", json!({"anchor": {"block": 1, "off": 1}, "focus": {"block": 3, "off": 0}}));
        run(&mut s, id, json!({}));
        assert_eq!(texts(&s.doc), ["Head", "OTail"], "{id}");
        assert_eq!(labels(&mut s), ["1."], "{id}: the item keeps its number");
    }
}

/// A footnote inserted with Track Changes on is a tracked insertion: Reject All takes the
/// reference and the note out, Accept All keeps them.
#[test]
fn a_footnote_inserted_under_track_changes_is_a_tracked_insertion() {
    let mut s = new_session();
    typed(&mut s, "Alpha beta");
    run(&mut s, "review.trackChanges", json!({"value": true}));
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 5}}));
    run(&mut s, "references.footnote", json!({"text": "A note"}));
    assert_eq!(note_refs(&s.doc, StoryRef::Body).len(), 1);
    let mut rejected = Session::new(s.doc.clone());
    run(&mut rejected, "review.rejectAll", json!({}));
    assert!(note_refs(&rejected.doc, StoryRef::Body).is_empty(), "rejected: the reference goes");
    assert_eq!(rejected.doc.plain_text(StoryRef::Body).trim_end(), "Alpha beta");
    let mut accepted = Session::new(s.doc.clone());
    run(&mut accepted, "review.acceptAll", json!({}));
    assert_eq!(note_refs(&accepted.doc, StoryRef::Body), vec![(note_refs(&s.doc, StoryRef::Body)[0].0, "A note".to_string())]);
    let p = accepted.doc.para(StoryRef::Body, &Path::top(0)).unwrap();
    assert!(p.runs.iter().all(|r| r.props.ins.is_none()), "accepted: no insertion marks left");
}

/// A list item from a web page (`<ul><li>`) pasted on a blank line is a list item; pasted into
/// a paragraph that has text, the text joins it and the paragraph keeps its own formatting.
#[test]
fn an_html_list_item_pasted_on_a_blank_line_is_a_list_item() {
    let mut s = new_session();
    run(&mut s, "edit.paste", json!({"text": "one", "html": "<ul><li>one</li></ul>"}));
    assert_eq!(labels(&mut s), ["•"]);
    assert_eq!(texts(&s.doc), ["one"]);
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "text.newParagraph", json!({}));
    typed(&mut s, "Plain text");
    run(&mut s, "caret.home", json!({}));
    run(&mut s, "edit.paste", json!({"text": "two", "html": "<ol><li>two</li></ol>"}));
    assert_eq!(texts(&s.doc), ["one", "twoPlain text"]);
    assert_eq!(labels(&mut s), ["•"], "the paragraph with text keeps its formatting");
}

/// A picture, a shape or a text box inserted with Track Changes on is a tracked insertion, like
/// typed text: Reject All takes it out (and the box's text with it), Accept All keeps it.
#[test]
fn objects_inserted_under_track_changes_are_tracked_insertions() {
    let objects = |d: &Document| d.body.iter().filter_map(|b| b.as_para()).map(|p| p.objects.len()).sum::<usize>();
    let mut s = new_session();
    typed(&mut s, "Alpha beta");
    run(&mut s, "review.trackChanges", json!({"value": true}));
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 5}}));
    run(&mut s, "insert.picture", json!({"data": crate::cmd::insert::base64_encode(&tiny_png()), "width": 36}));
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 5}}));
    run(&mut s, "insert.shape", json!({"kind": "ellipse"}));
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 5}}));
    run(&mut s, "insert.textBox", json!({"text": "boxed"}));
    assert_eq!(objects(&s.doc), 3);
    let mut rejected = Session::new(s.doc.clone());
    run(&mut rejected, "review.rejectAll", json!({}));
    assert_eq!(objects(&rejected.doc), 0, "rejected: the objects go");
    assert_eq!(rejected.doc.plain_text(StoryRef::Body).trim_end(), "Alpha beta");
    assert!(!rejected.doc.parts.values().any(|p| p.kind == wordcraft_doc::PartKind::TextBox), "and the box's story");
    let mut accepted = Session::new(s.doc.clone());
    run(&mut accepted, "review.acceptAll", json!({}));
    assert_eq!(objects(&accepted.doc), 3, "accepted: they stay");
    let p = accepted.doc.para(StoryRef::Body, &Path::top(0)).unwrap();
    assert!(p.runs.iter().all(|r| r.props.ins.is_none()), "with no insertion marks left");
}
