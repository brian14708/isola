use std::{
    cell::Cell,
    fmt,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeadlineOverflow;

impl fmt::Display for DeadlineOverflow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sleep duration exceeds the monotonic clock range")
    }
}

impl std::error::Error for DeadlineOverflow {}

impl Deadline {
    /// Create a deadline relative to the current instant.
    ///
    /// # Errors
    ///
    /// Returns [`DeadlineOverflow`] when the duration cannot be represented by
    /// the platform monotonic clock.
    pub fn after(duration: Duration) -> Result<Self, DeadlineOverflow> {
        Instant::now()
            .checked_add(duration)
            .map(|ready_at| Self(Some(ready_at)))
            .ok_or(DeadlineOverflow)
    }

    /// Create a deadline from a duration in seconds.
    ///
    /// A non-finite or non-positive duration yields an immediately-ready
    /// deadline.
    ///
    /// # Errors
    ///
    /// Returns [`DeadlineOverflow`] when the duration cannot be represented by
    /// the platform monotonic clock.
    pub fn after_secs_f64(secs: f64) -> Result<Self, DeadlineOverflow> {
        if secs.is_finite() && secs > 0.0 {
            let duration = Duration::try_from_secs_f64(secs).map_err(|_| DeadlineOverflow)?;
            Self::after(duration)
        } else {
            Ok(Self::default())
        }
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    #[test]
    fn rejects_deadlines_outside_the_clock_range() {
        assert!(super::Deadline::after(Duration::MAX).is_err());
    }
}
