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
    #[cfg(unix)]
    AfterCasLink,
    #[cfg(unix)]
    BeforeCasDirectorySync,
    #[cfg(unix)]
    AfterCasDirectorySync,
    #[cfg(unix)]
    BeforePendingUnlink,
    #[cfg(unix)]
    AfterPendingUnlink,
    #[cfg(unix)]
    BeforePendingDirectorySync,
    #[cfg(unix)]
    AfterPendingDirectorySync,
    BeforeFsckObjectOpen,
    BeforeMetadataCommit,
    AfterMetadataCommit,
}

#[cfg(test)]
thread_local! {
    static NEXT_FAULT: std::cell::Cell<Option<FaultPoint>> = const { std::cell::Cell::new(None) };
    static NEXT_PAUSE: std::cell::RefCell<Option<Pause>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
struct Pause {
    point: FaultPoint,
    entered: std::sync::Arc<std::sync::Barrier>,
    release: std::sync::Arc<std::sync::Barrier>,
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
        let pause = NEXT_PAUSE.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_some_and(|pause| pause.point == point) {
                slot.take()
            } else {
                None
            }
        });
        if let Some(pause) = pause {
            pause.entered.wait();
            pause.release.wait();
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

#[cfg(test)]
pub(crate) fn pause_once(
    point: FaultPoint,
    entered: std::sync::Arc<std::sync::Barrier>,
    release: std::sync::Arc<std::sync::Barrier>,
) {
    NEXT_PAUSE.with(|slot| {
        *slot.borrow_mut() = Some(Pause {
            point,
            entered,
            release,
        });
    });
}
