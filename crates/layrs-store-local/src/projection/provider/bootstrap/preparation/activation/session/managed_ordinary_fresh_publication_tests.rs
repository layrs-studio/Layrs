//! Proposed clean fresh-owner publication from genuine SourceReady B/C8.
//! Every setup holder is released after positive original B close. An ACK
//! withheld after actual successful B close is not a native xClose failure,
//! process crash or general owner-loss recovery. Resolution still requires B.
//! Fresh Staging and mismatched-receipt refusals remain unqualified here.
use super::{
    managed_ordinary_installing_tests::{self as witness, Database, Tree},
    managed_ordinary_source_publication_tests::{
        PublicationFixture, ingress, only_source_database_changed, only_source_rows_changed,
        source, with_source_ready_data,
    },
};
use crate::{
    error::StoreError,
    lock::StoreLock,
    projection::{
        ProjectionStore, SessionMode, format,
        provider::{
            database_io::{
                WriterCloseObserver, WriterOpenPoint,
                tests::fresh_ordinary_support::{ReadBoundary, with_observer},
            },
            managed_admission::read_admitted_import,
            managed_export::OrdinaryManagedExport,
            managed_ready_reopen::{
                AdmissionStep, OrdinaryReadyPublication, OrdinaryReadyPublicationEffect,
                OrdinaryReadyPublicationFailure,
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
use layrs_vfs::Cancellation;
use rusqlite::ffi;
use std::{
    cell::{Cell, RefCell},
    fs::OpenOptions,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    rc::Rc,
};

const WITHHELD_ACK: &str = "fresh ordinary Published successful B close acknowledgement withheld";
const B_READ_REFUSED: &str = "explicit publication resolution requires a fresh actual B read";

fn snapshot(path: &Path) -> Database {
    witness::read(path, witness::database)
}

/// Only owned immutable DATA and paths escape setup. In particular a copied
/// Ready record is not an owner, physical C proof or authority to publish B.
struct Fixture {
    source: PathBuf,
    destination: PathBuf,
    family: PathBuf,
    source_database: PathBuf,
    destination_database: PathBuf,
    family_database: PathBuf,
    source_marker: format::Marker,
    original_ingress: Vec<Vec<u8>>,
    source_ready: ProviderTransitionIntentRecord,
    published: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
    source_rows: Database,
    destination_rows: Database,
    family_rows: Database,
    pins: [PathBuf; 2],
}

fn close_at_ready(
    owner: OrdinaryManagedExport<'_>,
    f: &PublicationFixture<'_>,
    _: &Cancellation,
) -> Fixture {
    let (source_ready, original_ingress) = witness::read(f.source_database, |c| {
        let admitted = read_admitted_import(c, f.source_marker, None).unwrap();
        let actual = admitted
            .outgoing()
            .iter()
            .find(|row| row.transaction_id == f.ready.transaction_id)
            .unwrap();
        assert_eq!(
            actual.state,
            TransitionState::Ready,
            "setup must stop before Published"
        );
        assert_eq!(
            admitted
                .outgoing_receipt(&f.receipt.digest().unwrap())
                .unwrap(),
            f.receipt
        );
        (
            ProviderTransitionIntentRecord::decode(&actual.encode().unwrap()).unwrap(),
            ingress(c, f.source_marker),
        )
    });
    let source_rows = source(
        f.source_database,
        f.source_marker,
        &source_ready,
        Some(f.receipt),
        &original_ingress,
    );
    let destination_rows = snapshot(f.destination_database);
    let family_rows = snapshot(f.family_database);
    let mut published =
        ProviderTransitionIntentRecord::decode(&source_ready.encode().unwrap()).unwrap();
    published.state = TransitionState::Published;
    published.require_transition(&source_ready).unwrap();
    published.require_receipt(f.receipt).unwrap();
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
    // Original B access positively closes while B is truly Ready. Do not use
    // an ordinary B opener here: Ready must continue fencing ordinary work.
    owner.close().unwrap();
    assert_eq!(
        source(
            f.source_database,
            f.source_marker,
            &source_ready,
            Some(f.receipt),
            &original_ingress
        ),
        source_rows
    );
    assert_eq!(snapshot(f.destination_database), destination_rows);
    assert_eq!(snapshot(f.family_database), family_rows);
    Fixture {
        source: f.source_root.to_owned(),
        destination: f.destination_root.to_owned(),
        family: f.family_root.to_owned(),
        source_database: f.source_database.to_owned(),
        destination_database: f.destination_database.to_owned(),
        family_database: f.family_database.to_owned(),
        source_marker: f.source_marker.clone(),
        original_ingress,
        source_ready,
        published,
        receipt: ProviderTransitionReceiptRecord::decode(&f.receipt.encode().unwrap()).unwrap(),
        source_rows,
        destination_rows,
        family_rows,
        pins,
    }
}

fn fresh_ready_fixture() -> Fixture {
    let f = with_source_ready_data(close_at_ready);
    // Only after callback close and helper teardown may fresh B/C capabilities
    // be acquired. These sharing probes retain no handle and change no bytes.
    for path in &f.pins {
        drop(
            OpenOptions::new()
                .write(true)
                .share_mode(7)
                .open(path)
                .expect("all original C checkout/CAS publication pins must be released"),
        );
    }
    assert_eq!(ready_rows(&f), f.source_rows);
    f
}

fn ready_rows(f: &Fixture) -> Database {
    source(
        &f.source_database,
        &f.source_marker,
        &f.source_ready,
        Some(&f.receipt),
        &f.original_ingress,
    )
}

fn published_rows(f: &Fixture) -> Database {
    source(
        &f.source_database,
        &f.source_marker,
        &f.published,
        Some(&f.receipt),
        &f.original_ingress,
    )
}

fn trees(f: &Fixture) -> (Tree, Tree, Tree) {
    let b_lock = f.source_database.parent().unwrap().join("layrs.lock");
    let c_lock = f.destination_database.parent().unwrap().join("layrs.lock");
    witness::trees(&f.source, &f.family, &f.destination, [&b_lock, &c_lock])
}

fn require_unchanged_destination(f: &Fixture) {
    assert_eq!(snapshot(&f.destination_database), f.destination_rows);
    assert_eq!(snapshot(&f.family_database), f.family_rows);
}

fn require_only_published(f: &Fixture) -> Database {
    let rows = published_rows(f);
    // This exact comparison allows only the existing Ready transition's
    // state/payload/digest to change. No receipt or frontier row is added.
    only_source_rows_changed(
        &f.source_rows,
        &rows,
        &f.source_ready,
        &f.published,
        &f.receipt,
    );
    require_unchanged_destination(f);
    rows
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
        // Sharing/identity only inside SQL/catalog scopes, never byte hashing
        // or a checkout scan from a writer observer.
        for path in &self.pins {
            let error = OpenOptions::new()
                .write(true)
                .share_mode(7)
                .open(path)
                .unwrap_err();
            assert_eq!(
                error.raw_os_error(),
                Some(32),
                "fresh immutable C pin {path:?}"
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
        assert!(success, "requires a real successful B writer close");
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

fn require_read_controls(events: &[(PathBuf, ReadBoundary)], f: &Fixture) {
    for path in [
        &f.source_database,
        &f.destination_database,
        &f.family_database,
    ] {
        for boundary in [
            ReadBoundary::BeforeSnapshot,
            ReadBoundary::DatabaseSnapshot,
            ReadBoundary::DatabaseIdentity,
        ] {
            assert!(
                events
                    .iter()
                    .any(|(actual, point)| actual == path && *point == boundary),
                "observer must see genuine {boundary:?} route {path:?}"
            );
        }
    }
    assert!(
        events.iter().any(|(path, point)| path == &f.source_database
            && *point == ReadBoundary::OrdinarySourceIdentity),
        "observer must see B physical identity route"
    );
}

fn read_counts(events: &[(PathBuf, ReadBoundary)], f: &Fixture) -> (usize, usize, usize) {
    let count = |path: &Path| {
        events
            .iter()
            .filter(|(actual, point)| actual == path && *point == ReadBoundary::BeforeSnapshot)
            .count()
    };
    (
        count(&f.source_database),
        count(&f.destination_database),
        count(&f.family_database),
    )
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_publication_advances_released_ready_and_replays_published_without_writer()
{
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_fresh_publication_advances_released_ready_and_replays_published_without_writer"
        ),
        3600,
        || {
            let f = fresh_ready_fixture();
            let cancel = Cancellation::default();
            let before_trees = trees(&f);
            // An authentic family-unavailable refusal; no catalog mutation,
            // detached receipt or repaired schema can bless the attempt.
            let denied = OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&f.family_database)
                .unwrap();
            assert!(OrdinaryReadyPublication::open(&f.source, &f.destination, &cancel).is_err());
            drop(denied);
            assert_eq!(ready_rows(&f), f.source_rows);
            require_unchanged_destination(&f);

            let held = OrdinaryReadyPublication::open(&f.source, &f.destination, &cancel).unwrap();
            let b_lock = f.source_database.parent().unwrap().join("layrs.lock");
            let c_lock = f.destination_database.parent().unwrap().join("layrs.lock");
            require_owner_contention(&b_lock, SessionMode::Shared);
            require_owner_contention(&c_lock, SessionMode::Exclusive);
            // A still-Ready source cannot be resolved as if it were Published.
            let failure = held
                .resolve(&cancel)
                .err()
                .expect("actual Ready must refuse Published resolution");
            assert_eq!(failure.step, AdmissionStep::Validating);
            assert!(
                failure
                    .cause
                    .to_string()
                    .contains("requires actual Published")
            );
            assert_eq!(ready_rows(&f), f.source_rows);
            require_unchanged_destination(&f);
            let mut close = Close::new(&f, false);
            let after_close = close.after_close.clone();
            let source_path = f.source_database.clone();
            let destination_path = f.destination_database.clone();
            let family_path = f.family_database.clone();
            let readback = Rc::new(RefCell::new((0usize, 0usize, 0usize)));
            let observed_readback = readback.clone();
            let events = Rc::new(RefCell::new(Vec::new()));
            let observed = events.clone();
            let progress = with_observer(
                move |path, point| {
                    observed.borrow_mut().push((path.to_owned(), point));
                    if after_close.get() && point == ReadBoundary::BeforeSnapshot {
                        let mut reads = observed_readback.borrow_mut();
                        if path == source_path.as_path() {
                            reads.0 += 1;
                        } else if path == destination_path.as_path() {
                            reads.1 += 1;
                        } else if path == family_path.as_path() {
                            reads.2 += 1;
                        } else {
                            panic!("unexpected post-close publication readback {path:?}");
                        }
                    }
                    Ok(())
                },
                || failure.held.publish(&mut close, &cancel),
            )
            .unwrap_or_else(|e| panic!("fresh publication {:?}: {}", e.step, e.cause));
            assert_eq!(progress.effect(), OrdinaryReadyPublicationEffect::Updated);
            assert_eq!(progress.published(), &f.published);
            assert_eq!(progress.receipt(), &f.receipt);
            let digest = progress.published_digest().to_owned();
            assert_eq!(digest, f.published.digest().unwrap());
            assert_eq!(close.counts, (1, 1, 1, 0));
            assert_eq!(
                *readback.borrow(),
                (1, 1, 1),
                "fresh B/C/family readback follows positive writer close"
            );
            require_read_controls(&events.borrow(), &f);
            drop(progress);
            let published = require_only_published(&f);
            let after_trees = trees(&f);
            only_source_database_changed(&before_trees, &after_trees, &f.source_database);

            let replay =
                OrdinaryReadyPublication::open(&f.source, &f.destination, &cancel).unwrap();
            let mut replay_close = Close::new(&f, false);
            let progress = replay
                .publish(&mut replay_close, &cancel)
                .unwrap_or_else(|e| panic!("fresh Published replay {:?}: {}", e.step, e.cause));
            assert_eq!(
                progress.effect(),
                OrdinaryReadyPublicationEffect::AlreadyPublished
            );
            assert_eq!(progress.published_digest(), digest);
            assert_eq!(progress.published(), &f.published);
            assert_eq!(progress.receipt(), &f.receipt);
            assert_eq!(
                replay_close.counts,
                (0, 0, 0, 0),
                "Published replay never opens a writer"
            );
            drop(progress);
            assert_eq!(published_rows(&f), published);
            require_unchanged_destination(&f);
            assert_eq!(
                trees(&f),
                after_trees,
                "exact replay preserves all checkout/CAS/database evidence"
            );
            let reopened = ProjectionStore::open(&f.source).unwrap();
            reopened.state().unwrap();
            drop(reopened);
            assert_eq!(published_rows(&f), published);
        },
    );
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_publication_withheld_b_close_ack_resolves_actual_published_with_b_without_writer()
 {
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_fresh_publication_withheld_b_close_ack_resolves_actual_published_with_b_without_writer"
        ),
        3600,
        || {
            let f = fresh_ready_fixture(); // Independent fresh fixture, never recipe 1's paths.
            let cancel = Cancellation::default();
            let before_trees = trees(&f);
            let held = OrdinaryReadyPublication::open(&f.source, &f.destination, &cancel).unwrap();
            let mut close = Close::new(&f, true);
            let after_close = close.after_close.clone();
            let after_close_boundaries = Rc::new(Cell::new(0usize));
            let observed_after_close = after_close_boundaries.clone();
            let events = Rc::new(RefCell::new(Vec::new()));
            let observed = events.clone();
            let result = with_observer(
                move |path, point| {
                    observed.borrow_mut().push((path.to_owned(), point));
                    if after_close.get() {
                        observed_after_close.set(observed_after_close.get() + 1);
                        return Err(invalid(
                            "failed B close ACK forbids later SQL/identity/snapshot probes",
                        ));
                    }
                    Ok(())
                },
                || held.publish(&mut close, &cancel),
            );
            let OrdinaryReadyPublicationFailure { step, cause, held } = result
                .err()
                .expect("withheld B close ACK must retain independent fresh authority");
            assert_eq!(step, AdmissionStep::CommitReturned);
            let primary = cause.to_string();
            assert!(primary.contains(WITHHELD_ACK), "{primary}");
            assert_eq!(close.counts, (1, 1, 1, 1));
            assert_eq!(
                after_close_boundaries.get(),
                0,
                "only mandatory close cleanup may follow the failed ACK"
            );
            require_read_controls(&events.borrow(), &f);
            // Explicit caller observation, outside the failed call. The durable
            // actual row is Published despite uncertain enclosing acknowledgement.
            let committed = require_only_published(&f);
            let published_trees = trees(&f);
            only_source_database_changed(&before_trees, &published_trees, &f.source_database);

            let source_path = f.source_database.clone();
            let b_refusals = Rc::new(Cell::new(0usize));
            let observed_refusals = b_refusals.clone();
            let blocked = with_observer(
                move |path, point| {
                    if path == source_path.as_path() && point == ReadBoundary::BeforeSnapshot {
                        observed_refusals.set(observed_refusals.get() + 1);
                        return Err(invalid(B_READ_REFUSED));
                    }
                    Ok(())
                },
                || held.resolve(&cancel),
            )
            .err()
            .expect("resolution really requires current B evidence");
            assert!(blocked.cause.to_string().contains(B_READ_REFUSED));
            assert_eq!(b_refusals.get(), 1);
            assert_eq!(close.counts, (1, 1, 1, 1));
            assert_eq!(published_rows(&f), committed);
            require_unchanged_destination(&f);

            let events = Rc::new(RefCell::new(Vec::new()));
            let observed = events.clone();
            let progress = with_observer(
                move |path, point| {
                    observed.borrow_mut().push((path.to_owned(), point));
                    Ok(())
                },
                || blocked.held.resolve(&cancel),
            )
            .unwrap_or_else(|e| {
                panic!(
                    "explicit actual-Published resolution {:?}: {}",
                    e.step, e.cause
                )
            });
            assert_eq!(
                progress.effect(),
                OrdinaryReadyPublicationEffect::AlreadyPublished
            );
            assert_eq!(progress.published_digest(), f.published.digest().unwrap());
            assert_eq!(progress.published(), &f.published);
            assert_eq!(progress.receipt(), &f.receipt);
            require_read_controls(&events.borrow(), &f);
            assert_eq!(
                read_counts(&events.borrow(), &f),
                (2, 2, 2),
                "resolution reads actual Published twice"
            );
            assert_eq!(
                close.counts,
                (1, 1, 1, 1),
                "explicit resolution has no writer parameter or extra writer"
            );
            assert_eq!(
                cause.to_string(),
                primary,
                "later explicit actions preserve the original ACK error"
            );
            drop(progress);
            assert_eq!(published_rows(&f), committed);
            require_unchanged_destination(&f);
            assert_eq!(
                trees(&f),
                published_trees,
                "resolution preserves durable evidence"
            );
            let reopened = ProjectionStore::open(&f.source).unwrap();
            reopened.state().unwrap();
            drop(reopened);
            assert_eq!(published_rows(&f), committed);
        },
    );
}
