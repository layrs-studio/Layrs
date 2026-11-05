//! Proposed genuine fresh B Staging -> Ready after clean setup teardown.
//! Fresh B selected-image pins and C checkout/CAS pins must span the B writer
//! close. ACK withholding follows a successful physical close; it is neither
//! an OS close failure nor crash/rollback/owner-loss recovery qualification.
//! Missing/alias/protected paths, exact-manifest substitution and changed
//! plan/history/selection/receipt/family negatives remain native-route gaps.
use super::{
    managed_ordinary_family_ready_tests::family,
    managed_ordinary_installing_tests::{self as witness, Database, Tree},
    managed_ordinary_ready_tests::CURRENT,
    managed_ordinary_source_publication_tests::{
        ingress, only_source_database_changed, only_source_rows_changed, source,
        with_source_staging_data, PublicationFixture,
    },
};
use crate::{
    error::StoreError,
    lock::StoreLock,
    projection::{
        format,
        provider::{
            database_io::{
                tests::{
                    fresh_ordinary_support::{with_observer, ReadBoundary},
                    writer_support::{self, AdmissionPoint},
                },
                WriterCloseObserver, WriterOpenPoint,
            },
            managed_admission::read_admitted_import,
            managed_export::{
                OrdinaryManagedExport, OrdinaryStagingPromotion, OrdinaryStagingPromotionFailure,
            },
            managed_ready_reopen::{AdmissionStep, OrdinaryReadyPublication},
            process_profile::tests::child,
            records::{
                import::ProviderTransitionReceiptRecord,
                transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
                Record,
            },
            recovery::error::{invalid, ProviderBootstrapCause as Cause, Result as CauseResult},
            transition_sql::SourceIntentEffect,
        },
        SessionMode,
    },
};
use layrs_vfs::Cancellation;
use rusqlite::ffi;
use std::{
    cell::{Cell, RefCell},
    fs::{self, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    rc::Rc,
};

const DRIFT: &[u8] = b"real B image drift must refuse fresh Staging promotion";
const WITHHELD_ACK: &str = "fresh Staging Ready successful B close acknowledgement withheld";

fn snapshot(path: &Path) -> Database {
    witness::read(path, witness::database)
}

/// Paths and canonical immutable DATA only. No old selected image, allocation,
/// checkout, CAS, SQL scope or owner can escape through the setup callback.
struct Fixture {
    source: PathBuf,
    destination: PathBuf,
    family: PathBuf,
    source_database: PathBuf,
    destination_database: PathBuf,
    family_database: PathBuf,
    source_marker: format::Marker,
    original_ingress: Vec<Vec<u8>>,
    staging: ProviderTransitionIntentRecord,
    ready: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
    source_rows: Database,
    destination_rows: Database,
    family_rows: Database,
    pins: [PathBuf; 3],
}

fn close_at_family_ready(
    owner: OrdinaryManagedExport<'_>,
    f: &PublicationFixture<'_>,
    _: &Cancellation,
) -> Fixture {
    let (staging, original_ingress) = witness::read(f.source_database, |c| {
        let admitted = read_admitted_import(c, f.source_marker, None).unwrap();
        let actual = admitted
            .outgoing()
            .iter()
            .find(|row| row.transaction_id == f.ready.transaction_id)
            .unwrap();
        assert_eq!(
            actual.state,
            TransitionState::Staging,
            "setup must never call SourceReady"
        );
        assert!(actual.receipt_id.is_none());
        assert!(admitted
            .outgoing_receipt(&f.receipt.digest().unwrap())
            .is_err());
        (
            ProviderTransitionIntentRecord::decode(&actual.encode().unwrap()).unwrap(),
            ingress(c, f.source_marker),
        )
    });
    let source_rows = source(
        f.source_database,
        f.source_marker,
        &staging,
        None,
        &original_ingress,
    );
    let destination_rows = witness::read(f.destination_database, |c| {
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            8
        );
        let payload: Vec<u8> = c
            .query_row(
                "SELECT payload FROM projection_provider_transitions WHERE id=?1",
                [&f.ready.transaction_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            ProviderTransitionIntentRecord::decode(&payload).unwrap(),
            *f.ready
        );
        witness::database(c)
    });
    let family_rows = family(f.family_database, &staging, f.required, "ready");
    // Calculation DATA only: the actual B observation above is still Staging.
    let mut ready = ProviderTransitionIntentRecord::decode(&f.ready.encode().unwrap()).unwrap();
    ready.role = TransitionRole::Source;
    ready.local_generation = staging.local_generation;
    ready.require_transition(&staging).unwrap();
    ready.require_receipt(f.receipt).unwrap();
    let file = &f.expected.files()[0];
    let manifest = f
        .required
        .iter()
        .find(|m| m.id() == &file.manifest_id)
        .unwrap();
    let pins = [
        f.source_root.join(file.path.as_str()),
        f.destination_root.join(file.path.as_str()),
        witness::object(
            f.family_root,
            "chunks",
            manifest.chunks()[0].chunk_id().as_str(),
        ),
    ];
    assert_eq!(fs::read(&pins[0]).unwrap(), CURRENT);
    // A successful explicit original B owner close is required before the
    // helper may drop all C proofs/allocation/store and return these DATA.
    owner.close().unwrap();
    assert_eq!(
        source(
            f.source_database,
            f.source_marker,
            &staging,
            None,
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
        staging,
        ready,
        receipt: ProviderTransitionReceiptRecord::decode(&f.receipt.encode().unwrap()).unwrap(),
        source_rows,
        destination_rows,
        family_rows,
        pins,
    }
}

fn require_released_pins(f: &Fixture) {
    for path in &f.pins {
        drop(
            OpenOptions::new()
                .write(true)
                .share_mode(7)
                .open(path)
                .expect("all former B image and C checkout/CAS pins must be released"),
        );
    }
}

fn fresh_staging_fixture() -> Fixture {
    let f = with_source_staging_data(close_at_family_ready);
    require_released_pins(&f);
    assert_eq!(staging_rows(&f), f.source_rows);
    f
}

fn staging_rows(f: &Fixture) -> Database {
    source(
        &f.source_database,
        &f.source_marker,
        &f.staging,
        None,
        &f.original_ingress,
    )
}

fn ready_rows(f: &Fixture) -> Database {
    source(
        &f.source_database,
        &f.source_marker,
        &f.ready,
        Some(&f.receipt),
        &f.original_ingress,
    )
}

fn require_unchanged_destination(f: &Fixture) {
    assert_eq!(snapshot(&f.destination_database), f.destination_rows);
    assert_eq!(snapshot(&f.family_database), f.family_rows);
}

fn require_only_ready(f: &Fixture) -> Database {
    let rows = ready_rows(f);
    only_source_rows_changed(&f.source_rows, &rows, &f.staging, &f.ready, &f.receipt);
    require_unchanged_destination(f);
    rows
}

fn trees(f: &Fixture) -> (Tree, Tree, Tree) {
    let b_lock = f.source_database.parent().unwrap().join("layrs.lock");
    let c_lock = f.destination_database.parent().unwrap().join("layrs.lock");
    witness::trees(&f.source, &f.family, &f.destination, [&b_lock, &c_lock])
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

/// Scope the existing thread-local clean-writer observation; no new global
/// state or SQLite/VFS registration is introduced by this recipe.
fn with_writer_probe<T>(
    observer: impl FnMut(&Path, AdmissionPoint) -> CauseResult<()> + 'static,
    action: impl FnOnce() -> T,
) -> T {
    writer_support::observe(observer);
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            writer_support::clear_observer();
        }
    }
    let _reset = Reset;
    action()
}

struct Close {
    pins: [PathBuf; 3],
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
        // Three independent positive controls: genuine current B bytes, C
        // checkout bytes and shared CAS. No hashing/scan beneath SQL/catalog.
        for path in &self.pins {
            let error = OpenOptions::new()
                .write(true)
                .share_mode(7)
                .open(path)
                .unwrap_err();
            assert_eq!(
                error.raw_os_error(),
                Some(32),
                "fresh retained B/C immutable byte pin {path:?}"
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
            "requires actual successful physical B writer close"
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
        "observer must see genuine B identity route"
    );
}

fn require_fresh_ready_handoff(f: &Fixture, expected: &Database, cancel: &Cancellation) {
    // This distinct fresh adapter authenticates actual B Ready and physical
    // C/family Ready after promotion holders have gone. Do not call publish:
    // promotion and a later Published transaction are separate caller actions.
    let handoff = OrdinaryReadyPublication::open(&f.source, &f.destination, cancel)
        .unwrap_or_else(|e| panic!("fresh actual-Ready handoff {e}"));
    drop(handoff);
    assert_eq!(
        &ready_rows(f),
        expected,
        "handoff inspection never auto-publishes B"
    );
    require_unchanged_destination(f);
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_staging_refuses_changed_b_image_then_promotes_exact_ready_after_all_holders_release(
) {
    child::run(concat!(module_path!(), "::managed_ordinary_fresh_staging_refuses_changed_b_image_then_promotes_exact_ready_after_all_holders_release"), 3600, || {
        let f = fresh_staging_fixture();
        eprintln!("STAGING_RECIPE_PHASE=setup-released");
        let cancel = Cancellation::default();
        let before_trees = trees(&f);
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(OrdinaryStagingPromotion::open(&f.source, &f.destination, &f.staging.transaction_id, &cancelled).is_err());
        assert_eq!(staging_rows(&f), f.source_rows);
        require_unchanged_destination(&f);

        fs::write(&f.pins[0], DRIFT).unwrap();
        eprintln!("STAGING_RECIPE_PHASE=drift-open");
        let writer_attempts = Rc::new(Cell::new(0usize));
        let observed_attempts = writer_attempts.clone();
        let preflights = Rc::new(RefCell::new(Vec::new()));
        let observed_preflights = preflights.clone();
        let refused = with_writer_probe(move |path, point| {
            observed_preflights.borrow_mut().push((path.to_owned(), point));
            if point == AdmissionPoint::BeforeWritableOpen {
                observed_attempts.set(observed_attempts.get() + 1);
                return Err(invalid("changed B image must refuse before any writable open"));
            }
            Ok(())
        }, || OrdinaryStagingPromotion::open(&f.source, &f.destination, &f.staging.transaction_id, &cancel))
            .err().expect("real changed B bytes must not promote using C's copied image");
        assert!(refused.to_string().contains("image"), "{refused}");
        assert_eq!(writer_attempts.get(), 0);
        assert!(preflights.borrow().iter().any(|(path, point)| path == &f.source_database
            && *point == AdmissionPoint::ReadonlyClosed), "actual B preflight closed before image refusal");
        assert_eq!(staging_rows(&f), f.source_rows);
        require_unchanged_destination(&f);
        // Restore only exact original bytes at this fresh fixture's real leaf.
        // No SQL evidence, grants, manifests, plan or identity is repaired.
        fs::write(&f.pins[0], CURRENT).unwrap();
        assert_eq!(fs::read(&f.pins[0]).unwrap(), CURRENT);
        assert_eq!(trees(&f), before_trees);

        eprintln!("STAGING_RECIPE_PHASE=exact-open");
        let held = OrdinaryStagingPromotion::open(&f.source, &f.destination, &f.staging.transaction_id, &cancel)
            .unwrap_or_else(|e| panic!("unchanged authentic fresh Staging {e}"));
        let b_lock = f.source_database.parent().unwrap().join("layrs.lock");
        let c_lock = f.destination_database.parent().unwrap().join("layrs.lock");
        require_owner_contention(&b_lock, SessionMode::Shared);
        require_owner_contention(&c_lock, SessionMode::Exclusive);
        let mut close = Close::new(&f, false);
        close.require_pins();
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        let after_close = close.after_close.clone();
        let post_close_reads = Rc::new(RefCell::new(Vec::new()));
        let observed_post_close = post_close_reads.clone();
        eprintln!("STAGING_RECIPE_PHASE=promote");
        let progress = with_observer(move |path, point| {
            observed.borrow_mut().push((path.to_owned(), point));
            if after_close.get() && point == ReadBoundary::BeforeSnapshot {
                observed_post_close.borrow_mut().push(path.to_owned());
            }
            Ok(())
        }, || held.promote_ready(&mut close, &cancel))
            .unwrap_or_else(|e| panic!("fresh Staging promotion {:?}: {}", e.step, e.cause));
        assert_eq!(progress.effect(), SourceIntentEffect::Updated);
        assert_eq!(progress.ready(), &f.ready);
        assert_eq!(progress.receipt(), &f.receipt);
        assert_eq!(progress.ready_digest(), f.ready.digest().unwrap());
        assert_eq!(close.counts, (1, 1, 1, 0));
        require_read_controls(&events.borrow(), &f);
        for path in [&f.source_database, &f.destination_database, &f.family_database] {
            assert!(post_close_reads.borrow().contains(path),
                "fresh actual {path:?} readback must follow positive B writer close");
        }
        drop(progress);
        let ready = require_only_ready(&f);
        let ready_trees = trees(&f);
        only_source_database_changed(&before_trees, &ready_trees, &f.source_database);
        require_released_pins(&f);
        require_fresh_ready_handoff(&f, &ready, &cancel);
    });
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_staging_withheld_b_close_ack_reopens_actual_ready_after_explicit_holder_release(
) {
    child::run(concat!(module_path!(), "::managed_ordinary_fresh_staging_withheld_b_close_ack_reopens_actual_ready_after_explicit_holder_release"), 3600, || {
        let f = fresh_staging_fixture(); // Fresh independent parent/setup from recipe 1.
        let cancel = Cancellation::default();
        let before_trees = trees(&f);
        let held = OrdinaryStagingPromotion::open(&f.source, &f.destination, &f.staging.transaction_id, &cancel)
            .unwrap_or_else(|e| panic!("authentic fresh Staging {e}"));
        let mut close = Close::new(&f, true);
        close.require_pins();
        let after_close = close.after_close.clone();
        let after_close_boundaries = Rc::new(Cell::new(0usize));
        let observed_after_close = after_close_boundaries.clone();
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        let result = with_observer(move |path, point| {
            observed.borrow_mut().push((path.to_owned(), point));
            if after_close.get() {
                observed_after_close.set(observed_after_close.get() + 1);
                return Err(invalid("failed Staging close ACK forbids later SQL/identity/snapshot probes"));
            }
            Ok(())
        }, || held.promote_ready(&mut close, &cancel));
        let OrdinaryStagingPromotionFailure { step, cause, held } = result.err()
            .expect("withheld positive B close ACK must retain its genuine image/C authority");
        assert_eq!(step, AdmissionStep::CommitReturned);
        let primary = cause.to_string();
        assert!(primary.contains(WITHHELD_ACK), "{primary}");
        assert_eq!(close.counts, (1, 1, 1, 1));
        assert_eq!(after_close_boundaries.get(), 0, "only mandatory close cleanup follows failed ACK");
        require_read_controls(&events.borrow(), &f);
        close.require_pins(); // B's true current bytes and C proofs remain held.
        // Separate deliberate caller observations; no implicit success readback
        // or rollback acknowledgement is inferred from the original failure.
        let ready = require_only_ready(&f);
        let ready_trees = trees(&f);
        only_source_database_changed(&before_trees, &ready_trees, &f.source_database);
        drop(held); // No in-holder Staging resolve/replay is exposed by this unit.
        require_released_pins(&f);
        require_fresh_ready_handoff(&f, &ready, &cancel);
        assert_eq!(cause.to_string(), primary, "fresh inspection preserves the original uncertain ACK error");
        assert_eq!(close.counts, (1, 1, 1, 1), "fresh Ready inspection adds no promotion writer");
    });
}
