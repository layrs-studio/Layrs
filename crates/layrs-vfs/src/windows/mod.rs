mod api;
mod attached;
mod callback;
mod checkpoint;
mod control;
mod control_directory;
mod control_directory_io;
mod control_directory_privacy;
mod control_directory_publish;
mod control_stage_observation;
mod export;
mod fs;
mod population;
mod provider_root;
mod publication;
mod raw;
mod root_probe;
#[cfg(test)]
mod stage2_fixture;
#[cfg(test)]
mod stage2_tests;
mod visibility;

use crate::{
    Capability, CaptureEvidence, Error, Inspection, PlaceholderEvidence, Registration, Result,
    SnapshotSource, classify_placeholder,
};
use api::{Api, check};
use attached::AttachedControl;
pub use callback::{CallbackFailure, HydrationRequest};
pub use checkpoint::Checkpoint;
pub use control::read_control_file;
pub use control_directory::{
    ControlDirectoryGuard, ControlRootPreflight, discover_control_directory,
    observe_control_directory_stage,
};
pub use control_directory_publish::publish_control_directory_recoverable;
pub use control_stage_observation::{
    ObservedAllocatedControlStage, ObservedAllocatedControlStageIdentity, ObservedRootNamespace,
    observe_allocated_control_stage,
};
pub use export::ExportCheckpoint;
use layrs_core::scm::RepoPath;
pub use provider_root::ProviderRootGuard;
pub use root_probe::ExportParentGuard;
use std::{
    collections::BTreeSet,
    ffi::OsStr,
    mem::{offset_of, size_of},
    path::Path,
    sync::Arc,
};
use windows_sys::{
    Win32::{
        Foundation::ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT,
        Storage::{CloudFilters::*, FileSystem::*},
    },
    core::GUID,
};

// One policy for both registration and reconnect verification. Existing
// PARTIAL prototype roots require explicit migration; never update them here.
const HYDRATION_POLICY: CF_HYDRATION_POLICY_PRIMARY = CF_HYDRATION_POLICY_PROGRESSIVE;

#[derive(Clone, Copy, PartialEq, Eq)]
enum InspectionPurpose {
    Capture,
    Export,
}

pub fn capability() -> Result<Capability> {
    let api = Api::load()?;
    let mut info = CF_PLATFORM_INFO::default();
    check("CfGetPlatformInfo", unsafe { (api.platform)(&mut info) })?;
    Ok(Capability {
        backend: "windows-cfapi-prototype",
        build: info.BuildNumber,
        revision: info.RevisionNumber,
        integration: info.IntegrationNumber,
    })
}

/// One private NTFS namespace over an immutable source. Multiple sessions may
/// share the source but must have distinct fresh roots and root IDs.
pub struct Session {
    api: Arc<Api>,
    source: Arc<dyn SnapshotSource>,
    root: fs::RootGuard,
    registration: Registration,
    connection: Option<CF_CONNECTION_KEY>,
    context: Option<Box<callback::Context>>,
    control: AttachedControl,
    catalog: Arc<crate::Catalog>,
}

impl Session {
    /// Caller explicitly supplies an existing, empty, disposable directory.
    /// Errors after registration retain that directory and every created file.
    /// No rollback deletes files or unregisters a possibly useful partial root.
    pub fn create(root: &Path, root_id: [u8; 16], source: Arc<dyn SnapshotSource>) -> Result<Self> {
        Self::create_with_registration_hook(root, root_id, source, |_| Ok(()))
    }

    /// The hook must durably persist the ticket before returning success. A
    /// hook error precedes all native registration or population mutations.
    pub fn create_with_registration_hook(
        root: &Path,
        root_id: [u8; 16],
        source: Arc<dyn SnapshotSource>,
        persist: impl FnOnce(&Registration) -> Result<()>,
    ) -> Result<Self> {
        // Re-validate all catalog invariants before even loading/registering.
        let catalog = source.catalog();
        let catalog = Arc::new(crate::Catalog::new(
            catalog.version(),
            catalog.entries().to_vec(),
        )?);
        if root_id == [0; 16] {
            return Err(Error::InvalidRoot("nonzero root identity required".into()));
        }
        let api = Arc::new(Api::load()?);
        let root = fs::RootGuard::open(root, false)?;
        if std::fs::read_dir(&root.root)?.next().transpose()?.is_some() {
            return Err(Error::InvalidRoot("root is not empty".into()));
        }
        // Only the documented not-under-sync-root result authorizes registration.
        // A foreign, nested or inaccessible registration is never overwritten.
        let mut info = CF_SYNC_ROOT_BASIC_INFO::default();
        let hr = unsafe {
            (api.root_info)(
                fs::handle(root.file()),
                CF_SYNC_ROOT_INFO_BASIC,
                (&mut info as *mut CF_SYNC_ROOT_BASIC_INFO).cast(),
                size_of::<CF_SYNC_ROOT_BASIC_INFO>() as u32,
                std::ptr::null_mut(),
            )
        };
        if hr >= 0 {
            return Err(Error::ForeignRoot);
        }
        if hr != (0x80070000u32 | ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT) as i32 {
            check("CfGetSyncRootInfoByHandle(preflight)", hr)?;
        }
        let identity = fs::identity(root.file())?;
        let registration = Registration {
            root: root.root.clone(),
            root_id,
            catalog_id: catalog.identity().clone(),
            volume: identity.dwVolumeSerialNumber,
            file_index: fs::index(&identity),
            flavor: crate::registration::RegistrationFlavor::Legacy,
        };
        let mut session = Self {
            api,
            source,
            root,
            registration,
            connection: None,
            context: None,
            control: AttachedControl::None,
            catalog,
        };
        persist(&session.registration)?;
        session.register(false, false)?;
        session.populate()?;
        // Only a fully created namespace is reconnectable. A crash before this
        // update leaves an explicitly incomplete root, never inferred deletions.
        session.register(true, true)?;
        session.connect()?;
        Ok(session)
    }

    /// Reconnect an already populated root. This never repopulates, replaces,
    /// marks files in-sync, or repairs namespace changes behind the caller.
    pub fn reconnect(registration: Registration, source: Arc<dyn SnapshotSource>) -> Result<Self> {
        registration.require_legacy()?;
        let catalog = Arc::new(source.catalog().clone());
        if registration.catalog_id != *catalog.identity() {
            return Err(Error::IdentityMismatch);
        }
        let api = Arc::new(Api::load()?);
        let root = fs::RootGuard::open(&registration.root, true)?;
        let mut session = Self {
            api,
            source,
            root,
            registration,
            connection: None,
            context: None,
            control: AttachedControl::None,
            catalog,
        };
        session.connect()?;
        Ok(session)
    }
    pub fn registration(&self) -> &Registration {
        &self.registration
    }
    /// Sole V2 source-binding route. Compares source metadata once and remains
    /// disconnected. The provider cannot authenticate a caller-supplied source:
    /// private Store phase/map binding is a separate adapter responsibility.
    pub fn reconnect_disconnected_with_control_directory(
        proof: ProviderRootGuard,
        source: Arc<dyn SnapshotSource>,
    ) -> Result<Self> {
        proof.revalidate()?;
        if source.catalog().identity() != proof.catalog().identity() {
            return Err(Error::IdentityMismatch);
        }
        let catalog = proof.frozen_catalog();
        let registration = proof.registration().clone();
        let api = Arc::new(Api::load()?);
        let root = fs::RootGuard::open(proof.root(), true)?;
        let native = fs::identity(root.file())?;
        if native.dwVolumeSerialNumber != registration.volume
            || fs::index(&native) != registration.file_index
        {
            return Err(Error::IdentityMismatch);
        }
        let value = Self {
            api,
            source,
            root,
            registration,
            connection: None,
            context: None,
            control: AttachedControl::Directory(proof),
            catalog,
        };
        value.verify_registration()?;
        Ok(value)
    }

    /// Clones the actual retained V2 proof. Legacy sessions cannot mint one.
    pub fn root_guard(&self) -> Result<ProviderRootGuard> {
        self.control
            .proof()
            .ok_or(Error::Unsupported(
                "legacy session has no complete provider root guard",
            ))?
            .try_clone()
    }
    pub fn is_connected(&self) -> bool {
        self.connection.is_some()
    }
    pub fn callback_failures(&self) -> Vec<CallbackFailure> {
        self.context
            .as_ref()
            .map_or_else(Vec::new, |context| context.failures())
    }
    pub fn hydration_requests(&self) -> Vec<HydrationRequest> {
        self.context
            .as_ref()
            .map_or_else(Vec::new, |context| context.traces())
    }

    pub fn disconnect(&mut self) -> Result<()> {
        if let Some(key) = self.connection {
            let context = self.context.as_ref().expect("connected context");
            context.stop();
            // CfDisconnectSyncRoot may deliver callbacks before returning.
            check("CfDisconnectSyncRoot", unsafe {
                (self.api.disconnect)(key)
            })?;
            context.drain();
            self.connection = None;
            self.context = None;
        }
        Ok(())
    }
    pub fn connect(&mut self) -> Result<()> {
        if self.connection.is_some() {
            return Err(Error::Busy);
        }
        self.verify_registration()?;
        let context = Box::new(callback::Context::new(
            self.api.clone(),
            self.source.clone(),
            self.catalog.clone(),
            &self.registration,
            self.control
                .proof()
                .map(|proof| proof.try_clone())
                .transpose()?,
        ));
        let callbacks = callback::table();
        let path = fs::wide(self.root.root.as_os_str())?;
        let mut key = 0;
        // This is the last metadata boundary before native connection. A V2
        // context owns its own actual proof, including all callback-lifetime pins.
        self.verify_registration()?;
        #[cfg(test)]
        stage2_tests::connection_call();
        check("CfConnectSyncRoot", unsafe {
            (self.api.connect)(
                path.as_ptr(),
                callbacks.as_ptr(),
                (&*context as *const callback::Context).cast(),
                CF_CONNECT_FLAG_BLOCK_SELF_IMPLICIT_HYDRATION
                    | CF_CONNECT_FLAG_REQUIRE_PROCESS_INFO,
                &mut key,
            )
        })?;
        self.context = Some(context);
        self.connection = Some(key);
        Ok(())
    }

    fn register(&self, ready: bool, update: bool) -> Result<()> {
        let name = fs::wide(OsStr::new("Layrs disposable VFS prototype"))?;
        let version = fs::wide(OsStr::new("1"))?;
        let path = fs::wide(self.root.root.as_os_str())?;
        let identity = self.registration.root_blob(ready);
        let registration = CF_SYNC_REGISTRATION {
            StructSize: size_of::<CF_SYNC_REGISTRATION>() as u32,
            ProviderName: name.as_ptr(),
            ProviderVersion: version.as_ptr(),
            SyncRootIdentity: identity.as_ptr().cast(),
            SyncRootIdentityLength: identity.len() as u32,
            FileIdentity: identity.as_ptr().cast(),
            FileIdentityLength: identity.len() as u32,
            ProviderId: GUID::from_u128(0xc34d5080_5f37_4981_b726_614bec4cb46c),
        };
        let policies = CF_SYNC_POLICIES {
            StructSize: size_of::<CF_SYNC_POLICIES>() as u32,
            Hydration: CF_HYDRATION_POLICY {
                Primary: HYDRATION_POLICY,
                Modifier: CF_HYDRATION_POLICY_MODIFIER_NONE,
            },
            Population: CF_POPULATION_POLICY {
                Primary: CF_POPULATION_POLICY_ALWAYS_FULL,
                Modifier: CF_POPULATION_POLICY_MODIFIER_NONE,
            },
            InSync: CF_INSYNC_POLICY_TRACK_ALL,
            HardLink: CF_HARDLINK_POLICY_NONE,
            PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
        };
        let flags = if update {
            CF_REGISTER_FLAG_UPDATE
        } else {
            CF_REGISTER_FLAG_NONE
        } | CF_REGISTER_FLAG_DISABLE_ON_DEMAND_POPULATION_ON_ROOT;
        check("CfRegisterSyncRoot", unsafe {
            (self.api.register)(path.as_ptr(), &registration, &policies, flags)
        })
    }

    fn populate(&self) -> Result<()> {
        // Directories contain metadata only and are native, fully enumerated.
        // Parent-first BTree order and create_dir (not create_dir_all) ensure
        // foreign collisions fail instead of adopting an existing directory.
        for directory in self.catalog.directories() {
            let _parents = self.root.parents(directory)?;
            std::fs::create_dir(self.root.root.join(directory))?;
        }
        for (index, entry) in self.catalog.entries().iter().enumerate() {
            let _parents = self.root.parents(entry.path.as_str())?;
            let relative = Path::new(entry.path.as_str());
            let parent = self.root.root.join(relative.parent().unwrap());
            let parent = fs::wide(parent.as_os_str())?;
            let leaf = fs::wide(relative.file_name().unwrap())?;
            let identity = self.catalog.file_identity(index);
            let mut create = CF_PLACEHOLDER_CREATE_INFO {
                RelativeFileName: leaf.as_ptr(),
                FsMetadata: CF_FS_METADATA {
                    BasicInfo: FILE_BASIC_INFO {
                        FileAttributes: FILE_ATTRIBUTE_NORMAL,
                        ..Default::default()
                    },
                    FileSize: entry.size as i64,
                },
                FileIdentity: identity.as_ptr().cast(),
                FileIdentityLength: identity.len() as u32,
                Flags: CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC,
                ..Default::default()
            };
            let mut count = 0;
            check("CfCreatePlaceholders", unsafe {
                (self.api.create)(
                    parent.as_ptr(),
                    &mut create,
                    1,
                    CF_CREATE_FLAG_STOP_ON_ERROR,
                    &mut count,
                )
            })?;
            check("CfCreatePlaceholders(entry)", create.Result)?;
            if count != 1 {
                return Err(Error::Unsupported(
                    "CFAPI did not create the requested placeholder",
                ));
            }
        }
        Ok(())
    }

    fn verify_registration(&self) -> Result<()> {
        if let Some(proof) = self.control.proof() {
            if proof.registration().encode() != self.registration.encode()
                || proof.catalog().identity() != self.catalog.identity()
            {
                return Err(Error::IdentityMismatch);
            }
            proof.revalidate()
        } else {
            self.registration.require_legacy()?;
            verify_owned_registration(&self.api, &self.root, &self.registration)
        }
    }

    /// No content reads. Unchanged partial evidence keeps a handle denying writes
    /// and deletes. Known absent paths mean namespace deletion, not missing CAS
    /// bytes. Unexpected identities/renames fail instead of guessing by path.
    pub fn inspect(&self, path: &RepoPath) -> Result<Inspection> {
        self.inspect_inner(path)
    }
    fn inspect_inner(&self, path: &RepoPath) -> Result<Inspection> {
        crate::catalog::validate_path(path)?;
        self.verify_registration()?;
        self.inspect_validated(path)
    }
}

pub(crate) fn verify_registration(
    registration: &Registration,
    catalog: &crate::Catalog,
) -> Result<()> {
    if registration.catalog_id != *catalog.identity() {
        return Err(Error::IdentityMismatch);
    }
    verify_root_identity(registration)
}

pub(crate) fn verify_root_identity(registration: &Registration) -> Result<()> {
    let api = Api::load()?;
    let root = fs::RootGuard::open(&registration.root, true)?;
    verify_owned_registration(&api, &root, registration)
}

fn verify_owned_registration(
    api: &Api,
    root: &fs::RootGuard,
    registration: &Registration,
) -> Result<()> {
    let native = fs::identity(root.file())?;
    if registration.volume != native.dwVolumeSerialNumber
        || registration.file_index != fs::index(&native)
    {
        return Err(Error::ForeignRoot);
    }
    let mut storage = [0u64; 1024];
    let mut returned = 0;
    check("CfGetSyncRootInfoByHandle", unsafe {
        (api.root_info)(
            fs::handle(root.file()),
            CF_SYNC_ROOT_INFO_STANDARD,
            storage.as_mut_ptr().cast(),
            size_of_val(&storage) as u32,
            &mut returned,
        )
    })?;
    let offset = offset_of!(CF_SYNC_ROOT_STANDARD_INFO, SyncRootIdentity);
    if returned as usize > size_of_val(&storage) || (returned as usize) < offset {
        return Err(Error::IdentityMismatch);
    }
    // Storage is u64-aligned and large enough for the fixed CFAPI header.
    let info = unsafe { &*storage.as_ptr().cast::<CF_SYNC_ROOT_STANDARD_INFO>() };
    let len = info.SyncRootIdentityLength as usize;
    if len > 4096 || offset + len > returned as usize {
        return Err(Error::IdentityMismatch);
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>().add(offset), len) };
    if bytes == registration.root_blob(false) {
        return Err(Error::NeedsRepair(
            "owned provider population is incomplete; retained without mutation",
        ));
    }
    if bytes != registration.root_blob(true)
        || info.SyncRootFileId as u64 != registration.file_index
        || info.HydrationPolicy.Primary != HYDRATION_POLICY
        || info.HydrationPolicy.Modifier != CF_HYDRATION_POLICY_MODIFIER_NONE
        || info.PopulationPolicy.Primary != CF_POPULATION_POLICY_ALWAYS_FULL
        || info.HardLinkPolicy != CF_HARDLINK_POLICY_NONE
        || info.InSyncPolicy != CF_INSYNC_POLICY_TRACK_ALL
    {
        return Err(Error::ForeignRoot);
    }
    Ok(())
}

impl Session {
    fn inspect_validated(&self, path: &RepoPath) -> Result<Inspection> {
        let Some((file, parents)) = self.root.inspection_file(path.as_str())? else {
            return self.deleted(path);
        };
        self.inspect_file(path, file, parents)
    }

    fn inspect_file(
        &self,
        path: &RepoPath,
        file: std::fs::File,
        parents: Vec<std::fs::File>,
    ) -> Result<Inspection> {
        self.inspect_file_for(path, file, parents, InspectionPurpose::Capture)
    }

    fn inspect_file_for(
        &self,
        path: &RepoPath,
        file: std::fs::File,
        parents: Vec<std::fs::File>,
        purpose: InspectionPurpose,
    ) -> Result<Inspection> {
        let attrs = fs::attributes(&file)?;
        if self.is_control(path.as_str(), &file)? {
            return Err(Error::Unsupported(
                "attached .layrs control file is not snapshot content",
            ));
        }
        let native = fs::identity(&file)?;
        if attrs.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 || native.nNumberOfLinks != 1 {
            return Err(Error::Unsupported(
                "inspection requires a regular file with one link",
            ));
        }
        let state = unsafe { (self.api.state)(attrs.FileAttributes, attrs.ReparseTag) };
        if state == CF_PLACEHOLDER_STATE_INVALID {
            return Err(Error::Unsupported("invalid placeholder state"));
        }
        if state & CF_PLACEHOLDER_STATE_PLACEHOLDER == 0 {
            if attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(Error::Unsupported("non-CFAPI reparse point"));
            }
            return Ok(Inspection {
                path: path.clone(),
                evidence: CaptureEvidence::NeedsHash {
                    base: self.catalog.entry(path).map(|e| e.file_id.clone()),
                    size: fs::size(&native),
                },
                placeholder: None,
                _guard: Some(file),
                _parents: parents,
            });
        }
        let mut storage = [0u64; 1024];
        let mut returned = 0;
        check("CfGetPlaceholderInfo", unsafe {
            (self.api.placeholder_info)(
                fs::handle(&file),
                CF_PLACEHOLDER_INFO_STANDARD,
                storage.as_mut_ptr().cast(),
                size_of_val(&storage) as u32,
                &mut returned,
            )
        })?;
        let offset = offset_of!(CF_PLACEHOLDER_STANDARD_INFO, FileIdentity);
        if returned as usize > size_of_val(&storage) || (returned as usize) < offset {
            return Err(Error::IdentityMismatch);
        }
        let info = unsafe { &*storage.as_ptr().cast::<CF_PLACEHOLDER_STANDARD_INFO>() };
        let len = info.FileIdentityLength as usize;
        if len > 4096
            || offset + len > returned as usize
            || info.OnDiskDataSize < 0
            || info.ModifiedDataSize < 0
        {
            return Err(Error::IdentityMismatch);
        }
        let identity =
            unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>().add(offset), len) };
        let entry = self.catalog.resolve_identity(identity)?;
        if info.SyncRootFileId as u64 != self.registration.file_index
            || native.dwVolumeSerialNumber != self.registration.volume
            || info.FileId as u64 != fs::index(&native)
        {
            return Err(Error::ForeignRoot);
        }
        let relocated = entry.path != *path;
        if relocated && purpose == InspectionPurpose::Capture {
            return Err(Error::Unsupported(
                "placeholder renamed: identity is preserved, capture reconciliation required",
            ));
        }
        let evidence = PlaceholderEvidence {
            on_disk_bytes: info.OnDiskDataSize as u64,
            modified_bytes: info.ModifiedDataSize as u64,
            in_sync: info.InSyncState == CF_IN_SYNC_STATE_IN_SYNC
                && state & CF_PLACEHOLDER_STATE_IN_SYNC != 0,
            partial: state & CF_PLACEHOLDER_STATE_PARTIAL != 0,
            file_id: info.FileId,
            sync_root_file_id: info.SyncRootFileId,
        };
        Ok(Inspection {
            path: path.clone(),
            evidence: if relocated {
                CaptureEvidence::NeedsHash {
                    base: Some(entry.file_id.clone()),
                    size: fs::size(&native),
                }
            } else {
                classify_placeholder(entry, fs::size(&native), &evidence)
            },
            placeholder: Some(evidence),
            _guard: Some(file),
            _parents: parents,
        })
    }
    fn deleted(&self, path: &RepoPath) -> Result<Inspection> {
        let entry = self
            .catalog
            .entry(path)
            .ok_or_else(|| Error::Io(std::io::Error::from(std::io::ErrorKind::NotFound)))?;
        Ok(Inspection {
            path: path.clone(),
            evidence: CaptureEvidence::Deleted {
                base: entry.file_id.clone(),
            },
            placeholder: None,
            _guard: None,
            _parents: Vec::new(),
        })
    }

    /// All current regular files plus catalog paths (so deletions remain visible).
    /// Quiesce external writers for namespace-level capture consistency. Any
    /// unsupported file type/path aborts, rather than silently dropping it.
    pub fn namespace(&self) -> Result<Vec<RepoPath>> {
        self.verify_registration()?;
        let mut files: BTreeSet<RepoPath> = self
            .catalog
            .entries()
            .iter()
            .map(|e| e.path.clone())
            .collect();
        let mut pending = vec![String::new()];
        let mut budget = crate::catalog::NamespaceBudget::default();
        while let Some(relative) = pending.pop() {
            let check_path = if relative.is_empty() {
                "leaf".into()
            } else {
                format!("{relative}/leaf")
            };
            let _parents = self.root.parents(&check_path)?;
            for item in std::fs::read_dir(self.root.root.join(&relative))? {
                let item = item?;
                let name = item
                    .file_name()
                    .into_string()
                    .map_err(|_| Error::Unsupported("non-Unicode file name"))?;
                let name = if relative.is_empty() {
                    name
                } else {
                    format!("{relative}/{name}")
                };
                // Only the attached fixed-name capability may bypass the
                // provider's reserved-name validation. Verify its native
                // identity first; all other paths still use normal validation.
                if name == ".layrs" && self.control.is_some() {
                    let file = fs::open(
                        &item.path(),
                        control_directory_io::ALL_SHARING,
                        self.control.observer_access(),
                    )?;
                    if self.is_control(&name, &file)? {
                        budget.verified_control_entry()?;
                        continue;
                    }
                }
                budget.user_entry()?;
                let path = RepoPath::parse(&name)
                    .map_err(|_| Error::Unsupported("invalid repository path"))?;
                crate::catalog::validate_path(&path)?;
                let file = fs::open(
                    &item.path(),
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    FILE_READ_ATTRIBUTES,
                )?;
                let attrs = fs::attributes(&file)?;
                if attrs.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                    if attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                        return Err(Error::Unsupported("reparse directory"));
                    }
                    pending.push(name);
                } else {
                    let state = unsafe { (self.api.state)(attrs.FileAttributes, attrs.ReparseTag) };
                    if state == CF_PLACEHOLDER_STATE_INVALID
                        || (attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
                            && state & CF_PLACEHOLDER_STATE_PLACEHOLDER == 0)
                        || fs::identity(&file)?.nNumberOfLinks != 1
                    {
                        return Err(Error::Unsupported(
                            "namespace includes unsupported reparse point/hardlink",
                        ));
                    }
                    files.insert(path);
                }
            }
        }
        budget.finish(self.control.is_some())?;
        self.control.revalidate()?;
        Ok(files.into_iter().collect())
    }

    /// Explicit full hydration for a hashing consumer in this process, which is
    /// blocked from implicitly hydrating. No publication/capture is performed.
    pub fn hydrate_for_hash(&self, path: &RepoPath) -> Result<()> {
        let inspected = self.inspect(path)?;
        if inspected.placeholder.is_some() {
            let file = inspected._guard.as_ref().unwrap();
            #[cfg(test)]
            stage2_tests::hydration_call();
            check("CfHydratePlaceholder", unsafe {
                (self.api.hydrate)(
                    fs::handle(file),
                    0,
                    -1,
                    CF_HYDRATE_FLAG_NONE,
                    std::ptr::null_mut(),
                )
            })?;
        }
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.disconnect().is_err() {
            // A failed disconnect cannot establish that callbacks have stopped.
            // Keep context, source, DLL and V2 actual proof/pins alive instead of
            // a use-after-free. This retains ownership locks and can block work;
            // it is not a harmless leak or successful teardown.
            // This exceptional leak lasts until process exit; explicit callers
            // get the precise HRESULT from disconnect() and can retry.
            if let Some(context) = self.context.take() {
                Box::leak(context);
            }
        }
    }
}
