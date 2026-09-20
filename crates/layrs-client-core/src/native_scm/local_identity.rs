//! Local declarations for new captures. No remote account assertion is implied.
use super::{NativeWorkspace, Result};
pub use layrs_store_local::scm_v1::LocalIdentity;

impl NativeWorkspace {
    pub fn local_identity(&self) -> Result<Option<LocalIdentity>> {
        let store = layrs_store_local::scm_v1::open(self.store_root())?;
        Ok(layrs_store_local::scm_v1::local_identity(&store)?)
    }

    /// Explicit registration. Previously prepared captures keep their identity;
    /// changing configuration never rewrites an Operation or observation.
    pub fn configure_identity(&self, identity: LocalIdentity) -> Result<LocalIdentity> {
        let store = layrs_store_local::scm_v1::open_exclusive(self.store_root())?;
        Ok(layrs_store_local::scm_v1::configure_local_identity(
            &store, identity,
        )?)
    }
}
