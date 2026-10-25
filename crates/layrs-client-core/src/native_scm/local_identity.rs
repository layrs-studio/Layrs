//! Local declarations for new captures. No remote account assertion is implied.
use super::{NativeWorkspace, Result};
pub use layrs_store_local::scm_v1::LocalIdentity;

impl NativeWorkspace {
    pub fn local_identity(&self) -> Result<Option<LocalIdentity>> {
        let store = self.open_store()?;
        Ok(layrs_store_local::scm_v1::local_identity(&store)?)
    }

    /// Explicit registration. Previously prepared captures keep their identity;
    /// changing configuration never rewrites an Operation or observation.
    pub fn configure_identity(&self, identity: LocalIdentity) -> Result<LocalIdentity> {
        let store = layrs_store_local::scm_v1::open_exclusive(self.store_root())?
            .with_filesystem_identity(self.store_identity())?
            .with_metadata_identity(self.metadata_identity())?;
        Ok(layrs_store_local::scm_v1::configure_local_identity(
            &store, identity,
        )?)
    }
}
