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

/// A monotonic deadline that is ready now, at a finite instant, or never.
#[derive(Clone, Copy, Debug, Default)]
pub struct Deadline(DeadlineState);

#[derive(Clone, Copy, Debug, Default)]
enum DeadlineState {
    #[default]
    Ready,
    At(Instant),
    Never,
}

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
    pub(crate) fn after(duration: Duration) -> Result<Self, DeadlineOverflow> {
        Instant::now()
            .checked_add(duration)
            .map(|ready_at| Self(DeadlineState::At(ready_at)))
            .ok_or(DeadlineOverflow)
    }

    /// Create a deadline from a duration in seconds.
    ///
    /// Positive infinity yields a never-ready deadline. Other non-finite and
    /// non-positive durations yield an immediately-ready deadline.
    ///
    /// # Errors
    ///
    /// Returns [`DeadlineOverflow`] when the duration cannot be represented by
    /// the platform monotonic clock.
    pub fn after_secs_f64(secs: f64) -> Result<Self, DeadlineOverflow> {
        if secs == f64::INFINITY {
            Ok(Self(DeadlineState::Never))
        } else if secs.is_finite() && secs > 0.0 {
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
        match self.0 {
            DeadlineState::Ready => true,
            DeadlineState::At(ready_at) => now >= ready_at,
            DeadlineState::Never => false,
        }
    }

    pub(crate) fn wait(self) {
        match self.0 {
            DeadlineState::Ready => {}
            DeadlineState::At(ready_at) => {
                if let Some(remaining) = ready_at.checked_duration_since(Instant::now()) {
                    std::thread::sleep(remaining);
                }
            }
            DeadlineState::Never => loop {
                std::thread::sleep(Duration::from_hours(24));
            },
        }
    }

    #[must_use]
    pub(crate) const fn ready_at(self) -> Option<Instant> {
        match self.0 {
            DeadlineState::At(ready_at) => Some(ready_at),
            DeadlineState::Ready | DeadlineState::Never => None,
        }
    }

    #[must_use]
    pub(crate) const fn is_never(self) -> bool {
        matches!(self.0, DeadlineState::Never)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    #[test]
    fn rejects_deadlines_outside_the_clock_range() {
        assert!(super::Deadline::after(Duration::MAX).is_err());
    }

    #[test]
    fn positive_infinity_is_never_ready() {
        let deadline = super::Deadline::after_secs_f64(f64::INFINITY).unwrap();
        assert!(!deadline.is_ready());
        assert!(deadline.is_never());
        assert_eq!(deadline.ready_at(), None);
    }
}
