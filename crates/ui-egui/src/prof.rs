//! Frame-time breakdown for the headless frame benchmark (`tests_frame_perf.rs`). Under `cfg(test)`
//! a [`span`] adds the time until it is dropped to a per-thread total named after it; in every
//! other build it is an empty value and costs nothing.

#[cfg(test)]
thread_local! {
    static TOTALS: std::cell::RefCell<Vec<(&'static str, f64, u32)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A timed section, ended when dropped.
#[cfg(test)]
pub(crate) struct Span(&'static str, std::time::Instant);

/// A timed section (nothing outside tests).
#[cfg(not(test))]
pub(crate) struct Span;

impl Drop for Span {
    #[inline(always)]
    fn drop(&mut self) {
        #[cfg(test)]
        add(self.0, self.1.elapsed().as_secs_f64() * 1e3);
    }
}

/// Start timing the section `name`.
#[inline(always)]
pub(crate) fn span(_name: &'static str) -> Span {
    #[cfg(test)]
    return Span(_name, std::time::Instant::now());
    #[cfg(not(test))]
    Span
}

/// Add `ms` to `name`'s total.
#[cfg(test)]
pub(crate) fn add(name: &'static str, ms: f64) {
    TOTALS.with(|t| {
        let mut t = t.borrow_mut();
        match t.iter_mut().find(|(n, _, _)| *n == name) {
            Some(e) => {
                e.1 += ms;
                e.2 += 1;
            }
            None => t.push((name, ms, 1)),
        }
    });
}

/// The totals since the last call (name, milliseconds, count), and start again.
#[cfg(test)]
pub(crate) fn take() -> Vec<(&'static str, f64, u32)> {
    TOTALS.with(|t| std::mem::take(&mut *t.borrow_mut()))
}
