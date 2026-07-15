#[cfg(test)]
mod tests {
    use super::*;
    include!("tests/unit.rs");
    include!("tests/weaves.rs");
    mod store_objects;
    use store_objects::{
        SyncTestFixture, create_test_workspace_member_headers, publish_artifact, test_state,
        test_tree_id,
    };

    mod weave_authz {
        use super::*;
        include!("tests/weave_authz.rs");
        include!("tests/weave_hardening.rs");
    }
}
