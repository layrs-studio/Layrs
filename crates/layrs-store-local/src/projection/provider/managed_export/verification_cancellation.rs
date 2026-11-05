//! Scoped observer for the owned native recipe, after the real lease has read
//! and verified a chunk but before the cancellation-aware sink accepts it.
use layrs_vfs::Cancellation;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

struct Armed {
    cancel: Cancellation,
    fired: Rc<Cell<usize>>,
}
thread_local! {
    static ARMED: RefCell<Option<Armed>> = const { RefCell::new(None) };
}

pub(in crate::projection::provider) struct Guard(Rc<Cell<usize>>);

pub(in crate::projection::provider) fn arm(cancel: &Cancellation) -> Guard {
    let fired = Rc::new(Cell::new(0));
    ARMED.with(|slot| {
        let mut value = slot.borrow_mut();
        assert!(value.is_none(), "nested verification cancellation observer");
        *value = Some(Armed {
            cancel: cancel.clone(),
            fired: fired.clone(),
        });
    });
    Guard(fired)
}

impl Guard {
    pub(in crate::projection::provider) fn fired(&self) -> usize {
        self.0.get()
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        ARMED.with(|slot| {
            slot.borrow_mut().take();
        });
    }
}

pub(super) fn at_verified_chunk() {
    let armed = ARMED.with(|slot| slot.borrow_mut().take());
    if let Some(armed) = armed {
        armed.fired.set(armed.fired.get() + 1);
        armed.cancel.cancel();
    }
}
