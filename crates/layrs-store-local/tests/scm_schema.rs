use layrs_store_local::scm_v1::{REQUIRED_TABLES, SCHEMA_CHECKSUM, SCHEMA_SQL, schema_checksum};
use rusqlite::Connection;

#[test]
fn embedded_fixture_and_published_checksum_are_exact() {
    assert_eq!(
        SCHEMA_SQL,
        include_str!("fixtures/scm-v2.sql"),
        "production schema must use the reviewed fixture verbatim"
    );
    assert_eq!(schema_checksum(), SCHEMA_CHECKSUM);
}

#[test]
fn fixture_executes_standalone_with_the_published_table_set() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    connection.execute_batch(SCHEMA_SQL).unwrap();

    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_schema \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap();
    let actual: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let mut expected: Vec<String> = REQUIRED_TABLES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    expected.sort();
    assert_eq!(actual, expected);
}

#[test]
fn recovery_schema_exposes_only_native_states_and_fenced_resources() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch(SCHEMA_SQL).unwrap();
    let recovery_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'recovery_operations'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    for state in ["pending", "prepared", "applying", "completed", "failed"] {
        assert!(recovery_sql.contains(&format!("'{state}'")));
    }
    for obsolete in [
        "metadata_committed",
        "finalized",
        "rolling_back",
        "rolled_back",
        "needs_repair",
    ] {
        assert!(!recovery_sql.contains(obsolete));
    }
    for column in [
        "idempotency_key",
        "request_digest",
        "evidence_json",
        "attempt_count",
        "owner_token",
    ] {
        assert!(recovery_sql.contains(column));
    }

    let lease_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'materialization_leases'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    for column in [
        "worktree_id",
        "resource_key",
        "recovery_operation_id",
        "owner_token",
        "generation",
        "heartbeat_at_ms",
        "expires_at_ms",
        "released_at_ms",
    ] {
        assert!(lease_sql.contains(column));
    }
    assert!(lease_sql.contains("PRIMARY KEY (worktree_id, resource_key)"));
    assert!(lease_sql.contains("REFERENCES recovery_operations(recovery_operation_id)"));
}

#[test]
fn gc_inventory_has_an_explicit_retained_operation_root_kind() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch(SCHEMA_SQL).unwrap();
    let gc_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'gc_roots'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(gc_sql.contains("'retained_operation'"));
}

#[test]
fn applied_transactions_expose_complete_replay_fields_and_valid_json_constraint() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch(SCHEMA_SQL).unwrap();

    let columns: Vec<(String, i64)> = connection
        .prepare("PRAGMA table_info(applied_transactions)")
        .unwrap()
        .query_map([], |row| Ok((row.get(1)?, row.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for required in [
        "transaction_id",
        "request_digest",
        "receipt_json",
        "applied_at_ms",
    ] {
        assert!(
            columns.iter().any(|(name, not_null)| name == required
                && (required == "transaction_id" || *not_null == 1)),
            "missing NOT NULL replay field {required}"
        );
    }

    let digest = format!("blake3:{}", "0".repeat(64));
    connection
        .execute(
            "INSERT INTO applied_transactions( \
                 transaction_id, request_digest, receipt_json, applied_at_ms \
             ) VALUES ('transaction:canonical', ?1, ?2, 0)",
            rusqlite::params![
                digest,
                format!(
                    "{{\"requestDigest\":\"blake3:{}\",\"result\":\"applied\"}}",
                    "0".repeat(64)
                )
            ],
        )
        .unwrap();
    assert!(
        connection
            .execute(
                "INSERT INTO applied_transactions( \
                     transaction_id, request_digest, receipt_json, applied_at_ms \
                 ) VALUES ('transaction:invalid', ?1, '{not-json}', 0)",
                [format!("blake3:{}", "1".repeat(64))],
            )
            .is_err()
    );
}

#[test]
fn each_space_has_at_most_one_initialize_operation() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    connection.execute_batch(SCHEMA_SQL).unwrap();

    let index_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema \
             WHERE type = 'index' AND name = 'operations_single_initialize_per_space_idx'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(index_sql.contains("UNIQUE INDEX operations_single_initialize_per_space_idx"));
    assert!(index_sql.contains("WHERE operation_kind = 'initialize'"));
    for index_name in [
        "operations_payload_space_kind_idx",
        "operations_single_initialize_payload_space_idx",
    ] {
        let sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type = 'index' AND name = ?1",
                [index_name],
                |row| row.get(0),
            )
            .unwrap();
        assert!(sql.contains("json_extract(payload_json, '$.spaceId')"));
        assert!(sql.contains("json_extract(payload_json, '$.kind')"));
    }
    let projection_space_payload_kind: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema \
             WHERE type = 'index' AND name = 'operations_projection_space_payload_kind_idx'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(projection_space_payload_kind.contains("space_id"));
    assert!(projection_space_payload_kind.contains("json_extract(payload_json, '$.kind')"));
    let payload_space_projection_kind: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema \
             WHERE type = 'index' AND name = 'operations_payload_space_projection_kind_idx'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(payload_space_projection_kind.contains("json_extract(payload_json, '$.spaceId')"));
    assert!(payload_space_projection_kind.contains("operation_kind"));

    for (space_id, view_id, object_path) in [
        ("space:one", "view:one", "repository_view/one"),
        ("space:two", "view:two", "repository_view/two"),
    ] {
        connection
            .execute(
                "INSERT INTO spaces(space_id, payload_json, created_at_ms) \
                 VALUES (?1, '{}', 0)",
                [space_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO objects( \
                     object_id, object_kind, storage_path, byte_length, verified, created_at_ms \
                 ) VALUES (?1, 'repository_view', ?2, 1, 1, 0)",
                rusqlite::params![view_id, object_path],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO repository_views(repository_view_id, space_id) VALUES (?1, ?2)",
                rusqlite::params![view_id, space_id],
            )
            .unwrap();
    }

    let insert_operation =
        |operation_id: &str, space_id: &str, view_id: &str, operation_kind: &str| {
            let payload = format!(
                "{{\"actorId\":null,\"createdAt\":\"2026-07-20T00:00:00Z\",\
                 \"deviceId\":null,\"kind\":\"{operation_kind}\",\"metadata\":[],\
                 \"operationId\":\"{operation_id}\",\"parentOperationIds\":[],\
                 \"resultingViewId\":\"{view_id}\",\"rewriteMap\":[],\
                 \"schemaVersion\":\"layrs.operation.v1\",\"spaceId\":\"{space_id}\",\
                 \"summary\":\"schema test\"}}"
            );
            connection.execute(
                "INSERT INTO operations( \
                 operation_id, space_id, resulting_view_id, operation_kind, \
                 actor_id, device_id, created_at, payload_json \
             ) VALUES (?1, ?2, ?3, ?4, NULL, NULL, '2026-07-20T00:00:00Z', ?5)",
                rusqlite::params![operation_id, space_id, view_id, operation_kind, payload],
            )
        };

    insert_operation("operation:init-one", "space:one", "view:one", "initialize").unwrap();
    assert!(
        connection
            .execute(
                "UPDATE operations SET operation_kind = 'weave' \
                 WHERE operation_id = 'operation:init-one'",
                [],
            )
            .is_err(),
        "updating only the projected kind must violate payload consistency"
    );
    assert!(
        connection
            .execute(
                "UPDATE operations SET space_id = 'space:two' \
                 WHERE operation_id = 'operation:init-one'",
                [],
            )
            .is_err(),
        "updating only the projected Space must violate payload consistency"
    );
    assert!(
        insert_operation(
            "operation:init-one-duplicate",
            "space:one",
            "view:one",
            "initialize"
        )
        .is_err(),
        "a Space must reject its second Initialize operation"
    );
    insert_operation("operation:init-two", "space:two", "view:two", "initialize").unwrap();

    insert_operation(
        "operation:capture-one-a",
        "space:one",
        "view:one",
        "capture_worktree",
    )
    .unwrap();
    insert_operation(
        "operation:capture-one-b",
        "space:one",
        "view:one",
        "capture_worktree",
    )
    .unwrap();

    connection
        .pragma_update(None, "ignore_check_constraints", "ON")
        .unwrap();
    let forged_payload = |operation_id: &str, view_id: &str| {
        format!(
            "{{\"actorId\":null,\"createdAt\":\"2026-07-20T00:00:00Z\",\
             \"deviceId\":null,\"kind\":\"initialize\",\"metadata\":[],\
             \"operationId\":\"{operation_id}\",\"parentOperationIds\":[],\
             \"resultingViewId\":\"{view_id}\",\"rewriteMap\":[],\
             \"schemaVersion\":\"layrs.operation.v1\",\"spaceId\":\"space:payload-only\",\
             \"summary\":\"forged projection\"}}"
        )
    };
    connection
        .execute(
            "INSERT INTO operations( \
                 operation_id, space_id, resulting_view_id, operation_kind, \
                 actor_id, device_id, created_at, payload_json \
             ) VALUES (?1, 'space:one', 'view:one', 'weave', NULL, NULL, \
                       '2026-07-20T00:00:00Z', ?2)",
            rusqlite::params![
                "operation:forged-payload-a",
                forged_payload("operation:forged-payload-a", "view:one")
            ],
        )
        .unwrap();
    assert!(
        connection
            .execute(
                "INSERT INTO operations( \
                     operation_id, space_id, resulting_view_id, operation_kind, \
                     actor_id, device_id, created_at, payload_json \
                 ) VALUES (?1, 'space:two', 'view:two', 'weave', NULL, NULL, \
                           '2026-07-20T00:00:00Z', ?2)",
                rusqlite::params![
                    "operation:forged-payload-b",
                    forged_payload("operation:forged-payload-b", "view:two")
                ],
            )
            .is_err(),
        "payload-based uniqueness must survive forged scalar projections"
    );
    connection
        .pragma_update(None, "ignore_check_constraints", "OFF")
        .unwrap();

    let initialize_count: u32 = connection
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE operation_kind = 'initialize'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(initialize_count, 2);
}

#[test]
fn object_edges_allow_repeated_targets_at_distinct_ordinals_only() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    connection.execute_batch(SCHEMA_SQL).unwrap();

    for (object_id, path) in [
        ("tree:source", "tree/source"),
        ("tree:target", "tree/target"),
    ] {
        connection
            .execute(
                "INSERT INTO objects( \
                     object_id, object_kind, storage_path, byte_length, verified, created_at_ms \
                 ) VALUES (?1, 'tree', ?2, 1, 1, 0)",
                rusqlite::params![object_id, path],
            )
            .unwrap();
    }

    for ordinal in [0, 1] {
        connection
            .execute(
                "INSERT INTO object_edges( \
                     source_object_id, target_object_id, edge_kind, ordinal \
                 ) VALUES ('tree:source', 'tree:target', 'tree_entry', ?1)",
                [ordinal],
            )
            .unwrap();
    }
    assert!(
        connection
            .execute(
                "INSERT INTO object_edges( \
                     source_object_id, target_object_id, edge_kind, ordinal \
                 ) VALUES ('tree:source', 'tree:target', 'tree_entry', 1)",
                [],
            )
            .is_err(),
        "the source/kind ordinal remains the unique ordered position"
    );
}
