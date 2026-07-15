//! Test-only, one-shot failures at durable write boundaries.

use std::io;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FaultPoint {
    BeforeTempSync,
    AfterTempSync,
    BeforeTempHash,
    BeforeRename,
    AfterRename,
    BeforeDirectorySync,
    BeforeMetadataCommit,
    AfterMetadataCommit,
}

#[cfg(test)]
thread_local! {
    static NEXT_FAULT: std::cell::Cell<Option<FaultPoint>> = const { std::cell::Cell::new(None) };
}

pub(crate) fn hit(point: FaultPoint) -> io::Result<()> {
    #[cfg(test)]
    {
        let should_fail = NEXT_FAULT.with(|slot| {
            if slot.get() == Some(point) {
                slot.set(None);
                true
            } else {
                false
            }
        });
        if should_fail {
            return Err(io::Error::other(format!("injected fault at {point:?}")));
        }
    }

    #[cfg(not(test))]
    let _ = point;

    Ok(())
}

#[cfg(test)]
pub(crate) fn fail_once(point: FaultPoint) {
    NEXT_FAULT.with(|slot| slot.set(Some(point)));
}
