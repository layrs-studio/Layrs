//! Administrator-only storage observations and monitoring exit codes. Logical
//! quotas, PostgreSQL bytes and CAS filesystem capacity remain distinct.
mod thresholds;
use crate::identity::{IdentityError, PgIdentity};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
    routing::get,
};
use layrs_core::scm::SpaceId;
use layrs_store_server::v3::{CasBudgetUsage, CasQuotas, read_cas_budget_in};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Transaction};
use std::path::PathBuf;
pub use thresholds::StorageThresholds;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Ok,
    Warning,
    Critical,
}
impl Severity {
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::Ok => 0,
            Self::Warning => 1,
            Self::Critical => 2,
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub resource: &'static str,
    pub unit: &'static str,
    pub used: u64,
    pub warning_at: Option<u64>,
    pub critical_at: Option<u64>,
    pub monitored: bool,
    pub severity: Severity,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CasVolume {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub free_bytes: u64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageReport {
    pub schema_version: &'static str,
    pub space_id: Option<SpaceId>,
    pub global_usage: CasBudgetUsage,
    pub space_usage: Option<CasBudgetUsage>,
    pub limits: CasQuotas,
    pub thresholds: StorageThresholds,
    pub database_bytes: u64,
    pub cas_volume: CasVolume,
    /// CAS occupancy cannot be inferred from the containing volume's usage.
    pub physical_cas_bytes: Option<u64>,
    pub readings: Vec<Reading>,
    pub severity: Severity,
}
#[derive(Debug)]
pub enum StorageMonitorError {
    InvalidThresholds,
    Unavailable,
    InvalidSpace,
}
impl std::fmt::Display for StorageMonitorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidThresholds => "invalid storage alert thresholds",
            Self::Unavailable => "storage observation unavailable",
            Self::InvalidSpace => "invalid storage scope",
        })
    }
}
impl std::error::Error for StorageMonitorError {}

#[derive(Clone)]
pub struct StorageMonitor {
    pool: PgPool,
    cas_root: PathBuf,
    limits: CasQuotas,
    thresholds: StorageThresholds,
    deployment_id: String,
}
impl StorageMonitor {
    pub fn new(
        pool: PgPool,
        cas_root: PathBuf,
        limits: CasQuotas,
        thresholds: StorageThresholds,
        deployment_id: String,
    ) -> Result<Self, StorageMonitorError> {
        limits
            .validate()
            .map_err(|_| StorageMonitorError::InvalidThresholds)?;
        Ok(Self {
            pool,
            cas_root,
            limits,
            thresholds: thresholds.validate()?,
            deployment_id,
        })
    }
    pub async fn read(
        &self,
        space: Option<&SpaceId>,
    ) -> Result<StorageReport, StorageMonitorError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| StorageMonitorError::Unavailable)?;
        let report = self.read_in(&mut tx, space).await?;
        tx.commit()
            .await
            .map_err(|_| StorageMonitorError::Unavailable)?;
        Ok(report)
    }
    async fn read_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        space: Option<&SpaceId>,
    ) -> Result<StorageReport, StorageMonitorError> {
        if let Some(space) = space {
            let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM spaces s JOIN workspaces w USING(workspace_id) WHERE s.space_id=$1 AND w.deployment_id=$2)")
                .bind(space.as_str()).bind(&self.deployment_id).fetch_one(&mut **tx).await.map_err(|_|StorageMonitorError::Unavailable)?;
            if !valid {
                return Err(StorageMonitorError::InvalidSpace);
            }
        }
        let global_usage = read_cas_budget_in(tx, None)
            .await
            .map_err(|_| StorageMonitorError::Unavailable)?;
        let space_usage = match space {
            Some(space) => Some(
                read_cas_budget_in(tx, Some(space))
                    .await
                    .map_err(|_| StorageMonitorError::Unavailable)?,
            ),
            None => None,
        };
        let database: i64 = sqlx::query_scalar("SELECT pg_database_size(current_database())")
            .fetch_one(&mut **tx)
            .await
            .map_err(|_| StorageMonitorError::Unavailable)?;
        let database_bytes =
            u64::try_from(database).map_err(|_| StorageMonitorError::Unavailable)?;
        let root = self.cas_root.clone();
        let cas_volume = tokio::task::spawn_blocking(move || {
            // Windows volume APIs can report the containing drive even when
            // the supplied leaf is absent. A missing CAS root is not healthy.
            if !std::fs::metadata(&root).is_ok_and(|metadata| metadata.is_dir()) {
                return Err(StorageMonitorError::Unavailable);
            }
            let stats = fs2::statvfs(root).map_err(|_| StorageMonitorError::Unavailable)?;
            let volume = CasVolume {
                total_bytes: stats.total_space(),
                available_bytes: stats.available_space(),
                free_bytes: stats.free_space(),
            };
            if volume.total_bytes == 0
                || volume.available_bytes > volume.total_bytes
                || volume.free_bytes > volume.total_bytes
            {
                return Err(StorageMonitorError::Unavailable);
            }
            Ok(volume)
        })
        .await
        .map_err(|_| StorageMonitorError::Unavailable)??;
        let mut readings = vec![
            self.thresholds.quota(
                "global_cas_bytes",
                "bytes",
                global_usage.charged_bytes,
                self.limits.max_total_bytes,
            ),
            self.thresholds.quota(
                "global_cas_objects",
                "objects",
                global_usage.objects,
                self.limits.max_total_objects,
            ),
            self.thresholds.database(database_bytes),
            self.thresholds
                .volume(cas_volume.total_bytes, cas_volume.available_bytes),
        ];
        if let Some(usage) = &space_usage {
            readings.push(self.thresholds.quota(
                "space_cas_bytes",
                "bytes",
                usage.charged_bytes,
                self.limits.max_space_bytes,
            ));
            readings.push(self.thresholds.quota(
                "space_cas_objects",
                "objects",
                usage.objects,
                self.limits.max_space_objects,
            ));
        }
        let severity = readings
            .iter()
            .map(|r| r.severity)
            .max()
            .unwrap_or(Severity::Ok);
        Ok(StorageReport {
            schema_version: "layrs.storage-budget.v1",
            space_id: space.cloned(),
            global_usage,
            space_usage,
            limits: self.limits,
            thresholds: self.thresholds,
            database_bytes,
            cas_volume,
            physical_cas_bytes: None,
            readings,
            severity,
        })
    }
}

#[derive(Clone)]
struct HttpState {
    identity: PgIdentity,
    monitor: StorageMonitor,
}
pub fn router(identity: PgIdentity, monitor: StorageMonitor) -> Router {
    Router::new()
        .route("/v3/admin/storage-budget", get(storage_budget))
        .with_state(HttpState { identity, monitor })
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Scope {
    space_id: Option<SpaceId>,
}
async fn storage_budget(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(scope): Query<Scope>,
) -> Result<Response, IdentityError> {
    let mut tx = state
        .monitor
        .pool
        .begin()
        .await
        .map_err(|_| IdentityError::Unavailable)?;
    // The administrator Principal stays locked until observations finish;
    // revocation cannot race a check performed outside this transaction.
    state
        .identity
        .require_administrator_in(&headers, &mut tx)
        .await?;
    let report = state
        .monitor
        .read_in(&mut tx, scope.space_id.as_ref())
        .await
        .map_err(|e| match e {
            StorageMonitorError::InvalidSpace => IdentityError::Invalid,
            _ => IdentityError::Unavailable,
        })?;
    tx.commit().await.map_err(|_| IdentityError::Unavailable)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(report)).into_response())
}
