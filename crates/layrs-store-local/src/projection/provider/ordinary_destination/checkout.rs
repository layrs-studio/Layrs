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
    read_copied_kernel(
        c,
        marker,
        source,
        &incoming,
        Prejournal::Checkout(&fresh),
        cancel,
    )?
    .ok_or_else(|| invalid("ordinary checkout journal absent"))
}
