//! Population-independent metadata proofs; no registration/source authority.
use super::{
    api::Api,
    control_directory_io as io,
    control_directory_privacy::{Privacy, READ_CONTROL},
    fs,
};
use crate::{
    ControlDirectoryIdentity, ControlDirectoryStageIdentity, Error, ObservedControlDirectory,
    Result,
    control_directory::{filesystem_identity, validate_control_paths, validate_stage_paths},
};
use layrs_core::scm::FileId;
use std::{fs::File, path::Path, sync::Arc};
use windows_sys::Win32::Storage::FileSystem::*;

/// Retains an initially empty, ordinary local NTFS root and all ancestors.
/// revalidate checks identity/registration/streams, not perpetual emptiness:
/// publication of the expected .layrs must not invalidate the retained pins.
pub struct ControlRootPreflight {
    pub(super) root: io::RootPins,
    pub(super) api: Arc<Api>,
    pub(super) privacy: Arc<Privacy>,
}
impl ControlRootPreflight {
    pub fn open(root: &Path) -> Result<Self> {
        validate_control_paths(&io::normalized(root)?)?;
        let value = Self {
            root: io::RootPins::open(root, false)?,
            api: Arc::new(Api::load()?),
            privacy: Arc::new(Privacy::load()?),
        };
        validate_control_paths(value.root().to_str().ok_or(Error::IdentityMismatch)?)?;
        value.revalidate()?;
        io::empty(value.root())?;
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
    pub fn revalidate(&self) -> Result<()> {
        self.root.outside_sync_roots(&self.api)?;
        io::stream_inventory(self.root.file(), true)
    }
}

/// Observe an EMPTY FORMAT allocation before the caller writes bytes. Only DATA
/// is returned. The caller must compare its own allocated native IDs and persist
/// authenticated ownership before writing FORMAT or initializing private SQL.
/// The caller owns creation, protected ACLs, writes, flushes and receipts.
pub fn observe_control_directory_stage(
    preflight: &ControlRootPreflight,
    staged_control: &Path,
) -> Result<ControlDirectoryStageIdentity> {
    preflight.revalidate()?;
    io::empty(preflight.root())?;
    let normalized = io::normalized(staged_control)?;
    validate_stage_paths(&normalized)?;
    let staged_control = Path::new(&normalized);
    let parent = io::RootPins::open(
        staged_control.parent().ok_or(Error::IdentityMismatch)?,
        false,
    )?;
    validate_parent(preflight, &parent)?;
    let directory = io::directory_observer(staged_control)?;
    io::not_under(&preflight.api, &directory)?;
    io::control_directory_attributes(&directory)?;
    preflight.privacy.verify(&directory)?;
    let stage = io::canonical(&directory)?;
    if !stage.eq_ignore_ascii_case(&normalized) {
        return Err(Error::IdentityMismatch);
    }
    io::only_child(Path::new(&stage), "FORMAT")?;
    io::stream_inventory(&directory, true)?;
    // Attribute/READ_CONTROL-only observation is intentional here: this is no
    // frozen-file capability and can coexist with the caller's allocation pin.
    let format = fs::open(
        &Path::new(&stage).join("FORMAT"),
        io::ALL_SHARING,
        FILE_READ_ATTRIBUTES | READ_CONTROL,
    )?;
    io::not_under(&preflight.api, &format)?;
    io::format_attributes(&format)?;
    preflight.privacy.verify(&format)?;
    io::stream_inventory(&format, false)?;
    if fs::size(&fs::identity(&format)?) != 0 {
        return Err(Error::Unsupported(
            "stage observation requires empty FORMAT",
        ));
    }
    let (volume, root_index) = preflight.root.identity();
    let (directory_volume, directory_index) = io::id(&directory)?;
    let (format_volume, format_index) = io::id(&format)?;
    if volume != directory_volume || volume != format_volume {
        return Err(Error::IdentityMismatch);
    }
    let value = ControlDirectoryStageIdentity {
        root: preflight
            .root()
            .to_str()
            .ok_or(Error::IdentityMismatch)?
            .to_owned(),
        parent: parent
            .root
            .to_str()
            .ok_or(Error::IdentityMismatch)?
            .to_owned(),
        stage,
        volume,
        root_index,
        parent_index: parent.identity().1,
        directory_index,
        format_index,
    };
    value.validate()?;
    let current = io::directory_observer(value.stage())?;
    if io::id(&current)? != (volume, directory_index) {
        return Err(Error::IdentityMismatch);
    }
    io::control_directory_attributes(&current)?;
    preflight.privacy.verify(&current)?;
    io::only_child(value.stage(), "FORMAT")?;
    io::format_attributes(&format)?;
    if fs::size(&fs::identity(&format)?) != 0 {
        return Err(Error::IdentityMismatch);
    }
    let current_format = fs::open(
        &value.stage().join("FORMAT"),
        io::ALL_SHARING,
        FILE_READ_ATTRIBUTES | READ_CONTROL,
    )?;
    io::format_attributes(&current_format)?;
    preflight.privacy.verify(&current_format)?;
    if io::id(&current_format)? != (volume, format_index)
        || fs::size(&fs::identity(&current_format)?) != 0
    {
        return Err(Error::IdentityMismatch);
    }
    validate_parent(preflight, &parent)?;
    control_registration_boundary(&preflight.api, &directory, &format, value.stage())?;
    preflight.revalidate()?;
    Ok(value)
}
pub(super) fn validate_parent(
    preflight: &ControlRootPreflight,
    parent: &io::RootPins,
) -> Result<()> {
    parent.outside_sync_roots(&preflight.api)?;
    if parent.identity().0 != preflight.root.identity().0
        || parent.contains(preflight.root.identity())
    {
        return Err(Error::Unsupported(
            "stage parent must be outside checkout on the same volume",
        ));
    }
    // The caller may allocate a protected stage in an ordinary inherited target
    // parent. Only stage and FORMAT require owner-private protected DACLs; the
    // parent is an identity/ancestry boundary, not a private Store directory.
    Ok(())
}

/// Exact retained .layrs/FORMAT layout, independent of population/SQL/tickets.
/// This grants no namespace exclusion, registration, source or readiness.
/// It owns the sole DELETE-capable directory pin. Additional live proofs must
/// use try_clone; a separate open can conflict with an existing owner's pin.
/// Populated-root layout checks do not enumerate the user namespace or rule out
/// other case-sensitive names such as .LAYRS. Only the exact canonical .layrs is
/// bound; later namespace proofs must handle aliases separately. Bootstrap checks
/// reject every extra root entry. No lowercase merging or global exclusion exists.
/// Privacy uses the process-token owner SID. Impersonation may cause refusal;
/// this API neither switches tokens nor relaxes the owner/DACL contract.
pub struct ControlDirectoryGuard {
    pub(super) root: io::RootPins,
    pub(super) directory: File,
    pub(super) format: File,
    pub(super) expected: ControlDirectoryIdentity,
    pub(super) bytes: Vec<u8>,
    pub(super) privacy: Arc<Privacy>,
}
impl ControlDirectoryGuard {
    /// The caller authenticates the expected receipt independently. The native
    /// proof checks its physical fields. Historical stage paths need not exist.
    pub fn open(root: &Path, expected: &ControlDirectoryIdentity) -> Result<Self> {
        let build = || {
            expected.stage.validate()?;
            let root = io::RootPins::open(root, true)?;
            match_root(&root, &expected.stage)?;
            let directory = io::directory_owner(&root.root.join(".layrs"))?;
            let privacy = Arc::new(Privacy::load()?);
            let path = root.root.join(".layrs");
            check_directory(&root, &directory, &path, &expected.stage, &privacy)?;
            let format = io::format_pin(&path.join("FORMAT"))?;
            let bytes = io::read_format(&format, &privacy)?;
            check_format(&format, &bytes, expected)?;
            let value = Self {
                root,
                directory,
                format,
                expected: expected.clone(),
                bytes,
                privacy,
            };
            value.revalidate_inner()?;
            Ok(value)
        };
        build().map_err(|cause| {
            super::control_directory_publish::failure(
                expected.stage.root(),
                expected.stage.stage(),
                crate::ControlDirectoryFailure::NeedsRepair,
                crate::ControlDirectoryPublishPoint::BeforeProof,
                "open expected control layout",
                cause,
            )
        })
    }
    pub fn root(&self) -> &Path {
        &self.root.root
    }
    pub fn identity(&self) -> &ControlDirectoryIdentity {
        &self.expected
    }
    pub fn format_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn root_filesystem_identity(&self) -> String {
        self.expected.stage.root_filesystem_identity()
    }
    pub fn directory_filesystem_identity(&self) -> String {
        self.expected.stage.directory_filesystem_identity()
    }
    pub fn format_binding_identity(&self) -> String {
        self.expected.stage.format_binding_identity()
    }
    pub fn try_clone(&self) -> Result<Self> {
        self.revalidate()?;
        let root = self.root.duplicate(&self.privacy)?;
        let directory = self.privacy.duplicate(&self.directory)?;
        // A fresh compatible strong FORMAT reader has its own file cursor.
        let format = io::format_pin(&self.root().join(".layrs/FORMAT"))?;
        let bytes = io::read_format(&format, &self.privacy)?;
        check_format(&format, &bytes, &self.expected)?;
        let value = Self {
            root,
            directory,
            format,
            expected: self.expected.clone(),
            bytes,
            privacy: self.privacy.clone(),
        };
        value.revalidate()?;
        Ok(value)
    }
    pub fn revalidate(&self) -> Result<()> {
        self.revalidate_inner().map_err(|cause| {
            super::control_directory_publish::failure(
                self.root(),
                self.expected.stage.stage(),
                crate::ControlDirectoryFailure::NeedsRepair,
                crate::ControlDirectoryPublishPoint::BeforeProof,
                "revalidate control layout",
                cause,
            )
        })
    }
    pub(super) fn revalidate_inner(&self) -> Result<()> {
        self.root.revalidate(true)?;
        match_root(&self.root, &self.expected.stage)?;
        let path = self.root().join(".layrs");
        check_directory(
            &self.root,
            &self.directory,
            &path,
            &self.expected.stage,
            &self.privacy,
        )?;
        io::format_attributes(&self.format)?;
        self.privacy.verify(&self.format)?;
        io::stream_inventory(&self.format, false)?;
        if io::id(&self.format)? != (self.expected.stage.volume, self.expected.stage.format_index) {
            return Err(Error::IdentityMismatch);
        }
        let current = io::format_pin(&path.join("FORMAT"))?;
        let bytes = io::read_format(&current, &self.privacy)?;
        check_format(&current, &bytes, &self.expected)?;
        if bytes != self.bytes {
            return Err(Error::IdentityMismatch);
        }
        check_directory(
            &self.root,
            &self.directory,
            &path,
            &self.expected.stage,
            &self.privacy,
        )?;
        self.root.revalidate(true)
    }
    /// Fresh-bootstrap proof, distinct from generic populated layout. Call
    /// before and after the external control_published commit. Pins are not
    /// registration locks or atomic namespace snapshots; this is revalidation.
    pub fn revalidate_bootstrap_root(&self) -> Result<()> {
        self.revalidate_unregistered(true)
    }
    /// Revalidates exact held/fresh root, ancestor, control and FORMAT identity,
    /// layout and NOT_UNDER boundaries without enumerating the user namespace.
    /// Unrelated descendants may be registered. Historical stage paths need not
    /// exist. Generic open/revalidate may read layout before this separate check;
    /// neither this result nor layout access grants source/readiness authority.
    pub fn revalidate_unregistered_root(&self) -> Result<()> {
        self.revalidate_unregistered(false)
    }
    fn revalidate_unregistered(&self, require_sole_child: bool) -> Result<()> {
        let check = || {
            let api = Api::load()?;
            self.root.outside_sync_roots(&api)?;
            control_registration_boundary(
                &api,
                &self.directory,
                &self.format,
                &self.root().join(".layrs"),
            )?;
            if require_sole_child {
                io::only_child(self.root(), ".layrs")?;
            }
            self.revalidate_inner()?;
            if require_sole_child {
                io::only_child(self.root(), ".layrs")?;
            }
            control_registration_boundary(
                &api,
                &self.directory,
                &self.format,
                &self.root().join(".layrs"),
            )?;
            // Last registration boundary is AFTER final file/layout opens.
            self.root.outside_sync_roots(&api)
        };
        check().map_err(|cause| {
            super::control_directory_publish::failure(
                self.root(),
                self.expected.stage.stage(),
                crate::ControlDirectoryFailure::NeedsRepair,
                crate::ControlDirectoryPublishPoint::BeforeProof,
                if require_sole_child {
                    "revalidate bootstrap root"
                } else {
                    "revalidate unregistered root"
                },
                cause,
            )
        })
    }
}
pub(super) fn match_root(root: &io::RootPins, stage: &ControlDirectoryStageIdentity) -> Result<()> {
    if root.root != stage.root() || root.identity() != (stage.volume, stage.root_index) {
        return Err(Error::IdentityMismatch);
    }
    Ok(())
}
pub(super) fn check_directory(
    root: &io::RootPins,
    directory: &File,
    path: &Path,
    stage: &ControlDirectoryStageIdentity,
    privacy: &Privacy,
) -> Result<()> {
    io::control_directory_attributes(directory)?;
    privacy.verify(directory)?;
    let current = io::directory_observer(path)?; // SHARE_DELETE is required by the sole owner.
    io::control_directory_attributes(&current)?;
    privacy.verify(&current)?;
    if io::id(directory)? != (stage.volume, stage.directory_index)
        || io::id(&current)? != io::id(directory)?
        || io::canonical(&current)? != io::normalized(path)?
    {
        return Err(Error::IdentityMismatch);
    }
    io::only_child(path, "FORMAT")?;
    io::stream_inventory(directory, true)?;
    io::stream_inventory(root.file(), true)?;
    Ok(())
}
pub(super) fn check_format(
    file: &File,
    bytes: &[u8],
    expected: &ControlDirectoryIdentity,
) -> Result<()> {
    if io::id(file)? != (expected.stage.volume, expected.stage.format_index)
        || bytes.len() != expected.size as usize
        || FileId::from_bytes(bytes) != expected.content
    {
        return Err(Error::IdentityMismatch);
    }
    Ok(())
}

/// Bounded fixed-name discovery, not a guard or receipt authenticator. No user
/// namespace traversal, content hydration, SQL access or registration mutation.
pub fn discover_control_directory(root: &Path) -> Result<ObservedControlDirectory> {
    let root = io::RootPins::open(root, true)?;
    let directory = io::directory_observer(&root.root.join(".layrs"))?;
    let privacy = Privacy::load()?;
    io::control_directory_attributes(&directory)?;
    privacy.verify(&directory)?;
    let path = root.root.join(".layrs");
    if io::canonical(&directory)? != path.to_str().ok_or(Error::IdentityMismatch)? {
        return Err(Error::IdentityMismatch);
    }
    io::only_child(&path, "FORMAT")?;
    let format = io::format_pin(&path.join("FORMAT"))?;
    let bytes = io::read_format(&format, &privacy)?;
    let (volume, root_index) = root.identity();
    let (directory_volume, directory_index) = io::id(&directory)?;
    let (format_volume, format_index) = io::id(&format)?;
    if volume != directory_volume || volume != format_volume {
        return Err(Error::IdentityMismatch);
    }
    let current = io::directory_observer(&path)?;
    io::control_directory_attributes(&current)?;
    privacy.verify(&current)?;
    if io::id(&current)? != (volume, directory_index) {
        return Err(Error::IdentityMismatch);
    }
    io::only_child(&path, "FORMAT")?;
    io::stream_inventory(&directory, true)?;
    io::stream_inventory(root.file(), true)?;
    root.revalidate(true)?;
    Ok(ObservedControlDirectory {
        root: root
            .root
            .to_str()
            .ok_or(Error::IdentityMismatch)?
            .to_owned(),
        volume,
        root_index,
        directory_index,
        format_index,
        content: FileId::from_bytes(&bytes),
        bytes,
    })
}

/// Bootstrap-only positive registration boundary on the exact control objects,
/// including files. Root/ancestor checks alone miss a nested registered root.
pub(super) fn control_registration_boundary(
    api: &Api,
    directory: &File,
    format: &File,
    path: &Path,
) -> Result<()> {
    io::not_under(api, directory)?;
    io::control_directory_attributes(directory)?;
    io::not_under(api, format)?;
    io::format_attributes(format)?;
    let current_directory = io::directory_observer(path)?;
    io::not_under(api, &current_directory)?;
    io::control_directory_attributes(&current_directory)?;
    if io::id(&current_directory)? != io::id(directory)?
        || io::canonical(&current_directory)? != io::normalized(path)?
    {
        return Err(Error::IdentityMismatch);
    }
    let current_format = fs::open(
        &path.join("FORMAT"),
        io::ALL_SHARING,
        FILE_READ_ATTRIBUTES | READ_CONTROL,
    )?;
    io::not_under(api, &current_format)?;
    io::format_attributes(&current_format)?;
    if io::id(&current_format)? != io::id(format)?
        || io::canonical(&current_format)? != format!("{}\\FORMAT", io::normalized(path)?)
    {
        return Err(Error::IdentityMismatch);
    }
    Ok(())
}
