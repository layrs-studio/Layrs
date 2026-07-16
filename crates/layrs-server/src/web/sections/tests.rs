#[cfg(test)]
mod tests {
    use super::*;
    include!("tests/unit.rs");
    include!("tests/weaves.rs");
    mod store_objects;
    use store_objects::{
        SyncTestFixture, TerminalWeaveLoser, assert_apply_delete_contention,
        assert_apply_waits_for_paused_publish, assert_terminal_transition_wins,
        create_test_workspace_member_headers, dedicated_test_pool, publish_artifact,
        spawn_weave_apply, test_state, test_tree_id, wait_for_backend_lock,
    };

    mod weave_authz {
        use super::*;
        include!("tests/weave_authz.rs");
        include!("tests/weave_hardening.rs");
    }
}
