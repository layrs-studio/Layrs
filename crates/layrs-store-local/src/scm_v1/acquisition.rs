//! Bounded Store acquisition only. No prepared batch is retried by this module.
use super::{NativeStoreError, NativeStoreOpen};
use std::{
    fmt, io,
    path::Path,
    time::{Duration, Instant},
};

pub(super) const ORDINARY_WAIT: Duration = Duration::from_secs(60);
pub(super) const SHORT_WAIT: Duration = Duration::from_secs(5);
const INITIAL_BACKOFF: Duration = Duration::from_millis(2);
const MAX_BACKOFF: Duration = Duration::from_millis(50);

/// Ordinary facade reads may wait for a legitimate full acceptance. Each raw
/// open attempts full format/boundary validation; schema checks run once the
/// lock is acquired. Callers check their expected binding tokens on that
/// acquired capability, including Store ID and root/metadata identities.
pub fn open_for_read_with_retry(
    root: impl AsRef<Path>,
) -> Result<NativeStoreOpen, NativeStoreError> {
    acquire("ordinary Store read", ORDINARY_WAIT, || {
        super::open(root.as_ref())
    })
}

/// Registration already holds the namespace gate, so its read must retain the
/// fixed short acquisition budget. Expected bindings remain the caller's check.
pub fn open_for_registration_with_retry(
    root: impl AsRef<Path>,
) -> Result<NativeStoreOpen, NativeStoreError> {
    acquire("registration Store read", SHORT_WAIT, || {
        super::open(root.as_ref())
    })
}

#[derive(Debug)]
struct AcquisitionTimeout {
    stage: &'static str,
    budget: Duration,
    elapsed: Duration,
    attempts: u64,
    contention: NativeStoreError,
}

impl fmt::Display for AcquisitionTimeout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Store acquisition timed out at {}; budget={}ms, elapsed={}ms, attempts={}: {}",
            self.stage,
            self.budget.as_millis(),
            self.elapsed.as_millis(),
            self.attempts,
            self.contention
        )
    }
}
impl std::error::Error for AcquisitionTimeout {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.contention)
    }
}

pub(super) fn acquire<T>(
    stage: &'static str,
    budget: Duration,
    mut attempt: impl FnMut() -> Result<T, NativeStoreError>,
) -> Result<T, NativeStoreError> {
    #[cfg(test)]
    let budget = tests::budget_for_test(stage, budget);
    let started = Instant::now();
    let mut attempts = 0;
    let mut backoff = INITIAL_BACKOFF;
    loop {
        attempts += 1;
        let contention = match attempt() {
            Ok(value) => return Ok(value),
            Err(
                error @ NativeStoreError::Lock(
                    crate::StoreError::LockBusy(_) | crate::StoreError::ReentrantLock(_),
                ),
            ) => error,
            Err(error) => return Err(error),
        };
        let elapsed = started.elapsed();
        if elapsed < budget {
            // A failed attempt owns no Store capability while it sleeps.
            std::thread::sleep(backoff.min(budget - elapsed));
        }
        let elapsed = started.elapsed();
        if elapsed >= budget {
            return Err(NativeStoreError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                AcquisitionTimeout {
                    stage,
                    budget,
                    elapsed,
                    attempts,
                    contention,
                },
            )));
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

#[cfg(test)]
#[path = "acquisition_tests.rs"]
mod tests;
