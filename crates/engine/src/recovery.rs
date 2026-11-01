//! Local desktop recovery. Encoding and file I/O run on a worker, outside typing.
use crate::{CmdError, CmdResult, Session};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryInfo {
    pub id: String,
    pub name: String,
    pub saved_at: String,
    /// The file the document was opened from or saved to when the copy was made (`None`: an
    /// untitled document, or a name that isn't valid Unicode). A confined control connection may
    /// restore or discard only a copy whose file lies inside its roots (`docs/recovery.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A password-protected document's copy: encrypted with its password, which restoring it
    /// asks for. Listed as "Protected document" without its file (`docs/recovery.md`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub protected: bool,
    /// Pictures and other parts (by part name) this copy couldn't keep: over the size budget,
    /// not written before an emergency copy's deadline, or found damaged when restored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted: Vec<String>,
    /// The copy's newest generation was damaged (its checksum didn't match, or it couldn't be
    /// read): this is the generation before it, which File › Recover says. Found when the copies
    /// are listed or restored; never written into a copy.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub earlier: bool,
    /// The file the copy was made from (`path`) was changed after the copy was made: it is older
    /// than the saved file, which File › Recover says. Found when the copies are listed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub older_than_file: bool,
}

/// What a restore says when the copy's newest generation was damaged and the one before it was
/// restored ([`RecoveryInfo::earlier`]); the interface translates it.
pub const RESTORED_EARLIER: &str =
    "The newest recovery copy was damaged, so the copy saved before it was restored. The most recent edits may be missing.";

/// What the current document's recovery copy can't cover, shown as "Not protected by
/// recovery" until a later copy covers it all or the document is saved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryGap {
    /// The copy keeps the text and structure but not these pictures/parts (the size budget).
    Omitted(Vec<String>),
    /// No copy could be written; why.
    Failed(String),
}

impl RecoveryInfo {
    /// May the automation roots in force (if any) restore (`Access::Read`) or discard
    /// (`Access::Write`) this copy? Only when its original file is inside the root of that kind;
    /// an untitled document is outside every root. Without roots, always.
    pub fn allowed(&self, access: crate::file_access::Access) -> Result<(), String> {
        if crate::file_access::current().is_none() {
            return Ok(());
        }
        let what = match access {
            crate::file_access::Access::Read => "restore",
            crate::file_access::Access::Write => "discard",
        };
        let Some(path) = &self.path else {
            return Err(format!("automation can't {what} the recovery copy of an untitled document: it has no file inside the automation roots"));
        };
        let checked = match access {
            crate::file_access::Access::Read => crate::file_access::check_read(path),
            crate::file_access::Access::Write => crate::file_access::check_write(path),
        };
        checked.map_err(|e| format!("automation can't {what} this recovery copy: {e}"))
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use desktop::Recovery;

impl Session {
    /// The desktop host supplies the profile folder and a thread-safe wakeup.
    /// Headless sessions and the browser never write recovery files implicitly.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn enable_recovery(&mut self, root: std::path::PathBuf, wake: std::sync::Arc<dyn Fn() + Send + Sync>) -> Result<(), String> {
        self.recovery = Some(Recovery::start(root, wake)?);
        Ok(())
    }
    pub fn recovery_documents(&self) -> Vec<RecoveryInfo> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &self.recovery {
            return r.documents.clone();
        }
        Vec::new()
    }
    pub fn recovery_ready(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &self.recovery {
            return r.ready;
        }
        false
    }
    pub fn recovery_busy(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &self.recovery {
            return r.busy;
        }
        false
    }
    pub fn recovery_error(&self) -> Option<&str> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &self.recovery {
            return r.error.as_deref();
        }
        None
    }
    /// A delayed restore can't replace edits made while it was loading. Restores have no path.
    pub fn recovery_tick(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(mut r) = self.recovery.take() else { return };
            if let Some(restored) = r.poll() {
                if restored.expected == (self.document_id(), self.rev()) {
                    let desktop::Restored { snapshot, password, .. } = restored;
                    let id = snapshot.info.id;
                    let omitted = snapshot.info.omitted;
                    let earlier = snapshot.info.earlier;
                    self.set_document(snapshot.document);
                    // A protected document's copy restores protected, with the password given.
                    self.password = password;
                    self.path = None;
                    self.dirty = true;
                    r.adopt(id, self.document_id(), self.rev());
                    if !omitted.is_empty() {
                        r.error = Some(format!("Restored without pictures the recovery copy couldn't keep: {}", omitted.join(", ")));
                    } else if earlier {
                        // The person is told: the newest copy didn't restore, an earlier one did.
                        r.error = Some(RESTORED_EARLIER.into());
                    }
                } else {
                    r.error = Some("Recovery cancelled: the document changed while the copy was loading.".into());
                }
            }
            if let Err(e) = r.tick(self, std::time::Instant::now()) {
                r.error = Some(e);
            }
            if let Some(e) = &r.error {
                self.status = e.clone();
            }
            self.recovery = Some(r);
        }
    }
    pub fn recovery_wait(&self) -> Option<std::time::Duration> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &self.recovery
            && !r.busy
            && (self.dirty || r.has_stale())
        {
            return r.wait(self.document_id(), self.rev());
        }
        None
    }

    /// Write the recovery copy of unsaved changes now, synchronously (waiting at most
    /// `timeout`): the app is being terminated by the system (SIGTERM, SIGINT, SIGHUP, logout)
    /// or a panic is unwinding, and the periodic copy may be up to 10 seconds old. `Ok(true)`
    /// once an up-to-date copy is on disk, `Ok(false)` when there is nothing to keep (no recovery
    /// in this session, or a saved document). A password-protected document's copy is encrypted;
    /// pictures not yet in an earlier copy are written only while time is left for the text.
    /// See `docs/recovery.md`.
    pub fn emergency_snapshot(&mut self, timeout: std::time::Duration) -> Result<bool, String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(mut r) = self.recovery.take() else { return Ok(false) };
            let result = r.snapshot_now(self, timeout);
            self.recovery = Some(r);
            result
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = timeout;
            Ok(false)
        }
    }

    /// What recovery can't cover of the current document's unsaved changes (`None`: all of them,
    /// or nothing to cover). The desktop status bar shows it.
    pub fn recovery_gap(&self) -> Option<RecoveryGap> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &self.recovery
            && self.dirty
        {
            if !r.alive() {
                return Some(RecoveryGap::Failed(r.error.clone().unwrap_or_else(|| "recovery worker is unavailable".into())));
            }
            if r.document_is(self.document_id()) {
                return r.gap().cloned();
            }
        }
        None
    }

    /// Tests: the largest a recovery record (text, pictures and parts) may be, to see what
    /// happens over the budget without writing gigabytes.
    #[doc(hidden)]
    pub fn set_recovery_record_budget(&mut self, bytes: u64) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &mut self.recovery {
            r.limits.record = bytes;
        }
        #[cfg(target_arch = "wasm32")]
        let _ = bytes;
    }

    /// The id of a protected recovery copy whose restore failed because the password was wrong,
    /// once: the front end asks for the password again.
    pub fn take_recovery_wrong_password(&mut self) -> Option<String> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &mut self.recovery {
            return r.take_wrong_password();
        }
        None
    }

    /// Remove the current document's recovery copy now, waiting for the worker: the user chose
    /// Don't Save, or is closing a saved document. A copy left behind would be offered as
    /// unsaved work at the next start. Failures are logged; the copy then stays (never lost).
    pub fn discard_recovery(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let document = self.document_id();
            if let Some(r) = &mut self.recovery {
                if r.document_is(document)
                    && let Err(e) = r.purge()
                {
                    log::warn!("recovery: cannot discard the copy: {e}");
                }
                r.purge_stale();
            }
        }
    }
    /// Recovery's housekeeping before the process exits (#3140, upstream f550ab7b): a saved
    /// document's copy is out of date, so it goes now, waiting for the worker, as do the copies
    /// of replaced saved documents. The window's close button does this in
    /// `WordApp::close_requested`, but an exit that doesn't pass through it (on macOS a quit
    /// scheduled after the Save Changes prompt's Save, or a control-channel quit, goes straight
    /// to termination) left the copy behind, and the next start offered to recover a document
    /// that was saved cleanly. Desktop hosts call this where
    /// they wait out pending print jobs. Unsaved changes keep their copy, a protected document's
    /// (encrypted) one too. Failures are logged; the copy then stays (never lost).
    pub fn finish_recovery(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if !self.dirty {
            self.discard_recovery();
        } else if let Some(r) = &mut self.recovery {
            r.purge_stale();
        }
    }
    /// Only an explicit protect/save action waits for the worker and removes plaintext copies.
    /// A failure prevents applying the password; typing never waits.
    pub(crate) fn clear_recovery_before_password(&mut self) -> Result<(), CmdError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let document = self.document_id();
            if let Some(r) = &mut self.recovery
                && r.document_is(document)
            {
                r.purge().map_err(CmdError::Failed)?;
            }
        }
        Ok(())
    }
    pub(crate) fn recover(&mut self, v: &Value) -> CmdResult {
        let restore = v.get("restore");
        let discard = v.get("discard");
        if restore.is_some() && discard.is_some() {
            return Err(CmdError::Params("choose restore or discard".into()));
        }
        if restore.is_some() || discard.is_some() || v.get("refresh").is_some() {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let expected = (self.document_id(), self.rev());
                // Confined automation: only copies of files inside its roots (`RecoveryInfo::allowed`).
                for (id, access) in [(restore, crate::file_access::Access::Read), (discard, crate::file_access::Access::Write)] {
                    if let Some(info) = id.and_then(Value::as_str).and_then(|id| self.recovery_documents().into_iter().find(|d| d.id == id)) {
                        info.allowed(access).map_err(CmdError::Failed)?;
                    }
                }
                let r = self.recovery.as_mut().ok_or_else(|| CmdError::Disabled("local recovery requires the desktop host".into()))?;
                if let Some(id) = restore {
                    // A protected copy's password, as `file.open` takes one.
                    let password = match v.get("password") {
                        None | Some(Value::Null) => None,
                        Some(Value::String(pw)) if !pw.is_empty() => Some(crate::io::Password::new(pw)),
                        Some(_) => return Err(CmdError::Params("`password` must be a non-empty string".into())),
                    };
                    r.restore(id.as_str().ok_or_else(|| CmdError::Params("restore must be a recovery id".into()))?, expected, password)
                        .map_err(CmdError::Failed)?;
                } else if let Some(id) = discard {
                    r.discard(id.as_str().ok_or_else(|| CmdError::Params("discard must be a recovery id".into()))?, expected)
                        .map_err(CmdError::Failed)?;
                } else if v.get("refresh").and_then(Value::as_bool) == Some(true) {
                    r.refresh().map_err(CmdError::Failed)?;
                } else {
                    return Err(CmdError::Params("refresh must be true".into()));
                }
            }
            #[cfg(target_arch = "wasm32")]
            return Err(CmdError::Disabled("local recovery is available on desktop only".into()));
        }
        // Confined automation isn't told where files outside its read root are.
        let mut documents = self.recovery_documents();
        if let Some(roots) = crate::file_access::current() {
            for d in &mut documents {
                if d.path.as_deref().is_some_and(|p| roots.check(p, crate::file_access::Access::Read).is_err()) {
                    d.path = None;
                }
            }
        }
        Ok(json!({
            "enabled": self.recovery_ready(),
            "busy": self.recovery_busy(),
            "documents": documents,
            "error": self.recovery_error(),
            "gap": self.recovery_gap(),
        }))
    }
}
