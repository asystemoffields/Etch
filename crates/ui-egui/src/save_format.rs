//! Saving in a format that can't keep everything, the way Word guards it.
//!
//! Only Word documents (and WordCraft's own JSON) keep the whole document
//! ([`wordcraft_engine::io::keeps_everything`]). The first time the person saves over a file in
//! another format (an `.odt`, `.rtf`, `.txt`, `.md`, `.html`…) in a session, a compatibility
//! prompt says what that format would lose ([`wordcraft_engine::io::lossy_features`]) and offers
//! Keep format (save it, and the document counts as saved, as Word's choice does) or Save as
//! .docx… (Save As with a Word document suggested). It asks again only when the document has
//! gained something more the format would lose. AutoSave never writes such a file.
//!
//! A format WordCraft can't write at all (Word 97-2003 `.doc`) goes to Save As with `.docx`
//! suggested instead of failing.
//!
//! Scripts (`WordApp::execute`, the control channel) never see the prompt: their
//! `file.save` saves and reports what was lost in its `lossy` field.

use serde_json::{Value, json};

use crate::file_dialogs::{AfterSave, Asked};
use crate::{WordApp, dialogs, i18n};

/// The command a "Save changes?" answer holds back until the save is done.
#[derive(Clone, Debug)]
pub struct Then {
    pub then: String,
    pub params: Value,
    pub document: u64,
}

impl Then {
    fn after_save(then: Option<Then>) -> AfterSave {
        match then {
            Some(t) => AfterSave::Continue { then: t.then, params: t.params, document: t.document },
            None => AfterSave::Nothing,
        }
    }
}

/// The interface name of a feature [`wordcraft_engine::io::lossy_features`] reports.
pub fn feature_label(id: &str) -> &'static str {
    match id {
        "comments" => tl!("Comments"),
        "footnotes" => tl!("Footnotes"),
        "endnotes" => tl!("Endnotes"),
        "headersFooters" => tl!("Headers and footers"),
        "trackedChanges" => tl!("Tracked changes"),
        "textBoxes" => tl!("Text boxes"),
        "equations" => tl!("Equations"),
        "contentControls" => tl!("Content controls"),
        "drawings" => tl!("Shapes and charts"),
        "nestedTables" => tl!("Nested tables"),
        "pictures" => tl!("Pictures"),
        "nestedFields" => tl!("Fields inside fields"),
        "fields" => tl!("Fields"),
        "mergedCells" => tl!("Merged table cells"),
        "sections" => tl!("Sections"),
        "pageSetup" => tl!("Page Setup"),
        "properties" => tl!("Properties"),
        "macros" => tl!("Macros"),
        // `formatting`, and anything newer than this list.
        _ => tl!("Formatting"),
    }
}

/// The macro-enabled extension (with its dot) to offer instead of saving `path` without the
/// document's macros: `.dotm` for a template, `.docm` otherwise.
pub fn macro_enabled_ext(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".dotx") || lower.ends_with(".dotm") || lower.ends_with(".dot") { ".dotm" } else { ".docm" }
}

impl WordApp {
    /// Before the person's Save (`params` are `file.save`'s): a format WordCraft can't write
    /// goes to Save As with `.docx` suggested, and the first save over the document's own file
    /// in a format that can't keep everything asks first ([`dialogs::Dialog::SaveFormat`]).
    /// `Some` is the answer when the save was routed there; `None` lets it go ahead. `then` is the
    /// command a "Save changes?" prompt holds back, run once the save is done.
    pub(crate) fn save_format_gate(&mut self, params: &Value, then: Option<Then>) -> Option<Result<Value, String>> {
        // The browser saves by downloading a copy; it has no file of its own to protect.
        if self.services.download.is_some() {
            return None;
        }
        let explicit = params.get("path").and_then(Value::as_str).map(std::path::PathBuf::from);
        let path = explicit.clone().or_else(|| self.session.path.clone())?;
        let path_text = path.to_string_lossy().into_owned();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path_text.clone());
        // Only the implicit Save goes to Save As; a path given explicitly keeps its error, as for
        // scripts (upstream #2445).
        if explicit.is_some() && !wordcraft_engine::io::can_save(&path_text) {
            return None;
        }
        if !wordcraft_engine::io::can_save(&path_text) {
            self.status(i18n::fmt(tl!("Etch can't save {name} in its own format. Save it as a Word document (.docx)."), &[("name", &name)]));
            return Some(match self.save_as(Then::after_save(then)) {
                Asked::Done(r) => r,
                Asked::Pending => Ok(json!({"pending": "saveAs"})),
                Asked::Busy => Ok(json!({"done": false})),
            });
        }
        // Only a save over the document's own file asks about the format: Save As already chose
        // it. But a Save As (or Export) that would leave out the document's macro project asks
        // first, as Word does before saving a macro-free document.
        let save_as = explicit.is_some() && explicit != self.session.path;
        let lost = if save_as {
            if !wordcraft_engine::io::drops_macros(&path_text, &self.session.doc) {
                return None;
            }
            vec!["macros"]
        } else {
            // What the save itself would report: an `.xml` file may be a Word 2003 XML Document.
            wordcraft_engine::cmd::file::lossy_on_save(&self.session, params)?
        };
        let document = self.session.document_id();
        // Asked once for this document and file, unless the document has since gained something
        // more the format can't keep (a footnote added to plain text): that is asked about too,
        // never dropped without a word.
        if self.lossy_ok.as_ref().is_some_and(|(d, p, told)| *d == document && *p == path && lost.iter().all(|f| told.iter().any(|t| t == f))) {
            return None;
        }
        let target = save_as.then_some(path_text);
        self.dialog = Some(dialogs::Dialog::SaveFormat { name, lost: lost.into_iter().map(str::to_string).collect(), then, document, target });
        Some(Ok(json!({"pending": "saveFormat"})))
    }

    /// Answer the compatibility prompt (`ui.saveFormat`): `keep` saves in the file's format
    /// (and doesn't ask again for this document and file, unless it would lose more), `docx`
    /// opens Save As with a Word document that keeps everything suggested (`.docm`/`.dotm` when
    /// the prompt named the macros), `cancel` saves nothing.
    pub(crate) fn answer_save_format(&mut self, answer: &str) -> Result<Value, String> {
        if !matches!(answer, "keep" | "docx" | "cancel") {
            return Err("`answer` must be keep, docx or cancel".into());
        }
        let Some(dialogs::Dialog::SaveFormat { then, document, lost, target, .. }) = self.dialog.take() else {
            return Err("no save format prompt is open".into());
        };
        if document != self.session.document_id() {
            return Err("the document has been replaced".into());
        }
        match answer {
            "cancel" => Ok(json!({"saved": false})),
            "docx" => {
                let after = Then::after_save(then);
                let asked = if lost.iter().any(|f| f == "macros") {
                    // Back to Save As, in a format that keeps the macros (Word's No).
                    let at = target.clone().or_else(|| self.session.path.as_ref().map(|p| p.to_string_lossy().into_owned())).unwrap_or_default();
                    let name = format!("{}{}", self.title_stem(), macro_enabled_ext(&at));
                    self.ask_file(
                        crate::file_dialogs::FileDialogRequest::Save { name },
                        crate::file_dialogs::AfterPick::SaveAs { after, document: self.session.document_id() },
                    )
                } else {
                    self.save_as(after)
                };
                match asked {
                    Asked::Done(r) => r,
                    Asked::Pending => Ok(json!({"pending": "saveAs"})),
                    Asked::Busy => Ok(json!({"done": false})),
                }
            }
            _ => {
                // A Save As (or Export) to `target`, or a save over the document's own file.
                let Some(path) = target.as_ref().map(std::path::PathBuf::from).or_else(|| self.session.path.clone()) else {
                    return Err("the document has no file to save to".into());
                };
                self.lossy_ok = Some((document, path, lost));
                let params = target.map_or_else(|| json!({}), |t| json!({"path": t}));
                match then {
                    None => self.execute_user("file.save", params),
                    // As the Save Changes prompt's Save: the held-back command runs once the save
                    // is written (in the browser, once its picker has saved, #1315); a failed save
                    // brings the Save Changes prompt back with the reason (#1445).
                    Some(t) => match self.save_for_prompt(params) {
                        Ok(crate::PromptSave::Saved) => self.execute_user(&t.then, t.params).map(|r| json!({"done": true, "result": r})),
                        Ok(crate::PromptSave::NotSaved) => Ok(json!({"done": false})),
                        Ok(crate::PromptSave::Later(ticket)) => {
                            let rev = self.session.rev();
                            self.awaiting_save = Some(crate::AwaitingSave { ticket, then: t.then, params: t.params, document: t.document, rev });
                            Ok(json!({"pending": "save"}))
                        }
                        Err(error) => Err(self.save_failed(&error, t.then, t.params, t.document)),
                    },
                }
            }
        }
    }
}
