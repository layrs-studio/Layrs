use super::*;
use serde_json::json;
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> (Self, ProjectionStore) {
        let parent = std::env::current_dir().unwrap().join("target");
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join(format!("layrs-directory-client-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        let hash = |n: u8| format!("blake3:{n:064x}");
        let identity: ProjectionIdentity = serde_json::from_value(json!({
            "storeId": "store:directory-client", "worktreeId": "worktree:directory-client",
            "spaceId": "space:directory-client", "origin": "https://example.test",
            "principalId": "principal:alice"
        }))
        .unwrap();
        let snapshot = serde_json::from_value(json!({
            "schemaVersion": "layrs.projection-snapshot.v1", "baseStepId": hash(1),
            "baseRootTreeId": hash(2), "observedPolicySetId": hash(3),
            "observedAuthorizationEpoch": 7, "pathSemantics": "portable_windows",
            "entries": [
                {"path": "private", "target": {"kind":"file", "fileId":hash(4)}, "executable":false, "writable":false, "state":"reserved"},
                {"path": "src/a.txt", "target":null, "executable":false, "writable":true, "state":"absent"}
            ]
        })).unwrap();
        let store = ProjectionStore::create(&path, identity, snapshot).unwrap();
        fs::create_dir(path.join("src")).unwrap();
        fs::write(path.join("src/a.txt"), b"alpha").unwrap();
        (Self(path), store)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let parent = std::env::current_dir().unwrap().join("target");
        if !std::thread::panicking() && self.0.parent() == Some(parent.as_path()) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn path(value: &str) -> RepoPath {
    RepoPath::parse(value).unwrap()
}
fn access(
    store: &ProjectionStore,
    state: &VersionedProjection,
    paths: &MovePaths,
) -> ProjectionAccess {
    let entries: Vec<_> = paths
        .all
        .iter()
        .map(|path| {
            json!({
                "path": path, "target":null, "executable":false,
                "contentReadable":true, "writable":true
            })
        })
        .collect();
    serde_json::from_value(json!({
        "schemaVersion":"layrs.projection-access.v1", "principalId":store.identity().principal_id,
        "spaceId":store.identity().space_id, "stepId":state.snapshot.base_step_id,
        "rootTreeId":state.snapshot.base_root_tree_id,
        "policySetId":state.snapshot.observed_policy_set_id,
        "authorizationEpoch":state.snapshot.observed_authorization_epoch,
        "entries":entries
    }))
    .unwrap()
}

#[test]
fn reserved_parent_is_refused_before_any_classification() {
    let (_fixture, store) = Fixture::new();
    let state = store.state().unwrap();
    assert!(move_paths(&store, &state, &path("src"), &path("private/child")).is_err());
}

#[test]
fn exact_source_and_absent_destination_are_required_in_every_batch() {
    let (_fixture, store) = Fixture::new();
    let state = store.state().unwrap();
    let paths = move_paths(&store, &state, &path("src"), &path("dst")).unwrap();
    assert_eq!(paths.all, vec![path("dst/a.txt"), path("src/a.txt")]);
    let original = access(&store, &state, &paths);
    check_authorizations(store.identity(), &state, &paths, &[original.clone()]).unwrap();
    let mut first = original.clone();
    first.entries.truncate(1);
    let mut second = original.clone();
    second.entries.remove(0);
    check_authorizations(
        store.identity(),
        &state,
        &paths,
        &[first.clone(), second.clone()],
    )
    .unwrap();
    second.entries[0].content_readable = false;
    assert!(check_authorizations(store.identity(), &state, &paths, &[first, second]).is_err());
    let mut denied = original.clone();
    denied.entries[0].writable = false;
    assert!(check_authorizations(store.identity(), &state, &paths, &[denied]).is_err());
    let mut present = original.clone();
    present.entries[0].target =
        Nullable::some(TreeEntryTarget::file(FileId::from_bytes(b"occupied")));
    assert!(check_authorizations(store.identity(), &state, &paths, &[present]).is_err());
    let mut changed = original.clone();
    changed.authorization_epoch += 1;
    assert!(check_authorizations(store.identity(), &state, &paths, &[changed]).is_err());
    let mut missing = original;
    missing.entries.pop();
    assert!(check_authorizations(store.identity(), &state, &paths, &[missing]).is_err());
}

#[test]
fn unknown_source_file_never_becomes_a_move_candidate() {
    let (fixture, store) = Fixture::new();
    fs::write(fixture.0.join("src/unknown.txt"), b"unknown").unwrap();
    let state = store.state().unwrap();
    let paths = move_paths(&store, &state, &path("src"), &path("dst")).unwrap();
    assert_eq!(paths.sources, BTreeSet::from([path("src/a.txt")]));
    let observed = store
        .observe_absent_paths(
            state.generation,
            &state.digest,
            &paths.destinations.into_iter().collect::<Vec<_>>(),
        )
        .unwrap();
    assert!(store
        .preview_directory_move(observed.generation, &path("src"), &path("dst"))
        .is_err());
    assert_eq!(
        fs::read(fixture.0.join("src/unknown.txt")).unwrap(),
        b"unknown"
    );
    assert!(!fixture.0.join("dst").exists());
}
