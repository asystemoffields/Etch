//! [`Session::run`], the single entry point for commands (UI, CLI, control channel), as a
//! pipeline of named steps: the guard, the undo snapshot, running the command, the post-edit
//! invariants, and commit or roll-back. Every step keeps the order and the early returns the
//! inline function had; a step's doc comment says what it protects and what pinned it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use wordcraft_doc::edit::Fragment;
use wordcraft_doc::limits::Tally;
use wordcraft_doc::props::CharProps;
use wordcraft_doc::{Document, Pos};

use super::{
    ColumnBlock, MAX_TABLE_NESTING, MathEdit, Selection, Session, Undo, limit_refusal, tables_made_too_deep, tables_too_deep, tally_changed,
};
use crate::{CmdError, CommandSpec, Registry};

/// What the guard let through: the command's spec and what Restrict Editing decided.
struct Guarded<'a> {
    spec: &'a CommandSpec,
    /// Restrict Editing allowed the command on condition that it only edits the exceptions.
    guard: bool,
    /// Quick Parts before a guarded command: they aren't part of the document, so a refused edit
    /// puts them back itself (the Spike, #2507).
    blocks_before: Option<BTreeMap<String, Fragment>>,
}

/// What macro recording and Repeat noted before the command ran, for a failed command to take
/// back.
struct Repeat {
    /// Macro steps recorded before this command.
    recorded: Option<usize>,
    /// The Repeat command this one replaced (`Some(previous)` when it did).
    last_command: Option<Option<(String, Value)>>,
}

/// What undo keeps and roll-back restores. The undo stacks are snapshotted whole, not as a
/// length: commands run nested commands, whose checkpoints, undos and redos (and evictions at
/// the limit) must be put back too. The document copy is shared with the undo step the command
/// checkpoints, so it is made once.
struct Snapshot {
    doc: Arc<Document>,
    history: Vec<Arc<Undo>>,
    redo: Vec<Arc<Undo>>,
    typing_open: bool,
    undo_evicted: u64,
    /// Which document it was ([`Session::set_document`] changes it): a command that replaced
    /// the document (Open, New) had it counted whole, so the limits check leaves it alone.
    document_id: u64,
}

/// The open file and what belongs to it, which a command that replaces the document drops with
/// it: Compare and Combine shown in a new document, Envelopes and Labels. A refused one puts
/// them back with the document, or the next Save of the untouched document became a Save As
/// and its version snapshots were gone (#1327: upstream 5220199b, debb125d).
struct FileIdentity {
    path: Option<PathBuf>,
    file_name: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    file_version: Option<crate::io::FileVersion>,
    document_id: u64,
    password: Option<crate::io::Password>,
    versions: Vec<(String, String, Document)>,
    bib_style: String,
}

impl FileIdentity {
    /// The commands that can replace the document while they run. Only these pay for the copy:
    /// the version snapshots are whole documents.
    fn kept_for(s: &Session, id: &str) -> Option<Self> {
        matches!(id, "review.compare" | "review.combine" | "mailings.envelopes" | "mailings.labels").then(|| FileIdentity {
            path: s.path.clone(),
            file_name: s.file_name.clone(),
            #[cfg(not(target_arch = "wasm32"))]
            file_version: s.file_version.clone(),
            document_id: s.document_id,
            password: s.password.clone(),
            versions: s.versions.clone(),
            bib_style: s.bib_style.clone(),
        })
    }

    fn restore(self, s: &mut Session) {
        s.path = self.path;
        s.file_name = self.file_name;
        #[cfg(not(target_arch = "wasm32"))]
        {
            s.file_version = self.file_version;
        }
        s.document_id = self.document_id;
        s.password = self.password;
        s.versions = self.versions;
        s.bib_style = self.bib_style;
    }
}

/// What one run carries between its steps (it lives for that run only, so nothing is added to
/// `Session`): the command, what a failed command puts back, and what the steps found out.
struct RunCtx<'a> {
    id: &'a str,
    params: &'a Value,
    spec: &'a CommandSpec,
    /// [`Session::join_next_undo`] was set: the edit joins the undo step before it.
    join: bool,
    guard: bool,
    blocks_before: Option<BTreeMap<String, Fragment>>,
    recorded: Option<usize>,
    last_command: Option<Option<(String, Value)>>,
    /// What a failed command puts back besides the document: the equation being edited and the
    /// formatting waiting at the caret (taken or changed before or by the command), the
    /// selection and column block (pure commands move them too), and the dirty flag.
    math_before: Option<MathEdit>,
    pending_before: Option<CharProps>,
    sel_entry: Selection,
    column_entry: Option<ColumnBlock>,
    dirty_before: bool,
    /// The document's revision as the command enters: a command that leaves it unchanged
    /// changed nothing the outline follows.
    rev_entry: u64,
    /// The open file, for a command that can replace the document.
    identity_before: Option<FileIdentity>,
    /// The document and undo state before the command; `None` for a read-only command.
    before: Option<Snapshot>,
    /// The command panicked (and the panic was caught).
    panicked: bool,
    /// The count the limits check started from, to count again after the follow-ups.
    counted_from: Option<Tally>,
}

impl<'a> RunCtx<'a> {
    /// What a failed command puts back besides the document, as the command enters.
    fn enter(s: &Session, id: &'a str, params: &'a Value, join: bool, guarded: Guarded<'a>, repeat: Repeat) -> Self {
        RunCtx {
            id,
            params,
            spec: guarded.spec,
            join,
            guard: guarded.guard,
            blocks_before: guarded.blocks_before,
            recorded: repeat.recorded,
            last_command: repeat.last_command,
            math_before: s.math.clone(),
            pending_before: s.pending.clone(),
            sel_entry: s.sel.clone(),
            column_entry: s.column.clone(),
            dirty_before: s.dirty,
            rev_entry: s.rev,
            identity_before: FileIdentity::kept_for(s, id),
            before: None,
            panicked: false,
            counted_from: None,
        }
    }
}

impl Session {
    /// Run a command by id with JSON params (the single entry point for the UI, CLI and the
    /// control channel). Mutating commands are undoable; a failed command leaves the document as
    /// it was; a panic inside a command becomes an error.
    pub fn run(&mut self, id: &str, params: &Value) -> Result<Value, CmdError> {
        // Taken before anything can refuse the command: a refused command doesn't join the next.
        let join = std::mem::take(&mut self.join_next);
        let reg = self.registry.clone();
        let guarded = self.guard(&reg, id, params)?;
        let repeat = self.record_for_repeat(guarded.spec, id, params);
        let mut cx = RunCtx::enter(self, id, params, join, guarded, repeat);
        self.leave_equation(&cx);
        self.snapshot(&mut cx);
        self.forget_tracked_cut(&cx);
        // The selection as the command finds it.
        let sel_before = self.sel.clone();
        let mut result = self.execute(&mut cx);
        self.check_protected_regions(&cx, &mut result);
        self.check_table_nesting(&cx, &mut result);
        self.check_limits(&mut cx, &mut result);
        // Taken whatever the outcome: the flag is this command's alone.
        let unchanged = std::mem::take(&mut self.unchanged);
        match &result {
            Ok(_) => {
                if self.drop_unchanged_step(&mut cx, unchanged) {
                    return result;
                }
                if cx.spec.mutates {
                    // The follow-ups. Each says whether it may have changed the document (empty
                    // cells get a paragraph, text boxes no anchor shows go…): it is counted
                    // again then.
                    let mut followed_up = self.record_format_revisions(&cx);
                    followed_up |= self.tidy_comment_anchors(&cx);
                    self.touch();
                    followed_up |= self.keep_stories_nonempty();
                    followed_up |= self.prune_unshown_text_boxes();
                    followed_up |= self.reroute_connectors(&cx);
                    if followed_up {
                        self.recount_after_follow_ups(&mut cx);
                    }
                }
                self.commit(&cx, &sel_before);
            }
            Err(e) => self.roll_back(cx, e),
        }
        result
    }

    /// Step 1, the guard: an unknown or disabled command is refused. Under Restrict Editing a
    /// mutating command is refused outright or allowed as long as it only edits the exceptions
    /// (`cmd::protect`, #412), and a document protected for tracked changes has Track Changes
    /// forced on for it.
    fn guard<'a>(&mut self, reg: &'a Registry, id: &str, params: &Value) -> Result<Guarded<'a>, CmdError> {
        let Some(spec) = reg.get(id) else { return Err(CmdError::Unknown(id.to_string())) };
        if let Some(why) = (spec.enabled)(self) {
            return Err(CmdError::Disabled(format!("{id}: {why}")));
        }
        // Restrict Editing: refused outright, or allowed as long as it only edits the exceptions.
        let guard = if spec.mutates { crate::cmd::protect::check_before(self, id, params)? } else { false };
        // Quick Parts aren't part of the document, so a refused edit puts them back itself (the Spike).
        let blocks_before = guard.then(|| self.building_blocks.clone());
        if spec.mutates && self.doc.settings.protection.as_deref() == Some("trackedChanges") {
            self.doc.settings.track_changes = true;
        }
        Ok(Guarded { spec, guard, blocks_before })
    }

    /// Step 2, macro recording and Repeat. The command is recorded before it runs, so a recorded
    /// command comes before the commands it runs itself; a command that fails takes its steps
    /// (and its Repeat) back in `roll_back`. A password never lands in a recorded macro or Repeat.
    fn record_for_repeat(&mut self, spec: &CommandSpec, id: &str, params: &Value) -> Repeat {
        let record = !matches!(id, "tools.recordMacro" | "tools.macros" | "edit.undo" | "edit.redo" | "edit.repeat")
            && (spec.mutates || id.starts_with("caret.") || id.starts_with("select."));
        let kept = || {
            let mut p = params.clone();
            if let Some(m) = p.as_object_mut() {
                m.remove("password");
            }
            p
        };
        let recorded = self.recording.as_ref().map(|(_, steps)| steps.len());
        if record && let Some((_, steps)) = self.recording.as_mut() {
            steps.push((id.to_string(), kept()));
        }
        let last_command = if spec.mutates && !matches!(id, "edit.undo" | "edit.redo" | "edit.repeat") {
            Some(self.last_command.replace((id.to_string(), kept())))
        } else {
            None
        };
        Repeat { recorded, last_command }
    }

    /// Step 3: typing, caret movement, selection and Paste outside the equation commands leave
    /// the equation being edited, with the caret right after it (the equation editor, #191).
    fn leave_equation(&mut self, cx: &RunCtx) {
        let id = cx.id;
        if self.math.is_some()
            && (id.starts_with("text.") || id.starts_with("caret.") || id.starts_with("select.") || id.starts_with("edit.paste"))
            && let Some(m) = self.math.take()
        {
            let off = m.at.off + wordcraft_doc::para::OBJ.len_utf8();
            self.sel = Selection::caret(Pos { off, ..m.at });
        }
    }

    /// Step 4, the snapshot: what undo keeps and a failed command goes back to. A pure command
    /// copies the document too (blocks are shared, so it's cheap) unless it only moves the caret,
    /// the selection or the view, so a panic can't leave it half changed. Then the undo step:
    /// typing, and Backspace or Delete at a caret, group into the open step; any other edit
    /// closes the group and checkpoints under the command's label, unless it joins the step
    /// before it; caret movement closes the group without a step, other pure commands keep it
    /// (whole-stack snapshots: #153, c0ccda28; one copy per command, and pure commands other than
    /// caret, selection, view and `format.state` copied too: 75fe8c7b; Backspace runs: 7f65f619;
    /// joining: #57).
    fn snapshot(&mut self, cx: &mut RunCtx) {
        let (id, spec) = (cx.id, cx.spec);
        let read_only = !spec.mutates && (id.starts_with("caret.") || id.starts_with("select.") || id.starts_with("view.") || id == "format.state");
        cx.before = (!read_only).then(|| Snapshot {
            doc: Arc::new(self.doc.clone()),
            history: self.history.clone(),
            redo: self.redo.clone(),
            typing_open: self.typing_open,
            undo_evicted: self.undo_evicted,
            document_id: self.document_id,
        });
        let typing = spec.id == "text.insert" || spec.id == "equation.type";
        // Backspace and Delete at a caret group like typing; deleting a selection starts a step.
        let deleting = matches!(id, "text.backspace" | "text.delete") && self.sel.is_collapsed();
        if spec.mutates && !typing && !deleting {
            self.typing_open = false;
        }
        if spec.mutates {
            let label = if typing { "Typing" } else { spec.label };
            if !cx.join || self.history.is_empty() {
                match &cx.before {
                    Some(b) => self.checkpoint_shared(label, Arc::clone(&b.doc)),
                    None => self.checkpoint(label),
                }
            }
        } else if !spec.id.starts_with("caret.") && !spec.id.starts_with("view.") {
            // Non-mutating commands other than caret movement keep the typing group.
        } else if spec.id.starts_with("caret.") {
            self.typing_open = false;
        }
    }

    /// Step 5: only the paste right after a tracked cut makes it a move (`cmd::moves`, #400);
    /// any other edit, Undo or Redo forgets the cut.
    fn forget_tracked_cut(&mut self, cx: &RunCtx) {
        let id = cx.id;
        if (cx.spec.mutates || matches!(id, "edit.undo" | "edit.undoTo" | "edit.redo"))
            && !matches!(id, "edit.cut" | "edit.paste" | "edit.pasteSpecial")
        {
            self.pending_move = None;
        }
    }

    /// Step 6, execute: the command runs through the column-block dispatcher. A panic inside it
    /// is caught and becomes an error, the last-resort guard of the never-crash rule; roll-back
    /// then restores the document (75fe8c7b).
    fn execute(&mut self, cx: &mut RunCtx) -> Result<Value, CmdError> {
        let (id, params, mutates, run) = (cx.id, cx.params, cx.spec.mutates, cx.spec.run);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::cmd::column::dispatch(self, id, mutates, run, params)));
        match result {
            Ok(r) => r,
            Err(p) => {
                cx.panicked = true;
                let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
                Err(CmdError::Failed(format!("{id} panicked: {msg}")))
            }
        }
    }

    /// Post-edit check: under Restrict Editing nothing outside the editable regions may change;
    /// an edit that changed something there is refused, and rolled back (`cmd::protect`, #412).
    fn check_protected_regions(&self, cx: &RunCtx, result: &mut Result<Value, CmdError>) {
        if result.is_ok()
            && cx.guard
            && let Some(b) = &cx.before
            && !crate::cmd::protect::outside_unchanged(&b.doc, &self.doc, &self.author)
        {
            *result = Err(CmdError::Disabled(format!("{}: only the editable regions of this protected document can change", cx.id)));
        }
    }

    /// Post-edit check: tables nest at most [`MAX_TABLE_NESTING`] deep. Deeper cells are
    /// invisible to the paragraph walk and dropped on save, so whatever made them (Insert Table,
    /// paste, Draw Table…) is refused; a document opened already nested deeper stays editable
    /// (ef32ccf4; only what the command changed is looked at: f3fb261f).
    fn check_table_nesting(&self, cx: &RunCtx, result: &mut Result<Value, CmdError>) {
        if result.is_ok()
            && cx.spec.mutates
            && let Some(b) = &cx.before
            && tables_made_too_deep(&b.doc, &self.doc)
            && !tables_too_deep(&b.doc)
        {
            *result = Err(CmdError::Disabled(format!("{}: tables can be nested at most {MAX_TABLE_NESTING} deep", cx.id)));
        }
    }

    /// Post-edit check: a document stays within the limits a file may have to be opened again
    /// (`wordcraft_doc::limits`): an edit that would take it past one (a paste, Enter at the
    /// paragraph limit, a picture past the pictures' bytes, typing past the saved size, Replace
    /// All, Compare) is refused, naming what is over, so whatever the person writes saves into a
    /// file that opens. Only what the command changed is counted. Undo and Redo go back to
    /// documents that were; a document already over a limit stays editable as long as it doesn't
    /// grow past it. A command that replaced the document (Open, New) had it counted whole
    /// (`set_document`); Undo and Redo put back the count their step kept (tasks 06 and 09:
    /// ad4bf6b3, 0003ff87, 254a650b; `tests_limits`).
    fn check_limits(&mut self, cx: &mut RunCtx, result: &mut Result<Value, CmdError>) {
        let undoing = matches!(cx.id, "edit.undo" | "edit.redo" | "edit.undoTo");
        if result.is_ok()
            && !undoing
            && let Some(b) = &cx.before
            && self.document_id == b.document_id
        {
            let (base, after) = self.count_changes(&b.doc);
            match after.grown_over(&base, &self.limits) {
                Some(over) if cx.spec.mutates => *result = Err(limit_refusal(cx.id, over)),
                _ => {
                    self.tally = Some(after);
                    cx.counted_from = Some(base);
                }
            }
        }
    }

    /// Post-edit invariant: an edit that changed nothing (Backspace at the start of the
    /// document, Delete at its end) leaves no undo step of its own, or Undo would seem to do
    /// nothing, and doesn't mark the document edited: the undo stacks and the dirty flag go back
    /// to the snapshot (88d4d96e; `tests_typing::backspace_or_delete_with_nothing_to_delete_leaves_no_undo_step`).
    /// Returns whether it did, in which case `run` returns at once: none of the follow-ups or the
    /// commit bookkeeping run.
    fn drop_unchanged_step(&mut self, cx: &mut RunCtx, unchanged: bool) -> bool {
        if cx.spec.mutates
            && unchanged
            && let Some(b) = cx.before.take()
        {
            self.history = b.history;
            self.redo = b.redo;
            self.typing_open = b.typing_open;
            self.undo_evicted = b.undo_evicted;
            self.dirty = cx.dirty_before;
            self.clamp_selection();
            return true;
        }
        false
    }

    /// Post-edit follow-up: formatting changed while Track Changes is on becomes a formatting
    /// revision (`w:rPrChange`/`w:pPrChange`, #41: 972df1e0; tracked table rows and cells:
    /// 0c4e30c1). Undo and Redo record none, as `fmt_revisions::record` knows. Returns whether
    /// it may have changed the document (tracking was on), so the limits count is redone (254a650b).
    fn record_format_revisions(&mut self, cx: &RunCtx) -> bool {
        let Some(b) = &cx.before else { return false };
        let followed_up = self.doc.settings.track_changes && !matches!(cx.id, "edit.undo" | "edit.redo");
        crate::cmd::fmt_revisions::record(self, cx.id, &b.doc);
        followed_up
    }

    /// Post-edit follow-up: no comment anchor dangles after an edit (0c4e30c1, 62fd993d;
    /// `tests_review::deleting_commented_text_leaves_no_dangling_anchor`). Not after opening or
    /// comparing documents or undoing, and not after Cut: Paste puts the comment back. Returns
    /// whether it may have changed the document (it has comments).
    fn tidy_comment_anchors(&mut self, cx: &RunCtx) -> bool {
        let id = cx.id;
        let Some(b) = &cx.before else { return false };
        let edits =
            !(id.starts_with("file.") || matches!(id, "review.compare" | "review.combine" | "edit.cut" | "edit.undo" | "edit.redo" | "edit.undoTo"));
        if !edits {
            return false;
        }
        let followed_up = !self.doc.comments.is_empty();
        crate::cmd::review::tidy_comments(self, &b.doc);
        followed_up
    }

    /// Post-edit follow-up: every story and every table cell holds at least one paragraph
    /// (b8dd46cc). Only the blocks the edit didn't share with the body as it was last checked
    /// are looked at, so typing in one cell of a long table looks at that row (f3fb261f;
    /// `wordcraft_doc` `tests::ensure_nonempty_since_looks_at_what_changed`). Returns whether a
    /// paragraph was added.
    fn keep_stories_nonempty(&mut self) -> bool {
        let added = match self.nonempty_body.take() {
            Some(base) => self.doc.ensure_nonempty_since(&base),
            None => self.doc.ensure_nonempty(),
        };
        self.nonempty_body = Some(self.doc.body.clone());
        added
    }

    /// Post-edit follow-up: the text of a text box no anchor shows any more (a deleted or cut
    /// box) goes with it; undo snapshots and the clipboard keep their own copies (#46: e4456efb;
    /// `deleted_text_box_takes_its_text_with_it_until_undo`). Returns whether any went.
    fn prune_unshown_text_boxes(&mut self) -> bool {
        self.doc.prune_text_boxes() > 0
    }

    /// Post-edit follow-up: glued connectors follow the shapes they join, after the commands
    /// that move, resize, turn, add or change drawings (#416: 2ffba93b;
    /// `tests_wordart::moving_a_joined_shape_reroutes_its_connector`). Returns whether one moved.
    fn reroute_connectors(&mut self, cx: &RunCtx) -> bool {
        if crate::cmd::connectors::reroutes_after(cx.id) && crate::cmd::connectors::reroute(self) {
            self.touch();
            return true;
        }
        false
    }

    /// Post-edit follow-up: a follow-up that may have changed the document has it counted again
    /// for the limits, from the document before the command (what changed in place since the
    /// first count isn't told apart by its pointer), or whole when there was no first count
    /// (0003ff87, 254a650b; `tests_save_safety::the_paragraph_cap_follows_every_kind_of_edit`).
    fn recount_after_follow_ups(&mut self, cx: &mut RunCtx) {
        self.tally = match (cx.counted_from.take(), &cx.before) {
            (Some(base), Some(b)) => tally_changed(&b.doc, &self.doc).map(|(gone, come)| base.minus(gone).plus(come)),
            _ => None,
        }
        .or_else(|| Some(Tally::of(&self.doc)));
    }

    /// The last step of a command that succeeded: the selection is made valid; a pure command
    /// that moved the insertion point ends the typing group, like caret movement (typing,
    /// Select, typing are two undo steps, #419: 50505d9e;
    /// `tests_typing::typing_then_selecting_then_typing_are_two_undo_steps`) and drops the
    /// formatting waiting at the old caret (88d4d96e;
    /// `tests_typing::formatting_waiting_at_the_caret_is_dropped_by_a_new_selection`); an edit
    /// is remembered for Go Back (#343: adcbf526); Outline view's collapsed headings follow
    /// when the document changed (#493: 900c0f44; #3621: 831b86da); extra selected objects are
    /// let go (29e978d6); and the word being typed is noted for the spelling squiggles (#1552:
    /// 61a62682; `tests_typing::the_word_being_typed_is_not_marked_until_it_ends`).
    fn commit(&mut self, cx: &RunCtx, sel_before: &Selection) {
        let (id, mutates) = (cx.id, cx.spec.mutates);
        self.clamp_selection();
        if !mutates && self.sel != *sel_before {
            self.typing_open = false;
            // Formatting waiting at the caret (Bold with nothing selected) belongs to that
            // caret: a new selection (Select All, Find, Select Word…) drops it, as caret
            // movement does, so it can't land on text typed or replaced there. Undo and
            // Redo bring back the step's own.
            if !matches!(id, "edit.undo" | "edit.redo" | "edit.undoTo") {
                self.pending = None;
            }
        }
        if mutates && !matches!(id, "edit.undo" | "edit.redo") {
            self.note_edit();
        }
        // Outline view: collapsed headings follow edits; the caret stays where it shows.
        crate::cmd::outline::after_command(self, sel_before, self.rev != cx.rev_entry);
        // Extra selected objects last until something else is edited or selected.
        if !matches!(id, "select.addObject" | "arrange.ungroup") && (mutates || self.sel != *sel_before) {
            self.also_selected.clear();
        }
        self.note_typing(id, cx.before.as_ref().map(|b| &*b.doc), sel_before);
    }

    /// The last step of a command that failed: a failed mutating command, or a pure one that
    /// panicked, leaves no trace. The undo stacks go back first, so the document copy they
    /// shared is ours alone again, then the document, the selection, the column block, the
    /// equation, the formatting waiting at the caret and the dirty flag, with a new revision for
    /// the caches (75fe8c7b; a failed command keeps undo history: #153, c0ccda28;
    /// `tests::failed_command_leaves_document_unchanged`), and the open file a command that
    /// replaced the document dropped (#1327; `tests_review::a_refused_comparison_keeps_the_documents_file_identity`). A pure command that fails normally
    /// keeps what it did, as a pure command may: Find remembers the search, Save the session's
    /// settings it checked. Whatever the command was, its macro steps and its Repeat are taken
    /// back, the Quick Parts a guarded edit changed are put back (the Spike, #2507: 4362e662),
    /// the pending automatic capitalisation is forgotten (#1857: 1d370431), and the error
    /// becomes the status.
    fn roll_back(&mut self, cx: RunCtx, e: &CmdError) {
        self.autocap = None;
        let undo = cx.spec.mutates || cx.panicked;
        if undo && let Some(b) = cx.before {
            // The stacks first, so the document copy they shared is ours alone again.
            self.history = b.history;
            self.redo = b.redo;
            self.doc = Arc::unwrap_or_clone(b.doc);
            self.typing_open = b.typing_open;
            // The steps the command pushed out are back, so they no longer count as gone.
            self.undo_evicted = b.undo_evicted;
        }
        if undo {
            self.sel = cx.sel_entry;
            self.column = cx.column_entry;
            self.math = cx.math_before;
            self.pending = cx.pending_before;
            // The document is as it was: caches see a new revision, the dirty flag is back.
            self.rev = self.rev.wrapping_add(1);
            self.dirty = cx.dirty_before;
            if let Some(f) = cx.identity_before {
                f.restore(self);
            }
        }
        if let Some(n) = cx.recorded
            && let Some((_, steps)) = self.recording.as_mut()
        {
            steps.truncate(n);
        }
        if let Some(last) = cx.last_command {
            self.last_command = last;
        }
        if let Some(b) = cx.blocks_before {
            self.building_blocks = b;
        }
        self.status = e.to_string();
    }
}
