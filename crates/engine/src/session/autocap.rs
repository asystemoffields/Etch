//! The newest automatic capitalisation, kept on the session so that reversing it makes an
//! AutoCorrect exception as in Word (#1857). `cmd::tools` makes one when it capitalises;
//! `cmd::text` and undo consult it.

use wordcraft_doc::Pos;

use crate::cmd::tools::AutoCorrectPrefs;

/// The newest automatic capitalisation (a sentence capital, a TWo INitial CApitals fix), kept so
/// that undoing it, or backspacing it and typing the word again, makes an exception (#1857).
#[derive(Clone, Debug)]
pub struct AutoCap {
    /// Undo steps pushed (ever) right after the change (`u64::MAX` until the caller has pushed its step).
    pub(crate) depth: u64,
    pub(crate) story: wordcraft_doc::StoryRef,
    pub(crate) path: wordcraft_doc::Path,
    /// Byte offset of the word in its paragraph.
    pub(crate) start: usize,
    /// The token before a sentence capital's word, lowercase ("there."); empty when none or when
    /// the capital wasn't a sentence one.
    pub(crate) prev: String,
    /// The TWo INitial CApitals word as typed ("TWo").
    pub(crate) two_caps: Option<String>,
    /// The user backspaced into the word after the change.
    pub(crate) backspaced: bool,
}

impl AutoCap {
    pub(crate) fn add_exceptions(self, ac: &mut AutoCorrectPrefs) {
        if ac.auto_add_first_letter && !self.prev.is_empty() {
            ac.add_exception(self.prev);
        }
        if ac.auto_add_initial_caps
            && let Some(w) = self.two_caps
        {
            ac.add_initial_caps_exception(&w);
        }
    }

    /// A Backspace deleted `a..f`; it reached back over the capitalised word's first letter.
    pub(crate) fn note_backspace(&mut self, a: &Pos, f: &Pos) {
        if f.story == self.story && f.path == self.path && a.off <= self.start && f.off > self.start {
            self.backspaced = true;
        }
    }
}
