//! Read-only pins for an already populated ordinary root. This does not prove
//! FORMAT, private ownership, contents, readiness or permission to mutate it.
use super::{api::Api, control_directory_io as io};
use crate::{
    control_directory::{filesystem_identity, validate_control_paths},
    Error, Result,
};
use std::{path::Path, sync::Arc};

/// Retains a local NTFS root and its ancestors outside every CFAPI sync root.
/// Unlike allocation preflight, this guard permits an existing namespace.
/// Callers must independently authenticate all metadata and content they use.
pub struct OrdinaryRootGuard {
    root: io::RootPins,
    api: Arc<Api>,
}

impl OrdinaryRootGuard {
    pub fn open(root: &Path) -> Result<Self> {
        validate_control_paths(&io::normalized(root)?)?;
        let value = Self {
            root: io::RootPins::open(root, false)?,
            api: Arc::new(Api::load()?),
        };
        validate_control_paths(value.root().to_str().ok_or(Error::IdentityMismatch)?)?;
        value.revalidate()?;
        Ok(value)
    }

    pub fn root(&self) -> &Path {
        &self.root.root
    }

    pub fn filesystem_identity(&self) -> String {
        let (volume, index) = self.root.identity();
        filesystem_identity(volume, index)
    }

    /// Registration is observed afresh; a prior NOT_UNDER result is never a
    /// permanent assertion. Both retained and freshly opened ancestors agree.
    pub fn revalidate(&self) -> Result<()> {
        self.root.revalidate(false)?;
        self.root.outside_sync_roots(&self.api)?;
        io::stream_inventory(self.root.file(), true)
    }
}
