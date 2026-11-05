//! Read-only Active/Ready admission including durable capture prefixes.
//! Caller holds lifecycle, owner and catalog gates for the whole operation.
use super::*;
use layrs_vfs::Cancellation;

enum ReadMode {
    Strict,
    Live,
}
enum ReadProof {
    Strict(validation::PrivateActiveCaptureValidation),
    Live(validation::PrivateActivePublicationValidation),
}

pub(super) fn read(
    held: &outcome::PublicationPins,
    native: Option<&layrs_vfs::ProviderRootGuard>,
    cancel: Option<&Cancellation>,
) -> std::result::Result<validation::PrivateActiveCaptureValidation, preparation_report::Failure> {
    match read_mode(held, native, cancel, ReadMode::Strict)? {
        ReadProof::Strict(proof) => Ok(proof),
        ReadProof::Live(_) => unreachable!("strict reader dispatch"),
    }
}

/// Explicit runtime READ interpretation. Ready is readable but remains a
/// pending source transition; Published is historical. The complete outgoing
/// intent/receipt set is retained, and no capture mutation permission follows.
pub(super) fn read_live(
    held: &outcome::PublicationPins,
    native: Option<&layrs_vfs::ProviderRootGuard>,
    cancel: Option<&Cancellation>,
) -> std::result::Result<validation::PrivateActivePublicationValidation, preparation_report::Failure>
{
    match read_mode(held, native, cancel, ReadMode::Live)? {
        ReadProof::Live(proof) => Ok(proof),
        ReadProof::Strict(_) => unreachable!("live reader dispatch"),
    }
}

fn read_mode(
    held: &outcome::PublicationPins,
    native: Option<&layrs_vfs::ProviderRootGuard>,
    cancel: Option<&Cancellation>,
    mode: ReadMode,
) -> std::result::Result<ReadProof, preparation_report::Failure> {
    let verify = || -> CauseResult<()> {
        crate::projection::provider::cancellation::check(cancel)?;
        held.family.verify()?;
        held.artifacts.verify(&held.family)?;
        held.artifacts.writer_require_guard(&held.guard)?;
        held.guard.revalidate()?;
        if held.artifacts.binding()? != held.binding {
            return Err(invalid("capture reader bootstrap binding changed"));
        }
        if let Some(native) = native {
            boundary(held, native)?;
        }
        Ok(())
    };
    verify().map_err(preparation_report::Failure::from)?;
    let inspect_private = |c: &Connection| {
        let prepared = held
            .artifacts
            .prepared
            .as_ref()
            .ok_or_else(|| invalid("capture reader P absent"))?;
        match mode {
            ReadMode::Strict => validation::private_active_readable_with_source(
                c,
                &held.artifacts.allocation,
                prepared,
                &held.binding,
                native.map(|root| root.registration().root_id()),
                cancel,
            )
            .map(|readable| ReadProof::Strict(readable.capture)),
            ReadMode::Live => validation::private_active_publication_with_source(
                c,
                &held.artifacts.allocation,
                prepared,
                &held.binding,
                native.map(|root| root.registration().root_id()),
                cancel,
            )
            .map(ReadProof::Live),
        }
    };
    let state = match cancel {
        Some(cancel) => {
            database_io::read_cancellable(&held.artifacts.database, cancel, inspect_private)
        }
        None => database_io::read(&held.artifacts.database, inspect_private),
    }
    .map_err(preparation_report::Failure::private)?;
    let private = read_state(state, ProviderDatabaseScope::PrivateMetadata)?;
    let inspect_family = |c: &Connection| match &private {
        ReadProof::Strict(proof) => held.family.validate_capture_catalog(
            c,
            &held.artifacts.allocation,
            &proof.capture,
            cancel,
        ),
        ReadProof::Live(proof) => held.family.validate_source_companion_catalog(
            c,
            &held.artifacts.allocation,
            &crate::projection::shared::source_companion::SourceCompanionFamilyContext {
                publication: proof,
                pending: None,
            },
            cancel,
        ),
    };
    let state = match cancel {
        Some(cancel) => database_io::read_cancellable(&held.family.catalog, cancel, inspect_family),
        None => database_io::read(&held.family.catalog, inspect_family),
    }
    .map_err(|cause| {
        preparation_report::Failure::new(cause, Some(ProviderDatabaseScope::FamilyCatalog))
    })?;
    if read_state(state, ProviderDatabaseScope::FamilyCatalog)? != ActivationRegistration::Ready {
        return Err(preparation_report::Failure::new(
            invalid("capture reader requires ready family registration"),
            Some(ProviderDatabaseScope::FamilyCatalog),
        ));
    }
    verify().map_err(preparation_report::Failure::from)?;
    Ok(private)
}

/// Match the retained Active owner while keeping its complete publication
/// proof. This is read-only and deliberately does not require no pending row.
pub(super) fn require_exact_live(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
) -> std::result::Result<validation::PrivateActivePublicationValidation, preparation_report::Failure>
{
    let state = read_live(held, Some(root), None)?;
    if state.active.mount != *mount || state.active.activation != *activation {
        return Err(preparation_report::Failure::private(invalid(
            "capture reader active map changed",
        )));
    }
    Ok(state)
}

pub(super) fn require_exact(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
) -> std::result::Result<(), preparation_report::Failure> {
    let state = read(held, Some(root), None)?;
    if state.active.mount != *mount || state.active.activation != *activation {
        return Err(preparation_report::Failure::private(invalid(
            "capture reader active map changed",
        )));
    }
    Ok(())
}
