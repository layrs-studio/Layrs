#[path = "weaves/apply.rs"]
mod weave_apply;
#[path = "weaves/operations.rs"]
mod weave_operations;
#[path = "weaves/propagation.rs"]
mod weave_propagation;
#[path = "weaves/reconcile.rs"]
mod weave_reconcile;
#[path = "weaves/resolution.rs"]
mod weave_resolution;
#[path = "weaves/session.rs"]
mod weave_session;
#[path = "weaves/state.rs"]
mod weave_state;

pub use self::weave_operations::{
    abort_weave, continue_weave, resolve_weave_conflict, weave_conflicts, weave_layers,
    weave_status,
};

use self::weave_apply::unique_weave_id;
use self::weave_operations::apply_completed_sync_weave;
use self::weave_propagation::{
    inherit_parent_steps, propagate_step_to_linked_children,
};
use self::weave_reconcile::{
    apply_conflict_marker_to_state, reconcile_path_with_lens, PathReconcileOutcome,
};
use self::weave_session::{
    active_weave_id, proposed_state_path, read_weave_session, write_active_weave_marker,
    write_sync_weave_plan_guard, write_weave_session,
};
use self::weave_state::{apply_file_choice, is_safe_take_theirs, sorted_steps};
