//! Deterministic owned-test interference only; absent from production.
use super::error::Result;
use std::cell::RefCell;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::projection::provider) enum Point {
    AfterFamilyCompletion,
    BeforeConcludingBoundary,
}
type Hook = Box<dyn FnMut(Point) -> Result<()>>;
thread_local! { static HOOK:RefCell<Option<Hook>>=RefCell::new(None); }
pub(in crate::projection::provider) fn at(point: Point) -> Result<()> {
    HOOK.with(|slot| match slot.borrow_mut().as_mut() {
        Some(h) => h(point),
        None => Ok(()),
    })
}
pub(in crate::projection::provider) fn with<T>(
    hook: impl FnMut(Point) -> Result<()> + 'static,
    action: impl FnOnce() -> T,
) -> T {
    HOOK.with(|slot| {
        assert!(
            slot.borrow_mut().replace(Box::new(hook)).is_none(),
            "no nested test hook"
        )
    });
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            HOOK.with(|slot| {
                slot.borrow_mut().take();
            });
        }
    }
    let _reset = Reset;
    action()
}
