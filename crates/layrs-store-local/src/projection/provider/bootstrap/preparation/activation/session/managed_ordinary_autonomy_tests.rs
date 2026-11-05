//! Proposed fresh Provider A -> admitted B9 -> ordinary C10 autonomy recipe.
//! Positive admission/owner closes precede release of every setup holder and
//! public C reopen with both ancestor checkout paths unavailable. This proves
//! released-holder autonomy, not crash recovery, failing native xClose/rollback,
//! or a live D -> E export. No old fixture is reopened, deleted or repurposed.
use super::{
    managed_ordinary_installing_tests::{self as witness, Database},
    managed_ordinary_ready_tests::CURRENT,
    managed_ordinary_source_publication_tests::{with_admission_data, PublicationFixture, HISTORY},
};
use crate::{
    lock::StoreLock,
    projection::{
        capture::storage,
        format,
        provider::{
            admitted_ordinary::{self, AdmittedOrdinaryData, CaptureOrigin},
            database_io::{self, DatabaseFile, ReadState, WriterCloseObserver, WriterOpenPoint},
            managed_admission::read_admitted_import,
            managed_export::{AdmissionStep, OrdinaryManagedExport, OrdinaryReadyAdmission},
            managed_ready_reopen::OrdinaryTerminalResolution,
            process_profile::tests::child,
            records::{transition::TransitionState, Record},
            recovery::error::{ProviderBootstrapCause as Cause, Result as CauseResult},
        },
        shared::provider_access::FamilyAccess,
        CapturedPatch, ProjectionStore, RestoreSource, SessionMode,
    },
};
use layrs_core::scm::{FileId, RepoPath};
use layrs_vfs::Cancellation;
use rusqlite::ffi;
use std::{
    fs::{self, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::PathBuf,
};

const EDIT: &[u8] = b"autonomous ordinary C10 captured edit";
const DIRTY: &[u8] = b"autonomous ordinary C10 inverse preserves these bytes";

/// Paths only: this observer owns no native file/SQL/CAS capability.
struct Close {
    pins: [PathBuf; 2],
    counts: (usize, usize, usize),
}
impl Close {
    fn require_pins(&self) {
        for path in &self.pins {
            let error = OpenOptions::new()
                .write(true)
                .share_mode(7)
                .open(path)
                .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(32), "retained pin {path:?}");
        }
    }
}
impl WriterCloseObserver for Close {
    fn open_point(&mut self, point: WriterOpenPoint, raw: *mut ffi::sqlite3) -> CauseResult<()> {
        match point {
            WriterOpenPoint::BeforeRaw => {
                assert!(raw.is_null());
                self.counts.0 += 1;
                self.require_pins();
            }
            WriterOpenPoint::AfterRaw => {
                assert!(!raw.is_null());
                self.counts.1 += 1;
            }
            _ => {}
        }
        Ok(())
    }
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        self.require_pins();
        None
    }
    fn after_close(&mut self, success: bool) -> Option<Cause> {
        assert!(success, "requires a real successful C writer close");
        self.counts.2 += 1;
        self.require_pins();
        None
    }
}

/// Owned immutable observations and paths only; no setup holder can escape.
struct Released {
    provider: PathBuf,
    source: PathBuf,
    destination: PathBuf,
    family: PathBuf,
    source_database: PathBuf,
    source_marker: format::Marker,
    source_admission: AdmittedOrdinaryData,
    source_rows: Database,
    destination_admission: AdmittedOrdinaryData,
    captures: Vec<(CapturedPatch, CaptureOrigin)>,
    pins: [PathBuf; 2],
}

fn before_published(
    _: &mut OrdinaryManagedExport<'_>,
    f: &PublicationFixture<'_>,
    _: &Cancellation,
) {
    witness::read(f.source_database, |c| {
        let admitted = read_admitted_import(c, f.source_marker, None).unwrap();
        let actual = admitted
            .outgoing()
            .iter()
            .find(|row| row.transaction_id == f.ready.transaction_id)
            .unwrap();
        assert_eq!(actual.state, TransitionState::Ready);
    });
    witness::read(f.destination_database, |c| {
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            8
        );
        assert!(read_admitted_import(c, &f.destination.proof().format, None).is_err());
    });
}

fn admit_and_close(
    mut owner: OrdinaryManagedExport<'_>,
    f: &PublicationFixture<'_>,
    cancel: &Cancellation,
) -> Released {
    let (source_admission, source_rows, published) = witness::read(f.source_database, |c| {
        let admitted = read_admitted_import(c, f.source_marker, None).unwrap();
        let published = admitted
            .outgoing()
            .iter()
            .find(|row| row.transaction_id == f.ready.transaction_id)
            .unwrap();
        assert_eq!(published.state, TransitionState::Published);
        (
            admitted_ordinary::inspect(c, f.source_marker, None).unwrap(),
            witness::database(c),
            published.encode().unwrap(),
        )
    });
    let family_rows = witness::read(f.family_database, witness::database);
    let file = &f.expected.files()[0];
    let manifest = f
        .required
        .iter()
        .find(|m| m.id() == &file.manifest_id)
        .unwrap();
    let mut close = Close {
        pins: [
            f.destination_root.join(file.path.as_str()),
            witness::object(
                f.family_root,
                "chunks",
                manifest.chunks()[0].chunk_id().as_str(),
            ),
        ],
        counts: (0, 0, 0),
    };
    let held =
        OrdinaryReadyAdmission::from_retained(f.destination, f.protection, f.checkout, cancel)
            .unwrap_or_else(|e| panic!("retained C8 Ready {e}"));
    let progress = owner
        .admit_ready_destination(held, &mut close, cancel)
        .unwrap_or_else(|e| panic!("ordinary admission {:?}: {}", e.step, e.cause));
    assert_eq!(progress.step(), AdmissionStep::FreshVerified);
    assert_eq!(close.counts, (1, 1, 1));
    let (destination_admission, captures) = witness::read(f.destination_database, |c| {
        let admitted = read_admitted_import(c, &f.destination.proof().format, None).unwrap();
        assert_eq!(admitted.private_schema(), 10);
        assert_eq!(admitted.ready(), f.ready);
        assert_eq!(admitted.receipt(), f.receipt);
        assert_eq!(admitted.source_published().encode().unwrap(), published);
        assert_eq!(admitted.expected().header(), f.expected.header());
        assert_eq!(
            admitted.witness_column_bytes(),
            f.required.len() as u64 * 150
        );
        let data = admitted_ordinary::inspect(c, &f.destination.proof().format, None).unwrap();
        assert_eq!(data.admission_digest(), progress.admission_digest());
        assert!(
            data.imported_capture_count() >= 2,
            "B's initial admission capture and historical edit must both be inherited"
        );
        assert!(
            data.ready_capture_sequence() <= 8,
            "tiny fresh fixture capture bound"
        );
        let captures = (1..=data.ready_capture_sequence())
            .map(|sequence| {
                let origin =
                    admitted_ordinary::capture_origin(c, data.marker(), sequence, None).unwrap();
                let patch = storage::load_metadata_for_origin(
                    c,
                    sequence,
                    origin.identity(),
                    origin.maximum_generation(),
                )
                .unwrap();
                assert_eq!(&patch.identity, origin.identity());
                assert!(patch.projection_generation >= origin.minimum_generation());
                assert_eq!(origin.imported(), sequence <= data.imported_capture_count());
                (patch, origin)
            })
            .collect();
        (data, captures)
    });
    drop(progress);
    // Admission FreshVerified includes positive independent B/family closes;
    // the original B Access owner still needs its separate explicit close.
    owner.close().unwrap();
    assert_eq!(
        witness::read(f.source_database, witness::database),
        source_rows
    );
    assert_eq!(
        witness::read(f.family_database, witness::database),
        family_rows
    );
    close.require_pins(); // C's checkout/CAS proofs remain live until helper return.
    Released {
        provider: f.provider_source_root.to_owned(),
        source: f.source_root.to_owned(),
        destination: f.destination_root.to_owned(),
        family: f.family_root.to_owned(),
        source_database: f.source_database.to_owned(),
        source_marker: f.source_marker.clone(),
        source_admission,
        source_rows,
        destination_admission,
        captures,
        pins: close.pins,
    }
}

/// Fresh admitted C10 roots/commitments only. The publication helper returns
/// after positively closing B and dropping every original C/store holder;
/// opening both former pins for writing independently checks that release.
pub(super) fn released_source10() -> (PathBuf, AdmittedOrdinaryData) {
    let f = with_admission_data(before_published, admit_and_close);
    for pin in &f.pins {
        drop(
            OpenOptions::new()
                .write(true)
                .share_mode(7)
                .open(pin)
                .expect("released C10 setup must retain no checkout/CAS pin"),
        );
    }
    (f.destination, f.destination_admission)
}

fn move_ancestor_checkouts(f: &Released) -> [(PathBuf, PathBuf); 2] {
    // canonicalize resolves the existing directories/junctions before checking
    // containment; destinations are fresh siblings of those resolved sources.
    let parent = fs::canonicalize(std::env::var_os("LAYRS_VFS_TEST_PARENT").unwrap()).unwrap();
    let destination = fs::canonicalize(&f.destination).unwrap();
    let family = fs::canonicalize(&f.family).unwrap();
    let moves = [&f.provider, &f.source].map(|path| {
        let source = fs::canonicalize(path).unwrap();
        let unavailable = source.with_extension("c10-source-unavailable");
        assert!(source.is_absolute() && unavailable.is_absolute());
        for path in [&source, &unavailable] {
            assert!(path.starts_with(&parent) && path != &parent);
            assert!(!destination.starts_with(path) && !path.starts_with(&destination));
            assert!(!family.starts_with(path) && !path.starts_with(&family));
        }
        assert!(matches!(fs::symlink_metadata(&unavailable), Err(e) if e.kind() == std::io::ErrorKind::NotFound));
        (source, unavailable)
    });
    assert!(!moves[0].0.starts_with(&moves[1].0) && !moves[1].0.starts_with(&moves[0].0));
    assert_ne!(moves[0].1, moves[1].1);
    // Both preflights complete before either move. Preserve the fresh A/B bytes
    // for investigation; do not move C or the shared family and do not delete.
    for (source, unavailable) in &moves {
        fs::rename(source, unavailable).unwrap();
        assert!(!source.try_exists().unwrap() && unavailable.is_dir());
    }
    assert!(!f.provider.try_exists().unwrap() && !f.source.try_exists().unwrap());
    moves
}

fn require_ancestors_unavailable(f: &Released, moved: &[(PathBuf, PathBuf); 2]) {
    assert!(!f.provider.try_exists().unwrap() && !f.source.try_exists().unwrap());
    for (source, unavailable) in moved {
        assert!(!source.try_exists().unwrap() && unavailable.is_dir());
    }
}

/// B's immutable DATA is not physical authority. The fixture preserves its
/// shared private DB; acquire fresh lifecycle/owner gates and read it independently.
/// never open B's moved checkout or construct an ordinary store for B.
fn require_source_unchanged(f: &Released) {
    assert!(!f.source.try_exists().unwrap());
    assert!(
        f.source_database.try_exists().unwrap(),
        "preserved shared B metadata required for the unchanged-admission check"
    );
    let shared = f.source_marker.shared.as_ref().unwrap();
    let family = FamilyAccess::open(&shared.root).unwrap();
    let _lifecycle = family.lifecycle_lease().unwrap();
    let _owner = StoreLock::acquire_existing(
        &shared.metadata_path.join("layrs.lock"),
        SessionMode::Shared,
    )
    .unwrap();
    let database =
        DatabaseFile::open(f.source_database.clone(), &shared.database_identity).unwrap();
    match database_io::read(&database, |c| {
        f.source_admission
            .require_same_import(&admitted_ordinary::inspect(c, &f.source_marker, None)?)?;
        assert_eq!(witness::database(c), f.source_rows);
        Ok(())
    })
    .unwrap()
    {
        ReadState::Valid(()) => {}
        _ => panic!("fresh B private read did not positively validate and close"),
    }
    database.verify().unwrap();
    family.verify().unwrap();
    assert!(!f.source.try_exists().unwrap());
}

fn expected_bytes(file_id: &FileId) -> &'static [u8] {
    [HISTORY, CURRENT]
        .into_iter()
        .find(|bytes| &FileId::from_bytes(bytes) == file_id)
        .expect("inherited B/current capture must reference genuine fixture bytes")
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_c10_reopens_and_captures_restores_after_all_holders_and_ancestors_release() {
    // Finite diagnostic proposal: prior B publication setup took 1438s;
    // retained C10 admission and this autonomy cycle remain unmeasured.
    child::run(concat!(module_path!(), "::managed_ordinary_c10_reopens_and_captures_restores_after_all_holders_and_ancestors_release"), 3600, || {
        let f = with_admission_data(before_published, admit_and_close);
        // A disconnected positively inside admitted_destination_with_source;
        // B closed positively in admit_and_close; helper return dropped every
        // C allocation, checkout/CAS proof and store. Only DATA remains here.
        for pin in &f.pins {
            drop(OpenOptions::new().write(true).share_mode(7).open(pin)
                .expect("all setup checkout/CAS pins must have been released"));
        }
        let moved = move_ancestor_checkouts(&f);
        require_source_unchanged(&f);
        // Keep B's actual private DB inaccessible during every C operation,
        // as well as its moved checkout. This fixture-only deny-share handle
        // is released solely for the final independent B baseline check.
        let source_database_unavailable = OpenOptions::new().read(true).share_mode(0)
            .open(&f.source_database).unwrap();
        let denied = OpenOptions::new().read(true).share_mode(7)
            .open(&f.source_database).unwrap_err();
        assert_eq!(denied.raw_os_error(), Some(32));
        eprintln!("C10_AUTONOMY_PHASE=released-and-ancestors-unavailable");
        let cancel = Cancellation::default();
        let terminal = OrdinaryTerminalResolution::open(&f.destination, &cancel).unwrap();
        assert_eq!(terminal.admission_digest(), f.destination_admission.admission_digest());
        f.destination_admission.require_same_import(terminal.admitted()).unwrap();
        terminal.require(&cancel).unwrap();
        drop(terminal); // Release its newly acquired owner and immutable byte pins.
        require_ancestors_unavailable(&f, &moved);
        let store = ProjectionStore::open(&f.destination).unwrap();
        let state = store.state().unwrap();
        let path = RepoPath::parse("README.md").unwrap();
        assert_eq!(fs::read(f.destination.join(path.as_str())).unwrap(), CURRENT);
        assert!(f.captures.iter().any(|(_, origin)| origin.imported()
            && origin.identity() == &f.source_marker.identity));
        // A -> B's Ready capture is authored by B, not by Provider A. The
        // genuine B captures stay imported in C; C's Ready suffix is local.
        assert!(f.captures.iter().any(|(_, origin)| !origin.imported()
            && origin.identity() == store.identity()));
        let mut empty_admission_seen = false;
        let mut historical_bytes_seen = false;
        let mut current_bytes_seen = false;
        for (patch, origin) in &f.captures {
            assert_eq!(store.captured_patch(patch.sequence).unwrap(), *patch);
            assert_eq!(&patch.identity, origin.identity());
            if patch.sequence == f.source_admission.ready_capture_sequence() {
                assert_eq!(patch.identity, f.source_marker.identity);
                assert!(origin.imported());
                assert!(patch.changes.is_empty(), "initial B Ready preserves the unchanged provider base");
                assert!(!empty_admission_seen);
                empty_admission_seen = true;
                continue;
            }
            assert!(!patch.changes.is_empty(), "only the initial B Ready capture is empty in this fixture");
            for change in &patch.changes {
                assert_eq!(change.path, path);
                let replacement = change.replacement.as_ref().expect("fresh HISTORY/CURRENT replacement");
                let expected = expected_bytes(&replacement.file_id);
                let mut bytes = Vec::new();
                store.write_captured_file(patch.sequence, &change.path, &mut bytes).unwrap();
                assert_eq!(bytes, expected);
                if expected == HISTORY {
                    assert!(origin.imported());
                    assert_eq!(patch.identity, f.source_marker.identity);
                    historical_bytes_seen = true;
                } else {
                    assert_eq!(expected, CURRENT);
                    assert!(!origin.imported());
                    assert_eq!(patch.sequence, f.destination_admission.ready_capture_sequence());
                    current_bytes_seen = true;
                }
            }
        }
        assert!(empty_admission_seen && historical_bytes_seen && current_bytes_seen);
        format::access(&store, SessionMode::Shared).unwrap().run(|access| {
            f.destination_admission.require_same_import(
                &admitted_ordinary::inspect(&access.connection, &store.marker, None)?)?;
            for (patch, origin) in &f.captures {
                assert_eq!(admitted_ordinary::capture_origin(&access.connection, &store.marker, patch.sequence, None)?, *origin);
            }
            Ok(())
        }).unwrap();
        fs::write(f.destination.join(path.as_str()), EDIT).unwrap();
        let captured = store.capture_patch(state.generation).unwrap();
        assert_eq!(captured.identity, *store.identity());
        assert_eq!(captured.sequence, f.destination_admission.ready_capture_sequence() + 1);
        assert_eq!(store.capture_patch(state.generation).unwrap(), captured, "unchanged edit deduplicates");
        let mut bytes = Vec::new();
        store.write_captured_file(captured.sequence, &path, &mut bytes).unwrap();
        assert_eq!(bytes, EDIT);
        fs::write(f.destination.join(path.as_str()), DIRTY).unwrap();
        let preview = store.preview_restore(state.generation, &RestoreSource::Capture {
            sequence: captured.sequence, paths: vec![path.clone()],
        }).unwrap();
        let receipt = store.apply_restore(&preview).unwrap();
        assert_eq!(fs::read(f.destination.join(path.as_str())).unwrap(), EDIT);
        let inverse = store.preview_restore(state.generation, &RestoreSource::Inverse {
            operation_id: receipt.operation_id,
        }).unwrap();
        store.apply_restore(&inverse).unwrap();
        assert_eq!(fs::read(f.destination.join(path.as_str())).unwrap(), DIRTY);
        bytes.clear();
        store.write_captured_file(captured.sequence, &path, &mut bytes).unwrap();
        assert_eq!(bytes, EDIT, "inverse preserves immutable captured bytes");
        format::access(&store, SessionMode::Shared).unwrap().run(|access| {
            let current = admitted_ordinary::inspect(&access.connection, &store.marker, None)?;
            f.destination_admission.require_same_import(&current)?;
            let origin = admitted_ordinary::capture_origin(&access.connection, &store.marker, captured.sequence, None)?;
            assert!(!origin.imported());
            assert_eq!(origin.identity(), store.identity());
            Ok(())
        }).unwrap();
        drop(store);
        // Restore the original checkout bytes before testing the terminal
        // fence. Real captures/restores add C retention roots, so the complete
        // family interpreter rejects this mutable state before private-row
        // inventory. The SQL-only recipe separately exercises that inventory.
        fs::write(f.destination.join(path.as_str()), CURRENT).unwrap();
        match OrdinaryTerminalResolution::open(&f.destination, &cancel) {
            Err(error) => assert_eq!(error.to_string(), "exact protection owner has additional roots"),
            Ok(_) => panic!("terminal resolution accepted coherent later ordinary mutations"),
        }
        fs::write(f.destination.join(path.as_str()), DIRTY).unwrap();
        let reopened = ProjectionStore::open(&f.destination).unwrap();
        assert_eq!(reopened.state().unwrap(), state);
        assert_eq!(reopened.captured_patch(captured.sequence).unwrap(), captured);
        assert_eq!(fs::read(f.destination.join(path.as_str())).unwrap(), DIRTY);
        drop(reopened);
        drop(source_database_unavailable);
        require_source_unchanged(&f);
        require_ancestors_unavailable(&f, &moved);
        eprintln!("C10_AUTONOMY_PHASE=public-reopen-capture-restore-inverse-complete");
    });
}
