//! `complete_in_transaction` inside a caller's transaction (the metadata
//! batch of a materialization): refused leases roll back with the caller.
use super::*;
use layrs_core::scm::{OperationId, StoreId};
use serde_json::json;

struct Root(std::path::PathBuf);

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A Store with one Space, one Operation and the Worktree `worktree:one`.
fn store() -> Root {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = Root(std::env::temp_dir().join(format!(
        "layrs-batch-completion-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )));
    let store_id = StoreId::parse("store:batch-completion").unwrap();
    // The rows below are those of the format 5 schema, whatever
    // LAYRS_STORE_FORMAT selects.
    drop(super::super::init_with_format(&root.0, store_id.clone(), 5).unwrap());
    let connection = rusqlite::Connection::open(root.0.join("metadata/layrs.sqlite3")).unwrap();
    let view = format!("blake3:{}", "1".repeat(64));
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .unwrap();
    connection
        .execute(
            "INSERT INTO spaces(space_id, payload_json, created_at_ms) VALUES ('space:test', '{}', 0)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO objects(object_id, object_kind, storage_path, byte_length, verified, created_at_ms)
             VALUES (?1, 'repository_view', 'test/view', 1, 1, 0)",
            [&view],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO repository_views(repository_view_id, space_id) VALUES (?1, 'space:test')",
            [&view],
        )
        .unwrap();
    let operation = json!({
        "schemaVersion": "layrs.operation.v1",
        "operationId": "operation:base",
        "spaceId": "space:test",
        "parentOperationIds": [],
        "resultingViewId": view.as_str(),
        "kind": "initialize",
        "summary": "batch completion baseline",
        "rewriteMap": [],
        "actorId": null,
        "deviceId": null,
        "createdAt": "2026-07-19T00:00:00Z",
        "metadata": []
    })
    .to_string();
    connection
        .execute(
            "INSERT INTO operations(operation_id, space_id, resulting_view_id, operation_kind,
             actor_id, device_id, created_at, payload_json)
             VALUES ('operation:base', 'space:test', ?1, 'initialize', NULL, NULL,
             '2026-07-19T00:00:00Z', ?2)",
            [&view, &operation],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO worktrees(worktree_id, store_id, space_id, active_operation_id,
             checkout_root, filesystem_identity, payload_json, created_at_ms)
             VALUES ('worktree:one', ?1, 'space:test', 'operation:base', 'C:/checkout/one',
             'identity:one', '{}', 0)",
            [store_id.as_str()],
        )
        .unwrap();
    root
}

/// Council s5 r1/r2: only a session that ends with a checked checkpoint
/// uses NORMAL; every other writer, the recovery journal's included, is FULL.
#[test]
fn only_checked_sessions_write_with_normal_synchronous() {
    let root = store();
    let synchronous = |connection: &rusqlite::Connection| -> u32 {
        connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .unwrap()
    };
    let mut store = super::super::open_exclusive(&root.0).unwrap();
    assert_eq!(synchronous(&store.begin_write_connection().unwrap()), 2);
    assert_eq!(synchronous(&store.begin_session_writer().unwrap()), 1);
    let recovery = NativeRecovery::new(&mut store).unwrap();
    assert_eq!(synchronous(&recovery.connection), 2);
}

#[test]
fn a_completion_inside_a_batch_transaction_refuses_expired_leases_and_rolls_back() {
    let root = store();
    let id = RecoveryId::parse("recovery:batch-completion").unwrap();
    let plan = RecoveryPlan::new(
        id.clone(),
        "idem:batch-completion",
        WorktreeId::parse("worktree:one").unwrap(),
        Some(OperationId::parse("operation:base").unwrap()),
        RecoveryType::Restore,
        json!({"intent": "test"}),
        vec![RecoveryPathPlan::new("cb.txt", RecoveryPathRole::Destination, None).unwrap()],
        10,
    )
    .unwrap();
    let claim = {
        let mut store = super::super::open_exclusive(&root.0).unwrap();
        let mut recovery = NativeRecovery::new(&mut store).unwrap();
        recovery.create(&plan).unwrap();
        let preparation = RecoveryPreparation::new(
            json!({"backupDurable": true, "retryable": false}),
            vec![
                PathEvidence::new(
                    "cb.txt",
                    RecoveryPathRole::Destination,
                    json!({"original": "absent"}),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        recovery
            .prepare_and_claim(&id, &preparation, "owner:cb", 100, 50)
            .unwrap()
    };
    let applied = || {
        vec![
            RecoveryPathResult::new(
                "cb.txt",
                RecoveryPathRole::Destination,
                RecoveryPathState::Applied,
                json!({"verified": true}),
            )
            .unwrap(),
        ]
    };
    let connection = rusqlite::Connection::open(root.0.join("metadata/layrs.sqlite3")).unwrap();
    let state = |connection: &rusqlite::Connection| -> String {
        connection
            .query_row(
                "SELECT state FROM recovery_operations WHERE recovery_operation_id=?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .unwrap()
    };
    // At its expiry the lease no longer completes: the caller's transaction
    // (the metadata batch) rolls back, the journal stays Applying.
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(matches!(
        complete_in_transaction(
            &connection,
            &id,
            "owner:cb",
            &claim.leases,
            json!({"done": true}),
            &applied(),
            150,
        ),
        Err(RecoveryError::LeaseFenced { .. })
    ));
    connection.execute_batch("ROLLBACK").unwrap();
    assert_eq!(state(&connection), "applying");
    // A wrong owner is refused as well.
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(matches!(
        complete_in_transaction(
            &connection,
            &id,
            "owner:other",
            &claim.leases,
            json!({"done": true}),
            &applied(),
            120,
        ),
        Err(RecoveryError::OwnerMismatch(_))
    ));
    connection.execute_batch("ROLLBACK").unwrap();
    // Within its lease, the completion commits with the caller.
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    complete_in_transaction(
        &connection,
        &id,
        "owner:cb",
        &claim.leases,
        json!({"done": true}),
        &applied(),
        120,
    )
    .unwrap();
    connection.execute_batch("COMMIT").unwrap();
    assert_eq!(state(&connection), "completed");
}
