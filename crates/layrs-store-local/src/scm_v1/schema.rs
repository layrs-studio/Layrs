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
    "11339f5a4f8616a8320139041f533238292c2faf7d84ad718d38cf7d2c48e91e";
pub const SCHEMA_V3_FINGERPRINT: &str =
    "96eafcebe6ad17da351b0f7e3b81c7d15a7508912242286e4bfde22730a9e596";
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
    "cas_pack_inline",
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

/// The schema of Store format 7 (council s13 Q6): the v3 tables without the
/// `pending_*` staging tables format 6 never writes, `WITHOUT ROWID` for
/// text or composite primary keys (no separate key index), and statements
/// on one line (SQLite stores each statement's text). Same columns and
/// constraints: every query of v3 runs unchanged.
pub const SCHEMA_V4_VERSION: u32 = 4;
pub const BASELINE_V4_NAME: &str = "native_scm_v4_compact_baseline";
pub const SCHEMA_V4_CHECKSUM: &str =
    "3816861bf7124cdfeb92397b373b12015ef507e191364ef9462a63c635fe91d8";
pub const SCHEMA_V4_FINGERPRINT: &str =
    "0bbd90d22f1e634d616b7969c56b20a6fccde9a21a954f21f374219d3089d762";
pub const SCHEMA_V4_SQL: &str = include_str!("../../tests/fixtures/scm-v4.sql");

pub fn schema_v4_checksum() -> String {
    blake3::hash(SCHEMA_V4_SQL.as_bytes()).to_hex().to_string()
}

/// The v3 tables without the `pending_*` staging tables.
pub const REQUIRED_TABLES_V4: &[&str] = &[
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
    "cas_pack_inline",
    "space_pack_admissions",
    "loose_objects",
    "space_loose_admissions",
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
    /// Whether a fresh database returns the pages a commit frees to the file
    /// system (`auto_vacuum = FULL`; format 6: retired inline pack images,
    /// a dropped Room's rows), instead of keeping them free in the file.
    pub auto_vacuum: bool,
}

pub const BASELINE_V2: Baseline = Baseline {
    version: SCHEMA_VERSION,
    name: BASELINE_NAME,
    sql: SCHEMA_SQL,
    checksum: SCHEMA_CHECKSUM,
    fingerprint: SCHEMA_FINGERPRINT,
    required_tables: REQUIRED_TABLES,
    page_size: None,
    auto_vacuum: false,
};

pub const BASELINE_V3: Baseline = Baseline {
    version: SCHEMA_V3_VERSION,
    name: BASELINE_V3_NAME,
    sql: SCHEMA_V3_SQL,
    checksum: SCHEMA_V3_CHECKSUM,
    fingerprint: SCHEMA_V3_FINGERPRINT,
    required_tables: REQUIRED_TABLES_V3,
    page_size: Some(1024),
    auto_vacuum: true,
};

pub const BASELINE_V4: Baseline = Baseline {
    version: SCHEMA_V4_VERSION,
    name: BASELINE_V4_NAME,
    sql: SCHEMA_V4_SQL,
    checksum: SCHEMA_V4_CHECKSUM,
    fingerprint: SCHEMA_V4_FINGERPRINT,
    required_tables: REQUIRED_TABLES_V4,
    page_size: Some(1024),
    auto_vacuum: true,
};

/// The baseline of a Store of `format_version` (format 7: v4; format 6:
/// v3; else v2).
pub fn baseline_for_format(format_version: u32) -> &'static Baseline {
    if format_version == super::format::WHOLE_FILES_FORMAT_VERSION {
        &BASELINE_V4
    } else if format_version == super::format::COMPACT_FORMAT_VERSION {
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

    #[test]
    fn v4_baseline_identity_is_pinned() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(super::SCHEMA_V4_SQL).unwrap();
        let fingerprint = super::super::sqlite::schema_fingerprint(&connection).unwrap();
        let checksum = super::schema_v4_checksum();
        assert_eq!(
            (checksum.as_str(), fingerprint.as_str()),
            (super::SCHEMA_V4_CHECKSUM, super::SCHEMA_V4_FINGERPRINT)
        );
    }

    /// v4 keeps every column and constraint of v3: only the pending_*
    /// tables, the rowid of keyed tables and the statements' spacing differ.
    #[test]
    fn v4_has_the_columns_of_v3_without_the_pending_tables() {
        let v3 = rusqlite::Connection::open_in_memory().unwrap();
        v3.execute_batch(super::SCHEMA_V3_SQL).unwrap();
        let v4 = rusqlite::Connection::open_in_memory().unwrap();
        v4.execute_batch(super::SCHEMA_V4_SQL).unwrap();
        let columns =
            |connection: &rusqlite::Connection, table: &str| -> Vec<(String, String, bool, bool)> {
                let mut statement = connection
                    .prepare(&format!("PRAGMA table_info(\"{table}\")"))
                    .unwrap();
                statement
                    .query_map([], |row| {
                        Ok((
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get::<_, i64>(5)? > 0,
                        ))
                    })
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap()
            };
        for table in super::REQUIRED_TABLES_V3 {
            if table.starts_with("pending_") {
                assert!(!super::REQUIRED_TABLES_V4.contains(table));
                continue;
            }
            assert!(super::REQUIRED_TABLES_V4.contains(table), "{table}");
            assert_eq!(columns(&v3, table), columns(&v4, table), "{table}");
        }
        assert_eq!(
            super::REQUIRED_TABLES_V4.len() + 4,
            super::REQUIRED_TABLES_V3.len()
        );
    }
}
