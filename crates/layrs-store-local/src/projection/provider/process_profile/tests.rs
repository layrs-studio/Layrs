//! Isolated execution-profile recipes; never alter the parent test process's SQLite globals.
use super::*;
pub(in crate::projection::provider) mod child;
use std::sync::atomic::{AtomicUsize, Ordering};
static EXTENSIONS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn extension(
    _: *mut ffi::sqlite3,
    _: *mut *mut std::ffi::c_char,
    _: *const ffi::sqlite3_api_routines,
) -> i32 {
    EXTENSIONS.fetch_add(1, Ordering::SeqCst);
    ffi::SQLITE_OK
}

#[test]
#[ignore = "owned audited Windows test host; emits actual candidate, no qualification claim"]
fn profile_candidate_report() {
    child::run(
        concat!(module_path!(), "::profile_candidate_report"),
        60,
        || {
            // Report actual native values independently of declaration success. No
            // expected runtime hash is invented; coordinator captures/freezes stdout.
            unsafe {
                println!(
                    "PROVIDER_SQLITE_SOURCE_ID={}",
                    CStr::from_ptr(ffi::sqlite3_sourceid()).to_str().unwrap()
                );
                println!(
                    "PROVIDER_SQLITE_VERSION={}",
                    ffi::sqlite3_libversion_number()
                );
                for index in 0..512 {
                    let p = ffi::sqlite3_compileoption_get(index);
                    if p.is_null() {
                        break;
                    }
                    let option = CStr::from_ptr(p).to_str().unwrap();
                    assert!(option.len() <= 256);
                    println!("PROVIDER_SQLITE_OPTION={option}");
                }
            }
            child::declare();
            let observed = gate().unwrap();
            println!("PROVIDER_SQLITE_VFS_GEOMETRY={:?}", &observed.vfs[..3]);
            declare().unwrap();
            assert_eq!(gate().unwrap().build, observed.build);
        },
    );
}

#[test]
#[ignore = "isolated owned test process mutates its automatic-extension registry"]
fn undeclared_profile_prevents_reference_and_file_open_callbacks() {
    child::run(
        concat!(
            module_path!(),
            "::undeclared_profile_prevents_reference_and_file_open_callbacks"
        ),
        60,
        || {
            assert_eq!(
                unsafe { ffi::sqlite3_auto_extension(Some(extension)) },
                ffi::SQLITE_OK
            );
            assert!(matches!(
                gate(),
                Err(ProviderSqliteProfileError::Undeclared)
            ));
            let reference = super::super::database_io::BareConnection::reference();
            assert!(matches!(
                reference,
                Err(super::super::recovery::ProviderBootstrapCause::Profile(
                    ProviderSqliteProfileError::Undeclared
                ))
            ));
            let file = super::super::database_io::BareConnection::open(
                std::path::Path::new(r"C:\provider-test-must-not-open.sqlite3"),
                false,
            );
            assert!(matches!(
                file,
                Err(super::super::recovery::ProviderBootstrapCause::Profile(
                    ProviderSqliteProfileError::Undeclared
                ))
            ));
            assert_eq!(
                EXTENSIONS.load(Ordering::SeqCst),
                0,
                "no SQLite open reached extension dispatch"
            );
            // Demonstrate the limitation through the real C API, not a pretend
            // detector: an autoextension runs before sqlite3_open_v2 returns.
            let mut raw = std::ptr::null_mut();
            assert_eq!(
                unsafe {
                    ffi::sqlite3_open_v2(
                        c":memory:".as_ptr(),
                        &mut raw,
                        ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_PRIVATECACHE,
                        c"win32".as_ptr(),
                    )
                },
                ffi::SQLITE_OK
            );
            assert_eq!(EXTENSIONS.load(Ordering::SeqCst), 1);
            assert_eq!(unsafe { ffi::sqlite3_close(raw) }, ffi::SQLITE_OK);
        },
    );
}

#[test]
#[ignore = "isolated owned process; deliberately changes an observable VFS field"]
fn observable_vfs_drift_invalidates_without_rearming() {
    child::run(
        concat!(
            module_path!(),
            "::observable_vfs_drift_invalidates_without_rearming"
        ),
        60,
        || {
            child::declare();
            let p = unsafe { ffi::sqlite3_vfs_find(c"win32".as_ptr()) };
            assert!(!p.is_null());
            let original = unsafe { (*p).mxPathname };
            unsafe {
                (*p).mxPathname = original + 1;
            }
            assert!(matches!(gate(), Err(ProviderSqliteProfileError::Drift)));
            unsafe {
                (*p).mxPathname = original;
            }
            assert!(matches!(
                declare(),
                Err(ProviderSqliteProfileError::Invalidated)
            ));
            assert!(matches!(
                gate(),
                Err(ProviderSqliteProfileError::Invalidated)
            ));
        },
    );
}

#[test]
fn profile_build_rejects_unqualified_callbacks_and_limit_drift() {
    let base = ProviderSqliteBuildProfile {
        source_id: PROVIDER_SQLITE_SOURCE_ID.into(),
        version_number: 3_046_000,
        compile_options: [
            "THREADSAFE=1",
            "MUTEX_W32",
            "MAX_PAGE_SIZE=65536",
            "DEFAULT_SECTOR_SIZE=4096",
            "USE_URI",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    };
    validate_build(&base).unwrap();
    for option in [
        "EXTRA_AUTOEXT=custom",
        "EXTRA_INIT=custom",
        "THREADSAFE=0",
        "MAX_PAGE_SIZE=32768",
        "OMIT_WAL",
    ] {
        let mut changed = base.clone();
        changed.compile_options.push(option.into());
        assert!(
            matches!(validate_build(&changed), Err(ProviderSqliteProfileError::UnsupportedBuild(cause)) if cause.contains(option))
        );
    }
}
