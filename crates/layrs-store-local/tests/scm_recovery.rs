use layrs_core::scm::{Chunk, OperationId, StoreId, WorktreeId};
use layrs_store::scm::CanonicalObject;
use layrs_store_local::scm_v1::recovery::MAX_FINDING_MESSAGE_BYTES;
use layrs_store_local::scm_v1::{
    MutationOutcome, NativeCas, NativeRecovery, PathEvidence, RecoveryDecision, RecoveryError,
    RecoveryFaultInjector, RecoveryFaultPoint, RecoveryFindingCode, RecoveryHandler, RecoveryId,
    RecoveryPathPlan, RecoveryPathResult, RecoveryPathRole, RecoveryPathState, RecoveryPlan,
    RecoveryPreparation, RecoveryRecord, RecoveryState, RecoveryType, init, open_exclusive,
};
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "layrs-native-recovery-{label}-{}-{nonce}-{serial}",
            std::process::id()
        ));
        Self(root)
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn initialize(label: &str, worktrees: &[&str]) -> TestRoot {
    let root = TestRoot::new(label);
    let store_id = StoreId::parse(format!("store:{label}")).unwrap();
    drop(init(&root.0, store_id.clone()).unwrap());
    let path = root.0.join("metadata").join("layrs.sqlite3");
    let connection = Connection::open(path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    connection
        .execute(
            "INSERT INTO spaces(space_id, payload_json, created_at_ms) VALUES ('space:test', '{}', 0)",
            [],
        )
        .unwrap();
    let view_id = format!("blake3:{}", "1".repeat(64));
    connection
        .execute(
            "INSERT INTO objects(object_id, object_kind, storage_path, byte_length, verified, created_at_ms)
             VALUES (?1, 'repository_view', 'test/view', 1, 1, 0)",
            [&view_id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO repository_views(repository_view_id, space_id) VALUES (?1, 'space:test')",
            [&view_id],
        )
        .unwrap();
    let operation_payload = json!({
        "schemaVersion": "layrs.operation.v1",
        "operationId": "operation:base",
        "spaceId": "space:test",
        "parentOperationIds": [],
        "resultingViewId": view_id.as_str(),
        "kind": "initialize",
        "summary": "recovery test baseline",
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
            [&view_id, &operation_payload],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO operation_heads(space_id, operation_id, ordinal, updated_by_transaction_id)
             VALUES ('space:test', 'operation:base', 0, 'seed')",
            [],
        )
        .unwrap();
    for worktree in worktrees {
        connection
            .execute(
                "INSERT INTO worktrees(worktree_id, store_id, space_id, active_operation_id,
                 checkout_root, filesystem_identity, payload_json, created_at_ms)
                 VALUES (?1, ?2, 'space:test', 'operation:base', ?3, ?4, '{}', 0)",
                [
                    *worktree,
                    store_id.as_str(),
                    &format!("C:/checkout/{worktree}"),
                    &format!("identity:{worktree}"),
                ],
            )
            .unwrap();
    }
    drop(connection);
    root
}

fn plan(id: &str, key: &str, worktree: &str, path: &str, kind: RecoveryType) -> RecoveryPlan {
    let paths = if kind == RecoveryType::CasStagingCleanup {
        Vec::new()
    } else {
        vec![RecoveryPathPlan::new(path, RecoveryPathRole::Destination, None).unwrap()]
    };
    RecoveryPlan::new(
        RecoveryId::parse(id).unwrap(),
        key,
        WorktreeId::parse(worktree).unwrap(),
        Some(OperationId::parse("operation:base").unwrap()),
        kind,
        json!({"intent": "test"}),
        paths,
        10,
    )
    .unwrap()
}

fn preparation(path: Option<&str>, retryable: bool) -> RecoveryPreparation {
    let paths = path
        .map(|path| {
            vec![
                PathEvidence::new(
                    path,
                    RecoveryPathRole::Destination,
                    json!({"original": "absent"}),
                )
                .unwrap(),
            ]
        })
        .unwrap_or_default();
    RecoveryPreparation::new(
        json!({"backupDurable": true, "retryable": retryable}),
        paths,
    )
    .unwrap()
}

fn applied(path: &str) -> RecoveryPathResult {
    RecoveryPathResult::new(
        path,
        RecoveryPathRole::Destination,
        RecoveryPathState::Applied,
        json!({"verified": true}),
    )
    .unwrap()
}

fn operation_snapshot(root: &Path) -> Vec<Vec<String>> {
    let connection = Connection::open(root.join("metadata").join("layrs.sqlite3")).unwrap();
    [
        (
            "operations",
            "operation_id, space_id, resulting_view_id, operation_kind, actor_id, device_id, created_at, payload_json",
            "operation_id",
        ),
        (
            "operation_parents",
            "operation_id, parent_operation_id, ordinal",
            "operation_id, ordinal",
        ),
        (
            "operation_heads",
            "space_id, operation_id, ordinal, updated_by_transaction_id",
            "space_id, ordinal",
        ),
    ]
        .into_iter()
        .map(|(table, columns, ordering)| {
            let mut statement = connection
                .prepare(&format!(
                    "SELECT json_array({columns}) FROM {table} ORDER BY {ordering}"
                ))
                .unwrap();
            statement
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        })
        .collect()
}

#[derive(Default)]
struct CompleteHandler {
    calls: u64,
}

impl RecoveryHandler for CompleteHandler {
    fn recover(
        &mut self,
        record: &RecoveryRecord,
    ) -> layrs_store_local::scm_v1::RecoveryResult<RecoveryDecision> {
        self.calls += 1;
        Ok(RecoveryDecision::Complete {
            evidence: json!({"resultVerified": true}),
            path_results: record
                .paths
                .iter()
                .map(|path| {
                    RecoveryPathResult::new(
                        path.path.clone(),
                        path.role,
                        RecoveryPathState::Applied,
                        json!({"resultVerified": true}),
                    )
                    .unwrap()
                })
                .collect(),
        })
    }
}

struct FailOnce {
    point: RecoveryFaultPoint,
    fired: AtomicBool,
}

impl FailOnce {
    fn new(point: RecoveryFaultPoint) -> Self {
        Self {
            point,
            fired: AtomicBool::new(false),
        }
    }
}

impl RecoveryFaultInjector for FailOnce {
    fn check(&self, point: RecoveryFaultPoint) -> Result<(), String> {
        if point == self.point && !self.fired.swap(true, Ordering::SeqCst) {
            Err("simulated crash".into())
        } else {
            Ok(())
        }
    }
}

#[path = "scm_recovery/handlers.rs"]
mod handlers;
#[path = "scm_recovery/journal.rs"]
mod journal;
#[path = "scm_recovery/leases.rs"]
mod leases;
#[path = "scm_recovery/lifecycle.rs"]
mod lifecycle;
