//! Fresh physical resolution of an unchanged Provider9 C14 terminal. Success
//! reports authenticated DATA while retaining an exclusive destination owner
//! and immutable CAS/checkout pins. It grants no Capture/Restore/export access.
use super::*;
use crate::projection::{
    provider::{
        admitted_ordinary::{self, AdmittedOrdinaryData},
        managed_admission::read_admitted_import,
    },
    shared::{
        admitted_catalog::{validate_terminal_ready_catalog, AdmittedFamilyContext},
        exact_content::{ExactBinding, ExactInstallState},
        exact_protection::ExactProtection,
    },
};

struct TerminalSnapshot {
    admitted: AdmittedOrdinaryData,
    expected: ExpectedCheckout,
    journal: CheckoutJournal,
    required: Vec<FileManifest>,
    installation: ExactInstallState,
}

impl TerminalSnapshot {
    fn require_same(&self, current: &Self) -> Result<()> {
        self.admitted.require_same_import(&current.admitted)?;
        if self.expected.header() != current.expected.header()
            || self.journal.rows() != current.journal.rows()
            || self.required != current.required
            || self.installation != current.installation
        {
            return Err(invalid("detached terminal denial snapshot changed"));
        }
        Ok(())
    }

    fn binding(&self) -> &ExactBinding {
        self.admitted.binding()
    }

    fn require_pins(&self, pins: &ManagedReadyPins, cancel: &Cancellation) -> Result<()> {
        pins.require_parts(
            self.admitted.marker(),
            self.admitted.ready(),
            &self.expected,
            self.binding(),
            cancel,
        )
    }
}

/// Independent terminal Provider9 resolution after former holders are dropped.
/// No source connection, writer, admission ACK or lock upgrade is available.
/// Revalidation failures keep this holder's owner and immutable physical pins.
pub(in crate::projection::provider) struct DenialTerminalResolution {
    pins: ManagedReadyPins,
    data: Box<TerminalSnapshot>,
    cas: PublishedCasProof,
    checkout: VerifiedCheckout,
    _owner: StoreLock,
    _lifecycle: StoreLock,
}

impl DenialTerminalResolution {
    pub(in crate::projection::provider) fn open(
        root: &Path,
        cancel: &Cancellation,
    ) -> Result<Self> {
        cancel.check()?;
        let pins = ManagedReadyPins::open(root, cancel)?;
        let lifecycle = pins.family.lifecycle_lease()?;
        let owner = StoreLock::acquire_existing(
            &pins.metadata_path().join("layrs.lock"),
            SessionMode::Exclusive,
        )?;
        let data = read_terminal_snapshot(&pins, cancel)?;
        data.require_pins(&pins, cancel)?;
        // All retained SQL scopes positively close before byte verification or
        // full checkout namespace enumeration. No source owner is reacquired.
        let storage = ExactStorage::open(
            pins.family.root(),
            &data.binding().family_root_identity,
            pins.family.lifecycle_lease()?,
        )?;
        let cas = verify_published_cas(storage, data.binding(), &data.required, cancel)?;
        data.require_pins(&pins, cancel)?;
        let checkout = verify_managed_ready_checkout(
            pins.root.try_clone()?,
            pins.control.try_clone()?,
            &data.expected,
            &data.journal,
            cancel,
        )?;
        cas.require(data.binding(), &data.required, cancel)?;
        checkout.require(&data.expected, &data.journal, cancel)?;
        let current = read_terminal_snapshot(&pins, cancel)?;
        data.require_same(&current)?;
        data.require_pins(&pins, cancel)?;
        cas.require(data.binding(), &data.required, cancel)?;
        checkout.require(&data.expected, &data.journal, cancel)?;
        cancel.check()?;
        Ok(Self {
            pins,
            data,
            cas,
            checkout,
            _owner: owner,
            _lifecycle: lifecycle,
        })
    }

    pub(in crate::projection::provider) fn require(&self, cancel: &Cancellation) -> Result<()> {
        self.require_physical(cancel)?;
        let current = read_terminal_snapshot(&self.pins, cancel)?;
        self.data.require_same(&current)?;
        self.require_physical(cancel)
    }

    fn require_physical(&self, cancel: &Cancellation) -> Result<()> {
        self.data.require_pins(&self.pins, cancel)?;
        self.cas
            .require(self.data.binding(), &self.data.required, cancel)?;
        self.checkout
            .require(&self.data.expected, &self.data.journal, cancel)
    }

    pub(in crate::projection::provider) fn root(&self) -> &Path {
        self.pins.checkout_path()
    }
    pub(in crate::projection::provider) fn admission_digest(&self) -> &str {
        self.data.admitted.admission_digest()
    }
    pub(in crate::projection::provider) fn admitted(&self) -> &AdmittedOrdinaryData {
        &self.data.admitted
    }
    pub(in crate::projection::provider) fn ready(&self) -> &ProviderTransitionIntentRecord {
        self.data.admitted.ready()
    }
    pub(in crate::projection::provider) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        self.data.admitted.receipt()
    }
    pub(in crate::projection::provider) fn expected(&self) -> &ExpectedCheckout {
        &self.data.expected
    }
    pub(in crate::projection::provider) fn journal(&self) -> &CheckoutJournal {
        &self.data.journal
    }
    pub(in crate::projection::provider) fn required(&self) -> &[FileManifest] {
        &self.data.required
    }
}

fn read_terminal_snapshot(
    pins: &ManagedReadyPins,
    cancel: &Cancellation,
) -> Result<Box<TerminalSnapshot>> {
    pins.verify(cancel)?;
    let result = {
        // Lifecycle -> fresh exclusive destination owner -> short catalog gate.
        let _catalog = StoreLock::acquire_existing(
            &pins.family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )?;
        pins.verify(cancel)?;
        let result = valid(database_io::read_cancellable_strict(
            &pins.database,
            cancel,
            |c| {
                let admitted = admitted_ordinary::inspect(c, &pins.marker, Some(cancel))?;
                if admitted.private_schema() != 9
                    || admitted.ready().plan.denial_coverage.is_none()
                    || admitted.receipt().denial_coverage.is_none()
                {
                    return Err(invalid(
                        "terminal denial resolution requires exact Provider9 C14",
                    ));
                }
                let context = AdmittedFamilyContext::new(&admitted)?;
                let proof = read_admitted_import(c, &pins.marker, Some(cancel))?;
                valid(database_io::read_cancellable_strict(
                    &pins.family.catalog,
                    cancel,
                    |family| {
                        // Canonical manifests come from the complete current
                        // family interpretation, never from private references.
                        let required = context
                            .validate_catalog(family, pins.family.marker_bytes(), Some(cancel))?
                            .into_manifests();
                        let binding = admitted.binding();
                        let protection = ExactProtection::new(
                            &binding.family_incarnation,
                            &binding.owner,
                            &binding.database_identity,
                            &binding.transaction_id,
                            &required,
                            Some(cancel),
                        )?;
                        if protection.id() != admitted.root_id() {
                            return Err(invalid("terminal denial family protection differs"));
                        }
                        let installation = validate_terminal_ready_catalog(
                            family,
                            pins.family.marker_bytes(),
                            &pins.marker,
                            binding,
                            &required,
                            &protection,
                            Some(cancel),
                        )?;
                        let (current, expected, journal) =
                            proof.into_denial_terminal(&required, installation, Some(cancel))?;
                        admitted.require_same_import(&current)?;
                        // Keep complete detached DATA on the heap across both
                        // SQL close/result layers; no proof lifetime changes.
                        let data = Box::new(TerminalSnapshot {
                            admitted: current,
                            expected,
                            journal,
                            required,
                            installation,
                        });
                        // Only physical identity comparisons under SQL/catalog;
                        // checkout enumeration and CAS hashing happen outside.
                        data.require_pins(pins, cancel)?;
                        Ok(data)
                    },
                )?)
            },
        )?)?;
        pins.verify(cancel)?;
        result
    };
    result.require_pins(pins, cancel)?;
    Ok(result)
}
