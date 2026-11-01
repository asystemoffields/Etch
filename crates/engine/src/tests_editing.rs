//! The editing core under random command sequences: whatever the keys, nothing panics, every
//! paragraph stays well formed, the selection stays inside the document, undoing everything
//! gives back the document exactly and redoing everything gives back the result.

use serde_json::{Value, json};
use wordcraft_doc::{Block, Blocks, Document, Pos};

use crate::{Selection, Session};

/// A small deterministic generator (xorshift), so a failing seed can be replayed.
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
        (self.next() % n as u64) as usize
    }
}

/// Text that stresses caret stepping: combining marks, surrogate pairs, emoji sequences, CJK,
/// right-to-left letters, tabs and line breaks.
const PIECES: &[&str] = &[
    "a",
    "b",
    " ",
    "e\u{301}",
    "\u{1F600}",
    "\u{1F469}\u{200D}\u{1F4BB}",
    "\u{4E2D}",
    "\u{05D0}",
    "\u{0627}",
    ".",
    "-",
    "\"",
    "'",
    "1",
    "*",
    "\u{1F1EB}\u{1F1F7}",
    "\u{0915}\u{094D}\u{0937}",
    "teh",
    "(c)",
];

/// Table Layout's row, column and table edits (tracked as row and cell revisions).
const TABLE_EDITS: &[&str] = &[
    "table.insertRowAbove",
    "table.insertRowBelow",
    "table.insertColumnLeft",
    "table.insertColumnRight",
    "table.deleteRow",
    "table.deleteColumn",
    "table.deleteCells",
    "table.deleteTable",
];

/// Other programs' clipboard payloads, as they put them on the system clipboard: HTML (a web
/// page, a Word copy with lists, a table, a picture), RTF, and plain text with line breaks.
const HTML_PAYLOADS: &[&str] = &[
    "<p><b>Bold</b> and <i>italic</i> text</p>",
    "<ol><li>first</li><li>second<ul><li>nested</li></ul></li></ol><p>after</p>",
    "<table><tr><td>a1</td><td>b1</td></tr><tr><td>a2</td><td>b2</td></tr></table>",
    "<h1>Title</h1><p style=\"text-align:center\">centred</p><blockquote>quoted</blockquote>",
    "Version:0.9\r\nStartHTML:0000000105\r\nEndHTML:0000000160\r\nStartFragment:0000000141\r\nEndFragment:0000000150\r\n<html><body><!--StartFragment--><p>frag</p><!--EndFragment--></body></html>",
    "<p>one</p><p></p><p>three</p>",
    "<p>img <img src=\"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==\" alt=\"dot\"></p>",
];
const RTF_PAYLOADS: &[&str] = &[
    "{\\rtf1\\ansi{\\b bold} plain\\par next\\par}",
    "{\\rtf1\\ansi\\pard{\\*\\pn\\pnlvlblt}\\fi-360\\li720 item one\\par item two\\par}",
    "{\\rtf1\\ansi\\trowd\\cellx3000\\cellx6000 c1\\cell c2\\cell\\row\\pard after\\par}",
];
const TEXT_PAYLOADS: &[&str] = &["plain", "two\nlines", "a\tb\r\nc", "", "x\n\ny\n"];

/// Find what texts, with and without special codes and wildcards, and Replace with texts.
const FIND_TEXTS: &[&str] = &["a", "b", "teh", "^p", "^t", "b^pa", "a^?", "^#", "^w", "?", "[a-c]", "a*b", "(a)(b)", "<a", "e\u{301}", "\u{1F600}"];
const REPLACE_TEXTS: &[&str] = &["X", "", "^p", "^t", "^&^&", "\\1", "^l", "^m", "two words"];

/// The list and numbering commands (Home › Paragraph and the item's context menu).
fn random_list_command(r: &mut Rng) -> (&'static str, Value) {
    match r.below(9) {
        0 => ("para.restartNumbering", json!({})),
        1 => ("para.continueNumbering", json!({})),
        2 => ("para.setNumberingValue", json!({"value": r.below(12)})),
        3 => ("para.listLevel", json!({"level": r.below(4)})),
        4 => ("para.outdent", json!({})),
        5 => ("para.multilevel", json!({"kind": (["legal", "outline"][r.below(2)])})),
        6 => ("para.numbering", json!({"kind": (["numbered", "numberedParen", "lowerLetter", "upperLetter", "lowerRoman"][r.below(5)])})),
        7 => ("para.bullets", json!({"off": true})),
        _ => ("list.define", json!({"levels": [{"format": "upperRoman", "text": "%1)"}, {"format": "lowerLetter", "text": "%2."}]})),
    }
}

/// One random editing command, as the keyboard, ribbon or mouse would send it.
fn random_command(r: &mut Rng) -> (&'static str, Value) {
    let ext = json!({"extend": r.below(3) == 0});
    let find = |r: &mut Rng| {
        let wild = r.below(4) == 0;
        json!({
            "text": FIND_TEXTS[r.below(FIND_TEXTS.len())],
            "with": REPLACE_TEXTS[r.below(REPLACE_TEXTS.len())],
            "matchCase": r.below(3) == 0,
            "wholeWord": r.below(3) == 0,
            "wildcards": wild,
            "special": r.below(5) != 0,
        })
    };
    match r.below(60) {
        0..=7 => ("text.insert", json!({"text": (PIECES[r.below(PIECES.len())])})),
        8 => ("text.newParagraph", json!({})),
        9 => ("text.lineBreak", json!({})),
        10 | 11 => ("text.backspace", json!({})),
        12 => ("text.delete", json!({})),
        13 => ("text.deleteWordBack", json!({})),
        14 => ("text.deleteWordForward", json!({})),
        15 => ("caret.left", ext),
        16 => ("caret.right", ext),
        17 => ("caret.up", ext),
        18 => ("caret.down", ext),
        19 => (["caret.home", "caret.end", "caret.wordLeft", "caret.wordRight"][r.below(4)], ext),
        20 => (["caret.docStart", "caret.docEnd", "caret.paraUp", "caret.paraDown"][r.below(4)], ext),
        21 => ("edit.undo", json!({})),
        22 => ("edit.redo", json!({})),
        23 => ("edit.cut", json!({})),
        24 => ("edit.copy", json!({})),
        25 => ("edit.paste", json!({})),
        26 => (["format.bold", "format.italic", "format.underline", "format.clear"][r.below(4)], json!({})),
        27 => ("format.size", json!({"size": 8 + r.below(30)})),
        28 => (["para.alignCenter", "para.bullets", "para.numbering", "para.indent"][r.below(4)], json!({})),
        29 => ("text.tab", json!({})),
        30 => ("text.backTab", json!({})),
        31 => ("select.all", json!({})),
        32 => (["select.word", "select.paragraph", "select.sentence"][r.below(3)], json!({})),
        33 => ("insert.table", json!({"rows": 1 + r.below(3), "cols": 1 + r.below(3)})),
        34 => ("review.trackChanges", json!({})),
        35 => ("edit.replaceAll", json!({"text": (PIECES[r.below(PIECES.len())]), "with": (PIECES[r.below(PIECES.len())])})),
        36 => ("format.changeCase", json!({"mode": (["upper", "lower", "title", "toggle", "sentence"][r.below(5)])})),
        37 => ("edit.pasteText", json!({})),
        38 => (["text.pageBreak", "text.nbsp", "text.emDash"][r.below(3)], json!({})),
        39 => (TABLE_EDITS[r.below(TABLE_EDITS.len())], json!({})),
        40 => (["review.accept", "review.reject", "review.acceptAll", "review.rejectAll", "review.nextChange"][r.below(5)], json!({})),
        41 => match r.below(5) {
            0 => ("review.newComment", json!({"text": "note"})),
            1 => ("review.reply", json!({"id": r.below(3), "text": "re"})),
            2 => ("review.resolveComment", json!({})),
            3 => ("review.deleteComment", json!({})),
            _ => ("review.deleteComment", json!({"all": true})),
        },
        42 => (["edit.copyFormat", "edit.pasteFormat", "table.deleteRow", "table.merge"][r.below(4)], json!({})),
        // Other programs' clipboard: Paste keeps their formatting, Merge Formatting their
        // emphasis, Paste Special a chosen format, Keep Text Only the text.
        43 | 44 => {
            let v = json!({"text": TEXT_PAYLOADS[r.below(TEXT_PAYLOADS.len())], "html": HTML_PAYLOADS[r.below(HTML_PAYLOADS.len())], "rtf": RTF_PAYLOADS[r.below(RTF_PAYLOADS.len())]});
            match r.below(4) {
                0 => ("edit.paste", v),
                1 => ("edit.pasteMerge", v),
                2 => ("edit.paste", json!({"text": TEXT_PAYLOADS[r.below(TEXT_PAYLOADS.len())]})),
                _ => {
                    let mut v = v;
                    v["as"] = json!(["text", "html", "rtf", "formatted", "picture"][r.below(5)]);
                    ("edit.pasteSpecial", v)
                }
            }
        }
        45 => ("edit.pasteSpecial", json!({"as": (["text", "formatted", "html"][r.below(3)])})),
        46 => (["edit.pasteClipboardItem", "edit.pasteAllClipboard"][r.below(2)], json!({"index": r.below(3)})),
        47 | 48 => ("edit.replaceAll", find(r)),
        49 => ("edit.replace", find(r)),
        50 => (["edit.find", "edit.findNext", "edit.findPrevious"][r.below(3)], json!({"text": FIND_TEXTS[r.below(FIND_TEXTS.len())]})),
        51 | 52 => random_list_command(r),
        53 => (["references.footnote", "references.endnote"][r.below(2)], json!({"text": "note"})),
        54 => (["insert.header", "insert.footer", "insert.closeHeader"][r.below(3)], json!({"text": "hdr"})),
        55 => ("insert.textBox", json!({"text": "box"})),
        56 => ("layout.break", json!({"kind": (["page", "nextPage", "continuous", "column"][r.below(4)])})),
        57 => (["para.moveUp", "para.moveDown"][r.below(2)], json!({})),
        58 => ("caret.set", json!({"pos": {"block": r.below(8), "off": r.below(6)}})),
        _ => ("edit.pasteText", json!({"text": TEXT_PAYLOADS[r.below(TEXT_PAYLOADS.len())]})),
    }
}

/// Structure every editing command must keep, beyond [`check_blocks`]: a section break only in
/// a top-level body paragraph (never in a cell, a header, a note or a text box); every list
/// paragraph in a list the document defines; every footnote or endnote reference pointing at a
/// note of its kind, and no two references at the same note (a pasted copy is a note of its own);
/// every text box shown pointing at a text box story.
pub(crate) fn check_structure(d: &Document, what: &str) {
    use wordcraft_doc::para::{InlineObject, NoteKind};
    use wordcraft_doc::{PartKind, StoryRef};
    let stories: Vec<StoryRef> = std::iter::once(StoryRef::Body).chain(d.parts.keys().map(|k| StoryRef::Part(*k))).collect();
    let mut notes_seen = std::collections::BTreeMap::new();
    for story in stories {
        let is_note = match story {
            StoryRef::Body => false,
            StoryRef::Part(id) => d.parts.get(&id).is_some_and(|p| matches!(p.kind, PartKind::Footnote | PartKind::Endnote)),
        };
        for path in d.para_paths(story) {
            let Some(p) = d.para(story, &path) else { continue };
            if p.section.is_some() {
                assert!(story == StoryRef::Body && path.depth() == 0, "{what}: a section break in {story:?} at {path:?} ({:?})", p.text);
            }
            if let Some(n) = p.props.numbering.filter(|n| n.num != 0) {
                assert!(d.numbering.num(n.num).is_some(), "{what}: paragraph {path:?} in list {} which doesn't exist", n.num);
            }
            for o in &p.objects {
                match o {
                    InlineObject::NoteRef { kind, id, .. } => {
                        let want = if *kind == NoteKind::Footnote { PartKind::Footnote } else { PartKind::Endnote };
                        assert_eq!(d.parts.get(id).map(|pt| pt.kind), Some(want), "{what}: note reference {id} in {story:?} {path:?}");
                        // A note's own mark isn't a reference to it.
                        if !is_note && let Some(first) = notes_seen.insert(*id, (story, path.clone())) {
                            panic!("{what}: note {id} referenced twice: {first:?} and {story:?} {path:?}");
                        }
                    }
                    o => {
                        for id in o.text_boxes() {
                            assert_eq!(d.parts.get(&id).map(|pt| pt.kind), Some(PartKind::TextBox), "{what}: text box {id} in {story:?} {path:?}");
                        }
                    }
                }
            }
        }
    }
}

/// Every paragraph's runs cover its text exactly on character boundaries, and it has one
/// object per object placeholder.
fn check_blocks(blocks: &Blocks, what: &str) {
    for b in blocks.iter() {
        match &**b {
            Block::Para(p) => {
                let total: usize = p.runs.iter().map(|r| r.len).sum();
                assert_eq!(total, p.text.len(), "{what}: runs don't cover {:?}", p.text);
                let mut at = 0;
                for r in &p.runs {
                    assert!(r.len > 0, "{what}: empty run in {:?}", p.text);
                    at += r.len;
                    assert!(p.text.is_char_boundary(at), "{what}: run splits a character in {:?}", p.text);
                }
                let placeholders = p.text.chars().filter(|c| *c == wordcraft_doc::para::OBJ).count();
                assert_eq!(placeholders, p.objects.len(), "{what}: objects out of step in {:?}", p.text);
            }
            Block::Table(t) => {
                assert!(!t.rows.is_empty(), "{what}: a table without rows");
                for row in &t.rows {
                    assert!(!row.cells.is_empty(), "{what}: a row without cells");
                    for c in &row.cells {
                        assert!(!c.blocks.is_empty(), "{what}: a cell without paragraphs");
                        check_blocks(&c.blocks, what);
                    }
                }
            }
        }
    }
}

fn check_pos(s: &Session, p: &Pos, what: &str) {
    let para = s.doc.para_at(p).unwrap_or_else(|| panic!("{what}: {p:?} isn't in a paragraph"));
    assert!(p.off <= para.len() && para.text.is_char_boundary(p.off), "{what}: {p:?} isn't a boundary of {:?}", para.text);
}

pub(crate) fn check(s: &Session, what: &str) {
    check_blocks(&s.doc.body, what);
    for part in s.doc.parts.values() {
        check_blocks(&part.blocks, what);
    }
    check_structure(&s.doc, what);
    check_pos(s, &s.sel.anchor, what);
    check_pos(s, &s.sel.focus, what);
}

/// The layout cache keys a paragraph's layout by its revision (`Paragraph::rev`): a paragraph
/// changed without a new one (`Paragraph::touch`) is shown as it was. `seen` maps each revision
/// met so far (in any document of the session, undo steps too) to a hash of its content; returns
/// the paragraphs of `d` whose revision stood for other content before.
pub(crate) fn changed_under_old_revision(seen: &mut std::collections::HashMap<u64, u64>, d: &Document) -> Vec<String> {
    use std::hash::{Hash, Hasher};
    fn walk(bl: &Blocks, seen: &mut std::collections::HashMap<u64, u64>, out: &mut Vec<String>, depth: usize) {
        for b in bl {
            match &**b {
                Block::Para(p) => {
                    // (Without the section it ends: pagination reads that from the document, not
                    // from paragraph layouts.)
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    let mut q = p.clone();
                    q.section = None;
                    serde_json::to_string(&q).unwrap_or_default().hash(&mut h);
                    let h = h.finish();
                    match seen.entry(p.rev) {
                        std::collections::hash_map::Entry::Occupied(e) if *e.get() != h => {
                            out.push(format!("revision {} now {:?}", p.rev, p.text));
                        }
                        std::collections::hash_map::Entry::Occupied(_) => {}
                        std::collections::hash_map::Entry::Vacant(e) => {
                            e.insert(h);
                        }
                    }
                }
                Block::Table(t) if depth < 64 => {
                    for c in t.rows.iter().flat_map(|r| &r.cells) {
                        walk(&c.blocks, seen, out, depth + 1);
                    }
                }
                Block::Table(_) => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&d.body, seen, &mut out, 0);
    for part in d.parts.values() {
        walk(&part.blocks, seen, &mut out, 0);
    }
    out
}

fn serialized(d: &Document) -> String {
    serde_json::to_string(d).unwrap()
}

/// A document with a bit of everything to start from.
fn start(r: &mut Rng) -> Session {
    start_kind(r.below(3))
}

/// An empty document, a few paragraphs with a list and a table, the sample document, or the
/// paragraphs without the table (3).
fn start_kind(kind: usize) -> Session {
    let mut s = Session::new(Document::new());
    match kind {
        0 => {}
        1 | 3 => {
            for k in ["Hello wor", "ld\n", "* item\n", "second\n\n"] {
                for c in k.chars() {
                    let _ = if c == '\n' { s.run("text.newParagraph", &json!({})) } else { s.run("text.insert", &json!({"text": c.to_string()})) };
                }
            }
            if kind == 1 {
                let _ = s.run("insert.table", &json!({"rows": 2, "cols": 2}));
                let _ = s.run("text.insert", &json!({"text": "cell"}));
            }
            let _ = s.run("caret.docStart", &json!({}));
        }
        _ => {
            s = Session::new(crate::sample::sample_document());
        }
    }
    s.reset_history();
    s
}

#[test]
fn random_edits_keep_the_document_valid_and_undo_redo_exact() {
    let seeds: u64 = std::env::var("WC_FUZZ_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let steps: usize = std::env::var("WC_FUZZ_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let mut stats: std::collections::BTreeMap<&str, (u32, u32)> = Default::default();
    for seed in 1..=seeds {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut s = start(&mut r);
        let original = s.doc.clone();
        let mut log = Vec::new();
        let mut revs = std::collections::HashMap::new();
        let _ = changed_under_old_revision(&mut revs, &s.doc);
        for i in 0..steps {
            let (id, v) = random_command(&mut r);
            log.push(format!("{id} {v}"));
            let res = s.run(id, &v);
            let stale = changed_under_old_revision(&mut revs, &s.doc);
            assert!(stale.is_empty(), "seed {seed} step {i}: changed without a new revision: {stale:?}\n{}", log.join("\n"));
            if std::env::var("WC_FUZZ_STATS").is_ok() {
                let e = stats.entry(id).or_insert((0, 0));
                if res.is_ok() { e.0 += 1 } else { e.1 += 1 }
            }
            if let Err(e) = &res {
                assert!(!e.to_string().contains("panicked"), "seed {seed} step {i}: {e}\n{}", log.join("\n"));
            }
            check(&s, &format!("seed {seed} step {i} after {id} {v}\n{}", log.join("\n")));
        }
        let end = s.doc.clone();
        let mut undone = 0;
        while s.can_undo() {
            s.run("edit.undo", &json!({})).unwrap();
            check(&s, &format!("seed {seed} undo {undone}"));
            undone += 1;
        }
        assert_eq!(serialized(&s.doc), serialized(&original), "seed {seed}: undoing everything\n{}", log.join("\n"));
        for _ in 0..undone {
            s.run("edit.redo", &json!({})).unwrap();
            check(&s, &format!("seed {seed} redo"));
        }
        assert_eq!(serialized(&s.doc), serialized(&end), "seed {seed}: redoing everything\n{}", log.join("\n"));
    }
    if !stats.is_empty() {
        eprintln!("{stats:?}");
    }
}

#[test]
fn random_tracked_edits_reject_back_to_the_original() {
    let seeds: u64 = std::env::var("WC_FUZZ_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let steps: usize = std::env::var("WC_FUZZ_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    // Everything but undo, toggling tracking, accepting or rejecting, comments (not revisions)
    // and merging or splitting cells (not tracked, as in Word).
    // Section breaks and paragraph moves aren't tracked (docs/typing-parity.md, Known gaps), so
    // Reject All can't take them back; headers, footers and text boxes are other stories.
    let skip = |id: &str| {
        matches!(id, "review.trackChanges" | "edit.undo" | "edit.redo" | "table.merge" | "layout.break" | "para.moveUp" | "para.moveDown")
            || id.starts_with("review.")
    };
    let begin = |seed: u64| {
        let mut r = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1);
        // Tables included: rows and cells inserted or deleted are tracked (#1998).
        let s = start_kind(r.below(4));
        (r, s)
    };
    for seed in 1..=seeds {
        let (mut r, mut s) = begin(seed);
        let original = norm(&s.doc.body, REJECTED);
        s.run("review.trackChanges", &json!({})).unwrap();
        let mut log = Vec::new();
        let mut revs = std::collections::HashMap::new();
        let _ = changed_under_old_revision(&mut revs, &s.doc);
        for _ in 0..steps {
            let (id, v) = random_command(&mut r);
            if skip(id) {
                continue;
            }
            log.push(format!("{id} {v}"));
            let _ = s.run(id, &v);
            let stale = changed_under_old_revision(&mut revs, &s.doc);
            assert!(stale.is_empty(), "seed {seed}: changed without a new revision: {stale:?}\n{}", log.join("\n"));
            check(&s, &format!("seed {seed}\n{}", log.join("\n")));
        }
        s.run("review.rejectAll", &json!({})).unwrap();
        check(&s, &format!("seed {seed} rejected\n{}", log.join("\n")));
        if norm(&s.doc.body, REJECTED) != original {
            // Shrink the failing sequence: drop steps while it still fails.
            let replay = |cmds: &[(String, Value)]| -> bool {
                let (_, mut s) = begin(seed);
                let original = norm(&s.doc.body, REJECTED);
                s.run("review.trackChanges", &json!({})).unwrap();
                for (id, v) in cmds {
                    let _ = s.run(id, v);
                }
                s.run("review.rejectAll", &json!({})).unwrap();
                norm(&s.doc.body, REJECTED) != original
            };
            let cmds = shrink(&log, &replay);
            let (_, mut s) = begin(seed);
            let original = norm(&s.doc.body, REJECTED);
            s.run("review.trackChanges", &json!({})).unwrap();
            for (id, v) in &cmds {
                let _ = s.run(id, v);
            }
            s.run("review.rejectAll", &json!({})).unwrap();
            let got = norm(&s.doc.body, REJECTED);
            panic!("seed {seed}: Reject All doesn't restore the document after\n{cmds:#?}\n{}", first_difference(&got, &original));
        }
    }
}

/// Drop steps from `log` (`id params` lines) while `fails` still holds.
fn shrink(log: &[String], fails: &dyn Fn(&[(String, Value)]) -> bool) -> Vec<(String, Value)> {
    let mut cmds: Vec<(String, Value)> = log
        .iter()
        .map(|l| {
            let (id, v) = l.split_once(' ').unwrap();
            (id.to_string(), serde_json::from_str(v).unwrap())
        })
        .collect();
    let mut i = 0;
    while i < cmds.len() {
        let mut fewer = cmds.clone();
        fewer.remove(i);
        if fails(&fewer) {
            cmds = fewer;
        } else {
            i += 1;
        }
    }
    cmds
}

/// What [`norm`] compares. Never the formatting of paragraph marks (their revisions it does):
/// formatting a word that is all a paragraph's text formats its mark too, but tracked the
/// paragraph also holds deleted text; emptying a paragraph gives its mark the formatting of the
/// text deleted (an automatic correction of typed text, say), which isn't a tracked change; and
/// Enter leaves the mark's formatting on the first part while its revisions go with the second.
#[derive(Clone, Copy)]
struct Compare {
    /// Table widths, rounded (rejecting an inserted column scales the others back, up to
    /// rounding). Not after accepting: tracked, a column inserted beside deleted ones shares the
    /// width with them.
    widths: bool,
}

const ACCEPTED: Compare = Compare { widths: false };
const REJECTED: Compare = Compare { widths: true };

/// The body as a reader sees its structure: runs that look alike merged, compared as `how` says.
fn norm(blocks: &Blocks, how: Compare) -> Value {
    let mut b = blocks.clone();
    norm_blocks(&mut b, how);
    serde_json::to_value(&b).unwrap()
}

fn norm_blocks(blocks: &mut Blocks, how: Compare) {
    let widths = how.widths;
    let round = |w: f32| if widths { (w * 100.0).round() / 100.0 } else { 0.0 };
    for b in blocks.iter_mut() {
        match std::sync::Arc::make_mut(b) {
            Block::Para(p) => {
                p.normalize();
                {
                    p.mark = wordcraft_doc::props::CharProps { ins: p.mark.ins, del: p.mark.del, ..Default::default() };
                }
            }
            Block::Table(t) => {
                t.grid.iter_mut().for_each(|g| *g = round(*g));
                for row in &mut t.rows {
                    for c in &mut row.cells {
                        c.props.width = c.props.width.map(round).filter(|_| widths);
                        norm_blocks(&mut c.blocks, how);
                    }
                }
            }
        }
    }
}

/// The first block where two [`norm`]s differ, both ways, and the block counts.
fn first_difference(got: &Value, want: &Value) -> String {
    let (g, w) = (got.as_array().cloned().unwrap_or_default(), want.as_array().cloned().unwrap_or_default());
    let i = g.iter().zip(&w).position(|(a, b)| a != b).unwrap_or(g.len().min(w.len()));
    format!(
        "{} blocks vs {} untracked; block {i}:\n  got  {}\n  want {}",
        g.len(),
        w.len(),
        g.get(i).map_or("-".into(), Value::to_string),
        w.get(i).map_or("-".into(), Value::to_string)
    )
}

/// The paragraph text a reader sees with tracked deletions left out, and the paragraph offset of
/// each of its bytes.
fn live_text(p: &wordcraft_doc::Paragraph) -> (String, Vec<usize>) {
    let (mut text, mut at) = (String::new(), Vec::new());
    for (r, c) in p.run_ranges() {
        if c.del.is_some() {
            continue;
        }
        let Some(t) = p.text.get(r.clone()) else { continue };
        text.push_str(t);
        at.extend(r);
    }
    (text, at)
}

/// Where the word `w` is in the body's text that isn't deleted: its first character and the end
/// of its last (`w` is ASCII and found as a whole word).
fn find_word(d: &Document, w: &str) -> Option<(Pos, Pos)> {
    let body = wordcraft_doc::StoryRef::Body;
    for path in d.para_paths(body) {
        let Some(p) = d.para(body, &path) else { continue };
        let (text, at) = live_text(p);
        for (i, _) in text.match_indices(w) {
            let j = i + w.len();
            let alone = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
            if alone(text[..i].chars().next_back()) && alone(text[j..].chars().next()) {
                let (a, b) = (*at.get(i)?, *at.get(j - 1)? + 1);
                return Some((Pos { story: body, path: path.clone(), off: a }, Pos { story: body, path: path.clone(), off: b }));
            }
        }
    }
    None
}

/// A document of distinct words: two paragraphs, a bulleted list, a 2×3 table and a closing
/// paragraph; AutoCorrect off so typing changes nothing but what is typed.
fn word_doc() -> Session {
    let mut s = Session::new(Document::new());
    s.prefs.autocorrect.enabled = false;
    let run = |s: &mut Session, id: &str, v: Value| {
        s.run(id, &v).unwrap_or_else(|e| panic!("{id}: {e}"));
    };
    run(&mut s, "text.insert", json!({"text": "wa wb wc", "raw": true}));
    for line in ["wd we", "wf wg", "wh"] {
        run(&mut s, "text.newParagraph", json!({}));
        run(&mut s, "text.insert", json!({"text": line, "raw": true}));
    }
    run(&mut s, "select.range", json!({"anchor": {"block": 2, "off": 0}, "focus": {"block": 3, "off": 1}}));
    run(&mut s, "para.bullets", json!({}));
    run(&mut s, "caret.docEnd", json!({}));
    // Enter twice: the second, on the empty item, ends the list.
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "text.insert", json!({"text": "wi wj", "raw": true}));
    run(&mut s, "caret.home", json!({}));
    run(&mut s, "insert.table", json!({"rows": 2, "cols": 3}));
    for (i, w) in ["ta", "tb", "tc", "td", "te", "tf"].iter().enumerate() {
        if i > 0 {
            run(&mut s, "text.tab", json!({}));
        }
        run(&mut s, "text.insert", json!({"text": w, "raw": true}));
    }
    s.reset_history();
    s
}

/// Whether the paragraph at `at` joins another once deletions are accepted: its mark, or the
/// mark of the paragraph before it in the same container (past tables whose every row is
/// deleted), is a tracked deletion.
fn joined(d: &Document, at: &Pos) -> bool {
    if d.para_at(at).is_some_and(|p| p.mark.del.is_some()) {
        return true;
    }
    let mut i = at.path.last();
    while i > 0 {
        i -= 1;
        match d.block(at.story, &at.path.with_last(i)) {
            Some(Block::Table(t)) if t.rows.iter().all(|r| r.props.del.is_some()) => {}
            Some(Block::Para(p)) => return p.mark.del.is_some(),
            _ => return false,
        }
    }
    false
}

/// Whether accepting deletions takes the paragraph at `at` out as it is: it [`joined`] another,
/// or it is in a deleted table row or cell.
fn goes(d: &Document, at: &Pos) -> bool {
    let mut path = at.path.clone();
    while let Some((tp, r, c)) = path.cell() {
        let row = d.table(at.story, &tp).and_then(|t| t.rows.get(r));
        if row.is_some_and(|x| x.props.del.is_some() || x.cells.get(c).is_some_and(|cl| cl.props.del.is_some())) {
            return true;
        }
        path = tp;
    }
    joined(d, at)
}

/// One edit at a word both documents still have, in both: `None` when it can't be made there.
fn lockstep_edit(r: &mut Rng, plain: &mut Session, tracked: &mut Session, words: &mut Vec<String>, n: &mut usize) -> Option<String> {
    let pick = |r: &mut Rng, words: &[String]| words.get(r.below(words.len().max(1))).cloned();
    let w = pick(r, words)?;
    let (Some((pa, pb)), Some((ta, tb))) = (find_word(&plain.doc, &w), find_word(&tracked.doc, &w)) else {
        words.retain(|x| *x != w);
        return None;
    };
    let both = |plain: &mut Session, tracked: &mut Session, sel: (Selection, Selection), id: &str, v: Value| {
        plain.sel = sel.0;
        tracked.sel = sel.1;
        let a = plain.run(id, &v).is_ok();
        let b = tracked.run(id, &v).is_ok();
        (a, b)
    };
    let caret = |at: &Pos| Selection::caret(at.clone());
    let span = |a: &Pos, b: &Pos| Selection { anchor: a.clone(), focus: b.clone() };
    let in_cell = pa.path.cell().is_some() && ta.path.cell().is_some();
    let (desc, ok) = match r.below(14) {
        0..=2 => {
            *n += 1;
            let new = format!("n{n}");
            let ok = both(plain, tracked, (caret(&pb), caret(&tb)), "text.insert", json!({"text": format!(" {new}"), "raw": true}));
            words.push(new.clone());
            (format!("type {new} after {w}"), ok)
        }
        3 | 4 => {
            let id = if r.below(2) == 0 { "text.delete" } else { "text.backspace" };
            words.retain(|x| *x != w);
            (format!("{id} {w}"), both(plain, tracked, (span(&pa, &pb), span(&ta, &tb)), id, json!({})))
        }
        5 => {
            // From this word to another in the same container, in both documents.
            let w2 = pick(r, words)?;
            let (qa, qb) = find_word(&plain.doc, &w2)?;
            let (ua, ub) = find_word(&tracked.doc, &w2)?;
            if qa.path.parent() != pa.path.parent() || ua.path.parent() != ta.path.parent() || w2 == w || (qa < pa) != (ua < ta) {
                return None;
            }
            let (ps, ts) = if qa < pa { (span(&qa, &pb), span(&ua, &tb)) } else { (span(&pa, &qb), span(&ta, &ub)) };
            // From a paragraph's start into a later one: untracked, the first paragraph keeps its
            // formatting; accepted, the emptied paragraph goes and the last keeps its own (an
            // open question about Word).
            if ps.anchor.off == 0 && (ps.anchor.path != ps.focus.path || ts.anchor.path != ts.focus.path) {
                return None;
            }
            let gone: Vec<String> =
                words.iter().filter(|x| find_word(&plain.doc, x).is_some_and(|(xa, _)| xa >= ps.anchor && xa < ps.focus)).cloned().collect();
            words.retain(|x| !gone.contains(x));
            (format!("delete {w}..{w2}"), both(plain, tracked, (ps, ts), "text.delete", json!({})))
        }
        // Enter and list changes go to a paragraph that is one paragraph in both: tracked, one
        // whose mark (or the one before's) is deleted is still apart, with its own formatting.
        6 if !joined(&tracked.doc, &ta) => {
            (format!("enter after {w}"), both(plain, tracked, (caret(&pb), caret(&tb)), "text.newParagraph", json!({})))
        }
        7 => {
            let id = ["format.bold", "format.italic", "format.underline"][r.below(3)];
            (format!("{id} {w}"), both(plain, tracked, (span(&pa, &pb), span(&ta, &tb)), id, json!({})))
        }
        8 => {
            let size = 8 + r.below(20);
            (format!("size {size} {w}"), both(plain, tracked, (span(&pa, &pb), span(&ta, &tb)), "format.size", json!({"size": size})))
        }
        // A list continues the one before it: that paragraph mustn't go or join another either.
        9 if !joined(&tracked.doc, &ta)
            && !tracked.doc.prev_para(ta.story, &ta.path).is_some_and(|q| goes(&tracked.doc, &Pos { path: q, ..ta.clone() })) =>
        {
            let id = ["para.bullets", "para.numbering"][r.below(2)];
            (format!("{id} at {w}"), both(plain, tracked, (caret(&pa), caret(&ta)), id, json!({})))
        }
        10 | 11 if in_cell => {
            let id = TABLE_EDITS[r.below(TABLE_EDITS.len())];
            // Delete Cells leaves the grid as it is untracked, while accepting a column of deleted
            // cells takes the column out.
            if id == "table.deleteCells" {
                return None;
            }
            (format!("{id} at {w}"), both(plain, tracked, (caret(&pa), caret(&ta)), id, json!({})))
        }
        12 if !in_cell && pa.path.depth() == 0 && ta.path.depth() == 0 && !joined(&tracked.doc, &ta) => {
            let (rows, cols) = (1 + r.below(2), 1 + r.below(3));
            let ok = both(plain, tracked, (caret(&pb), caret(&tb)), "insert.table", json!({"rows": rows, "cols": cols}));
            *n += 1;
            let new = format!("c{n}");
            let sels = (plain.sel.clone(), tracked.sel.clone());
            let ok2 = both(plain, tracked, sels, "text.insert", json!({"text": new, "raw": true}));
            words.push(new.clone());
            (format!("insert table {rows}x{cols} after {w}, type {new}"), (ok.0 && ok2.0, ok.1 && ok2.1))
        }
        13 => {
            *n += 1;
            let new = format!("r{n}");
            words.retain(|x| *x != w);
            words.push(new.clone());
            (format!("replace {w} with {new}"), both(plain, tracked, (caret(&pa), caret(&ta)), "edit.replaceAll", json!({"text": w, "with": new})))
        }
        _ => return None,
    };
    assert_eq!(ok.0, ok.1, "{desc}: tracked {} and untracked {}", ok.1, ok.0);
    Some(desc)
}

/// With Track Changes on, any edits accepted give what the same edits give with tracking off:
/// text, paragraph and list formatting, and table rows, columns and tables inserted or deleted.
/// The edits are made at words both documents still have (a caret position in the tracked
/// document, where deleted text is still there, means a different place untracked). Rejecting
/// gives the document back, and undoing everything and redoing it are exact.
#[test]
fn random_tracked_edits_accept_like_the_same_edits_untracked() {
    let seeds: u64 = std::env::var("WC_FUZZ_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let steps: usize = std::env::var("WC_FUZZ_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    for seed in 1..=seeds {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x51ED | 1);
        let mut plain = word_doc();
        let mut tracked = word_doc();
        let original = (serialized(&tracked.doc), norm(&tracked.doc.body, REJECTED));
        tracked.doc.settings.track_changes = true;
        let mut words: Vec<String> =
            ["wa", "wb", "wc", "wd", "we", "wf", "wg", "wh", "wi", "wj", "ta", "tb", "tc", "td", "te", "tf"].map(String::from).to_vec();
        let mut n = 0;
        let mut log = Vec::new();
        for _ in 0..steps {
            let Some(desc) = lockstep_edit(&mut r, &mut plain, &mut tracked, &mut words, &mut n) else { continue };
            log.push(desc);
            let what = format!("seed {seed}\n{}", log.join("\n"));
            // WC_FUZZ_DEBUG=1: the tracked document's blocks when a check fails.
            let dump = |d: &Document| {
                if std::env::var("WC_FUZZ_DEBUG").is_ok() {
                    d.body.iter().enumerate().for_each(|(i, b)| eprintln!("tracked {i}: {}", serde_json::to_string(b).unwrap_or_default()));
                }
            };
            check(&tracked, &what);
            let mut accepted = Session::new(tracked.doc.clone());
            accepted.run("review.acceptAll", &json!({})).unwrap();
            check(&accepted, &what);
            let (got, want) = (norm(&accepted.doc.body, ACCEPTED), norm(&plain.doc.body, ACCEPTED));
            if got != want {
                dump(&tracked.doc);
                panic!("{what}: Accept All differs from the edits untracked:\n{}", first_difference(&got, &want));
            }
            let mut rejected = Session::new(tracked.doc.clone());
            rejected.run("review.rejectAll", &json!({})).unwrap();
            let got = norm(&rejected.doc.body, REJECTED);
            if got != original.1 {
                dump(&tracked.doc);
                panic!("{what}: Reject All doesn't give the document back:\n{}", first_difference(&got, &original.1));
            }
        }
        let end = serialized(&tracked.doc);
        let mut undone = 0;
        while tracked.can_undo() {
            tracked.run("edit.undo", &json!({})).unwrap();
            undone += 1;
        }
        tracked.doc.settings.track_changes = false;
        assert_eq!(serialized(&tracked.doc), original.0, "seed {seed}: undoing everything\n{}", log.join("\n"));
        tracked.doc.settings.track_changes = true;
        for _ in 0..undone {
            tracked.run("edit.redo", &json!({})).unwrap();
        }
        assert_eq!(serialized(&tracked.doc), end, "seed {seed}: redoing everything\n{}", log.join("\n"));
    }
}

/// Format Painter copies formatting only: painting from a tracked insertion or a hyperlink
/// doesn't make the target a tracked insertion or a link, and the target keeps its own.
#[test]
fn format_painter_copies_formatting_not_revisions_or_links() {
    let run = |s: &mut Session, id: &str, v: Value| {
        s.run(id, &v).unwrap_or_else(|e| panic!("{id}: {e}"));
    };
    let mut s = Session::new(Document::from_text("plain words here"));
    // "here" becomes a bold tracked insertion.
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "review.trackChanges", json!({"value": true}));
    run(&mut s, "text.insert", json!({"text": " new", "raw": true}));
    run(&mut s, "review.trackChanges", json!({"value": false}));
    run(&mut s, "select.text", json!({"text": "new"}));
    run(&mut s, "format.bold", json!({"value": true}));
    run(&mut s, "insert.link", json!({"url": "https://example.com"}));
    let new = s.doc.para_at(&Pos::body(0, 0)).unwrap().text.find("new").unwrap();
    let pick = |s: &mut Session| {
        s.run("caret.set", &json!({"pos": {"block": 0, "off": new + 1}})).unwrap();
        s.run("edit.formatPainter", &json!({})).unwrap();
    };
    // With a selection, and with the caret in a word.
    pick(&mut s);
    run(&mut s, "select.text", json!({"text": "plain"}));
    run(&mut s, "edit.pasteFormat", json!({}));
    pick(&mut s);
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 8}}));
    run(&mut s, "edit.pasteFormat", json!({}));
    let p = s.doc.para_at(&Pos::body(0, 0)).unwrap();
    for off in [0, 8] {
        let c = p.props_of_char(off);
        assert_eq!(c.bold, Some(true), "{off}: the formatting is painted");
        assert_eq!((c.ins, c.del, c.link.is_some()), (None, None, false), "{off}: {c:?}");
    }
    // Painting onto a tracked insertion keeps it tracked.
    run(&mut s, "caret.set", json!({"pos": {"block": 0, "off": 0}}));
    run(&mut s, "edit.formatPainter", json!({}));
    run(&mut s, "select.range", json!({"anchor": {"block": 0, "off": new}, "focus": {"block": 0, "off": new + 3}}));
    run(&mut s, "edit.pasteFormat", json!({}));
    let c = s.doc.para_at(&Pos::body(0, 0)).unwrap().props_of_char(new).clone();
    assert!(c.ins.is_some() && c.link.is_some(), "{c:?}");
}

/// What AutoSave, the recovery copy and the word count do: a copy of the document goes to a
/// worker thread and is written out there while the window goes on editing. The copy shares
/// every block, row and part with the document, so each edit must copy what it writes to
/// (`Arc::make_mut`, `Shared`): the copy, written out on the worker meanwhile and again after,
/// is the document as it was when copied, and the layouts don't share anything that changes
/// either (`Send + Sync`).
#[test]
fn a_copy_saved_on_another_thread_keeps_what_it_had_while_editing_goes_on() {
    fn shareable<T: Send + Sync>() {}
    shareable::<Document>();
    shareable::<wordcraft_layout::DocLayout>();
    let mut r = Rng(0x5DEE_CE66_D1CE_4E5B);
    let mut s = start_kind(2);
    let _ = s.layout();
    for round in 0..6 {
        let copy = s.doc.clone();
        let expected = serialized(&copy);
        let worker = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..20 {
                seen.push(serialized(&copy));
            }
            (copy, seen)
        });
        for _ in 0..40 {
            let (id, v) = random_command(&mut r);
            let _ = s.run(id, &v);
            let _ = s.layout();
        }
        let (copy, seen) = worker.join().unwrap();
        assert!(seen.iter().all(|x| *x == expected), "round {round}: the copy changed while it was written out");
        assert_eq!(serialized(&copy), expected, "round {round}: the copy changed after the edits");
    }
}

/// Typing in the body of a document with a text box leaves the document's parts (headers,
/// notes, text boxes) shared with the undo step before: every edit checks for text boxes no
/// longer shown, and finding none it must not copy the parts (each undo step held its own copy).
#[test]
fn edits_beside_a_text_box_leave_the_parts_shared() {
    let mut s = Session::new(Document::from_text("Before the box"));
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("insert.textBox", &json!({"text": "Boxed"})).unwrap();
    s.run("caret.set", &json!({"pos": {"block": 0, "off": 0}})).unwrap();
    for id in ["text.insert", "text.insert", "text.backspace", "text.newParagraph"] {
        let before = s.doc.clone();
        s.run(id, &json!({"text": "a"})).unwrap();
        assert!(wordcraft_doc::Shared::ptr_eq(&before.parts, &s.doc.parts), "{id} copied the parts");
    }
    assert!(s.doc.parts.values().any(|p| p.kind == wordcraft_doc::PartKind::TextBox), "the box is still there");
}

/// Commands that change how the document paginates beyond typing: section breaks, page setup
/// and columns (of the caret's section, wherever the layout last resumed), first-page and odd
/// and even headers, tracked changes shown with markup or without, hidden text, page fields
/// and fields updated, content controls, footnotes, comments, and the view: Draft, Web, Read
/// and Outline and back, zoom, field codes, formatting marks.
fn random_layout_command(r: &mut Rng) -> (&'static str, Value) {
    match r.below(26) {
        0 | 1 => ("layout.break", json!({"kind": (["page", "column", "nextPage", "continuous", "evenPage", "oddPage"][r.below(6)])})),
        2 => ("layout.margins", json!({"preset": (["normal", "narrow", "wide", "moderate"][r.below(4)])})),
        3 => ("layout.orientation", json!({"value": (["landscape", "portrait"][r.below(2)])})),
        4 => ("layout.size", json!({"name": (["Letter", "A4", "Legal"][r.below(3)])})),
        5 => ("layout.columns", json!({"count": 1 + r.below(3)})),
        6 => ("layout.differentFirstPage", json!({})),
        7 => ("layout.differentOddEven", json!({})),
        8 => ("review.trackChanges", json!({})),
        9 => ("review.markup", json!({"value": (["all", "simple", "noMarkup", "original"][r.below(4)])})),
        10 => ("format.hidden", json!({})),
        11 => ("view.marks", json!({})),
        12 => ("references.updateFields", json!({})),
        13 => ("insert.field", json!({"instr": (["PAGE", "SECTIONPAGES", "DATE", "AUTHOR"][r.below(4)])})),
        14 => (["developer.checkBox", "developer.plainText", "developer.dropDown"][r.below(3)], json!({})),
        15 => ("developer.toggleCheckBox", json!({})),
        16 => (
            "view.zoom",
            [json!({"value": 50}), json!({"value": 150}), json!({"value": "pageWidth"}), json!({"value": "multiplePages"})][r.below(4)].clone(),
        ),
        17 => (["view.draft", "view.webLayout", "view.readMode", "view.outline"][r.below(4)], json!({})),
        18 | 19 => ("view.printLayout", json!({})),
        20 => ("view.fieldCodes", json!({})),
        21 => ("references.footnote", json!({"text": "A note"})),
        22 => (["review.acceptAll", "review.rejectAll"][r.below(2)], json!({})),
        23 => ("insert.pageNumber", json!({"position": (["top", "bottom"][r.below(2)])})),
        24 => ("review.newComment", json!({"text": "A comment"})),
        _ => ("insert.header", json!({"text": "A header"})),
    }
}

/// The sample document three times over (fresh copies), for a few pages to paginate.
fn long_sample() -> Session {
    let mut d = crate::sample::sample_document();
    let more: Vec<_> = d.body.iter().map(|b| std::sync::Arc::new((**b).clone())).collect();
    for _ in 0..2 {
        let at = d.body.len().saturating_sub(1);
        for (k, b) in more.iter().enumerate() {
            d.body.insert(at + k, std::sync::Arc::new((**b).clone()));
        }
    }
    let mut s = Session::new(d);
    s.reset_history();
    s
}

/// Random editing, as above, mixed with [`random_layout_command`]'s commands: after every step
/// the session's layout (resumed from the last one where it can) is the one laid out from
/// scratch with the same view, and Print Markup lays out too. Undo all the way back and the
/// layout still matches.
#[test]
fn random_edits_relayout_like_a_layout_from_scratch() {
    let seeds: u64 = std::env::var("WC_FUZZ_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(10);
    let steps: usize = std::env::var("WC_FUZZ_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(50);
    let dump = |l: &wordcraft_layout::DocLayout| format!("{:?}\n{}", l.pages, l.text_boxes);
    let mut resumed = 0;
    for seed in 1..=seeds {
        let mut r = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
        let mut s = long_sample();
        let _ = s.layout();
        let mut log = Vec::new();
        let mut revs = std::collections::HashMap::new();
        let _ = changed_under_old_revision(&mut revs, &s.doc);
        let check = |s: &mut Session, what: &str, log: &[String]| {
            let inc = s.layout();
            let full = wordcraft_layout::layout(&s.doc, &mut wordcraft_layout::LayoutCache::new(), &s.layout_options());
            assert!(
                dump(&inc) == dump(&full),
                "seed {seed} {what}: {} pages, {} from scratch, first different page {:?}\n{}",
                inc.pages.len(),
                full.pages.len(),
                inc.pages.iter().zip(&full.pages).position(|(a, b)| format!("{a:?}") != format!("{b:?}")),
                log.join("\n")
            );
        };
        for i in 0..steps {
            let (id, v) = if r.below(3) == 0 { random_layout_command(&mut r) } else { random_command(&mut r) };
            log.push(format!("{id} {v}"));
            let res = s.run(id, &v);
            if let Err(e) = &res {
                assert!(!e.to_string().contains("panicked"), "seed {seed} step {i}: {e}\n{}", log.join("\n"));
            }
            let stale = changed_under_old_revision(&mut revs, &s.doc);
            assert!(stale.is_empty(), "seed {seed} step {i}: changed without a new revision: {stale:?}\n{}", log.join("\n"));
            check(&mut s, &format!("step {i}"), &log);
            resumed += usize::from(s.cache_resumed().is_some());
            if i % 10 == 9 {
                let _ = s.export_layout_with(true);
            }
        }
        let mut k = 0;
        while s.can_undo() && k < 60 {
            s.run("edit.undo", &json!({})).unwrap();
            check(&mut s, &format!("undo {k}"), &log);
            k += 1;
        }
    }
    assert!(resumed > 0, "some passes resumed");
}

/// Replays the commands in the file `WC_REPLAY` (one `id json` a line) on [`long_sample`],
/// then undoes everything, checking the layout against one from scratch after each step;
/// shrinks a failing sequence and prints it. A debugging aid for the test above.
#[test]
#[ignore]
fn replay_relayout() {
    let Some(path) = std::env::var("WC_REPLAY").ok() else { return };
    let text = std::fs::read_to_string(path).unwrap();
    let cmds: Vec<(String, Value)> =
        text.lines().filter_map(|l| l.split_once(' ')).map(|(id, v)| (id.to_string(), serde_json::from_str(v).unwrap_or(json!({})))).collect();
    let dump = |l: &wordcraft_layout::DocLayout| format!("{:?}\n{}", l.pages, l.text_boxes);
    let fails = |cmds: &[(String, Value)]| -> Option<String> {
        let mut s = long_sample();
        let _ = s.layout();
        let check = |s: &mut Session, what: String| {
            let inc = s.layout();
            let full = wordcraft_layout::layout(&s.doc, &mut wordcraft_layout::LayoutCache::new(), &s.layout_options());
            let again = wordcraft_layout::layout(&s.doc, &mut wordcraft_layout::LayoutCache::new(), &s.layout_options());
            if dump(&full) != dump(&again) {
                eprintln!("REPLAY {what}: two layouts from scratch differ");
            }
            if dump(&inc) != dump(&full) {
                for (pi, (a, b)) in inc.pages.iter().zip(&full.pages).enumerate() {
                    if format!("{a:?}") != format!("{b:?}") {
                        eprintln!("REPLAY page {pi}: items {} vs {}, body {:?} vs {:?}", a.items.len(), b.items.len(), a.body, b.body);
                        for (x, y) in a.items.iter().zip(b.items.iter()) {
                            let (fx, fy) = (format!("{x:?}"), format!("{y:?}"));
                            if fx != fy {
                                let at = fx.bytes().zip(fy.bytes()).take_while(|(a, b)| a == b).count();
                                let from = fx.floor_char_boundary(at.saturating_sub(300));
                                let to = |f: &str| f.floor_char_boundary((at + 300).min(f.len()));
                                eprintln!("  at {at}\n  inc  {}\n  full {}", &fx[from..to(&fx)], &fy[from..to(&fy)]);
                                break;
                            }
                        }
                        if format!("{:?}", a.header) != format!("{:?}", b.header) {
                            eprintln!("  headers differ");
                        }
                        break;
                    }
                }
            }
            (dump(&inc) != dump(&full)).then(|| format!("{what}: resumed {:?}", s.cache_resumed()))
        };
        for (i, (id, v)) in cmds.iter().enumerate() {
            let _ = s.run(id, v);
            if let Some(f) = check(&mut s, format!("step {i} {id}")) {
                return Some(f);
            }
        }
        let mut k = 0;
        while s.can_undo() && k < 60 {
            let label = s.undo_label().unwrap_or("").to_string();
            s.run("edit.undo", &json!({})).unwrap();
            if let Some(f) = check(&mut s, format!("undo {k} ({label})")) {
                return Some(f);
            }
            k += 1;
        }
        None
    };
    let runs: usize = std::env::var("WC_REPLAY_RUNS").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    for k in 0..runs {
        eprintln!("REPLAY run {k}: {:?}", fails(&cmds));
    }
    if std::env::var("WC_REPLAY_SHRINK").is_err() {
        return;
    }
    let Some(first) = fails(&cmds) else {
        eprintln!("REPLAY passes");
        return;
    };
    let mut cmds = cmds;
    let mut i = 0;
    while i < cmds.len() {
        let mut fewer = cmds.clone();
        fewer.remove(i);
        if fails(&fewer).is_some() {
            cmds = fewer;
        } else {
            i += 1;
        }
    }
    eprintln!("REPLAY first failure: {first}\nshrunk to ({}):", fails(&cmds).unwrap_or_default());
    for (id, v) in &cmds {
        eprintln!("{id} {v}");
    }
}
