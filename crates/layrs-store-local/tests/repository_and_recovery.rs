use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use layrs_store_local::{
    JournalOperation, JournalPath, JournalPathInput, JournalState, LayerRecord, LocalStore,
    LocalStoreConfig, NewJournalOperation, ObjectRecord, OperationKind, PendingPublishRecord,
    RecoveryAction, RecoveryHandler, RefUpdate, SessionMode, SpaceRecord, StepPathRecord,
    StepRecord, StoreError, StoreResult, recover_operations,
};

#[test]
fn refs_and_cursors_use_generation_compare_and_swap() {
    let root = unique_test_dir("generation");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("initialize store");
    let (object_id, relative_path) = write_fixture_object(&store, b"root tree");
    let mut session = store
        .session(SessionMode::Exclusive)
        .expect("exclusive session");
    seed_space_layer_step(session.repository_mut(), &root, &object_id, &relative_path);

    let first = session
        .repository_mut()
        .compare_and_swap_ref(&ref_update(0, 10))
        .expect("create ref");
    assert_eq!(first.generation, 1);
    assert!(matches!(
        session
            .repository_mut()
            .compare_and_swap_ref(&ref_update(0, 11)),
        Err(StoreError::GenerationConflict {
            expected: 0,
            actual: Some(1),
            ..
        })
    ));
    let second = session
        .repository_mut()
        .compare_and_swap_ref(&ref_update(1, 12))
        .expect("advance ref");
    assert_eq!(second.generation, 2);

    let cursor = session
        .repository_mut()
        .compare_and_swap_sync_cursor("space-1", "origin", 0, "cursor-a", 13)
        .expect("create cursor");
    assert_eq!(cursor.generation, 1);
    assert!(matches!(
        session
            .repository_mut()
            .compare_and_swap_sync_cursor("space-1", "origin", 0, "stale", 14),
        Err(StoreError::GenerationConflict { .. })
    ));
    session
        .repository_mut()
        .insert_pending_publish(&PendingPublishRecord {
            publish_id: "publish-1".into(),
            space_id: "space-1".into(),
            layer_id: "layer-1".into(),
            step_id: "step-1".into(),
            operation_id: None,
            idempotency_key: "idempotency-1".into(),
            expected_generation: 2,
            state: "prepared".into(),
            created_at_ms: 15,
            updated_at_ms: 15,
        })
        .expect("insert pending publish");
    session
        .repository_mut()
        .update_pending_publish_state("publish-1", "prepared", "uploading", 16)
        .expect("advance pending publish");

    drop(session);
    let mut read_only = store.session(SessionMode::Shared).expect("shared session");
    assert!(matches!(
        read_only
            .repository_mut()
            .compare_and_swap_sync_cursor("space-1", "origin", 1, "blocked", 15),
        Err(StoreError::ReadOnlySession)
    ));
    drop(read_only);
    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn recovery_advances_only_valid_journal_transitions() {
    let root = unique_test_dir("recovery");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("initialize store");
    let mut session = store
        .session(SessionMode::Exclusive)
        .expect("exclusive session");
    session
        .repository_mut()
        .insert_space(&space_record(&root))
        .expect("insert space");
    let operation = NewJournalOperation {
        operation_id: "operation-1".into(),
        space_id: "space-1".into(),
        kind: OperationKind::ObjectInstall,
        payload_json: "{\"digest\":\"abc\"}".into(),
        created_at_ms: 20,
        paths: vec![JournalPathInput {
            path: "tmp/object.tmp".into(),
            role: "temporary".into(),
            expected_object_id: None,
        }],
    };
    session
        .repository_mut()
        .prepare_operation(&operation)
        .expect("prepare operation");

    let competing = NewJournalOperation {
        operation_id: "operation-2".into(),
        ..operation.clone()
    };
    assert!(
        session
            .repository_mut()
            .prepare_operation(&competing)
            .is_err()
    );

    let mut handler = AdvancingRecovery;
    let first =
        recover_operations(session.repository_mut(), &mut handler, 21).expect("recover prepared");
    assert_eq!(first.operations[0].resulting_state, JournalState::Applying);
    let second =
        recover_operations(session.repository_mut(), &mut handler, 22).expect("recover applying");
    assert_eq!(
        second.operations[0].resulting_state,
        JournalState::MetadataCommitted
    );
    let third =
        recover_operations(session.repository_mut(), &mut handler, 23).expect("recover committed");
    assert_eq!(third.operations[0].resulting_state, JournalState::Finalized);
    assert!(
        session
            .repository()
            .list_recoverable_operations()
            .expect("list recoverable")
            .is_empty()
    );
    assert!(
        session
            .repository_mut()
            .transition_operation(
                "operation-1",
                JournalState::Finalized,
                JournalState::Applying,
                24,
                None,
            )
            .is_err()
    );

    drop(session);
    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

struct AdvancingRecovery;

impl RecoveryHandler for AdvancingRecovery {
    fn decide(
        &mut self,
        operation: &JournalOperation,
        _paths: &[JournalPath],
    ) -> StoreResult<RecoveryAction> {
        let next = match operation.state {
            JournalState::Prepared => JournalState::Applying,
            JournalState::Applying => JournalState::MetadataCommitted,
            JournalState::MetadataCommitted => JournalState::Finalized,
            state => {
                return Err(StoreError::InvalidState(format!(
                    "unexpected recovery state {state}"
                )));
            }
        };
        Ok(RecoveryAction::Advance(next))
    }
}

fn seed_space_layer_step(
    repository: &mut layrs_store_local::MetadataRepository,
    root: &std::path::Path,
    object_id: &str,
    relative_path: &str,
) {
    repository
        .insert_space(&space_record(root))
        .expect("insert space");
    repository
        .insert_layer(&LayerRecord {
            layer_id: "layer-1".into(),
            space_id: "space-1".into(),
            name: "main".into(),
            layer_kind: "local".into(),
            state: "active".into(),
            base_layer_id: None,
            timeline_active: true,
            generation: 0,
            created_at_ms: 2,
            updated_at_ms: 2,
        })
        .expect("insert layer");
    repository
        .insert_object(&ObjectRecord {
            object_id: object_id.into(),
            algorithm: "blake3".into(),
            byte_len: 9,
            object_kind: "tree".into(),
            relative_path: relative_path.into(),
            state: "available".into(),
            verified_at_ms: Some(3),
            created_at_ms: 3,
        })
        .expect("insert object");
    repository
        .append_step(
            &StepRecord {
                step_id: "step-1".into(),
                space_id: "space-1".into(),
                layer_id: "layer-1".into(),
                parent_step_id: None,
                root_object_id: object_id.into(),
                sequence: 1,
                created_at_ms: 4,
                authored_at: Some("2026-07-15T12:00:00Z".into()),
            },
            &[StepPathRecord {
                step_id: "step-1".into(),
                path: "root.txt".into(),
                object_id: object_id.into(),
                entry_kind: "file".into(),
                executable: false,
            }],
        )
        .expect("append step");
}

fn space_record(root: &std::path::Path) -> SpaceRecord {
    SpaceRecord {
        space_id: "space-1".into(),
        canonical_root: root.to_string_lossy().into_owned(),
        display_name: Some("Fixture".into()),
        case_sensitive: false,
        identity_token: None,
        state: "active".into(),
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

fn ref_update(expected_generation: u64, updated_at_ms: i64) -> RefUpdate {
    RefUpdate {
        space_id: "space-1".into(),
        ref_name: "refs/layers/main".into(),
        ref_kind: "layer_head".into(),
        layer_id: "layer-1".into(),
        step_id: "step-1".into(),
        expected_generation,
        updated_at_ms,
    }
}

fn write_fixture_object(store: &LocalStore, bytes: &[u8]) -> (String, String) {
    let digest = layrs_sync::ObjectDigest::blake3_for(bytes);
    let outcome = store
        .write_object_v2(bytes, &digest)
        .expect("write fixture object");
    let relative_path = outcome
        .path
        .strip_prefix(&store.layout().cas_dir)
        .expect("relative CAS path")
        .to_string_lossy()
        .replace('\\', "/");
    (digest.hex().to_string(), relative_path)
}

fn unique_test_dir(label: &str) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "layrs-store-local-repository-{label}-{}-{timestamp}",
        std::process::id()
    ))
}
