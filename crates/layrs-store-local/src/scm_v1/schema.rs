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
    "0728c3e1b5e8d361d896a1af55393104cc1de27f010bae969866b4d410c4288c";
pub const SCHEMA_V3_FINGERPRINT: &str =
    "8b8083b2a1dc98e3d747cc5fc3854fdce38c692fa7482d174391399dba5140b2";
pub const SCHEMA_V3_SQL: &str = include_str!("../../tests/fixtures/scm-v3.sql");

pub fn schema_v3_checksum() -> String {
    blake3::hash(SCHEMA_V3_SQL.as_bytes()).to_hex().to_string()
}

/// Store format 8 baseline, draft 5 (council s25, s27, s28): v3 without
/// `space_object_admissions` (every object kind is admitted by the pack
/// bitmaps or `space_loose_admissions`), without the object projections
/// (the canonical records are the only source), without the unused
/// `pending_*` tables, and without the history tables (Operations and
/// StepObservations are pack records, sequenced by `history_clock`;
/// `space_initializers` and `history_identities` answer what must stay
/// complete). Draft 5: history section entries carry their record's body
/// digest (packs v4 changed, schema unchanged). Draft 6: compact replay
/// receipts (phase 2c). Draft 7: packs v4 commit every block's raw hash in
/// their block table and their history entries carry no digest (phase 3a,
/// schema unchanged). Draft 8: Tree records are binary (phase 3b, schema
/// unchanged). Draft 9: Step, RepositoryView, Operation and StepObservation
/// records are binary (phase 3c, schema unchanged). Draft 10: their
/// dictionaries gain the rewrite map's keys (council s33). Draft 11: a
/// receipt's key is its transaction id as a tagged UUID or text (phase 3e).
/// Drafts change until format 8 is frozen; a Store of an earlier draft is
/// refused.
pub const SCHEMA_V4_VERSION: u32 = 4;
pub const BASELINE_V4_NAME: &str = "native_scm_v4_history_out_draft11";
pub const SCHEMA_V4_CHECKSUM: &str =
    "428d51f992050b023fbd02e9be013fdf7d990c166cf4aa45415a13cc26588fb1";
pub const SCHEMA_V4_FINGERPRINT: &str =
    "b7765ac2762ab282533e3b58a9f618963218492b12fc2d438b0de1f249a9f3a2";
pub const SCHEMA_V4_SQL: &str = include_str!("../../tests/fixtures/scm-v4.sql");

pub fn schema_v4_checksum() -> String {
    blake3::hash(SCHEMA_V4_SQL.as_bytes()).to_hex().to_string()
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
    "cas_pack_keys",
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

/// The v3 tables but `space_object_admissions`, the object projections
/// (`objects`, `object_edges`, `steps`, `step_parents`, `repository_views`
/// and its child tables), the unused `pending_*` tables and the history
/// tables; plus the history clock, initializers and identities.
pub const REQUIRED_TABLES_V4: &[&str] = &[
    "meta",
    "migrations",
    "applied_transactions",
    "stores",
    "spaces",
    "layers",
    "layer_parents",
    "products",
    "cas_packs",
    "cas_pack_inline",
    "cas_pack_keys",
    "space_pack_admissions",
    "loose_objects",
    "space_loose_admissions",
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
    "history_clock",
    "space_initializers",
    "history_identities",
];

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

/// Whether a Store of `format_version` keeps compact replay receipts: its
/// schema is v4 (format 8, phase 2c). Every reader and writer of
/// `applied_transactions` asks this.
pub(crate) fn compact_receipts(format_version: u32) -> bool {
    baseline_for_format(format_version).version == BASELINE_V4.version
}

/// The baseline of a Store of `format_version` (formats 6 and 7: v3;
/// format 8: v4; else v2).
pub fn baseline_for_format(format_version: u32) -> &'static Baseline {
    if format_version == super::format::HISTORY_FORMAT_VERSION {
        return &BASELINE_V4;
    }
    if format_version == super::format::COMPACT_FORMAT_VERSION
        || format_version == super::format::WHOLE_FILES_FORMAT_VERSION
    {
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
