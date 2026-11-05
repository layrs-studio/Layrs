use super::*;
use std::{
    cell::Cell,
    io::{Seek, SeekFrom, Write},
    rc::Rc,
};

fn native_setup<T>(label: &str, action: impl FnOnce() -> T) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)) {
        Ok(value) => value,
        Err(cause) => {
            eprintln!("UNQUALIFIED {label}: native setup/mutation failed; original panic retained");
            std::panic::resume_unwind(cause)
        }
    }
}

#[test]
#[ignore = "drive-letter-only root receipt must refuse before strong FORMAT access; ordinary baseline retained"]
fn control_stage_observation_root_drive_case() {
    let run = fixture::run("root-drive-case");
    let f = Fixture::new(&run, "only-root-drive-letter", FORMAT);
    let mut wrong = f.allocated.clone();
    let drive = wrong.root.as_bytes()[0];
    assert!(drive.is_ascii_alphabetic());
    let changed = if drive.is_ascii_uppercase() {
        drive.to_ascii_lowercase()
    } else {
        drive.to_ascii_uppercase()
    };
    wrong
        .root
        .replace_range(0..1, &(changed as char).to_string());
    assert_ne!(wrong.root.as_bytes()[0], drive);
    assert_eq!(
        &wrong.root.as_bytes()[1..],
        &f.allocated.root.as_bytes()[1..]
    );
    wrong
        .validate()
        .expect("drive-case-only receipt must remain valid comparison data");
    let mut inverse = wrong.clone();
    inverse.root = f.allocated.root.clone();
    assert_eq!(
        inverse, f.allocated,
        "every other receipt field must be identical"
    );
    let original_wire = f.allocated.encode();
    let wrong_wire = wrong.encode();
    assert_eq!(original_wire.len(), wrong_wire.len());
    assert_eq!(
        original_wire
            .iter()
            .zip(&wrong_wire)
            .filter(|(a, b)| a != b)
            .count(),
        1
    );

    let writer = fs::open(&f.stage.join("FORMAT"), io::ALL_SHARING, FILE_WRITE_DATA).unwrap();
    let data_open = Rc::new(Cell::new(false));
    let seen = data_open.clone();
    let result = fault::scoped(
        Box::new(move |point, _| {
            if point == fault::Point::BeforeStrong {
                seen.set(true);
            }
            Ok(())
        }),
        || observe_allocated_control_stage(&wrong),
    );
    eprintln!(
        "ACTUAL drive-only receipt root={} versus canonical={}; BeforeStrong={}, error={:?}; writer retained through negative call",
        wrong.root,
        f.allocated.root,
        data_open.get(),
        result.as_ref().err()
    );
    drop(writer);
    let ordinary = observe_allocated_control_stage(&f.allocated).unwrap();
    assert!(ordinary.identity().matches_expected(&f.allocated));
    assert_eq!(ordinary.format_bytes(), FORMAT);
    assert_eq!(ordinary.root_namespace(), ObservedRootNamespace::Empty);
    drop(ordinary);
    eprintln!("PASS separate normal receipt baseline after dropping writer");
    // This is deliberately a failing regression on the unfixed implementation.
    // A sharing/type/native error is not a successful identity refusal.
    assert!(
        matches!(&result, Err(Error::IdentityMismatch)),
        "wrong refusal: {:?}",
        result.as_ref().err()
    );
    assert!(
        !data_open.get(),
        "drive-case mismatch reached strong FORMAT access"
    );
}

#[test]
#[ignore = "real zero/prefix/full/divergent/bounds observations and retained writer exclusion"]
fn control_stage_observation_bytes_and_writers() {
    let run = fixture::run("bytes-writers");
    for (label, bytes) in [
        ("zero", Vec::new()),
        ("prefix", FORMAT[..7].to_vec()),
        ("full", FORMAT.to_vec()),
        ("divergent", b"not the intended FORMAT".to_vec()),
        ("maximum", vec![0x6d; 65536]),
    ] {
        let f = Fixture::new(&run, label, &bytes);
        let observed = observe_allocated_control_stage(&f.allocated).unwrap();
        assert_eq!(observed.format_bytes(), bytes);
        assert!(observed.identity().matches_expected(&f.allocated));
        assert_eq!(observed.root_namespace(), ObservedRootNamespace::Empty);
        fixture::sharing(fs::open(
            &f.stage.join("FORMAT"),
            io::ALL_SHARING,
            FILE_WRITE_DATA,
        ));
        drop(observed);
        let writer = fs::open(&f.stage.join("FORMAT"), io::ALL_SHARING, FILE_WRITE_DATA).unwrap();
        fixture::sharing(observe_allocated_control_stage(&f.allocated));
        drop(writer);
        let second = observe_allocated_control_stage(&f.allocated).unwrap();
        assert_eq!(second.format_bytes(), bytes);
        eprintln!(
            "PASS {label} exact {} bytes; observation is not intended-byte authority; writer refusal32 in both directions",
            bytes.len()
        );
    }
    let f = Fixture::new(&run, "over-maximum", &vec![0x7b; 65537]);
    assert!(matches!(
        observe_allocated_control_stage(&f.allocated),
        Err(Error::Unsupported("FORMAT byte bounds"))
    ));
    let f = Fixture::new(&run, "ordinary-extra-entries", FORMAT);
    fixture::write_new(&f.root.join("ordinary-user-file"), b"retained");
    std::fs::create_dir(f.root.join("ordinary-user-dir")).unwrap();
    let observed = observe_allocated_control_stage(&f.allocated).unwrap();
    assert_eq!(observed.root_namespace(), ObservedRootNamespace::Nonempty);
    assert_eq!(observed.format_bytes(), FORMAT);
}

#[test]
#[ignore = "actual retained strong observation blocks publish; explicit same-prepared retry after drop"]
fn control_stage_observation_publication_lifetime() {
    let run = fixture::run("publication-lifetime");
    let f = Fixture::new(&run, "held-strong-observation", FORMAT);
    let observed = observe_allocated_control_stage(&f.allocated).unwrap();
    let persisted = Cell::new(0);
    let result = publish_control_directory_recoverable(
        &f.preflight,
        f.allocated.stage(),
        &f.allocated,
        FORMAT,
        None,
        |p| {
            persisted.set(persisted.get() + 1);
            f.persist(p)
        },
        &NoControlDirectoryPublishFaults,
    );
    let error = fixture::error(result);
    assert_eq!(persisted.get(), 1);
    assert!(
        matches!(
            &error,
            Error::ControlDirectory {
                kind: ControlDirectoryFailure::RenameRefused,
                point: ControlDirectoryPublishPoint::PreRename,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(
        matches!(fixture::raw(&error), Some(5 | 32)),
        "expected native rename refusal, never collision80/183: {error:?}"
    );
    let prepared = f.prepared();
    assert_eq!(prepared.stage_identity(), &f.allocated);
    assert_eq!(observed.format_bytes(), FORMAT);
    fixture::unchanged_stage(&f, FORMAT);
    eprintln!(
        "ACTUAL held A publication: persist=1, {error:?}; exact prepared receipt/stage/FORMAT and empty root retained"
    );
    drop(observed);
    let guard = publish_control_directory_recoverable(
        &f.preflight,
        f.allocated.stage(),
        &f.allocated,
        FORMAT,
        Some(&prepared),
        |_| panic!("prepared retry must not persist"),
        &NoControlDirectoryPublishFaults,
    )
    .unwrap();
    assert_eq!(guard.identity(), &prepared);
    guard.revalidate_bootstrap_root().unwrap();
    eprintln!("PASS explicit same-prepared retry after dropping A, callback0");

    let f = Fixture::new(&run, "stage-observer-only", FORMAT);
    let observer = io::directory_observer(&f.stage).unwrap();
    let result = publish_control_directory_recoverable(
        &f.preflight,
        f.allocated.stage(),
        &f.allocated,
        FORMAT,
        None,
        |p| f.persist(p),
        &NoControlDirectoryPublishFaults,
    );
    match result {
        Ok(guard) => {
            eprintln!("ACTUAL stage SHARE_ALL LIST observer alone permits publication");
            guard.revalidate_bootstrap_root().unwrap();
        }
        Err(error) => {
            eprintln!(
                "ACTUAL stage observer alone blocks publication: {error:?}; dropping only observer before explicit retry"
            );
            assert!(matches!(
                &error,
                Error::ControlDirectory {
                    kind: ControlDirectoryFailure::RenameRefused,
                    ..
                }
            ));
            assert!(matches!(fixture::raw(&error), Some(5 | 32)));
            fixture::unchanged_stage(&f, FORMAT);
            drop(observer);
            let p = f.prepared();
            publish_control_directory_recoverable(
                &f.preflight,
                f.allocated.stage(),
                &f.allocated,
                FORMAT,
                Some(&p),
                |_| panic!("prepared retry persisted"),
                &NoControlDirectoryPublishFaults,
            )
            .unwrap()
            .revalidate_bootstrap_root()
            .unwrap();
        }
    }
    for point in [
        ControlDirectoryPublishPoint::PostClose,
        ControlDirectoryPublishPoint::PreRename,
    ] {
        let f = Fixture::new(&run, &format!("cut-{point:?}"), FORMAT);
        let fault = fixture::Fault(|p| {
            if p == point {
                Err(std::io::Error::other("owned observation cut"))
            } else {
                Ok(())
            }
        });
        let error = fixture::error(publish_control_directory_recoverable(
            &f.preflight,
            f.allocated.stage(),
            &f.allocated,
            FORMAT,
            None,
            |p| f.persist(p),
            &fault,
        ));
        assert!(
            matches!(error,Error::ControlDirectory { kind:ControlDirectoryFailure::Interrupted,point:p,.. } if p==point)
        );
        fixture::unchanged_stage(&f, FORMAT);
        let observed = observe_allocated_control_stage(&f.allocated).unwrap();
        assert_eq!(observed.format_bytes(), FORMAT);
        drop(observed);
        let p = f.prepared();
        publish_control_directory_recoverable(
            &f.preflight,
            f.allocated.stage(),
            &f.allocated,
            FORMAT,
            Some(&p),
            |_| panic!("prepared retry persisted"),
            &NoControlDirectoryPublishFaults,
        )
        .unwrap()
        .revalidate_bootstrap_root()
        .unwrap();
        eprintln!("PASS {point:?} cut retained exact namespace; explicit retry only");
    }
}

#[test]
#[ignore = "native allocation identity/privacy/stream/link/type refusals; original witnesses retained"]
fn control_stage_observation_refusals() {
    let run = fixture::run("refusals");
    for field in 0..8 {
        let f = Fixture::new(&run, &format!("identity-{field}"), FORMAT);
        let mut wrong = f.allocated.clone();
        match field {
            0 => wrong.root = wrong.root.to_lowercase(),
            1 => {
                wrong.parent = wrong.parent.to_lowercase();
                wrong.stage = wrong.stage.to_lowercase();
            }
            2 => {
                let (parent, leaf) = wrong.stage.rsplit_once('\\').unwrap();
                wrong.stage = format!("{parent}\\{}", leaf.to_ascii_uppercase());
                assert_eq!(wrong.stage.rsplit_once('\\').unwrap().0, wrong.parent);
            }
            3 => wrong.volume ^= 1,
            4 => wrong.root_index ^= 0x4000,
            5 => wrong.parent_index ^= 0x4000,
            6 => wrong.directory_index ^= 0x4000,
            7 => wrong.format_index ^= 0x4000,
            _ => unreachable!(),
        }
        assert_ne!(wrong, f.allocated);
        wrong
            .validate()
            .expect("identity arm must reach native comparison, not fail pure validation");
        // A writer would deny a strong reader. Identity refusal must happen
        // first, through compatible metadata observers, with no data-open hook.
        let _writer = fs::open(&f.stage.join("FORMAT"), io::ALL_SHARING, FILE_WRITE_DATA).unwrap();
        let data_open = Rc::new(Cell::new(false));
        let seen = data_open.clone();
        let result = fault::scoped(
            Box::new(move |point, _| {
                if point == fault::Point::BeforeStrong {
                    seen.set(true);
                }
                Ok(())
            }),
            || observe_allocated_control_stage(&wrong),
        );
        assert!(
            matches!(result, Err(Error::IdentityMismatch)),
            "field {field}: {:?}",
            result.as_ref().err()
        );
        assert!(!data_open.get());
    }
    for mutation in [
        "root-replacement",
        "stage-replacement",
        "format-replacement",
        "stage-privacy",
        "format-privacy",
        "root-ads",
        "stage-ads",
        "format-ads",
        "format-hardlink",
        "stage-reparse",
        "format-reparse",
        "extra-stage-child",
    ] {
        let f = native_setup(mutation, || Fixture::new(&run, mutation, FORMAT));
        let expected = f.allocated.clone();
        // Preserve the original preflight lifetime in every other arm.
        let mut retained_preflight = Some(f.preflight);
        native_setup(mutation, || match mutation {
            "root-replacement" => {
                drop(retained_preflight.take());
                std::fs::rename(&f.root, f.base.join("root-retained-witness")).unwrap();
                std::fs::create_dir(&f.root).unwrap();
            }
            "stage-replacement" => {
                std::fs::rename(&f.stage, f.base.join("stage-retained-witness")).unwrap();
                fixture::private_directory(&f.stage);
                let mut file = fixture::private_file(&f.stage.join("FORMAT"));
                file.write_all(FORMAT).unwrap();
                file.sync_all().unwrap();
            }
            "format-replacement" => {
                std::fs::rename(
                    f.stage.join("FORMAT"),
                    f.base.join("FORMAT-retained-witness"),
                )
                .unwrap();
                let mut file = fixture::private_file(&f.stage.join("FORMAT"));
                file.write_all(FORMAT).unwrap();
                file.sync_all().unwrap();
            }
            "stage-privacy" => fixture::unprotect(&f.stage),
            "format-privacy" => fixture::unprotect(&f.stage.join("FORMAT")),
            "root-ads" => fixture::write_new(
                Path::new(&format!("{}:witness", f.root.display())),
                b"retained",
            ),
            "stage-ads" => fixture::write_new(
                Path::new(&format!("{}:witness", f.stage.display())),
                b"retained",
            ),
            "format-ads" => fixture::write_new(
                Path::new(&format!("{}:witness", f.stage.join("FORMAT").display())),
                b"retained",
            ),
            "format-hardlink" => {
                std::fs::hard_link(
                    f.stage.join("FORMAT"),
                    f.base.join("FORMAT-hardlink-witness"),
                )
                .unwrap();
                assert_eq!(
                    fs::identity(&format_observer(&f.stage.join("FORMAT")).unwrap())
                        .unwrap()
                        .nNumberOfLinks,
                    2
                );
            }
            "stage-reparse" => {
                std::fs::rename(
                    f.stage.join("FORMAT"),
                    f.base.join("FORMAT-retained-witness"),
                )
                .unwrap();
                assert!(std::fs::read_dir(&f.stage).unwrap().next().is_none());
                fixture::guid_reparse(&f.stage);
            }
            "format-reparse" => fixture::guid_reparse(&f.stage.join("FORMAT")),
            "extra-stage-child" => fixture::write_new(&f.stage.join("foreign"), b"retained"),
            _ => unreachable!(),
        });
        let error = fixture::error(observe_allocated_control_stage(&expected));
        let exact = match mutation {
            "root-replacement" | "stage-replacement" | "format-replacement" => {
                matches!(&error, Error::IdentityMismatch)
            }
            "stage-privacy" | "format-privacy" => matches!(
                &error,
                Error::Unsupported("control requires protected current-owner-only DACL")
            ),
            "root-ads" | "stage-ads" | "format-ads" => matches!(
                &error,
                Error::Unsupported("named control streams unsupported")
            ),
            "format-hardlink" => matches!(
                &error,
                Error::Unsupported("FORMAT must have exactly one link")
            ),
            "stage-reparse" => matches!(
                &error,
                Error::Unsupported("unsupported control directory attributes")
            ),
            "format-reparse" => {
                matches!(&error, Error::Unsupported("unsupported FORMAT attributes"))
            }
            "extra-stage-child" => matches!(
                &error,
                Error::NeedsRepair("control/bootstrap layout differs")
            ),
            _ => unreachable!(),
        };
        assert!(
            exact,
            "UNQUALIFIED {mutation}: expected its exact first-check predicate, got {error:?}"
        );
        eprintln!(
            "PASS actual {mutation} observation refusal: {error:?}; witness/fixture retained"
        );
    }
}

#[test]
#[ignore = "cfg(test) native cursor EOF, real growth, and boundary mutation gates"]
fn control_stage_observation_read_boundaries() {
    let run = fixture::run("read-boundaries");
    for change in [
        "eof",
        "growth",
        "final-appearance",
        "extra-child",
        "hardlink",
    ] {
        let f = Fixture::new(&run, change, FORMAT);
        let stage = f.stage.clone();
        let root = f.root.clone();
        let base = f.base.clone();
        let injected = Rc::new(Cell::new(false));
        let seen = injected.clone();
        let result = fault::scoped(
            Box::new(move |point, file| {
                let when = match change {
                    "eof" => fault::Point::BeforeRead,
                    "growth" | "hardlink" => fault::Point::BeforeStrong,
                    _ => fault::Point::BeforeFinalBoundary,
                };
                if point != when || seen.replace(true) {
                    return Ok(());
                }
                match change {
                    "eof" => {
                        let mut reader = file.unwrap();
                        reader.seek(SeekFrom::End(0))?;
                    }
                    "growth" => {
                        let mut writer =
                            fs::open(&stage.join("FORMAT"), io::ALL_SHARING, FILE_WRITE_DATA)?;
                        writer.seek(SeekFrom::End(0))?;
                        writer.write_all(b"extra")?;
                        writer.sync_all()?;
                        assert_eq!(fs::size(&fs::identity(&writer)?), FORMAT.len() as u64 + 5);
                    }
                    "final-appearance" => {
                        fixture::write_new(&root.join(".layrs"), b"foreign retained")
                    }
                    "extra-child" => fixture::write_new(&stage.join("foreign"), b"retained"),
                    "hardlink" => {
                        std::fs::hard_link(
                            stage.join("FORMAT"),
                            base.join("FORMAT-retained-link"),
                        )?;
                        assert_eq!(
                            fs::identity(&format_observer(&stage.join("FORMAT"))?)?.nNumberOfLinks,
                            2
                        );
                    }
                    _ => unreachable!(),
                }
                Ok(())
            }),
            || observe_allocated_control_stage(&f.allocated),
        );
        assert!(injected.get());
        let error = fixture::error(result);
        match change {
            "eof" => assert!(
                matches!(&error,Error::Io(e) if e.kind()==std::io::ErrorKind::UnexpectedEof)
            ),
            "growth" => assert!(matches!(&error, Error::IdentityMismatch)),
            "final-appearance" => assert!(matches!(&error, Error::ForeignRoot)),
            "extra-child" => assert!(matches!(
                &error,
                Error::NeedsRepair("control/bootstrap layout differs")
            )),
            "hardlink" => assert!(matches!(
                &error,
                Error::Unsupported("FORMAT must have exactly one link")
            )),
            _ => unreachable!(),
        }
        eprintln!("PASS actual {change} boundary refusal: {error:?}");
    }
}

#[test]
#[ignore = "B retains exact populated layout without bootstrap-only emptiness or historical parent"]
fn control_directory_unregistered_populated_layout() {
    let run = fixture::run("unregistered-layout");
    let f = Fixture::new(&run, "populated-and-history", FORMAT);
    let guard = f.publish();
    let expected = guard.identity().clone();
    fixture::write_new(&f.root.join("user-file"), b"private user data");
    std::fs::create_dir(f.root.join("user-directory")).unwrap();
    std::fs::rename(
        f.allocated.stage_parent(),
        f.base.join("allocation-parent-retained-witness"),
    )
    .unwrap();
    assert!(!f.allocated.stage_parent().exists());
    guard.revalidate().unwrap();
    guard.revalidate_unregistered_root().unwrap();
    let error = fixture::error(guard.revalidate_bootstrap_root());
    assert!(matches!(
        &error,
        Error::ControlDirectory {
            operation: "revalidate bootstrap root",
            ..
        }
    ));
    drop(guard);
    let fresh = ControlDirectoryGuard::open(&f.root, &expected).unwrap();
    fresh.revalidate().unwrap();
    fresh.revalidate_unregistered_root().unwrap();
    assert!(fresh.revalidate_bootstrap_root().is_err());
    eprintln!(
        "PASS fresh B with user entries and unavailable historical stage parent; bootstrap refuses extras, generic layout remains valid"
    );
}

#[test]
#[ignore = "native exact final collisions, dangling reparse and delete-pending; unavailable privilege fails qualification"]
fn control_stage_observation_final_collisions() {
    let run = fixture::run("final-collisions");
    for kind in [
        "file",
        "directory",
        "case-alias",
        "dangling-reparse",
        "cloud",
    ] {
        let f = Fixture::new(&run, kind, FORMAT);
        let final_path = f.root.join(if kind == "case-alias" {
            ".LAYRS"
        } else {
            ".layrs"
        });
        match kind {
            "file" => fixture::write_new(&final_path, b"foreign retained"),
            "directory" => std::fs::create_dir(&final_path).unwrap(),
            "case-alias" => {
                std::fs::create_dir(&final_path).unwrap();
                assert_eq!(
                    io::id(&io::directory_observer(&final_path).unwrap()).unwrap(),
                    io::id(&io::directory_observer(&f.root.join(".layrs")).unwrap()).unwrap()
                );
            }
            "dangling-reparse" => {
                std::os::windows::fs::symlink_dir(f.base.join("absent-target"), &final_path)
                    .unwrap_or_else(|e| {
                        panic!("UNQUALIFIED dangling reparse native creation: {e:?}")
                    });
                assert!(!f.base.join("absent-target").exists());
                let file = fs::open(&final_path, io::ALL_SHARING, FILE_READ_ATTRIBUTES).unwrap();
                assert_ne!(
                    fs::attributes(&file).unwrap().FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT,
                    0
                );
            }
            "cloud" => {
                std::fs::create_dir(&final_path).unwrap();
                registration::register(&final_path, "foreign final cloud collision");
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                observe_allocated_control_stage(&f.allocated),
                Err(Error::ForeignRoot)
            ),
            "{kind}"
        );
        eprintln!("PASS exact final {kind} collision; no adoption");
    }
    let f = Fixture::new(&run, "delete-pending", FORMAT);
    let path = f.root.join(".layrs");
    fixture::write_new(&path, b"owned delete-pending probe");
    fixture::write_new(
        &f.base.join("delete-pending-byte-witness"),
        b"owned delete-pending probe",
    );
    let file = fs::open(&path, io::ALL_SHARING, DELETE | FILE_READ_ATTRIBUTES).unwrap();
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    assert_ne!(
        unsafe {
            SetFileInformationByHandle(
                fs::handle(&file),
                FileDispositionInfo,
                (&info as *const FILE_DISPOSITION_INFO).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        },
        0,
        "UNQUALIFIED delete-pending mutation: {:?}",
        std::io::Error::last_os_error()
    );
    // Keep the pending object until process exit. The separate byte witness and
    // fixture survive; this intentionally exercises deletion, not cleanup.
    let _retained = Box::leak(Box::new(file));
    let raw = fixture::error(fs::open(&path, io::ALL_SHARING, FILE_READ_ATTRIBUTES));
    assert_ne!(
        fixture::raw(&raw),
        Some(2),
        "UNQUALIFIED native pending-name behavior cannot exercise unavailable-vs-absent"
    );
    let observed = fixture::error(observe_allocated_control_stage(&f.allocated));
    assert_eq!(fixture::raw(&observed), fixture::raw(&raw));
    eprintln!(
        "PASS native delete-pending refusal preserved exactly: direct={raw:?}, observation={observed:?}"
    );
}

#[test]
#[ignore = "owned directory case-sensitive metadata only; unavailable native support is unqualified"]
fn control_stage_observation_distinct_case_entry() {
    // SDK/windows-sys SystemServices constant, copied to avoid a feature/dependency change.
    const FILE_CS_FLAG_CASE_SENSITIVE_DIR: u32 = 1;
    let run = fixture::run("case-sensitive-entry");
    let f = Fixture::new(&run, "distinct-upper-control", FORMAT);
    let directory = fs::open(
        &f.root,
        io::ALL_SHARING,
        FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES,
    )
    .unwrap();
    let mode = FILE_CASE_SENSITIVE_INFO {
        Flags: FILE_CS_FLAG_CASE_SENSITIVE_DIR,
    };
    let ok = unsafe {
        SetFileInformationByHandle(
            fs::handle(&directory),
            FileCaseSensitiveInfo,
            (&mode as *const FILE_CASE_SENSITIVE_INFO).cast(),
            std::mem::size_of_val(&mode) as u32,
        )
    };
    assert_ne!(
        ok,
        0,
        "UNQUALIFIED per-fixture case-sensitive directory mode: {:?}; no OS policy changes",
        std::io::Error::last_os_error()
    );
    let mut actual = FILE_CASE_SENSITIVE_INFO::default();
    assert_ne!(
        unsafe {
            GetFileInformationByHandleEx(
                fs::handle(&directory),
                FileCaseSensitiveInfo,
                (&mut actual as *mut FILE_CASE_SENSITIVE_INFO).cast(),
                std::mem::size_of_val(&actual) as u32,
            )
        },
        0
    );
    assert_eq!(
        actual.Flags & FILE_CS_FLAG_CASE_SENSITIVE_DIR,
        FILE_CS_FLAG_CASE_SENSITIVE_DIR
    );
    std::fs::create_dir(f.root.join(".LAYRS")).unwrap();
    assert!(
        matches!(fs::open(&f.root.join(".layrs"),io::ALL_SHARING,FILE_READ_ATTRIBUTES),Err(Error::Io(e)) if e.raw_os_error()==Some(2)),
        "UNQUALIFIED distinct-name arm: exact lower-case native open still resolves alias or is unavailable"
    );
    let observation = observe_allocated_control_stage(&f.allocated).unwrap();
    assert_eq!(
        observation.root_namespace(),
        ObservedRootNamespace::Nonempty
    );
    assert_eq!(observation.format_bytes(), FORMAT);
    eprintln!(
        "PASS actual distinct case-sensitive .LAYRS remains visible Nonempty; exact .layrs absent; no alias audit"
    );
}

// Independent fresh fixtures leave the earlier aggregate (including its
// privilege-dependent dangling-symlink failure) unchanged and reproducible.
#[test]
#[ignore = "fresh cloud final collision independent of dangling-symlink privilege; registration retained"]
fn control_stage_observation_cloud_final_collision() {
    let run = fixture::run("cloud-final-independent");
    let f = Fixture::new(&run, "cloud", FORMAT);
    let path = f.root.join(".layrs");
    std::fs::create_dir(&path).unwrap();
    let held = io::directory_observer(&path).unwrap();
    io::plain_directory(&held).unwrap();
    let before = io::id(&held).unwrap();
    registration::register(&path, "independent foreign final cloud collision");
    let _fresh = registration::registered_pair("cloud final", &path, &held, before);
    let error = fixture::error(observe_allocated_control_stage(&f.allocated));
    assert!(
        matches!(&error, Error::ForeignRoot),
        "UNQUALIFIED exact final cloud collision refusal: {error:?}"
    );
    assert_eq!(io::id(&held).unwrap(), before);
    assert_eq!(
        io::id(&io::directory_observer(&path).unwrap()).unwrap(),
        before
    );
    assert_eq!(
        io::id(&io::directory_observer(&f.stage).unwrap()).unwrap(),
        (f.allocated.volume, f.allocated.directory_index)
    );
    let format = format_observer(&f.stage.join("FORMAT")).unwrap();
    assert_eq!(
        io::id(&format).unwrap(),
        (f.allocated.volume, f.allocated.format_index)
    );
    assert_eq!(std::fs::read(f.stage.join("FORMAT")).unwrap(), FORMAT);
    eprintln!(
        "PASS exact ForeignRoot for independently registered cloud final; foreign final and allocated stage identities/bytes retained, no adoption"
    );
}

#[test]
#[ignore = "fresh owned delete-pending final; exact non-2 native failure preserved, handle retained until process exit"]
fn control_stage_observation_delete_pending() {
    let run = fixture::run("delete-pending-independent");
    let f = Fixture::new(&run, "delete-pending", FORMAT);
    let path = f.root.join(".layrs");
    let bytes = b"owned delete-pending independent probe";
    fixture::write_new(&path, bytes);
    let witness = f.base.join("delete-pending-byte-witness");
    fixture::write_new(&witness, bytes);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let file = fs::open(&path, io::ALL_SHARING, DELETE | FILE_READ_ATTRIBUTES).unwrap();
    let before = io::id(&file).unwrap();
    assert_eq!(file.metadata().unwrap().len(), bytes.len() as u64);
    let attributes = fs::attributes(&file).unwrap();
    assert_eq!(attributes.ReparseTag, 0);
    assert_eq!(attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY, 0);
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    assert_ne!(
        unsafe {
            SetFileInformationByHandle(
                fs::handle(&file),
                FileDispositionInfo,
                (&info as *const FILE_DISPOSITION_INFO).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        },
        0,
        "UNQUALIFIED delete-pending mutation: {:?}",
        std::io::Error::last_os_error()
    );
    // As in the retained aggregate, the pending object lives until process exit;
    // its separate byte witness remains. This mutation is not fixture cleanup.
    let retained = Box::leak(Box::new(file));
    let direct = fixture::error(fs::open(&path, io::ALL_SHARING, FILE_READ_ATTRIBUTES));
    let Error::Io(native) = &direct else {
        panic!("UNQUALIFIED expected direct native open cause: {direct:?}");
    };
    let raw = native.raw_os_error().expect("native OS code required");
    assert_ne!(raw, 2, "UNQUALIFIED pending name appeared absent (raw2)");
    let observed = fixture::error(observe_allocated_control_stage(&f.allocated));
    assert!(
        matches!(&observed, Error::Io(error) if error.raw_os_error() == Some(raw) && error.kind() == native.kind()),
        "UNQUALIFIED original native failure must be preserved: direct={direct:?}, observation={observed:?}"
    );
    assert_eq!(io::id(retained).unwrap(), before);
    assert_eq!(retained.metadata().unwrap().len(), bytes.len() as u64);
    assert_eq!(std::fs::read(witness).unwrap(), bytes);
    assert_eq!(
        io::id(&io::directory_observer(&f.stage).unwrap()).unwrap(),
        (f.allocated.volume, f.allocated.directory_index)
    );
    let format = format_observer(&f.stage.join("FORMAT")).unwrap();
    assert_eq!(
        io::id(&format).unwrap(),
        (f.allocated.volume, f.allocated.format_index)
    );
    assert_eq!(std::fs::read(f.stage.join("FORMAT")).unwrap(), FORMAT);
    eprintln!(
        "PASS delete-pending exact non-2 refusal: direct={direct:?}, observation={observed:?}; original native ID/size, separate bytes and stage retained; no fixed raw5 assumption"
    );
}
