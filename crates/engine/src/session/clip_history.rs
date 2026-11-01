//! The Clipboard pane's collection (Home › Clipboard): what Copy and Cut gathered this session,
//! kept on the session; the `edit.clipboard*` commands in `cmd::edit` read and paste from it.

use serde_json::{Value, json};
use wordcraft_doc::edit::Fragment;

/// Most items the Clipboard pane keeps.
pub const CLIP_MAX_ITEMS: usize = 24;
/// A copy bigger than this (estimated) is not collected (it still reaches the clipboard).
const CLIP_ITEM_BYTES: usize = 4 << 20;
/// All collected items together stay under this; the oldest go first.
const CLIP_TOTAL_BYTES: usize = 24 << 20;
/// Characters of preview per item.
const CLIP_PREVIEW_CHARS: usize = 120;

/// One item collected by Copy or Cut.
#[derive(Clone, Debug)]
pub struct ClipItem {
    pub fragment: Fragment,
    /// Its plain text.
    pub text: String,
    /// Rough memory use.
    bytes: usize,
}

impl ClipItem {
    /// One line of the item's text for a list: whitespace runs become one space, objects drop out.
    pub fn preview(&self) -> String {
        let words = self.text.split(|c: char| c.is_whitespace() || c == wordcraft_doc::para::OBJ).filter(|w| !w.is_empty());
        let mut out = String::new();
        let mut n = 0;
        for w in words {
            if !out.is_empty() {
                out.push(' ');
                n += 1;
            }
            for c in w.chars() {
                if n >= CLIP_PREVIEW_CHARS {
                    out.push('…');
                    return out;
                }
                out.push(c);
                n += 1;
            }
        }
        out
    }
}

/// Items collected by Copy and Cut during the session, newest first: the Clipboard pane
/// (Home › Clipboard). At most [`CLIP_MAX_ITEMS`]; oversized copies are skipped.
#[derive(Clone, Debug, Default)]
pub struct ClipHistory {
    pub(crate) items: Vec<ClipItem>,
}

impl ClipHistory {
    pub fn items(&self) -> &[ClipItem] {
        &self.items
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(crate) fn push(&mut self, f: &Fragment, text: &str) {
        // Text plus per-paragraph overhead; pictures are references into the document.
        let paras = text.matches('\n').count().saturating_add(f.blocks.len()).saturating_add(f.parts.len()).saturating_add(1);
        let bytes = text.len().saturating_mul(2).saturating_add(paras.saturating_mul(256));
        if bytes > CLIP_ITEM_BYTES || f.blocks.is_empty() {
            return;
        }
        // Copying the same thing again doesn't add a second item.
        if self.items.first().is_some_and(|i| i.text == text && i.fragment == *f) {
            return;
        }
        self.items.insert(0, ClipItem { fragment: f.clone(), text: text.to_string(), bytes });
        self.items.truncate(CLIP_MAX_ITEMS);
        while self.items.len() > 1 && self.items.iter().map(|i| i.bytes).sum::<usize>() > CLIP_TOTAL_BYTES {
            self.items.pop();
        }
    }

    /// The items for agents and the pane, newest first.
    pub(crate) fn describe(&self) -> Value {
        Value::Array(
            self.items
                .iter()
                .enumerate()
                .map(|(i, it)| {
                    json!({
                        "index": i,
                        "preview": it.preview(),
                        "chars": it.text.chars().count(),
                        "objects": it.text.matches(wordcraft_doc::para::OBJ).count(),
                    })
                })
                .collect(),
        )
    }
}
