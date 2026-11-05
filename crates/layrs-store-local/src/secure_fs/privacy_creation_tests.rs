//! Windows allocation primitives only; provider observation/publication is a
//! separate integration gate. Retain these small disposable fixtures for review.
use super::*;
use std::{
    io::{Read, Seek, SeekFrom, Write},
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt, symlink_dir, symlink_file},
        io::AsRawHandle,
        process::CommandExt,
    },
    process::{Child, Command, Stdio},
    ptr,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
        Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SE_FILE_OBJECT,
            SetSecurityInfo,
        },
        CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetLengthSid,
        GetSecurityDescriptorControl, INHERIT_ONLY_ACE, INHERITED_ACE, IsValidAcl, IsValidSid,
        OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        SE_DACL_PROTECTED,
    },
    Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_DATA,
    },
};

const READ_CONTROL_ACCESS: u32 = 0x0002_0000;
const DELETE_ACCESS: u32 = 0x0001_0000;
const ALL_SHARING: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

struct Fixture(PathBuf);
impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "layrs-private-create-{label}-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        eprintln!("retained private allocation fixture: {}", path.display());
        Self(path)
    }
    fn pin(&self) -> PinnedDirectory {
        PinnedDirectory::open(&self.0).unwrap()
    }
}

fn observe(path: &Path) -> File {
    fs::OpenOptions::new()
        .access_mode(FILE_READ_DATA | FILE_READ_ATTRIBUTES | READ_CONTROL_ACCESS)
        .share_mode(ALL_SHARING)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .unwrap()
}

fn strong_read(path: &Path) -> io::Result<File> {
    fs::OpenOptions::new()
        .access_mode(FILE_READ_DATA | FILE_READ_ATTRIBUTES | READ_CONTROL_ACCESS)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[derive(Debug, PartialEq, Eq)]
struct Security {
    control: u16,
    owner_and_dacl: String,
}

fn security(file: &File) -> Security {
    let mut descriptor = ptr::null_mut();
    // SAFETY: the retained handle has READ_CONTROL; the descriptor output lives
    // through the call and is released with LocalFree via Allocation.
    let error = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    assert_eq!(error, 0);
    let _descriptor = windows::Allocation(descriptor);
    let mut control = 0;
    let mut revision = 0;
    assert_ne!(
        unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
        0
    );
    let mut encoded = ptr::null_mut();
    let mut length = 0;
    assert_ne!(
        unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                1,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut encoded,
                &mut length,
            )
        },
        0
    );
    let _encoded = windows::Allocation(encoded.cast());
    assert!((1..=65536).contains(&length));
    assert!(!encoded.is_null());
    // The returned length is buffer capacity, not string length. Read only
    // within that capacity and stop at the first terminator; trailing storage
    // need not contain string data. Both native allocations remain live.
    let string_length = (0..length as usize)
        .find(|&index| unsafe { *encoded.add(index) } == 0)
        .expect("SDDL buffer has no NUL within its reported capacity");
    // SAFETY: the bounded scan established this initialized UTF-16 prefix.
    let units = unsafe { std::slice::from_raw_parts(encoded, string_length) };
    Security {
        control,
        owner_and_dacl: String::from_utf16(units).unwrap(),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct DirectSecurity {
    control: u16,
    owner: Vec<u8>,
    acl_revision: u8,
    ace_flags: u8,
    mask: u32,
    ace_sid: Vec<u8>,
}

// The mutated-ACE fixture uses native fields, not SDDL formatting or its buffer
// capacity. Every copied SID/ACE range stays within the validated descriptor.
fn direct_security(file: &File) -> DirectSecurity {
    let mut descriptor = ptr::null_mut();
    let mut owner = ptr::null_mut();
    let mut acl = ptr::null_mut();
    assert_eq!(
        unsafe {
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
        },
        0,
    );
    let _descriptor = windows::Allocation(descriptor);
    assert!(!owner.is_null() && !acl.is_null());
    let mut control = 0;
    let mut revision = 0;
    assert_ne!(
        unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
        0,
    );
    // SAFETY: owner and acl are outputs in the retained GetSecurityInfo result.
    assert_ne!(unsafe { IsValidSid(owner) }, 0);
    let owner_length = unsafe { GetLengthSid(owner) } as usize;
    assert!((8..=68).contains(&owner_length));
    assert_ne!(unsafe { IsValidAcl(acl) }, 0);
    assert_eq!(unsafe { (*acl).AceCount }, 1);
    let acl_length = usize::from(unsafe { (*acl).AclSize });
    let mut ace = ptr::null_mut();
    assert_ne!(unsafe { GetAce(acl, 0, &mut ace) }, 0);
    let ace_offset = (ace as usize).checked_sub(acl as usize).unwrap();
    assert!(ace_offset >= std::mem::size_of::<ACL>());
    let remaining = acl_length.checked_sub(ace_offset).unwrap();
    assert!(remaining >= std::mem::size_of::<ACE_HEADER>());
    let header = unsafe { ace.cast::<ACE_HEADER>().read_unaligned() };
    let sid_offset = std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
    assert_eq!(header.AceType, 0);
    assert!((sid_offset + 8..=remaining).contains(&usize::from(header.AceSize)));
    // SAFETY: the verified ACE range contains its mask and SID header. Bound
    // subauthorities before calling SID APIs or copying any SID data.
    let mask = unsafe {
        ace.cast::<u8>()
            .add(std::mem::offset_of!(ACCESS_ALLOWED_ACE, Mask))
            .cast::<u32>()
            .read_unaligned()
    };
    assert_eq!(mask, 0x001f_01ff);
    let sid = unsafe { ace.cast::<u8>().add(sid_offset) };
    let sid_length = 8 + 4 * usize::from(unsafe { *sid.add(1) });
    assert_eq!(sid_offset + sid_length, usize::from(header.AceSize));
    assert_ne!(unsafe { IsValidSid(sid.cast()) }, 0);
    assert_eq!(unsafe { GetLengthSid(sid.cast()) } as usize, sid_length);
    assert_ne!(unsafe { EqualSid(sid.cast(), owner) }, 0);
    DirectSecurity {
        control,
        owner: unsafe { std::slice::from_raw_parts(owner.cast::<u8>(), owner_length) }.to_vec(),
        acl_revision: unsafe { (*acl).AclRevision },
        ace_flags: header.AceFlags,
        mask,
        ace_sid: unsafe { std::slice::from_raw_parts(sid, sid_length) }.to_vec(),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ObjectState {
    identity: FileIdentity,
    attributes: u32,
    security: Security,
}
fn object_state(path: &Path) -> ObjectState {
    let file = observe(path);
    ObjectState {
        identity: file_identity(&file).unwrap(),
        attributes: file.metadata().unwrap().file_attributes(),
        security: security(&file),
    }
}

fn assert_collision(error: io::Error, allocator: &str, name: &str, expected_raw: i32) {
    eprintln!(
        "private allocation collision: allocator={allocator}, name={name}, raw={:?}, kind={:?}, error={error}",
        error.raw_os_error(),
        error.kind(),
    );
    assert_eq!(
        error.raw_os_error(),
        Some(expected_raw),
        "unexpected native collision cause for {allocator} over {name}",
    );
    assert_eq!(
        error.kind(),
        io::Error::from_raw_os_error(expected_raw).kind(),
    );
}

struct OwnedJunctionChild {
    child: Child,
    reaped: bool,
}
impl Drop for OwnedJunctionChild {
    fn drop(&mut self) {
        if !self.reaped {
            // Own exactly this child; never search for or terminate another PID.
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn log_prefix(path: &Path) -> String {
    let mut bytes = Vec::new();
    File::open(path)
        .unwrap()
        .take(2048)
        .read_to_end(&mut bytes)
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn create_junction(fixture: &Fixture, target: &Path, link: &Path) {
    for path in [target, link] {
        assert!(path.is_absolute() && path.starts_with(&fixture.0));
        assert!(
            !path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        );
        let value = path
            .to_str()
            .expect("junction fixture path must be Unicode");
        assert!(
            !value
                .chars()
                .any(|c| c.is_control()
                    || matches!(c, '"' | '%' | '!' | '^' | '&' | '|' | '<' | '>')),
            "junction fixture path cannot contain shell metacharacters",
        );
    }
    assert!(target.is_dir());
    assert_eq!(
        fs::symlink_metadata(link).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    let stdout = fixture.0.join("junction.stdout.log");
    let stderr = fixture.0.join("junction.stderr.log");
    let output = |path: &Path| {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap()
    };
    let system_root = PathBuf::from(std::env::var_os("SystemRoot").expect("Windows SystemRoot"));
    assert!(system_root.is_absolute());
    let mut command = Command::new(system_root.join("System32").join("cmd.exe"));
    command
        .args(["/d", "/v:off", "/c"])
        // Constant code only. Validated fixture paths are environment data;
        // AutoRun and delayed expansion are disabled, and the child is hidden.
        .raw_arg("mklink /J \"%LAYRS_PRIVATE_JUNCTION_LINK%\" \"%LAYRS_PRIVATE_JUNCTION_TARGET%\"")
        .env(
            "LAYRS_PRIVATE_JUNCTION_LINK",
            link.to_str().unwrap().replace('/', "\\"),
        )
        .env(
            "LAYRS_PRIVATE_JUNCTION_TARGET",
            target.to_str().unwrap().replace('/', "\\"),
        )
        .current_dir(&fixture.0)
        .creation_flags(0x0800_0000)
        .stdin(Stdio::null())
        .stdout(output(&stdout))
        .stderr(output(&stderr));
    let mut owned = OwnedJunctionChild {
        child: command
            .spawn()
            .expect("launch owned junction fixture child"),
        reaped: false,
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        for path in [&stdout, &stderr] {
            assert!(
                fs::metadata(path).unwrap().len() <= 65536,
                "junction output exceeded bound"
            );
        }
        if let Some(status) = owned.child.try_wait().unwrap() {
            owned.reaped = true;
            for path in [&stdout, &stderr] {
                assert!(
                    fs::metadata(path).unwrap().len() <= 65536,
                    "junction output exceeded bound"
                );
            }
            assert!(
                status.success(),
                "junction fixture failed ({status}); stdout: {}; stderr: {}",
                log_prefix(&stdout),
                log_prefix(&stderr),
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "junction fixture timed out; stdout: {}; stderr: {}",
            log_prefix(&stdout),
            log_prefix(&stderr),
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_ne!(
        fs::symlink_metadata(link).unwrap().file_attributes() & 0x400,
        0
    );
}

#[test]
fn exclusive_private_directory_and_file_are_protected_at_creation() {
    let f = Fixture::new("created");
    let parent = f.pin();
    let stage = parent.create_new_private_child("stage".as_ref()).unwrap();
    let stage_path = f.0.join("stage");
    assert_eq!(
        stage.identity_token(),
        PinnedDirectory::open(&stage_path).unwrap().identity_token()
    );
    assert_ne!(security(&stage.file).control & SE_DACL_PROTECTED, 0);
    windows::verify_protected_private(&stage.file).unwrap();
    let mut format = stage.create_new_private_file("FORMAT".as_ref()).unwrap();
    let format_path = stage_path.join("FORMAT");
    assert_eq!(format.file().metadata().unwrap().len(), 0);
    assert_ne!(security(format.file()).control & SE_DACL_PROTECTED, 0);
    windows::verify_protected_private(format.file()).unwrap();
    // A SHARE_ALL observer is compatible with the actual creator's WRITE/DELETE
    // handle before any bytes are written; it confers no provider authority.
    let empty = observe(&format_path);
    assert_eq!(
        file_identity(&empty).unwrap(),
        file_identity(format.file()).unwrap()
    );
    assert_eq!(empty.metadata().unwrap().len(), 0);
    drop(empty);
    format
        .file_mut()
        .write_all(b"owned FORMAT bytes\0\xff")
        .unwrap();
    format.file().sync_all().unwrap();
    stage.sync().unwrap();
    format.verify_parent().unwrap();
    stage.verify_path_identity().unwrap();
    let reopened = stage.open_file("FORMAT".as_ref()).unwrap();
    assert!(format.same_file(&reopened));
    assert_eq!(fs::read(&format_path).unwrap(), b"owned FORMAT bytes\0\xff");
    assert_eq!(fs::read_dir(&stage_path).unwrap().count(), 1);
}

#[test]
fn protected_allocation_verifier_refuses_inherited_unprotected_acl() {
    let f = Fixture::new("inherited");
    let parent = f.pin();
    let stage = parent.create_new_private_child("stage".as_ref()).unwrap();
    // Existing creation deliberately inherits; the legacy verifier must retain
    // its old acceptance while the new allocation verifier requires protection.
    let parent_security = direct_security(&stage.file);
    let inherited = stage.create_file("inherited".as_ref()).unwrap();
    let path = f.0.join("stage/inherited");
    let observed = observe(&path);
    assert!(inherited.same_file(&stage.open_file("inherited".as_ref()).unwrap()));
    let before = direct_security(&observed);
    assert_eq!(before.control & SE_DACL_PROTECTED, 0);
    assert_ne!(u32::from(before.ace_flags) & INHERITED_ACE, 0);
    assert_eq!(u32::from(before.ace_flags) & INHERIT_ONLY_ACE, 0);
    assert_eq!(before.owner, parent_security.owner);
    assert_eq!(before.ace_sid, parent_security.ace_sid);
    // This native control proves the combined inherited/unprotected refusal;
    // it does not isolate the INHERITED_ACE check from the protection check.
    windows::verify(&observed).unwrap();
    assert_eq!(
        windows::verify_protected_private(&observed)
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied,
    );
    assert_eq!(direct_security(&observed), before);
}

#[test]
fn protected_allocation_verifier_refuses_inherit_only_directory_ace() {
    let f = Fixture::new("ace-flags");
    let parent = f.pin();
    // R4's attempted inherit-only ACE on a regular file round-tripped as an
    // empty DACL. Use a container, where an inherit-only ACE governs children.
    // SetSecurityInfo's protected-DACL auto-inheritance clears INHERITED_ACE:
    // a protected+inherited descriptor was not constructed/qualified here.
    // The separate native inherited-file control above is not isolated coverage
    // of that production flag branch; do not normalize flags to claim otherwise.
    let name = "inherit-only-directory";
    let created = parent.create_new_private_child(name.as_ref()).unwrap();
    let mut witness = created.create_new_private_file("witness".as_ref()).unwrap();
    witness
        .file_mut()
        .write_all(b"private ACL witness")
        .unwrap();
    witness.file().sync_all().unwrap();
    let witness_identity = file_identity(witness.file()).unwrap();
    let witness_security = direct_security(witness.file());
    assert_ne!(witness_security.control & SE_DACL_PROTECTED, 0);
    let identity = file_identity(&created.file).unwrap();
    windows::verify_protected_private(&created.file).unwrap();
    let original = direct_security(&created.file);
    // Modify only this disposable container's DACL through its retained
    // WRITE_DAC handle. The child has its own protected DACL; all handles
    // remain live across mutation and verification. No privilege changes.
    let acl_handle = fs::OpenOptions::new()
        .access_mode(READ_CONTROL_ACCESS | FILE_READ_ATTRIBUTES | 0x0004_0000) // WRITE_DAC.
        .share_mode(ALL_SHARING)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(f.0.join(&name))
        .unwrap();
    let mut descriptor = ptr::null_mut();
    let mut acl = ptr::null_mut();
    assert_eq!(
        unsafe {
            GetSecurityInfo(
                acl_handle.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut acl,
                ptr::null_mut(),
                &mut descriptor,
            )
        },
        0,
    );
    let _descriptor = windows::Allocation(descriptor);
    assert_ne!(unsafe { IsValidAcl(acl) }, 0);
    assert_eq!(unsafe { (*acl).AceCount }, 1);
    let mut ace = ptr::null_mut();
    assert_ne!(unsafe { GetAce(acl, 0, &mut ace) }, 0);
    // SAFETY: GetAce returned the sole ACE in the live, valid descriptor;
    // only flags change, retaining the current-owner full-access SID/mask.
    unsafe {
        (*ace.cast::<ACE_HEADER>()).AceFlags =
            (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE | INHERIT_ONLY_ACE) as u8;
    }
    assert_ne!(unsafe { IsValidAcl(acl) }, 0);
    assert_eq!(
        unsafe {
            SetSecurityInfo(
                acl_handle.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                acl,
                ptr::null_mut(),
            )
        },
        0,
    );
    let before = direct_security(&acl_handle);
    assert_ne!(before.control & SE_DACL_PROTECTED, 0);
    assert_eq!(before.owner, original.owner);
    assert_eq!(before.ace_sid, original.ace_sid);
    assert_eq!(before.acl_revision, original.acl_revision);
    assert_eq!(before.mask, original.mask);
    assert_eq!(
        u32::from(before.ace_flags),
        OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE | INHERIT_ONLY_ACE,
    );
    // Discriminating control: the unchanged legacy verifier accepts this
    // protected owner-only descriptor, while allocation requires an
    // effective (not inherit-only) allow ACE. Exact native flags above are
    // required, so a normalized or empty DACL cannot pass this control.
    windows::verify(&acl_handle).unwrap();
    assert_eq!(
        windows::verify_protected_private(&acl_handle)
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied,
    );
    assert_eq!(direct_security(&acl_handle), before);
    assert_eq!(file_identity(&acl_handle).unwrap(), identity);
    assert_eq!(file_identity(&created.file).unwrap(), identity);
    assert_eq!(file_identity(witness.file()).unwrap(), witness_identity);
    assert_eq!(direct_security(witness.file()), witness_security);
    windows::verify_protected_private(witness.file()).unwrap();
    witness.file_mut().seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = Vec::new();
    witness.file_mut().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"private ACL witness");
}

#[test]
fn all_format_writer_duplicates_must_close_before_strong_read() {
    let f = Fixture::new("handoff");
    let parent = f.pin();
    let stage = parent.create_new_private_child("stage".as_ref()).unwrap();
    let mut format = stage.create_new_private_file("FORMAT".as_ref()).unwrap();
    let path = f.0.join("stage/FORMAT");
    format
        .file_mut()
        .write_all(b"complete canonical fixture\n")
        .unwrap();
    format.file().sync_all().unwrap();
    stage.sync().unwrap();
    let identity = file_identity(format.file()).unwrap();
    let original_security = security(format.file());
    let duplicate = format.file().try_clone().unwrap();
    assert_eq!(strong_read(&path).unwrap_err().raw_os_error(), Some(32));
    drop(format);
    assert_eq!(strong_read(&path).unwrap_err().raw_os_error(), Some(32));
    drop(duplicate);
    let mut strong = strong_read(&path).unwrap();
    assert_eq!(file_identity(&strong).unwrap(), identity);
    assert_eq!(security(&strong), original_security);
    let mut bytes = Vec::new();
    strong.seek(SeekFrom::Start(0)).unwrap();
    strong.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"complete canonical fixture\n");
    for access in [FILE_WRITE_DATA, DELETE_ACCESS] {
        let error = fs::OpenOptions::new()
            .access_mode(access)
            .share_mode(ALL_SHARING)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)
            .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(32));
    }
}

#[test]
fn file_and_directory_collisions_preserve_bytes_identity_and_security() {
    let f = Fixture::new("collisions");
    fs::write(f.0.join("existing-file"), b"accepted file witness").unwrap();
    fs::create_dir(f.0.join("existing-dir")).unwrap();
    fs::write(f.0.join("existing-dir/child"), b"accepted child witness").unwrap();
    let parent = f.pin();
    for name in ["existing-file", "existing-dir", "EXISTING-FILE"] {
        let path = f.0.join(name);
        let before = object_state(&path);
        assert_collision(
            parent.create_new_private_child(name.as_ref()).unwrap_err(),
            "directory",
            name,
            183, // STATUS_OBJECT_NAME_COLLISION -> ERROR_ALREADY_EXISTS.
        );
        assert_eq!(object_state(&path), before);
        assert_collision(
            parent.create_new_private_file(name.as_ref()).unwrap_err(),
            "file",
            name,
            // FILE_NON_DIRECTORY_FILE against a directory refuses with native
            // access denied; production must preserve that original cause.
            if name == "existing-dir" { 5 } else { 183 },
        );
        assert_eq!(object_state(&path), before);
    }
    assert_eq!(
        fs::read(f.0.join("existing-file")).unwrap(),
        b"accepted file witness"
    );
    assert_eq!(
        fs::read(f.0.join("existing-dir/child")).unwrap(),
        b"accepted child witness"
    );
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 2);
    assert_eq!(fs::read_dir(f.0.join("existing-dir")).unwrap().count(), 1);
}

#[test]
fn junction_collisions_and_reparsed_parent_preserve_every_witness() {
    let f = Fixture::new("junction");
    let parent_path = f.0.join("parent");
    let target = f.0.join("target-dir");
    fs::create_dir(&parent_path).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(target.join("child"), b"junction target witness").unwrap();
    let link = parent_path.join("junction");
    create_junction(&f, &target, &link);
    let parent = PinnedDirectory::open(&parent_path).unwrap();
    let parent_before = object_state(&parent_path);
    let link_before = object_state(&link);
    let link_target = fs::read_link(&link).unwrap();
    let target_before = object_state(&target);
    let child_before = object_state(&target.join("child"));
    assert_collision(
        parent
            .create_new_private_child("junction".as_ref())
            .unwrap_err(),
        "directory",
        "junction",
        183,
    );
    assert_eq!(object_state(&link), link_before);
    assert_collision(
        parent
            .create_new_private_file("junction".as_ref())
            .unwrap_err(),
        "file",
        "junction (directory reparse)",
        5,
    );
    assert_eq!(object_state(&link), link_before);
    assert_eq!(fs::read_link(&link).unwrap(), link_target);
    assert!(PinnedDirectory::open(&link).is_err());
    // A matching final directory inode reached through a reparse path is still
    // insufficient. Model it as the recorded parent and verify before creation.
    let mut redirected = PinnedDirectory::open(&target).unwrap();
    redirected.display_path = link;
    let directory_error = redirected
        .create_new_private_child("new-dir".as_ref())
        .unwrap_err();
    let file_error = redirected
        .create_new_private_file("new-file".as_ref())
        .unwrap_err();
    for error in [directory_error, file_error] {
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("symlink or reparse point"));
    }
    assert_eq!(object_state(&parent_path), parent_before);
    assert_eq!(object_state(&target), target_before);
    assert_eq!(object_state(&target.join("child")), child_before);
    assert_eq!(
        fs::read(target.join("child")).unwrap(),
        b"junction target witness"
    );
    assert_eq!(fs::read_dir(&parent_path).unwrap().count(), 1);
    assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
}

#[test]
#[ignore = "NOT qualified on this host; requires Windows symlink privilege or Developer Mode and an explicit opt-in run"]
fn reparse_collisions_and_reparsed_parent_never_follow_the_target() {
    let f = Fixture::new("reparse");
    fs::create_dir(f.0.join("parent")).unwrap();
    fs::create_dir(f.0.join("target-dir")).unwrap();
    fs::write(f.0.join("target-file"), b"outside allocation witness").unwrap();
    // No silent skip: this test requires Windows symlink creation support
    // (Developer Mode or the symlink privilege) in the qualification environment.
    symlink_dir(f.0.join("target-dir"), f.0.join("parent/dir-link"))
        .expect("Windows no-follow test requires symlink creation support");
    symlink_file(f.0.join("target-file"), f.0.join("parent/file-link"))
        .expect("Windows no-follow test requires symlink creation support");
    let parent = PinnedDirectory::open(&f.0.join("parent")).unwrap();
    let target_dir = object_state(&f.0.join("target-dir"));
    let target_file = object_state(&f.0.join("target-file"));
    for name in ["dir-link", "file-link"] {
        let path = f.0.join("parent").join(name);
        let before = object_state(&path);
        let link_target = fs::read_link(&path).unwrap();
        assert_collision(
            parent.create_new_private_child(name.as_ref()).unwrap_err(),
            "directory",
            name,
            183,
        );
        assert_collision(
            parent.create_new_private_file(name.as_ref()).unwrap_err(),
            "file",
            name,
            if name == "dir-link" { 5 } else { 183 },
        );
        assert_eq!(object_state(&path), before);
        assert_eq!(fs::read_link(&path).unwrap(), link_target);
    }
    let mut redirected = PinnedDirectory::open(&f.0.join("target-dir")).unwrap();
    redirected.display_path = f.0.join("parent/dir-link");
    assert_eq!(
        redirected
            .create_new_private_child("new-dir".as_ref())
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        redirected
            .create_new_private_file("new-file".as_ref())
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(object_state(&f.0.join("target-dir")), target_dir);
    assert_eq!(object_state(&f.0.join("target-file")), target_file);
    assert_eq!(
        fs::read(f.0.join("target-file")).unwrap(),
        b"outside allocation witness"
    );
    assert_eq!(fs::read_dir(f.0.join("target-dir")).unwrap().count(), 0);
}

#[test]
fn private_allocations_reject_traversal_stream_and_nul_names_without_effects() {
    let f = Fixture::new("names");
    let parent = f.pin();
    for name in [
        "",
        ".",
        "..",
        "../outside",
        "child/nested",
        "FORMAT:stream",
        "nul\0tail",
        "CON\\",
        "trailing.\\",
        "stage\\",
        "CON/",
        "trailing./",
        "stage/",
        "trailing.",
        "trailing ",
        "trailing. ",
        "CON",
        "cOn.txt",
        "PRN",
        "aux.log",
        "NuL.backup.txt",
        "CON .txt",
        "CLOCK$.txt",
        "conin$",
        "ConOut$.bin",
        "COM1",
        "com9.txt",
        "LPT1.log",
        "lPt9",
        "COM\u{b9}.txt",
        "lpt\u{b2}",
        "COM\u{b3}.bin",
    ] {
        assert_eq!(
            parent
                .create_new_private_child(name.as_ref())
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            parent
                .create_new_private_file(name.as_ref())
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }
    for name in [
        "COM10.txt",
        "LPT0",
        "auxiliary",
        "nul-backup",
        ".stage",
        "stage.tmp",
    ] {
        validate_new_private_name(name.as_ref()).unwrap();
    }
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 0);
}
