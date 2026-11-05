//! Retained V2 metadata proof. No source, Session, host lease or Store authority.
use super::{
    HYDRATION_POLICY,
    api::{Api, check},
    control_directory::ControlDirectoryGuard,
    control_directory_io as io, fs, population,
};
use crate::{Catalog, Error, Registration, Result};
use std::{
    ffi::OsStr,
    fs::File,
    mem::{offset_of, size_of, size_of_val},
    path::Path,
    sync::Arc,
};
use windows_sys::{
    Win32::Storage::{CloudFilters::*, FileSystem::*},
    core::GUID,
};

const PROVIDER_NAME: &str = "Layrs disposable VFS prototype";
const PROVIDER_VERSION: &str = "2";

/// A complete V2 registration bound to retained root/control pins and a frozen
/// catalog. This is a metadata capability, not authentication of Store receipts
/// or an arbitrary SnapshotSource. No connection or user-data read is acquired.
/// FORMAT and the exact canonical root spelling are immutable for revision 1.
pub struct ProviderRootGuard {
    api: Arc<Api>,
    registration: Registration,
    catalog: Arc<Catalog>,
    control: ControlDirectoryGuard,
}

impl ProviderRootGuard {
    /// Creates metadata only. The caller must quiesce namespace, metadata AND
    /// registration changes through audit/update/readback. Pins are not a
    /// registration lock or an atomic namespace snapshot. The callback precedes
    /// the first OS registration; an error has unknown external receipt durability.
    /// Later errors retain the native root, registration and all created files.
    pub fn create_with_control_directory(
        control: &ControlDirectoryGuard,
        root_id: [u8; 16],
        catalog: Arc<Catalog>,
        persist: impl FnOnce(&Registration) -> Result<()>,
    ) -> Result<Self> {
        let control = control.try_clone()?;
        let catalog = Arc::new(Catalog::new(catalog.version(), catalog.entries().to_vec())?);
        let registration = Registration::for_control_directory(
            control.identity(),
            root_id,
            catalog.identity().clone(),
        )?;
        let api = Arc::new(Api::load()?);
        control.revalidate_bootstrap_root()?;
        // All catalog paths must fit native calls before persistence/mutation.
        population::validate_native_paths(control.root(), &catalog)?;
        persist(&registration)?;
        control.revalidate_bootstrap_root()?;
        population::test_point(population::Point::BeforeRegistration)?;
        register(&api, &registration, false, false)?;
        verify_bound(&api, &registration, &catalog, &control, false)?;
        let created = population::populate(&api, &registration, &catalog, &control)?;
        #[cfg(test)]
        population::test_point(population::Point::BeforeAudit)?;
        let verified = population::VerifiedPopulation::audit(
            &api,
            &registration,
            &catalog,
            &control,
            created,
        )?;
        population::test_point(population::Point::AfterAudit)?;
        // Only this private token can authorize the READY update. Its exact
        // audit handles remain live through the update and READY readback.
        verified.mark_ready(&api, &registration, &catalog, &control)?;
        population::test_point(population::Point::AfterReady)?;
        let proof = Self {
            api,
            registration,
            catalog,
            control,
        };
        proof.revalidate()?;
        drop(verified);
        Ok(proof)
    }

    pub fn root(&self) -> &Path {
        self.control.root()
    }
    pub fn registration(&self) -> &Registration {
        &self.registration
    }
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }
    pub fn control_directory(&self) -> &ControlDirectoryGuard {
        &self.control
    }
    pub fn filesystem_identity(&self) -> String {
        self.registration.filesystem_identity()
    }
    pub fn revalidate(&self) -> Result<()> {
        verify_bound(
            &self.api,
            &self.registration,
            &self.catalog,
            &self.control,
            true,
        )
    }
    pub fn try_clone(&self) -> Result<Self> {
        self.revalidate()?;
        let value = Self {
            api: self.api.clone(),
            registration: self.registration.clone(),
            catalog: self.catalog.clone(),
            control: self.control.try_clone()?,
        };
        value.revalidate()?;
        Ok(value)
    }
    pub(super) fn frozen_catalog(&self) -> Arc<Catalog> {
        self.catalog.clone()
    }
}

impl Registration {
    /// Reacquires a retained complete V2 proof without connecting or repopulating.
    /// READY attests the initial audit, not equality of today's user namespace:
    /// later private edits and deletions are preserved. Expected data must be
    /// independently bound to the caller's authenticated durable receipt.
    pub fn guard_with_control_directory(
        &self,
        catalog: Arc<Catalog>,
        control: &ControlDirectoryGuard,
    ) -> Result<ProviderRootGuard> {
        self.matches_control(control.identity())?; // V1 refusal before cloning/native effects.
        if self.catalog_id != *catalog.identity() {
            return Err(Error::IdentityMismatch);
        }
        let value = ProviderRootGuard {
            api: Arc::new(Api::load()?),
            registration: self.clone(),
            catalog,
            control: control.try_clone()?,
        };
        value.revalidate()?;
        Ok(value)
    }
}

pub(super) fn verify_bound(
    api: &Api,
    registration: &Registration,
    catalog: &Catalog,
    control: &ControlDirectoryGuard,
    ready: bool,
) -> Result<()> {
    registration.matches_control(control.identity())?;
    if registration.catalog_id != *catalog.identity() {
        return Err(Error::IdentityMismatch);
    }
    control.revalidate()?;
    registration_boundary(api, registration, control, ready)?;
    control.revalidate()?;
    // Last boundary follows all opens/layout checks. Held queries are required
    // even when a fresh observer succeeds; failure is not silently downgraded.
    registration_boundary(api, registration, control, ready)
}

fn registration_boundary(
    api: &Api,
    registration: &Registration,
    control: &ControlDirectoryGuard,
    ready: bool,
) -> Result<()> {
    // Pins are ordered from the volume to the retained leaf. Their requested
    // spelling may differ from the native canonical root; spelling is checked
    // below as identity evidence, never used to classify a leaf as an ancestor.
    let mut pins = control.root.held_paths().peekable();
    while let Some((path, held, identity)) = pins.next() {
        let current = fs::open(
            path,
            io::ALL_SHARING,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        )?;
        if io::id(held)? != identity || io::id(&current)? != identity {
            return Err(Error::IdentityMismatch);
        }
        if pins.peek().is_none() {
            if identity != (registration.volume, registration.file_index)
                || io::canonical(held)?
                    != registration.root.to_str().ok_or(Error::IdentityMismatch)?
                || io::canonical(&current)?
                    != registration.root.to_str().ok_or(Error::IdentityMismatch)?
            {
                return Err(Error::IdentityMismatch);
            }
            // root.revalidate checks exposed supported type on held and fresh.
            control.root.revalidate(true)?;
            for file in [held, &current] {
                let attributes = fs::attributes(file)?;
                let state =
                    unsafe { (api.state)(attributes.FileAttributes, attributes.ReparseTag) };
                if attributes.FileAttributes
                    & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
                    != FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT
                    || (attributes.ReparseTag & 0xffff0fff) != 0x9000001a
                    || state == CF_PLACEHOLDER_STATE_INVALID
                    || state & CF_PLACEHOLDER_STATE_SYNC_ROOT == 0
                {
                    return Err(Error::ForeignRoot);
                }
            }
            verify_native(
                api,
                held,
                registration,
                ready,
                "CfGetSyncRootInfo(V2 held root)",
            )?;
            verify_native(
                api,
                &current,
                registration,
                ready,
                "CfGetSyncRootInfo(V2 fresh root)",
            )?;
        } else {
            io::plain_directory(held)?;
            io::plain_directory(&current)?;
            io::not_under(api, held)?;
            io::not_under(api, &current)?;
        }
    }
    let path = control.root().join(".layrs");
    let directory = io::directory_observer(&path)?;
    io::control_directory_attributes(&control.directory)?;
    io::control_directory_attributes(&directory)?;
    if io::id(&directory)? != io::id(&control.directory)?
        || io::canonical(&directory)? != path.to_str().ok_or(Error::IdentityMismatch)?
    {
        return Err(Error::IdentityMismatch);
    }
    verify_native(
        api,
        &control.directory,
        registration,
        ready,
        "CfGetSyncRootInfo(V2 held .layrs)",
    )?;
    verify_native(
        api,
        &directory,
        registration,
        ready,
        "CfGetSyncRootInfo(V2 fresh .layrs)",
    )?;
    let path = path.join("FORMAT");
    let format = fs::open(&path, io::ALL_SHARING, FILE_READ_ATTRIBUTES)?;
    io::format_attributes(&control.format)?;
    io::format_attributes(&format)?;
    if io::id(&format)? != io::id(&control.format)?
        || io::canonical(&format)? != path.to_str().ok_or(Error::IdentityMismatch)?
    {
        return Err(Error::IdentityMismatch);
    }
    verify_native(
        api,
        &control.format,
        registration,
        ready,
        "CfGetSyncRootInfo(V2 held FORMAT)",
    )?;
    verify_native(
        api,
        &format,
        registration,
        ready,
        "CfGetSyncRootInfo(V2 fresh FORMAT)",
    )?;
    verify_native(
        api,
        control.root.file(),
        registration,
        ready,
        "CfGetSyncRootInfo(V2 final held root)",
    )
}

pub(super) fn verify_native(
    api: &Api,
    file: &File,
    registration: &Registration,
    ready: bool,
    operation: &'static str,
) -> Result<()> {
    let mut storage = [0u64; 1024];
    let mut returned = 0;
    check(operation, unsafe {
        (api.root_info)(
            fs::handle(file),
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
    // Full storage is initialized/aligned; no variable-length reference is made.
    let info = unsafe { &*storage.as_ptr().cast::<CF_SYNC_ROOT_STANDARD_INFO>() };
    let length = info.SyncRootIdentityLength as usize;
    if length != 183 || offset + length > returned as usize {
        return Err(Error::IdentityMismatch);
    }
    let blob =
        unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>().add(offset), length) };
    if ready && blob == registration.root_blob(false) {
        return Err(Error::NeedsRepair(
            "owned V2 population is incomplete; automatic repopulation refused",
        ));
    }
    if blob != registration.root_blob(ready)
        || info.SyncRootFileId as u64 != registration.file_index
        || info.HydrationPolicy.Primary != HYDRATION_POLICY
        || info.HydrationPolicy.Modifier != CF_HYDRATION_POLICY_MODIFIER_NONE
        || info.PopulationPolicy.Primary != CF_POPULATION_POLICY_ALWAYS_FULL
        || info.PopulationPolicy.Modifier != CF_POPULATION_POLICY_MODIFIER_NONE
        || info.InSyncPolicy != CF_INSYNC_POLICY_TRACK_ALL
        || info.HardLinkPolicy != CF_HARDLINK_POLICY_NONE
        || !exact_utf16(&info.ProviderName, PROVIDER_NAME)
        || !exact_utf16(&info.ProviderVersion, PROVIDER_VERSION)
    {
        return Err(Error::ForeignRoot);
    }
    // ProviderStatus is dynamic. STANDARD_INFO cannot read back ProviderId,
    // root FileIdentity, PlaceholderManagement or disable-on-demand-root flag.
    Ok(())
}
fn exact_utf16(value: &[u16], expected: &str) -> bool {
    let Some(end) = value.iter().position(|unit| *unit == 0) else {
        return false;
    };
    String::from_utf16(&value[..end]).is_ok_and(|value| value == expected)
}

pub(super) fn register(api: &Api, ticket: &Registration, ready: bool, update: bool) -> Result<()> {
    let name = fs::wide(OsStr::new(PROVIDER_NAME))?;
    let version = fs::wide(OsStr::new(PROVIDER_VERSION))?;
    let path = fs::native_path(&ticket.root)?;
    let identity = ticket.root_blob(ready);
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
    let flags = CF_REGISTER_FLAG_DISABLE_ON_DEMAND_POPULATION_ON_ROOT
        | if update {
            CF_REGISTER_FLAG_UPDATE
        } else {
            CF_REGISTER_FLAG_NONE
        };
    check("CfRegisterSyncRoot(V2)", unsafe {
        (api.register)(path.as_ptr(), &registration, &policies, flags)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_strings_require_exact_bounded_terminated_utf16() {
        assert!(exact_utf16(&[b'2' as u16, 0, 0xffff], "2"));
        for bad in [
            vec![b'2' as u16],
            vec![0xd800, 0],
            vec![b'2' as u16, b' ' as u16, 0],
            vec![0, b'2' as u16],
        ] {
            assert!(!exact_utf16(&bad, "2"));
        }
    }
}
