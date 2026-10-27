use super::*;

/// Resolved origin DATA from a genuine admitted lineage, never a fake legacy
/// ConversionReceipt. Both inclusive generation bounds belong to that origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::projection) struct CaptureOrigin {
    identity: ProjectionIdentity,
    minimum_generation: u64,
    maximum_generation: u64,
    imported: bool,
}
impl CaptureOrigin {
    pub(in crate::projection) fn identity(&self) -> &ProjectionIdentity {
        &self.identity
    }
    pub(in crate::projection) fn minimum_generation(&self) -> u64 {
        self.minimum_generation
    }
    pub(in crate::projection) fn maximum_generation(&self) -> u64 {
        self.maximum_generation
    }
    pub(in crate::projection) fn imported(&self) -> bool {
        self.imported
    }
    fn require_patch(&self, patch: &CapturedPatch) -> Result<()> {
        if patch.identity != self.identity
            || patch.projection_generation < self.minimum_generation
            || patch.projection_generation > self.maximum_generation
        {
            return Err(invalid(
                "admitted capture escaped its exact origin interval",
            ));
        }
        Ok(())
    }
}

fn resolve(
    proof: &VerifiedAdmittedImport<'_>,
    c: &Connection,
    sequence: u64,
    cancel: Option<&Cancellation>,
) -> Result<CaptureOrigin> {
    let (identity, minimum_generation, maximum_generation) =
        proof.capture_origin(c, sequence, cancel)?;
    Ok(CaptureOrigin {
        identity: identity.clone(),
        minimum_generation,
        maximum_generation,
        imported: sequence <= proof.record().boundaries.imported_capture_count,
    })
}

pub(in crate::projection) fn capture_origin(
    c: &Connection,
    marker: &format::Marker,
    sequence: u64,
    cancel: Option<&Cancellation>,
) -> Result<CaptureOrigin> {
    snapshot::with_snapshot(c, cancel, || {
        let proof = read_admitted_import(c, marker, cancel)?;
        resolve(&proof, c, sequence, cancel)
    })
}

/// Called by the normal capture validator after its bounded canonical load.
/// Do not call a provenance-dispatching capture loader from this function.
pub(in crate::projection) fn validate_capture_identity(
    c: &Connection,
    marker: &format::Marker,
    patch: &CapturedPatch,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    capture_origin(c, marker, patch.sequence, cancel)?.require_patch(patch)
}

pub(in crate::projection) fn imported(
    c: &Connection,
    marker: &format::Marker,
    sequence: u64,
    cancel: Option<&Cancellation>,
) -> Result<bool> {
    snapshot::with_snapshot(c, cancel, || {
        let proof = read_admitted_import(c, marker, cancel)?;
        Ok(sequence > 0 && sequence <= proof.record().boundaries.imported_capture_count)
    })
}

/// Resolve the actual ancestor, then use origin-only low-level interpreters.
/// This path cannot recurse through conversion::provenance or content leases.
pub(in crate::projection) fn imported_publication(
    c: &Connection,
    marker: &format::Marker,
    sequence: u64,
    cancel: Option<&Cancellation>,
) -> Result<Option<(ProjectionIdentity, CompletedPublication)>> {
    snapshot::with_snapshot(c, cancel, || {
        let proof = read_admitted_import(c, marker, cancel)?;
        if sequence == 0 || sequence > proof.record().boundaries.imported_capture_count {
            return Ok(None);
        }
        let origin = resolve(&proof, c, sequence, cancel)?;
        let patch = crate::projection::capture::storage::load_metadata_for_origin(
            c,
            sequence,
            origin.identity(),
            origin.maximum_generation(),
        )?;
        origin.require_patch(&patch)?;
        let completed = crate::projection::publication::storage::completed_record_for_origin(
            c,
            sequence,
            origin.identity(),
            origin.maximum_generation(),
        )?;
        if completed.as_ref().is_some_and(|(publication, _)| {
            publication.projection.generation < origin.minimum_generation
                || publication.projection.generation > origin.maximum_generation
        }) {
            return Err(invalid(
                "admitted inherited publication escaped its exact origin interval",
            ));
        }
        cancellation::check(cancel)?;
        Ok(completed.map(|(publication, _)| (origin.identity, publication)))
    })
}
