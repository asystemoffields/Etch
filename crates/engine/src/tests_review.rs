//! Review features: tracked table rows and cells (#1998), Accept / Reject (one, all, by
//! reviewer), the Reviewing Pane, comments and Restrict Editing.

use serde_json::{Value, json};
use wordcraft_doc::{Document, Path, Pos, StoryRef, Table};

use crate::{Selection, Session};

fn run(s: &mut Session, id: &str, v: Value) -> Value {
    s.run(id, &v).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn ser(d: &Document) -> String {
    serde_json::to_string(d).unwrap()
}

/// The body without the revision list (Accept All and Reject All empty it): what a reader sees.
fn body(d: &Document) -> String {
    serde_json::to_string(&d.body).unwrap()
}

/// A session on a copy of `s`'s document, with the same author (no history).
fn fork(s: &Session) -> Session {
    let mut f = Session::new(s.doc.clone());
    f.author = s.author.clone();
    f
}

/// Upstream 706d6cc7: a comparison shown in a new document replaces the document, so the version
/// snapshots of the one it replaces go with it; a restore of one would otherwise put the previous
/// document's content into the comparison result. Shown in the original, the document is still
/// the same one and keeps its versions. A refused comparison puts them back (below).
#[test]
fn a_comparison_into_a_new_document_clears_the_replaced_documents_versions() {
    let mut s = Session::new(Document::from_text("The original text."));
    run(&mut s, "file.versions", json!({"save": "before"}));
    assert_eq!(s.versions.len(), 1);
    run(&mut s, "review.compare", json!({"text": "The revised text.", "showIn": "new"}));
    assert!(s.versions.is_empty(), "the new document starts with no versions of another one");
    assert!(s.run("file.versions", &json!({"restore": 0})).is_err(), "nothing to restore");
    assert!(s.doc.plain_text(StoryRef::Body).contains("revised"));

    let mut same = Session::new(Document::from_text("The original text."));
    run(&mut same, "file.versions", json!({"save": "before"}));
    run(&mut same, "review.compare", json!({"text": "The revised text."}));
    assert_eq!(same.versions.len(), 1, "marked up in place, the document keeps its own versions");
}

/// Upstream debb125d (#1327): a comparison that `Session::run` refuses (here: the result would
/// take the document past the paragraph limit) must leave the document's file identity (and its version snapshots) as
/// it was. The command cleared `path` and `file_name` for the new document before the refusal, and
/// the rollback puts back the document but not those, so the next Save of the untouched
/// document became a Save As. `Session::run`'s roll-back puts the open file back.
#[test]
fn a_refused_comparison_keeps_the_documents_file_identity() {
    let mut s = Session::new(Document::from_text("One paragraph."));
    run(&mut s, "file.versions", json!({"save": "before"}));
    let path = std::path::PathBuf::from("/synthetic/kept.docx");
    s.path = Some(path.clone());
    s.file_name = Some("kept.docx".into());
    let mut limits = wordcraft_doc::limits::Limits::DOCUMENT;
    limits.paragraphs = 2;
    s.limits = limits;
    let refused = s.run("review.compare", &json!({"text": "One.\nTwo.\nThree.\nFour.", "showIn": "new"}));
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(s.doc.plain_text(StoryRef::Body), "One paragraph.", "the document is back");
    assert_eq!(s.path.as_deref(), Some(path.as_path()), "and so is its file");
    assert_eq!(s.file_name.as_deref(), Some("kept.docx"));
    assert_eq!(s.versions.len(), 1, "and its version snapshots");
}

/// The same for a comparison refused by Restrict Editing's post-check (upstream 5220199b): the
/// guard lets it run, since the document has an editable region, and the check refuses it.
#[test]
fn a_comparison_refused_by_restrict_editing_keeps_the_documents_versions() {
    let mut s = protected_with_one_region("Editable. Locked.");
    s.path = Some(std::path::PathBuf::from("/synthetic/original.docx"));
    s.file_name = Some("original.docx".into());
    run(&mut s, "file.versions", json!({"save": "snapshot"}));
    let before = ser(&s.doc);
    assert!(s.run("review.compare", &json!({"text": "Different document.", "showIn": "new"})).is_err());
    assert_eq!(ser(&s.doc), before);
    assert_eq!(s.versions.len(), 1, "the refused comparison discarded the document's snapshot");
    assert_eq!(s.path.as_deref(), Some(std::path::Path::new("/synthetic/original.docx")));
    assert_eq!(s.file_name.as_deref(), Some("original.docx"));
}

/// Envelopes replace the document through `set_document`, which starts a new document id and
/// drops the password, the file name and the versions: refused, all of them come back with the
/// document (upstream debb125d).
#[test]
fn a_refused_envelope_keeps_the_documents_file_identity() {
    let mut s = protected_with_one_region("Protected source.");
    s.path = Some(std::path::PathBuf::from("/synthetic/source.docx"));
    s.file_name = Some("source.docx".into());
    run(&mut s, "file.versions", json!({"save": "snapshot"}));
    s.password = Some(crate::io::Password::new("synthetic file password"));
    s.bib_style = "Chicago".into();
    let (before, id, dirty) = (ser(&s.doc), s.document_id(), s.dirty);
    assert!(s.run("mailings.envelopes", &json!({"delivery": "Recipient"})).is_err());
    assert_eq!(ser(&s.doc), before);
    assert_eq!(s.versions.len(), 1);
    assert_eq!(s.path.as_deref(), Some(std::path::Path::new("/synthetic/source.docx")));
    assert_eq!(s.file_name.as_deref(), Some("source.docx"));
    assert_eq!(s.document_id(), id);
    assert_eq!(s.dirty, dirty);
    assert_eq!(s.password.as_ref().map(crate::io::Password::as_str), Some("synthetic file password"));
    assert_eq!(s.bib_style, "Chicago");
}

/// A version snapshot belongs to the document it was taken from: a new or opened document
/// starts a new version history, so a restore can't put the previous document's content into
/// it (upstream 23c8dfa9).
#[test]
fn a_version_snapshot_does_not_survive_a_document_switch() {
    let mut s = Session::new(Document::from_text("First document."));
    run(&mut s, "file.versions", json!({"save": "snapshot"}));
    run(&mut s, "file.new", json!({}));
    assert!(s.versions.is_empty(), "the snapshot of the previous document survived File › New");
    assert!(s.run("file.versions", &json!({"restore": 0})).is_err());

    run(&mut s, "file.versions", json!({"save": "snapshot"}));
    run(&mut s, "file.open", json!({"path": "other.txt", "data": "aGk="}));
    assert!(s.versions.is_empty(), "the snapshot of the previous document survived File › Open");
    assert_eq!(s.doc.plain_text(StoryRef::Body), "hi");

    // A snapshot of the open document still restores into it.
    run(&mut s, "file.versions", json!({"save": "snapshot"}));
    run(&mut s, "text.insert", json!({"text": " there"}));
    run(&mut s, "file.versions", json!({"restore": 0}));
    assert_eq!(s.doc.plain_text(StoryRef::Body), "hi");
}

/// A read-only document whose whole text is one region everyone may edit: Restrict Editing's
/// guard lets an edit run and its post-check refuses whatever changed outside the region.
fn protected_with_one_region(text: &str) -> Session {
    use wordcraft_doc::{CharProps, InlineObject};
    let mut s = Session::new(Document::from_text(text));
    let end = s.doc.end_of(StoryRef::Body);
    s.doc.insert_object(&end, InlineObject::PermEnd { id: 1 }, &CharProps::default()).unwrap();
    let start = s.doc.start_of(StoryRef::Body);
    s.doc.insert_object(&start, InlineObject::PermStart { id: 1, group: String::new(), editor: s.author.clone() }, &CharProps::default()).unwrap();
    run(&mut s, "review.restrict", json!({"mode": "readOnly"}));
    s
}

fn table(s: &Session) -> Option<&Table> {
    s.doc.body.iter().find_map(|b| b.as_table())
}

/// Every cell's text, row by row (deleted text included).
fn cells(s: &Session) -> Vec<Vec<String>> {
    table(s)
        .map(|t| {
            t.rows
                .iter()
                .map(|r| r.cells.iter().map(|c| c.blocks.iter().filter_map(|b| b.as_para()).map(|p| p.plain_text()).collect()).collect())
                .collect()
        })
        .unwrap_or_default()
}

fn caret_in(s: &mut Session, r: u32, c: u32, off: usize) {
    s.sel = Selection::caret(Pos { story: StoryRef::Body, path: Path(vec![1, r, c, 0]), off });
}

/// "Before", a 2×2 table holding A1 B1 / A2 B2, an empty paragraph and "After"; history cleared.
fn table_doc() -> Session {
    let mut s = Session::new(Document::new());
    run(&mut s, "text.insert", json!({"text": "Before"}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "text.insert", json!({"text": "After"}));
    run(&mut s, "caret.docStart", json!({}));
    run(&mut s, "caret.end", json!({}));
    run(&mut s, "text.newParagraph", json!({}));
    run(&mut s, "insert.table", json!({"rows": 2, "cols": 2}));
    // The table goes in before the empty paragraph: Before, the table, "", After.
    for (i, t) in ["A1", "B1", "A2", "B2"].iter().enumerate() {
        if i > 0 {
            run(&mut s, "text.tab", json!({}));
        }
        run(&mut s, "text.insert", json!({"text": t}));
    }
    s.reset_history();
    caret_in(&mut s, 0, 0, 0);
    s
}

/// The same edit made with tracking off and on: Accept All gives what tracking off gave, Reject
/// All gives the document back, and undoing it all and redoing it all are exact.
fn tracked_like_untracked(name: &str, start: &dyn Fn() -> Session, edit: &dyn Fn(&mut Session)) {
    let mut plain = start();
    edit(&mut plain);
    let mut s = start();
    let original = ser(&s.doc);
    run(&mut s, "review.trackChanges", json!({"value": true}));
    edit(&mut s);
    let tracked = ser(&s.doc);
    let mut accepted = fork(&s);
    run(&mut accepted, "review.acceptAll", json!({}));
    assert_eq!(body(&accepted.doc), body(&plain.doc), "{name}: Accept All isn't the edit made untracked");
    let mut rejected = fork(&s);
    run(&mut rejected, "review.rejectAll", json!({}));
    assert_eq!(body(&rejected.doc), body(&start().doc), "{name}: Reject All doesn't restore the document");
    let mut steps = 0;
    while s.can_undo() {
        run(&mut s, "edit.undo", json!({}));
        steps += 1;
    }
    s.doc.settings.track_changes = false;
    assert_eq!(ser(&s.doc), original, "{name}: undo");
    s.doc.settings.track_changes = true;
    for _ in 0..steps {
        run(&mut s, "edit.redo", json!({}));
    }
    assert_eq!(ser(&s.doc), tracked, "{name}: redo");
}

#[test]
fn tracked_row_and_column_edits_accept_like_untracked_and_reject_back() {
    let cases: Vec<(&str, Box<dyn Fn(&mut Session)>)> = vec![
        (
            "insert row above",
            Box::new(|s: &mut Session| {
                caret_in(s, 1, 0, 0);
                run(s, "table.insertRowAbove", json!({}));
                run(s, "text.insert", json!({"text": "new"}));
            }),
        ),
        (
            "insert row below",
            Box::new(|s: &mut Session| {
                run(s, "table.insertRowBelow", json!({}));
            }),
        ),
        (
            "tab past the last cell",
            Box::new(|s: &mut Session| {
                caret_in(s, 1, 1, 0);
                run(s, "text.tab", json!({}));
                run(s, "text.insert", json!({"text": "C1"}));
            }),
        ),
        (
            "insert column left",
            Box::new(|s: &mut Session| {
                run(s, "table.insertColumnLeft", json!({}));
            }),
        ),
        (
            "insert column right",
            Box::new(|s: &mut Session| {
                caret_in(s, 0, 1, 0);
                run(s, "table.insertColumnRight", json!({}));
                run(s, "text.insert", json!({"text": "x"}));
            }),
        ),
        (
            "delete row",
            Box::new(|s: &mut Session| {
                run(s, "table.deleteRow", json!({}));
            }),
        ),
        (
            "delete second row",
            Box::new(|s: &mut Session| {
                caret_in(s, 1, 1, 0);
                run(s, "table.deleteRow", json!({}));
            }),
        ),
        (
            "delete column",
            Box::new(|s: &mut Session| {
                run(s, "table.deleteColumn", json!({}));
            }),
        ),
        (
            "delete table",
            Box::new(|s: &mut Session| {
                run(s, "table.deleteTable", json!({}));
            }),
        ),
        (
            "select across the table and delete",
            Box::new(|s: &mut Session| {
                s.sel = Selection { anchor: Pos::body(0, 3), focus: Pos::body(3, 2) };
                run(s, "text.delete", json!({}));
            }),
        ),
        (
            "select all and delete",
            Box::new(|s: &mut Session| {
                run(s, "select.all", json!({}));
                run(s, "text.backspace", json!({}));
            }),
        ),
        (
            "insert a table",
            Box::new(|s: &mut Session| {
                s.sel = Selection::caret(Pos::body(0, 3));
                run(s, "insert.table", json!({"rows": 2, "cols": 3}));
                run(s, "text.insert", json!({"text": "t"}));
            }),
        ),
        (
            "insert a table at a paragraph start",
            Box::new(|s: &mut Session| {
                s.sel = Selection::caret(Pos::body(3, 0));
                run(s, "insert.table", json!({"rows": 1, "cols": 1}));
            }),
        ),
        (
            "insert two rows and delete one of them",
            Box::new(|s: &mut Session| {
                run(s, "table.insertRowBelow", json!({}));
                run(s, "table.insertRowBelow", json!({}));
                run(s, "table.deleteRow", json!({}));
            }),
        ),
    ];
    for (name, edit) in &cases {
        tracked_like_untracked(name, &table_doc, edit.as_ref());
    }
}

#[test]
fn tracked_rows_and_cells_are_revisions_in_the_reviewing_pane() {
    let mut s = table_doc();
    run(&mut s, "review.trackChanges", json!({"value": true}));
    run(&mut s, "table.insertRowBelow", json!({}));
    caret_in(&mut s, 2, 0, 0);
    run(&mut s, "table.deleteRow", json!({}));
    caret_in(&mut s, 0, 1, 0);
    run(&mut s, "table.insertColumnRight", json!({}));
    let t = table(&s).unwrap();
    assert!(
        t.rows[1].props.ins.is_some() && t.rows[2].props.del.is_some(),
        "{:?}",
        t.rows.iter().map(|r| (r.props.ins, r.props.del)).collect::<Vec<_>>()
    );
    assert!(t.rows.iter().all(|r| r.cells[2].props.ins.is_some()));
    let pane = run(&mut s, "review.changes", json!({}));
    let objects: Vec<(String, String)> = pane
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c.get("object").is_some())
        .map(|c| (c["kind"].as_str().unwrap().to_string(), c["object"].as_str().unwrap().to_string()))
        .collect();
    assert!(objects.contains(&("insert".into(), "row".into())), "{objects:?}");
    assert!(objects.contains(&("delete".into(), "row".into())), "{objects:?}");
    assert_eq!(objects.iter().filter(|(k, o)| k == "insert" && o == "cell").count(), 3, "{objects:?}");
    let deleted = pane.as_array().unwrap().iter().find(|c| c["object"] == "row" && c["kind"] == "delete").unwrap();
    assert_eq!(deleted["text"].as_str().unwrap().replace(['\t', '\n'], ""), "A2B2", "{deleted}");
}

/// Accept or Reject with the caret in a tracked row takes that row (and its text); with the caret
/// in an inserted column's cell, the whole column.
#[test]
fn accept_and_reject_at_the_caret_take_one_row_or_column() {
    let mut s = table_doc();
    run(&mut s, "review.trackChanges", json!({"value": true}));
    run(&mut s, "table.insertRowBelow", json!({}));
    run(&mut s, "text.insert", json!({"text": "n"}));
    caret_in(&mut s, 2, 0, 0);
    run(&mut s, "table.deleteRow", json!({}));
    // Reject the deletion at the caret: the row and its text are back, the insertion stays.
    caret_in(&mut s, 2, 1, 0);
    run(&mut s, "review.reject", json!({}));
    let t = table(&s).unwrap();
    assert_eq!(t.rows.len(), 3);
    assert_eq!((t.rows[2].props.ins, t.rows[2].props.del), (None, None));
    assert!(t.rows[2].cells.iter().flat_map(|c| c.blocks.iter()).filter_map(|b| b.as_para()).all(|p| p.runs.iter().all(|r| r.props.del.is_none())));
    assert!(t.rows[1].props.ins.is_some());
    // Reject the inserted row at the caret: it goes.
    caret_in(&mut s, 1, 1, 0);
    run(&mut s, "review.reject", json!({}));
    assert_eq!(cells(&s), [["A1", "B1"], ["A2", "B2"]]);
    // An inserted column, rejected from one of its cells, goes whole and the widths come back.
    let widths = table(&s).unwrap().grid.clone();
    caret_in(&mut s, 0, 0, 0);
    run(&mut s, "table.insertColumnLeft", json!({}));
    caret_in(&mut s, 1, 0, 0);
    run(&mut s, "review.reject", json!({}));
    let t = table(&s).unwrap();
    assert_eq!(cells(&s), [["A1", "B1"], ["A2", "B2"]]);
    assert!(t.grid.iter().zip(&widths).all(|(a, b)| (a - b).abs() < 0.01), "{:?} vs {widths:?}", t.grid);
    // Accepting a deleted row from a selection over it removes it.
    caret_in(&mut s, 0, 0, 0);
    run(&mut s, "table.deleteRow", json!({}));
    s.sel = Selection {
        anchor: Pos { story: StoryRef::Body, path: Path(vec![1, 0, 0, 0]), off: 0 },
        focus: Pos { story: StoryRef::Body, path: Path(vec![1, 0, 1, 0]), off: 1 },
    };
    run(&mut s, "review.accept", json!({}));
    assert_eq!(cells(&s), [["A2", "B2"]]);
}

#[test]
fn accept_all_and_reject_all_can_take_one_reviewers_changes() {
    let mut s = table_doc();
    run(&mut s, "review.trackChanges", json!({"value": true}));
    s.author = "Ana".into();
    run(&mut s, "table.insertRowBelow", json!({}));
    s.author = "Ben".into();
    caret_in(&mut s, 2, 0, 0);
    run(&mut s, "table.deleteRow", json!({}));
    s.sel = Selection::caret(Pos::body(0, 6));
    run(&mut s, "text.insert", json!({"text": "!"}));
    // Ben's changes rejected: Ana's inserted row stays tracked.
    let mut only_ben = fork(&s);
    run(&mut only_ben, "review.rejectAll", json!({"author": "Ben"}));
    assert_eq!(only_ben.doc.para_at(&Pos::body(0, 0)).unwrap().text, "Before");
    let t = table(&only_ben).unwrap();
    assert_eq!(t.rows.len(), 3);
    assert!(t.rows[1].props.ins.is_some() && t.rows[2].props.del.is_none());
    // Then Ana's accepted: no revision left in the text.
    run(&mut only_ben, "review.acceptAll", json!({"author": ["Ana"]}));
    assert!(table(&only_ben).unwrap().rows.iter().all(|r| r.props.ins.is_none() && r.props.del.is_none()));
    assert_eq!(run(&mut only_ben, "review.changes", json!({})).as_array().map(Vec::len), Some(0));
    // Nobody's changes for a name no one has; bad params are refused.
    let before = ser(&s.doc);
    run(&mut s, "review.acceptAll", json!({"author": "Nobody"}));
    assert_eq!(body(&s.doc), serde_json::to_string(&serde_json::from_str::<Document>(&before).unwrap().body).unwrap());
    assert!(s.run("review.acceptAll", &json!({"author": 3})).is_err());
    assert!(s.run("review.rejectAll", &json!({"author": ["Ana", 3]})).is_err());
}

/// The comment anchors in the body: (id, start?) in document order.
fn anchors(s: &Session) -> Vec<(u32, bool)> {
    let mut out = Vec::new();
    for path in s.doc.para_paths(StoryRef::Body) {
        let Some(p) = s.doc.para(StoryRef::Body, &path) else { continue };
        for o in &p.objects {
            match o {
                wordcraft_doc::InlineObject::CommentStart { id } => out.push((*id, true)),
                wordcraft_doc::InlineObject::CommentEnd { id } => out.push((*id, false)),
                _ => {}
            }
        }
    }
    out
}

/// "one two three four" with a comment (and a reply) on "two three".
fn commented() -> (Session, u32) {
    let mut s = Session::new(Document::from_text("one two three four"));
    s.sel = Selection { anchor: Pos::body(0, 4), focus: Pos::body(0, 13) };
    let id = run(&mut s, "review.newComment", json!({"text": "look"}))["id"].as_u64().unwrap() as u32;
    run(&mut s, "review.reply", json!({"id": id, "text": "agreed"}));
    s.reset_history();
    (s, id)
}

/// #1847 (upstream 2d4455ff): a comment id past `u32::MAX` was cut to 32 bits, so 2^32 + id
/// resolved, answered or deleted comment `id`. It is refused, and so is an `id` that isn't a
/// number (it fell back to the comment at the caret); the comments stay as they were.
#[test]
fn comment_ids_past_32_bits_name_no_comment() {
    let (mut s, id) = commented();
    s.sel = Selection::caret(Pos::body(0, 6));
    let before = ser(&s.doc);
    let wrapped = (1u64 << 32) + u64::from(id);
    for (cmd, params) in [
        ("review.deleteComment", json!({"id": wrapped})),
        ("review.resolveComment", json!({"id": wrapped})),
        ("review.reply", json!({"id": wrapped, "text": "wrong thread"})),
        ("review.deleteComment", json!({"id": "two"})),
        ("review.resolveComment", json!({"id": -1})),
    ] {
        let r = s.run(cmd, &params);
        assert!(r.is_err(), "{cmd} {params}: {r:?}");
        assert_eq!(ser(&s.doc), before, "{cmd} {params} changed nothing");
    }
    // The id itself still works, and the caret's comment without one.
    run(&mut s, "review.resolveComment", json!({"id": id}));
    assert!(s.doc.comments.get(&id).is_some_and(|c| c.resolved));
    run(&mut s, "review.deleteComment", json!({}));
    assert!(s.doc.comments.is_empty());
}

/// The text a comment's range covers (anchors left out).
fn commented_text(s: &Session, id: u32) -> Option<String> {
    let w = wordcraft_doc::para::OBJ;
    let p = s.doc.para_at(&Pos::body(0, 0))?;
    let at = |want: bool| {
        p.object_offsets().into_iter().find(|o| {
            matches!(p.object_at(*o), Some(wordcraft_doc::InlineObject::CommentStart { id: x }) if *x == id && want)
                || matches!(p.object_at(*o), Some(wordcraft_doc::InlineObject::CommentEnd { id: x }) if *x == id && !want)
        })
    };
    let (a, b) = (at(true)?, at(false)?);
    Some(p.text.get(a..b)?.replace(w, ""))
}

/// Deleting text that holds a comment's whole range deletes the comment and its replies, as in
/// Word; deleting text over one end of the range keeps the comment on what is left, never leaving
/// an end behind. Undo gives everything back exactly, Redo redoes it.
#[test]
fn deleting_commented_text_leaves_no_dangling_anchor() {
    let w = wordcraft_doc::para::OBJ.len_utf8();
    // Whole range ("one [two three] four" → "one  four").
    let (mut s, id) = commented();
    let before = ser(&s.doc);
    s.sel = Selection { anchor: Pos::body(0, 3), focus: Pos::body(0, 14 + 2 * w) };
    run(&mut s, "text.delete", json!({}));
    assert_eq!(anchors(&s), [], "{:?}", s.doc.plain_text(StoryRef::Body));
    assert!(s.doc.comments.is_empty(), "the comment and its reply go");
    assert!(!s.doc.parts.values().any(|p| p.kind == wordcraft_doc::PartKind::Comment), "their text too");
    let after = ser(&s.doc);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(ser(&s.doc), before);
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(ser(&s.doc), after);
    // From before the range into it: the start stays, on "three".
    let (mut s, id2) = commented();
    s.sel = Selection { anchor: Pos::body(0, 2), focus: Pos::body(0, 8 + w) };
    run(&mut s, "text.delete", json!({}));
    assert_eq!(anchors(&s), [(id2, true), (id2, false)]);
    assert_eq!(commented_text(&s, id2).as_deref(), Some("three"));
    assert_eq!(s.doc.comments.len(), 2);
    // From inside the range past its end: the end stays, after "two".
    let (mut s, _) = commented();
    s.sel = Selection { anchor: Pos::body(0, 7 + w), focus: Pos::body(0, 18 + 2 * w) };
    run(&mut s, "text.backspace", json!({}));
    assert_eq!(commented_text(&s, id).as_deref(), Some("two"));
    // Select All, Delete: the comment goes.
    let (mut s, _) = commented();
    run(&mut s, "select.all", json!({}));
    run(&mut s, "text.delete", json!({}));
    assert!(anchors(&s).is_empty() && s.doc.comments.is_empty());
}

/// With Track Changes on, deleted commented text stays (marked deleted) with its comment;
/// accepting the deletion deletes the comment, rejecting it keeps it.
#[test]
fn accepting_a_deletion_of_commented_text_deletes_the_comment() {
    let w = wordcraft_doc::para::OBJ.len_utf8();
    let (mut s, id) = commented();
    run(&mut s, "review.trackChanges", json!({"value": true}));
    s.sel = Selection { anchor: Pos::body(0, 3), focus: Pos::body(0, 14 + 2 * w) };
    run(&mut s, "text.delete", json!({}));
    assert_eq!(anchors(&s), [(id, true), (id, false)]);
    let mut rejected = fork(&s);
    run(&mut rejected, "review.rejectAll", json!({}));
    assert_eq!(rejected.doc.comments.len(), 2);
    assert_eq!(commented_text(&rejected, id).as_deref(), Some("two three"));
    run(&mut s, "review.acceptAll", json!({}));
    assert!(anchors(&s).is_empty() && s.doc.comments.is_empty());
    assert_eq!(s.doc.plain_text(StoryRef::Body), "onefour");
}

/// Cut and Paste moves a comment with its text; Copy and Paste makes a second comment (with its
/// replies) on the copy, so no id is anchored twice.
#[test]
fn cut_moves_a_comment_and_copy_makes_another() {
    let w = wordcraft_doc::para::OBJ.len_utf8();
    let (mut s, id) = commented();
    s.sel = Selection { anchor: Pos::body(0, 4), focus: Pos::body(0, 13 + 2 * w) };
    run(&mut s, "edit.cut", json!({}));
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "edit.paste", json!({}));
    assert_eq!(anchors(&s), [(id, true), (id, false)]);
    assert_eq!(commented_text(&s, id).as_deref(), Some("two three"));
    assert_eq!(s.doc.comments.len(), 2);
    let (mut s, id) = commented();
    s.sel = Selection { anchor: Pos::body(0, 4), focus: Pos::body(0, 13 + 2 * w) };
    run(&mut s, "edit.copy", json!({}));
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "edit.paste", json!({}));
    let a = anchors(&s);
    assert_eq!(a.len(), 4);
    let copy = a.iter().map(|(x, _)| *x).find(|x| *x != id).unwrap();
    assert_eq!(a, [(id, true), (id, false), (copy, true), (copy, false)]);
    assert_eq!(s.doc.comments.len(), 4, "the comment and its reply, twice");
    assert_eq!(s.doc.plain_text(StoryRef::Part(s.doc.comments[&copy].part)), "look");
    assert!(s.doc.comments.values().any(|c| c.parent == Some(copy)));
    // Pasted into another document, the anchors are left out.
    let frag = s.clipboard.clone();
    let mut other = Session::new(Document::from_text("x"));
    other.clipboard = frag;
    run(&mut other, "caret.docEnd", json!({}));
    run(&mut other, "edit.paste", json!({}));
    assert!(anchors(&other).is_empty());
}

/// Every comment command undoes and redoes exactly.
#[test]
fn comment_commands_undo_and_redo_exactly() {
    let (mut s, id) = commented();
    for (cmd, v) in [
        ("review.newComment", json!({"text": "new"})),
        ("review.reply", json!({"id": id, "text": "re"})),
        ("review.editComment", json!({"id": id, "text": "changed\nlines"})),
        ("review.resolveComment", json!({"id": id})),
        ("review.resolveComment", json!({"id": id, "value": false})),
        ("review.deleteComment", json!({"id": id})),
        ("review.deleteComment", json!({"all": true})),
    ] {
        s.sel = Selection::caret(Pos::body(0, 1));
        let before = ser(&s.doc);
        run(&mut s, cmd, v.clone());
        let after = ser(&s.doc);
        assert_ne!(before, after, "{cmd} {v} changed nothing");
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(ser(&s.doc), before, "{cmd} {v}: undo");
        run(&mut s, "edit.redo", json!({}));
        assert_eq!(ser(&s.doc), after, "{cmd} {v}: redo");
    }
}

/// A comment thread survives a .docx save: the reply, the resolved state, authors, initials.
#[test]
fn comment_threads_survive_a_docx_round_trip() {
    let (mut s, id) = commented();
    s.author = "Ana Ruiz".into();
    let reply = run(&mut s, "review.reply", json!({"id": id, "text": "second"}))["id"].as_u64().unwrap() as u32;
    run(&mut s, "review.resolveComment", json!({"id": id, "value": true}));
    let back = wordcraft_docx::read(&wordcraft_docx::write(&s.doc).unwrap()).unwrap();
    let mut thread: Vec<(String, String, bool, bool, String)> = back
        .comments
        .values()
        .map(|c| (c.author.clone(), c.initials.clone(), c.parent.is_some(), c.resolved, back.plain_text(StoryRef::Part(c.part))))
        .collect();
    thread.sort_by(|a, b| a.4.cmp(&b.4));
    let me = &s.doc.comments[&id];
    let mine = &s.doc.comments[&reply];
    assert_eq!(
        thread,
        [
            (me.author.clone(), me.initials.clone(), true, false, "agreed".to_string()),
            (me.author.clone(), me.initials.clone(), false, true, "look".to_string()),
            ("Ana Ruiz".to_string(), mine.initials.clone(), true, false, "second".to_string()),
        ]
    );
    // The replies hang on the comment they answer.
    let root = back.comments.iter().find(|(_, c)| c.parent.is_none()).map(|(k, _)| *k).unwrap();
    assert!(back.comments.values().filter(|c| c.parent.is_some()).all(|c| c.parent == Some(root)));
}

/// Each paragraph's text, style, list, direction, border and mark formatting: what Reject All
/// must give back.
fn shape(d: &Document) -> Vec<String> {
    d.body
        .iter()
        .map(|b| match b.as_para() {
            Some(p) => format!(
                "{:?} {:?} {:?} {:?} {:?} {:?}",
                p.text,
                p.props.style,
                p.props.numbering.map(|n| n.num),
                p.props.bidi,
                p.props.borders.is_some(),
                p.mark.formatting()
            ),
            None => "table".into(),
        })
        .collect()
}

/// Edits made with Track Changes on that the randomized tests found Reject All couldn't take
/// back, each made and rejected: the document is as it was.
#[test]
fn reject_all_takes_back_what_these_tracked_edits_changed() {
    type Step = Box<dyn Fn(&mut Session)>;
    let none: fn() -> Step = || Box::new(|_| {});
    // (name, text, set-up made untracked, edit made tracked)
    let cases: Vec<(&str, &str, Step, Step)> = vec![
        // Text typed into an empty paragraph and rejected: the mark keeps its own formatting.
        (
            "typed bigger into an empty paragraph",
            "\nafter",
            none(),
            Box::new(|s| {
                run(s, "format.size", json!({"size": 30}));
                run(s, "text.insert", json!({"text": "abc", "raw": true}));
            }),
        ),
        // Your own typing deleted again: no trace in the mark either.
        (
            "typed and deleted",
            "\nafter",
            none(),
            Box::new(|s| {
                run(s, "format.size", json!({"size": 30}));
                run(s, "text.insert", json!({"text": "abc", "raw": true}));
                s.sel = Selection { anchor: Pos::body(0, 0), focus: Pos::body(0, 3) };
                run(s, "text.delete", json!({}));
            }),
        ),
        // Right-to-left letters typed at a paragraph start set its direction: tracked.
        (
            "Arabic typed into an empty paragraph",
            "\nafter",
            none(),
            Box::new(|s| {
                run(s, "text.insert", json!({"text": "\u{0627}"}));
            }),
        ),
        // AutoFormat lists and border lines change the paragraph: tracked.
        (
            "\"- \" starts a bulleted list",
            "\nafter",
            none(),
            Box::new(|s| {
                for c in ["-", " ", "x"] {
                    run(s, "text.insert", json!({"text": c}));
                }
            }),
        ),
        (
            "--- then Enter draws a border",
            "\nafter",
            none(),
            Box::new(|s| {
                for c in ["-", "-", "-"] {
                    run(s, "text.insert", json!({"text": c}));
                }
                run(s, "text.newParagraph", json!({}));
            }),
        ),
        (
            "+--+--+ then Enter makes a table",
            "\nafter",
            none(),
            Box::new(|s| {
                run(s, "text.insert", json!({"text": "+--+--+", "raw": true}));
                run(s, "text.newParagraph", json!({}));
            }),
        ),
        // Paragraphs pasted at the end of one give it the first one's formatting: tracked; and
        // the source's own tracked formatting change doesn't come with them.
        (
            "two paragraphs pasted into one",
            "plain\nafter\nHead\ntail",
            Box::new(|s| {
                s.sel = Selection::caret(Pos::body(2, 0));
                run(s, "para.style", json!({"style": "Heading1"}));
            }),
            Box::new(|s| {
                s.sel = Selection::caret(Pos::body(2, 0));
                run(s, "para.style", json!({"style": "Title"}));
                s.sel = Selection { anchor: Pos::body(2, 0), focus: Pos::body(3, 4) };
                run(s, "edit.copy", json!({}));
                s.sel = Selection::caret(Pos::body(0, 5));
                run(s, "edit.paste", json!({}));
            }),
        ),
        // Enter at the start of a heading, twice, then Backspace over your own empty paragraph
        // (made in Normal, the heading's next style): the heading keeps its style.
        (
            "Backspace over your own empty paragraph",
            "Heading text\nafter",
            Box::new(|s| {
                run(s, "para.style", json!({"style": "Heading1"}));
            }),
            Box::new(|s| {
                s.sel = Selection::caret(Pos::body(0, 0));
                run(s, "text.newParagraph", json!({}));
                s.sel = Selection::caret(Pos::body(0, 0));
                run(s, "text.newParagraph", json!({}));
                s.sel = Selection::caret(Pos::body(2, 0));
                run(s, "text.backspace", json!({}));
                let style = s.doc.body.iter().filter_map(|b| b.as_para()).find(|p| p.text == "Heading text").and_then(|p| p.props.style.clone());
                assert_eq!(style.as_deref(), Some("Heading1"), "the heading's own paragraph");
            }),
        ),
    ];
    for (name, text, setup, edit) in cases {
        let mut s = Session::new(Document::from_text(text));
        s.sel = Selection::caret(Pos::body(0, 0));
        setup(&mut s);
        let before = shape(&s.doc);
        run(&mut s, "review.trackChanges", json!({"value": true}));
        s.sel = Selection::caret(Pos::body(0, 0));
        edit(&mut s);
        assert_ne!(shape(&s.doc), before, "{name}: the edit changed nothing");
        run(&mut s, "review.rejectAll", json!({}));
        assert_eq!(shape(&s.doc), before, "{name}");
    }
}

/// Paragraph texts and tables ("T") of the body, with each paragraph's list (num) and objects.
fn outline(d: &Document) -> Vec<(String, Option<u32>, usize)> {
    d.body
        .iter()
        .map(|b| match b.as_para() {
            Some(p) => (p.plain_text(), p.props.numbering.map(|n| n.num), p.objects.len()),
            None => ("T".into(), None, 0),
        })
        .collect()
}

/// #1317: Compare Documents tracks whole paragraphs and tables (their marks and rows too), brings
/// the revised document's lists over under this document's ids, and doesn't flatten a changed
/// paragraph's field or formatting: Reject All gives the original back, Accept All the revised.
#[test]
fn compare_lets_accept_and_reject_give_either_document() {
    let typed = |lines: &[&str]| {
        let mut s = Session::new(Document::new());
        s.prefs.autocorrect.enabled = false;
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                run(&mut s, "text.newParagraph", json!({}));
            }
            run(&mut s, "text.insert", json!({"text": l, "raw": true}));
        }
        s
    };
    // The original: a bulleted item, a field in "Page ", and plain text.
    let mut s = typed(&["Alpha beta", "Item", "Page ", "End"]);
    s.sel = Selection::caret(Pos::body(1, 0));
    run(&mut s, "para.bullets", json!({}));
    s.sel = Selection::caret(Pos::body(2, 5));
    run(&mut s, "insert.field", json!({"instr": "PAGE", "result": "1"}));
    s.reset_history();
    let original = outline(&s.doc);
    // The revised: a word changed and made bold, a new numbered item (list 1 there is numbered),
    // a table, and the field's paragraph changed.
    let mut r = typed(&["Alpha gamma", "Counted", "Page here ", "End"]);
    r.sel = Selection { anchor: Pos::body(0, 6), focus: Pos::body(0, 11) };
    run(&mut r, "format.bold", json!({}));
    r.sel = Selection::caret(Pos::body(1, 0));
    run(&mut r, "para.numbering", json!({}));
    r.sel = Selection::caret(Pos::body(2, 10));
    run(&mut r, "insert.field", json!({"instr": "PAGE", "result": "1"}));
    r.sel = Selection::caret(Pos::body(3, 0));
    run(&mut r, "insert.table", json!({"rows": 1, "cols": 2}));
    run(&mut r, "text.insert", json!({"text": "cell", "raw": true}));
    let revised = outline(&r.doc);
    let path = std::env::temp_dir().join(format!("wc-compare-1317-{}.docx", std::process::id()));
    crate::io::save_path(&path, &r.doc).unwrap();
    let res = s.run("review.compare", &json!({"path": path.to_string_lossy()}));
    let _ = std::fs::remove_file(&path);
    res.unwrap();
    let mut rejected = fork(&s);
    run(&mut rejected, "review.rejectAll", json!({}));
    assert_eq!(outline(&rejected.doc), original, "Reject All");
    run(&mut s, "review.acceptAll", json!({}));
    let accepted = outline(&s.doc);
    let texts = |o: &[(String, Option<u32>, usize)]| o.iter().map(|x| (x.0.clone(), x.2)).collect::<Vec<_>>();
    assert_eq!(texts(&accepted), texts(&revised), "Accept All");
    // The numbered item is in a list of its own here, not this document's bulleted list.
    let bullets = original[1].1;
    let numbered = accepted.iter().find(|x| x.0 == "Counted").and_then(|x| x.1);
    assert!(numbered.is_some() && numbered != bullets, "{numbered:?} vs {bullets:?}");
    assert!(s.doc.numbering.nums.iter().any(|n| Some(n.id) == numbered));
    // "gamma" keeps its bold.
    let p = s.doc.para_at(&Pos::body(0, 0)).unwrap();
    assert_eq!(p.props_of_char(p.text.find("gamma").unwrap()).bold, Some(true));
}

/// #1880: a document protected for tracked changes keeps every change tracked. Accept and Reject
/// (one, all, by reviewer), turning Track Changes off and the Document Inspector removing content
/// are refused and change nothing; editing (tracked) and comments still work, and after Stop
/// Protection the changes can be accepted.
#[test]
fn tracked_changes_protection_refuses_accepting_rejecting_and_untracking() {
    let mut s = Session::new(Document::from_text("Hello world"));
    run(&mut s, "review.restrict", json!({"mode": "trackedChanges"}));
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "text.insert", json!({"text": "!", "raw": true}));
    assert!(s.doc.para_at(&Pos::body(0, 0)).unwrap().runs.iter().any(|r| r.props.ins.is_some()), "the edit is tracked");
    let author = s.author.clone();
    let before = ser(&s.doc);
    for (id, v) in [
        ("review.acceptAll", json!({})),
        ("review.rejectAll", json!({})),
        ("review.acceptAll", json!({"author": author})),
        ("review.accept", json!({})),
        ("review.reject", json!({})),
        ("review.trackChanges", json!({})),
        ("review.trackChanges", json!({"value": false})),
        ("file.inspect", json!({"remove": ["revisions"]})),
        ("file.inspect", json!({"remove": ["hidden", "comments"]})),
    ] {
        assert!(s.run(id, &v).is_err(), "{id} {v}");
        assert_eq!(ser(&s.doc), before, "{id} {v} changed the document");
    }
    assert!(s.doc.settings.track_changes);
    // Still allowed: inspecting, comments and tracked edits. (Track Changes is greyed out, as
    // in Word: on it stays.)
    assert!(s.run("review.trackChanges", &json!({"value": true})).is_err());
    run(&mut s, "file.inspect", json!({}));
    run(&mut s, "review.newComment", json!({"text": "fine"}));
    run(&mut s, "text.insert", json!({"text": "?", "raw": true}));
    run(&mut s, "review.restrict", json!({"stop": true}));
    run(&mut s, "review.acceptAll", json!({}));
    let text = s.doc.final_text(StoryRef::Body).replace(wordcraft_doc::para::OBJ, "");
    assert!(text.starts_with("Hello world!") && text.contains('?'), "{text:?}");
    assert!(s.doc.revisions.is_empty() && s.doc.comments.len() == 1);
}

/// The Document Inspector removing hidden text, comments, revisions or headers edits the
/// document: in a read-only document it is refused (it used to delete protected hidden text),
/// while inspecting and removing the document's properties still work.
#[test]
fn document_inspector_cannot_remove_content_from_a_read_only_document() {
    let mut s = Session::new(Document::from_text("keep secret"));
    s.sel = Selection { anchor: Pos::body(0, 5), focus: Pos::body(0, 11) };
    run(&mut s, "format.hidden", json!({"value": true}));
    run(&mut s, "review.newComment", json!({"text": "note"}));
    run(&mut s, "review.restrict", json!({"mode": "readOnly"}));
    let before = ser(&s.doc);
    for remove in ["hidden", "comments", "revisions", "headers"] {
        assert!(s.run("file.inspect", &json!({"remove": [remove]})).is_err(), "{remove}");
        assert_eq!(ser(&s.doc), before, "{remove}");
    }
    let found = run(&mut s, "file.inspect", json!({}));
    assert_eq!(found["found"]["hiddenText"], true);
    run(&mut s, "file.inspect", json!({"remove": ["properties"]}));
    assert_eq!(s.doc.plain_text(StoryRef::Body), "keep secret");
}

/// #1998: tracked row insertions and deletions survive a .docx save, so Reject All after
/// reopening takes out an inserted table instead of leaving it empty.
#[test]
fn tracked_rows_survive_a_docx_round_trip() {
    let mut s = table_doc();
    run(&mut s, "review.trackChanges", json!({"value": true}));
    s.sel = Selection::caret(Pos::body(3, 5));
    run(&mut s, "insert.table", json!({"rows": 2, "cols": 2}));
    run(&mut s, "text.insert", json!({"text": "new"}));
    caret_in(&mut s, 1, 0, 0);
    run(&mut s, "table.deleteRow", json!({}));
    let bytes = wordcraft_docx::write(&s.doc).unwrap();
    let mut back = Session::new(wordcraft_docx::read(&bytes).unwrap());
    let mut accepted = fork(&back);
    run(&mut back, "review.rejectAll", json!({}));
    assert_eq!(back.doc.body.iter().filter(|b| b.as_table().is_some()).count(), 1, "the inserted table is gone");
    assert_eq!(cells(&back), [["A1", "B1"], ["A2", "B2"]]);
    run(&mut accepted, "review.acceptAll", json!({}));
    assert_eq!(cells(&accepted), [["A1", "B1"]]);
    assert_eq!(accepted.doc.body.iter().filter(|b| b.as_table().is_some()).count(), 2);
}

/// #3632 (upstream d5f23907): `MAX_LCS` bounds one comparison table, but every changed
/// paragraph pair filled one, so a document of many long, slightly changed paragraphs took
/// minutes. The work over the whole compare is bounded (50M cells): the pairs past the budget are
/// compared whole (deleted and inserted), every pair still counts as a change, and accepting
/// everything still gives the revised text.
#[test]
fn compare_bounds_its_total_work() {
    let (words, paras) = (3000usize, 12usize);
    let text = |revised: bool| {
        (0..paras)
            .map(|p| (0..words).map(|w| if revised && w % 10 == 0 { format!("x{p}w{w}") } else { format!("p{p}w{w}") }).collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let (original, revised) = (text(false), text(true));
    let mut s = Session::new(Document::from_text(&original));
    let r = run(&mut s, "review.compare", json!({"text": revised}));
    assert_eq!(r["changes"], paras);
    // A pair compared word by word keeps most of its text unmarked; one replaced whole keeps none.
    let word_by_word = |b: &std::sync::Arc<wordcraft_doc::Block>| {
        let Some(p) = b.as_para() else { return false };
        let same: usize = p.run_ranges().filter(|(_, c)| c.ins.is_none() && c.del.is_none()).map(|(r, _)| r.len()).sum();
        same * 2 > p.text.len()
    };
    assert!(s.doc.body.first().is_some_and(word_by_word), "the first pairs fit the budget");
    let cells = (words * words) as u64;
    let diffed = s.doc.body.iter().filter(|b| word_by_word(b)).count() as u64;
    assert!(diffed * cells <= 50_000_000 + cells, "{diffed} pairs compared word by word");
    assert!(!s.doc.body.last().is_some_and(word_by_word), "the last pair is past the budget");
    run(&mut s, "review.acceptAll", json!({}));
    assert_eq!(s.doc.plain_text(StoryRef::Body), revised);
}
