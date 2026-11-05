//! Proposed fresh B -> C8 admission after every publication holder is gone.
//! Each recipe creates its own genuine Provider A -> admitted B9 -> C8 fixture.
//! ACK refusal follows a successful physical C writer close, not an OS close
//! failure or crash. Instrumented B refusal proves the resolution read/identity
//! route only; retained source pins do not make B physically unavailable.
use super::{
    managed_ordinary_installing_tests::{self as witness, Database},
    managed_ordinary_source_publication_tests::{PublicationFixture, with_admission_data},
};
use crate::{
    error::StoreError,
    lock::StoreLock,
    projection::{
        ProjectionStore, SessionMode, format,
        provider::{
            admitted_ordinary::{self, AdmittedOrdinaryData},
            database_io::{
                WriterCloseObserver, WriterOpenPoint,
                tests::fresh_ordinary_support::{ReadBoundary, with_observer},
            },
            managed_admission::read_admitted_import,
            managed_export::OrdinaryManagedExport,
            managed_ready_reopen::{
                AdmissionStep, OrdinaryAdmissionSource, OrdinaryFreshAdmission,
                OrdinaryFreshAdmissionFailure, OrdinaryReadyInspection, OrdinaryTerminalResolution,
            },
            process_profile::tests::child,
            records::{
                Record,
                import::ProviderTransitionReceiptRecord,
                transition::{ProviderTransitionIntentRecord, TransitionState},
            },
            recovery::error::{ProviderBootstrapCause as Cause, Result as CauseResult, invalid},
        },
    },
};
use layrs_core::scm::RepoPath;
use layrs_vfs::Cancellation;
use rusqlite::ffi;
use std::{
    cell::{Cell, RefCell},
    fs::{self, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    rc::Rc,
};

const WITHHELD_ACK: &str = "fresh ordinary C10 successful writer close acknowledgement withheld";
const SUFFIX: &[u8] = b"B coherent capture after Published and before fresh C admission";
const LATER_SUFFIX: &[u8] = b"B remains mutable after the fresh source owner is released";

fn snapshot(path: &Path) -> Database {
    witness::read(path, witness::database)
}

/// Owned paths, records and observations only. No source/checkout/CAS/SQL proof
/// escapes the callback or survives the publication helper's final teardown.
struct Fixture {
    source: PathBuf,
    destination: PathBuf,
    source_database: PathBuf,
    destination_database: PathBuf,
    family_database: PathBuf,
    source_marker: format::Marker,
    source_admission: AdmittedOrdinaryData,
    source_capture_count: u64,
    source_rows: Database,
    destination_rows: Database,
    family_rows: Database,
    ready: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
    published: Vec<u8>,
    pins: [PathBuf; 2],
}

fn before_noop(_: &mut OrdinaryManagedExport<'_>, _: &PublicationFixture<'_>, _: &Cancellation) {}

fn close_publication(
    owner: OrdinaryManagedExport<'_>,
    f: &PublicationFixture<'_>,
    _: &Cancellation,
) -> Fixture {
    let (source_admission, source_capture_count, source_rows, published) =
        witness::read(f.source_database, |c| {
            let admitted = read_admitted_import(c, f.source_marker, None).unwrap();
            let published = admitted
                .outgoing()
                .iter()
                .find(|row| row.transaction_id == f.ready.transaction_id)
                .unwrap();
            assert_eq!(published.state, TransitionState::Published);
            (
                admitted_ordinary::inspect(c, f.source_marker, None).unwrap(),
                c.query_row("SELECT count(*) FROM projection_captures", [], |r| {
                    r.get::<_, u64>(0)
                })
                .unwrap(),
                witness::database(c),
                published.encode().unwrap(),
            )
        });
    let destination_rows = witness::read(f.destination_database, |c| {
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            8
        );
        assert!(read_admitted_import(c, &f.destination.proof().format, None).is_err());
        witness::database(c)
    });
    let family_rows = snapshot(f.family_database);
    let file = &f.expected.files()[0];
    let manifest = f
        .required
        .iter()
        .find(|m| m.id() == &file.manifest_id)
        .unwrap();
    let pins = [
        f.destination_root.join(file.path.as_str()),
        witness::object(
            f.family_root,
            "chunks",
            manifest.chunks()[0].chunk_id().as_str(),
        ),
    ];
    // Only a positive explicit close acknowledges the original B Access owner.
    // No admission is attempted while these retained publication proofs live.
    owner.close().unwrap();
    assert_eq!(snapshot(f.source_database), source_rows);
    assert_eq!(snapshot(f.destination_database), destination_rows);
    assert_eq!(snapshot(f.family_database), family_rows);
    Fixture {
        source: f.source_root.to_owned(),
        destination: f.destination_root.to_owned(),
        source_database: f.source_database.to_owned(),
        destination_database: f.destination_database.to_owned(),
        family_database: f.family_database.to_owned(),
        source_marker: f.source_marker.clone(),
        source_admission,
        source_capture_count,
        source_rows,
        destination_rows,
        family_rows,
        ready: ProviderTransitionIntentRecord::decode(&f.ready.encode().unwrap()).unwrap(),
        receipt: ProviderTransitionReceiptRecord::decode(&f.receipt.encode().unwrap()).unwrap(),
        published,
        pins,
    }
}

fn fresh_fixture() -> Fixture {
    let f = with_admission_data(before_noop, close_publication);
    // Helper return is after positive A disconnect, explicit B close above,
    // and drop of C checkout/CAS protection, allocation and the B store.
    for path in &f.pins {
        drop(
            OpenOptions::new()
                .write(true)
                .share_mode(7)
                .open(path)
                .expect("original C checkout/CAS publication pins must all be released"),
        );
    }
    f
}

fn require_source_import(f: &Fixture) {
    witness::read(&f.source_database, |c| {
        f.source_admission
            .require_same_import(&admitted_ordinary::inspect(c, &f.source_marker, None).unwrap())
            .unwrap();
        let admitted = read_admitted_import(c, &f.source_marker, None).unwrap();
        let published = admitted
            .outgoing()
            .iter()
            .find(|row| row.transaction_id == f.ready.transaction_id)
            .unwrap();
        assert_eq!(published.encode().unwrap(), f.published);
    });
}

fn require_owner_contention(path: &Path, mode: SessionMode) {
    let error = StoreLock::acquire_existing(path, mode).unwrap_err();
    assert!(
        matches!(
            error,
            StoreError::LockBusy(_) | StoreError::ReentrantLock(_)
        ),
        "nonblocking owner contention, not another refusal: {error}"
    );
}

/// Reopens B through its public ordinary route and adds a legitimate suffix.
/// No direct SQL mutation, frontier repair or schema construction is involved.
fn capture_source_suffix(f: &Fixture, bytes: &[u8], expected_sequence: u64) {
    let store = ProjectionStore::open(&f.source).unwrap();
    let state = store.state().unwrap();
    let path = RepoPath::parse("README.md").unwrap();
    fs::write(f.source.join(path.as_str()), bytes).unwrap();
    let captured = store.capture_patch(state.generation).unwrap();
    assert_eq!(captured.sequence, expected_sequence);
    assert_eq!(captured.identity, *store.identity());
    let mut retained = Vec::new();
    store
        .write_captured_file(captured.sequence, &path, &mut retained)
        .unwrap();
    assert_eq!(retained, bytes);
    format::access(&store, SessionMode::Shared)
        .unwrap()
        .run(|access| {
            f.source_admission
                .require_same_import(&admitted_ordinary::inspect(
                    &access.connection,
                    &store.marker,
                    None,
                )?)?;
            Ok(())
        })
        .unwrap();
    drop(store);
    require_source_import(f);
}

struct Close {
    pins: [PathBuf; 2],
    counts: (usize, usize, usize, usize),
    withhold: bool,
    after_close: Rc<Cell<bool>>,
}
impl Close {
    fn new(f: &Fixture, withhold: bool) -> Self {
        Self {
            pins: f.pins.clone(),
            counts: (0, 0, 0, 0),
            withhold,
            after_close: Rc::new(Cell::new(false)),
        }
    }
    fn require_pins(&self) {
        // Identity-only sharing probes under catalog/SQL scopes; no full hash
        // or namespace scan is performed from a close callback.
        for path in &self.pins {
            let error = OpenOptions::new()
                .write(true)
                .share_mode(7)
                .open(path)
                .unwrap_err();
            assert_eq!(
                error.raw_os_error(),
                Some(32),
                "fresh retained immutable pin {path:?}"
            );
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
        assert!(
            success,
            "requires actual successful physical C writer close"
        );
        self.counts.2 += 1;
        self.require_pins();
        self.after_close.set(true);
        if std::mem::take(&mut self.withhold) {
            self.counts.3 += 1;
            Some(invalid(WITHHELD_ACK))
        } else {
            None
        }
    }
}

fn require_terminal(f: &Fixture, digest: &str, cancel: &Cancellation) {
    let terminal = OrdinaryTerminalResolution::open(&f.destination, cancel)
        .unwrap_or_else(|e| panic!("fresh terminal C10 {e}"));
    terminal.require(cancel).unwrap();
    assert_eq!(terminal.admission_digest(), digest);
    assert_eq!(terminal.admitted().private_schema(), 10);
    assert_eq!(terminal.ready(), &f.ready);
    assert_eq!(terminal.receipt(), &f.receipt);
    assert_eq!(
        terminal.admitted().source_published().encode().unwrap(),
        f.published
    );
    drop(terminal);
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_admission_accepts_advanced_published_b_after_all_publication_holders_release()
 {
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_fresh_admission_accepts_advanced_published_b_after_all_publication_holders_release"
        ),
        3600,
        || {
            let f = fresh_fixture();
            let cancel = Cancellation::default();
            let inspection = OrdinaryReadyInspection::open(&f.destination, &cancel).unwrap();
            inspection.require(&cancel).unwrap();
            assert_eq!(inspection.ready(), &f.ready);
            assert_eq!(inspection.receipt(), &f.receipt);
            let c_lock = f.destination_database.parent().unwrap().join("layrs.lock");
            let b_lock = f.source_database.parent().unwrap().join("layrs.lock");
            require_owner_contention(&c_lock, SessionMode::Exclusive);
            drop(inspection); // No shared-to-exclusive upgrade, nor retained proof reuse.
            assert_eq!(snapshot(&f.destination_database), f.destination_rows);
            assert_eq!(snapshot(&f.family_database), f.family_rows);

            // Fresh failure is filesystem-real: no B pins remain from setup. This
            // read handle denies all sharing without changing a byte or moving B.
            let denied = OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&f.source_database)
                .unwrap();
            assert!(OrdinaryAdmissionSource::open(&f.source, &cancel).is_err());
            assert_eq!(snapshot(&f.destination_database), f.destination_rows);
            assert_eq!(snapshot(&f.family_database), f.family_rows);
            drop(denied);
            assert_eq!(snapshot(&f.source_database), f.source_rows);

            capture_source_suffix(&f, SUFFIX, f.source_capture_count + 1);
            let source_rows = snapshot(&f.source_database);
            let family_rows = snapshot(&f.family_database);
            let source = OrdinaryAdmissionSource::open(&f.source, &cancel).unwrap();
            require_owner_contention(&b_lock, SessionMode::Exclusive);
            let held = OrdinaryFreshAdmission::open(&source, &f.destination, &cancel).unwrap();
            require_owner_contention(&c_lock, SessionMode::Shared);
            let mut close = Close::new(&f, false);
            let after_close = close.after_close.clone();
            let source_path = f.source_database.clone();
            let destination_path = f.destination_database.clone();
            let family_path = f.family_database.clone();
            let readback = Rc::new(RefCell::new((0usize, 0usize, 0usize)));
            let observed_readback = readback.clone();
            let digest = with_observer(
                move |path, point| {
                    if after_close.get() && point == ReadBoundary::BeforeSnapshot {
                        let mut reads = observed_readback.borrow_mut();
                        if path == source_path.as_path() {
                            reads.0 += 1;
                        } else if path == destination_path.as_path() {
                            reads.1 += 1;
                        } else if path == family_path.as_path() {
                            reads.2 += 1;
                        } else {
                            panic!("unexpected post-close readback path {path:?}");
                        }
                    }
                    Ok(())
                },
                || held.commit(&mut close, &cancel),
            )
            .unwrap_or_else(|e| panic!("fresh admission {:?}: {}", e.step, e.cause));
            assert_eq!(close.counts, (1, 1, 1, 0));
            assert_eq!(
                *readback.borrow(),
                (0, 1, 1),
                "fresh C/family readback follows positive closes"
            );
            assert_eq!(snapshot(&f.source_database), source_rows);
            assert_eq!(snapshot(&f.family_database), family_rows);
            require_source_import(&f);
            drop(source); // Shared B ownership ends before the next mutable session.
            capture_source_suffix(&f, LATER_SUFFIX, f.source_capture_count + 2);
            require_terminal(&f, &digest, &cancel);
            require_source_import(&f);
        },
    );
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_admission_withheld_c_ack_resolves_without_b_reads_or_identity_checks() {
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_fresh_admission_withheld_c_ack_resolves_without_b_reads_or_identity_checks"
        ),
        3600,
        || {
            let f = fresh_fixture(); // A separate fresh physical fixture from recipe 1.
            let cancel = Cancellation::default();
            let source = OrdinaryAdmissionSource::open(&f.source, &cancel).unwrap();
            let held = OrdinaryFreshAdmission::open(&source, &f.destination, &cancel).unwrap();
            let mut close = Close::new(&f, true);
            let after_close = close.after_close.clone();
            let after_close_boundaries = Rc::new(Cell::new(0usize));
            let observed_after_close = after_close_boundaries.clone();
            let boundaries = Rc::new(RefCell::new(Vec::new()));
            let observed = boundaries.clone();
            let result = with_observer(
                move |path, point| {
                    observed.borrow_mut().push((path.to_owned(), point));
                    if after_close.get() {
                        observed_after_close.set(observed_after_close.get() + 1);
                        return Err(invalid(
                            "no readback or B identity check after withheld close ACK",
                        ));
                    }
                    Ok(())
                },
                || held.commit(&mut close, &cancel),
            );
            let OrdinaryFreshAdmissionFailure { step, cause, held } = result
                .err()
                .expect("successful C physical close with withheld ACK must retain the holder");
            assert_eq!(step, AdmissionStep::CommitReturned);
            assert!(cause.to_string().contains(WITHHELD_ACK), "{cause}");
            assert_eq!(close.counts, (1, 1, 1, 1));
            assert_eq!(
                after_close_boundaries.get(),
                0,
                "no implicit read/identity boundary after failed ACK"
            );
            // Positive controls establish that the scoped observer saw the actual
            // B read and source identity route, not merely an unused callback.
            for (path, point) in [
                (&f.source_database, ReadBoundary::BeforeSnapshot),
                (&f.source_database, ReadBoundary::DatabaseSnapshot),
                (&f.source_database, ReadBoundary::DatabaseIdentity),
                (&f.source_database, ReadBoundary::OrdinarySourceIdentity),
                (&f.destination_database, ReadBoundary::BeforeSnapshot),
                (&f.destination_database, ReadBoundary::DatabaseSnapshot),
                (&f.destination_database, ReadBoundary::DatabaseIdentity),
                (&f.family_database, ReadBoundary::BeforeSnapshot),
                (&f.family_database, ReadBoundary::DatabaseSnapshot),
                (&f.family_database, ReadBoundary::DatabaseIdentity),
            ] {
                assert!(
                    boundaries
                        .borrow()
                        .iter()
                        .any(|(actual, actual_point)| actual == path && *actual_point == point)
                );
            }
            // The external caller's deliberate observation is separate from the
            // failed commit. C has durable C10, with B/family exact and unchanged.
            let destination_marker = format::read_marker(&f.destination).unwrap();
            let (committed_rows, digest) = witness::read(&f.destination_database, |c| {
                let admitted = read_admitted_import(c, &destination_marker, None).unwrap();
                assert_eq!(admitted.private_schema(), 10);
                assert_eq!(admitted.source_published().encode().unwrap(), f.published);
                (witness::database(c), admitted.record().digest().unwrap())
            });
            assert_eq!(snapshot(&f.source_database), f.source_rows);
            assert_eq!(snapshot(&f.family_database), f.family_rows);

            // Retained DatabasePin read handles normally forbid this guard. Do not
            // force it, release the capability or pretend B is physically absent.
            let denied = match OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&f.source_database)
            {
                Ok(file) => Some(file),
                Err(error) => {
                    assert_eq!(
                        error.raw_os_error(),
                        Some(32),
                        "retained source DB pin sharing conflict"
                    );
                    eprintln!(
                        "FRESH_ORDINARY_ACK_B_DENY_SHARE=blocked-by-retained-source-pins; resolve proof is instrumented"
                    );
                    None
                }
            };
            let source_path = f.source_database.clone();
            let destination_path = f.destination_database.clone();
            let family_path = f.family_database.clone();
            let calls = Rc::new(RefCell::new((0usize, 0usize, 0usize)));
            let observed_calls = calls.clone();
            let resolved = with_observer(
                move |path, point| {
                    let mut calls = observed_calls.borrow_mut();
                    if path == source_path.as_path() {
                        calls.0 += 1;
                        return Err(invalid(
                            "explicit resolution must not read or verify B identity",
                        ));
                    }
                    assert!(matches!(
                        point,
                        ReadBoundary::BeforeSnapshot
                            | ReadBoundary::DatabaseSnapshot
                            | ReadBoundary::DatabaseIdentity
                    ));
                    assert!(
                        path == destination_path.as_path() || path == family_path.as_path(),
                        "unexpected resolution read path {path:?}"
                    );
                    if point == ReadBoundary::BeforeSnapshot {
                        if path == destination_path.as_path() {
                            calls.1 += 1;
                        } else {
                            calls.2 += 1;
                        }
                    }
                    Ok(())
                },
                || held.resolve(&cancel),
            )
            .unwrap_or_else(|e| panic!("explicit B-free resolution {:?}: {}", e.step, e.cause));
            assert_eq!(resolved, digest);
            assert_eq!(
                *calls.borrow(),
                (0, 2, 2),
                "only two fresh C/family scopes, no B boundary"
            );
            assert_eq!(
                close.counts,
                (1, 1, 1, 1),
                "resolution never opens another writer"
            );
            assert_eq!(snapshot(&f.destination_database), committed_rows);
            drop(denied);
            drop(source); // Only now release B's independent fresh owner and pins.
            require_terminal(&f, &digest, &cancel);
            assert_eq!(snapshot(&f.source_database), f.source_rows);
            assert_eq!(snapshot(&f.family_database), f.family_rows);
            require_source_import(&f);
        },
    );
}
