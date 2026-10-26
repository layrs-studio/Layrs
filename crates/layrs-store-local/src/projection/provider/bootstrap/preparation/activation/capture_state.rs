//! Read-only Active/Ready admission including durable capture prefixes.
//! Caller holds lifecycle, owner and catalog gates for the whole operation.
use super::*;
use layrs_vfs::Cancellation;

pub(super) fn read(
    held: &outcome::PublicationPins,
    native: Option<&layrs_vfs::ProviderRootGuard>,
    cancel: Option<&Cancellation>,
) -> std::result::Result<validation::PrivateActiveCaptureValidation, preparation_report::Failure> {
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
        validation::private_active_capture_with_source(
            c,
            &held.artifacts.allocation,
            held.artifacts
                .prepared
                .as_ref()
                .ok_or_else(|| invalid("capture reader P absent"))?,
            &held.binding,
            native.map(|root| root.registration().root_id()),
            cancel,
        )
    };
    let state = match cancel {
        Some(cancel) => {
            database_io::read_cancellable(&held.artifacts.database, cancel, inspect_private)
        }
        None => database_io::read(&held.artifacts.database, inspect_private),
    }
    .map_err(preparation_report::Failure::private)?;
    let private = read_state(state, ProviderDatabaseScope::PrivateMetadata)?;
    let inspect_family = |c: &Connection| {
        held.family.validate_capture_catalog(
            c,
            &held.artifacts.allocation,
            &private.capture,
            cancel,
        )
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
