//! Deliberately break caller quiescence in fresh owned fixtures. Refusal is the
//! expected result; these mutation helpers are never production repair paths.
use super::*;
use windows_sys::Win32::{
    Foundation::*,
    System::{IO::OVERLAPPED, LibraryLoader::*},
};

fn change_placeholder(path: &Path, identity: Option<&[u8]>, clear_sync: bool) {
    update_placeholder(path, None, identity, clear_sync);
}

pub(super) fn update_placeholder(
    path: &Path,
    metadata: Option<&CF_FS_METADATA>,
    identity: Option<&[u8]>,
    clear_sync: bool,
) {
    type Update = unsafe extern "system" fn(
        HANDLE,
        *const CF_FS_METADATA,
        *const std::ffi::c_void,
        u32,
        *const CF_FILE_RANGE,
        u32,
        CF_UPDATE_FLAGS,
        *mut i64,
        *mut OVERLAPPED,
    ) -> i32;
    let name: Vec<u16> = "cldapi.dll\0".encode_utf16().collect();
    let module = unsafe {
        LoadLibraryExW(
            name.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    assert!(!module.is_null());
    struct Module(HMODULE);
    impl Drop for Module {
        fn drop(&mut self) {
            unsafe { FreeLibrary(self.0) };
        }
    }
    let _module = Module(module);
    let pointer = unsafe { GetProcAddress(module, b"CfUpdatePlaceholder\0".as_ptr()) }.unwrap();
    let update: Update = unsafe { std::mem::transmute(pointer) };
    let file = fs::open(
        path,
        io::ALL_SHARING,
        FILE_WRITE_DATA | FILE_READ_ATTRIBUTES,
    )
    .unwrap();
    let hr = unsafe {
        update(
            fs::handle(&file),
            metadata.map_or(std::ptr::null(), |metadata| metadata),
            identity.map_or(std::ptr::null(), |v| v.as_ptr().cast()),
            identity.map_or(0, |v| v.len() as u32),
            std::ptr::null(),
            0,
            if clear_sync {
                CF_UPDATE_FLAG_CLEAR_IN_SYNC
            } else {
                CF_UPDATE_FLAG_MARK_IN_SYNC
            },
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    eprintln!(
        "TEST mutation CfUpdatePlaceholder identity={} clear_sync={clear_sync} size={:?}: HRESULT0x{:08X}",
        identity.is_some(),
        metadata.map(|value| value.FileSize),
        hr as u32
    );
    assert!(
        hr >= 0,
        "mutation did not execute; negative arm unqualified, retained fixture"
    );
}

#[test]
#[ignore = "actual initial population faults retain INCOMPLETE; explicit disposable parent; no repair"]
fn stage2_population_and_namespace_refusals() {
    let run = fixture::run("negative");
    let (f, control) = fixture::Fixture::create(&run, "persist-unknown");
    assert!(matches!(
        ProviderRootGuard::create_with_control_directory(
            &control,
            f.root_id,
            catalog(false),
            |ticket| {
                f.persist(ticket)?;
                Err(Error::Busy)
            }
        ),
        Err(Error::Busy)
    ));
    assert!(f.base.join("ticket.bin").exists());
    control.revalidate_bootstrap_root().unwrap();
    fixture::not_under(control.root.file());
    fixture::denied_delete(&f.root.join(".layrs"));
    for mutation in [
        "missing",
        "ordinary",
        "directory",
        "backing",
        "out-of-sync",
        "late-extra",
    ] {
        let (f, control) = fixture::Fixture::create(&run, mutation);
        let target = f.root.join("Dir/data");
        let root = f.root.clone();
        let changed = Arc::new(AtomicBool::new(false));
        let flag = changed.clone();
        let catalog = catalog(false);
        let wrong_catalog = Catalog::new(99, catalog.entries().to_vec()).unwrap();
        let wrong_identity = wrong_catalog.file_identity(0);
        let outcome = population::test_hooks::scoped(
            move |point| {
                let wanted = if mutation == "late-extra" {
                    Point::AfterAudit
                } else {
                    Point::BeforeAudit
                };
                if point != wanted || flag.swap(true, Ordering::SeqCst) {
                    return Ok(());
                }
                match mutation {
                    "missing" => std::fs::remove_file(&target).unwrap(),
                    "ordinary" => {
                        std::fs::remove_file(&target).unwrap();
                        std::fs::write(&target, data()).unwrap();
                    }
                    "directory" => {
                        std::fs::remove_file(&target).unwrap();
                        std::fs::create_dir(&target).unwrap();
                    }
                    "backing" => change_placeholder(&target, Some(&wrong_identity), false),
                    "out-of-sync" => change_placeholder(&target, None, true),
                    "late-extra" => {
                        std::fs::write(root.join("extra"), b"preserve external writer bytes")
                            .unwrap()
                    }
                    _ => unreachable!(),
                }
                Ok(())
            },
            || {
                ProviderRootGuard::create_with_control_directory(
                    &control,
                    f.root_id,
                    catalog.clone(),
                    |ticket| f.persist(ticket),
                )
            },
        );
        assert!(changed.load(Ordering::SeqCst));
        assert!(outcome.is_err(), "{mutation} was incorrectly READY");
        assert!(
            matches!(
                f.ticket().guard_with_control_directory(catalog, &control),
                Err(Error::NeedsRepair(_))
            ),
            "{mutation}: no exact incomplete refusal"
        );
        assert_eq!(calls(), (0, 0));
        eprintln!("PASS {mutation} actual mutation -> factory refusal, exact INCOMPLETE retained");
    }
    // Error immediately after UPDATE is not rollback; expected reacquisition
    // must establish the exact current native outcome without repopulating.
    let (f, control) = fixture::Fixture::create(&run, "unknown-after-update");
    let outcome = population::test_hooks::scoped(
        |point| {
            if point == Point::AfterUpdate {
                Err(Error::Busy)
            } else {
                Ok(())
            }
        },
        || {
            ProviderRootGuard::create_with_control_directory(
                &control,
                f.root_id,
                catalog(false),
                |ticket| f.persist(ticket),
            )
        },
    );
    assert!(matches!(outcome, Err(Error::Busy)));
    f.ticket()
        .guard_with_control_directory(catalog(false), &control)
        .unwrap()
        .revalidate()
        .unwrap();
    // A legitimate complete layout is not an invisible-files capability.
    for name in [".git", "nested/.layrs"] {
        let (f, control) = fixture::Fixture::create(
            &run,
            if name == ".git" {
                "foreign-control"
            } else {
                "nested-control"
            },
        );
        let proof = create(&f, &control, catalog(false));
        let source = Source::new(catalog(false));
        let session =
            Session::reconnect_disconnected_with_control_directory(proof, source.clone()).unwrap();
        if name.contains('/') {
            std::fs::create_dir(f.root.join("nested")).unwrap();
        }
        std::fs::write(f.root.join(name), b"must not be hidden").unwrap();
        assert!(session.namespace().is_err());
        assert!(session.checkpoint().is_err());
        assert!(session.export_checkpoint().is_err());
        assert_eq!(source.bytes.load(Ordering::SeqCst), 0);
    }
}
