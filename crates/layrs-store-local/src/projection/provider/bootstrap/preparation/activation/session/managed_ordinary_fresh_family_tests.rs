//! Genuine fresh B Staging + private C8 Ready -> family Ready after every
//! setup owner/image/allocation/checkout/CAS holder has been released. Missing
//! ACK follows an actual SQLite connection close returning success; native
//! xClose/OS-close outcomes, crash and owner-loss recovery are not qualified.
//! Positive deletion/nested checkout remain separate.
use super::{
    managed_ordinary_family_ready_tests::{copy_cell, family, text},
    managed_ordinary_installing_tests::{self as witness, Cell as SqlCell, Database, Node, Tree},
    managed_ordinary_ready_tests::CURRENT,
    managed_ordinary_source_publication_tests::{
        PublicationFixture, ingress, source, with_private_ready_data,
    },
};
use crate::{
    error::StoreError,
    lock::StoreLock,
    projection::{
        SessionMode, format,
        provider::{
            database_io::{
                WriterCloseObserver, WriterOpenPoint,
                tests::{
                    fresh_ordinary_support::{ReadBoundary, with_observer},
                    writer_support::{self, AdmissionPoint},
                },
            },
            managed_admission::read_admitted_import,
            managed_export::{
                OrdinaryFamilyReadyPublication, OrdinaryFamilyReadyPublicationFailure,
                OrdinaryManagedExport, OrdinaryStagingPromotion,
            },
            managed_ready_reopen::AdmissionStep,
            process_profile::tests::child,
            records::{
                Record,
                import::ProviderTransitionReceiptRecord,
                transition::{ProviderTransitionIntentRecord, TransitionState},
            },
            recovery::error::{ProviderBootstrapCause as Cause, Result as CauseResult, invalid},
        },
        shared::admitted_catalog::{
            ExportPublicationEffect, ExportPublicationMember, ExportPublicationState,
        },
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

const DRIFT: &[u8] = b"changed actual B image must refuse fresh family publication";
const WITHHELD_ACK: &str = "fresh family successful catalog close acknowledgement withheld";

fn snapshot(path: &Path) -> Database {
    witness::read(path, witness::database)
}

/// Owned DATA only. Boxing this summary avoids transporting large Ready
/// records inline through the nested generic setup continuation.
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
    required: Vec<FileManifest>,
    source_rows: Database,
    destination_rows: Database,
    family_rows: Database,
    pins: [PathBuf; 3],
}

fn close_at_private_ready(
    owner: OrdinaryManagedExport<'_>,
    f: &PublicationFixture<'_>,
    _: &Cancellation,
) -> Box<Fixture> {
    let (staging, original_ingress) = witness::read(f.source_database, |c| {
        let admitted = read_admitted_import(c, f.source_marker, None).unwrap();
        let actual = admitted
            .outgoing()
            .iter()
            .find(|row| row.transaction_id == f.ready.transaction_id)
            .unwrap();
        assert_eq!(actual.state, TransitionState::Staging);
        assert!(actual.receipt_id.is_none());
        assert!(
            admitted
                .outgoing_receipt(&f.receipt.digest().unwrap())
                .is_err()
        );
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
        let actual = ProviderTransitionIntentRecord::decode(&payload).unwrap();
        assert_eq!(actual, *f.ready);
        assert_eq!(actual.state, TransitionState::Ready);
        witness::database(c)
    });
    let family_rows = family(f.family_database, &staging, f.required, "initializing");
    let file = &f.expected.files()[0];
    let manifest = f
        .required
        .iter()
        .find(|manifest| manifest.id() == &file.manifest_id)
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
    Box::new(Fixture {
        source: f.source_root.to_owned(),
        destination: f.destination_root.to_owned(),
        family: f.family_root.to_owned(),
        source_database: f.source_database.to_owned(),
        destination_database: f.destination_database.to_owned(),
        family_database: f.family_database.to_owned(),
        source_marker: f.source_marker.clone(),
        original_ingress,
        staging,
        ready: ProviderTransitionIntentRecord::decode(&f.ready.encode().unwrap()).unwrap(),
        receipt: ProviderTransitionReceiptRecord::decode(&f.receipt.encode().unwrap()).unwrap(),
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
                .expect("all old and fresh B image/C checkout/CAS pins must be released"),
        );
    }
}

fn fresh_private_ready_fixture() -> Box<Fixture> {
    let f = with_private_ready_data(close_at_private_ready);
    require_released_pins(&f);
    require_unchanged_private_ready(&f);
    f
}

fn require_unchanged_b_and_c(f: &Fixture) {
    assert_eq!(
        source(
            &f.source_database,
            &f.source_marker,
            &f.staging,
            None,
            &f.original_ingress,
        ),
        f.source_rows
    );
    assert_eq!(snapshot(&f.destination_database), f.destination_rows);
}

fn require_unchanged_private_ready(f: &Fixture) {
    require_unchanged_b_and_c(f);
    assert_eq!(
        family(&f.family_database, &f.staging, &f.required, "initializing"),
        f.family_rows
    );
}

fn require_only_member_ready(f: &Fixture) -> Database {
    require_unchanged_b_and_c(f);
    let after = family(&f.family_database, &f.staging, &f.required, "ready");
    assert_eq!(
        f.family_rows.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    let proof = f.staging.destination.as_ref().unwrap();
    let shared = proof.format.shared.as_ref().unwrap();
    let mut expected_member = vec![
        text(proof.format.identity.worktree_id.as_str()),
        text(shared.metadata_path.to_str().unwrap()),
        text(&proof.private_database_identity),
        SqlCell::Blob(proof.format.bytes().unwrap()),
        text("initializing"),
    ];
    assert_eq!(
        f.family_rows["worktrees"]
            .iter()
            .filter(|row| row.first() == expected_member.first())
            .collect::<Vec<_>>(),
        vec![&expected_member]
    );
    for (table, rows) in &f.family_rows {
        if table != "worktrees" {
            assert_eq!(&after[table], rows, "family publication changed {table}");
            continue;
        }
        let mut expected: Vec<_> = rows
            .iter()
            .map(|row| row.iter().map(copy_cell).collect::<Vec<_>>())
            .collect();
        let member = expected
            .iter_mut()
            .find(|row| row.first() == expected_member.first())
            .unwrap();
        expected_member[4] = text("ready");
        *member = expected_member.iter().map(copy_cell).collect();
        expected.sort();
        assert_eq!(
            after[table], expected,
            "only exact C family member state may change"
        );
    }
    after
}

fn trees(f: &Fixture) -> (Tree, Tree, Tree) {
    let b_lock = f.source_database.parent().unwrap().join("layrs.lock");
    let c_lock = f.destination_database.parent().unwrap().join("layrs.lock");
    witness::trees(&f.source, &f.family, &f.destination, [&b_lock, &c_lock])
}

fn only_catalog_changed(before: &(Tree, Tree, Tree), after: &(Tree, Tree, Tree), catalog: &Path) {
    assert_eq!(
        before.0, after.0,
        "B tree changed during family publication"
    );
    assert_eq!(
        before.2, after.2,
        "C tree changed during family publication"
    );
    assert_eq!(
        before.1.keys().collect::<Vec<_>>(),
        after.1.keys().collect::<Vec<_>>()
    );
    assert!(before.1.contains_key(catalog));
    for (path, node) in &before.1 {
        let next = &after.1[path];
        if path == catalog {
            let (Node::File((old_id, _, old_hash)), Node::File((new_id, _, new_hash))) =
                (node, next)
            else {
                panic!("family catalog must remain a regular file")
            };
            assert_eq!(old_id, new_id);
            assert_ne!(old_hash, new_hash, "family publication must be durable");
        } else {
            assert_eq!(node, next, "family publication changed unrelated {path:?}");
        }
    }
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
        assert!(
            success,
            "requires actual SQLite family writer connection close returning success"
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
                "genuine {boundary:?} route {path:?} must be observed"
            );
        }
    }
    assert!(events.iter().any(|(path, point)| path == &f.source_database
        && *point == ReadBoundary::OrdinarySourceIdentity));
}

fn require_fresh_no_writer_replay(
    f: &Fixture,
    ready_rows: &Database,
    expected_state: Option<ExportPublicationState>,
    cancel: &Cancellation,
) {
    let writer_attempts = Rc::new(Cell::new(0usize));
    let observed_attempts = writer_attempts.clone();
    let mut close = Close::new(f, false);
    let before_trees = trees(f);
    let replay = with_writer_probe(
        move |_, point| {
            if point == AdmissionPoint::BeforeWritableOpen {
                observed_attempts.set(observed_attempts.get() + 1);
                return Err(invalid(
                    "fresh actual family Ready replay forbids every writer",
                ));
            }
            Ok(())
        },
        || {
            let held = OrdinaryFamilyReadyPublication::open(
                &f.source,
                &f.destination,
                &f.staging.transaction_id,
                cancel,
            )
            .unwrap_or_else(|e| panic!("fresh actual family Ready replay {e}"));
            close.require_pins();
            require_held_owners(f);
            held.publish_family_ready(&mut close, cancel)
        },
    )
    .unwrap_or_else(|e| panic!("fresh family read-only replay {:?}: {}", e.step, e.cause));
    assert_eq!(replay.effect(), ExportPublicationEffect::AlreadyReady);
    assert_eq!(replay.ready(), &f.ready);
    assert_eq!(replay.receipt(), &f.receipt);
    assert_eq!(replay.family_state().member, ExportPublicationMember::Ready);
    if let Some(expected) = expected_state {
        assert_eq!(replay.family_state(), expected);
    }
    assert_eq!(writer_attempts.get(), 0);
    assert_eq!(close.counts, (0, 0, 0, 0));
    drop(replay);
    require_unchanged_b_and_c(f);
    assert_eq!(&snapshot(&f.family_database), ready_rows);
    assert_eq!(trees(f), before_trees);
    require_released_pins(f);
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_family_refuses_changed_b_image_then_updates_only_member_and_replays_without_writer()
 {
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_fresh_family_refuses_changed_b_image_then_updates_only_member_and_replays_without_writer"
        ),
        3600,
        || {
            let f = fresh_private_ready_fixture();
            let cancel = Cancellation::default();
            let before_trees = trees(&f);
            let cancelled = Cancellation::default();
            cancelled.cancel();
            assert!(
                OrdinaryFamilyReadyPublication::open(
                    &f.source,
                    &f.destination,
                    &f.staging.transaction_id,
                    &cancelled
                )
                .is_err()
            );
            require_unchanged_private_ready(&f);
            fs::write(&f.pins[0], DRIFT).unwrap();
            let writer_attempts = Rc::new(Cell::new(0usize));
            let observed_attempts = writer_attempts.clone();
            let preflights = Rc::new(RefCell::new(Vec::new()));
            let observed_preflights = preflights.clone();
            let refused = with_writer_probe(
                move |path, point| {
                    observed_preflights
                        .borrow_mut()
                        .push((path.to_owned(), point));
                    if point == AdmissionPoint::BeforeWritableOpen {
                        observed_attempts.set(observed_attempts.get() + 1);
                        return Err(invalid(
                            "changed B image must refuse before family writable open",
                        ));
                    }
                    Ok(())
                },
                || {
                    OrdinaryFamilyReadyPublication::open(
                        &f.source,
                        &f.destination,
                        &f.staging.transaction_id,
                        &cancel,
                    )
                },
            )
            .err()
            .expect("actual changed B image cannot be replaced by C's copied commitment");
            assert!(refused.to_string().contains("image"), "{refused}");
            assert_eq!(writer_attempts.get(), 0);
            assert!(
                preflights
                    .borrow()
                    .iter()
                    .any(|(path, point)| path == &f.source_database
                        && *point == AdmissionPoint::ReadonlyClosed)
            );
            require_unchanged_private_ready(&f);
            // Restore only this fresh fixture's known original bytes; no durable
            // identity, plan, commitment, receipt, quota or SQL evidence is repaired.
            fs::write(&f.pins[0], CURRENT).unwrap();
            assert_eq!(trees(&f), before_trees);
            let held = OrdinaryFamilyReadyPublication::open(
                &f.source,
                &f.destination,
                &f.staging.transaction_id,
                &cancel,
            )
            .unwrap_or_else(|e| panic!("fresh authentic private Ready {e}"));
            require_held_owners(&f);
            let mut close = Close::new(&f, false);
            close.require_pins();
            let after_close = close.after_close.clone();
            let events = Rc::new(RefCell::new(Vec::new()));
            let observed = events.clone();
            let post_close_reads = Rc::new(RefCell::new(Vec::new()));
            let observed_post_close = post_close_reads.clone();
            let progress = with_observer(
                move |path, point| {
                    observed.borrow_mut().push((path.to_owned(), point));
                    if after_close.get() && point == ReadBoundary::BeforeSnapshot {
                        observed_post_close.borrow_mut().push(path.to_owned());
                    }
                    Ok(())
                },
                || held.publish_family_ready(&mut close, &cancel),
            )
            .unwrap_or_else(|e| panic!("fresh family Updated {:?}: {}", e.step, e.cause));
            assert_eq!(progress.effect(), ExportPublicationEffect::Updated);
            assert_eq!(progress.ready(), &f.ready);
            assert_eq!(progress.receipt(), &f.receipt);
            assert_eq!(
                progress.family_state().member,
                ExportPublicationMember::Ready
            );
            let family_state = progress.family_state();
            assert_eq!(close.counts, (1, 1, 1, 0));
            require_read_controls(&events.borrow(), &f);
            for path in [
                &f.source_database,
                &f.destination_database,
                &f.family_database,
            ] {
                assert!(
                    post_close_reads.borrow().contains(path),
                    "fresh actual {path:?} readback must follow positive family writer close"
                );
            }
            drop(progress);
            let ready_rows = require_only_member_ready(&f);
            only_catalog_changed(&before_trees, &trees(&f), &f.family_database);
            require_released_pins(&f);
            require_fresh_no_writer_replay(&f, &ready_rows, Some(family_state), &cancel);
            // Merely inspect the next distinct adapter after all family holders
            // have gone. No implicit B Ready or admission is part of this recipe.
            let handoff = OrdinaryStagingPromotion::open(
                &f.source,
                &f.destination,
                &f.staging.transaction_id,
                &cancel,
            )
            .unwrap_or_else(|e| panic!("fresh family Ready -> Staging adapter handoff {e}"));
            drop(handoff);
            require_unchanged_b_and_c(&f);
            assert_eq!(snapshot(&f.family_database), ready_rows);
            require_released_pins(&f);
        },
    );
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_family_withheld_catalog_close_ack_reopens_ready_after_explicit_holder_release()
 {
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_fresh_family_withheld_catalog_close_ack_reopens_ready_after_explicit_holder_release"
        ),
        3600,
        || {
            let f = fresh_private_ready_fixture(); // Separate genuine setup/parent.
            let cancel = Cancellation::default();
            let before_trees = trees(&f);
            let held = OrdinaryFamilyReadyPublication::open(
                &f.source,
                &f.destination,
                &f.staging.transaction_id,
                &cancel,
            )
            .unwrap_or_else(|e| panic!("fresh authentic private Ready {e}"));
            require_held_owners(&f);
            let mut close = Close::new(&f, true);
            close.require_pins();
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
                            "failed family close ACK forbids later observed SQL/identity/snapshot probes",
                        ));
                    }
                    Ok(())
                },
                || held.publish_family_ready(&mut close, &cancel),
            );
            let OrdinaryFamilyReadyPublicationFailure { step, cause, held } = result
                .err()
                .expect("withheld family close ACK must retain genuine fresh B/C authority");
            assert_eq!(step, AdmissionStep::CommitReturned);
            let primary = cause.to_string();
            assert!(primary.contains(WITHHELD_ACK), "{primary}");
            assert_eq!(close.counts, (1, 1, 1, 1));
            assert_eq!(
                after_close_boundaries.get(),
                0,
                "only mandatory closes follow failed ACK"
            );
            require_read_controls(&events.borrow(), &f);
            close.require_pins();
            require_held_owners(&f);
            // These explicit caller observations occur outside the failed driver;
            // they do not manufacture an acknowledgement of its uncertain result.
            let ready_rows = require_only_member_ready(&f);
            only_catalog_changed(&before_trees, &trees(&f), &f.family_database);
            drop(held);
            require_released_pins(&f);
            require_fresh_no_writer_replay(&f, &ready_rows, None, &cancel);
            assert_eq!(
                cause.to_string(),
                primary,
                "fresh replay preserves original ACK error"
            );
            assert_eq!(close.counts, (1, 1, 1, 1));
        },
    );
}
