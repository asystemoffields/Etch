//! Keystroke relayout is incremental (`wordcraft_layout::layout_after`): after every edit —
//! typing, Backspace, Delete, Enter, page breaks, paste, cut, undo and redo, in body text and in
//! table cells — the session's layout is the same as laying the document out from scratch.

use std::sync::Arc;

use serde_json::json;
use wordcraft_doc::para::{InlineObject, NoteKind};
use wordcraft_doc::props::{NumRef, ParaProps};
use wordcraft_doc::section::{Columns, SectionProps, SectionStart};
use wordcraft_doc::{Block, Document, Paragraph, PartKind, Path, Pos, StoryRef, Table, para_block};
use wordcraft_layout::{LayoutCache, layout};

use crate::Session;
use crate::cmd::pos_json;

const WORDS: &[&str] = &["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet"];

fn words(seed: usize, n: usize) -> String {
    (0..n).map(|k| WORDS[(seed * 7 + k * 3) % WORDS.len()]).collect::<Vec<_>>().join(" ")
}

fn para(text: &str, props: ParaProps) -> Arc<Block> {
    let mut p = Paragraph::with_text(text, Default::default());
    p.props = props;
    para_block(p)
}

fn field(d: &mut Document, pos: &Pos, instr: &str) {
    d.insert_object(
        pos,
        InlineObject::Field {
            instr: instr.into(),
            result: String::new(),
            locked: false,
            dirty: false,
            code: false,
            form: String::new(),
            nested: Vec::new(),
        },
        &Default::default(),
    )
    .unwrap();
}

/// Headings, a numbered list continued in table cells, tables with a repeated header row,
/// footnotes, a "Page N of M" header, a page break before; with `sections`, a continuous
/// two-column section and a new-page section restarting the page numbers.
fn document(sections: bool) -> Document {
    let mut d = Document::new();
    d.body.clear();
    let list = d.numbering.add_list(wordcraft_doc::ListKind::Numbered);
    let item = ParaProps { numbering: Some(NumRef { num: list, level: 0 }), ..Default::default() };
    for i in 0..60 {
        let b = match i % 10 {
            0 => para(&format!("Heading {i}"), ParaProps { style: Some("Heading1".into()), ..Default::default() }),
            3 => {
                let mut t = Table::new(4 + i % 7, 3, 468.0);
                for (r, row) in t.rows.iter_mut().enumerate() {
                    for (c, cell) in row.cells.iter_mut().enumerate() {
                        let props = if c == 0 { item.clone() } else { ParaProps::default() };
                        cell.blocks = vec![para(&words(i + r + c, 3 + (r + c) % 5), props)];
                    }
                }
                t.rows[0].props.header = true;
                Arc::new(Block::Table(t))
            }
            5 | 6 => para(&words(i, 5), item.clone()),
            8 if i == 38 => para(&words(i, 40), ParaProps { page_break_before: Some(true), ..Default::default() }),
            _ => para(&words(i, 25 + (i * 13) % 70), ParaProps::default()),
        };
        d.body.push(b);
    }
    // A long table (over several pages) before the last paragraph.
    let mut long = Table::new(150, 3, 468.0);
    for (r, row) in long.rows.iter_mut().enumerate() {
        for (c, cell) in row.cells.iter_mut().enumerate() {
            cell.blocks = vec![para(&words(r + c, 2 + (r * 3 + c) % 6), ParaProps::default())];
        }
    }
    long.rows[0].props.header = true;
    long.props.fixed = true;
    let at = d.body.len() - 1;
    d.body.insert(at, Arc::new(Block::Table(long)));
    for b in [7, 21, 44] {
        let id = d.add_part(PartKind::Footnote, vec![para(&words(b, 12), ParaProps::default())]);
        d.insert_object(&Pos::body(b, 0), InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() }, &Default::default())
            .unwrap();
    }
    let hid = d.add_part(PartKind::Header, vec![para("Page  of ", ParaProps::default())]);
    field(&mut d, &Pos { story: StoryRef::Part(hid), path: Path::top(0), off: 5 }, "PAGE");
    let end = d.end_of(StoryRef::Part(hid));
    field(&mut d, &end, "NUMPAGES");
    d.last_section.headers.default = Some(hid);
    if sections {
        let two = SectionProps { start: SectionStart::Continuous, columns: Columns { count: 2, ..Default::default() }, ..Default::default() };
        let restart = SectionProps { page_num_start: Some(5), ..Default::default() };
        for (b, s) in [(24, SectionProps::default()), (31, two), (47, restart)] {
            if let Block::Para(p) = Arc::make_mut(&mut d.body[b]) {
                p.section = Some(Box::new(s));
            }
        }
    }
    d
}

/// The session's layout is the one laid out from scratch.
fn check(s: &mut Session, what: &str) {
    let inc = s.layout();
    let full = layout(&s.doc, &mut LayoutCache::new(), &s.layout_options());
    let dump = |l: &wordcraft_layout::DocLayout| format!("{:?}\n{}", l.pages, l.text_boxes);
    assert!(dump(&inc) == dump(&full), "{what}: {} pages, {} from scratch", inc.pages.len(), full.pages.len());
}

fn run(s: &mut Session, id: &str, v: serde_json::Value) {
    s.run(id, &v).unwrap_or_else(|e| panic!("{id}: {e}"));
}

fn caret(s: &mut Session, path: Vec<u32>, off: usize) {
    let pos = Pos { story: StoryRef::Body, path: Path(path), off };
    run(s, "caret.set", json!({"pos": pos_json(&pos)}));
}

/// Where to edit: a body paragraph, or the first paragraph of a row's middle cell in the
/// `n`th table.
enum Spot {
    Body(u32),
    Cell(usize, u32),
}

impl Spot {
    fn path(&self, s: &Session) -> Vec<u32> {
        match *self {
            Spot::Body(b) => vec![b.min(s.doc.body.len() as u32 - 1)],
            Spot::Cell(n, row) => {
                let t = s.doc.body.iter().enumerate().filter(|(_, b)| matches!(&***b, Block::Table(_))).nth(n).map_or(0, |(i, _)| i);
                vec![t as u32, row, 1, 0]
            }
        }
    }
}

/// A paragraph's text length.
fn len(s: &Session, path: &[u32]) -> usize {
    s.doc.para_at(&Pos { story: StoryRef::Body, path: Path(path.to_vec()), off: 0 }).map_or(0, |p| p.text.len())
}

#[test]
fn keystroke_relayout_matches_a_layout_from_scratch() {
    let mut resumed = 0;
    for sections in [false, true] {
        let mut s = Session::new(document(sections));
        check(&mut s, "opened");
        let blocks = s.doc.body.len() as u32;
        // Where the edits go: the end, the middle, the start, a heading, a list item, a cell of
        // the second table, a paragraph with a footnote, the paragraph before a page break
        // before, a cell of the last table.
        let spots = [
            Spot::Body(blocks - 1),
            Spot::Body(blocks / 2),
            Spot::Body(1),
            Spot::Body(10),
            Spot::Body(15),
            Spot::Cell(1, 2),
            Spot::Body(21),
            Spot::Body(37),
            Spot::Cell(5, 1),
            Spot::Cell(6, 120),
            Spot::Cell(6, 40),
        ];
        for (k, spot) in spots.iter().enumerate() {
            let path = spot.path(&s);
            let n = len(&s, &path);
            caret(&mut s, path, n);
            for c in ["a", "b", " ", "c"] {
                run(&mut s, "text.insert", json!({"text": c}));
                check(&mut s, &format!("spot {k}: type {c:?}"));
                resumed += usize::from(s.cache_resumed().is_some());
            }
            run(&mut s, "text.backspace", json!({}));
            check(&mut s, &format!("spot {k}: backspace"));
            run(&mut s, "text.newParagraph", json!({}));
            check(&mut s, &format!("spot {k}: enter"));
            run(&mut s, "text.insert", json!({"text": words(k, 30)}));
            check(&mut s, &format!("spot {k}: paste-length typing"));
            run(&mut s, "text.backspace", json!({}));
            run(&mut s, "caret.home", json!({}));
            run(&mut s, "text.backspace", json!({}));
            check(&mut s, &format!("spot {k}: join paragraphs"));
            run(&mut s, "text.delete", json!({}));
            check(&mut s, &format!("spot {k}: delete"));
            if k % 3 == 0 {
                run(&mut s, "text.pageBreak", json!({}));
                check(&mut s, &format!("spot {k}: page break"));
            }
        }
        // Copy a paragraph and paste it elsewhere; cut one.
        caret(&mut s, vec![2], 0);
        run(&mut s, "select.paragraph", json!({}));
        run(&mut s, "edit.copy", json!({}));
        caret(&mut s, vec![blocks / 3], 0);
        run(&mut s, "edit.paste", json!({}));
        check(&mut s, "paste");
        caret(&mut s, vec![blocks / 4 + 1], 0);
        run(&mut s, "select.paragraph", json!({}));
        run(&mut s, "edit.cut", json!({}));
        check(&mut s, "cut");
        // Undo it all, step by step, then redo some.
        for k in 0..12 {
            run(&mut s, "edit.undo", json!({}));
            check(&mut s, &format!("undo {k}"));
        }
        for k in 0..5 {
            run(&mut s, "edit.redo", json!({}));
            check(&mut s, &format!("redo {k}"));
        }
    }
    // The incremental pass was taken (not a whole layout every time).
    assert!(resumed > 40, "resumed {resumed}");
}

/// Footnotes typed in, added and deleted, and paragraphs added before a long table, through the
/// commands: after every edit the layout is the one laid out from scratch, and the pass resumed
/// from the last one (a note's text counts as a change where it is referenced).
#[test]
fn notes_and_paragraphs_before_a_long_table_relayout_from_where_they_are() {
    let mut s = Session::new(document(false));
    check(&mut s, "opened");
    let (mut resumed, mut edits) = (0, 0);
    let mut tally = |s: &Session| {
        edits += 1;
        resumed += usize::from(s.cache_resumed().is_some());
    };
    for b in [5u32, 30, 50] {
        caret(&mut s, vec![b], 3);
        run(&mut s, "references.footnote", json!({"text": "A note"}));
        check(&mut s, &format!("note at {b}: inserted"));
        tally(&s);
        for c in ["a", " ", words(b as usize, 40).as_str(), "z"] {
            run(&mut s, "text.insert", json!({"text": c}));
            check(&mut s, &format!("note at {b}: typed {c:?}"));
            tally(&s);
        }
        run(&mut s, "text.newParagraph", json!({}));
        run(&mut s, "text.insert", json!({"text": words(b as usize + 1, 12)}));
        check(&mut s, &format!("note at {b}: a second paragraph"));
        tally(&s);
    }
    // The first note's reference deleted (Backspace after it).
    caret(&mut s, vec![5], 4);
    run(&mut s, "text.backspace", json!({}));
    check(&mut s, "note reference deleted");
    tally(&s);
    // Enter at the start, layouts not read in between (their pages not copied out), then read.
    caret(&mut s, vec![0], 0);
    for k in 0..4 {
        run(&mut s, "text.newParagraph", json!({}));
        let _ = s.layout();
        tally(&s);
        if k % 2 == 1 {
            check(&mut s, &format!("enter {k}"));
        }
    }
    assert!(resumed + 2 >= edits, "resumed {resumed} of {edits}");
}

/// A value for parameter `key` of a command from its type as the command's `params` doc writes
/// it (`bool`, `pt`, `"a|b"`, `Pos`…); `None` for what can't be guessed or reaches outside the
/// session (paths, files).
fn guess_value(key: &str, ty: &str, s: &Session) -> Option<serde_json::Value> {
    let ty = ty.trim();
    if ["path", "file", "folder", "dir", "data", "picture", "html", "rtf"].iter().any(|k| key.to_ascii_lowercase().contains(k)) {
        return None;
    }
    if ty.contains("Pos") {
        return Some(pos_json(&s.sel.focus));
    }
    if ty.contains("RRGGBB") {
        return Some(json!("C00000"));
    }
    if let Some(rest) = ty.strip_prefix('"') {
        let lit = rest.split('"').next().unwrap_or("");
        return Some(json!(lit.split('|').next().unwrap_or("").trim()));
    }
    if ty.starts_with("bool") {
        return Some(json!(true));
    }
    if ty.starts_with('[') || ty.starts_with('{') {
        return None;
    }
    if ty.starts_with("string") || ty.starts_with("family") || ty.starts_with("tag") {
        return Some(json!(match key {
            "font" | "family" => "Arial",
            "style" | "name" | "basedOn" => "Heading2",
            "url" | "address" => "https://example.com",
            "code" | "instr" => "PAGE",
            "char" => "x",
            _ => "Studio",
        }));
    }
    if ty.contains("pt") {
        return Some(json!(12));
    }
    let first = ty.split(|c: char| !c.is_ascii_digit()).find(|t| !t.is_empty()).and_then(|t| t.parse::<i64>().ok());
    if first.is_some() || ty.starts_with('n') || ty.starts_with("number") || ty.starts_with("percent") || ty.starts_with("int") {
        return Some(json!(first.unwrap_or(2).clamp(1, 400)));
    }
    None
}

/// Parameters for a command guessed from its `params` doc (`{"size": pt, "bold"?: bool}`):
/// every key whose type [`guess_value`] can fill in.
fn guess_params(doc: &str, s: &Session) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    let b = doc.as_bytes();
    let mut i = 0;
    while let Some(q) = doc.get(i..).and_then(|r| r.find('"')).map(|k| k + i) {
        let Some(end) = doc.get(q + 1..).and_then(|r| r.find('"')).map(|k| k + q + 1) else { break };
        let key = &doc[q + 1..end];
        let mut j = end + 1;
        if b.get(j) == Some(&b'?') {
            j += 1;
        }
        if b.get(j) != Some(&b':') || key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
            i = end + 1;
            continue;
        }
        // The type: up to the next comma or closing brace at depth 0.
        let mut depth = 0i32;
        let mut k = j + 1;
        while let Some(&c) = b.get(k) {
            match c {
                b'{' | b'[' | b'(' => depth += 1,
                b'}' | b']' | b')' if depth == 0 => break,
                b'}' | b']' | b')' => depth -= 1,
                b',' if depth == 0 => break,
                _ => {}
            }
            k += 1;
        }
        if let Some(v) = doc.get(j + 1..k).and_then(|ty| guess_value(key, ty, s)) {
            out.insert(key.to_string(), v);
        }
        i = k;
    }
    serde_json::Value::Object(out)
}

/// A few pages with a bit of everything an edit can change the pagination of: headings, a list
/// continued in a table, footnotes, a PAGE field in the body, a header with the page and the
/// page count, a hidden run, a tracked insertion and deletion, a comment, a bookmark referred to,
/// a content control, a two-column continuous section and a new-page section.
fn command_document() -> Session {
    let mut d = Document::new();
    d.body.clear();
    let list = d.numbering.add_list(wordcraft_doc::ListKind::Numbered);
    let item = ParaProps { numbering: Some(NumRef { num: list, level: 0 }), ..Default::default() };
    for i in 0..26 {
        let b = match i {
            0 | 13 => para(&format!("Heading {i}"), ParaProps { style: Some("Heading1".into()), ..Default::default() }),
            3 | 22 => {
                let mut t = Table::new(if i == 3 { 4 } else { 24 }, 3, 468.0);
                for (r, row) in t.rows.iter_mut().enumerate() {
                    for (c, cell) in row.cells.iter_mut().enumerate() {
                        let props = if c == 0 { item.clone() } else { ParaProps::default() };
                        cell.blocks = vec![para(&words(i + r + c, 2 + (r + c) % 5), props)];
                    }
                }
                t.rows[0].props.header = true;
                Arc::new(Block::Table(t))
            }
            5 | 6 => para(&words(i, 5), item.clone()),
            _ => para(&words(i, 30 + (i * 13) % 60), ParaProps::default()),
        };
        d.body.push(b);
    }
    for b in [7, 15] {
        let id = d.add_part(PartKind::Footnote, vec![para(&words(b, 12), ParaProps::default())]);
        d.insert_object(&Pos::body(b, 0), InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() }, &Default::default())
            .unwrap();
    }
    field(&mut d, &Pos::body(9, 0), "PAGE");
    let hid = d.add_part(PartKind::Header, vec![para("Page  of ", ParaProps::default())]);
    field(&mut d, &Pos { story: StoryRef::Part(hid), path: Path::top(0), off: 5 }, "PAGE");
    let end = d.end_of(StoryRef::Part(hid));
    field(&mut d, &end, "NUMPAGES");
    d.last_section.headers.default = Some(hid);
    let two = SectionProps { start: SectionStart::Continuous, columns: Columns { count: 2, ..Default::default() }, ..Default::default() };
    for (b, sp) in [(11, SectionProps::default()), (17, two)] {
        if let Block::Para(p) = Arc::make_mut(&mut d.body[b]) {
            p.section = Some(Box::new(sp));
        }
    }
    let mut s = Session::new(d);
    let sel = |s: &mut Session, b: usize, a: usize, f: usize| {
        run(s, "select.range", json!({"anchor": pos_json(&Pos::body(b, a)), "focus": pos_json(&Pos::body(b, f))}));
    };
    sel(&mut s, 4, 6, 12);
    run(&mut s, "format.hidden", json!({}));
    sel(&mut s, 8, 0, 5);
    run(&mut s, "insert.bookmark", json!({"name": "Mark"}));
    sel(&mut s, 10, 0, 5);
    run(&mut s, "review.newComment", json!({"text": "A comment"}));
    caret(&mut s, vec![12], 0);
    run(&mut s, "insert.field", json!({"instr": "REF Mark"}));
    caret(&mut s, vec![14], 3);
    run(&mut s, "developer.plainText", json!({}));
    run(&mut s, "review.trackChanges", json!({"value": true}));
    caret(&mut s, vec![16], 0);
    run(&mut s, "text.insert", json!({"text": "inserted while tracking "}));
    sel(&mut s, 18, 0, 12);
    run(&mut s, "text.delete", json!({}));
    run(&mut s, "review.trackChanges", json!({"value": false}));
    s.reset_history();
    s
}

/// Every command, with no parameters and with parameters guessed from its `params` doc, run at a
/// selection in body text and at a caret in a table cell: the session's layout afterwards is the
/// one laid out from scratch. A command that changes a paragraph without giving it a new
/// revision leaves its old layout in the cache; one that changes something the incremental pass
/// doesn't compare leaves old pages. (`WC_COMMANDS=prefix` runs only the commands starting so.)
#[test]
fn every_command_relays_out_like_a_layout_from_scratch() {
    let only = std::env::var("WC_COMMANDS").ok();
    let base = command_document();
    let ids: Vec<&'static str> = base
        .registry
        .all()
        .iter()
        .map(|c| c.id)
        .filter(|id| !id.starts_with("file.") && !matches!(*id, "insert.picture" | "insert.textFromFile"))
        .filter(|id| only.as_ref().is_none_or(|o| id.starts_with(o.as_str())))
        .collect();
    let mut failures = Vec::new();
    let mut pure_changes = Vec::new();
    let mut undo_misses = Vec::new();
    let mut ran = 0;
    // Where the commands run: a selection in body text, a caret in a long table's cell, and a
    // selection in a footnote (a story the incremental pass doesn't resume in: what matters there
    // is the paragraph cache, so only the commands that change text and formatting run).
    for spot in 0..3 {
        let mut s = command_document();
        match spot {
            0 => run(&mut s, "select.range", json!({"anchor": pos_json(&Pos::body(19, 6)), "focus": pos_json(&Pos::body(19, 17))})),
            1 => caret(&mut s, vec![22, 5, 1, 0], 2),
            _ => {
                let note = s.doc.parts.iter().find(|(_, p)| p.kind == PartKind::Footnote).map(|(id, _)| *id).unwrap();
                let at = |off| pos_json(&Pos { story: StoryRef::Part(note), path: Path::top(0), off });
                run(&mut s, "select.range", json!({"anchor": at(3), "focus": at(9)}));
            }
        }
        let _ = s.layout();
        let (view, prefs, sel) = (s.view.clone(), s.prefs.clone(), s.sel.clone());
        let mut revs = std::collections::HashMap::new();
        let _ = crate::tests_editing::changed_under_old_revision(&mut revs, &s.doc);
        let in_note = ["format.", "para.", "text.", "styles.", "edit.", "fields.", "font.", "insert.field", "references.updateFields"];
        for id in ids.iter().filter(|id| spot < 2 || in_note.iter().any(|p| id.starts_with(p))) {
            let doc = s.registry.get(id).map_or("{}", |c| c.params);
            let guessed = guess_params(doc, &s);
            let mut sets = vec![json!({})];
            if guessed.as_object().is_some_and(|o| !o.is_empty()) {
                sets.push(guessed);
            }
            for v in sets {
                let snap = s.edit_snapshot();
                // A copy sharing every block, row and part with the document: the command must
                // copy what it writes to, leaving this copy (an undo step) as it was.
                let before = s.doc.clone();
                let written = serde_json::to_string(&before).unwrap();
                let depth = s.undo_depth();
                let r = s.run(id, &v);
                ran += 1;
                for stale in crate::tests_editing::changed_under_old_revision(&mut revs, &s.doc) {
                    failures.push(format!("spot {spot}: {id} {v}: changed without a new revision: {stale}"));
                }
                if serde_json::to_string(&before).unwrap() != written {
                    failures.push(format!("spot {spot}: {id} {v}: changed a copy of the document it shared parts with"));
                }
                let mutates = s.registry.get(id).is_some_and(|c| c.mutates);
                if !mutates && r.is_ok() && s.undo_depth() == depth && serde_json::to_string(&s.doc).unwrap() != written {
                    pure_changes.push(format!("spot {spot}: {id} {v}"));
                }
                let inc = s.layout();
                let full = layout(&s.doc, &mut LayoutCache::new(), &s.layout_options());
                let dump = |l: &wordcraft_layout::DocLayout| format!("{:?}\n{}", l.pages, l.text_boxes);
                if dump(&inc) != dump(&full) {
                    let first = inc.pages.iter().zip(&full.pages).position(|(a, b)| format!("{a:?}") != format!("{b:?}"));
                    failures.push(format!(
                        "spot {spot}: {id} {v} ({}): {} pages, {} from scratch, first different page {first:?}",
                        if r.is_ok() { "ok" } else { "failed" },
                        inc.pages.len(),
                        full.pages.len()
                    ));
                }
                // Undo gives back the document as it was.
                if mutates && r.is_ok() && s.undo_depth() > depth {
                    let _ = s.run("edit.undo", &json!({}));
                    if serde_json::to_string(&s.doc).unwrap() != written {
                        undo_misses.push(format!("spot {spot}: {id} {v}"));
                    }
                }
                // Back to the document as it was, through the same incremental path (an undo
                // across the place the last pass resumed from).
                s.restore(snap);
                (s.view, s.prefs, s.sel) = (view.clone(), prefs.clone(), sel.clone());
                s.ui_requests.clear();
            }
        }
    }
    assert!(ran > 500, "ran {ran}");
    eprintln!("PURE COMMANDS THAT CHANGED THE DOCUMENT ({}):\n{}", pure_changes.len(), pure_changes.join("\n"));
    eprintln!("UNDO DIDN'T RESTORE ({}):\n{}", undo_misses.len(), undo_misses.join("\n"));
    assert!(failures.is_empty(), "{} of {ran} runs laid out differently from scratch:\n{}", failures.len(), failures.join("\n"));
}

/// The process's resident set (Linux `/proc/self/status` `VmRSS`), MiB; `None` elsewhere.
fn rss_mib() -> Option<f64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: f64 = s.lines().find_map(|l| l.strip_prefix("VmRSS:"))?.split_whitespace().next()?.parse().ok()?;
    Some(kb / 1024.0)
}

/// A long editing session on a long document: 10,000 random edits (typing, Enter, Backspace,
/// deleting words, formatting, pasting, undo and redo, at the caret and at random places), the
/// layout read after each as the window does. Prints the resident memory, the undo memory
/// estimate and the paragraph cache's size every 1,000 edits: they level off (the undo steps,
/// the layout cache and the last layout are bounded), they don't grow with the session.
/// Ignored by default (minutes in a debug build): `cargo test -p wordcraft-engine
/// long_session_memory -- --ignored --nocapture` (`WC_MEMORY_EDITS` sets the count).
#[test]
#[ignore]
fn long_session_memory_levels_off() {
    let edits: usize = std::env::var("WC_MEMORY_EDITS").ok().and_then(|v| v.parse().ok()).unwrap_or(10_000);
    let mut d = Document::new();
    d.body.clear();
    let list = d.numbering.add_list(wordcraft_doc::ListKind::Numbered);
    for i in 0..2400 {
        let b = match i % 40 {
            0 => para(&format!("Heading {i}"), ParaProps { style: Some("Heading1".into()), ..Default::default() }),
            17 => {
                let mut t = Table::new(12, 3, 468.0);
                for (r, row) in t.rows.iter_mut().enumerate() {
                    for (c, cell) in row.cells.iter_mut().enumerate() {
                        cell.blocks = vec![para(&words(i + r + c, 2 + (r + c) % 6), ParaProps::default())];
                    }
                }
                Arc::new(Block::Table(t))
            }
            5..=8 => para(&words(i, 6), ParaProps { numbering: Some(NumRef { num: list, level: 0 }), ..Default::default() }),
            _ => para(&words(i, 20 + (i * 13) % 90), ParaProps::default()),
        };
        d.body.push(b);
    }
    for b in (30..2400).step_by(200) {
        let id = d.add_part(PartKind::Footnote, vec![para(&words(b, 15), ParaProps::default())]);
        d.insert_object(&Pos::body(b, 0), InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() }, &Default::default())
            .unwrap();
    }
    let mut s = Session::new(d);
    let first = s.layout();
    let pages = first.pages.len();
    drop(first);
    let start = rss_mib().unwrap_or(0.0);
    eprintln!("MEMORY {pages} pages, {} blocks: start {start:.1} MiB", s.doc.body.len());
    let mut rng = 0x2545_F491_4F6C_DD1Du64;
    let mut below = |n: usize| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng % n.max(1) as u64) as usize
    };
    let t0 = std::time::Instant::now();
    let mut curve = vec![(0usize, start)];
    for i in 1..=edits {
        let (id, v) = match below(40) {
            0..=13 => ("text.insert", json!({"text": WORDS[below(WORDS.len())].to_string() + " "})),
            14 | 15 => ("text.newParagraph", json!({})),
            16..=19 => ("text.backspace", json!({})),
            20 => ("text.deleteWordBack", json!({})),
            21 | 22 => ("edit.undo", json!({})),
            23 => ("edit.redo", json!({})),
            24 => ("select.paragraph", json!({})),
            25 => (["format.bold", "format.italic", "para.alignCenter"][below(3)], json!({})),
            26 => ("edit.copy", json!({})),
            27 => ("edit.paste", json!({})),
            28 => ("caret.down", json!({})),
            29..=33 => {
                // Somewhere else in the document (a paragraph's start, or a table cell's).
                let b = below(s.doc.body.len());
                let path = match &*s.doc.body[b] {
                    Block::Table(t) => vec![b as u32, below(t.rows.len()) as u32, below(3) as u32, 0],
                    Block::Para(_) => vec![b as u32],
                };
                ("caret.set", json!({"pos": pos_json(&Pos { story: StoryRef::Body, path: Path(path), off: 0 })}))
            }
            _ => ("caret.wordRight", json!({})),
        };
        let _ = s.run(id, &v);
        let _ = s.layout();
        if i % 1000 == 0 || i == edits {
            let now = rss_mib().unwrap_or(0.0);
            curve.push((i, now));
            eprintln!(
                "MEMORY after {i:>6} edits: {now:8.1} MiB (+{:.1}), undo {:>4} steps ~{:.1} MiB, paragraph cache {}, {} pages, {:.0} s",
                now - start,
                s.undo_depth(),
                s.undo_bytes() as f64 / (1 << 20) as f64,
                s.cache_len(),
                s.layout().pages.len(),
                t0.elapsed().as_secs_f64()
            );
        }
    }
    // Levelled off: the second half of the session grew the process by much less than the first.
    if let (Some(&(_, mid)), Some(&(_, end))) = (curve.get(curve.len() / 2), curve.last()) {
        eprintln!("MEMORY first half +{:.1} MiB, second half +{:.1} MiB", mid - start, end - mid);
    }
}

/// Formatting a selection that takes in an empty paragraph formats its mark, which is all an
/// empty paragraph's line is made of: the layout shows the new formatting. (Only formatting
/// text gave a paragraph a new revision, so the empty one kept its cached layout.)
#[test]
fn formatting_an_empty_paragraphs_mark_lays_it_out_again() {
    for id in ["format.size", "format.hidden"] {
        let mut s = Session::new(Document::from_text("one\n\nthree"));
        check(&mut s, "opened");
        run(&mut s, "select.all", json!({}));
        run(&mut s, id, json!({"size": 30}));
        check(&mut s, id);
    }
}

/// A tracked paste inserts copies marked as tracked insertions: they show as insertions, the
/// cells of a pasted table too. (Copied whole, a table's cells kept the revisions of the
/// paragraphs they were copied from, so they were shown with those paragraphs' cached layouts,
/// unmarked.)
#[test]
fn a_tracked_paste_shows_as_inserted() {
    for whole in [false, true] {
        let mut d = Document::from_text("before the table\nafter the table\nthe end");
        let mut t = Table::new(2, 2, 468.0);
        for (k, c) in t.rows.iter_mut().flat_map(|r| r.cells.iter_mut()).enumerate() {
            c.blocks = vec![para(&words(k, 3), ParaProps::default())];
        }
        d.body.insert(1, Arc::new(Block::Table(t)));
        let mut s = Session::new(d);
        check(&mut s, "opened");
        let mut revs = std::collections::HashMap::new();
        let _ = crate::tests_editing::changed_under_old_revision(&mut revs, &s.doc);
        if whole {
            run(&mut s, "select.all", json!({}));
        } else {
            run(&mut s, "select.range", json!({"anchor": pos_json(&Pos::body(0, 0)), "focus": pos_json(&Pos::body(2, 5))}));
        }
        run(&mut s, "edit.copy", json!({}));
        if !whole {
            run(&mut s, "caret.docEnd", json!({}));
        }
        run(&mut s, "review.trackChanges", json!({"value": true}));
        run(&mut s, "edit.paste", json!({}));
        let stale = crate::tests_editing::changed_under_old_revision(&mut revs, &s.doc);
        assert!(stale.is_empty(), "whole {whole}: pasted paragraphs changed under their sources' revisions: {stale:?}");
        check(&mut s, &format!("tracked paste, whole {whole}"));
    }
}

/// An untracked paste of whole paragraphs and a table shares them with what was copied: the
/// pasted paragraphs keep their revisions (their layouts are the copied ones', found in the
/// cache) and the pasted table its rows (the undo step holds no copy of them). Giving every
/// pasted paragraph a new revision made pasting 1,000 paragraphs lay them all out again and
/// doubled the undo history of a document of tables.
#[test]
fn an_untracked_paste_shares_what_it_pasted() {
    let mut d = Document::from_text("before the table\nafter the table\nthe end");
    let mut t = Table::new(3, 2, 468.0);
    for (k, c) in t.rows.iter_mut().flat_map(|r| r.cells.iter_mut()).enumerate() {
        c.blocks = vec![para(&words(k, 3), ParaProps::default())];
    }
    d.body.insert(1, Arc::new(Block::Table(t)));
    d.body.insert(1, para("a whole paragraph", ParaProps::default()));
    let mut s = Session::new(d);
    run(&mut s, "select.range", json!({"anchor": pos_json(&Pos::body(0, 0)), "focus": pos_json(&Pos::body(3, 5))}));
    run(&mut s, "edit.copy", json!({}));
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "edit.paste", json!({}));
    let tables: Vec<&Table> = s.doc.body.iter().filter_map(|b| b.as_table()).collect();
    let [source, pasted] = tables[..] else { panic!("the table was pasted: {} tables", tables.len()) };
    assert!(source.rows.iter().zip(&pasted.rows).all(|(a, b)| wordcraft_doc::Shared::ptr_eq(a, b)), "the pasted table shares its rows");
    let rev = |d: &Document, text: &str| d.body.iter().filter_map(|b| b.as_para()).filter(|p| p.text == text).map(|p| p.rev).collect::<Vec<_>>();
    let whole = rev(&s.doc, "a whole paragraph");
    assert!(whole.len() == 2 && whole[0] == whole[1], "the pasted paragraph keeps its revision: {whole:?}");
    check(&mut s, "pasted");
}

/// A copied comment pasted again is a second comment: its anchors take new ids, which changes the
/// pasted paragraph, so that paragraph gets a revision of its own (sharing the copied one's, the
/// cache would show the copied paragraph's layout for it). What the paste doesn't change, a table
/// beside it without comments, still shares its rows.
#[test]
fn a_pasted_comment_gets_new_anchors_and_a_revision_of_its_own() {
    // The commented paragraph is copied whole, between parts of the paragraphs around it, so it
    // is pasted whole too (not joined to the paragraph pasted into).
    let mut d = Document::from_text("the intro\na commented paragraph\nafter the table\nthe end");
    let mut t = Table::new(2, 2, 468.0);
    for (k, c) in t.rows.iter_mut().flat_map(|r| r.cells.iter_mut()).enumerate() {
        c.blocks = vec![para(&words(k, 3), ParaProps::default())];
    }
    d.body.insert(2, Arc::new(Block::Table(t)));
    let mut s = Session::new(d);
    run(&mut s, "select.range", json!({"anchor": pos_json(&Pos::body(1, 2)), "focus": pos_json(&Pos::body(1, 11))}));
    run(&mut s, "review.newComment", json!({"text": "note"}));
    check(&mut s, "commented");
    run(&mut s, "select.range", json!({"anchor": pos_json(&Pos::body(0, 4)), "focus": pos_json(&Pos::body(3, 5))}));
    run(&mut s, "edit.copy", json!({}));
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "edit.paste", json!({}));
    let anchors = |p: &Paragraph| {
        p.objects
            .iter()
            .filter_map(|o| match o {
                InlineObject::CommentStart { id } | InlineObject::CommentEnd { id } => Some(*id),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let commented: Vec<&Paragraph> = s.doc.body.iter().filter_map(|b| b.as_para()).filter(|p| !anchors(p).is_empty()).collect();
    let [source, pasted] = commented[..] else { panic!("two commented paragraphs: {}", commented.len()) };
    assert!(anchors(source).iter().all(|id| !anchors(pasted).contains(id)), "the pasted comment has new anchors");
    assert_eq!(s.doc.comments.len(), 2, "a second comment");
    assert_ne!(source.rev, pasted.rev, "the changed copy has a revision of its own");
    let tables: Vec<&Table> = s.doc.body.iter().filter_map(|b| b.as_table()).collect();
    let [a, b] = tables[..] else { panic!("the table was pasted: {} tables", tables.len()) };
    assert!(a.rows.iter().zip(&b.rows).all(|(x, y)| wordcraft_doc::Shared::ptr_eq(x, y)), "the table without comments shares its rows");
    check(&mut s, "pasted");
    run(&mut s, "edit.undo", json!({}));
    check(&mut s, "undone");
    run(&mut s, "edit.redo", json!({}));
    check(&mut s, "redone");
}

/// Undoing and redoing a paste of paragraphs with footnotes lays (almost) nothing out again. The
/// paste numbers notes again and its undo numbers them back: the cache keeps the paragraphs'
/// layouts with the numbers before as well as after (it used to keep only the last ones', so the
/// undo laid out every paragraph and note from the paste's first copied paragraph on again, and
/// so did the redo).
#[test]
fn undo_and_redo_of_a_paste_with_notes_find_their_layouts() {
    let mut d = Document::new();
    d.body.clear();
    for i in 0..60 {
        d.body.push(para(&words(i, 40), ParaProps::default()));
    }
    for i in 0..60 {
        let id = d.add_part(PartKind::Footnote, vec![para(&words(i, 8), ParaProps::default())]);
        d.insert_object(&Pos::body(i, 5), InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() }, &Default::default())
            .unwrap();
    }
    let mut s = Session::new(d);
    check(&mut s, "opened");
    run(&mut s, "select.range", json!({"anchor": pos_json(&Pos::body(20, 0)), "focus": pos_json(&Pos::body(59, 0))}));
    run(&mut s, "edit.copy", json!({}));
    caret(&mut s, vec![59], 0);
    run(&mut s, "edit.paste", json!({}));
    check(&mut s, "pasted");
    // (Unless another test changed the user dictionary meanwhile: every paragraph is laid out
    // again then, see `Session::cache_env_resets`.)
    let (misses, resets) = (s.cache_misses(), s.cache_env_resets());
    run(&mut s, "edit.undo", json!({}));
    check(&mut s, "undone");
    let laid = s.cache_misses() - misses;
    assert!(laid <= 2 || s.cache_env_resets() != resets, "the undo laid out {laid} paragraphs again");
    let (misses, resets) = (s.cache_misses(), s.cache_env_resets());
    run(&mut s, "edit.redo", json!({}));
    check(&mut s, "redone");
    let laid = s.cache_misses() - misses;
    assert!(laid <= 2 || s.cache_env_resets() != resets, "the redo laid out {laid} paragraphs again");
}

/// Typing in a long AutoFit table as the keyboard does: a word that fits its column lays out
/// only its cell; each key of a word too long for the column widens it, and the other cells,
/// laid out again at the new widths, have their lines broken again but their text not shaped
/// again; Backspace narrows it the same way, and a paste and its undo too. Every layout is the
/// layout from scratch.
#[test]
fn keys_in_an_autofit_table_shape_only_the_cell_typed_in() {
    let mut d = Document::new();
    d.body.clear();
    d.body.push(para("Before the table.", ParaProps::default()));
    let mut t = Table::new(300, 4, 468.0);
    for (r, row) in t.rows.iter_mut().enumerate() {
        for (c, cell) in row.cells.iter_mut().enumerate() {
            cell.blocks = vec![para(&words(r * 4 + c, 3), ParaProps::default())];
        }
    }
    d.body.push(Arc::new(Block::Table(t)));
    let mut s = Session::new(d);
    check(&mut s, "opened");
    caret(&mut s, vec![1, 150, 1, 0], 0);
    let cells = 300 * 4;
    // Each key's paragraphs laid out, broken again and measured, and whether the counts are its
    // own: another test changing the user dictionary meanwhile has every paragraph laid out again
    // (`Session::cache_env_resets`), and then the key isn't checked.
    let key = |s: &mut Session, id: &str, v: serde_json::Value| {
        let (misses, (rebroken, measured), resets) = (s.cache_misses(), s.cache_rebroken_measured(), s.cache_env_resets());
        run(s, id, v);
        check(s, id);
        let (r, m) = s.cache_rebroken_measured();
        (s.cache_misses() - misses, r - rebroken, m - measured, s.cache_env_resets() == resets)
    };
    // A word with spaces: the column keeps its width. (The first key may lay its cell out twice,
    // fonts loading meanwhile.)
    key(&mut s, "text.insert", json!({"text": "a"}));
    for c in " few ".chars() {
        let (misses, rebroken, measured, own) = key(&mut s, "text.insert", json!({"text": c.to_string()}));
        assert!(!own || (misses, rebroken, measured) == (2, 0, 1), "{c:?}: {misses} missed, {rebroken} broken again, {measured} measured");
    }
    let mut widened = 0;
    for c in "Pneumonoultramicroscopicsilicovolcanoconiosis".chars() {
        let (misses, rebroken, measured, own) = key(&mut s, "text.insert", json!({"text": c.to_string()}));
        assert!(!own || misses - rebroken - measured <= 1, "{c:?}: {misses} missed, {rebroken} broken again: only the cell typed in is shaped");
        widened += usize::from(own && rebroken + 1 >= cells as u64);
    }
    assert!(widened > 20, "the column widened on {widened} keys");
    for _ in 0..10 {
        let (misses, rebroken, measured, own) = key(&mut s, "text.backspace", json!({}));
        assert!(!own || misses - rebroken - measured <= 1, "Backspace: {misses} missed, {rebroken} broken again");
    }
    let (misses, rebroken, measured, own) = key(&mut s, "text.insert", json!({"text": "Supercalifragilisticexpialidociously"}));
    assert!(!own || (misses - rebroken - measured <= 1 && rebroken + 1 >= cells as u64), "paste: {misses} missed, {rebroken} broken again");
    let (misses, rebroken, measured, own) = key(&mut s, "edit.undo", json!({}));
    assert!(!own || misses - rebroken - measured <= 1, "undo: {misses} missed, {rebroken} broken again");
}

/// Upstream 1dbbade6 rewinds a revision after a no-op or failure. Etch does not: a cached
/// intermediate document must never share the next edit's revision (#4004).
#[test]
fn unchanged_and_failed_commands_do_not_reuse_a_cached_revision() {
    fn no_change(s: &mut Session, _: &serde_json::Value) -> crate::CmdResult {
        s.touch();
        let _ = s.layout();
        s.unchanged = true;
        Ok(json!({}))
    }
    fn fail_after_layout(s: &mut Session, _: &serde_json::Value) -> crate::CmdResult {
        s.doc = Document::from_text(&"Synthetic rejected content ".repeat(500));
        s.touch();
        let _ = s.layout();
        Err(crate::CmdError::Failed("synthetic refusal".into()))
    }
    type Probe = fn(&mut Session, &serde_json::Value) -> crate::CmdResult;
    for (id, command) in [("test.unchanged", no_change as Probe), ("test.failed", fail_after_layout as Probe)] {
        let mut s = Session::new(Document::from_text("Keep this document"));
        let _ = s.layout();
        let before = s.doc.clone();
        let rev = s.rev();
        let normal_registry = s.registry.clone();
        s.registry = Arc::new(crate::Registry::new(vec![crate::CommandSpec::new(id, "Probe", "Test", command)]));
        let r = s.run(id, &json!({}));
        assert_eq!(r.is_ok(), id == "test.unchanged");
        assert_eq!(s.doc, before);
        assert!(s.rev() > rev, "{id}: do not rewind the cached revision");
        s.registry = normal_registry;
        run(&mut s, "text.insert", json!({"text": "New edit "}));
        check(&mut s, id);
    }
}
