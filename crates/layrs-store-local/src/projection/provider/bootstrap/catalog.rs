//! Bound bootstrap metadata only. The published holder retains the control pins;
//! the returned view owns validated data but no database handle or lock.
use super::*;
use layrs_core::{
    projection::{ProjectionSelection, ProjectionSnapshot},
    scm::{FileId, FileManifest, FileManifestId, RepoPath},
};

/// Validated bootstrap metadata bound to a live published control holder.
/// Snapshot, selection and descriptors are data, not a content or activation
/// capability. The view holds no SQL connection, operation lock or read lease.
pub struct ProviderBootstrapCatalog<'published> {
    published: &'published ProviderControlPublished,
    source: ProviderBootstrapSource,
}

/// Borrowed descriptor metadata; its manifest does not prove resident bytes.
pub struct ProviderCatalogDescriptor<'catalog> {
    record: &'catalog DescriptorRecord,
}

impl ProviderControlPublished {
    /// Reconstruct the exact bootstrap metadata under the existing published
    /// guard, then release all operation locks before returning the view.
    pub fn bootstrap_catalog(&self) -> Result<ProviderBootstrapCatalog<'_>> {
        Ok(ProviderBootstrapCatalog {
            published: self,
            source: self.read_catalog_source("read published provider bootstrap catalog")?,
        })
    }

    fn read_catalog_source(&self, operation: &'static str) -> Result<ProviderBootstrapSource> {
        let context = WriteContext {
            operation,
            report: Some(self.issuance.retained()),
        };
        let result = (|| -> Result<ProviderBootstrapSource> {
            let _locks = self
                .held
                .family
                .locks(
                    &self.held.artifacts.allocation.format.identity.worktree_id,
                    false,
                )
                .map_err(|e| context.error(e, None))?;
            self.held
                .guard
                .revalidate_bootstrap_root()
                .map_err(|e| context.error(e.into(), None))?;
            let inspection = recovery::writer_inspect_locked(
                &self.held.family,
                &self.held.artifacts,
                &self.held.binding,
                Some(&self.held.guard),
            )
            .map_err(|e| context.attach(e))?;
            outcome::require_published(inspection).map_err(|e| context.error(e, None))?;
            #[cfg(test)]
            catalog_test_hooks::point(catalog_test_hooks::CatalogReadPoint::AfterInspection, None);

            // This separate read preserves the full validator and its bounded
            // reconstruction. `read` closes the connection observably before it
            // returns, including on a validation failure.
            let state = database_io::read(&self.held.artifacts.database, |connection| {
                let result = validation::private_with_source(
                    connection,
                    &self.held.artifacts.allocation,
                    self.held.artifacts.prepared.as_ref(),
                );
                #[cfg(test)]
                catalog_test_hooks::point(
                    catalog_test_hooks::CatalogReadPoint::AfterPrivateValidation,
                    Some(connection),
                );
                result
            })
            .map_err(|e| context.error(e, Some(ProviderDatabaseScope::PrivateMetadata)))?;
            let scope = Some(ProviderDatabaseScope::PrivateMetadata);
            let private = match state {
                database_io::ReadState::Valid(value) => value,
                database_io::ReadState::Hot(hot) => {
                    return Err(context.error(Cause::ReadonlyRollback(hot.trigger), scope));
                }
                database_io::ReadState::Rejected { cause, .. }
                | database_io::ReadState::ReadButUnavailable(cause)
                | database_io::ReadState::Unavailable(cause) => {
                    return Err(context.error(cause, scope));
                }
                database_io::ReadState::UnboundRollback { trigger, cause } => {
                    return Err(recovery::writer_failure(
                        context.operation,
                        cause,
                        scope,
                        Some(trigger),
                        context.report.clone(),
                    ));
                }
                database_io::ReadState::Uninitialized => {
                    return Err(
                        context.error(invalid("provider private database uninitialized"), scope)
                    );
                }
            };
            let source = match private {
                validation::PrivateSourceValidation::Initialized { mount, source }
                    if mount.phase == MountPhase::ControlPublished =>
                {
                    source
                }
                _ => {
                    return Err(context.error(
                        invalid("published provider catalog requires published private mount"),
                        Some(ProviderDatabaseScope::PrivateMetadata),
                    ));
                }
            };
            source
                .require_commitment(&self.held.artifacts.allocation.source)
                .map_err(|e| {
                    context.error(e.into(), Some(ProviderDatabaseScope::PrivateMetadata))
                })?;
            self.held
                .artifacts
                .writer_require_guard(&self.held.guard)
                .map_err(|e| context.error(e, None))?;
            self.held
                .guard
                .revalidate_bootstrap_root()
                .map_err(|e| context.error(e.into(), None))?;
            self.held
                .artifacts
                .verify(&self.held.family)
                .map_err(|e| context.error(e, None))?;
            if self
                .held
                .artifacts
                .binding()
                .map_err(|e| context.error(e, None))?
                != self.held.binding
            {
                return Err(context.error(invalid("published catalog binding changed"), None));
            }
            Ok(source)
        })();
        result.map_err(|e| context.attach(e))
    }
}

#[cfg(test)]
pub(super) mod catalog_test_hooks {
    use std::{cell::RefCell, thread_local};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(in crate::projection::provider::bootstrap) enum CatalogReadPoint {
        AfterInspection,
        AfterPrivateValidation,
    }

    type Hook = Box<dyn FnMut(CatalogReadPoint, Option<&rusqlite::Connection>)>;
    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    pub(in crate::projection::provider::bootstrap) fn install(
        hook: impl FnMut(CatalogReadPoint, Option<&rusqlite::Connection>) + 'static,
    ) {
        HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    }
    pub(in crate::projection::provider::bootstrap) fn reset() {
        HOOK.with(|slot| *slot.borrow_mut() = None);
    }
    pub(super) fn point(point: CatalogReadPoint, connection: Option<&rusqlite::Connection>) {
        HOOK.with(|slot| {
            if let Some(hook) = slot.borrow_mut().as_mut() {
                hook(point, connection);
            }
        });
    }
}

impl ProviderBootstrapCatalog<'_> {
    pub fn binding(&self) -> &ProviderBootstrapBinding {
        self.published.binding()
    }
    pub fn source_generation(&self) -> u64 {
        self.source.commitment().source_generation
    }
    pub fn source_digest(&self) -> &str {
        &self.source.commitment().snapshot_digest
    }
    pub fn selection_digest(&self) -> &str {
        &self.source.commitment().selection_digest
    }
    pub fn catalog_id(&self) -> &str {
        &self.source.commitment().catalog_id
    }
    pub fn source_snapshot(&self) -> &ProjectionSnapshot {
        self.source.initial()
    }
    pub fn selection(&self) -> &ProjectionSelection {
        self.source.selection()
    }
    pub fn vfs_catalog(&self) -> &layrs_vfs::Catalog {
        self.source.vfs_catalog()
    }
    pub fn descriptors(&self) -> impl ExactSizeIterator<Item = ProviderCatalogDescriptor<'_>> + '_ {
        self.source
            .descriptors()
            .iter()
            .map(|record| ProviderCatalogDescriptor { record })
    }
    pub fn descriptor(&self, path: &RepoPath) -> Option<ProviderCatalogDescriptor<'_>> {
        self.source
            .descriptors()
            .binary_search_by(|record| record.path.cmp(path))
            .ok()
            .map(|index| ProviderCatalogDescriptor {
                record: &self.source.descriptors()[index],
            })
    }
    /// Check that the retained metadata still matches the exact published
    /// binding and a fresh full private validation.
    pub fn revalidate(&self) -> Result<()> {
        let fresh = self
            .published
            .read_catalog_source("revalidate published provider bootstrap catalog")?;
        if fresh.commitment() != self.source.commitment() {
            let context = WriteContext {
                operation: "revalidate published provider bootstrap catalog",
                report: Some(self.published.issuance.retained()),
            };
            return Err(context.error(invalid("published catalog data changed"), None));
        }
        Ok(())
    }
}

impl ProviderCatalogDescriptor<'_> {
    pub fn path(&self) -> &RepoPath {
        &self.record.path
    }
    pub fn file_id(&self) -> &FileId {
        &self.record.file_id
    }
    pub fn manifest_id(&self) -> &FileManifestId {
        &self.record.manifest_id
    }
    pub fn manifest(&self) -> &FileManifest {
        &self.record.manifest
    }
    pub fn byte_length(&self) -> u64 {
        self.record.bytes
    }
    pub fn executable(&self) -> bool {
        self.record.executable
    }
}
