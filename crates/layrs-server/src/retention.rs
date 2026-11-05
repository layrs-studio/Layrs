//! Read-only observation, never an authorization to collect/delete an object.
mod filesystem;
pub mod gc;
use crate::{config::ServerConfig, installation};
use serde::Serialize;
use sqlx::{PgPool, Row};
use std::{collections::BTreeMap, path::PathBuf};

type Result<T> = std::result::Result<T, AuditError>;
#[derive(Debug)]
pub struct AuditError(pub &'static str);
impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for AuditError {}
#[derive(Clone, Copy)]
pub struct AuditLimits {
    pub catalog_objects: usize,
    pub filesystem_entries: usize,
    pub findings: usize,
    pub depth: usize,
    pub file_bytes: u64,
    pub report_bytes: usize,
}
impl Default for AuditLimits {
    fn default() -> Self {
        Self {
            catalog_objects: 100_000,
            filesystem_entries: 200_000,
            findings: 20_000,
            depth: 8,
            file_bytes: 16 * 1024 * 1024 * 1024,
            report_bytes: 32 * 1024 * 1024,
        }
    }
}
impl AuditLimits {
    fn validate(self) -> Result<Self> {
        let maximum = Self::default();
        if self.catalog_objects == 0
            || self.catalog_objects > maximum.catalog_objects
            || self.filesystem_entries == 0
            || self.filesystem_entries > maximum.filesystem_entries
            || self.findings == 0
            || self.findings > maximum.findings
            || self.depth == 0
            || self.depth > maximum.depth
            || self.file_bytes == 0
            || self.file_bytes > maximum.file_bytes
            || self.report_bytes == 0
            || self.report_bytes > maximum.report_bytes
        {
            return Err(AuditError("invalid retention audit limits"));
        }
        Ok(self)
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFinding {
    pub relative_path: String,
    pub observed_bytes: Option<u64>,
    pub expected_bytes: Option<u64>,
    pub classification: &'static str,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionReport {
    pub schema_version: &'static str,
    pub deployment_id: String,
    pub store_id: String,
    pub database_snapshot_xmin: String,
    pub observed_at: String,
    pub catalogued_objects_retained: usize,
    pub quarantined_objects_retained: usize,
    pub catalogued_files_present: usize,
    pub reserved_objects_retained: u64,
    pub physical_files_observed: usize,
    pub observed_file_bytes: u64,
    pub missing_files: Vec<FileFinding>,
    pub size_mismatches: Vec<FileFinding>,
    pub files_to_examine: Vec<FileFinding>,
    pub content_verified: bool,
    pub deletion_authorized: bool,
    pub observation_only: bool,
    pub limitations: [&'static str; 4],
}
struct CatalogEntry {
    bytes: u64,
    quarantined: bool,
}

/// Credentials for the configured PostgreSQL installation are the maintenance
/// boundary, as for storage-budget. No public/ordinary-user HTTP route exists.
pub async fn audit(
    pool: &PgPool,
    config: &ServerConfig,
    limits: AuditLimits,
) -> Result<RetentionReport> {
    let limits = limits.validate()?;
    installation::verify_identity_schema(pool)
        .await
        .map_err(|_| AuditError("retention schema unavailable or incompatible"))?;
    let mut tx = pool.begin().await.map_err(database)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(database)?;
    sqlx::query("SET LOCAL statement_timeout='15s'")
        .execute(&mut *tx)
        .await
        .map_err(database)?;
    let backend =
        serde_jcs::to_string(&serde_json::json!({"path":config.cas_root.to_string_lossy()}))
            .map_err(|_| AuditError("invalid retention CAS configuration"))?;
    let matches:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deployments d JOIN stores s USING(store_id) WHERE d.deployment_id=$1 AND d.store_id=$2 AND s.backend_kind='filesystem' AND s.backend_config_jcs=$3)")
        .bind(&config.deployment_id).bind(config.store_id.as_str()).bind(backend).fetch_one(&mut *tx).await.map_err(database)?;
    if !matches {
        return Err(AuditError(
            "retention deployment/CAS configuration mismatch",
        ));
    }
    let snapshot=sqlx::query("SELECT txid_snapshot_xmin(txid_current_snapshot())::text AS xmin,clock_timestamp()::text AS observed_at").fetch_one(&mut *tx).await.map_err(database)?;
    let mut catalogue = BTreeMap::new();
    let mut after = String::new();
    loop {
        let rows=sqlx::query("SELECT object_id,object_kind,encoded_byte_length,verification_state,left(storage_key,1025) AS storage_key FROM canonical_objects WHERE store_id=$1 AND object_id>$2 ORDER BY object_id LIMIT 256")
            .bind(config.store_id.as_str()).bind(&after).fetch_all(&mut *tx).await.map_err(database)?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            if catalogue.len() >= limits.catalog_objects {
                return Err(AuditError("retention catalogue limit exceeded"));
            }
            let id: String = row.get("object_id");
            let kind: String = row.get("object_kind");
            let key: String = row.get("storage_key");
            if id.len() != 71
                || !id.starts_with("blake3:")
                || !id[7..]
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                || !matches!(
                    kind.as_str(),
                    "step"
                        | "repository_view"
                        | "chunk"
                        | "file_manifest"
                        | "tree"
                        | "conflict"
                        | "symlink"
                        | "tombstone"
                        | "evidence"
                        | "product"
                        | "policy_set"
                        | "authorization_snapshot"
                )
                || key != format!("objects/{kind}/{}/{}", &id[7..9], &id[7..])
            {
                return Err(AuditError("retention catalogue key is noncanonical"));
            }
            let bytes = u64::try_from(row.get::<i64, _>("encoded_byte_length"))
                .map_err(|_| AuditError("invalid retention catalogue length"))?;
            catalogue.insert(
                key,
                CatalogEntry {
                    bytes,
                    quarantined: row.get::<String, _>("verification_state") == "quarantined",
                },
            );
            after = id;
        }
    }
    let reserved:i64=sqlx::query_scalar("SELECT count(DISTINCT i.object_id) FROM object_plan_items i JOIN object_plans p USING(plan_id) JOIN spaces s USING(space_id) JOIN workspaces w USING(workspace_id) JOIN deployments d USING(deployment_id) WHERE d.store_id=$1")
        .bind(config.store_id.as_str()).fetch_one(&mut *tx).await.map_err(database)?;
    let root: PathBuf = config.cas_root.clone();
    let files = tokio::task::spawn_blocking(move || filesystem::scan(&root, limits))
        .await
        .map_err(|_| AuditError("retention filesystem worker unavailable"))??;
    let mut report = RetentionReport {
        schema_version: "layrs.retention-audit.v1",
        deployment_id: config.deployment_id.clone(),
        store_id: config.store_id.as_str().into(),
        database_snapshot_xmin: snapshot.get("xmin"),
        observed_at: snapshot.get("observed_at"),
        catalogued_objects_retained: catalogue.len(),
        quarantined_objects_retained: catalogue.values().filter(|e| e.quarantined).count(),
        catalogued_files_present: 0,
        reserved_objects_retained: u64::try_from(reserved)
            .map_err(|_| AuditError("retention reservation overflow"))?,
        physical_files_observed: files.len(),
        observed_file_bytes: 0,
        missing_files: vec![],
        size_mismatches: vec![],
        files_to_examine: vec![],
        content_verified: false,
        deletion_authorized: false,
        observation_only: true,
        limitations: [
            "Concurrent CAS preparation may precede SQL admission; uncatalogued does not mean orphan.",
            "Every catalogued object and every reservation remains retained, including expired or quarantined entries.",
            "Metadata-only observation; no canonical checksum verification or physical reclaimable-byte estimate.",
            "Database snapshot and filesystem observations are not one atomic snapshot; no deletion is authorized.",
        ],
    };
    for file in files {
        report.observed_file_bytes = report
            .observed_file_bytes
            .checked_add(file.bytes)
            .ok_or(AuditError("retention byte count overflow"))?;
        if let Some(entry) = catalogue.remove(&file.relative_path) {
            report.catalogued_files_present += 1;
            if entry.bytes != file.bytes {
                report.size_mismatches.push(FileFinding {
                    relative_path: file.relative_path,
                    observed_bytes: Some(file.bytes),
                    expected_bytes: Some(entry.bytes),
                    classification: "catalogued_size_mismatch_retained",
                });
            }
        } else {
            let classification = if file.relative_path.starts_with("staging/") {
                "staging_to_examine_may_be_active"
            } else if file.relative_path.starts_with("objects/") {
                "uncatalogued_to_examine_may_be_prepared"
            } else {
                "unrecognized_file_to_examine"
            };
            report.files_to_examine.push(FileFinding {
                relative_path: file.relative_path,
                observed_bytes: Some(file.bytes),
                expected_bytes: None,
                classification,
            });
        }
        check_findings(&report, limits)?;
    }
    for (relative_path, entry) in catalogue {
        report.missing_files.push(FileFinding {
            relative_path,
            observed_bytes: None,
            expected_bytes: Some(entry.bytes),
            classification: "catalogued_file_not_observed",
        });
        check_findings(&report, limits)?;
    }
    tx.commit().await.map_err(database)?;
    if serde_json::to_vec(&report)
        .map_err(|_| AuditError("retention report encoding failed"))?
        .len()
        > limits.report_bytes
    {
        return Err(AuditError("retention report byte limit exceeded"));
    }
    Ok(report)
}
fn check_findings(report: &RetentionReport, limits: AuditLimits) -> Result<()> {
    if report.missing_files.len() + report.size_mismatches.len() + report.files_to_examine.len()
        > limits.findings
    {
        return Err(AuditError("retention finding limit exceeded"));
    }
    Ok(())
}
fn database(_: sqlx::Error) -> AuditError {
    AuditError("retention database observation unavailable")
}
