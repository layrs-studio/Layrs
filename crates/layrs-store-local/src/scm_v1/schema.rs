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

/// Store format 6 baseline (ADR 0028): content leaves SQLite for registered
/// packs v2 (`cas_packs`), per-Space admission bitmaps and loose objects.
/// Format 2 and 5 Stores keep the v2 baseline above; a database is validated
/// against the baseline its catalog has, and the Store format must name it.
pub const SCHEMA_V3_VERSION: u32 = 3;
pub const BASELINE_V3_NAME: &str = "native_scm_v3_compact_baseline";
pub const SCHEMA_V3_CHECKSUM: &str =
    "42e1a1598676b7cfe5dc7ee50c9f9511b7b1da04cde9c47706948448bcb61c3a";
pub const SCHEMA_V3_FINGERPRINT: &str =
    "00f94c6ce26dfde23e1df8b8c83e1b09473cd5bea7904a6ee788c7d157d78578";
pub const SCHEMA_V3_SQL: &str = include_str!("../../tests/fixtures/scm-v3.sql");

pub fn schema_v3_checksum() -> String {
    blake3::hash(SCHEMA_V3_SQL.as_bytes()).to_hex().to_string()
}

pub const REQUIRED_TABLES_V3: &[&str] = &[
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
    "space_object_admissions",
    "cas_packs",
    "space_pack_admissions",
    "loose_objects",
    "space_loose_admissions",
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

/// One immutable SQLite baseline: what creation writes and validation
/// requires, exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Baseline {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
    pub checksum: &'static str,
    pub fingerprint: &'static str,
    pub required_tables: &'static [&'static str],
    /// SQLite page size of a fresh database (format 6: 1 KiB pages, so the
    /// ~80 nearly empty tables and indexes do not cost 4 KiB each).
    pub page_size: Option<u32>,
}

pub const BASELINE_V2: Baseline = Baseline {
    version: SCHEMA_VERSION,
    name: BASELINE_NAME,
    sql: SCHEMA_SQL,
    checksum: SCHEMA_CHECKSUM,
    fingerprint: SCHEMA_FINGERPRINT,
    required_tables: REQUIRED_TABLES,
    page_size: None,
};

pub const BASELINE_V3: Baseline = Baseline {
    version: SCHEMA_V3_VERSION,
    name: BASELINE_V3_NAME,
    sql: SCHEMA_V3_SQL,
    checksum: SCHEMA_V3_CHECKSUM,
    fingerprint: SCHEMA_V3_FINGERPRINT,
    required_tables: REQUIRED_TABLES_V3,
    page_size: Some(1024),
};

/// The baseline of a Store of `format_version` (format 6: v3; else v2).
pub fn baseline_for_format(format_version: u32) -> &'static Baseline {
    if format_version == super::format::COMPACT_FORMAT_VERSION {
        &BASELINE_V3
    } else {
        &BASELINE_V2
    }
}

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

    #[test]
    fn v3_baseline_identity_is_pinned() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(super::SCHEMA_V3_SQL).unwrap();
        let fingerprint = super::super::sqlite::schema_fingerprint(&connection).unwrap();
        let checksum = super::schema_v3_checksum();
        assert_eq!(
            (checksum.as_str(), fingerprint.as_str()),
            (super::SCHEMA_V3_CHECKSUM, super::SCHEMA_V3_FINGERPRINT)
        );
    }
}
