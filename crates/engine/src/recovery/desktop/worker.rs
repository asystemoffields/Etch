//! The worker thread and [`Recovery`], the session-facing side: queueing jobs, pacing periodic
//! copies, polling results, stale copies, and the synchronous emergency copy.
use super::format::new_id;
use super::{DUTY, INTERVAL, Limits, MAX_INTERVAL, Snapshot, Store};
use crate::Session;
use crate::io::Password;
use crate::recovery::{RecoveryGap, RecoveryInfo};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

enum Job {
    Save { snapshot: Box<Snapshot>, password: Option<Password>, deadline: Option<Instant>, limits: Limits },
    Scan,
    Load(String, (u64, u64), Option<Password>),
    Discard(String),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Save,
    Scan,
    Load,
    Discard,
}
enum Outcome {
    /// A copy was written; the pictures and parts it couldn't keep.
    Saved(Vec<String>),
    Done,
    Discarded(String),
    Loaded(Box<Snapshot>, (u64, u64), Option<Password>),
}
struct Event {
    documents: Vec<RecoveryInfo>,
    kind: Kind,
    outcome: Result<Outcome, String>,
}
/// A restored copy, for the session to adopt.
pub(crate) struct Restored {
    pub snapshot: Snapshot,
    pub expected: (u64, u64),
    pub password: Option<Password>,
}
pub(crate) struct Recovery {
    /// `None` once dropped, which ends the worker ([`Drop`]).
    tx: Option<mpsc::SyncSender<Job>>,
    worker: Option<std::thread::JoinHandle<()>>,
    rx: mpsc::Receiver<Event>,
    pub documents: Vec<RecoveryInfo>,
    pub ready: bool,
    pub busy: bool,
    pub error: Option<String>,
    document: Option<u64>,
    id: Option<String>,
    revision: Option<u64>,
    pub(super) due: Instant,
    alive: bool,
    /// When the snapshot under way was queued, to pace the next one ([`DUTY`]).
    pub(super) saving_since: Option<Instant>,
    /// Copies of documents that were saved and then replaced before their copy was discarded:
    /// removed when the worker is free, so they aren't offered after the next start.
    stale: Vec<String>,
    pub(crate) limits: Limits,
    /// What the current document's last copy couldn't cover (`None`: all of it).
    gap: Option<RecoveryGap>,
    /// The protected copy being restored, and one whose password was wrong (to ask again).
    loading: Option<String>,
    wrong_password: Option<String>,
    /// The document whose copy is being written.
    saving_for: Option<u64>,
}
impl Recovery {
    pub fn start(root: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let (tx, jobs) = mpsc::sync_channel(1);
        let (events, rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("wordcraft-recovery".into())
            .spawn(move || {
                let mut store = match Store::new(root) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = events.send(Event { documents: Vec::new(), kind: Kind::Scan, outcome: Err(e) });
                        wake();
                        return;
                    }
                };
                let initial = store.scan().map(|_| Outcome::Done);
                if events.send(Event { documents: store.inventory(), kind: Kind::Scan, outcome: initial }).is_err() {
                    return;
                }
                wake();
                while let Ok(job) = jobs.recv() {
                    let (kind, result) = match job {
                        Job::Save { snapshot, password, deadline, limits } => {
                            (Kind::Save, store.save(*snapshot, password.as_ref(), deadline, limits).map(Outcome::Saved))
                        }
                        Job::Scan => (Kind::Scan, store.scan().map(|_| Outcome::Done)),
                        Job::Load(id, expected, password) => {
                            (Kind::Load, store.load(&id, password.as_ref()).map(|s| Outcome::Loaded(Box::new(s), expected, password)))
                        }
                        Job::Discard(id) => (Kind::Discard, store.discard(&id).map(|_| Outcome::Discarded(id))),
                    };
                    if events.send(Event { documents: store.inventory(), kind, outcome: result }).is_err() {
                        break;
                    }
                    wake();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx: Some(tx),
            worker: Some(worker),
            rx,
            documents: Vec::new(),
            ready: false,
            busy: true,
            error: None,
            document: None,
            id: None,
            revision: None,
            due: Instant::now() + INTERVAL,
            alive: true,
            saving_since: None,
            stale: Vec::new(),
            limits: Limits::default(),
            gap: None,
            loading: None,
            wrong_password: None,
            saving_for: None,
        })
    }
    fn queue(&mut self, job: Job) -> Result<(), String> {
        if !self.alive {
            return Err("recovery worker is unavailable".into());
        }
        if self.busy {
            return Err("recovery is busy; try again when it finishes".into());
        }
        self.tx.as_ref().ok_or("recovery worker is unavailable")?.try_send(job).map_err(|e| e.to_string())?;
        self.busy = true;
        self.error = None;
        Ok(())
    }
    /// Note what a save says about the current document's coverage (not one of a document
    /// replaced while it was being copied).
    fn saved(&mut self, kind: Kind, outcome: &Result<Outcome, String>) {
        if kind == Kind::Save && self.saving_for.take() != self.document {
            return;
        }
        match (kind, outcome) {
            (Kind::Save, Ok(Outcome::Saved(omitted))) => {
                self.gap = (!omitted.is_empty()).then(|| RecoveryGap::Omitted(omitted.clone()));
            }
            (Kind::Save, Err(e)) => self.gap = Some(RecoveryGap::Failed(e.clone())),
            _ => {}
        }
    }
    pub fn poll(&mut self) -> Option<Restored> {
        match self.rx.try_recv() {
            Ok(e) => {
                self.busy = false;
                self.ready = true;
                self.documents = e.documents;
                if let Some(since) = self.saving_since.take() {
                    let pace = since.elapsed().saturating_mul(DUTY).min(MAX_INTERVAL);
                    self.due = self.due.max(Instant::now() + pace);
                }
                self.saved(e.kind, &e.outcome);
                if e.kind == Kind::Load {
                    let loading = self.loading.take();
                    if let Err(err) = &e.outcome
                        && crate::io::wrong_password(err)
                    {
                        self.wrong_password = loading;
                    }
                }
                match e.outcome {
                    Ok(Outcome::Done | Outcome::Saved(_)) => None,
                    Ok(Outcome::Discarded(id)) => {
                        if self.id.as_deref() == Some(&id) {
                            self.id = None;
                        }
                        None
                    }
                    Ok(Outcome::Loaded(snapshot, expected, password)) => Some(Restored { snapshot: *snapshot, expected, password }),
                    Err(err) => {
                        self.error = Some(err);
                        if e.kind == Kind::Save {
                            self.revision = None;
                        }
                        self.due = Instant::now() + INTERVAL;
                        None
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.busy = false;
                self.alive = false;
                if self.error.is_none() {
                    self.error = Some("recovery worker is unavailable".into());
                }
                None
            }
        }
    }
    pub fn adopt(&mut self, id: String, document: u64, revision: u64) {
        self.id = Some(id);
        self.document = Some(document);
        self.revision = Some(revision);
        self.gap = None;
        self.due = Instant::now() + INTERVAL;
    }
    /// The job that copies `s` now, into its record.
    fn save_job(&mut self, s: &Session, deadline: Option<Instant>) -> Result<(Job, String), String> {
        let id = match &self.id {
            Some(id) => id.clone(),
            None => new_id()?,
        };
        let name = s.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled draft".into());
        let info = RecoveryInfo {
            id: id.clone(),
            name,
            saved_at: crate::cmd::now_iso(),
            path: s.path.as_ref().and_then(|p| p.to_str()).map(str::to_string),
            protected: s.password.is_some(),
            omitted: Vec::new(),
            earlier: false,
            older_than_file: false,
        };
        let job = Job::Save { snapshot: Box::new(Snapshot::new(info, s.doc.clone())), password: s.password.clone(), deadline, limits: self.limits };
        Ok((job, id))
    }
    fn follow(&mut self, s: &Session) {
        if self.document != Some(s.document_id()) {
            self.document = Some(s.document_id());
            self.id = None;
            self.revision = None;
            self.gap = None;
        }
    }
    pub fn tick(&mut self, s: &Session, now: Instant) -> Result<(), String> {
        self.follow(s);
        if self.busy || !self.ready || !self.alive {
            return Ok(());
        }
        if let Some(id) = self.stale.pop() {
            return self.queue(Job::Discard(id));
        }
        if !s.dirty {
            self.gap = None;
            if let Some(id) = self.id.clone() {
                self.queue(Job::Discard(id))?;
            }
            return Ok(());
        }
        if self.revision == Some(s.rev()) || now < self.due {
            return Ok(());
        }
        let (job, id) = self.save_job(s, None)?;
        self.queue(job)?;
        self.saving_for = self.document;
        self.saving_since = Some(now);
        self.id = Some(id);
        self.revision = Some(s.rev());
        self.due = now + INTERVAL;
        Ok(())
    }
    pub fn document_is(&self, document: u64) -> bool {
        self.document == Some(document)
    }
    /// What the current document's last copy couldn't cover.
    pub fn gap(&self) -> Option<&RecoveryGap> {
        self.gap.as_ref()
    }
    pub fn alive(&self) -> bool {
        self.alive
    }
    /// The protected copy whose password was wrong, once: the front end asks again.
    pub fn take_wrong_password(&mut self) -> Option<String> {
        self.wrong_password.take()
    }
    /// The document `document` is being replaced after it was saved: its copy is out of date.
    pub fn replaced_clean(&mut self, document: u64) {
        if self.document == Some(document)
            && let Some(id) = self.id.take()
        {
            self.stale.push(id);
            self.revision = None;
        }
    }
    pub fn wait(&self, document: u64, revision: u64) -> Option<Duration> {
        if self.alive && self.ready && !self.busy && !self.stale.is_empty() {
            return Some(Duration::from_millis(100));
        }
        if !self.alive || !self.ready || self.busy || (self.document == Some(document) && self.revision == Some(revision)) {
            return None;
        }
        Some(self.due.saturating_duration_since(Instant::now()).max(Duration::from_millis(100)))
    }
    pub fn restore(&mut self, id: &str, expected: (u64, u64), password: Option<Password>) -> Result<(), String> {
        let Some(info) = self.documents.iter().find(|d| d.id == id) else {
            return Err("no such recovery copy".into());
        };
        if info.protected && password.is_none() {
            return Err(format!("{}: enter it to restore this recovery copy", wordcraft_docx::PASSWORD_REQUIRED));
        }
        self.queue(Job::Load(id.into(), expected, password))?;
        self.loading = Some(id.into());
        self.wrong_password = None;
        Ok(())
    }
    pub fn discard(&mut self, id: &str, current: (u64, u64)) -> Result<(), String> {
        if !self.documents.iter().any(|d| d.id == id) {
            return Err("no such recovery copy".into());
        }
        self.queue(Job::Discard(id.into()))?;
        if self.id.as_deref() == Some(id) {
            self.revision = Some(current.1);
        }
        Ok(())
    }
    pub fn has_stale(&self) -> bool {
        !self.stale.is_empty()
    }
    pub fn refresh(&mut self) -> Result<(), String> {
        self.queue(Job::Scan)
    }
    pub fn purge(&mut self) -> Result<(), String> {
        let Some(id) = self.id.clone() else { return Ok(()) };
        self.discard_now(id)?;
        self.id = None;
        self.revision = None;
        self.gap = None;
        Ok(())
    }
    /// Remove the copies of replaced saved documents now ([`Self::replaced_clean`]), before the
    /// app exits.
    pub fn purge_stale(&mut self) {
        while let Some(id) = self.stale.pop() {
            if let Err(e) = self.discard_now(id) {
                log::warn!("recovery: cannot discard an out-of-date copy: {e}");
            }
        }
    }
    /// Discard the copy `id`, waiting for the worker.
    fn discard_now(&mut self, id: String) -> Result<(), String> {
        if self.busy {
            let e = self.rx.recv().map_err(|e| e.to_string())?;
            self.documents = e.documents;
            self.busy = false;
            self.ready = true;
            self.saving_since = None;
            if matches!(e.outcome, Ok(Outcome::Loaded(..))) {
                return Err("finish recovery before protecting this document".into());
            }
        }
        self.queue(Job::Discard(id))?;
        let e = self.rx.recv().map_err(|e| e.to_string())?;
        self.busy = false;
        self.documents = e.documents;
        e.outcome.map(|_| ())
    }
    /// Write a copy of `s`'s unsaved changes now and wait for it, at most `timeout`: the app is
    /// being terminated (a signal, the session ending) or has panicked, and the periodic copy can
    /// be [`INTERVAL`] old. A protected document's copy is encrypted; pictures not yet on disk
    /// are written only while there is time left for the text. `Ok(true)` once an up-to-date
    /// copy is on disk; `Ok(false)` when there is nothing to keep (saved).
    pub fn snapshot_now(&mut self, s: &Session, timeout: Duration) -> Result<bool, String> {
        if !s.dirty {
            return Ok(false);
        }
        let deadline = Instant::now() + timeout;
        let wait = |r: &mut Self| -> Result<Event, String> {
            r.rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).map_err(|e| format!("recovery copy not written: {e}"))
        };
        // The job under way (the start-up scan, a periodic copy…) finishes first.
        if self.busy {
            let e = wait(self)?;
            self.busy = false;
            self.ready = true;
            self.saving_since = None;
            self.documents = e.documents;
            self.saved(e.kind, &e.outcome);
            if e.outcome.is_err() && e.kind == Kind::Save {
                self.revision = None;
            }
        }
        self.follow(s);
        if self.revision == Some(s.rev()) {
            return Ok(true);
        }
        let (job, id) = self.save_job(s, Some(deadline))?;
        self.queue(job)?;
        self.saving_for = self.document;
        self.id = Some(id);
        let e = wait(self)?;
        self.busy = false;
        self.documents = e.documents;
        self.saved(e.kind, &e.outcome);
        if let Ok(Outcome::Saved(omitted)) = &e.outcome
            && !omitted.is_empty()
        {
            log::warn!("recovery: the emergency copy left out {} pictures or parts", omitted.len());
        }
        e.outcome?;
        self.revision = Some(s.rev());
        Ok(true)
    }
}

/// Let the worker finish the job under way (a snapshot completes rather than being cut off at a
/// normal exit) and release its record locks before the session is gone, so a new session (the
/// next start) can list those copies.
impl Drop for Recovery {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            log::warn!("recovery: the worker stopped abnormally");
        }
    }
}
