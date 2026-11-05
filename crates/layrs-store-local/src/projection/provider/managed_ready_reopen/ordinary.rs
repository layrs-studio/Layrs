//! Fresh, clean, read-only Ordinary terminal inspection. These holders acquire
//! their own lifecycle and C owner; they never open B or any writer. Drop the
//! holder before opening an ordinary mutable session. Hot/cold rollback and
//! failed/unknown closes remain refusals, never recovery or a rollback ACK.
use super::*;
use crate::projection::{
    provider::{
        admitted_ordinary::{self, AdmittedOrdinaryData},
        destination_ready::read_ordinary_managed_ready,
        managed_admission::{RequiredRepresentation, read_admitted_import},
    },
    shared::{
        admitted_catalog::{AdmittedFamilyContext, validate_terminal_ready_catalog},
        exact_content::{ExactBinding, ExactInstallState},
        exact_protection::ExactProtection,
    },
};

/// Genuine Ordinary C8 Ready plus independently acquired immutable physical
/// proofs. No admission writer or lock upgrade is available from this holder.
pub(in crate::projection::provider) struct OrdinaryReadyInspection {
    pub(super) pins: ManagedReadyPins,
    pub(super) data: OwnedReadySnapshot,
    cas: PublishedCasProof,
    checkout: VerifiedCheckout,
    _owner: StoreLock,
    _lifecycle: StoreLock,
}

impl OrdinaryReadyInspection {
    pub(in crate::projection::provider) fn open(
        root: &Path,
        cancel: &Cancellation,
    ) -> Result<Self> {
        cancel.check()?;
        let pins = ManagedReadyPins::open(root, cancel)?;
        Self::open_pins(pins, SessionMode::Shared, false, cancel)
    }

    /// A separate acquisition, never an upgrade of a shared inspection. The
    /// fresh admission caller checks that these pins are distinct from B
    /// before attempting the exclusive C owner.
    pub(super) fn open_exclusive(pins: ManagedReadyPins, cancel: &Cancellation) -> Result<Self> {
        Self::open_pins(pins, SessionMode::Exclusive, false, cancel)
    }

    pub(super) fn open_shared(pins: ManagedReadyPins, cancel: &Cancellation) -> Result<Self> {
        Self::open_pins(pins, SessionMode::Shared, true, cancel)
    }

    fn open_pins(
        pins: ManagedReadyPins,
        mode: SessionMode,
        strict: bool,
        cancel: &Cancellation,
    ) -> Result<Self> {
        cancel.check()?;
        let lifecycle = pins.family.lifecycle_lease()?;
        let owner = StoreLock::acquire_existing(&pins.metadata_path().join("layrs.lock"), mode)?;
        let data = read_ready_snapshot_mode(&pins, strict, cancel)?;
        pins.require_data(&data, cancel)?;
        // All SQL scopes closed positively before any full byte/namespace I/O.
        let storage = ExactStorage::open(
            pins.family.root(),
            &data.binding.family_root_identity,
            pins.family.lifecycle_lease()?,
        )?;
        let cas = verify_published_cas(storage, &data.binding, &data.required, cancel)?;
        pins.require_data(&data, cancel)?;
        let checkout = verify_managed_ready_checkout(
            pins.root.try_clone()?,
            pins.control.try_clone()?,
            &data.expected,
            &data.journal,
            cancel,
        )?;
        cas.require(&data.binding, &data.required, cancel)?;
        checkout.require(&data.expected, &data.journal, cancel)?;
        let current = read_ready_snapshot_mode(&pins, strict, cancel)?;
        data.require_same(&current)?;
        pins.require_data(&data, cancel)?;
        cas.require(&data.binding, &data.required, cancel)?;
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
        let current = read_ready_snapshot(&self.pins, cancel)?;
        self.data.require_same(&current)?;
        self.require_physical(cancel)
    }

    pub(in crate::projection::provider) fn require_physical(
        &self,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.pins.require_data(&self.data, cancel)?;
        self.cas
            .require(&self.data.binding, &self.data.required, cancel)?;
        self.checkout
            .require(&self.data.expected, &self.data.journal, cancel)
    }

    /// Identity-only boundary for retained SQL scopes and post-COMMIT close.
    /// Full namespace checks remain outside every catalog/SQL scope.
    pub(in crate::projection::provider) fn require_identity(
        &self,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.pins.require_data(&self.data, cancel)?;
        self.cas.verify_identity(cancel)?;
        self.checkout
            .require_identity(&self.data.expected, &self.data.journal, cancel)
    }

    pub(in crate::projection::provider) fn root(&self) -> &Path {
        self.pins.checkout_path()
    }
    pub(in crate::projection::provider) fn ready(&self) -> &ProviderTransitionIntentRecord {
        &self.data.ready
    }
    pub(in crate::projection::provider) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.data.receipt
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
    pub(in crate::projection::provider) fn snapshot(&self) -> &OwnedReadySnapshot {
        &self.data
    }
    pub(in crate::projection::provider) fn database(&self) -> &database_io::DatabaseFile {
        &self.pins.database
    }
    pub(in crate::projection::provider) fn family(
        &self,
    ) -> &crate::projection::shared::provider_access::FamilyAccess {
        &self.pins.family
    }
}

/// Authenticated C10 DATA is distinct from C8 Ready DATA. ExpectedCheckout and
/// journal are moved out of the actual archived interpreter, never rebuilt
/// from the detached admitted summary. Canonical manifests come from family.
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
            return Err(invalid("detached terminal Ordinary10 snapshot changed"));
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

/// Resolves only an unchanged terminal C10 after all former owners/holders
/// have gone. It acquires its own exclusive C owner and immutable CAS/checkout
/// handles. Success reports authenticated bounded DATA, not a writer ACK or
/// authority to edit; require failures retain this holder and its owner.
pub(in crate::projection::provider) struct OrdinaryTerminalResolution {
    pins: ManagedReadyPins,
    data: TerminalSnapshot,
    cas: PublishedCasProof,
    checkout: VerifiedCheckout,
    _owner: StoreLock,
    _lifecycle: StoreLock,
}

impl OrdinaryTerminalResolution {
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

fn read_ready_snapshot(
    pins: &ManagedReadyPins,
    cancel: &Cancellation,
) -> Result<OwnedReadySnapshot> {
    read_ready_snapshot_mode(pins, false, cancel)
}

fn read_ready_snapshot_mode(
    pins: &ManagedReadyPins,
    strict: bool,
    cancel: &Cancellation,
) -> Result<OwnedReadySnapshot> {
    pins.verify(cancel)?;
    let result = {
        let _catalog = StoreLock::acquire_existing(
            &pins.family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )?;
        pins.verify(cancel)?;
        // Lifecycle -> one C owner -> catalog. No lock reacquisition/upgrade.
        let result = valid(read_ready_database(&pins.database, strict, cancel, |c| {
            valid(read_ready_database(
                &pins.family.catalog,
                strict,
                cancel,
                |family| {
                    let data = read_ordinary_managed_ready(c, &pins.marker, family, Some(cancel))?
                        .into_snapshot()?;
                    pins.require_data(&data, cancel)?;
                    Ok(data)
                },
            )?)
        })?)?;
        // A failed/unknown inner close prevents all further SQL; a failed
        // outer close prevents all following physical namespace verification.
        pins.verify(cancel)?;
        result
    };
    pins.require_data(&result, cancel)?;
    Ok(result)
}

fn read_ready_database<T>(
    database: &database_io::DatabaseFile,
    strict: bool,
    cancel: &Cancellation,
    validate: impl FnOnce(&rusqlite::Connection) -> Result<T>,
) -> Result<ReadState<T>> {
    if strict {
        database_io::read_cancellable_strict(database, cancel, validate)
    } else {
        database_io::read_cancellable(database, cancel, validate)
    }
}

fn read_terminal_snapshot(
    pins: &ManagedReadyPins,
    cancel: &Cancellation,
) -> Result<TerminalSnapshot> {
    pins.verify(cancel)?;
    let result = {
        let _catalog = StoreLock::acquire_existing(
            &pins.family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )?;
        pins.verify(cancel)?;
        let result = valid(database_io::read_cancellable(
            &pins.database,
            cancel,
            |c| {
                let admitted = admitted_ordinary::inspect(c, &pins.marker, Some(cancel))?;
                if admitted.private_schema() != 10 {
                    return Err(invalid("terminal Ordinary resolution requires exact C10"));
                }
                let context = AdmittedFamilyContext::new(&admitted)?;
                let proof = read_admitted_import(c, &pins.marker, Some(cancel))?;
                valid(database_io::read_cancellable(
                    &pins.family.catalog,
                    cancel,
                    |family| {
                        let required = context
                            .validate_catalog(family, pins.family.marker_bytes(), Some(cancel))?
                            .into_manifests();
                        let mut witness: Vec<_> = required
                            .iter()
                            .map(|m| RequiredRepresentation {
                                file_id: m.file_id().clone(),
                                manifest_id: m.id().clone(),
                                bytes: m.byte_length(),
                            })
                            .collect();
                        witness.sort_unstable();
                        if proof.private_schema() != 10 || proof.required() != witness {
                            return Err(invalid(
                                "terminal Ordinary family differs from actual C10 witness",
                            ));
                        }
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
                            return Err(invalid("terminal Ordinary family protection differs"));
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
                            proof.into_ordinary_terminal(&required, installation, Some(cancel))?;
                        admitted.require_same_import(&current)?;
                        let data = TerminalSnapshot {
                            admitted: current,
                            expected,
                            journal,
                            required,
                            installation,
                        };
                        // Under the catalog gate: identity comparisons only, never
                        // full checkout namespace enumeration or content verification.
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
