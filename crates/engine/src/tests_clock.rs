//! Dates a session stamps on what people do (comments and replies, tracked insertions,
//! deletions, moves and formatting changes, versions) come from `Session::clock`, so a fixed
//! clock fixes them and a reproducible run reads no wall clock for them. The
//! real clock is the default: nothing changes for a person.

use serde_json::{Value, json};
use wordcraft_doc::{Document, Pos, RevisionKind};

use crate::clock::Clock;
use crate::{Selection, Session};

/// 2023-11-14T22:13:20Z.
const NOW: i64 = 1_700_000_000;
const STAMP: &str = "2023-11-14T22:13:20Z";

fn run(s: &mut Session, id: &str, v: Value) -> Value {
    s.run(id, &v).unwrap_or_else(|e| panic!("{id} {v}: {e}"))
}

/// A session on `clock` after a comment and a reply, a tracked insertion, deletion and
/// formatting change, a tracked cut of a sentence (a move in waiting) and a saved version.
fn stamped_session(clock: Clock) -> Session {
    let mut s = Session::new(Document::from_text("Alpha beta gamma delta.\nSecond sentence here. Third one too."));
    s.clock = clock;
    s.prefs.autocorrect.enabled = false;
    run(&mut s, "select.text", json!({"text": "beta"}));
    let id = run(&mut s, "review.newComment", json!({"text": "A comment"}))["id"].clone();
    run(&mut s, "review.reply", json!({"id": id, "text": "A reply"}));
    run(&mut s, "review.trackChanges", json!({"value": true}));
    s.sel = Selection::caret(Pos::body(0, 0));
    run(&mut s, "text.insert", json!({"text": "Inserted "}));
    run(&mut s, "select.text", json!({"text": "gamma"}));
    run(&mut s, "text.delete", json!({}));
    run(&mut s, "select.text", json!({"text": "delta"}));
    run(&mut s, "format.bold", json!({}));
    run(&mut s, "select.text", json!({"text": "Second sentence here."}));
    run(&mut s, "edit.cut", json!({}));
    run(&mut s, "file.versions", json!({"save": "v1"}));
    s
}

/// Every date `s` stamped: the comments', the revisions' and the versions'.
fn stamps(s: &Session) -> Vec<String> {
    s.doc
        .comments
        .values()
        .map(|c| c.date.clone())
        .chain(s.doc.revisions.iter().map(|r| r.date.clone()))
        .chain(s.versions.iter().map(|v| v.1.clone()))
        .collect()
}

#[test]
fn comments_tracked_changes_and_versions_are_stamped_from_the_sessions_clock() {
    let s = stamped_session(Clock::fixed(NOW, wordcraft_tz::Zone::UTC));
    let kinds: Vec<RevisionKind> = s.doc.revisions.iter().map(|r| r.kind).collect();
    for kind in [RevisionKind::Insert, RevisionKind::Delete, RevisionKind::Format] {
        assert!(kinds.contains(&kind), "a {kind:?} revision: {kinds:?}");
    }
    assert_eq!(s.doc.comments.len(), 2, "the comment and its reply");
    assert_eq!(s.versions.len(), 1);
    let stamps = stamps(&s);
    assert!(stamps.len() >= 6, "{stamps:?}");
    assert!(stamps.iter().all(|d| d == STAMP), "every stamp is the clock's: {stamps:?}");
}

/// With the clock a session has by default, the stamps are the wall clock's, in the form files
/// record (`2026-10-19T21:59:44Z`).
#[test]
fn the_real_clock_stamps_by_default() {
    let before = crate::cmd::now_iso();
    let s = stamped_session(Clock::default());
    let after = crate::cmd::now_iso();
    let stamps = stamps(&s);
    assert!(stamps.len() >= 6, "{stamps:?}");
    for d in &stamps {
        assert!(d.len() == 20 && d.ends_with('Z') && *d >= before && *d <= after, "{d} is between {before} and {after}");
    }
    assert!(stamps.iter().all(|d| d != STAMP));
}
