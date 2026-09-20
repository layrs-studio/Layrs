//! PostgreSQL schema contract for the fresh-only Sync V3 server store.
//!
//! The baseline is embedded so every deployment executes the exact SQL that
//! was reviewed with the binary. Runtime code must probe `schema_metadata`
//! before opening a writer and reject missing, V1/V2, or future schemas.

use std::fmt;

pub const SCHEMA_VERSION: &str = "layrs.server-store.v3-baseline.1";
pub const PROTOCOL_VERSION: &str = "layrs.sync.v3";
pub const BASELINE_MIGRATION: &str = "0001_sync_v3_baseline.sql";
pub const BASELINE_SQL: &str = include_str!("../../migrations/0001_sync_v3_baseline.sql");

/// Updated deliberately whenever the reviewed baseline SQL changes.
pub const BASELINE_SQL_FNV1A64: &str = "fnv1a64:05888aa06ffcba5a";

/// Hash of the stable schema/protocol/table/lock-order contract, independent
/// from SQL whitespace. It is persisted in `schema_metadata` at installation.
pub const SCHEMA_CONTRACT_FNV1A64: &str = "fnv1a64:03d739a95324da99";
pub const EXPECTED_CATALOG_FINGERPRINT: &str = "md5:e4ff5dd9a9cad6f92f413fbee31102f1";

pub const TABLES: &[&str] = &[
    "schema_metadata",
    "stores",
    "deployments",
    "principals",
    "workspaces",
    "workspace_memberships",
    "spaces",
    "canonical_objects",
    "chunks",
    "file_manifests",
    "file_manifest_chunks",
    "canonical_object_edges",
    "space_object_availability",
    "steps",
    "step_parents",
    "space_steps",
    "step_observations",
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
    "current_layer_heads_projection",
    "authorization_snapshots",
    "sync_transactions",
    "current_layer_definitions",
    "layer_definition_history",
    "repository_view_history",
    "policy_root_history",
    "space_events",
    "object_plans",
    "object_plan_items",
    "upload_capabilities",
    "fetch_plans",
    "fetch_plan_claims",
    "fetch_claim_objects",
    "fetch_capabilities",
    "object_leases",
];

/// Canonical row-lock order for final Sync V3 transactions.
///
/// Within each class, callers sort by the full primary key. Advisory locks or
/// application convenience must never invert this order.
pub const TRANSACTION_LOCK_ORDER: &[&str] = &[
    "principals.identity_and_revocation",
    "workspace_memberships",
    "spaces.authorization_and_roots",
    "policy_set_entries",
    "repository_view_history",
    "products.layer_definitions",
    "sync_transactions.idempotency",
    "space_object_availability",
];

pub const METADATA_PROBE_SQL: &str = r#"
SELECT
    schema_version,
    protocol_version,
    baseline_migration,
    schema_contract_checksum,
    catalog_fingerprint,
    layrs_schema_catalog_fingerprint() AS actual_catalog_fingerprint
FROM schema_metadata
WHERE singleton_id = 1
"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaMetadata<'a> {
    pub schema_version: &'a str,
    pub protocol_version: &'a str,
    pub baseline_migration: &'a str,
    pub schema_contract_checksum: &'a str,
    pub catalog_fingerprint: &'a str,
    pub actual_catalog_fingerprint: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchemaCompatibilityError {
    MissingMetadata,
    UnsupportedSchemaVersion { found: String },
    UnsupportedProtocolVersion { found: String },
    UnsupportedBaseline { found: String },
    ContractChecksumMismatch { found: String },
    UnsupportedCatalogFingerprint { found: String },
    RuntimeCatalogFingerprintMismatch { stored: String, actual: String },
}

impl fmt::Display for SchemaCompatibilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingMetadata => formatter
                .write_str("missing Layrs schema metadata; Sync V3 requires a fresh V3 baseline"),
            Self::UnsupportedSchemaVersion { found } => write!(
                formatter,
                "unsupported Layrs schema version {found:?}; V1/V2 and future schemas are refused"
            ),
            Self::UnsupportedProtocolVersion { found } => write!(
                formatter,
                "unsupported Layrs protocol {found:?}; expected {PROTOCOL_VERSION:?}"
            ),
            Self::UnsupportedBaseline { found } => write!(
                formatter,
                "unsupported Layrs baseline {found:?}; expected {BASELINE_MIGRATION:?}"
            ),
            Self::ContractChecksumMismatch { found } => write!(
                formatter,
                "Layrs schema contract checksum mismatch: found {found:?}"
            ),
            Self::UnsupportedCatalogFingerprint { found } => write!(
                formatter,
                "unsupported Layrs PostgreSQL catalog fingerprint: found {found:?}"
            ),
            Self::RuntimeCatalogFingerprintMismatch { stored, actual } => write!(
                formatter,
                "Layrs PostgreSQL catalog drift: stored {stored:?}, actual {actual:?}"
            ),
        }
    }
}

impl std::error::Error for SchemaCompatibilityError {}

pub fn verify_metadata(
    metadata: Option<SchemaMetadata<'_>>,
) -> Result<(), SchemaCompatibilityError> {
    let metadata = metadata.ok_or(SchemaCompatibilityError::MissingMetadata)?;
    if metadata.schema_version != SCHEMA_VERSION {
        return Err(SchemaCompatibilityError::UnsupportedSchemaVersion {
            found: metadata.schema_version.to_owned(),
        });
    }
    if metadata.protocol_version != PROTOCOL_VERSION {
        return Err(SchemaCompatibilityError::UnsupportedProtocolVersion {
            found: metadata.protocol_version.to_owned(),
        });
    }
    if metadata.baseline_migration != BASELINE_MIGRATION {
        return Err(SchemaCompatibilityError::UnsupportedBaseline {
            found: metadata.baseline_migration.to_owned(),
        });
    }
    if metadata.schema_contract_checksum != SCHEMA_CONTRACT_FNV1A64 {
        return Err(SchemaCompatibilityError::ContractChecksumMismatch {
            found: metadata.schema_contract_checksum.to_owned(),
        });
    }
    if metadata.catalog_fingerprint != EXPECTED_CATALOG_FINGERPRINT {
        return Err(SchemaCompatibilityError::UnsupportedCatalogFingerprint {
            found: metadata.catalog_fingerprint.to_owned(),
        });
    }
    if metadata.actual_catalog_fingerprint != metadata.catalog_fingerprint {
        return Err(
            SchemaCompatibilityError::RuntimeCatalogFingerprintMismatch {
                stored: metadata.catalog_fingerprint.to_owned(),
                actual: metadata.actual_catalog_fingerprint.to_owned(),
            },
        );
    }
    Ok(())
}

pub fn baseline_sql_checksum() -> String {
    format!("fnv1a64:{:016x}", fnv1a64(BASELINE_SQL.as_bytes()))
}

pub fn schema_contract_checksum() -> String {
    let mut material = String::new();
    material.push_str(SCHEMA_VERSION);
    material.push('\0');
    material.push_str(PROTOCOL_VERSION);
    for table in TABLES {
        material.push('\0');
        material.push_str(table);
    }
    for lock in TRANSACTION_LOCK_ORDER {
        material.push('\0');
        material.push_str(lock);
    }
    format!("fnv1a64:{:016x}", fnv1a64(material.as_bytes()))
}

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
