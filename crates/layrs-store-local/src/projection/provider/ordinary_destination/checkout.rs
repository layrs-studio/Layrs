//! Explicit checkout-progress inspection, backed by actual SourceStaging and
//! exact C8 DestinationStaging. No proposal, opener or physical rights escape.
//! A complete row inventory may carry any legal progress phase; publication
//! completion and physical checkout verification belong to the caller.
use super::*;

pub(in crate::projection::provider) fn read_checkout_staging(
    c: &Connection,
    marker: &format::Marker,
    source: &OrdinaryCopyInputs<'_>,
    expected: &ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<CheckoutJournal> {
    read_checkout(c, marker, source, expected, false, cancel)?
        .ok_or_else(|| invalid("ordinary checkout journal absent"))
}

/// Actual C Staging with either no journal or a complete typed row inventory
/// in any legal progress phase. None describes SQL absence only; it grants no
/// ownership of a checkout namespace or right to adopt filesystem objects.
pub(in crate::projection::provider) fn read_checkout_progress(
    c: &Connection,
    marker: &format::Marker,
    source: &OrdinaryCopyInputs<'_>,
    expected: &ExpectedCheckout,
    cancel: Option<&Cancellation>,
) -> Result<Option<Box<CheckoutJournal>>> {
    Ok(read_checkout(c, marker, source, expected, true, cancel)?.map(Box::new))
}

fn read_checkout(
    c: &Connection,
    marker: &format::Marker,
    source: &OrdinaryCopyInputs<'_>,
    expected: &ExpectedCheckout,
    allow_absent: bool,
    cancel: Option<&Cancellation>,
) -> Result<Option<CheckoutJournal>> {
    let incoming = source.require_phase(c, marker, TransitionState::Staging, cancel)?;
    // Rebuild the bound header from the actual durable source's canonical
    // destination expectation. Caller inventory is DATA, not source authority.
    let fresh = ExpectedCheckout::ordinary(
        &incoming,
        expected.files(),
        expected.deletions(),
        &expected.header().image_digest,
        cancel,
    )?;
    if fresh.header() != expected.header()
        || fresh.marker() != expected.marker()
        || fresh.files() != expected.files()
        || fresh.directories() != expected.directories()
        || fresh.deletions() != expected.deletions()
    {
        return Err(invalid(
            "ordinary checkout expectation differs from actual Staging",
        ));
    }
    if allow_absent {
        // The bounded discriminant is queried only after authenticating B and
        // C's exact schema. Any row selects the strict full journal reader.
        import_validation::require_schema(c, marker, ImportDatabaseKind::Provider, cancel)?;
        let present: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_prejournal)",
            [],
            |row| row.get(0),
        )?;
        if !present {
            if staging::read_stageable(c, marker, source, cancel)? != TransitionState::Staging {
                return Err(invalid(
                    "ordinary checkout progress requires destination Staging",
                ));
            }
            cancellation::check(cancel)?;
            return Ok(None);
        }
    }
    read_copied_kernel(
        c,
        marker,
        source,
        &incoming,
        Prejournal::Checkout(&fresh),
        cancel,
    )
}
