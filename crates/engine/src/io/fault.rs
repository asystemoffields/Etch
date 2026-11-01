//! Test seam (test builds only): make the Nth run of one durable step of a write fail, as a full
//! disk, a read-only folder or a failing drive would, or let another program act at that moment
//! (replace the file being saved). Saves (`safe_save`) and recovery copies (`recovery/desktop.rs`)
//! call [`at`] and [`write_all`] at each step; a plan applies only to files under its folder, so
//! tests running side by side (each in a folder of its own) don't see each other's faults.
//! Nothing of this is compiled outside `cfg(test)`.
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A durable step of a write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Creating the temporary file.
    Create,
    /// Writing its bytes.
    Write,
    /// Synchronizing it to disk.
    SyncFile,
    /// Reading it back (recovery copies compare what they wrote).
    ReadBack,
    /// Checking that the destination is still the version the session expects (saves).
    Verify,
    /// Renaming it over the destination.
    Rename,
    /// Synchronizing the folder, so the new name is on disk.
    SyncDir,
}

/// What a planned step does.
pub(crate) enum Act {
    /// Fails with this kind of error (a write writes nothing).
    Fail(io::ErrorKind),
    /// A write writes the first half of its bytes, then fails like this (a disk filling up).
    Partial(io::ErrorKind),
    /// Runs this first (another program acting at that moment), then the step goes ahead.
    Run(Box<dyn FnOnce() + Send>),
}

struct Plan {
    under: PathBuf,
    step: Step,
    /// Runs of the step still to let through before acting.
    skip: usize,
    act: Option<Act>,
}

static PLANS: Mutex<Vec<Plan>> = Mutex::new(Vec::new());

fn plans() -> std::sync::MutexGuard<'static, Vec<Plan>> {
    PLANS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The `nth` (from 1) run of `step` for a file under `under` does `act`, once.
pub(crate) fn inject(under: &Path, step: Step, nth: usize, act: Act) {
    plans().push(Plan { under: under.to_path_buf(), step, skip: nth.saturating_sub(1), act: Some(act) });
}

/// Forget every plan for files under `under` (whether it acted or not).
pub(crate) fn clear(under: &Path) {
    plans().retain(|p| p.under != under);
}

/// Whether every plan for files under `under` has acted.
pub(crate) fn spent(under: &Path) -> bool {
    plans().iter().filter(|p| p.under == under).all(|p| p.act.is_none())
}

/// The act planned for this run of `step` on `path`, if one is due.
fn due(path: &Path, step: Step) -> Option<Act> {
    let mut plans = plans();
    let plan = plans.iter_mut().find(|p| p.step == step && p.act.is_some() && path.starts_with(&p.under))?;
    if plan.skip > 0 {
        plan.skip -= 1;
        return None;
    }
    plan.act.take()
}

fn injected(kind: io::ErrorKind, step: Step) -> io::Error {
    io::Error::new(kind, format!("injected {kind} at {step:?}"))
}

/// Whether `e` is a failure a plan injected at `step` (not one from the file system).
pub(crate) fn is_injected(e: &io::Error, step: Step) -> bool {
    e.get_ref().is_some_and(|inner| inner.to_string().ends_with(&format!(" at {step:?}")) && inner.to_string().starts_with("injected "))
}

/// Run `step` for `path`: an error when a plan fails it here.
pub(crate) fn at(path: &Path, step: Step) -> io::Result<()> {
    match due(path, step) {
        None => Ok(()),
        Some(Act::Fail(kind) | Act::Partial(kind)) => Err(injected(kind, step)),
        Some(Act::Run(f)) => {
            f();
            Ok(())
        }
    }
}

/// `file.write_all(bytes)` for `path`, unless a plan fails the write (after writing half of the
/// bytes, for [`Act::Partial`]).
pub(crate) fn write_all(path: &Path, file: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    match due(path, Step::Write) {
        None => file.write_all(bytes),
        Some(Act::Fail(kind)) => Err(injected(kind, Step::Write)),
        Some(Act::Partial(kind)) => {
            file.write_all(bytes.get(..bytes.len() / 2).unwrap_or_default())?;
            Err(injected(kind, Step::Write))
        }
        Some(Act::Run(f)) => {
            f();
            file.write_all(bytes)
        }
    }
}
