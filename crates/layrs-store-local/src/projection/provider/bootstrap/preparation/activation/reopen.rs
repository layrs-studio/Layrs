//! Read-only recovery of an already activated root and explicit durable resume.
use super::*;
use std::{error::Error, fmt, path::Path, sync::Arc};

/// Fresh owner reconstructed from complete private/family DATA and native READY.
/// No prior writer report or resident content source is reconstructed.
pub struct ProviderActiveReopened {
    pub(super) held: outcome::PublicationPins,
    pub(super) root: layrs_vfs::ProviderRootGuard,
    pub(super) record: ProviderActiveMountRecord,
    pub(super) activation: ProviderActivationRecord,
}

#[derive(Debug)]
pub struct ProviderActiveOpenFailure {
    pub kind: ProviderActivationFailureKind,
    pub operation: &'static str,
    pub database: Option<ProviderDatabaseScope>,
    pub cause: Cause,
    pub readonly_rollback: Option<Arc<rusqlite::Error>>,
    pub rollback_triggers: Vec<recovery::ProviderRollbackTrigger>,
}

impl fmt::Display for ProviderActiveOpenFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "provider active {} ({:?}): {}",
            self.operation, self.kind, self.cause
        )
    }
}
impl Error for ProviderActiveOpenFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}
impl ProviderActiveOpenFailure {
    pub(super) fn from_failure(
        operation: &'static str,
        failure: preparation_report::Failure,
    ) -> Self {
        Self {
            kind: failure.kind().into(),
            operation,
            database: failure.database,
            cause: failure.cause,
            readonly_rollback: failure.rollback,
            rollback_triggers: failure.rollback_triggers,
        }
    }
}

impl ProviderActiveReopened {
    pub fn root(&self) -> &Path {
        self.root.root()
    }
    pub fn binding(&self) -> &ProviderBootstrapBinding {
        &self.held.binding
    }
    pub fn catalog_id(&self) -> &str {
        &self.record.prepared.source.catalog_id
    }
    pub fn generation(&self) -> u64 {
        self.activation.activated_generation
    }
    pub fn digest(&self) -> &str {
        &self.activation.activated_digest
    }
    pub fn revalidate(&self) -> std::result::Result<(), ProviderActiveOpenFailure> {
        let result = (|| -> std::result::Result<(), preparation_report::Failure> {
            let _locks = self
                .held
                .family
                .locks(
                    &self.held.artifacts.allocation.format.identity.worktree_id,
                    false,
                )
                .map_err(preparation_report::Failure::from)?;
            require_exact_state(
                &self.held,
                &self.root,
                &self.record,
                &self.activation,
                ActivationRegistration::Ready,
            )
        })();
        result.map_err(|e| {
            ProviderActiveOpenFailure::from_failure("revalidate reopened active provider", e)
        })
    }
}

impl PartialStoreFamily {
    /// Reopen an already Active/Ready provider after prior holders are dropped.
    /// Caller keeps namespace, metadata and native registration quiescent. This
    /// operation does not create, populate or write any provider state.
    pub fn open_provider_active(
        family_root: impl AsRef<Path>,
        worktree: &WorktreeId,
        transaction: &str,
        expected_binding: &ProviderBootstrapBinding,
    ) -> std::result::Result<ProviderActiveReopened, ProviderActiveOpenFailure> {
        let recovered = open(
            family_root.as_ref(),
            worktree,
            transaction,
            expected_binding,
            RequiredFamily::Ready,
        )
        .map_err(|e| ProviderActiveOpenFailure::from_failure("open active provider", e))?;
        Ok(ProviderActiveReopened {
            held: recovered.held,
            root: recovered.root,
            record: recovered.record,
            activation: recovered.activation,
        })
    }

    /// Explicitly finish an Active private transition left before family READY,
    /// or reobserve an already Ready transition. Admission is strictly read-only;
    /// after admission the consuming writer owns the recovered pins and proof.
    pub fn resume_provider_activation(
        family_root: impl AsRef<Path>,
        worktree: &WorktreeId,
        transaction: &str,
        expected_binding: &ProviderBootstrapBinding,
    ) -> std::result::Result<ProviderActivationOutcome, ProviderActiveOpenFailure> {
        let recovered = open(
            family_root.as_ref(),
            worktree,
            transaction,
            expected_binding,
            RequiredFamily::InitializingOrReady,
        )
        .map_err(|e| {
            ProviderActiveOpenFailure::from_failure("resume active provider admission", e)
        })?;
        // The prior write report is not durable. Empty history marks this as a
        // recovered invocation and never claims an earlier SQL effect.
        let recovered_report = ProviderActivationWriteReport {
            provenance: ProviderActivationWriteReportProvenance::ResumedInvocation,
            resumed: true,
            previous_effects: Vec::new(),
            effects: Vec::new(),
        };
        Ok(super::activate(
            recovered.held,
            recovered.root,
            recovered.record.prepared,
            Some(recovered_report),
        ))
    }
}

struct Recovered {
    held: outcome::PublicationPins,
    root: layrs_vfs::ProviderRootGuard,
    record: ProviderActiveMountRecord,
    activation: ProviderActivationRecord,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum RequiredFamily {
    Ready,
    InitializingOrReady,
}

fn open(
    family_root: &Path,
    worktree: &WorktreeId,
    transaction: &str,
    expected_binding: &ProviderBootstrapBinding,
    required: RequiredFamily,
) -> std::result::Result<Recovered, preparation_report::Failure> {
    let binding = ProviderBootstrapBinding::decode(
        &expected_binding
            .encode()
            .map_err(|e| preparation_report::Failure::new(e.into(), None))?,
    )
    .map_err(|e| preparation_report::Failure::new(e.into(), None))?;
    let family = FamilyAccess::open(family_root).map_err(preparation_report::Failure::from)?;
    let _locks = family
        .locks(worktree, false)
        .map_err(preparation_report::Failure::from)?;
    let artifacts = Artifacts::open(&family, worktree, transaction)
        .map_err(preparation_report::Failure::from)?;
    if artifacts.prepared.is_none() || artifacts.abandoned.is_some() {
        return Err(invalid("active reopen requires P and excludes T").into());
    }
    if artifacts
        .binding()
        .map_err(preparation_report::Failure::from)?
        != binding
    {
        return Err(invalid("active reopen expected bootstrap binding changed").into());
    }
    let guard = match artifacts
        .retain_control()
        .map_err(preparation_report::Failure::from)?
    {
        RetainedControl::Final(guard) => guard,
        RetainedControl::Stage(_) => {
            return Err(invalid("active reopen requires final control directory").into());
        }
    };
    guard
        .revalidate()
        .map_err(|e| preparation_report::Failure::new(e.into(), None))?;
    artifacts
        .writer_require_guard(&guard)
        .map_err(preparation_report::Failure::from)?;
    artifacts
        .verify(&family)
        .map_err(preparation_report::Failure::from)?;
    let held = outcome::PublicationPins {
        family,
        artifacts,
        binding,
        guard,
    };
    let (observed_family, interpreted) = if required == RequiredFamily::Ready {
        let capture = capture_state::read_live(&held, None, None)?;
        // Both complete SQL reads and positive closes precede projecting plain
        // mount/activation DATA into the holder. Every runtime use below calls
        // the explicit live reader again; no capture permission is constructed.
        (ActivationRegistration::Ready, capture.active)
    } else {
        // Activation recovery remains strict: capture starts only after READY.
        let family = super::family_state(&held)?;
        let private = database_io::read(&held.artifacts.database, |c| {
            validation::private_active_with_source(
                c,
                &held.artifacts.allocation,
                held.artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("active reopen P absent"))?,
                &held.binding,
                None,
                None,
            )
        })
        .map_err(preparation_report::Failure::private)?;
        (
            family,
            super::read_state(private, ProviderDatabaseScope::PrivateMetadata)?,
        )
    };
    let record = interpreted.mount;
    let activation = interpreted.activation;
    let ticket = record
        .prepared
        .registration()
        .map_err(|e| preparation_report::Failure::private(e.into()))?;
    let catalog = Arc::new(interpreted.source.vfs_catalog().clone());
    let root = ticket
        .guard_with_control_directory(catalog, &held.guard)
        .map_err(|e| preparation_report::Failure::new(e.into(), None))?;
    require_exact_state(&held, &root, &record, &activation, observed_family)?;
    Ok(Recovered {
        held,
        root,
        record,
        activation,
    })
}

fn require_exact_state(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    record: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    family: ActivationRegistration,
) -> std::result::Result<(), preparation_report::Failure> {
    if family == ActivationRegistration::Ready {
        return capture_state::require_exact_live(held, root, record, activation).map(|_| ());
    }
    super::boundary(held, root).map_err(preparation_report::Failure::from)?;
    if super::family_state(held)? != family {
        return Err(preparation_report::Failure::new(
            invalid("active family registration changed"),
            Some(ProviderDatabaseScope::FamilyCatalog),
        ));
    }
    let super::PrivateState::Active(checked_mount, checked_activation) =
        super::private_state(held, root, &record.prepared)?
    else {
        return Err(preparation_report::Failure::private(invalid(
            "active private mount changed",
        )));
    };
    if checked_mount != *record || checked_activation != *activation {
        return Err(preparation_report::Failure::private(invalid(
            "active private placement changed",
        )));
    }
    super::boundary(held, root).map_err(preparation_report::Failure::from)
}
