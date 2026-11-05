//! Exact installation under the retained ordinary source snapshot.
use super::{allocation::ManagedDestinationAllocation, content::ContentPhase, *};
use crate::projection::{provider::database_io::WriterCloseObserver, shared::exact_content};

impl OrdinaryManagedExport<'_> {
    pub(in crate::projection::provider) fn install_content(
        &mut self,
        destination: &ManagedDestinationAllocation,
        staging: &ProviderTransitionIntentRecord,
        id: &FileManifestId,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> Result<FileManifest> {
        self.with_content_source(
            destination,
            staging,
            ContentPhase::Installing,
            close,
            cancel,
            |catalog| {
                let mut attempt = exact_content::prepare(catalog, id, cancel)?;
                let manifest = attempt.manifest().clone();
                if !attempt.published() {
                    while let Some(mut piece) =
                        exact_content::prepare_piece(catalog, &mut attempt, cancel)?
                    {
                        if piece.needs_produce() {
                            let ordinal = piece.ordinal();
                            piece.produce(
                                |sink| catalog.copy_chunk(&manifest, ordinal, sink, cancel),
                                cancel,
                            )?;
                        }
                        exact_content::finish_piece(catalog, &mut attempt, piece, cancel)?;
                    }
                }
                exact_content::publish(catalog, &mut attempt, cancel)?;
                if !attempt.published() {
                    return Err(crate::projection::provider::recovery::error::invalid(
                        "ordinary installation acknowledgement lacks Published journal",
                    ));
                }
                catalog.revalidate(cancel)?;

                Ok(manifest)
            },
        )
    }
}
