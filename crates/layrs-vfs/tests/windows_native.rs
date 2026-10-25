//! Opt-in REAL kernel/provider tests; never silently skipped after opt-in.
//! Set LAYRS_VFS_TEST_PARENT to an explicit disposable directory and run:
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact two_roots_real_cfapi --nocapture
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact control_publication_recovers_interrupted_writes --nocapture
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact exact_case_ordinary_replacement --nocapture
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact disconnected_placeholder_rename_probe --nocapture
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact relocated_placeholder_export_matrix --nocapture
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact export_refuses_missing_source_and_changed_identity --nocapture
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact export_dirty_partial_qualification --nocapture
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact export_seeded_dirty_partial_qualification --nocapture
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact disconnected_placeholder_visibility_and_root_probe --nocapture
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact retained_export_parent_guard --nocapture
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact control_directory_native_feasibility --nocapture
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact control_directory_retained_format_handoff --nocapture
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact control_directory_close_window_feasibility --nocapture
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact control_directory_close_window_r2 --nocapture
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact control_directory_step1_native --nocapture
//! cargo test -p layrs-vfs --test windows_native -- --ignored --exact control_directory_step1_process_recovery --nocapture
//! Roots and tickets are retained, including on failure; no automatic deletion.
#![cfg(windows)]

#[path = "support/compatibility.rs"]
mod compatibility;
#[path = "support/control_directory.rs"]
mod control_directory;
#[path = "support/control_handoff.rs"]
mod control_handoff;
#[path = "support/control_close_window.rs"]
mod control_close_window;
#[path = "support/control_close_window_r2.rs"]
mod control_close_window_r2;
#[path = "support/control_directory_step1.rs"]
mod control_directory_step1;
#[path = "support/export_parent.rs"]
mod export_parent;
#[path = "support/seeded_partial.rs"]
mod seeded_partial;

#[test]
#[ignore = "Step1 production API native gates; explicit disposable parent; registrations/fixtures retained"]
fn control_directory_step1_native() {
    control_directory_step1::run_native();
}

#[test]
#[ignore = "Step1 eight process-exit/resume boundaries; explicit disposable parent; all fixtures retained"]
fn control_directory_step1_process_recovery() {
    control_directory_step1::run_process();
}

#[test]
#[ignore = "internal owned child; only run through control_directory_step1_process_recovery"]
fn control_directory_step1_child() {
    control_directory_step1::run_child();
}

#[test]
#[ignore = "Stage1b R2 test-only gates; explicit disposable parent; registrations/fixtures retained"]
fn control_directory_close_window_r2() {
    control_close_window_r2::run();
}

#[test]
#[ignore = "Stage1b close-window feasibility only; explicit disposable parent; no production guard/readiness"]
fn control_directory_close_window_feasibility() {
    control_close_window::run();
}

#[test]
#[ignore = "Stage1b retained FORMAT handoff feasibility only; explicit disposable parent; no close/retry"]
fn control_directory_retained_format_handoff() {
    control_handoff::run();
}

#[test]
#[ignore = "Stage1b feasibility only; explicit disposable parent; strong FORMAT rename refusal fails gate"]
fn control_directory_native_feasibility() {
    control_directory::run();
}

#[test]
#[ignore = "real retained export-parent guards; explicit disposable parent required"]
fn retained_export_parent_guard() {
    export_parent::run();
}

#[test]
#[ignore = "real CFAPI metadata under forced disguise; explicit disposable parent required"]
fn disconnected_placeholder_visibility_and_root_probe() {
    compatibility::run();
}

#[test]
#[ignore = "native two-arm seeded mixed-state qualification; explicit UNQUALIFIED outcomes retained"]
fn export_seeded_dirty_partial_qualification() {
    seeded_partial::run();
}

use layrs_core::scm::{FileId, RepoPath};
use layrs_vfs::{
    Cancellation, CaptureEvidence, Catalog, CatalogEntry, ControlFileIdentity,
    ControlPublishFaults, Error, NoControlPublishFaults, Registration, Result, Session,
    SnapshotSource,
};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn native_open(path: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::*;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .unwrap()
}

fn native_info(
    file: &std::fs::File,
) -> windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::*;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    assert_ne!(
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) },
        0
    );
    info
}

fn native_file_id(path: &Path) -> (u32, u64) {
    let info = native_info(&native_open(path));
    (
        info.dwVolumeSerialNumber,
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    )
}

#[derive(Debug)]
struct PlaceholderProbe {
    native_id: (u32, u64),
    backing_identity: Vec<u8>,
    size: u64,
    on_disk: i64,
    modified: i64,
}

/// Metadata-only diagnostic, including after a rename that Session refuses.
/// Keep CFAPI dynamically loaded even in the opt-in test binary.
fn placeholder_probe(path: &Path) -> PlaceholderProbe {
    use std::{
        mem::{offset_of, size_of_val},
        os::windows::io::AsRawHandle,
    };
    use windows_sys::Win32::{
        Foundation::FreeLibrary, Storage::CloudFilters::*, System::LibraryLoader::*,
    };
    let file = native_open(path);
    let native = native_info(&file);
    let dll: Vec<u16> = "cldapi.dll\0".encode_utf16().collect();
    let module = unsafe {
        LoadLibraryExW(
            dll.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    assert!(!module.is_null());
    let symbol =
        unsafe { GetProcAddress(module, c"CfGetPlaceholderInfo".as_ptr().cast()) }.unwrap();
    let query = unsafe {
        std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            unsafe extern "system" fn(
                windows_sys::Win32::Foundation::HANDLE,
                CF_PLACEHOLDER_INFO_CLASS,
                *mut std::ffi::c_void,
                u32,
                *mut u32,
            ) -> i32,
        >(symbol)
    };
    let mut storage = [0u64; 1024];
    let mut returned = 0;
    let hr = unsafe {
        query(
            file.as_raw_handle(),
            CF_PLACEHOLDER_INFO_STANDARD,
            storage.as_mut_ptr().cast(),
            size_of_val(&storage) as u32,
            &mut returned,
        )
    };
    unsafe {
        FreeLibrary(module);
    }
    assert!(
        hr >= 0,
        "CfGetPlaceholderInfo probe HRESULT 0x{:08x}",
        hr as u32
    );
    let offset = offset_of!(CF_PLACEHOLDER_STANDARD_INFO, FileIdentity);
    assert!(returned as usize >= offset && returned as usize <= size_of_val(&storage));
    let info = unsafe { &*storage.as_ptr().cast::<CF_PLACEHOLDER_STANDARD_INFO>() };
    let length = info.FileIdentityLength as usize;
    assert!(length <= 4096 && offset + length <= returned as usize);
    PlaceholderProbe {
        native_id: (
            native.dwVolumeSerialNumber,
            (u64::from(native.nFileIndexHigh) << 32) | u64::from(native.nFileIndexLow),
        ),
        backing_identity: unsafe {
            std::slice::from_raw_parts(storage.as_ptr().cast::<u8>().add(offset), length)
        }
        .to_vec(),
        size: (u64::from(native.nFileSizeHigh) << 32) | u64::from(native.nFileSizeLow),
        on_disk: info.OnDiskDataSize,
        modified: info.ModifiedDataSize,
    }
}

// Test-only metadata mutation on an owned disposable placeholder. No data
// invalidation, dehydration, or identity adoption is performed by production.
fn update_placeholder(path: &Path, identity: &[u8], size: Option<u64>) -> i32 {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::{
        Foundation::{FreeLibrary, HANDLE},
        Storage::{CloudFilters::*, FileSystem::*},
        System::{LibraryLoader::*, IO::OVERLAPPED},
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(WRITE_DAC)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .unwrap();
    let name: Vec<u16> = "cldapi.dll\0".encode_utf16().collect();
    let module = unsafe {
        LoadLibraryExW(
            name.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    assert!(!module.is_null());
    let symbol = unsafe { GetProcAddress(module, c"CfUpdatePlaceholder".as_ptr().cast()) }.unwrap();
    let update = unsafe {
        std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            unsafe extern "system" fn(
                HANDLE,
                *const CF_FS_METADATA,
                *const std::ffi::c_void,
                u32,
                *const CF_FILE_RANGE,
                u32,
                CF_UPDATE_FLAGS,
                *mut i64,
                *mut OVERLAPPED,
            ) -> i32,
        >(symbol)
    };
    let metadata = size.map(|size| CF_FS_METADATA {
        BasicInfo: FILE_BASIC_INFO::default(),
        FileSize: i64::try_from(size).unwrap(),
    });
    let result = unsafe {
        update(
            file.as_raw_handle(),
            metadata
                .as_ref()
                .map_or(std::ptr::null(), |m| m as *const _),
            identity.as_ptr().cast(),
            identity.len() as u32,
            std::ptr::null(),
            0,
            CF_UPDATE_FLAG_NONE,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    unsafe {
        FreeLibrary(module);
    }
    result
}

fn export_cause(error: &Error) -> &Error {
    match error {
        Error::ExportRead { cause, .. } | Error::ExportConsumer { cause, .. } => {
            export_cause(cause)
        }
        other => other,
    }
}

fn assert_rename_capture_refused(session: &Session, path: &RepoPath) {
    assert!(matches!(
        session.checkpoint(),
        Err(Error::Unsupported(
            "placeholder renamed: identity is preserved, capture reconciliation required"
        ))
    ));
    assert!(matches!(
        session.inspect(path),
        Err(Error::Unsupported(
            "placeholder renamed: identity is preserved, capture reconciliation required"
        ))
    ));
}

#[test]
#[ignore = "requires disposable native root; faults original source and CFAPI identity metadata"]
fn export_refuses_missing_source_and_changed_identity() {
    let (run, nonce) = disposable_run("export-errors");
    let root = run.join("root");
    std::fs::create_dir(&root).unwrap();
    let source = Arc::new(Fixture::new());
    let mut session = Session::create_with_registration_hook(
        &root,
        nonce.to_le_bytes(),
        source.clone(),
        |ticket| {
            let mut f = std::fs::File::create(run.join("ticket"))?;
            f.write_all(&ticket.encode())?;
            f.sync_all()?;
            Ok(())
        },
    )
    .unwrap();
    session.disconnect().unwrap();
    std::fs::rename(root.join("untouched.bin"), root.join("relocated.bin")).unwrap();
    session.connect().unwrap();
    let path = RepoPath::parse("relocated.bin").unwrap();
    let native_path = root.join(path.as_str());
    let baseline = placeholder_probe(&native_path);
    source.deny_reads.store(true, Ordering::SeqCst);
    let mut checkpoint = session.export_checkpoint().unwrap();
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    let mut called = false;
    let failure = checkpoint
        .with_reader(&path, |_| {
            called = true;
            Ok(())
        })
        .unwrap_err();
    assert!(!called, "failed hydration invoked byte consumer");
    assert!(
        matches!(&failure, Error::ExportRead { path: actual, backing: Some(backing), cause } if actual == &path && backing == &FileId::from_bytes(&pattern()) && matches!(cause.as_ref(), Error::Windows { operation: "CfHydratePlaceholder(export)", hresult } if *hresult < 0))
    );
    assert!(source.reads.load(Ordering::SeqCst) > 0);
    assert_eq!(source.supplied.load(Ordering::SeqCst), 0);
    assert_eq!(
        placeholder_probe(&native_path).native_id,
        baseline.native_id
    );
    assert_eq!(placeholder_probe(&native_path).on_disk, 0);
    assert!(session
        .callback_failures()
        .iter()
        .any(|failure| failure.message.contains("MissingBytes")));
    eprintln!(
        "EXPORT MISSING REFUSAL: {failure}; diagnostics={:?}",
        session.callback_failures()
    );
    checkpoint.revalidate_namespace().unwrap();
    source.deny_reads.store(false, Ordering::SeqCst);
    let restored = checkpoint
        .with_reader(&path, |reader| {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
        .unwrap();
    assert_eq!(restored, pattern());
    checkpoint.revalidate_namespace().unwrap();
    assert_rename_capture_refused(&session, &path);

    // Same FileId/size, different catalog-entry blob: the full backing identity
    // must be retained, not reconstructed from the content ID alone.
    let other = placeholder_probe(&root.join("nested/large.bin"));
    assert_ne!(other.backing_identity, baseline.backing_identity);
    let reads = source.reads.load(Ordering::SeqCst);
    let hr = update_placeholder(&native_path, &other.backing_identity, None);
    assert!(hr >= 0, "test identity update HRESULT 0x{:08x}", hr as u32);
    let error = checkpoint.revalidate_namespace().unwrap_err();
    assert!(matches!(export_cause(&error), Error::IdentityMismatch));
    let mut called = false;
    let error = checkpoint
        .with_reader(&path, |_| {
            called = true;
            Ok(())
        })
        .unwrap_err();
    assert!(!called);
    assert!(matches!(export_cause(&error), Error::IdentityMismatch));
    assert_eq!(source.reads.load(Ordering::SeqCst), reads);
    assert!(update_placeholder(&native_path, &baseline.backing_identity, None) >= 0);
    checkpoint.revalidate_namespace().unwrap();
    drop(checkpoint);
    let mut foreign = baseline.backing_identity.clone();
    foreign[8] ^= 1;
    assert!(update_placeholder(&native_path, &foreign, None) >= 0);
    assert!(matches!(
        session.export_checkpoint(),
        Err(Error::IdentityMismatch)
    ));
    assert!(update_placeholder(&native_path, &baseline.backing_identity, None) >= 0);
    session
        .export_checkpoint()
        .unwrap()
        .revalidate_namespace()
        .unwrap();

    // A nonresident resized placeholder cannot use the immutable backing's
    // original length to invent new or truncated ranges.
    let resized = root.join("nested/large.bin");
    assert!(update_placeholder(&resized, &other.backing_identity, Some(other.size + 4096)) >= 0);
    let ordinary_inspection = session
        .inspect(&RepoPath::parse("nested/large.bin").unwrap())
        .unwrap();
    assert!(ordinary_inspection.placeholder.as_ref().unwrap().partial);
    drop(ordinary_inspection);
    let error = match session.export_checkpoint() {
        Err(error) => error,
        Ok(_) => panic!("partial resized export accepted"),
    };
    assert!(matches!(
        export_cause(&error),
        Error::Unsupported("partial placeholder size differs from immutable backing")
    ));
    assert_eq!(source.reads.load(Ordering::SeqCst), reads);
    assert!(update_placeholder(&resized, &other.backing_identity, Some(other.size)) >= 0);
    session
        .export_checkpoint()
        .unwrap()
        .revalidate_namespace()
        .unwrap();
    eprintln!(
        "EXPORT NEGATIVES: native HRESULT preserved; source restoration exact; native/backing identity retained; partial resized refused; roots/tickets retained"
    );
}

#[test]
#[ignore = "native mixed-state qualification; explicitly reports UNQUALIFIED if Windows fully hydrates the write"]
fn export_dirty_partial_qualification() {
    let (run, nonce) = disposable_run("export-dirty-partial");
    let root = run.join("root");
    std::fs::create_dir(&root).unwrap();
    let bytes: Vec<u8> = (0..16 * 1024 * 1024 + 37)
        .map(|i| ((i * 17 + i / 251) % 251) as u8)
        .collect();
    let file_id = FileId::from_bytes(&bytes);
    let source = Arc::new(Fixture {
        catalog: Catalog::new(
            1,
            vec![CatalogEntry {
                path: RepoPath::parse("mixed.bin").unwrap(),
                file_id: file_id.clone(),
                size: bytes.len() as u64,
            }],
        )
        .unwrap(),
        bytes: [(file_id.clone(), bytes.clone())].into_iter().collect(),
        reads: AtomicUsize::new(0),
        supplied: AtomicUsize::new(0),
        deny_reads: AtomicBool::new(true),
    });
    let mut session = Session::create_with_registration_hook(
        &root,
        nonce.to_le_bytes(),
        source.clone(),
        |ticket| {
            let mut f = std::fs::File::create(run.join("ticket"))?;
            f.write_all(&ticket.encode())?;
            f.sync_all()?;
            Ok(())
        },
    )
    .unwrap();
    // A full aligned page write need not read the original bytes. Refuse source
    // reads while attempting it so background prefetch cannot qualify the file
    // accidentally as a fully resident dirty file.
    child(&root, "mixed-page-write", "mixed.bin");
    let outcome = std::fs::read_to_string(run.join("mixed-write-result")).unwrap();
    let evidence = session
        .inspect(&RepoPath::parse("mixed.bin").unwrap())
        .unwrap();
    let metadata = evidence.placeholder.as_ref().unwrap();
    if outcome != "SUCCESS" || !metadata.partial || metadata.modified_bytes == 0 {
        eprintln!(
            "UNQUALIFIED mixed dirty/nonresident export: write={outcome}; metadata={metadata:?}; retained={}",
            run.display()
        );
        return;
    }
    assert!(metadata.on_disk_bytes < bytes.len() as u64);
    eprintln!("QUALIFYING REAL MIXED STATE: {metadata:?}");
    drop(evidence);
    session.disconnect().unwrap();
    std::fs::rename(root.join("mixed.bin"), root.join("relocated-mixed.bin")).unwrap();
    session.connect().unwrap();
    let path = RepoPath::parse("relocated-mixed.bin").unwrap();
    let before = placeholder_probe(&root.join(path.as_str()));
    let reads = source.reads.load(Ordering::SeqCst);
    let mut export = session.export_checkpoint().unwrap();
    export.revalidate_namespace().unwrap();
    assert_eq!(source.reads.load(Ordering::SeqCst), reads);
    let mut called = false;
    let error = export
        .with_reader(&path, |_| {
            called = true;
            Ok(())
        })
        .unwrap_err();
    assert!(!called);
    assert!(
        matches!(export_cause(&error), Error::Windows { operation: "CfHydratePlaceholder(export)", hresult } if *hresult < 0)
    );
    let after = placeholder_probe(&root.join(path.as_str()));
    assert_eq!(before.native_id, after.native_id);
    assert_eq!(before.backing_identity, after.backing_identity);
    assert_eq!(before.modified, after.modified);
    child(&root, "verify-mixed-page", path.as_str());
    export.revalidate_namespace().unwrap();
    source.deny_reads.store(false, Ordering::SeqCst);
    let restored = export
        .with_reader(&path, |reader| {
            let mut data = Vec::new();
            reader.read_to_end(&mut data)?;
            Ok(data)
        })
        .unwrap();
    let mut expected = bytes;
    expected[65536..65536 + 4096].fill(0xa5);
    assert_eq!(restored, expected);
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(run.join("exported-mixed"))
        .unwrap();
    export
        .with_reader(&path, |reader| {
            std::io::copy(reader, &mut output)?;
            Ok(())
        })
        .unwrap();
    output.sync_all().unwrap();
    drop(output);
    assert_eq!(std::fs::read(run.join("exported-mixed")).unwrap(), expected);
    export.revalidate_namespace().unwrap();
    assert_rename_capture_refused(&session, &path);
    eprintln!(
        "MIXED DIRTY/PARTIAL QUALIFIED: real missing-range refusal preserved private page; restored original ranges merged with private bytes; exact export hash={}",
        FileId::from_bytes(&expected)
    );
}

fn disposable_run(label: &str) -> (PathBuf, u128) {
    let parent = PathBuf::from(
        std::env::var_os("LAYRS_VFS_TEST_PARENT")
            .expect("set an explicit disposable parent directory"),
    );
    assert!(parent.is_absolute() && parent.is_dir());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let run = parent.join(format!("layrs-vfs-{label}-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&run).unwrap();
    eprintln!("RETAINED disposable fixture {}", run.display());
    (run, nonce)
}

#[test]
#[ignore = "requires explicit disposable LAYRS_VFS_TEST_PARENT; real CFAPI and native case guards"]
fn exact_case_ordinary_replacement() {
    let (run, nonce) = disposable_run("exact-case");
    let root = run.join("root");
    std::fs::create_dir(&root).unwrap();
    let base = b"immutable before replacement\n";
    let source = Arc::new(Fixture {
        catalog: Catalog::new(
            1,
            ["Dir/x", "keep.bin"]
                .into_iter()
                .map(|path| CatalogEntry {
                    path: RepoPath::parse(path).unwrap(),
                    file_id: FileId::from_bytes(base),
                    size: base.len() as u64,
                })
                .collect(),
        )
        .unwrap(),
        bytes: [(FileId::from_bytes(base), base.to_vec())]
            .into_iter()
            .collect(),
        reads: AtomicUsize::new(0),
        supplied: AtomicUsize::new(0),
        deny_reads: AtomicBool::new(false),
    });
    let session = Session::create_with_registration_hook(
        &root,
        nonce.to_le_bytes(),
        source.clone(),
        |ticket| {
            let mut output = std::fs::File::create(run.join("ticket"))?;
            output.write_all(&ticket.encode())?;
            output.sync_all()?;
            Ok(())
        },
    )
    .unwrap();
    // No descendant Inspection/Checkpoint is retained during replacement.
    std::fs::remove_file(root.join("Dir/x")).unwrap();
    std::fs::remove_dir(root.join("Dir")).unwrap();
    std::fs::create_dir(root.join("dir")).unwrap();
    let dirty = b"private replacement, never immutable source bytes\n";
    std::fs::write(root.join("dir/x"), dirty).unwrap();
    let old = RepoPath::parse("Dir/x").unwrap();
    let actual = RepoPath::parse("dir/x").unwrap();
    let file_id = native_file_id(&root.join("dir/x"));
    let directory_id = native_file_id(&root.join("dir"));
    assert!(matches!(
        session.inspect(&old).unwrap().evidence,
        CaptureEvidence::Deleted { .. }
    ));
    // A wrong leaf spelling absent from the catalog is NotFound, never an alias.
    assert!(
        matches!(session.inspect(&RepoPath::parse("dir/X").unwrap()), Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound)
    );
    {
        let entry = session.inspect(&actual).unwrap();
        assert!(
            matches!(entry.evidence, CaptureEvidence::NeedsHash { base: None, size } if size == dirty.len() as u64)
        );
        child(&root, "case-guard-refusals", "dir/x");
    }
    let mut checkpoint = session.checkpoint().unwrap();
    assert_eq!(checkpoint.entries().len(), 3);
    assert_eq!(
        checkpoint
            .entries()
            .iter()
            .filter(|entry| entry.path == actual)
            .count(),
        1
    );
    assert_eq!(
        checkpoint
            .entries()
            .iter()
            .filter(|entry| matches!(entry.evidence, CaptureEvidence::NeedsHash { .. }))
            .count(),
        1
    );
    assert!(matches!(
        &checkpoint
            .entries()
            .iter()
            .find(|entry| entry.path == old)
            .unwrap()
            .evidence,
        CaptureEvidence::Deleted { .. }
    ));
    child(&root, "case-guard-refusals", "dir/x");
    assert_eq!(native_file_id(&root.join("dir/x")), file_id);
    assert_eq!(native_file_id(&root.join("dir")), directory_id);
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    // Exercise both exact hashing and an external export through the same pin.
    let hashed = checkpoint
        .with_reader(&actual, |reader| {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes)?;
            assert_eq!(bytes, dirty);
            Ok(FileId::from_bytes(&bytes))
        })
        .unwrap();
    let export = run.join("exported-dirty-file");
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&export)
        .unwrap();
    checkpoint
        .with_reader(&actual, |reader| {
            std::io::copy(reader, &mut output)?;
            Ok(())
        })
        .unwrap();
    output.sync_all().unwrap();
    drop(output);
    assert_eq!(std::fs::read(&export).unwrap(), dirty);
    assert_eq!(hashed, FileId::from_bytes(dirty));
    checkpoint.revalidate_namespace().unwrap();
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    let clean = checkpoint
        .entries()
        .iter()
        .find(|entry| entry.path.as_str() == "keep.bin")
        .unwrap();
    assert_eq!(clean.placeholder.as_ref().unwrap().on_disk_bytes, 0);
    eprintln!(
        "CASE RECONCILIATION: Deleted Dir/x + one NeedsHash dir/x; exact dirty hash/export bytes; guards unchanged; source_reads0"
    );
}

#[test]
#[ignore = "diagnostic only: explicit disposable root; records actual offline placeholder rename behavior"]
fn disconnected_placeholder_rename_probe() {
    offline_rename_matrix(false);
}

#[test]
#[ignore = "requires explicit disposable root; qualifies actual renamed-file export, not capture"]
fn relocated_placeholder_export_matrix() {
    offline_rename_matrix(true);
}

fn offline_rename_matrix(export: bool) {
    let (run, nonce) = disposable_run("offline-rename");
    let mut case_index = 0u128;
    for lifecycle in ["disconnect", "process-exit"] {
        for state in ["partial", "full", "dirty"] {
            for kind in ["case-only", "distinct"] {
                case_index += 1;
                let case = run.join(format!("{lifecycle}-{state}-{kind}"));
                std::fs::create_dir(&case).unwrap();
                let root = case.join("root");
                std::fs::create_dir(&root).unwrap();
                let source = Arc::new(Fixture::new());
                let ticket_path = case.join("ticket");
                let mut session = Session::create_with_registration_hook(
                    &root,
                    (nonce + case_index).to_le_bytes(),
                    source.clone(),
                    |ticket| {
                        let mut file = std::fs::File::create(&ticket_path)?;
                        file.write_all(&ticket.encode())?;
                        file.sync_all()?;
                        Ok(())
                    },
                )
                .unwrap();
                let original = if state == "partial" {
                    "untouched.bin"
                } else {
                    "notes.txt"
                };
                let destination = if kind == "case-only" {
                    original.to_ascii_uppercase()
                } else {
                    format!("moved-{original}")
                };
                if state != "partial" {
                    child(&root, "full-notes", "");
                }
                if state == "dirty" {
                    child(&root, "write", "private notes!!\n");
                }
                let before = placeholder_probe(&root.join(original));
                let evidence = session
                    .inspect(&RepoPath::parse(original).unwrap())
                    .unwrap();
                eprintln!(
                    "OFFLINE RENAME baseline {lifecycle}/{state}/{kind}: {:?} {:?}",
                    evidence.evidence, evidence.placeholder
                );
                if state == "partial" {
                    assert!(matches!(
                        evidence.evidence,
                        CaptureEvidence::Immutable { .. }
                    ));
                    assert_eq!(before.on_disk, 0);
                } else {
                    assert!(matches!(
                        evidence.evidence,
                        CaptureEvidence::NeedsHash { .. }
                    ));
                }
                drop(evidence);
                let reads = source.reads.load(Ordering::SeqCst);
                session.disconnect().unwrap();
                let mut retained_session = Some(session);
                if lifecycle == "process-exit" {
                    drop(retained_session.take());
                    child(&root, "exit-before-rename", ticket_path.to_str().unwrap());
                }
                // Bound the native rename in a child, including any kernel wait.
                child(
                    &root,
                    "offline-rename",
                    &format!("{original}|{destination}"),
                );
                let outcome = std::fs::read_to_string(case.join("rename-result")).unwrap();
                eprintln!("OFFLINE RENAME {lifecycle}/{state}/{kind}: {outcome}");
                let renamed = outcome == "SUCCESS";
                let current = if renamed {
                    destination.as_str()
                } else {
                    original
                };
                let after = placeholder_probe(&root.join(current));
                assert_eq!(before.native_id, after.native_id);
                assert_eq!(before.backing_identity, after.backing_identity);
                assert_eq!(before.size, after.size);
                assert_eq!(
                    before.on_disk, after.on_disk,
                    "rename/probe hydrated content"
                );
                assert_eq!(before.modified, after.modified);
                assert!(std::fs::read_dir(&root)
                    .unwrap()
                    .any(|entry| entry.unwrap().file_name() == std::ffi::OsStr::new(current)));
                drop(retained_session);
                let session = Session::reconnect(
                    Registration::decode(&std::fs::read(&ticket_path).unwrap()).unwrap(),
                    source.clone(),
                )
                .unwrap();
                if renamed {
                    assert!(matches!(
                        session.inspect(&RepoPath::parse(current).unwrap()),
                        Err(Error::Unsupported(
                            "placeholder renamed: identity is preserved, capture reconciliation required"
                        ))
                    ));
                    assert!(matches!(
                        session.checkpoint(),
                        Err(Error::Unsupported(
                            "placeholder renamed: identity is preserved, capture reconciliation required"
                        ))
                    ));
                } else {
                    session.inspect(&RepoPath::parse(current).unwrap()).unwrap();
                }
                assert_eq!(
                    source.reads.load(Ordering::SeqCst),
                    reads,
                    "metadata reconciliation read immutable bytes"
                );
                if state != "partial" {
                    let expected: &[u8] = if state == "dirty" {
                        b"private notes!!\n"
                    } else {
                        b"immutable notes\n"
                    };
                    assert_eq!(std::fs::read(root.join(current)).unwrap(), expected);
                }
                assert_eq!(
                    source.reads.load(Ordering::SeqCst),
                    reads,
                    "resident-byte verification unexpectedly hydrated"
                );
                eprintln!(
                    "OFFLINE RENAME evidence {lifecycle}/{state}/{kind}: native={:?} identity={} on_disk={} modified={} metadata_source_delta=0; no export support claimed",
                    after.native_id,
                    FileId::from_bytes(&after.backing_identity),
                    after.on_disk,
                    after.modified
                );
                if export {
                    assert!(
                        renamed,
                        "export qualification requires an actual native rename"
                    );
                    let path = RepoPath::parse(current).unwrap();
                    let original_path = RepoPath::parse(original).unwrap();
                    let mut checkpoint = session.export_checkpoint().unwrap();
                    assert_eq!(source.reads.load(Ordering::SeqCst), reads);
                    let clean = RepoPath::parse("missing.bin").unwrap();
                    let clean_error = checkpoint
                        .with_reader(&clean, |_| -> Result<()> {
                            panic!("clean immutable export entry exposed a byte reader")
                        })
                        .unwrap_err();
                    assert!(matches!(
                        export_cause(&clean_error),
                        Error::Unsupported("only NeedsHash export entries may read content")
                    ));
                    assert_eq!(source.reads.load(Ordering::SeqCst), reads);
                    let actual = checkpoint
                        .entries()
                        .iter()
                        .find(|entry| entry.path == path)
                        .unwrap();
                    assert!(
                        matches!(&actual.evidence, CaptureEvidence::NeedsHash { base: Some(id), size } if id == &source.catalog().entry(&original_path).unwrap().file_id && *size == after.size)
                    );
                    assert!(matches!(
                        &checkpoint
                            .entries()
                            .iter()
                            .find(|entry| entry.path == original_path)
                            .unwrap()
                            .evidence,
                        CaptureEvidence::Deleted { .. }
                    ));
                    child(&root, "export-guard-refusals", current);
                    // Resident private bytes must not depend on source availability.
                    source
                        .deny_reads
                        .store(state != "partial", Ordering::SeqCst);
                    let expected = if state == "partial" {
                        pattern()
                    } else if state == "dirty" {
                        b"private notes!!\n".to_vec()
                    } else {
                        b"immutable notes\n".to_vec()
                    };
                    let observed = checkpoint
                        .with_reader(&path, |reader| {
                            let mut bytes = Vec::new();
                            reader.read_to_end(&mut bytes)?;
                            Ok(bytes)
                        })
                        .unwrap();
                    assert_eq!(observed, expected);
                    let short = checkpoint.with_reader(&path, |_| Ok(())).unwrap_err();
                    assert!(
                        matches!(short, Error::ExportConsumer { cause, .. } if matches!(*cause, Error::InvalidRange))
                    );
                    let disk_full = checkpoint
                        .with_reader(&path, |reader| -> Result<()> {
                            reader.read_exact(&mut [0u8; 1])?;
                            Err(std::io::Error::from_raw_os_error(
                                windows_sys::Win32::Foundation::ERROR_DISK_FULL as i32,
                            )
                            .into())
                        })
                        .unwrap_err();
                    assert!(
                        matches!(&disk_full, Error::ExportConsumer { path: actual, cause } if actual == &path && matches!(cause.as_ref(), Error::Io(error) if error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_DISK_FULL as i32)))
                    );
                    assert!(!disk_full.to_string().contains("backing"));
                    let mut output = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(case.join("exported-file"))
                        .unwrap();
                    checkpoint
                        .with_reader(&path, |reader| {
                            std::io::copy(reader, &mut output)?;
                            Ok(())
                        })
                        .unwrap();
                    output.sync_all().unwrap();
                    drop(output);
                    assert_eq!(std::fs::read(case.join("exported-file")).unwrap(), expected);
                    if state != "partial" {
                        assert_eq!(
                            source.reads.load(Ordering::SeqCst),
                            reads,
                            "resident export attempted source reads"
                        );
                    }
                    let after_reads = source.reads.load(Ordering::SeqCst);
                    checkpoint.revalidate_namespace().unwrap();
                    assert_eq!(source.reads.load(Ordering::SeqCst), after_reads);
                    assert!(matches!(
                        session.checkpoint(),
                        Err(Error::Unsupported(
                            "placeholder renamed: identity is preserved, capture reconciliation required"
                        ))
                    ));
                    assert!(matches!(
                        session.inspect(&path),
                        Err(Error::Unsupported(
                            "placeholder renamed: identity is preserved, capture reconciliation required"
                        ))
                    ));
                    let final_probe = placeholder_probe(&root.join(current));
                    assert_eq!(after.native_id, final_probe.native_id);
                    assert_eq!(after.backing_identity, final_probe.backing_identity);
                    eprintln!(
                        "EXPORT QUALIFIED {lifecycle}/{state}/{kind}: actual_path={current} hash={} bytes={} strict_capture_refused; resident_source_fault={}",
                        FileId::from_bytes(&expected),
                        expected.len(),
                        state != "partial"
                    );
                }
            }
        }
    }
}

fn pattern() -> Vec<u8> {
    (0..4 * 1024 * 1024 + 37)
        .map(|i| ((i * 17 + i / 251) % 251) as u8)
        .collect()
}
struct Fixture {
    catalog: Catalog,
    bytes: BTreeMap<FileId, Vec<u8>>,
    reads: AtomicUsize,
    supplied: AtomicUsize,
    deny_reads: AtomicBool,
}
impl Fixture {
    fn new() -> Self {
        let definitions = [
            ("nested/large.bin", pattern()),
            ("untouched.bin", pattern()),
            ("notes.txt", b"immutable notes\n".to_vec()),
            ("missing.bin", b"intentionally unavailable".to_vec()),
            ("zero.bin", vec![]),
        ];
        let entries = definitions
            .iter()
            .map(|(path, bytes)| CatalogEntry {
                path: RepoPath::parse(*path).unwrap(),
                file_id: FileId::from_bytes(bytes),
                size: bytes.len() as u64,
            })
            .collect();
        let bytes = definitions
            .into_iter()
            .filter(|(path, _)| *path != "missing.bin")
            .map(|(_, bytes)| (FileId::from_bytes(&bytes), bytes))
            .collect();
        Self {
            catalog: Catalog::new(1, entries).unwrap(),
            bytes,
            reads: AtomicUsize::new(0),
            supplied: AtomicUsize::new(0),
            deny_reads: AtomicBool::new(false),
        }
    }
}
impl SnapshotSource for Fixture {
    fn catalog(&self) -> &Catalog {
        &self.catalog
    }
    fn read_exact_at(
        &self,
        id: &FileId,
        offset: u64,
        output: &mut [u8],
        cancel: &Cancellation,
    ) -> Result<()> {
        cancel.check()?;
        assert!(
            output.len() <= layrs_vfs::READ_BUFFER_SIZE,
            "provider exceeded its bounded read buffer"
        );
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.deny_reads.load(Ordering::SeqCst) {
            return Err(Error::MissingBytes {
                file_id: id.clone(),
                offset,
                length: output.len(),
            });
        }
        let data = self.bytes.get(id).ok_or_else(|| Error::MissingBytes {
            file_id: id.clone(),
            offset,
            length: output.len(),
        })?;
        let range = layrs_vfs::checked_range(data.len() as u64, offset, output.len() as u64)?;
        output.copy_from_slice(&data[range.start as usize..range.end as usize]);
        self.supplied.fetch_add(output.len(), Ordering::SeqCst);
        Ok(())
    }
}

fn child(root: &Path, action: &str, value: &str) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "native_child_io", "--nocapture"])
        .env("LAYRS_VFS_CHILD_ROOT", root)
        .env("LAYRS_VFS_CHILD_ACTION", action)
        .env("LAYRS_VFS_CHILD_VALUE", value)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "child {action} failed: {status}");
            return;
        }
        if started.elapsed() > Duration::from_secs(75) {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("child {action} timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "helper spawned only by explicitly selected native CFAPI test"]
fn native_child_io() {
    let root = PathBuf::from(
        std::env::var_os("LAYRS_VFS_CHILD_ROOT").expect("native helper requires explicit root"),
    );
    let action = std::env::var("LAYRS_VFS_CHILD_ACTION").unwrap();
    let value = std::env::var("LAYRS_VFS_CHILD_VALUE").unwrap();
    match action.as_str() {
        "range" => {
            let mut file = std::fs::File::open(root.join("nested/large.bin")).unwrap();
            file.seek(SeekFrom::Start(1_048_587)).unwrap();
            let mut bytes = [0u8; 71];
            file.read_exact(&mut bytes).unwrap();
            assert_eq!(&bytes, &pattern()[1_048_587..1_048_658]);
        }
        "full-notes" => assert_eq!(
            std::fs::read(root.join("notes.txt")).unwrap(),
            b"immutable notes\n"
        ),
        "write" => {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(root.join("notes.txt"))
                .unwrap();
            file.write_all(value.as_bytes()).unwrap();
            file.sync_all().unwrap();
        }
        "verify-write" => assert_eq!(
            std::fs::read(root.join("notes.txt")).unwrap(),
            value.as_bytes()
        ),
        "read-error" => {
            let error = std::fs::read(root.join(value)).unwrap_err();
            assert!(error.raw_os_error().is_some());
            eprintln!("expected native read error: {error:?}");
        }
        "delete" => std::fs::remove_file(root.join(value)).unwrap(),
        "rename-refused" => {
            let error =
                std::fs::rename(root.join("untouched.bin"), root.join("renamed.bin")).unwrap_err();
            eprintln!("expected rename refusal: {error:?}");
        }
        "case-guard-refusals" => {
            let file = root.join(&value);
            let directory = file.parent().unwrap();
            for (from, to) in [
                (file.clone(), directory.join("X")),
                (directory.to_path_buf(), root.join("DIR")),
            ] {
                let before = native_file_id(&from);
                let error = std::fs::rename(&from, &to).unwrap_err();
                assert_eq!(error.raw_os_error(), Some(32), "{from:?} -> {to:?}");
                assert_eq!(native_file_id(&from), before);
                assert!(std::fs::read_dir(from.parent().unwrap())
                    .unwrap()
                    .any(|entry| entry.unwrap().file_name() == from.file_name().unwrap()));
            }
            eprintln!(
                "CASE GUARDS: leaf and parent case-only rename refused32; exact names/native IDs unchanged"
            );
        }
        "guard-refusals" => {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::{
                Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE},
                Storage::FileSystem::*,
            };
            let file = root.join(&value);
            let parent = file.parent().unwrap();
            for (path, access) in [
                (&file as &Path, FILE_WRITE_DATA),
                (&file, DELETE),
                (parent, DELETE),
                (root.as_path(), DELETE),
            ] {
                let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
                let handle = unsafe {
                    CreateFileW(
                        wide.as_ptr(),
                        access,
                        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                        std::ptr::null(),
                        OPEN_EXISTING,
                        FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                        std::ptr::null_mut(),
                    )
                };
                let error = unsafe { GetLastError() };
                if handle != INVALID_HANDLE_VALUE {
                    unsafe {
                        CloseHandle(handle);
                    }
                }
                assert_eq!(
                    handle,
                    INVALID_HANDLE_VALUE,
                    "guard allowed access {access:#x} at {}",
                    path.display()
                );
                assert_eq!(
                    error,
                    32,
                    "expected native sharing violation at {}",
                    path.display()
                );
            }
            let error = std::fs::rename(parent, root.join("moved-directory")).unwrap_err();
            assert_eq!(
                error.raw_os_error(),
                Some(32),
                "directory guard did not prevent rename"
            );
            eprintln!(
                "NATIVE GUARDS: file WRITE_DATA/DELETE and parent/root DELETE/rename refused with32"
            );
        }
        "control-refusals" => {
            assert_eq!(
                std::fs::write(root.join(".layrs"), b"replacement")
                    .unwrap_err()
                    .raw_os_error(),
                Some(32)
            );
            assert_eq!(
                std::fs::remove_file(root.join(".layrs"))
                    .unwrap_err()
                    .raw_os_error(),
                Some(32)
            );
        }
        "checkpoint-refusals" => {
            assert_eq!(
                std::fs::write(root.join("notes.txt"), b"lost write")
                    .unwrap_err()
                    .raw_os_error(),
                Some(32)
            );
            assert_eq!(
                std::fs::remove_file(root.join("notes.txt"))
                    .unwrap_err()
                    .raw_os_error(),
                Some(32)
            );
        }
        "export-guard-refusals" => {
            let file = root.join(&value);
            let identity = native_file_id(&file);
            for result in [
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&file)
                    .map(|_| ()),
                std::fs::remove_file(&file),
                std::fs::rename(&file, root.join("guard-must-not-move")),
            ] {
                assert_eq!(result.unwrap_err().raw_os_error(), Some(32));
            }
            assert_eq!(native_file_id(&file), identity);
        }
        "mixed-page-write" => {
            let result = (|| -> std::io::Result<()> {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(root.join(&value))?;
                file.seek(SeekFrom::Start(65536))?;
                file.write_all(&[0xa5; 4096])?;
                file.sync_all()
            })();
            let outcome = match result {
                Ok(()) => "SUCCESS".to_owned(),
                Err(error) => format!("ERROR raw={:?}: {error}", error.raw_os_error()),
            };
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(root.parent().unwrap().join("mixed-write-result"))
                .unwrap();
            output.write_all(outcome.as_bytes()).unwrap();
            output.sync_all().unwrap();
            eprintln!("mixed aligned native write: {outcome}");
        }
        "seed-hydrate" | "seed-write" | "seed-read" => {
            seeded_partial::child_action(&root, &action, &value);
        }
        "verify-mixed-page" => {
            let mut file = std::fs::File::open(root.join(&value)).unwrap();
            file.seek(SeekFrom::Start(65536)).unwrap();
            let mut bytes = [0u8; 4096];
            file.read_exact(&mut bytes).unwrap();
            assert_eq!(bytes, [0xa5; 4096], "private resident page was lost");
        }
        "restart-service" => {
            let ticket = Registration::decode(&std::fs::read(value).unwrap()).unwrap();
            let source = Arc::new(Fixture::new());
            let session = Session::reconnect(ticket, source.clone()).unwrap();
            child(&root, "verify-write", "private A\n");
            session
                .hydrate_for_hash(&RepoPath::parse("untouched.bin").unwrap())
                .unwrap();
            assert_eq!(
                std::fs::read(root.join("untouched.bin")).unwrap(),
                pattern()
            );
            assert!(source.supplied.load(Ordering::SeqCst) > 0);
            eprintln!(
                "fresh provider process reconnected and hydrated; exiting without Session::drop"
            );
            std::mem::forget(session);
            std::process::exit(0);
        }
        "exit-before-rename" => {
            let ticket = Registration::decode(&std::fs::read(value).unwrap()).unwrap();
            let source = Arc::new(Fixture::new());
            let session = Session::reconnect(ticket, source.clone()).unwrap();
            assert_eq!(source.reads.load(Ordering::SeqCst), 0);
            eprintln!(
                "RENAME PROBE: exiting connected provider without destructors; source_reads0"
            );
            std::mem::forget(session);
            std::process::exit(0);
        }
        "offline-rename" => {
            let (original, destination) = value.split_once('|').unwrap();
            let outcome = match std::fs::rename(root.join(original), root.join(destination)) {
                Ok(()) => "SUCCESS".to_owned(),
                Err(error) => format!("ERROR raw={:?}: {error}", error.raw_os_error()),
            };
            eprintln!("native offline rename: {outcome}");
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(root.parent().unwrap().join("rename-result"))
                .unwrap();
            output.write_all(outcome.as_bytes()).unwrap();
            output.sync_all().unwrap();
        }
        "publish-crash" => {
            let directory = root.parent().unwrap();
            let ticket =
                Registration::decode(&std::fs::read(directory.join("ticket")).unwrap()).unwrap();
            let mut session = Session::reconnect(ticket, Arc::new(Fixture::new())).unwrap();
            let crash = PublicationCrash(value.clone());
            session
                .publish_control_file_recoverable(
                    CONTROL_TEST_BYTES,
                    &directory.join("staging"),
                    None,
                    |identity| {
                        persist_control_intent(directory, identity)?;
                        crash.exit_at("intent-persisted");
                        Ok(())
                    },
                    &crash,
                )
                .unwrap();
            panic!("publication did not hit requested crash point {value}");
        }
        _ => panic!("unknown child action"),
    }
}

#[test]
#[ignore = "requires explicit disposable LAYRS_VFS_TEST_PARENT; performs real CFAPI registration"]
fn two_roots_real_cfapi() {
    eprintln!("capability: {:?}", layrs_vfs::capability().unwrap());
    let parent = PathBuf::from(
        std::env::var_os("LAYRS_VFS_TEST_PARENT")
            .expect("set an explicit disposable parent directory"),
    );
    assert!(parent.is_absolute() && parent.is_dir());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let run = parent.join(format!("layrs-vfs-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&run).unwrap();
    let a = run.join("a");
    let b = run.join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    eprintln!("RETAINED disposable roots: {}", run.display());
    let source = Arc::new(Fixture::new());
    let mut hook_ticket = None;
    let mut first =
        Session::create_with_registration_hook(&a, nonce.to_le_bytes(), source.clone(), |ticket| {
            assert!(
                ticket.verify(source.catalog()).is_err(),
                "registration happened before durable hook"
            );
            assert!(ticket.verify_root_identity().is_err());
            assert!(
                std::fs::read_dir(&a)?.next().is_none(),
                "population happened before durable hook"
            );
            let mut file = std::fs::File::create(run.join("prepared.ticket"))?;
            file.write_all(&ticket.encode())?;
            file.sync_all()?;
            hook_ticket = Some(ticket.encode());
            Ok(())
        })
        .unwrap_or_else(|error| {
            panic!(
                "REAL registration A failed: {error}; retained {}",
                a.display()
            )
        });
    let mut second = Session::create(&b, (nonce + 1).to_le_bytes(), source.clone()).unwrap();
    eprintln!(
        "POLICY: PROGRESSIVE hydration verified on connect; ALWAYS_FULL population; self implicit hydration blocked"
    );
    let ticket_a = first.registration().encode();
    assert_eq!(hook_ticket.as_ref(), Some(&ticket_a));
    first.registration().verify(source.catalog()).unwrap();
    first.registration().verify_root_identity().unwrap();
    assert!(matches!(
        first
            .registration()
            .verify(&Catalog::new(2, Vec::new()).unwrap()),
        Err(Error::IdentityMismatch)
    ));
    // A root-only proof still binds every saved native/catalog identity field.
    for offset in [8, 31, 95, 99] {
        let mut changed = first.registration().encode();
        changed[offset] = if offset == 31 {
            if changed[offset] == b'0' {
                b'1'
            } else {
                b'0'
            }
        } else {
            changed[offset] ^ 1
        };
        assert!(matches!(
            Registration::decode(&changed)
                .unwrap()
                .verify_root_identity(),
            Err(Error::ForeignRoot)
        ));
    }
    let ticket_b = second.registration().encode();
    std::fs::write(run.join("a.ticket"), &ticket_a).unwrap();
    std::fs::write(run.join("b.ticket"), &ticket_b).unwrap();
    let control_bytes = b"provider native test locator";
    let staging = run.join("control-staging");
    std::fs::create_dir(&staging).unwrap();
    let control = first
        .publish_control_file_recoverable(
            control_bytes,
            &staging,
            None,
            |identity| persist_control_intent(&run, identity),
            &NoControlPublishFaults,
        )
        .unwrap();
    let (observed_bytes, observed_identity) = layrs_vfs::read_control_file(&a).unwrap();
    assert_eq!(observed_bytes, control_bytes);
    assert_eq!(observed_identity, control);
    child(&a, "control-refusals", "");
    let large = RepoPath::parse("nested/large.bin").unwrap();
    for session in [&first, &second] {
        let inspection = session.inspect(&large).unwrap();
        assert!(matches!(
            inspection.evidence,
            CaptureEvidence::Immutable { .. }
        ));
        assert_eq!(inspection.placeholder.unwrap().on_disk_bytes, 0);
        assert_eq!(session.namespace().unwrap().len(), 5);
    }
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        0,
        "metadata operations hydrated content"
    );
    {
        let mut checkpoint = first.checkpoint().unwrap();
        assert_eq!(checkpoint.entries().len(), 5);
        assert!(
            checkpoint.with_reader(&large, |_| Ok(())).is_err(),
            "clean immutable bytes were exposed to implicit reading"
        );
        checkpoint.revalidate_namespace().unwrap();
        assert_eq!(
            source.reads.load(Ordering::SeqCst),
            0,
            "checkpoint hydrated clean data"
        );
        let guard = first.inspect(&large).unwrap();
        child(&a, "guard-refusals", large.as_str());
        assert_eq!(
            source.reads.load(Ordering::SeqCst),
            0,
            "effective guard hydrated content"
        );
        assert_eq!(guard.placeholder.as_ref().unwrap().on_disk_bytes, 0);
        assert_eq!(
            first
                .inspect(&large)
                .unwrap()
                .placeholder
                .unwrap()
                .on_disk_bytes,
            0
        );
    }
    // A capture/status consumer in the provider process must opt in explicitly
    // before reading content. This read must not trigger a FETCH_DATA callback.
    let error = std::fs::read(a.join("untouched.bin")).unwrap_err();
    eprintln!("expected provider implicit-read refusal: {error:?}");
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        0,
        "provider implicit read hydrated a clean file"
    );
    assert_eq!(
        first
            .inspect(&RepoPath::parse("untouched.bin").unwrap())
            .unwrap()
            .placeholder
            .unwrap()
            .on_disk_bytes,
        0
    );
    child(&a, "range", "");
    child(&b, "range", "");
    assert!(source.supplied.load(Ordering::SeqCst) > 0);
    let traces = first
        .hydration_requests()
        .into_iter()
        .chain(second.hydration_requests())
        .collect::<Vec<_>>();
    assert!(!traces.is_empty());
    let mut kernel_required = 0u64;
    let mut supplied = 0u64;
    for trace in &traces {
        eprintln!("HYDRATION REQUEST: {trace:?}");
        assert_eq!(trace.completion_status, 0);
        let ranges = layrs_vfs::HydrationRange::new(
            trace.file_size as u64,
            trace.required_offset,
            trace.required_length,
        )
        .unwrap()
        .collect::<Vec<_>>();
        assert_eq!(trace.transfer_start, ranges.first().unwrap().start);
        assert_eq!(trace.transfer_end, ranges.last().unwrap().end);
        assert_eq!(
            trace.source_bytes_read,
            trace.transfer_end - trace.transfer_start,
            "source overread beyond aligned required range"
        );
        assert_eq!(
            trace.transferred_bytes, trace.source_bytes_read,
            "source/transfer accounting diverged"
        );
        supplied += trace.source_bytes_read;
        kernel_required += if trace.required_length == -1 {
            (trace.file_size - trace.required_offset) as u64
        } else {
            trace.required_length as u64
        };
    }
    assert_eq!(supplied, source.supplied.load(Ordering::SeqCst) as u64);
    let application_bytes = 2 * 71;
    eprintln!(
        "HYDRATION AMPLIFICATION: app_bytes={application_bytes}, kernel_required_bytes={kernel_required}, provider_source_bytes={supplied}; page-range residency savings are NOT qualified"
    );
    for session in [&first, &second] {
        eprintln!(
            "POST-READ RESIDENCY: {:?}",
            session.inspect(&large).unwrap().placeholder
        );
        let before = source.supplied.load(Ordering::SeqCst);
        let inspection = session
            .inspect(&RepoPath::parse("untouched.bin").unwrap())
            .unwrap();
        assert!(matches!(
            inspection.evidence,
            CaptureEvidence::Immutable { .. }
        ));
        assert_eq!(inspection.placeholder.unwrap().on_disk_bytes, 0);
        assert_eq!(
            source.supplied.load(Ordering::SeqCst),
            before,
            "clean inspection hydrated a different file"
        );
    }
    child(&a, "full-notes", "");
    child(&b, "full-notes", "");
    child(&a, "write", "private A\n");
    child(&b, "write", "private B\n");
    let notes = RepoPath::parse("notes.txt").unwrap();
    for session in [&first, &second] {
        assert!(matches!(
            session.inspect(&notes).unwrap().evidence,
            CaptureEvidence::NeedsHash { .. }
        ));
    }
    assert_eq!(
        source
            .bytes
            .get(&FileId::from_bytes(b"immutable notes\n"))
            .unwrap(),
        b"immutable notes\n"
    );
    {
        let mut checkpoint = first.checkpoint().unwrap();
        child(&a, "checkpoint-refusals", "");
        checkpoint
            .with_reader(&notes, |reader| {
                let mut bytes = Vec::new();
                reader.read_to_end(&mut bytes)?;
                assert_eq!(bytes, b"private A\n");
                Ok(())
            })
            .unwrap();
        checkpoint.revalidate_namespace().unwrap();
        // Directory identity guards are not an atomic child-name freeze.
        match std::fs::write(a.join("late-native-file"), b"new file") {
            Ok(()) => assert!(matches!(
                checkpoint.revalidate_namespace(),
                Err(Error::Busy)
            )),
            Err(error) => assert_eq!(error.raw_os_error(), Some(32)),
        }
    }
    child(&a, "rename-refused", "");
    child(&a, "read-error", "missing.bin");
    assert!(!first.callback_failures().is_empty());
    first.disconnect().unwrap();
    second.disconnect().unwrap();
    child(&a, "read-error", "untouched.bin");
    child(&a, "verify-write", "private A\n");
    child(&b, "verify-write", "private B\n");
    drop(first);
    drop(second);
    child(
        &a,
        "restart-service",
        run.join("a.ticket").to_str().unwrap(),
    );
    let mut first =
        Session::reconnect(Registration::decode(&ticket_a).unwrap(), source.clone()).unwrap();
    first.attach_control_file(&control).unwrap();
    let mut second =
        Session::reconnect(Registration::decode(&ticket_b).unwrap(), source.clone()).unwrap();
    child(&a, "verify-write", "private A\n");
    child(&b, "verify-write", "private B\n");
    first
        .hydrate_for_hash(&RepoPath::parse("untouched.bin").unwrap())
        .unwrap();
    assert_eq!(std::fs::read(a.join("untouched.bin")).unwrap(), pattern());
    child(&a, "delete", "zero.bin");
    assert!(matches!(
        first
            .inspect(&RepoPath::parse("zero.bin").unwrap())
            .unwrap()
            .evidence,
        CaptureEvidence::Deleted { .. }
    ));
    assert!(!matches!(
        second
            .inspect(&RepoPath::parse("zero.bin").unwrap())
            .unwrap()
            .evidence,
        CaptureEvidence::Deleted { .. }
    ));
    let mut foreign = ticket_a.clone();
    foreign[8] ^= 1;
    first.disconnect().unwrap();
    second.disconnect().unwrap();
    assert!(matches!(
        Session::reconnect(Registration::decode(&foreign).unwrap(), source),
        Err(Error::ForeignRoot)
    ));
    let broken = run.join("incomplete");
    std::fs::create_dir(&broken).unwrap();
    let repair_source = Arc::new(Fixture::new());
    let mut repair_ticket = None;
    let failed = Session::create_with_registration_hook(
        &broken,
        (nonce + 2).to_le_bytes(),
        repair_source.clone(),
        |ticket| {
            repair_ticket = Some(ticket.clone());
            std::fs::write(broken.join("notes.txt"), b"preserved collision")?;
            Ok(())
        },
    );
    assert!(failed.is_err());
    let repair_ticket = repair_ticket.unwrap();
    assert!(matches!(
        repair_ticket.verify(repair_source.catalog()),
        Err(Error::NeedsRepair(_))
    ));
    assert!(matches!(
        repair_ticket.verify_root_identity(),
        Err(Error::NeedsRepair(_))
    ));
    assert!(matches!(
        Session::reconnect(repair_ticket, repair_source),
        Err(Error::NeedsRepair(_))
    ));
    assert_eq!(
        std::fs::read(broken.join("notes.txt")).unwrap(),
        b"preserved collision"
    );
    eprintln!(
        "RECOVERY/CONTROL/CHECKPOINT: durable hook, readonly verify, bound .layrs, retained byte reader, namespace revalidation, incomplete NeedsRepair passed"
    );
    eprintln!(
        "PASS: real lazy placeholders, exact kernel-requested hydration, two divergent native writes, missing-byte failure, provider process restart, identity guard, namespace deletion. Page-range residency savings NOT qualified. RETAINED {}",
        run.display()
    );
}

const CONTROL_TEST_BYTES: &[u8] = b"durable exact locator bytes for interrupted publication\n";

fn persist_control_intent(directory: &Path, identity: &ControlFileIdentity) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(directory.join("control.intent"))?;
    file.write_all(&identity.encode())?;
    file.sync_all()?;
    Ok(())
}

struct PublicationCrash(String);
impl PublicationCrash {
    fn exit_at(&self, point: &str) {
        if self.0 == point {
            eprintln!("CONTROL CRASH: {point}; exiting without provider/handle destructors");
            std::process::exit(0);
        }
    }
}
impl ControlPublishFaults for PublicationCrash {
    fn after_stage_created(&self) -> std::io::Result<()> {
        self.exit_at("stage-created");
        Ok(())
    }
    fn write_staged(&self, output: &mut dyn Write, bytes: &[u8]) -> std::io::Result<()> {
        if self.0 == "during-write" {
            output.write_all(&bytes[..bytes.len() / 2])?;
            self.exit_at("during-write");
        }
        output.write_all(bytes)
    }
    fn after_stage_flushed(&self) -> std::io::Result<()> {
        self.exit_at("stage-flushed");
        Ok(())
    }
    fn after_published(&self) -> std::io::Result<()> {
        self.exit_at("published");
        Ok(())
    }
}

fn control_case(
    run: &Path,
    label: &str,
    id: u128,
) -> (PathBuf, PathBuf, Arc<Fixture>, Registration) {
    let directory = run.join(label);
    let root = directory.join("root");
    std::fs::create_dir(&directory).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(directory.join("staging")).unwrap();
    let source = Arc::new(Fixture::new());
    let session = Session::create(&root, id.to_le_bytes(), source.clone()).unwrap();
    let ticket = session.registration().clone();
    let mut file = std::fs::File::create(directory.join("ticket")).unwrap();
    file.write_all(&ticket.encode()).unwrap();
    file.sync_all().unwrap();
    drop(session);
    (directory, root, source, ticket)
}

#[test]
#[ignore = "requires explicit disposable LAYRS_VFS_TEST_PARENT; real CFAPI and process exits"]
fn control_publication_recovers_interrupted_writes() {
    let parent = PathBuf::from(std::env::var_os("LAYRS_VFS_TEST_PARENT").unwrap());
    assert!(parent.is_absolute() && parent.is_dir());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let run = parent.join(format!("layrs-control-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&run).unwrap();
    eprintln!("RETAINED control publication fixtures: {}", run.display());
    for (index, point) in [
        "stage-created",
        "during-write",
        "stage-flushed",
        "intent-persisted",
        "published",
    ]
    .into_iter()
    .enumerate()
    {
        let (directory, root, source, ticket) = control_case(&run, point, nonce + index as u128);
        child(&root, "publish-crash", point);
        let staged = std::fs::read_dir(directory.join("staging"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        let expected = std::fs::read(directory.join("control.intent"))
            .ok()
            .map(|bytes| ControlFileIdentity::decode(&bytes).unwrap());
        assert_eq!(
            expected.is_some(),
            matches!(point, "intent-persisted" | "published")
        );
        if point == "published" {
            assert!(staged.is_empty());
            let (bytes, identity) = layrs_vfs::read_control_file(&root).unwrap();
            assert_eq!(bytes, CONTROL_TEST_BYTES);
            assert_eq!(Some(identity), expected);
        } else {
            assert!(
                !root.join(".layrs").exists(),
                "incomplete bytes became final"
            );
            assert_eq!(staged.len(), 1);
            assert_eq!(
                std::fs::metadata(&staged[0]).unwrap().len(),
                match point {
                    "stage-created" => 0,
                    "during-write" => (CONTROL_TEST_BYTES.len() / 2) as u64,
                    _ => CONTROL_TEST_BYTES.len() as u64,
                }
            );
        }
        let before = staged
            .iter()
            .map(|path| (path.clone(), std::fs::read(path).unwrap()))
            .collect::<Vec<_>>();
        let mut session = Session::reconnect(ticket, source.clone()).unwrap();
        let identity = session
            .publish_control_file_recoverable(
                CONTROL_TEST_BYTES,
                &directory.join("staging"),
                expected.as_ref(),
                |identity| persist_control_intent(&directory, identity),
                &NoControlPublishFaults,
            )
            .unwrap();
        if let Some(expected) = expected {
            assert_eq!(identity, expected, "resume substituted native identity");
        } else {
            for (path, bytes) in before {
                assert_eq!(
                    std::fs::read(path).unwrap(),
                    bytes,
                    "unbound stage was modified"
                );
            }
        }
        let (bytes, observed) = layrs_vfs::read_control_file(&root).unwrap();
        assert_eq!(bytes, CONTROL_TEST_BYTES);
        assert_eq!(observed, identity);
        assert_eq!(session.namespace().unwrap().len(), 5);
        assert_eq!(session.checkpoint().unwrap().entries().len(), 5);
        assert_eq!(source.reads.load(Ordering::SeqCst), 0);
        eprintln!("CONTROL RECOVERY: {point} recovered exact identity/bytes; source_reads=0");
    }

    // Unlike the process-exit cases above, propagate a persistence-hook error
    // through normal Drop. Exercise both a private directory and a staging leaf
    // beyond MAX_PATH, as in the complete-Store journal layout.
    let (directory, root, source, ticket) = control_case(&run, "long-hook-error", nonce + 7);
    let staging = directory
        .join("staging")
        .join(format!("private-journal-{}", "x".repeat(180)));
    std::fs::create_dir(&staging).unwrap();
    let mut session = Session::reconnect(ticket.clone(), source.clone()).unwrap();
    let error = session
        .publish_control_file_recoverable(
            CONTROL_TEST_BYTES,
            &staging,
            None,
            |identity| {
                persist_control_intent(&directory, identity)?;
                Err(std::io::Error::other("injected error after durable control intent").into())
            },
            &NoControlPublishFaults,
        )
        .unwrap_err();
    assert!(matches!(error, Error::Io(_)));
    drop(session);
    let staged = std::fs::read_dir(&staging)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(staged.len(), 1);
    use std::os::windows::ffi::OsStrExt;
    let stage_length = staged[0].as_os_str().encode_wide().count();
    assert!(stage_length > 260);
    assert_eq!(std::fs::read(&staged[0]).unwrap(), CONTROL_TEST_BYTES);
    assert!(!root.join(".layrs").exists());
    let intent =
        ControlFileIdentity::decode(&std::fs::read(directory.join("control.intent")).unwrap())
            .unwrap();
    let mut session = Session::reconnect(ticket, source.clone()).unwrap();
    let recovered = session
        .publish_control_file_recoverable(
            CONTROL_TEST_BYTES,
            &staging,
            Some(&intent),
            |_| panic!("durable identity was replaced"),
            &NoControlPublishFaults,
        )
        .unwrap();
    assert_eq!(recovered, intent);
    assert_eq!(
        layrs_vfs::read_control_file(&root).unwrap(),
        (CONTROL_TEST_BYTES.to_vec(), intent)
    );
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    eprintln!(
        "CONTROL LONG PATH: {stage_length} UTF-16 characters; hook-error resume preserved native identity; source_reads=0"
    );

    struct ShortSuccess;
    impl ControlPublishFaults for ShortSuccess {
        fn write_staged(&self, output: &mut dyn Write, bytes: &[u8]) -> std::io::Result<()> {
            output.write_all(&bytes[..bytes.len() / 2])
        }
    }
    let (directory, root, source, ticket) = control_case(&run, "short-success", nonce + 10);
    let mut session = Session::reconnect(ticket, source.clone()).unwrap();
    assert!(matches!(
        session.publish_control_file_recoverable(
            CONTROL_TEST_BYTES,
            &directory.join("staging"),
            None,
            |_| panic!("short bytes reached durable intent"),
            &ShortSuccess,
        ),
        Err(Error::IdentityMismatch)
    ));
    assert!(!root.join(".layrs").exists());
    session
        .publish_control_file_recoverable(
            CONTROL_TEST_BYTES,
            &directory.join("staging"),
            None,
            |identity| persist_control_intent(&directory, identity),
            &NoControlPublishFaults,
        )
        .unwrap();

    for (index, bytes) in [b"".as_slice(), CONTROL_TEST_BYTES].into_iter().enumerate() {
        let (directory, root, source, ticket) = control_case(
            &run,
            &format!("foreign-final-{index}"),
            nonce + 20 + index as u128,
        );
        std::fs::write(root.join(".layrs"), bytes).unwrap();
        let mut session = Session::reconnect(ticket, source).unwrap();
        assert!(matches!(
            session.publish_control_file_recoverable(
                CONTROL_TEST_BYTES,
                &directory.join("staging"),
                None,
                |_| panic!("foreign final reached durable intent"),
                &NoControlPublishFaults,
            ),
            Err(Error::ForeignRoot)
        ));
        assert_eq!(std::fs::read(root.join(".layrs")).unwrap(), bytes);
        assert!(
            session.namespace().is_err(),
            "unattached foreign control was excluded"
        );
        assert!(
            session.checkpoint().is_err(),
            "foreign control entered a checkpoint"
        );
        assert_eq!(
            std::fs::read_dir(directory.join("staging"))
                .unwrap()
                .count(),
            0
        );
    }

    let (directory, root, source, ticket) = control_case(&run, "late-collision", nonce + 30);
    let mut session = Session::reconnect(ticket, source).unwrap();
    let mut intent = None;
    assert!(session
        .publish_control_file_recoverable(
            CONTROL_TEST_BYTES,
            &directory.join("staging"),
            None,
            |identity| {
                persist_control_intent(&directory, identity)?;
                intent = Some(identity.clone());
                std::fs::write(root.join(".layrs"), CONTROL_TEST_BYTES)?;
                Ok(())
            },
            &NoControlPublishFaults,
        )
        .is_err());
    assert_eq!(
        std::fs::read(root.join(".layrs")).unwrap(),
        CONTROL_TEST_BYTES
    );
    assert!(matches!(
        session.publish_control_file_recoverable(
            CONTROL_TEST_BYTES,
            &directory.join("staging"),
            intent.as_ref(),
            |_| panic!("existing intent was repersisted"),
            &NoControlPublishFaults,
        ),
        Err(Error::IdentityMismatch)
    ));
    assert_eq!(
        std::fs::read_dir(directory.join("staging"))
            .unwrap()
            .count(),
        1
    );
    eprintln!(
        "CONTROL PASS: five process-interruption points, short-success rejection, empty/same-byte/late foreign collisions preserved; fixtures retained"
    );
}
