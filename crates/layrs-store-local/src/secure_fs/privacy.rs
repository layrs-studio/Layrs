//! Owner-only directories for resumable transfer data; never weaken an existing
//! directory's ACL, follow a symlink, or expose bytes before applying privacy.
use super::*;

/// A temporary spool whose bytes are private from its creation onward.
/// No reusable pathname is exposed. Unix unlinks immediately; Windows uses a
/// protected owner-only DACL, exclusive sharing and delete-on-last-close.
pub fn private_spool_file() -> io::Result<File> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    for _ in 0..16 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let name = format!(
            "layrs-private-spool-{}-{nanos:032x}-{:016x}.tmp",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(name);
        let file = match create_spool(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        #[cfg(unix)]
        fs::remove_file(&path)?;
        return Ok(file);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "private spool creation collision limit",
    ))
}

#[cfg(unix)]
fn create_spool(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    if file.metadata()?.mode() & 0o777 != 0o600 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "spool requires mode0600",
        ));
    }
    Ok(file)
}

#[cfg(windows)]
fn create_spool(path: &Path) -> io::Result<File> {
    use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
    use windows_sys::Win32::{
        Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE},
        Security::SECURITY_ATTRIBUTES,
        Storage::FileSystem::{
            CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_DELETE_ON_CLOSE,
        },
    };
    let descriptor = windows::private_descriptor()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let mut name: Vec<_> = path.as_os_str().encode_wide().collect();
    if name.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "spool path contains NUL",
        ));
    }
    name.push(0);
    // SAFETY: the terminated filename and security descriptor outlive this
    // call. CREATE_NEW never opens/replaces a preexisting file or reparse leaf;
    // sharing=0 and delete-on-close remain enforced for the handle lifetime.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_DELETE_ON_CLOSE,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ownership of the newly created valid handle transfers to File.
    let file = unsafe { File::from_raw_handle(handle) };
    windows::verify(&file)?;
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn create_spool(_: &Path) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private spool unsupported on this platform",
    ))
}

impl PinnedDirectory {
    /// Exclusively create a directory with a protected current-owner-only DACL.
    /// A failure after creation retains the object; this helper never cleans up
    /// uncertain state or opens/adopts a colliding entry.
    /// Native errors are preserved. Win32 error 5 alone must not be used to infer
    /// a collision, its underlying cause, or whether retry is appropriate.
    #[cfg(windows)]
    pub(crate) fn create_new_private_child(&self, name: &OsStr) -> io::Result<Self> {
        validate_new_private_name(name)?;
        self.verify_path_identity()?;
        let descriptor = windows::private_descriptor()?;
        let file = platform::create_new_private_directory_at(&self.file, name, descriptor.0)?;
        let display_path = self.display_path.join(name);
        validate_directory_metadata(&display_path, &file.metadata()?)?;
        windows::verify_protected_private(&file)?;
        let child = Self {
            identity: file_identity(&file)?,
            file,
            display_path,
        };
        child.sync()?;
        self.sync()?;
        self.verify_path_identity()?;
        child.verify_path_identity()?;
        Ok(child)
    }

    /// Exclusively create an empty file with a protected current-owner-only DACL.
    /// The caller owns subsequent writes and must sync and close every file handle
    /// (including duplicates) before handing FORMAT to the provider publisher.
    /// Failure never removes or re-ACLs an existing or newly created object.
    /// As with directory allocation, Win32 error 5 does not classify the cause
    /// and grants no authority to adopt, clean up, or retry a colliding entry.
    #[cfg(windows)]
    pub(crate) fn create_new_private_file(&self, name: &OsStr) -> io::Result<PinnedFile> {
        validate_new_private_name(name)?;
        self.verify_path_identity()?;
        let descriptor = windows::private_descriptor()?;
        let file = platform::create_new_private_file_at(&self.file, name, descriptor.0)?;
        windows::verify_protected_private(&file)?;
        let created = self.wrap_file(name, file)?;
        created.file().sync_all()?;
        self.sync()?;
        self.verify_path_identity()?;
        let observed = self.open_file(name)?;
        if !created.same_file(&observed) {
            return Err(identity_changed(&self.display_path.join(name)));
        }
        created.verify_parent()?;
        Ok(created)
    }

    pub(crate) fn open_private_child(&self, name: &OsStr) -> io::Result<Self> {
        self.open_existing_private_child(name, false)
    }
    /// Reopen an existing private directory for journal mutation, without
    /// creating a missing directory or changing its owner/permissions.
    pub(crate) fn open_private_child_for_write(&self, name: &OsStr) -> io::Result<Self> {
        self.open_existing_private_child(name, true)
    }
    fn open_existing_private_child(&self, name: &OsStr, writable: bool) -> io::Result<Self> {
        validate_name(name)?;
        self.verify_path_identity()?;
        let file = platform::open_existing_private_directory_at(&self.file, name, writable)?;
        let display_path = self.display_path.join(name);
        validate_directory_metadata(&display_path, &file.metadata()?)?;
        let child = Self {
            identity: file_identity(&file)?,
            file,
            display_path,
        };
        verify_owner_only(&child.file)?;
        child.verify_path_identity()?;
        Ok(child)
    }
    pub(crate) fn ensure_private_child(&self, name: &OsStr) -> io::Result<Self> {
        validate_name(name)?;
        #[cfg(unix)]
        let file = platform::open_private_directory_at(&self.file, name)?;
        #[cfg(windows)]
        let file = {
            let descriptor = windows::private_descriptor()?;
            platform::open_private_directory_at(&self.file, name, descriptor.0)?
        };
        #[cfg(not(any(unix, windows)))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "private transfer cache unsupported",
        ));
        #[cfg(any(unix, windows))]
        {
            let metadata = file.metadata()?;
            let display_path = self.display_path.join(name);
            validate_directory_metadata(&display_path, &metadata)?;
            verify_owner_only(&file)?;
            let result = Self {
                identity: file_identity(&file)?,
                file,
                display_path,
            };
            self.sync()?;
            result.sync()?;
            result.verify_path_identity()?;
            Ok(result)
        }
    }
}

#[cfg(windows)]
fn validate_new_private_name(name: &OsStr) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    validate_name(name)?;
    // Components can normalize trailing separators, so also check the raw name.
    // ':' can target an alternate stream; neither separators nor NUL belong in
    // an exclusively allocated single leaf.
    if name.encode_wide().any(|unit| {
        unit == 0 || unit == u16::from(b':') || unit == u16::from(b'/') || unit == u16::from(b'\\')
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private allocation requires a raw leaf name without separators, streams or NUL",
        ));
    }
    let text = name.to_string_lossy();
    let stem = text
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let numbered_device = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2"
                    | "3"
                    | "4"
                    | "5"
                    | "6"
                    | "7"
                    | "8"
                    | "9"
                    | "\u{b9}"
                    | "\u{b2}"
                    | "\u{b3}"
            )
        });
    // These spellings can alias another leaf or a DOS device when a later
    // consumer uses Win32 path resolution instead of this relative NT handle.
    if text.ends_with(['.', ' '])
        || matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
        )
        || numbered_device
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private allocation requires a canonical leaf name without DOS device aliases",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn verify_owner_only(file: &File) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no arguments or mutable output.
    if metadata.uid() != unsafe { geteuid() } || metadata.mode() & 0o777 != 0o700 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "transfer cache directory must be owned by current user and mode0700",
        ));
    }
    Ok(())
}
#[cfg(windows)]
fn verify_owner_only(file: &File) -> io::Result<()> {
    windows::verify(file)
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{
        ffi::c_void,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{Authorization::*, *},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    pub(super) struct Allocation(pub(super) *mut c_void);
    impl Drop for Allocation {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }
    fn user() -> io::Result<Vec<usize>> {
        let mut token = ptr::null_mut();
        // SAFETY: writable handle slot and process pseudo-handle are valid.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut length = 0;
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                ptr::null_mut(),
                0,
                &mut length,
            );
        }
        if length == 0 || length > 64 * 1024 {
            return Err(io::Error::other("invalid current user token size"));
        }
        let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(buffer)
    }
    pub(super) fn private_descriptor() -> io::Result<Allocation> {
        let user = user()?;
        let sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut value = ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(sid, &mut value) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let allocation = Allocation(value.cast());
        let mut length = 0;
        while unsafe { *value.add(length) } != 0 {
            length += 1;
            if length > 256 {
                return Err(io::Error::other("invalid user SID length"));
            }
        }
        let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(value, length) })
            .map_err(|_| io::Error::other("invalid user SID"))?;
        drop(allocation);
        let sddl: Vec<u16> = format!("O:{sid}D:P(A;OICI;FA;;;{sid})\0")
            .encode_utf16()
            .collect();
        let mut descriptor = ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Allocation(descriptor))
    }
    pub(super) fn verify(file: &File) -> io::Result<()> {
        let user = user()?;
        let user_sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut owner = ptr::null_mut();
        let mut acl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        let error = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut acl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        let _allocation = Allocation(descriptor);
        let rejected = || {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "transfer cache requires current owner and an owner-only ACL",
            )
        };
        if owner.is_null()
            || acl.is_null()
            || unsafe { EqualSid(owner, user_sid) } == 0
            || unsafe { (*acl).AceCount } != 1
        {
            return Err(rejected());
        }
        let mut ace = ptr::null_mut();
        if unsafe { GetAce(acl, 0, &mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        if allowed.Header.AceType != 0
            || allowed.Mask != 0x001f_01ff
            || unsafe { EqualSid(ptr::addr_of!(allowed.SidStart).cast_mut().cast(), user_sid) } == 0
        {
            return Err(rejected());
        }
        Ok(())
    }

    /// The stricter allocation contract is deliberately separate from the legacy
    /// verifier used by transfer directories and temporary spools.
    pub(super) fn verify_protected_private(file: &File) -> io::Result<()> {
        let user = user()?;
        let user_sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut owner = ptr::null_mut();
        let mut acl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        // SAFETY: file retains READ_CONTROL; output slots live through the call.
        let error = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut acl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        let _allocation = Allocation(descriptor);
        let mut control = 0u16;
        let mut revision = 0;
        // SAFETY: GetSecurityInfo supplied this live security descriptor.
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let rejected = || {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private allocation requires a protected current-owner-only DACL",
            )
        };
        if owner.is_null()
            || acl.is_null()
            || control & SE_DACL_PROTECTED == 0
            || unsafe { IsValidSid(owner) } == 0
            || unsafe { EqualSid(owner, user_sid) } == 0
            || unsafe { IsValidAcl(acl) } == 0
            || unsafe { (*acl).AceCount } != 1
        {
            return Err(rejected());
        }
        let mut ace = ptr::null_mut();
        // SAFETY: acl has one ACE and remains owned by the descriptor allocation.
        if unsafe { GetAce(acl, 0, &mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let acl_bytes = usize::from(unsafe { (*acl).AclSize });
        let ace_offset = (ace as usize)
            .checked_sub(acl as usize)
            .ok_or_else(rejected)?;
        let remaining = acl_bytes.checked_sub(ace_offset).ok_or_else(rejected)?;
        if ace_offset < std::mem::size_of::<ACL>() || remaining < std::mem::size_of::<ACE_HEADER>()
        {
            return Err(rejected());
        }
        let header = unsafe { ace.cast::<ACE_HEADER>().read_unaligned() };
        let sid_offset = std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
        if header.AceType != 0
            || u32::from(header.AceFlags) & (INHERIT_ONLY_ACE | INHERITED_ACE) != 0
            || usize::from(header.AceSize) < sid_offset + 8
            || usize::from(header.AceSize) > remaining
        {
            return Err(rejected());
        }
        // SAFETY: the ACE bound above contains the mask and the eight-byte SID
        // header. Check the encoded subauthority count before SID API reads.
        let mask = unsafe {
            ace.cast::<u8>()
                .add(std::mem::offset_of!(ACCESS_ALLOWED_ACE, Mask))
                .cast::<u32>()
                .read_unaligned()
        };
        let sid = unsafe { ace.cast::<u8>().add(sid_offset) };
        let sid_bytes = 8 + 4 * usize::from(unsafe { *sid.add(1) });
        if sid_bytes > usize::from(header.AceSize) - sid_offset
            || mask != 0x001f_01ff
            || unsafe { IsValidSid(sid.cast()) } == 0
            || unsafe { EqualSid(sid.cast(), user_sid) } == 0
        {
            return Err(rejected());
        }
        Ok(())
    }
}

#[cfg(all(test, windows))]
#[path = "privacy_creation_tests.rs"]
mod creation_tests;

#[cfg(all(test, windows))]
#[path = "provider_control_interop_tests.rs"]
mod provider_control_interop_tests;

#[cfg(test)]
mod spool_tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};

    fn path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "layrs-spool-test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn anonymous_spool_is_private_seekable_and_independent() {
        let mut first = private_spool_file().unwrap();
        let mut second = private_spool_file().unwrap();
        first.write_all(b"private binary\0\xff").unwrap();
        second.write_all(b"separate").unwrap();
        first.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        first.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"private binary\0\xff");
        assert_eq!(second.metadata().unwrap().len(), 8);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            unsafe extern "C" {
                fn geteuid() -> u32;
            }
            let metadata = first.metadata().unwrap();
            assert_eq!(metadata.mode() & 0o777, 0o600);
            assert_eq!(metadata.nlink(), 0);
            // SAFETY: geteuid has no arguments or mutable output.
            assert_eq!(metadata.uid(), unsafe { geteuid() });
        }
        #[cfg(windows)]
        windows::verify(&first).unwrap();
    }

    #[test]
    fn spool_creation_never_overwrites_an_existing_leaf() {
        let path = path("collision");
        fs::write(&path, b"retain original").unwrap();
        assert!(create_spool(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"retain original");
        #[cfg(unix)]
        {
            let alias = path.with_extension("symlink");
            std::os::unix::fs::symlink(&path, &alias).unwrap();
            assert!(create_spool(&alias).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"retain original");
            fs::remove_file(alias).unwrap();
        }
        fs::remove_file(path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_spool_has_owner_only_dacl_exclusive_sharing_and_delete_on_close() {
        let path = path("windows");
        let mut file = create_spool(&path).unwrap();
        windows::verify(&file).unwrap();
        file.write_all(b"private").unwrap();
        assert!(
            fs::File::open(&path).is_err(),
            "even the owner cannot open a second shared handle"
        );
        assert!(fs::OpenOptions::new().write(true).open(&path).is_err());
        drop(file);
        assert!(
            !path.exists(),
            "spool must disappear when its last handle closes"
        );
    }
}
