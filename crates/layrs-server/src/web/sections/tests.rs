#[cfg(test)]
mod tests {
    use super::*;
    include!("tests/unit.rs");
    include!("tests/weaves.rs");
    mod store_objects;
    use store_objects::{
        create_test_workspace_member_headers, publish_artifact, test_state, test_tree_id,
        SyncTestFixture,
    };

    mod weave_authz {
        use super::*;
        include!("tests/weave_authz.rs");
    }
}
