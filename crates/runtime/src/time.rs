use std::{
    cell::Cell,
    time::{Duration, Instant},
};

thread_local! {
    static MONOTONIC_BASE: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Return seconds elapsed since the first call on this runtime thread.
#[must_use]
pub fn monotonic() -> f64 {
    MONOTONIC_BASE.with(|base| {
        let started_at = base.get().unwrap_or_else(|| {
            let now = Instant::now();
            base.set(Some(now));
            now
        });
        started_at.elapsed().as_secs_f64()
    })
}

pub fn reset_monotonic() {
    MONOTONIC_BASE.with(|base| base.set(None));
}

/// A monotonic deadline, or an immediately-ready state when no deadline exists.
#[derive(Clone, Copy, Debug, Default)]
pub struct Deadline(Option<Instant>);

impl Deadline {
    #[must_use]
    pub fn after(duration: Duration) -> Self {
        Self(Instant::now().checked_add(duration))
    }

    #[must_use]
    pub fn is_ready(self) -> bool {
        self.is_ready_at(Instant::now())
    }

    pub(crate) fn is_ready_at(self, now: Instant) -> bool {
        self.0.is_none_or(|ready_at| now >= ready_at)
    }

    pub fn wait(self) {
        if let Some(ready_at) = self.0
            && let Some(remaining) = ready_at.checked_duration_since(Instant::now())
        {
            std::thread::sleep(remaining);
        }
    }

    #[must_use]
    pub const fn ready_at(self) -> Option<Instant> {
        self.0
    }

    pub const fn clear(&mut self) {
        self.0 = None;
    }
}
