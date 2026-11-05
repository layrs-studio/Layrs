//! Test-only short-write/error sink at the fresh checkout's real Owned copy.
//! It never creates or edits a file outside the driver's actual Write calls.
use std::{
    cell::{Cell, RefCell},
    fmt,
    io::{self, Write},
    rc::Rc,
};

thread_local! {
    static ACTIVE: RefCell<Option<Rc<State>>> = const { RefCell::new(None) };
}

struct State {
    limit: usize,
    written: Cell<usize>,
    errors: Cell<usize>,
}

#[derive(Debug)]
pub(in crate::projection::provider) struct InjectedCopyError;
impl fmt::Display for InjectedCopyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("fresh checkout copy failed after actual prefix write")
    }
}
impl std::error::Error for InjectedCopyError {}

pub(in crate::projection::provider) struct Observed {
    pub written: usize,
    pub errors: usize,
}

/// The scope cannot nest and is removed even during unwinding. A disabled
/// observer passes through the exact original sink, including its errors.
pub(in crate::projection::provider) fn with_prefix_failure<T>(
    bytes: usize,
    action: impl FnOnce() -> T,
) -> (T, Observed) {
    assert!(bytes > 0);
    let state = Rc::new(State {
        limit: bytes,
        written: Cell::new(0),
        errors: Cell::new(0),
    });
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        assert!(active.is_none(), "copy fault scopes cannot nest");
        *active = Some(state.clone());
    });
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ACTIVE.with(|active| *active.borrow_mut() = None);
        }
    }
    let _reset = Reset;
    let result = action();
    (
        result,
        Observed {
            written: state.written.get(),
            errors: state.errors.get(),
        },
    )
}

pub(super) struct Output<'a> {
    inner: &'a mut dyn Write,
    state: Option<Rc<State>>,
}
impl<'a> Output<'a> {
    pub(super) fn new(inner: &'a mut dyn Write) -> Self {
        Self {
            inner,
            state: ACTIVE.with(|active| active.borrow().clone()),
        }
    }
}
impl Write for Output<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Some(state) = &self.state else {
            return self.inner.write(bytes);
        };
        if bytes.is_empty() {
            return Ok(0);
        }
        let left = state
            .limit
            .checked_sub(state.written.get())
            .expect("bounded copy probe");
        if left == 0 {
            state.errors.set(state.errors.get() + 1);
            return Err(io::Error::new(io::ErrorKind::Other, InjectedCopyError));
        }
        let count = self.inner.write(&bytes[..bytes.len().min(left)])?;
        state.written.set(state.written.get() + count);
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
