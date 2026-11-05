//! Source-free initial population and the private capability for READY update.
use super::{
    api::{Api, check},
    attached,
    control_directory::ControlDirectoryGuard,
    control_directory_io as io, fs, provider_root,
};
use crate::{Catalog, Error, Registration, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    mem::{offset_of, size_of_val},
    path::Path,
};
use windows_sys::Win32::Storage::{CloudFilters::*, FileSystem::*};

pub(super) fn validate_native_paths(root: &Path, catalog: &Catalog) -> Result<()> {
    for directory in catalog.directories() {
        fs::native_path(&root.join(directory))?;
    }
    for entry in catalog.entries() {
        fs::native_path(&root.join(entry.path.as_str()))?;
    }
    Ok(())
}

pub(super) struct CreatedPopulation {
    nodes: BTreeMap<String, (bool, io::Id)>,
}
pub(super) fn populate(
    api: &Api,
    registration: &Registration,
    catalog: &Catalog,
    control: &ControlDirectoryGuard,
) -> Result<CreatedPopulation> {
    let root = fs::RootGuard::open(control.root(), true)?;
    let mut nodes = BTreeMap::new();
    for directory in catalog.directories() {
        let _parents = root.parents(directory)?;
        std::fs::create_dir(root.root.join(directory))?;
        let file = fs::open(
            &root.root.join(directory),
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        )?;
        io::plain_directory(&file)?;
        nodes.insert(directory.clone(), (true, io::id(&file)?));
        test_point(Point::DuringPopulation)?;
    }
    for (index, entry) in catalog.entries().iter().enumerate() {
        let _parents = root.parents(entry.path.as_str())?;
        let relative = Path::new(entry.path.as_str());
        let parent = fs::native_path(
            &root
                .root
                .join(relative.parent().ok_or(Error::IdentityMismatch)?),
        )?;
        let leaf = fs::wide(relative.file_name().ok_or(Error::IdentityMismatch)?)?;
        let identity = catalog.file_identity(index);
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
        check("CfCreatePlaceholders(V2)", unsafe {
            (api.create)(
                parent.as_ptr(),
                &mut create,
                1,
                CF_CREATE_FLAG_STOP_ON_ERROR,
                &mut count,
            )
        })?;
        check("CfCreatePlaceholders(V2 entry)", create.Result)?;
        if count != 1 {
            return Err(Error::IdentityMismatch);
        }
        let file = fs::open(
            &root.root.join(entry.path.as_str()),
            FILE_SHARE_READ,
            FILE_READ_DATA | FILE_READ_ATTRIBUTES,
        )?;
        audit_file(api, registration, catalog, index, &file)?;
        nodes.insert(entry.path.as_str().to_owned(), (false, io::id(&file)?));
        test_point(Point::DuringPopulation)?;
    }
    Ok(CreatedPopulation { nodes })
}

/// Not constructible outside this module; every audit handle survives READY
/// update/readback. The caller's quiescence must also exclude registration edits.
pub(super) struct VerifiedPopulation {
    nodes: BTreeMap<String, (bool, io::Id)>,
    pins: BTreeMap<String, File>,
    registration: Vec<u8>,
}
impl VerifiedPopulation {
    pub(super) fn audit(
        api: &Api,
        registration: &Registration,
        catalog: &Catalog,
        control: &ControlDirectoryGuard,
        created: CreatedPopulation,
    ) -> Result<Self> {
        provider_root::verify_bound(api, registration, catalog, control, false)?;
        let actual = namespace(control)?;
        let expected: BTreeSet<_> = catalog
            .directories()
            .iter()
            .cloned()
            .chain(
                catalog
                    .entries()
                    .iter()
                    .map(|entry| entry.path.as_str().to_owned()),
            )
            .collect();
        if actual.keys().cloned().collect::<BTreeSet<_>>() != expected
            || actual.len() != created.nodes.len()
        {
            return Err(Error::NeedsRepair(
                "initial population namespace differs from frozen catalog",
            ));
        }
        let mut pins = BTreeMap::new();
        for (path, (directory, identity)) in &created.nodes {
            if actual.get(path) != Some(&(*directory, *identity)) {
                return Err(Error::IdentityMismatch);
            }
            let file = fs::open(
                &control.root().join(path),
                if *directory {
                    FILE_SHARE_READ | FILE_SHARE_WRITE
                } else {
                    FILE_SHARE_READ
                },
                FILE_READ_ATTRIBUTES
                    | if *directory {
                        FILE_LIST_DIRECTORY
                    } else {
                        FILE_READ_DATA
                    },
            )?;
            if io::id(&file)? != *identity {
                return Err(Error::IdentityMismatch);
            }
            pins.insert(path.clone(), file);
        }
        let token = Self {
            nodes: created.nodes,
            pins,
            registration: registration.encode(),
        };
        token.revalidate(api, registration, catalog, control, false)?;
        Ok(token)
    }
    fn revalidate(
        &self,
        api: &Api,
        registration: &Registration,
        catalog: &Catalog,
        control: &ControlDirectoryGuard,
        ready: bool,
    ) -> Result<()> {
        if self.registration != registration.encode() {
            return Err(Error::IdentityMismatch);
        }
        provider_root::verify_bound(api, registration, catalog, control, ready)?;
        if namespace(control)? != self.nodes {
            return Err(Error::NeedsRepair(
                "initial population changed during audit",
            ));
        }
        for (path, file) in &self.pins {
            let (directory, identity) = self.nodes.get(path).ok_or(Error::IdentityMismatch)?;
            if io::id(file)? != *identity {
                return Err(Error::IdentityMismatch);
            }
            if *directory {
                io::plain_directory(file)?;
                provider_root::verify_native(
                    api,
                    file,
                    registration,
                    ready,
                    "CfGetSyncRootInfo(V2 audit directory)",
                )?;
            }
        }
        for (index, entry) in catalog.entries().iter().enumerate() {
            let file = self
                .pins
                .get(entry.path.as_str())
                .ok_or(Error::IdentityMismatch)?;
            audit_file(api, registration, catalog, index, file)?;
        }
        provider_root::verify_bound(api, registration, catalog, control, ready)
    }
    pub(super) fn mark_ready(
        &self,
        api: &Api,
        registration: &Registration,
        catalog: &Catalog,
        control: &ControlDirectoryGuard,
    ) -> Result<()> {
        self.revalidate(api, registration, catalog, control, false)?;
        provider_root::register(api, registration, true, true)?;
        #[cfg(test)]
        test_point(Point::AfterUpdate)?;
        // A failure here has unknown outcome. Never roll back or repopulate.
        self.revalidate(api, registration, catalog, control, true)
    }
}

fn namespace(control: &ControlDirectoryGuard) -> Result<BTreeMap<String, (bool, io::Id)>> {
    control.revalidate()?;
    let mut nodes = BTreeMap::new();
    let mut pending = vec![String::new()];
    let mut budget = crate::catalog::NamespaceBudget::default();
    let mut control_seen = false;
    while let Some(relative) = pending.pop() {
        let directory = fs::open(
            &control.root().join(&relative),
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        )?;
        if !relative.is_empty() {
            io::plain_directory(&directory)?;
        }
        for item in std::fs::read_dir(control.root().join(&relative))? {
            let item = item?;
            let leaf = item
                .file_name()
                .into_string()
                .map_err(|_| Error::Unsupported("non-Unicode initial namespace"))?;
            let path = if relative.is_empty() {
                leaf
            } else {
                format!("{relative}/{leaf}")
            };
            if path == ".layrs" {
                let file = io::directory_observer(&item.path())?;
                attached::verify_directory_entry(control, &file)?;
                budget.verified_control_entry()?;
                control_seen = true;
                continue;
            }
            budget.user_entry()?;
            let repo =
                layrs_core::scm::RepoPath::parse(&path).map_err(|_| Error::IdentityMismatch)?;
            crate::catalog::validate_path(&repo)?;
            let file = fs::open(&item.path(), io::ALL_SHARING, FILE_READ_ATTRIBUTES)?;
            let is_directory =
                fs::attributes(&file)?.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
            if is_directory {
                io::plain_directory(&file)?;
                pending.push(path.clone());
            }
            if nodes.insert(path, (is_directory, io::id(&file)?)).is_some() {
                return Err(Error::IdentityMismatch);
            }
        }
    }
    if !control_seen {
        return Err(Error::IdentityMismatch);
    }
    control.revalidate()?;
    Ok(nodes)
}

fn audit_file(
    api: &Api,
    registration: &Registration,
    catalog: &Catalog,
    index: usize,
    file: &File,
) -> Result<()> {
    let entry = &catalog.entries()[index];
    let attributes = fs::attributes(file)?;
    let native = fs::identity(file)?;
    let state = unsafe { (api.state)(attributes.FileAttributes, attributes.ReparseTag) };
    if attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
        || native.nNumberOfLinks != 1
        || fs::size(&native) != entry.size
        || native.dwVolumeSerialNumber != registration.volume
        || state == CF_PLACEHOLDER_STATE_INVALID
        || state & CF_PLACEHOLDER_STATE_PLACEHOLDER == 0
        || state & CF_PLACEHOLDER_STATE_IN_SYNC == 0
        || (entry.size != 0 && state & CF_PLACEHOLDER_STATE_PARTIAL == 0)
    {
        return Err(Error::NeedsRepair(
            "initial placeholder type/size/state differs from catalog",
        ));
    }
    let mut storage = [0u64; 1024];
    let mut returned = 0;
    check("CfGetPlaceholderInfo(V2 audit)", unsafe {
        (api.placeholder_info)(
            fs::handle(file),
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
    let expected = catalog.file_identity(index);
    if info.FileIdentityLength as usize != expected.len()
        || offset + expected.len() > returned as usize
        || info.FileId as u64 != fs::index(&native)
        || info.SyncRootFileId as u64 != registration.file_index
        || info.OnDiskDataSize != 0
        || info.ModifiedDataSize != 0
        || info.InSyncState != CF_IN_SYNC_STATE_IN_SYNC
    {
        return Err(Error::NeedsRepair(
            "initial placeholder identity/residency differs from catalog",
        ));
    }
    let identity = unsafe {
        std::slice::from_raw_parts(storage.as_ptr().cast::<u8>().add(offset), expected.len())
    };
    if identity != expected {
        return Err(Error::IdentityMismatch);
    }
    Ok(())
}

// Native predicate qualification on a real resident/dirty READY fixture. This
// exposes only the exact audit check to this crate's tests, never a token or a
// READY-update route, and does not connect a source during initial creation.
#[cfg(test)]
pub(super) fn audit_file_for_test(
    api: &Api,
    registration: &Registration,
    catalog: &Catalog,
    index: usize,
    file: &File,
) -> Result<()> {
    audit_file(api, registration, catalog, index, file)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Point {
    BeforeRegistration,
    DuringPopulation,
    #[cfg(test)]
    BeforeAudit,
    AfterAudit,
    #[cfg(test)]
    AfterUpdate,
    AfterReady,
}
// Test-only process cuts are private; production has no fault trait or env hook.
pub(super) fn test_point(point: Point) -> Result<()> {
    #[cfg(test)]
    {
        return test_hooks::at(point);
    }
    #[cfg(not(test))]
    {
        let _ = point;
        Ok(())
    }
}
#[cfg(test)]
pub(super) mod test_hooks {
    use super::*;
    std::thread_local! { static HOOK: std::cell::RefCell<Option<Box<dyn Fn(Point) -> Result<()>>>> = const { std::cell::RefCell::new(None) }; }
    pub fn at(point: Point) -> Result<()> {
        HOOK.with(|hook| match &*hook.borrow() {
            Some(hook) => hook(point),
            None => Ok(()),
        })
    }
    pub fn scoped<T>(hook: impl Fn(Point) -> Result<()> + 'static, work: impl FnOnce() -> T) -> T {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                HOOK.with(|h| *h.borrow_mut() = None);
            }
        }
        HOOK.with(|h| {
            assert!(h.borrow().is_none());
            *h.borrow_mut() = Some(Box::new(hook));
        });
        let _reset = Reset;
        work()
    }
}
