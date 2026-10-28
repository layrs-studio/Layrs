//! Owned-recipe observation only. No SQLite/VFS registration or process-global
//! state. The callback runs before a generic RO snapshot or source identity
//! probe, so a refused boundary performs none of that probe's filesystem/SQL.
use crate::projection::provider::recovery::error::Result;
use std::{cell::RefCell, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) enum ReadBoundary {
    BeforeSnapshot,
    DatabaseSnapshot,
    DatabaseIdentity,
    OrdinarySourceIdentity,
}

type Observer = Box<dyn FnMut(&Path, ReadBoundary) -> Result<()>>;
thread_local! {
    static OBSERVER: RefCell<Option<Observer>> = RefCell::new(None);
}

pub(in crate::projection::provider) fn probe(path: &Path, point: ReadBoundary) -> Result<()> {
    OBSERVER.with(|slot| match slot.borrow_mut().as_mut() {
        Some(observer) => observer(path, point),
        None => Ok(()),
    })
}

pub(in crate::projection::provider) fn with_observer<T>(
    observer: impl FnMut(&Path, ReadBoundary) -> Result<()> + 'static,
    action: impl FnOnce() -> T,
) -> T {
    OBSERVER.with(|slot| {
        let mut slot = slot.borrow_mut();
        assert!(slot.is_none(), "fresh Ordinary observer cannot be nested");
        *slot = Some(Box::new(observer));
    });
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            OBSERVER.with(|slot| *slot.borrow_mut() = None);
        }
    }
    let _reset = Reset;
    action()
}
