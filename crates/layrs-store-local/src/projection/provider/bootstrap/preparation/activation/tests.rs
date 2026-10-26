//! Scoped writer checkpoints used only by audited owned-host recipes.
pub(in crate::projection::provider::bootstrap) mod hooks {
    use super::super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct State {
        stop: Option<(ProviderActivationWriteStep, bool)>,
        busy_close: Option<ProviderActivationSqlPhase>,
    }
    thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
    pub(in crate::projection::provider::bootstrap) fn reset() {
        STATE.with(|state| *state.borrow_mut() = State::default());
    }
    pub(in crate::projection::provider::bootstrap) fn stop(
        step: ProviderActivationWriteStep,
        after: bool,
    ) {
        STATE.with(|state| state.borrow_mut().stop = Some((step, after)));
    }
    pub(in crate::projection::provider::bootstrap) fn busy_close(
        phase: ProviderActivationSqlPhase,
    ) {
        STATE.with(|state| state.borrow_mut().busy_close = Some(phase));
    }
    pub(in crate::projection::provider::bootstrap) fn point(
        step: ProviderActivationWriteStep,
        after: bool,
    ) -> CauseResult<()> {
        if STATE.with(|state| state.borrow().stop == Some((step, after))) {
            return Err(Cause::Io(std::io::Error::other(
                "synthetic activation checkpoint",
            )));
        }
        Ok(())
    }
    pub(in crate::projection::provider::bootstrap) fn before_close(
        raw: *mut rusqlite::ffi::sqlite3,
        phase: ProviderActivationSqlPhase,
    ) -> CauseResult<()> {
        if !STATE.with(|state| state.borrow().busy_close == Some(phase)) {
            return Ok(());
        }
        let mut statement = std::ptr::null_mut();
        let rc = unsafe {
            rusqlite::ffi::sqlite3_prepare_v2(
                raw,
                c"SELECT 1".as_ptr(),
                -1,
                &mut statement,
                std::ptr::null_mut(),
            )
        };
        if rc != rusqlite::ffi::SQLITE_OK || statement.is_null() {
            return Err(invalid("activation BUSY close fixture failed"));
        }
        Ok(())
    }
}
