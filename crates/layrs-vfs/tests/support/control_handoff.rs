//! TEST ONLY: retained immutable FORMAT object across a no-replace directory
//! rename. This neither changes the original failing recipe nor implements a
//! production guard. Native refusal is terminal; no close-and-retry fallback.
use super::{disposable_run, native_info};
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    mem::{offset_of, size_of, size_of_val},
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::{Component, Path, PathBuf, Prefix},
};
use windows_sys::Win32::{
    Foundation::ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT,
    Storage::{CloudFilters::*, FileSystem::*},
};

#[path = "../../src/windows/api.rs"]
mod api;

const BYTES: &[u8] = b"Retained FORMAT object: temporary share READ|DELETE, never WRITE.\n";
type NativeId = (u32, u64);

fn open(path: &Path, access: u32, sharing: u32) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(access)
        .share_mode(sharing)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

fn native_id(file: &File) -> NativeId {
    let info = native_info(file);
    (
        info.dwVolumeSerialNumber,
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    )
}

fn plain(file: &File, directory: bool) -> io::Result<()> {
    let mut attrs = FILE_ATTRIBUTE_TAG_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileAttributeTagInfo,
            (&mut attrs as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of_val(&attrs) as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || attrs.ReparseTag != 0
        || (attrs.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected ordinary no-follow object",
        ));
    }
    Ok(())
}

fn no_sync_root(api: &api::Api, file: &File) {
    let mut info = CF_SYNC_ROOT_BASIC_INFO::default();
    let hr = unsafe {
        (api.root_info)(
            file.as_raw_handle(),
            CF_SYNC_ROOT_INFO_BASIC,
            (&mut info as *mut CF_SYNC_ROOT_BASIC_INFO).cast(),
            size_of_val(&info) as u32,
            std::ptr::null_mut(),
        )
    };
    assert_eq!(
        hr,
        (0x80070000u32 | ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT) as i32,
        "test path must be outside EVERY sync root, HRESULT=0x{:08x}",
        hr as u32
    );
}

fn pins(api: &api::Api, path: &Path) -> Vec<File> {
    let mut components = path.components();
    assert!(
        matches!(components.next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)))
    );
    assert!(matches!(components.next(), Some(Component::RootDir)));
    assert!(components.all(|p| matches!(p, Component::Normal(_))));
    let spelling = path.to_str().unwrap().replace('/', "\\");
    assert!(!spelling[3..]
        .split('\\')
        .any(|p| matches!(p, "." | "..") || p.ends_with(['.', ' ']) || p.contains(':')));
    let mut held = Vec::new();
    for parent in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let file = open(
            parent,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        )
        .unwrap();
        plain(&file, true).unwrap();
        // Also reject compatibility-disguised cloud ancestors, before traversal
        // to the next component. This recipe never registers a sync root.
        no_sync_root(api, &file);
        held.push(file);
    }
    held
}

fn write_new(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn rename_no_replace(file: &File, path: &Path) -> io::Result<()> {
    let value = path.to_str().unwrap().replace('/', "\\");
    assert!(!value.starts_with("\\\\"));
    let name: Vec<u16> = format!("\\\\?\\{value}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let length = (offset_of!(FILE_RENAME_INFO, FileName) + name.len() * 2)
        .max(size_of::<FILE_RENAME_INFO>());
    let mut storage = vec![0u64; length.div_ceil(8)];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = ((name.len() - 1) * 2) as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            storage
                .as_mut_ptr()
                .cast::<u8>()
                .add(offset_of!(FILE_RENAME_INFO, FileName))
                .cast::<u16>(),
            name.len(),
        );
        if SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfo,
            info.cast(),
            length as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct Expected {
    directory: NativeId,
    format: NativeId,
    size: u64,
    links: u32,
}

#[derive(Debug)]
enum Refusal {
    Io(io::Error),
    DirectoryIdentity,
    FormatIdentity,
    SizeOrLinks,
    BytesOrEof,
    Layout,
}
impl From<io::Error> for Refusal {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn inspect_bytes(mut file: &File, expected: Expected) -> std::result::Result<(), Refusal> {
    plain(file, false)?;
    let info = native_info(file);
    if native_id(file) != expected.format {
        return Err(Refusal::FormatIdentity);
    }
    let size = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
    if size != expected.size || info.nNumberOfLinks != expected.links || expected.links != 1 {
        return Err(Refusal::SizeOrLinks);
    }
    file.seek(SeekFrom::Start(0))?;
    // Fixed bounded fixture length, then real EOF; never an unbounded read.
    let mut bytes = [0u8; BYTES.len()];
    file.read_exact(&mut bytes)?;
    if bytes != BYTES || file.read(&mut [0u8; 1])? != 0 {
        return Err(Refusal::BytesOrEof);
    }
    Ok(())
}

fn exact_layout(path: &Path, single: &str) -> std::result::Result<(), Refusal> {
    let mut entries = std::fs::read_dir(path)?;
    let first = entries.next().transpose()?.ok_or(Refusal::Layout)?;
    if first.file_name() != OsString::from(single) || entries.next().transpose()?.is_some() {
        return Err(Refusal::Layout);
    }
    Ok(())
}

// A test-private result only. It is never converted into a production guard,
// registration, source gate or readiness receipt. The caller retains the root
// and original no-delete directory handle separately for this entire lifetime.
struct FinalPin(File);

fn final_pin(
    root: &Path,
    directory: &File,
    temporary: &File,
    expected: Expected,
) -> std::result::Result<FinalPin, Refusal> {
    let control = root.join(".layrs");
    plain(directory, true)?;
    // The original directory handle owns DELETE. Observers must share DELETE;
    // it remains the effective no-delete owner and is never exposed to callers.
    let observed = open(
        &control,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    )?;
    plain(&observed, true)?;
    if native_id(directory) != expected.directory || native_id(&observed) != expected.directory {
        return Err(Refusal::DirectoryIdentity);
    }
    let strong = open(
        &control.join("FORMAT"),
        FILE_READ_DATA | FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ,
    )?;
    // Both handles are alive for every comparison. Same bytes never substitute
    // for exact original backing/native identity.
    inspect_bytes(temporary, expected)?;
    inspect_bytes(&strong, expected)?;
    exact_layout(root, ".layrs")?;
    exact_layout(&control, "FORMAT")?;
    Ok(FinalPin(strong))
}

fn write_refused(path: &Path, phase: &str) {
    for access in [FILE_WRITE_DATA, FILE_APPEND_DATA] {
        let error = open(
            path,
            access | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        )
        .unwrap_err();
        assert_eq!(
            error.raw_os_error(),
            Some(32),
            "{phase}: original FORMAT write access={access:x} {}",
            path.display()
        );
    }
    eprintln!("{phase}: original FORMAT WRITE_DATA/APPEND_DATA opens refused32");
}

fn deletion_refused(path: &Path, phase: &str) {
    let error = open(
        path,
        DELETE | FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    )
    .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(32));
    assert_eq!(
        std::fs::remove_file(path).unwrap_err().raw_os_error(),
        Some(32)
    );
    assert_eq!(
        std::fs::rename(
            path,
            path.with_file_name(format!("rename-must-fail-{phase}"))
        )
        .unwrap_err()
        .raw_os_error(),
        Some(32)
    );
    eprintln!("{phase}: FORMAT DELETE open + actual deletion + actual rename refused32");
}

#[derive(Clone, Copy, Debug)]
enum Arm {
    Clean,
    SameBytesReplacement,
    ExtraChild,
    WrongRecordedId,
}
impl Arm {
    fn name(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::SameBytesReplacement => "same-bytes-replaced",
            Self::ExtraChild => "extra-child",
            Self::WrongRecordedId => "wrong-recorded-id",
        }
    }
}

fn arm(run: &Path, api: &api::Api, scenario: Arm) -> bool {
    let base = run.join(scenario.name());
    std::fs::create_dir(&base).unwrap();
    let _base_pins = pins(api, &base);
    let root = base.join("root");
    let stage = base.join("unique-stage");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&stage).unwrap();
    let _root_pins = pins(api, &root);
    assert!(std::fs::read_dir(&root).unwrap().next().is_none());
    write_new(&stage.join("FORMAT"), BYTES);
    let directory = open(
        &stage,
        DELETE | FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
    )
    .unwrap();
    plain(&directory, true).unwrap();
    let temporary = open(
        &stage.join("FORMAT"),
        FILE_READ_DATA | FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_DELETE,
    )
    .unwrap();
    let expected = Expected {
        directory: native_id(&directory),
        format: native_id(&temporary),
        size: BYTES.len() as u64,
        links: 1,
    };
    inspect_bytes(&temporary, expected).unwrap();
    exact_layout(&stage, "FORMAT").unwrap();
    write_refused(&stage.join("FORMAT"), "temporary-before-directory-rename");
    write_new(
        &base.join("original-identity.txt"),
        format!(
            "TEST ONLY {expected:?}\n{}\n",
            layrs_core::scm::FileId::from_bytes(BYTES)
        )
        .as_bytes(),
    );
    if let Err(error) = rename_no_replace(&directory, &root.join(".layrs")) {
        eprintln!("HANDOFF {:?}: directory rename REFUSED Win32={:?} HRESULT=0x{:08x} ({error}); temporary FORMAT remains open; no close/retry",
            scenario, error.raw_os_error(), 0x80070000u32 | error.raw_os_error().unwrap_or(0) as u32);
        assert!(!root.join(".layrs").exists());
        assert_eq!(native_id(&directory), expected.directory);
        inspect_bytes(&temporary, expected).unwrap();
        exact_layout(&stage, "FORMAT").unwrap();
        write_refused(&stage.join("FORMAT"), "temporary-after-refused-rename");
        return false;
    }
    eprintln!("HANDOFF {:?}: same-handle/no-replace directory rename SUCCESS with original FORMAT READ_DATA|ATTR/shareREAD|DELETE held", scenario);
    assert!(!stage.exists());
    let format = root.join(".layrs/FORMAT");
    assert_eq!(native_id(&directory), expected.directory);
    inspect_bytes(&temporary, expected).unwrap();
    write_refused(&format, "temporary-after-directory-rename");
    let mut original_path = format.clone();
    let mut supplied = expected;
    match scenario {
        Arm::Clean => {}
        Arm::SameBytesReplacement => {
            // Rename the original into a retained external witness, preserving
            // its sole link and bytes. It is never deleted to stage the race.
            original_path = base.join("retained-original-FORMAT");
            let moving = open(
                &format,
                DELETE | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            )
            .unwrap();
            assert_eq!(native_id(&moving), expected.format);
            rename_no_replace(&moving, &original_path).unwrap();
            drop(moving); // No DELETE-access handle interferes with final pin.
            write_new(&format, BYTES);
            let replacement = open(
                &format,
                FILE_READ_DATA | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            )
            .unwrap();
            assert_ne!(native_id(&replacement), expected.format);
            assert_eq!(replacement.metadata().unwrap().len(), expected.size);
            eprintln!("SAME-BYTE REPLACEMENT: original={:?}, replacement={:?}; identical length/bytes, original remains retained by handle+external name", expected.format, native_id(&replacement));
        }
        Arm::ExtraChild => write_new(
            &root.join(".layrs/EXTRA"),
            b"unexpected control child retained\n",
        ),
        Arm::WrongRecordedId => {
            let witness = base.join("other-native-identity");
            write_new(&witness, BYTES);
            let witness = open(
                &witness,
                FILE_READ_DATA | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ,
            )
            .unwrap();
            supplied.format = native_id(&witness);
            assert_ne!(supplied.format, expected.format);
        }
    }
    inspect_bytes(&temporary, expected).unwrap();
    write_refused(&original_path, "temporary-after-gap-injection");
    match (scenario, final_pin(&root, &directory, &temporary, supplied)) {
        (Arm::Clean, Ok(strong)) => {
            // Establish protection while BOTH original temporary and new strong
            // handles remain open, then release only the temporary reference.
            inspect_bytes(&strong.0, expected).unwrap();
            inspect_bytes(&temporary, expected).unwrap();
            write_refused(&format, "both-pins-held");
            deletion_refused(&format, "both-held");
            drop(temporary);
            inspect_bytes(&strong.0, expected).unwrap();
            write_refused(&format, "strong-after-temporary-drop");
            deletion_refused(&format, "strong-only");
            exact_layout(&root.join(".layrs"), "FORMAT").unwrap();
            eprintln!("CLEAN HANDOFF: strong pin acquired before temporary drop; exact dir/FORMAT native IDs/size/links/fullbytes/EOF; post-handoff write/delete/rename refused32");
            drop(strong);
        }
        (Arm::SameBytesReplacement | Arm::WrongRecordedId, Err(Refusal::FormatIdentity))
        | (Arm::ExtraChild, Err(Refusal::Layout)) => {
            // Refusal never returns a final pin/capability or closes the
            // original temporary object before checking its preservation.
            inspect_bytes(&temporary, expected).unwrap();
            write_refused(&original_path, "temporary-after-validation-refusal");
            eprintln!("NEGATIVE {:?}: exact validation refused; no final pin issued; original native ID/size/linkcount/full bytes/EOF intact and readable throughout", scenario);
        }
        (_, Err(Refusal::Io(error))) => panic!("HANDOFF {:?}: native final-pin failure Win32={:?}, detail={error}; fixture retained; no close/retry", scenario, error.raw_os_error()),
        (_, Err(error)) => panic!("HANDOFF {:?}: unexpected proof refusal {error:?}; fixture retained", scenario),
        (_, Ok(_)) => panic!("HANDOFF {:?}: replacement/layout/recorded identity wrongly accepted", scenario),
    }
    true
}

pub(super) fn run() {
    let api = api::Api::load().unwrap();
    let parent = PathBuf::from(
        std::env::var_os("LAYRS_VFS_TEST_PARENT").expect("explicit disposable parent required"),
    );
    let _parent_pins = pins(&api, &parent);
    let (run, _) = disposable_run("control-handoff");
    eprintln!("HANDOFF PREFLIGHT: all retained ancestors outside every sync root; exact accepted HRESULT=0x80070186; no CF registration/connection/source adapter used");
    let outcomes = [
        Arm::Clean,
        Arm::SameBytesReplacement,
        Arm::ExtraChild,
        Arm::WrongRecordedId,
    ]
    .map(|scenario| arm(&run, &api, scenario));
    eprintln!("RETAINED FORMAT HANDOFF: clean/replaced/extra/wrong-id arms={outcomes:?}; root {}; original failing recipe unchanged; no production authority inferred", run.display());
    assert!(
        outcomes.into_iter().all(|passed| passed),
        "retained FORMAT handoff feasibility failed; no close-and-retry fallback authorized"
    );
}
