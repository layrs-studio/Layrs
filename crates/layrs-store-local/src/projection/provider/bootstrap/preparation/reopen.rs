//! Read-only durable handoff for an already prepared native provider root.
use super::*;
use std::{error::Error, fmt, path::Path, sync::Arc};

/// A fresh owner reconstructed from durable A/P, complete SQL and native READY.
/// No invocation write report or content-serving source is reconstructed.
pub struct ProviderPreparedReopened {
    pub(super) held: outcome::PublicationPins,
    pub(super) root: layrs_vfs::ProviderRootGuard,
    pub(super) record: ProviderPreparationRecord,
}

#[derive(Debug)]
pub struct ProviderPreparedOpenFailure {
    pub kind: ProviderPreparationFailureKind,
    pub operation: &'static str,
    pub database: Option<ProviderDatabaseScope>,
    pub cause: Cause,
    pub readonly_rollback: Option<Arc<rusqlite::Error>>,
    pub rollback_triggers: Vec<recovery::ProviderRollbackTrigger>,
}
impl fmt::Display for ProviderPreparedOpenFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "provider prepared {} ({:?}): {}",
            self.operation, self.kind, self.cause
        )
    }
}
impl Error for ProviderPreparedOpenFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}
impl ProviderPreparedOpenFailure {
    pub(super) fn from_failure(operation: &'static str, failure: report::Failure) -> Self {
        let kind = failure.kind();
        Self {
            kind,
            operation,
            database: failure.database,
            cause: failure.cause,
            readonly_rollback: failure.rollback,
            rollback_triggers: failure.rollback_triggers,
        }
    }
}

impl ProviderPreparedReopened {
    pub fn root(&self) -> &Path {
        self.root.root()
    }
    pub fn binding(&self) -> &ProviderBootstrapBinding {
        &self.held.binding
    }
    pub fn catalog_id(&self) -> &str {
        &self.record.source.catalog_id
    }
    pub fn revalidate(&self) -> std::result::Result<(), ProviderPreparedOpenFailure> {
        let result = (|| -> std::result::Result<(), report::Failure> {
            let _locks = self
                .held
                .family
                .locks(
                    &self.held.artifacts.allocation.format.identity.worktree_id,
                    false,
                )
                .map_err(report::Failure::from)?;
            sql::final_check(
                &self.held,
                &self.record,
                &self.root,
                self.root.registration().root_id(),
            )
        })();
        result.map_err(|e| {
            ProviderPreparedOpenFailure::from_failure("revalidate reopened provider", e)
        })
    }
}

impl PartialStoreFamily {
    /// Reopens one already prepared root after all earlier Store/native holders
    /// have been released. Caller quiesces namespace, metadata and registration
    /// changes while native READY is checked. No SQL/native mutation is made.
    pub fn open_provider_prepared(
        family_root: impl AsRef<Path>,
        worktree: &WorktreeId,
        transaction: &str,
        expected_binding: &ProviderBootstrapBinding,
    ) -> std::result::Result<ProviderPreparedReopened, ProviderPreparedOpenFailure> {
        let result = open(
            family_root.as_ref(),
            worktree,
            transaction,
            expected_binding,
        );
        result.map_err(|e| ProviderPreparedOpenFailure::from_failure("open prepared provider", e))
    }
}

fn open(
    family_root: &Path,
    worktree: &WorktreeId,
    transaction: &str,
    expected_binding: &ProviderBootstrapBinding,
) -> std::result::Result<ProviderPreparedReopened, report::Failure> {
    let binding = ProviderBootstrapBinding::decode(
        &expected_binding
            .encode()
            .map_err(|e| report::Failure::new(e.into(), None))?,
    )
    .map_err(|e| report::Failure::new(e.into(), None))?;
    let family = FamilyAccess::open(family_root).map_err(report::Failure::from)?;
    let _locks = family
        .locks(worktree, false)
        .map_err(report::Failure::from)?;
    let artifacts =
        Artifacts::open(&family, worktree, transaction).map_err(report::Failure::from)?;
    if artifacts.prepared.is_none() || artifacts.abandoned.is_some() {
        return Err(invalid("reopen requires P and excludes T").into());
    }
    if artifacts.binding().map_err(report::Failure::from)? != binding {
        return Err(invalid("reopen expected bootstrap binding changed").into());
    }
    let guard = match artifacts.retain_control().map_err(report::Failure::from)? {
        RetainedControl::Final(guard) => guard,
        RetainedControl::Stage(_) => {
            return Err(invalid("reopen requires final control directory").into())
        }
    };
    guard
        .revalidate()
        .map_err(|e| report::Failure::new(e.into(), None))?;
    artifacts
        .writer_require_guard(&guard)
        .map_err(report::Failure::from)?;
    artifacts.verify(&family).map_err(report::Failure::from)?;
    let held = outcome::PublicationPins {
        family,
        artifacts,
        binding,
        guard,
    };
    require_initializing(&held)?;
    let state = database_io::read(&held.artifacts.database, |c| {
        validation::private_preparation_reopen_with_source(
            c,
            &held.artifacts.allocation,
            held.artifacts
                .prepared
                .as_ref()
                .ok_or_else(|| invalid("reopen P absent"))?,
            &held.binding,
        )
    })
    .map_err(report::Failure::private)?;
    let interpreted = sql::read_state(state)?;
    let record = interpreted.mount;
    if !matches!(record.state, PreparationState::ProviderPrepared { .. }) {
        return Err(report::Failure::private(invalid(
            "reopen refuses registration pending",
        )));
    }
    let root_id = record
        .root_id()
        .map_err(|e| report::Failure::private(e.into()))?;
    let ticket = record
        .registration()
        .map_err(|e| report::Failure::private(e.into()))?;
    let catalog = Arc::new(interpreted.source.vfs_catalog().clone());
    let root = ticket
        .guard_with_control_directory(catalog, &held.guard)
        .map_err(|e| report::Failure::new(e.into(), None))?;
    sql::final_check(&held, &record, &root, root_id)?;
    Ok(ProviderPreparedReopened { held, root, record })
}

fn require_initializing(
    held: &outcome::PublicationPins,
) -> std::result::Result<(), report::Failure> {
    let scope = Some(ProviderDatabaseScope::FamilyCatalog);
    let result = database_io::read(&held.family.catalog, |c| {
        held.family.validate_catalog(c, &held.artifacts.allocation)
    })
    .map_err(|e| report::Failure::new(e, scope))?;
    match result {
        database_io::ReadState::Valid(Registration::Initializing) => Ok(()),
        database_io::ReadState::Valid(_) => Err(report::Failure::new(
            invalid("reopen family registry not initializing"),
            scope,
        )),
        database_io::ReadState::Uninitialized => Err(report::Failure::new(
            invalid("reopen family catalog uninitialized"),
            scope,
        )),
        database_io::ReadState::Hot(hot) => Err(report::Failure::new(
            Cause::ReadonlyRollback(hot.trigger),
            scope,
        )),
        database_io::ReadState::Rejected { cause, .. }
        | database_io::ReadState::ReadButUnavailable(cause)
        | database_io::ReadState::Unavailable(cause) => Err(report::Failure::new(cause, scope)),
        database_io::ReadState::UnboundRollback { trigger, cause } => {
            let mut failure = report::Failure::new(cause, scope);
            failure.rollback = Some(trigger);
            Err(failure)
        }
    }
}
