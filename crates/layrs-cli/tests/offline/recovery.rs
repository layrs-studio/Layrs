use super::*;
use layrs_store_local::projection::{CaptureFaults, CapturePoint};

#[test]
fn releasing_an_acquiring_preparation_cancels_its_reservation_without_touching_files() {
    let (f, store) = Fixture::new(true);
    let request = AvailabilityRequest {
        version: store.current_version().unwrap(),
        other: None,
        paths: vec![RepoPath::parse("a.bin").unwrap()],
        operation: OfflineOperation::Read,
    };
    let preparation = store.begin_preparation(&request).unwrap();
    assert_eq!(preparation.status, "acquiring");
    let family = store.shared_family().unwrap().unwrap();
    assert!(family.usage().unwrap().reserved_bytes > 0);
    let released = f.run(&["partial", "offline", "release", &preparation.id], true);
    assert_eq!(released["data"], json!({"released":preparation.id}));
    assert_eq!(store.preparations().unwrap()[0].status, "released");
    assert_eq!(family.usage().unwrap().reserved_bytes, 0);
    assert_eq!(fs::read(f.root.join("a.bin")).unwrap(), BASE);
    assert!(!f.root.join("later.txt").exists());
}

#[test]
fn cancelling_capture_reservation_keeps_edits_and_accepted_history() {
    struct Stop;
    impl CaptureFaults for Stop {
        fn check(&self, point: CapturePoint) -> io::Result<()> {
            if point == CapturePoint::BeforeCommit {
                Err(io::Error::other("before capture commit"))
            } else {
                Ok(())
            }
        }
    }
    let (f, store) = Fixture::new(true);
    fs::write(f.root.join("a.bin"), b"edit survives cancellation").unwrap();
    assert!(store.capture_patch_with_faults(1, &Stop).is_err());
    assert!(store.latest_captured_patch().unwrap().is_none());
    let cancelled = f.run(&["partial", "offline", "cancel-reservation"], true);
    assert_eq!(cancelled["data"], json!({"reservationCancelled":true}));
    assert_eq!(
        fs::read(f.root.join("a.bin")).unwrap(),
        b"edit survives cancellation"
    );
    assert_eq!(
        store
            .shared_family()
            .unwrap()
            .unwrap()
            .usage()
            .unwrap()
            .reserved_bytes,
        0
    );
    let capture = store.capture_patch(1).unwrap();
    f.run(&["partial", "offline", "cancel-reservation"], true);
    assert_eq!(
        store.latest_captured_patch().unwrap().unwrap().digest,
        capture.digest
    );
    assert_eq!(
        fs::read(f.root.join("a.bin")).unwrap(),
        b"edit survives cancellation"
    );
}

#[test]
fn release_generation_preserves_active_and_captured_content_through_eviction() {
    let (f, store) = Fixture::new(true);
    let active = f.run(&["partial", "offline", "release-generation", "1"], false);
    assert!(
        active["error"]["message"]
            .as_str()
            .unwrap()
            .contains("active base")
    );
    fs::write(f.root.join("a.bin"), b"accepted captured edit").unwrap();
    let capture = store.capture_patch(1).unwrap();
    let mut next = store.state().unwrap().snapshot;
    next.observed_authorization_epoch += 1;
    store.compare_and_swap(1, &next).unwrap();
    // Old generation claims can retire after exact current/capture-base subsets
    // are protected. A retained dependency no longer blocks the entire release.
    let released = f.run(&["partial", "offline", "release-generation", "1"], true);
    assert_eq!(released["data"], json!({"generation":1,"releasedClaims":1}));
    let repeated = f.run(&["partial", "offline", "release-generation", "1"], true);
    assert_eq!(repeated["data"], json!({"generation":1,"releasedClaims":0}));
    f.run(&["partial", "offline", "release-generation", "2"], false);
    f.run(&["partial", "offline", "release-generation", "999"], false);
    let released = f.run(&["partial", "offline", "release-generation", "0"], true);
    assert_eq!(released["data"], json!({"generation":0,"releasedClaims":0}));
    let selector = format!("capture:{}", capture.sequence);
    for phase in ["before-evict", "after-evict"] {
        if phase == "after-evict" {
            f.run(&["partial", "offline", "evict"], true);
        }
        for (version, expected, label) in [
            ("current", BASE, "active"),
            ("generation:1", BASE, "historical-base"),
            (
                selector.as_str(),
                b"accepted captured edit".as_slice(),
                "capture",
            ),
        ] {
            let check = f.run(
                &[
                    "partial",
                    "offline",
                    "check",
                    "read",
                    "a.bin",
                    "--version",
                    version,
                ],
                true,
            );
            assert_eq!(check["data"]["available"], true);
            assert_eq!(
                check["data"]["paths"][0]["fileId"],
                json!(FileId::from_bytes(expected))
            );
            assert_eq!(check["data"]["paths"][0]["retained"], true);
            let output = f.outer.join(format!("{phase}-{label}.bin"));
            f.run(
                &[
                    "partial",
                    "offline",
                    "read",
                    "a.bin",
                    "--version",
                    version,
                    "--output",
                    text(&output),
                ],
                true,
            );
            assert_eq!(fs::read(output).unwrap(), expected);
        }
    }
    assert_eq!(
        fs::read(f.root.join("a.bin")).unwrap(),
        b"accepted captured edit"
    );
    assert!(!f.root.join("later.txt").exists());
    assert_eq!(
        store.latest_captured_patch().unwrap().unwrap().digest,
        capture.digest
    );
    assert_eq!(store.state().unwrap().generation, 2);
}

struct StopConversion;
impl ConversionFaults for StopConversion {
    fn check(&self, point: ConversionPoint) -> io::Result<()> {
        if point == ConversionPoint::CheckoutCopied {
            Err(io::Error::other("after checkout copy"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn family_abandon_uses_only_explicit_binding_with_missing_source_and_retains_files() {
    let (f, source) = Fixture::new(false);
    let family_root = f.empty("family");
    f.run(
        &[
            "partial",
            "offline",
            "family",
            "create",
            text(&family_root),
            "--quota-bytes",
            "16777216",
        ],
        true,
    );
    let family = PartialStoreFamily::open(&family_root).unwrap();
    let destination = f.empty("retained");
    assert!(
        source
            .convert_into_family_with_faults(
                &family,
                &destination,
                WorktreeId::parse("worktree:abandon-cli").unwrap(),
                &StopConversion
            )
            .is_err()
    );
    assert_eq!(fs::read(destination.join("a.bin")).unwrap(), BASE);
    let marker_before = fs::read(destination.join(".layrs/FORMAT")).unwrap();
    fs::write(f.root.join("a.bin"), b"changed original").unwrap();
    assert!(source.resume_conversion_into(&destination).is_err());
    drop(source);
    let saved_source = f.outer.join("saved source");
    // Both paths belong to this disposable fixture; the original path is now absent.
    fs::rename(&f.root, &saved_source).unwrap();
    let abandoned = f.at(
        &f.outer,
        &[
            "--space",
            text(&f.root),
            "partial",
            "offline",
            "family",
            "abandon",
            "--family",
            text(&family_root),
            "--destination",
            text(&destination),
        ],
        true,
    );
    assert_eq!(
        abandoned["data"],
        json!({"destination":destination,"abandoned":true,"filesRetained":true})
    );
    assert_eq!(fs::read(destination.join("a.bin")).unwrap(), BASE);
    assert_eq!(
        fs::read(destination.join(".layrs/FORMAT")).unwrap(),
        marker_before
    );
    assert_eq!(
        fs::read(saved_source.join("a.bin")).unwrap(),
        b"changed original"
    );
    assert!(!f.root.exists());
}

#[test]
fn family_abandon_refuses_ready_or_foreign_destinations_without_touching_them() {
    let (f, _) = Fixture::new(false);
    let family_root = f.empty("family");
    let other_family = f.empty("other family");
    for path in [&family_root, &other_family] {
        f.run(
            &[
                "partial",
                "offline",
                "family",
                "create",
                text(path),
                "--quota-bytes",
                "16777216",
            ],
            true,
        );
    }
    let destination = f.empty("accepted");
    f.run(
        &[
            "partial",
            "offline",
            "family",
            "convert",
            "--family",
            text(&family_root),
            "--destination",
            text(&destination),
        ],
        true,
    );
    for family in [&family_root, &other_family] {
        f.at(
            &f.outer,
            &[
                "partial",
                "offline",
                "family",
                "abandon",
                "--family",
                text(family),
                "--destination",
                text(&destination),
            ],
            false,
        );
        assert_eq!(fs::read(destination.join("a.bin")).unwrap(), BASE);
        assert!(ProjectionStore::open(&destination).unwrap().is_shared());
    }
    assert_eq!(fs::read(f.root.join("a.bin")).unwrap(), BASE);
}
