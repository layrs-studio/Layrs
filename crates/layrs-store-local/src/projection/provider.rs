//! Ordinary schema fence plus Windows-only provider bootstrap metadata lifecycle.
//! Neither route grants provider population, source serving or readiness.
use super::*;
#[cfg(windows)]
mod bootstrap;
#[cfg(windows)]
pub use bootstrap::{
    ProviderBootstrapAttempt,
    ProviderBootstrapWriterOperation,
    ProviderBootstrapWriteReportProvenance,
    ProviderBootstrapReceipt,
    ProviderBootstrapCreate,
    ProviderBootstrapSqlWrite,
    ProviderBootstrapWriteStep,
    ProviderBootstrapWriteState,
    ProviderBootstrapWriteEffect,
    ProviderBootstrapWriteReport,
    ProviderControlPublicationOutcome,
    ProviderControlCommitDisposition,
    ProviderControlPublished,
    ProviderControlWithheld,
};
#[cfg(windows)]
pub use records::{ProviderBootstrapSource, ProviderSourceDescriptor};
#[cfg(windows)]
mod journal;
#[cfg(windows)]
pub(super) mod records;
#[cfg(windows)]
mod artifacts;
#[cfg(windows)]
pub(super) mod database_io;
#[cfg(windows)]
mod process_profile;
#[cfg(windows)]
pub(super) mod recovery;
#[cfg(windows)]
mod validation;
#[cfg(windows)]
pub use journal::Refusal as ProviderJournalRefusal;
#[cfg(windows)]
pub use process_profile::{
    PROVIDER_SQLITE_SOURCE_ID, ProviderSqliteBuildProfile, ProviderSqliteProcessDeclaration,
    ProviderSqliteProfileError, declare_provider_sqlite_process_profile,
};
#[cfg(windows)]
pub use records::{
    PreparedBinding as ProviderPreparedBinding, ProviderBootstrapBinding,
    ReceiptRef as ProviderReceiptReference,
};
#[cfg(windows)]
pub use recovery::{
    ProviderBootstrapCause, ProviderBootstrapFailure, ProviderBootstrapFailureKind,
    ProviderBootstrapInspection, ProviderBootstrapPhase, ProviderBootstrapRecovery,
    ProviderBootstrapState, ProviderControlLocation, ProviderDatabaseInspection,
    ProviderDatabaseScope, ProviderDatabaseStatus, ProviderFormatState,
    ProviderRecoveryEffect, ProviderRecoveryEffectState, ProviderRollbackTrigger,
};
pub(super) mod schema;

pub const PROJECTION_PROVIDER_SCHEMA_VERSION: u32 = 8;
pub const PROVIDER_CATALOG_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const PROVIDER_DESCRIPTOR_MAX_BYTES: u64 = 256 * 1024;
pub const PROVIDER_CATALOG_AGGREGATE_MAX_BYTES: u64 = 256 * 1024 * 1024;
pub const PROVIDER_CATALOG_MAX_DESCRIPTORS: u64 = 100_000;
pub const PROVIDER_COMPANION_MAX_FILES: u64 = 100_000;
pub const PROVIDER_COMPANION_MAX_DELETIONS: u64 = 100_000;
pub const PROVIDER_COMPANION_AGGREGATE_MAX_BYTES: u64 = 1024 * 1024 * 1024;

/// Requires an exact schema fingerprint first. Never interprets opaque provider
/// documents as authorization, readiness, or a reason to reclaim their content.
pub(super) fn require_ordinary(c: &rusqlite::Connection) -> Result<()> {
    let version: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != PROJECTION_PROVIDER_SCHEMA_VERSION {
        return Ok(());
    }
    let rows: u64 = c.query_row("SELECT COUNT(*) FROM projection_backend", [], |r| r.get(0))?;
    let ordinary: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_backend WHERE singleton=1 AND backend='ordinary' AND mount_id IS NULL)",
        [], |r| r.get(0),
    )?;
    if rows != 1 || !ordinary {
        return Err(invalid(if rows != 1 {
            "projection backend singleton missing or invalid; integrity failure"
        } else {
            "provider backend requires a provider capability; ordinary access refused"
        }));
    }
    for table in schema::EVIDENCE_TABLES {
        if c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table})"), [], |r| {
            r.get::<_, bool>(0)
        })? {
            return Err(invalid(
                "provider evidence requires a supported typed interpreter; ordinary access and reclamation refused",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderSchemaUpgradePoint {
    SchemaCreated,
    BeforeCommit,
    AfterCommit,
}
pub trait ProviderSchemaUpgradeFaults {
    fn check(&self, point: ProviderSchemaUpgradePoint) -> io::Result<()>;
}
pub struct NoProviderSchemaUpgradeFaults;
impl ProviderSchemaUpgradeFaults for NoProviderSchemaUpgradeFaults {
    fn check(&self, _: ProviderSchemaUpgradePoint) -> io::Result<()> {
        Ok(())
    }
}

impl ProjectionStore {
    /// Explicit, atomic private7 -> private8 upgrade of an idle ordinary format4
    /// member. Does not activate a provider or modify immutable history/FORMAT.
    pub fn upgrade_provider_schema(&self, expected_generation: u64) -> Result<()> {
        self.upgrade_provider_schema_with_faults(
            expected_generation,
            &NoProviderSchemaUpgradeFaults,
        )
    }
    pub fn upgrade_provider_schema_with_faults(
        &self,
        expected_generation: u64,
        faults: &dyn ProviderSchemaUpgradeFaults,
    ) -> Result<()> {
        if !self.is_shared() {
            return Err(invalid(
                "provider schema upgrade requires an ordinary format4 member",
            ));
        }
        // Nonmigrating, nonsettling acceptance: pending budgets/stages must not
        // disappear as a side effect of checking whether upgrade is permitted.
        let mut access = format::recovery_access(self, None, false)?;
        let current = database::current(&access.connection)?;
        if current.generation != expected_generation {
            return Err(ProjectionStoreError::Stale {
                expected: expected_generation,
                actual: current.generation,
            });
        }
        database::require_mutable(&access.connection)?;
        let pending: bool = access.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_budgets) OR EXISTS(SELECT 1 FROM projection_prejournal) OR EXISTS(SELECT 1 FROM projection_preparations WHERE status='acquiring') OR EXISTS(SELECT 1 FROM projection_rebase_previews)",
            [], |r| r.get(0),
        )?;
        if pending {
            return Err(invalid(
                "provider schema upgrade blocked by pending private work",
            ));
        }
        for slot in [
            "namespace-staging",
            "projection-staging",
            "projection-quarantine",
        ] {
            match std::fs::symlink_metadata(self.root.join(".layrs").join(slot)) {
                Ok(_) => {
                    let path = self.root.join(".layrs").join(slot);
                    let pin = crate::secure_fs::PinnedDirectory::open(&path)?;
                    if std::fs::read_dir(&path)?.next().transpose()?.is_some() {
                        return Err(invalid(
                            "provider schema upgrade blocked by checkout staging; explicit recovery required",
                        ));
                    }
                    pin.verify_path_identity()?;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        let family = access
            .family
            .as_ref()
            .ok_or_else(|| invalid("missing upgrade family lease"))?;
        let catalog = family.catalog_once(false)?;
        let reserved: bool = catalog.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM reservations WHERE owner=?1)",
            [self.identity().worktree_id.as_str()],
            |r| r.get(0),
        )?;
        if reserved {
            return Err(invalid(
                "provider schema upgrade blocked by shared reservation",
            ));
        }
        drop(catalog);
        let version: u32 = access
            .connection
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == PROJECTION_PROVIDER_SCHEMA_VERSION {
            return access.verify();
        }
        if version != 7 {
            return Err(invalid(
                "provider schema upgrade requires exact private schema7",
            ));
        }
        access.verify()?;
        let pins =
            format::preliminary_pins(&self.root, &self.marker, &access.metadata, &access.database)?;
        let tx = access
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch(schema::SQL)?;
        faults.check(ProviderSchemaUpgradePoint::SchemaCreated)?;
        tx.execute(
            "INSERT INTO projection_backend VALUES(1,'ordinary',NULL)",
            [],
        )?;
        tx.execute_batch("PRAGMA user_version=8;")?;
        database::validate(&tx, &self.marker)?;
        faults.check(ProviderSchemaUpgradePoint::BeforeCommit)?;
        pins.verify(&self.root, &self.marker)?;
        access.metadata.verify_path_identity()?;
        access.database.verify(
            &self
                .marker
                .metadata_path(&self.root)
                .join("projection.sqlite3"),
        )?;
        family.verify()?;
        tx.commit()?;
        faults.check(ProviderSchemaUpgradePoint::AfterCommit)?;
        access.verify()
    }
}
