//! Shared Worktree response contract for CLI and Desktop.
use super::{NativeClientError, NativeWorkspace, Result};
use layrs_core::scm::LayerId;
use layrs_store::scm::ScmReader;
use layrs_store_local::scm_v1::NativeScmReader;
use serde_json::{Value, json};

/// Names belong to the current View. An exact ID may select an existing Layer
/// from another Worktree's private history, but never from another Space.
pub(super) fn resolve_layer(workspace: &NativeWorkspace, selector: &str) -> Result<LayerId> {
    let Ok(id) = LayerId::parse(selector) else {
        return workspace.layer_named(selector);
    };
    let state = workspace.state()?;
    let store = workspace.open_store()?;
    let reader = NativeScmReader::new(&store)?;
    let layer = reader
        .get_layer(&id)?
        .ok_or(NativeClientError::Missing("Layer in source Space"))?;
    if layer.space_id() != state.space.id() {
        return Err(NativeClientError::Missing("Layer in source Space"));
    }
    Ok(id)
}

pub(super) fn describe(workspace: &NativeWorkspace) -> Result<Value> {
    let state = workspace.state()?;
    Ok(json!({
        "path": state.worktree.checkout_root(),
        "worktreeId": state.worktree.id(),
        "state": state,
    }))
}
