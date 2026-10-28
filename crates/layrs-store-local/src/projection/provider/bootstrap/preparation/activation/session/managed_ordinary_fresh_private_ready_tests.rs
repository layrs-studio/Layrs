//! Fresh private C Ready after genuine complete Staging checkout and clean
//! teardown of every old owner/image/allocation/CAS/checkout holder. Withheld
//! ACK follows SQLite connection close success. Native xClose/OS close, crash
//! and owner-loss outcomes are not qualified by these proposed recipes.
use super::{
    managed_ordinary_family_ready_tests::family,
    managed_ordinary_installing_tests::{self as witness, Database, Node, Tree},
    managed_ordinary_ready_tests::{self as ready_witness, ReadyRows, CURRENT},
    managed_ordinary_source_publication_tests::{
        ingress, source, with_complete_checkout_data, CheckoutFixture,
    },
};
use crate::{
    error::StoreError,
    lock::StoreLock,
    projection::{
        format,
        provider::{
            bootstrap::ManagedPlanData,
            checkout_journal::{self, ExpectedCheckout},
            database_io::{
                tests::{
                    fresh_ordinary_support::{with_observer, ReadBoundary},
                    writer_support::{self, AdmissionPoint},
                },
                WriterCloseObserver, WriterOpenPoint,
            },
            destination_ready::ReadyEffect,
            managed_admission::read_admitted_import,
            managed_export::{
                OrdinaryFamilyReadyPublication, OrdinaryManagedExport,
                OrdinaryPrivateReadyFinalization, OrdinaryPrivateReadyFinalizationFailure,
            },
            managed_ready_reopen::AdmissionStep,
            process_profile::tests::child,
            records::{
                transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
                Record,
            },
            recovery::error::{invalid, ProviderBootstrapCause as Cause, Result as CauseResult},
        },
        SessionMode,
    },
};
use layrs_core::scm::FileManifest;
use layrs_vfs::Cancellation;
use rusqlite::ffi;
use std::{
    cell::{Cell, RefCell},
    fs::{self, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    rc::Rc,
};

const B_DRIFT: &[u8] = b"actual B image drift refuses fresh private Ready";
const C_DRIFT: &[u8] = b"complete SQL journal cannot authenticate changed C checkout bytes";
const WITHHELD_ACK: &str = "fresh private Ready SQLite close acknowledgement withheld";

fn snapshot(path: &Path) -> Database {
    witness::read(path, witness::database)
}

/// Boxed immutable DATA only; neither setup capabilities nor an observed
/// private Ready/receipt can escape the checkout-complete callback.
struct Fixture {
    source: PathBuf,
    destination: PathBuf,
    family: PathBuf,
    source_database: PathBuf,
    destination_database: PathBuf,
    family_database: PathBuf,
    source_marker: format::Marker,
    original_ingress: Vec<Vec<u8>>,
    source_staging: ProviderTransitionIntentRecord,
    destination_staging: ProviderTransitionIntentRecord,
    expected: Box<ExpectedCheckout>,
    derived: Box<ManagedPlanData>,
    selection_payload: Vec<u8>,
    base: FileManifest,
    history: FileManifest,
    current: FileManifest,
    required: Vec<FileManifest>,
    source_rows: Database,
    destination_rows: Database,
    family_rows: Database,
    pins: [PathBuf; 3],
}

fn close_at_complete_checkout(
    owner: OrdinaryManagedExport<'_>,
    f: &CheckoutFixture<'_>,
    _: &Cancellation,
) -> Box<Fixture> {
    let mut phase = child::phase("fresh-private-setup.checkout-witness");
    assert_eq!(f.source_staging.role, TransitionRole::Source);
    assert_eq!(f.source_staging.state, TransitionState::Staging);
    assert!(f.source_staging.receipt_id.is_none());
    assert_eq!(f.destination_staging.role, TransitionRole::Destination);
    assert_eq!(f.destination_staging.state, TransitionState::Staging);
    assert!(f.destination_staging.receipt_id.is_none());
    // The setup really owns these proofs; they are borrowed only during this
    // callback and never returned as fresh capability or fabricated Ready.
    let _setup_proofs = (f.protection, f.checkout);
    let original_ingress = witness::read(f.source_database, |c| {
        let admitted = read_admitted_import(c, f.source_marker, None).unwrap();
        assert_eq!(admitted.outgoing(), std::slice::from_ref(f.source_staging));
        ingress(c, f.source_marker)
    });
    let source_rows = source(
        f.source_database,
        f.source_marker,
        f.source_staging,
        None,
        &original_ingress,
    );
    let destination_rows = witness::destination(f.destination_database, f.source_staging);
    witness::read(f.destination_database, |c| {
        let journal = checkout_journal::read(c, f.expected, None).unwrap();
        assert!(
            journal.is_complete(),
            "setup must finish genuine checkout before teardown"
        );
        let actual: Vec<u8> = c
            .query_row(
                "SELECT payload FROM projection_provider_transitions WHERE id=?1",
                [&f.source_staging.transaction_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            ProviderTransitionIntentRecord::decode(&actual).unwrap(),
            *f.destination_staging
        );
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM projection_provider_transition_receipts",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
            0
        );
    });
    let family_rows = family(
        f.family_database,
        f.source_staging,
        f.required,
        "initializing",
    );
    let file = &f.expected.files()[0];
    assert_eq!(&file.manifest_id, f.current.id());
    let pins = [
        f.source_root.join(file.path.as_str()),
        f.destination_root.join(file.path.as_str()),
        witness::object(
            f.family_root,
            "chunks",
            f.current.chunks()[0].chunk_id().as_str(),
        ),
    ];
    assert_eq!(fs::read(&pins[0]).unwrap(), CURRENT);
    assert_eq!(fs::read(&pins[1]).unwrap(), CURRENT);
    let source_staging =
        ProviderTransitionIntentRecord::decode(&f.source_staging.encode().unwrap()).unwrap();
    let destination_staging =
        ProviderTransitionIntentRecord::decode(&f.destination_staging.encode().unwrap()).unwrap();
    let expected = Box::new(
        ExpectedCheckout::ordinary(
            &source_staging,
            f.expected.files(),
            f.expected.deletions(),
            &f.expected.header().image_digest,
            None,
        )
        .unwrap(),
    );
    assert_eq!(expected.header(), f.expected.header());
    assert_eq!(
        expected.maximum_encoded_bytes(),
        f.expected.maximum_encoded_bytes()
    );
    let derived = Box::new(ManagedPlanData {
        observation: f.derived.observation.clone(),
        observation_digest: f.derived.observation_digest.clone(),
        placement: f.derived.placement.clone(),
        placement_digest: f.derived.placement_digest.clone(),
        capture: f.derived.capture.clone(),
        plan: ProviderTransitionIntentRecord::decode(&f.source_staging.encode().unwrap())
            .unwrap()
            .plan,
        required_manifests: f.derived.required_manifests.clone(),
        quota_bytes: f.derived.quota_bytes,
        transaction_id: f.derived.transaction_id.clone(),
        validated_metadata_floor: f.derived.validated_metadata_floor,
    });
    assert_eq!(derived.plan, f.derived.plan);
    phase.next("fresh-private-setup.close-b-owner");
    owner.close().unwrap();
    phase.next("fresh-private-setup.close-witness-and-detached-data");
    assert_eq!(
        source(
            f.source_database,
            f.source_marker,
            f.source_staging,
            None,
            &original_ingress
        ),
        source_rows
    );
    assert_eq!(snapshot(f.destination_database), destination_rows);
    assert_eq!(snapshot(f.family_database), family_rows);
    Box::new(Fixture {
        source: f.source_root.to_owned(),
        destination: f.destination_root.to_owned(),
        family: f.family_root.to_owned(),
        source_database: f.source_database.to_owned(),
        destination_database: f.destination_database.to_owned(),
        family_database: f.family_database.to_owned(),
        source_marker: f.source_marker.clone(),
        original_ingress,
        source_staging,
        destination_staging,
        expected,
        derived,
        selection_payload: f.selection_payload.to_vec(),
        base: f.base.clone(),
        history: f.history.clone(),
        current: f.current.clone(),
        required: f.required.to_vec(),
        source_rows,
        destination_rows,
        family_rows,
        pins,
    })
}

fn require_released_pins(f: &Fixture) {
    for path in &f.pins {
        drop(
            OpenOptions::new()
                .write(true)
                .share_mode(7)
                .open(path)
                .expect("all former B image/C checkout/CAS pins must be released"),
        );
    }
}

fn fresh_checkout_fixture() -> Box<Fixture> {
    let _total = child::phase("fresh-private-setup.total");
    let f = with_complete_checkout_data(close_at_complete_checkout);
    let _witness = child::phase("fresh-private-setup.released-holder-witness");
    require_released_pins(&f);
    require_unchanged_staging(&f);
    f
}

fn require_unchanged_b_and_family(f: &Fixture) {
    assert_eq!(
        source(
            &f.source_database,
            &f.source_marker,
            &f.source_staging,
            None,
            &f.original_ingress
        ),
        f.source_rows
    );
    assert_eq!(
        family(
            &f.family_database,
            &f.source_staging,
            &f.required,
            "initializing"
        ),
        f.family_rows
    );
}

fn require_unchanged_staging(f: &Fixture) {
    require_unchanged_b_and_family(f);
    assert_eq!(
        witness::destination(&f.destination_database, &f.source_staging),
        f.destination_rows
    );
    witness::read(&f.destination_database, |c| {
        let payload: Vec<u8> = c
            .query_row(
                "SELECT payload FROM projection_provider_transitions WHERE id=?1",
                [&f.destination_staging.transaction_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            ProviderTransitionIntentRecord::decode(&payload).unwrap(),
            f.destination_staging
        );
        assert!(checkout_journal::read(c, &f.expected, None)
            .unwrap()
            .is_complete());
    });
}

fn require_only_private_ready(f: &Fixture) -> Box<ReadyRows> {
    require_unchanged_b_and_family(f);
    Box::new(ready_witness::require_ready_rows(
        &f.destination_database,
        &f.destination_rows,
        &f.source_staging,
        &f.derived,
        &f.selection_payload,
        ready_witness::ReadyContent {
            base: &f.base,
            history: &f.history,
            current: &f.current,
        },
        &f.required,
        &f.expected,
    ))
}

fn trees(f: &Fixture) -> (Tree, Tree, Tree) {
    let b_lock = f.source_database.parent().unwrap().join("layrs.lock");
    let c_lock = f.destination_database.parent().unwrap().join("layrs.lock");
    witness::trees(&f.source, &f.family, &f.destination, [&b_lock, &c_lock])
}

fn only_c_database_changed(
    before: &(Tree, Tree, Tree),
    after: &(Tree, Tree, Tree),
    database: &Path,
) {
    let mut changes = 0;
    for (previous, current) in [
        (&before.0, &after.0),
        (&before.1, &after.1),
        (&before.2, &after.2),
    ] {
        assert_eq!(
            previous.keys().collect::<Vec<_>>(),
            current.keys().collect::<Vec<_>>()
        );
        for (path, node) in previous {
            let next = &current[path];
            if path == database {
                let (Node::File((old_id, _, old_hash)), Node::File((new_id, _, new_hash))) =
                    (node, next)
                else {
                    panic!("C database must remain a regular file")
                };
                assert_eq!(old_id, new_id);
                assert_ne!(old_hash, new_hash, "private Ready must be durable");
                changes += 1;
            } else {
                assert_eq!(node, next, "private Ready changed unrelated {path:?}");
            }
        }
    }
    assert!(changes > 0, "complete tree must include actual C database");
}

fn require_owner_contention(path: &Path, mode: SessionMode) {
    let error = StoreLock::acquire_existing(path, mode).unwrap_err();
    assert!(
        matches!(
            error,
            StoreError::LockBusy(_) | StoreError::ReentrantLock(_)
        ),
        "owner contention required: {error}"
    );
}

fn require_held_owners(f: &Fixture) {
    require_owner_contention(
        &f.source_database.parent().unwrap().join("layrs.lock"),
        SessionMode::Shared,
    );
    // Private finalization starts with C Exclusive; Shared must also fail.
    require_owner_contention(
        &f.destination_database.parent().unwrap().join("layrs.lock"),
        SessionMode::Shared,
    );
    require_owner_contention(
        &f.destination_database.parent().unwrap().join("layrs.lock"),
        SessionMode::Exclusive,
    );
}

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
        for path in &self.pins {
            let error = OpenOptions::new()
                .write(true)
                .share_mode(7)
                .open(path)
                .unwrap_err();
            assert_eq!(
                error.raw_os_error(),
                Some(32),
                "fresh B image/C checkout/CAS pin {path:?}"
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
        assert!(success, "requires actual SQLite connection close success, without native xClose/OS-close inference");
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
                "genuine {boundary:?} route {path:?} must be observed"
            );
        }
    }
    assert!(events.iter().any(|(path, point)| path == &f.source_database
        && *point == ReadBoundary::OrdinarySourceIdentity));
}

fn require_fresh_no_writer_replay(f: &Fixture, expected: &ReadyRows, cancel: &Cancellation) {
    let _total = child::phase("fresh-private-replay.total");
    let mut phase = child::phase("fresh-private-replay.baseline-witness");
    let writer_attempts = Rc::new(Cell::new(0usize));
    let observed_attempts = writer_attempts.clone();
    let before_trees = trees(f);
    let mut close = Close::new(f, false);
    let replay = with_writer_probe(
        move |_, point| {
            if point == AdmissionPoint::BeforeWritableOpen {
                observed_attempts.set(observed_attempts.get() + 1);
                return Err(invalid(
                    "actual private Ready replay forbids every writable open",
                ));
            }
            Ok(())
        },
        || {
            phase.next("fresh-private-replay.fresh-open");
            let held = OrdinaryPrivateReadyFinalization::open(
                &f.source,
                &f.destination,
                &f.source_staging.transaction_id,
                cancel,
            )
            .unwrap_or_else(|e| panic!("fresh actual private Ready replay {e}"));
            phase.next("fresh-private-replay.open-witness");
            close.require_pins();
            require_held_owners(f);
            phase.next("fresh-private-replay.finalize");
            held.finalize_ready(&mut close, cancel)
        },
    )
    .unwrap_or_else(|e| panic!("fresh private Ready replay {:?}: {}", e.step, e.cause));
    phase.next("fresh-private-replay.final-witness");
    assert_eq!(replay.effect(), ReadyEffect::AlreadyReady);
    assert_eq!(replay.ready(), &expected.ready);
    assert_eq!(replay.receipt(), &expected.receipt);
    assert_eq!(writer_attempts.get(), 0);
    assert_eq!(close.counts, (0, 0, 0, 0));
    drop(replay);
    assert_eq!(require_only_private_ready(f).rows, expected.rows);
    assert_eq!(trees(f), before_trees);
    require_released_pins(f);
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_private_ready_refuses_changed_b_and_c_then_appends_exact_suffix_and_replays_without_writer(
) {
    child::run(concat!(module_path!(), "::managed_ordinary_fresh_private_ready_refuses_changed_b_and_c_then_appends_exact_suffix_and_replays_without_writer"), 3600, || {
        let _total = child::phase("fresh-private-success.total");
        let f = fresh_checkout_fixture();
        let mut phase = child::phase("fresh-private-success.baseline-witness");
        let cancel = Cancellation::default();
        let before_trees = trees(&f);
        let cancelled = Cancellation::default();
        cancelled.cancel();
        phase.next("fresh-private-success.precancel");
        assert!(OrdinaryPrivateReadyFinalization::open(&f.source, &f.destination, &f.source_staging.transaction_id, &cancelled).is_err());
        phase.next("fresh-private-success.precancel-witness");
        require_unchanged_staging(&f);
        for (path, replacement, image_error) in [(&f.pins[0], B_DRIFT, true), (&f.pins[1], C_DRIFT, false)] {
            phase.next(if image_error { "fresh-private-success.negative-b-drift" } else { "fresh-private-success.negative-c-drift" });
            fs::write(path, replacement).unwrap();
            let writer_attempts = Rc::new(Cell::new(0usize));
            let observed_attempts = writer_attempts.clone();
            let preflights = Rc::new(RefCell::new(Vec::new()));
            let observed_preflights = preflights.clone();
            let refused = with_writer_probe(move |path, point| {
                observed_preflights.borrow_mut().push((path.to_owned(), point));
                if point == AdmissionPoint::BeforeWritableOpen { observed_attempts.set(observed_attempts.get()+1); return Err(invalid("changed actual B/C bytes must refuse before writable open")); }
                Ok(())
            }, || OrdinaryPrivateReadyFinalization::open(&f.source, &f.destination, &f.source_staging.transaction_id, &cancel))
                .err().expect("actual changed B/C bytes must not finalize from complete SQL journal alone");
            phase.next("fresh-private-success.negative-witness-and-restore");
            if image_error { assert!(refused.to_string().contains("image"), "{refused}"); }
            assert_eq!(writer_attempts.get(), 0);
            assert!(preflights.borrow().iter().any(|(path, point)| path == &f.source_database && *point == AdmissionPoint::ReadonlyClosed));
            require_unchanged_staging(&f);
            // Restore only this fresh fixture's original actual bytes. No SQL
            // phase, identity, journal, manifest, quota or grant is repaired.
            fs::write(path, CURRENT).unwrap();
            assert_eq!(trees(&f), before_trees);
            require_released_pins(&f);
        }
        phase.next("fresh-private-success.fresh-open");
        let held = OrdinaryPrivateReadyFinalization::open(&f.source, &f.destination, &f.source_staging.transaction_id, &cancel)
            .unwrap_or_else(|e| panic!("fresh genuine complete Staging checkout {e}"));
        phase.next("fresh-private-success.open-witness");
        require_held_owners(&f);
        let mut close = Close::new(&f, false);
        close.require_pins();
        let after_close = close.after_close.clone();
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        let post_close_reads = Rc::new(RefCell::new(Vec::new()));
        let observed_post_close = post_close_reads.clone();
        phase.next("fresh-private-success.finalize");
        let progress = with_observer(move |path, point| {
            observed.borrow_mut().push((path.to_owned(), point));
            if after_close.get() && point == ReadBoundary::BeforeSnapshot { observed_post_close.borrow_mut().push(path.to_owned()); }
            Ok(())
        }, || held.finalize_ready(&mut close, &cancel))
            .unwrap_or_else(|e| panic!("fresh private Ready finalization {:?}: {}", e.step, e.cause));
        phase.next("fresh-private-success.finalized-witness");
        assert_eq!(progress.effect(), ReadyEffect::Finalized);
        assert_eq!(close.counts, (1,1,1,0));
        require_read_controls(&events.borrow(), &f);
        for path in [&f.source_database, &f.destination_database, &f.family_database] {
            assert!(post_close_reads.borrow().contains(path), "fresh actual {path:?} readback follows positive SQLite C close");
        }
        let ready = require_only_private_ready(&f);
        assert_eq!(progress.ready(), &ready.ready);
        assert_eq!(progress.receipt(), &ready.receipt);
        drop(progress);
        only_c_database_changed(&before_trees, &trees(&f), &f.destination_database);
        require_released_pins(&f);
        phase.next("fresh-private-success.replay-and-witness");
        require_fresh_no_writer_replay(&f, &ready, &cancel);
        // Inspect the next independently fresh family adapter only. It must
        // leave the actual B Staging/family Initializing phases unchanged.
        phase.next("fresh-private-success.family-handoff-open");
        let handoff = OrdinaryFamilyReadyPublication::open(&f.source, &f.destination, &f.source_staging.transaction_id, &cancel)
            .unwrap_or_else(|e| panic!("fresh private Ready -> family adapter handoff {e}"));
        drop(handoff);
        phase.next("fresh-private-success.family-handoff-witness");
        assert_eq!(require_only_private_ready(&f).rows, ready.rows);
        require_released_pins(&f);
    });
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_private_ready_withheld_sqlite_close_ack_reopens_ready_after_explicit_holder_release(
) {
    child::run(concat!(module_path!(), "::managed_ordinary_fresh_private_ready_withheld_sqlite_close_ack_reopens_ready_after_explicit_holder_release"), 3600, || {
        let _total = child::phase("fresh-private-ack.total");
        let f = fresh_checkout_fixture(); // Separate genuine setup/parent.
        let mut phase = child::phase("fresh-private-ack.baseline-witness");
        let cancel = Cancellation::default();
        let before_trees = trees(&f);
        phase.next("fresh-private-ack.fresh-open");
        let held = OrdinaryPrivateReadyFinalization::open(&f.source, &f.destination, &f.source_staging.transaction_id, &cancel)
            .unwrap_or_else(|e| panic!("fresh genuine complete Staging checkout {e}"));
        phase.next("fresh-private-ack.open-witness");
        require_held_owners(&f);
        let mut close = Close::new(&f, true);
        close.require_pins();
        let after_close = close.after_close.clone();
        let after_close_boundaries = Rc::new(Cell::new(0usize));
        let observed_after_close = after_close_boundaries.clone();
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        phase.next("fresh-private-ack.finalize");
        let result = with_observer(move |path, point| {
            observed.borrow_mut().push((path.to_owned(), point));
            if after_close.get() { observed_after_close.set(observed_after_close.get()+1); return Err(invalid("failed private Ready close ACK forbids later observed SQL/identity/snapshot probes")); }
            Ok(())
        }, || held.finalize_ready(&mut close, &cancel));
        phase.next("fresh-private-ack.failure-witness-and-explicit-release");
        let OrdinaryPrivateReadyFinalizationFailure { step, cause, held } = result.err()
            .expect("withheld SQLite close ACK retains genuine fresh B/C authority");
        assert_eq!(step, AdmissionStep::CommitReturned);
        let primary = cause.to_string();
        assert!(primary.contains(WITHHELD_ACK), "{primary}");
        assert_eq!(close.counts, (1,1,1,1));
        assert_eq!(after_close_boundaries.get(), 0, "only mandatory closes follow failed ACK");
        require_read_controls(&events.borrow(), &f);
        close.require_pins();
        require_held_owners(&f);
        // Deliberate caller observations outside the failed driver do not
        // acknowledge its uncertain result or infer native close outcomes.
        let ready = require_only_private_ready(&f);
        only_c_database_changed(&before_trees, &trees(&f), &f.destination_database);
        drop(held);
        require_released_pins(&f);
        phase.next("fresh-private-ack.replay-and-witness");
        require_fresh_no_writer_replay(&f, &ready, &cancel);
        phase.next("fresh-private-ack.final-cause-witness");
        assert_eq!(cause.to_string(), primary, "fresh replay preserves original ACK error");
        assert_eq!(close.counts, (1,1,1,1));
    });
}
