#[path = "../src/v3/schema.rs"]
mod schema;

use std::collections::BTreeSet;

fn ddl_without_line_comments() -> String {
    schema::BASELINE_SQL
        .lines()
        .map(|line| line.split_once("--").map_or(line, |(sql, _)| sql))
        .collect::<Vec<_>>()
        .join("\n")
}

fn created_tables() -> Vec<String> {
    ddl_without_line_comments()
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("CREATE TABLE ")
                .and_then(|tail| tail.split_whitespace().next())
                .map(ToOwned::to_owned)
        })
        .collect()
}

#[test]
fn baseline_is_unique_fresh_only_and_rejects_legacy() {
    let sql = schema::BASELINE_SQL;
    assert!(sql.contains("the Sync V3 baseline is fresh-only"));
    assert!(sql.contains("pg_catalog.pg_class"));
    assert!(sql.contains("namespace.nspname = pg_catalog.current_schema()"));
    assert!(sql.contains("value.relkind IN ('r', 'p', 'v', 'm', 'S', 'f', 'c')"));
    assert!(sql.contains("FROM pg_catalog.pg_proc AS value"));
    assert!(sql.contains("FROM pg_catalog.pg_type AS value"));
    assert!(sql.contains("FROM pg_catalog.pg_collation AS value"));
    assert!(!ddl_without_line_comments().contains("CREATE TABLE IF NOT EXISTS"));
    // Fresh-only applies to the baseline. Later native V3 extensions are
    // append-only migrations, never conversion of V1/V2 stores.
    let mut names: Vec<_> = std::fs::read_dir("migrations")
        .expect("migration directory")
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".sql"))
        .collect();
    names.sort();
    assert_eq!(
        names.first().map(String::as_str),
        Some(schema::BASELINE_MIGRATION)
    );
    assert_eq!(
        names
            .iter()
            .filter(|name| name.contains("baseline"))
            .count(),
        1
    );
    for (index, name) in names.iter().enumerate() {
        assert!(
            name.starts_with(&format!("{:04}_", index + 1)),
            "noncontiguous native migration {name}"
        );
    }
}

#[test]
fn table_contract_exactly_matches_the_baseline() {
    let actual = created_tables();
    assert_eq!(actual, schema::TABLES);
    assert_eq!(actual.iter().collect::<BTreeSet<_>>().len(), actual.len());
    assert_eq!(actual.len(), 43);
}

#[test]
fn metadata_pins_schema_protocol_baseline_and_contract_checksum() {
    let sql = schema::BASELINE_SQL;
    assert!(sql.contains(schema::SCHEMA_VERSION));
    assert!(sql.contains(schema::PROTOCOL_VERSION));
    assert!(sql.contains(schema::BASELINE_MIGRATION));
    assert!(sql.contains(schema::SCHEMA_CONTRACT_FNV1A64));
    assert!(sql.contains("catalog_fingerprint"));
    assert!(sql.contains("layrs_schema_catalog_fingerprint()"));
    assert_eq!(
        schema::baseline_sql_checksum(),
        schema::BASELINE_SQL_FNV1A64
    );
    assert_eq!(
        schema::schema_contract_checksum(),
        schema::SCHEMA_CONTRACT_FNV1A64
    );
    assert!(schema::METADATA_PROBE_SQL.contains("WHERE singleton_id = 1"));
}

#[test]
fn metadata_probe_fails_closed_for_missing_legacy_and_future_versions() {
    assert_eq!(
        schema::verify_metadata(None),
        Err(schema::SchemaCompatibilityError::MissingMetadata)
    );

    let good = schema::SchemaMetadata {
        schema_version: schema::SCHEMA_VERSION,
        protocol_version: schema::PROTOCOL_VERSION,
        baseline_migration: schema::BASELINE_MIGRATION,
        schema_contract_checksum: schema::SCHEMA_CONTRACT_FNV1A64,
        catalog_fingerprint: schema::EXPECTED_CATALOG_FINGERPRINT,
        actual_catalog_fingerprint: schema::EXPECTED_CATALOG_FINGERPRINT,
    };
    assert_eq!(schema::verify_metadata(Some(good)), Ok(()));

    let legacy = schema::SchemaMetadata {
        schema_version: "layrs.server-store.v2",
        ..good
    };
    assert!(matches!(
        schema::verify_metadata(Some(legacy)),
        Err(schema::SchemaCompatibilityError::UnsupportedSchemaVersion { .. })
    ));

    let future = schema::SchemaMetadata {
        protocol_version: "layrs.sync.v4",
        ..good
    };
    assert!(matches!(
        schema::verify_metadata(Some(future)),
        Err(schema::SchemaCompatibilityError::UnsupportedProtocolVersion { .. })
    ));

    let drift = schema::SchemaMetadata {
        actual_catalog_fingerprint: "md5:00000000000000000000000000000000",
        ..good
    };
    assert!(matches!(
        schema::verify_metadata(Some(drift)),
        Err(schema::SchemaCompatibilityError::RuntimeCatalogFingerprintMismatch { .. })
    ));
}

#[test]
fn catalog_fingerprint_covers_runtime_schema_shape() {
    let sql = schema::BASELINE_SQL;
    for catalog in [
        "pg_catalog.pg_attribute",
        "pg_catalog.pg_attrdef",
        "pg_catalog.pg_constraint",
        "pg_catalog.pg_index",
        "pg_catalog.pg_trigger",
        "pg_catalog.pg_proc",
        "pg_catalog.pg_type",
        "pg_catalog.pg_enum",
        "pg_catalog.pg_sequence",
        "pg_catalog.pg_foreign_table",
        "pg_catalog.pg_conversion",
        "pg_catalog.pg_operator",
        "pg_catalog.pg_opclass",
        "pg_catalog.pg_opfamily",
        "pg_catalog.pg_ts_config",
        "pg_catalog.pg_ts_dict",
        "pg_catalog.pg_ts_parser",
        "pg_catalog.pg_ts_template",
    ] {
        assert!(sql.contains(catalog), "missing catalog source {catalog}");
    }
    for detail in [
        "pg_catalog.format_type",
        "pg_catalog.pg_get_expr",
        "pg_catalog.pg_get_constraintdef",
        "pg_catalog.pg_get_indexdef",
        "pg_catalog.pg_get_triggerdef",
        "pg_catalog.pg_get_functiondef",
    ] {
        assert!(sql.contains(detail), "missing fingerprint detail {detail}");
    }
    assert!(schema::METADATA_PROBE_SQL.contains("actual_catalog_fingerprint"));
    assert!(sql.contains("value.relkind IN ('r', 'p', 'v', 'm', 'S', 'f', 'c')"));
}

#[test]
fn only_space_pointer_is_authoritative_and_heads_are_rebuildable() {
    let sql = ddl_without_line_comments();
    assert!(sql.contains("current_repository_view_id TEXT NOT NULL"));
    assert!(sql.contains("current_repository_view_generation BIGINT NOT NULL"));
    assert!(sql.contains("current_policy_set_id TEXT NOT NULL"));
    assert!(sql.contains("authorization_epoch BIGINT NOT NULL"));
    assert!(sql.contains("next_event_sequence BIGINT NOT NULL"));
    assert!(sql.contains("CREATE TABLE repository_view_history"));
    assert!(sql.contains("CREATE TABLE current_layer_heads_projection"));
    assert!(!schema::TABLES.contains(&"layer_heads"));
    assert!(!schema::TABLES.contains(&"operations"));
    assert!(!schema::TABLES.contains(&"worktrees"));
}

#[test]
fn immutable_graph_and_space_availability_are_normalized() {
    let sql = ddl_without_line_comments();
    for table in [
        "canonical_objects",
        "space_object_availability",
        "canonical_object_edges",
        "chunks",
        "file_manifests",
        "file_manifest_chunks",
        "steps",
        "step_parents",
        "space_steps",
        "step_observations",
    ] {
        assert!(schema::TABLES.contains(&table), "missing {table}");
    }
    assert!(!sql.to_ascii_uppercase().contains(" BYTEA"));
    assert!(sql.contains("reconstruction_verified_at TIMESTAMPTZ NOT NULL"));
    assert!(sql.contains("UNIQUE (step_id, parent_step_id)"));
}

#[test]
fn products_policy_views_evidence_and_authorization_are_described() {
    for table in [
        "products",
        "product_step_dependencies",
        "product_dependencies",
        "evidence_descriptors",
        "policy_sets",
        "policy_set_entries",
        "repository_views",
        "repository_view_layer_heads",
        "repository_view_markers",
        "repository_view_releases",
        "authorization_snapshots",
        "current_layer_definitions",
        "layer_definition_history",
    ] {
        assert!(schema::TABLES.contains(&table), "missing {table}");
    }
    assert!(schema::BASELINE_SQL.contains("RFC 8785 JCS"));
    assert!(schema::BASELINE_SQL.contains("server-generated authorization evidence"));
    assert!(
        schema::BASELINE_SQL.contains("product_kind TEXT GENERATED ALWAYS AS ('marker'::TEXT)")
    );
    assert!(
        schema::BASELINE_SQL.contains("product_kind TEXT GENERATED ALWAYS AS ('release'::TEXT)")
    );
    assert!(
        schema::BASELINE_SQL
            .contains("FOREIGN KEY (space_id, product_kind, marker_id, marker_product_object_id)")
    );
    assert!(
        schema::BASELINE_SQL.contains(
            "FOREIGN KEY (space_id, product_kind, release_id, release_product_object_id)"
        )
    );
    assert!(schema::BASELINE_SQL.contains("CREATE FUNCTION layrs_cas_layer_definition"));
    assert!(schema::BASELINE_SQL.contains("Layer definition history is append-only"));
    assert!(schema::BASELINE_SQL.contains("SECURITY DEFINER"));
    assert!(schema::BASELINE_SQL.contains("writer_role_name"));
    assert!(schema::BASELINE_SQL.contains("NOLOGIN NOSUPERUSER"));
    assert!(schema::BASELINE_SQL.contains("REVOKE INSERT, UPDATE, DELETE, TRUNCATE"));
}

#[test]
fn transaction_event_plan_capability_and_lease_constraints_are_explicit() {
    let sql = ddl_without_line_comments();
    assert!(sql.contains("PRIMARY KEY (space_id, client_id, idempotency_key)"));
    assert!(sql.contains("PRIMARY KEY (space_id, sequence)"));
    assert!(sql.contains("sequence BIGINT NOT NULL CHECK (sequence > 0)"));
    assert!(sql.contains("event_kind TEXT NOT NULL CHECK (event_kind IN"));
    assert!(sql.contains("UNIQUE (plan_id, item_ordinal)"));
    assert!(sql.contains("WHERE released_at IS NULL"));
    assert!(sql.contains("fencing_generation BIGINT NOT NULL"));
    assert!(sql.contains("item_disposition TEXT GENERATED ALWAYS AS ('missing'::TEXT)"));
    assert!(sql.contains("CREATE TABLE fetch_claim_objects"));
    assert!(sql.contains("claim_decision TEXT GENERATED ALWAYS AS ('allowed'::TEXT)"));
    assert!(sql.contains("UNIQUE (plan_id, claim_ordinal, object_id)"));
    assert!(!sql.contains("UNIQUE (plan_id, claim_ordinal)\n"));
    assert!(sql.contains("layrs_validate_object_plan_bijection"));
    assert!(sql.contains("layrs_validate_fetch_plan_bijection"));
    assert!(sql.contains("DEFERRABLE INITIALLY DEFERRED"));
    assert_eq!(schema::TRANSACTION_LOCK_ORDER.len(), 8);
}

#[test]
fn baseline_transaction_is_ordered_and_atomic() {
    let sql = ddl_without_line_comments();
    let begin = sql.find("BEGIN;").expect("BEGIN");
    let lock = sql.find("pg_advisory_xact_lock").expect("baseline lock");
    let metadata = sql.find("CREATE TABLE schema_metadata").expect("metadata");
    let commit = sql.rfind("COMMIT;").expect("COMMIT");
    assert!(begin < lock && lock < metadata && metadata < commit);
}
