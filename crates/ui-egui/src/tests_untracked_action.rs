//! Merge Cells and Split Cells with Track Changes on ask first, as Word does: neither is
//! recorded as a change (`docs/word-behaviour-research.md` item 3).
#![allow(clippy::unwrap_used)]
use super::*;
use wordcraft_doc::Document;

fn app_with_table() -> WordApp {
    let mut a = WordApp::new(Session::new(Document::new()), Services::default());
    a.execute("insert.table", json!({"rows": 2, "cols": 2})).unwrap();
    a
}

fn cells_in_first_row(a: &WordApp) -> usize {
    a.session.doc.body.iter().find_map(|b| b.as_table()).map_or(0, |t| t.rows[0].cells.len())
}

#[test]
fn merge_and_split_with_tracking_on_ask_first_and_stay_untracked() {
    let mut a = app_with_table();
    // Tracking off: no prompt.
    assert_eq!(a.run("table.split", json!({"columns": 2})).unwrap().get("pending"), None);
    assert_eq!(cells_in_first_row(&a), 3);
    a.execute("review.trackChanges", json!({"value": true})).unwrap();
    // Tracking on: the prompt, then Cancel leaves the table alone.
    let r = a.run("table.split", json!({"columns": 2})).unwrap();
    assert_eq!(r["pending"], "untrackedAction");
    assert!(matches!(a.dialog, Some(dialogs::Dialog::UntrackedAction { ref then, .. }) if then == "table.split"));
    assert!(a.run("ui.untrackedAction", json!({"answer": "maybe"})).is_err());
    let r = a.run("ui.untrackedAction", json!({"answer": "cancel"})).unwrap();
    assert_eq!(r["done"], false);
    assert!(a.dialog.is_none());
    assert_eq!(cells_in_first_row(&a), 3);
    // OK runs it, untracked: no revision is recorded, and the engine says so.
    assert_eq!(a.run("table.split", json!({"columns": 2})).unwrap()["pending"], "untrackedAction");
    let r = a.run("ui.untrackedAction", json!({"answer": "ok"})).unwrap();
    assert_eq!(r["done"], true);
    assert_eq!(r["result"]["untracked"], true);
    assert_eq!(cells_in_first_row(&a), 4);
    assert!(a.session.doc.revisions.is_empty(), "not recorded as a change");
    assert!(a.dialog.is_none());
    // Merge Cells asks too; a script that knows passes `confirmed`.
    a.execute("select.all", json!({})).unwrap();
    a.session.sel = {
        let t = |r: u32, c: u32| wordcraft_doc::Pos { story: wordcraft_doc::StoryRef::Body, path: wordcraft_doc::Path(vec![0, r, c, 0]), off: 0 };
        wordcraft_engine::Selection { anchor: t(0, 0), focus: t(0, 1) }
    };
    assert_eq!(a.run("table.merge", json!({})).unwrap()["pending"], "untrackedAction");
    a.run("ui.untrackedAction", json!({"answer": "cancel"})).unwrap();
    assert_eq!(cells_in_first_row(&a), 4);
    let r = a.run("table.merge", json!({"confirmed": true})).unwrap();
    assert_eq!(r["untracked"], true);
    assert_eq!(cells_in_first_row(&a), 3);
    assert!(a.session.doc.revisions.is_empty());
    // No prompt is open: the answer is refused.
    assert!(a.run("ui.untrackedAction", json!({"answer": "ok"})).is_err());
}

/// With Track Changes on, Split Cells from the ribbon opens its dialog first (#2592); the
/// dialog's settings then wait for the untracked-change prompt and survive it.
#[test]
fn split_cells_dialog_settings_go_through_the_untracked_prompt() {
    let mut a = app_with_table();
    a.execute("review.trackChanges", json!({"value": true})).unwrap();
    assert_eq!(a.run("table.split", json!({})).unwrap()["pending"], "splitCells");
    a.dialog = None;
    let r = a.run("table.split", json!({"columns": 3, "rows": 2, "mergeFirst": false})).unwrap();
    assert_eq!(r["pending"], "untrackedAction");
    assert_eq!(cells_in_first_row(&a), 2, "nothing split before OK");
    let r = a.run("ui.untrackedAction", json!({"answer": "ok"})).unwrap();
    assert_eq!(r["result"]["untracked"], true);
    let rows: Vec<usize> = a.session.doc.body.iter().find_map(|b| b.as_table()).unwrap().rows.iter().map(|r| r.cells.len()).collect();
    assert_eq!(rows, [4, 4, 2], "split into three columns and two rows");
    assert!(a.session.doc.revisions.is_empty());
}
