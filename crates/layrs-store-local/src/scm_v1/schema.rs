//! Current immutable SQLite baseline for the native SCM store (format 2).
//!
//! This is deliberately not a migration chain. A database either has this
//! exact baseline or is refused; changing it requires a new schema version.
//! Any future conversion must be explicit and offline, outside this module.

pub const SCHEMA_MAGIC: &str = "layrs.native-scm.sqlite";
pub const SCHEMA_VERSION: u32 = 2;
pub const BASELINE_NAME: &str = "native_scm_v2_products_baseline";

/// BLAKE3 of [`SCHEMA_SQL`] exactly as embedded in the binary.
pub const SCHEMA_CHECKSUM: &str =
    "4c416baec9b695ccaa4cac440ac8284cd89b3c454feca56999cf098f987f79fb";

/// BLAKE3 fingerprint of ordered non-internal `sqlite_schema` rows produced
/// by the baseline, including tables and explicit indexes.
pub const SCHEMA_FINGERPRINT: &str =
    "4be528daa0c4e481a2551b5c4900ae1a6559f50bb7d4d7a8134cc0cf16997b8e";

/// Canonical fresh-database baseline, shared with the SQL fixture.
pub const SCHEMA_SQL: &str = include_str!("../../tests/fixtures/scm-v2.sql");

pub fn schema_checksum() -> String {
    blake3::hash(SCHEMA_SQL.as_bytes()).to_hex().to_string()
}

pub const REQUIRED_TABLES: &[&str] = &[
    "meta",
    "migrations",
    "applied_transactions",
    "stores",
    "spaces",
    "layers",
    "layer_parents",
    "objects",
    "products",
    "object_edges",
    "file_availability",
    "file_manifests",
    "file_manifest_chunks",
    "space_object_admissions",
    "pending_objects",
    "pending_admissions",
    "steps",
    "step_parents",
    "repository_views",
    "repository_view_layer_heads",
    "repository_view_markers",
    "repository_view_releases",
    "repository_view_worktrees",
    "step_observations",
    "operations",
    "operation_parents",
    "operation_rewrites",
    "operation_rewrite_targets",
    "worktrees",
    "markers",
    "marker_proofs",
    "releases",
    "release_proofs",
    "release_publications",
    "current_repository_views",
    "operation_heads",
    "recovery_operations",
    "recovery_paths",
    "materialization_leases",
    "pending_steps",
    "pending_ref_updates",
    "sync_cursors",
    "object_pins",
    "gc_roots",
];

#[cfg(test)]
mod baseline_identity_tests {
    #[test]
    fn current_baseline_identity_is_pinned() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(super::SCHEMA_SQL).unwrap();
        let fingerprint = super::super::sqlite::schema_fingerprint(&connection).unwrap();
        let checksum = super::schema_checksum();
        assert_eq!(
            (checksum.as_str(), fingerprint.as_str()),
            (super::SCHEMA_CHECKSUM, super::SCHEMA_FINGERPRINT)
        );
    }
}
