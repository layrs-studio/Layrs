//! Proposed fresh B -> C checkout recipes, from genuine completed protection.
//! Each recipe owns a new parent and releases all setup capabilities before
//! reopening. Withheld ACK follows SQLite connection close success; no native
//! xClose/OS-close failure, crash or owner-loss outcome is claimed.
use super::{
    managed_ordinary_checkout_tests::require_ack_primary,
    managed_ordinary_family_ready_tests::family,
    managed_ordinary_installing_tests::{self as witness, Database, Node, Tree},
    managed_ordinary_ready_tests::CURRENT,
    managed_ordinary_source_publication_tests::{
        ingress, source, with_protected_checkout_data, ProtectedCheckoutFixture,
    },
};
use crate::{
    error::StoreError,
    lock::StoreLock,
    projection::{
        format,
        provider::{
            checkout_journal::{
                self, CheckoutEntry, CheckoutJournal, DirectoryState, ExpectedCheckout, FileState,
                RowKey,
            },
            database_io::{
                tests::{
                    fresh_ordinary_support::{with_observer, ReadBoundary},
                    writer_support::{self, AdmissionPoint},
                },
                WriterCloseObserver, WriterOpenPoint,
            },
            managed_export::{
                CheckoutPopulationEffect, OrdinaryCheckoutPopulation,
                OrdinaryCheckoutPopulationFailure, OrdinaryManagedExport,
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
    collections::BTreeSet,
    fs::{self, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    rc::Rc,
};

const WITHHELD_ACK: &str = "fresh checkout SQLite close acknowledgement deliberately withheld";
const B_DRIFT: &[u8] = b"changed actual B bytes refuse fresh checkout";
const FOREIGN: &[u8] = b"foreign checkout file must never be adopted";

fn require_copy_io_primary(cause: &Cause) {
    use crate::projection::provider::managed_export::checkout_copy_probe::InjectedCopyError;
    match cause {
        Cause::Cleanup { primary, .. } => require_copy_io_primary(primary),
        Cause::Io(error) => {
            assert_eq!(error.kind(), std::io::ErrorKind::Other);
            assert!(error
                .get_ref()
                .is_some_and(|inner| inner.is::<InjectedCopyError>()));
        }
        other => panic!("original typed copy I/O error required: {other}"),
    }
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_checkout_partial_write_retains_owned_inode_and_restarts_copy() {
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_fresh_checkout_partial_write_retains_owned_inode_and_restarts_copy"
        ),
        3600,
        || {
            use crate::projection::provider::managed_export::checkout_copy_probe;
            let _total = child::phase("fresh-checkout-partial-write.total");
            let f = fixture();
            let before = trees(&f);
            let cancel = Cancellation::default();
            let mut phase = child::phase("fresh-checkout-partial-write.open");
            let held = OrdinaryCheckoutPopulation::open(
                &f.source,
                &f.destination,
                &f.source_staging.transaction_id,
                &cancel,
            )
            .unwrap_or_else(|e| panic!("fresh partial-write open: {e}"));
            require_owners(&f);
            let prefix = CURRENT.len() / 2;
            assert!(0 < prefix && prefix < CURRENT.len());
            let mut close = Close::new(&f, None);
            phase.next("fresh-checkout-partial-write.real-sink-failure");
            // Only the real Owned copy sink is shortened. Write::write_all writes
            // this prefix to its actual TEMP, then receives the typed I/O error.
            // There is no caller-created TEMP or post-failure byte mutation.
            let (result, observed) = checkout_copy_probe::with_prefix_failure(prefix, || {
                held.populate_checkout(&mut close, &cancel)
            });
            let OrdinaryCheckoutPopulationFailure { step, cause, held } = result
                .err()
                .expect("real short-write/error must stop checkout population");
            assert_eq!(observed.written, prefix);
            assert_eq!(observed.errors, 1);
            require_copy_io_primary(&cause);
            let primary = cause.to_string();
            assert_eq!(step, AdmissionStep::ReadScopesClosed);
            // Declare, Stage Owned and File Owned have acknowledged SQLite closes;
            // neither Verified nor Publishing may be committed after the copy error.
            assert_eq!(close.counts, (3, 3, 3, 0));
            close.require_pins();
            require_owners(&f);
            let temp = temporary(&f);
            let temp_identity =
                crate::projection::provider::database_io::tests::support::witness(&temp).0;
            assert_eq!(fs::read(&temp).unwrap(), &CURRENT[..prefix]);
            assert!(!final_path(&f).exists());
            require_mutable_handle(&temp, false, true);
            phase.next("fresh-checkout-partial-write.explicit-holder-release");
            drop(held);
            require_released(&f);
            require_mutable_handle(&temp, false, false);
            require_rows(&f);
            let partial = trees(&f);
            let Node::Directory(stage_identity) = &partial.2[&stage_path(&f)] else {
                panic!("actual owned stage directory required")
            };
            let Node::File((actual_inode, actual_bytes, _)) = &partial.2[&temp] else {
                panic!("actual partial TEMP required")
            };
            assert_eq!(actual_inode, &temp_identity);
            assert_eq!(*actual_bytes, prefix as u64);
            assert!(*actual_bytes < f.expected.files()[0].bytes);
            witness::read(&f.destination_database, |c| {
                let journal = checkout_journal::read(c, &f.expected, None).unwrap();
                assert_eq!(journal.rows().len(), 3);
                assert!(!journal.is_complete());
                assert_eq!(
                    journal.row(RowKey::Header).unwrap(),
                    &f.expected.declared_row(RowKey::Header).unwrap()
                );
                assert!(matches!(&journal.row(RowKey::Stage).unwrap().entry,
                CheckoutEntry::Stage { state: DirectoryState::Owned { identity } } if identity == stage_identity));
                assert!(matches!(&journal.row(RowKey::File(0)).unwrap().entry,
                CheckoutEntry::File { file, state: FileState::Owned { inode }, .. }
                    if inode == &temp_identity && file == &f.expected.files()[0]));
            });
            only_checkout_changed(
                &f,
                &before,
                &partial,
                BTreeSet::from([stage_path(&f), temp.clone()]),
            );
            phase.next("fresh-checkout-partial-write.fresh-open-without-writer");
            let open_writers = Rc::new(Cell::new(0usize));
            let observed_open_writers = open_writers.clone();
            let reopened = with_writer_probe(
                move |_, point| {
                    if point == AdmissionPoint::BeforeWritableOpen {
                        observed_open_writers.set(observed_open_writers.get() + 1);
                        return Err(invalid("partial TEMP reopen must not open a writer"));
                    }
                    Ok(())
                },
                || {
                    OrdinaryCheckoutPopulation::open(
                        &f.source,
                        &f.destination,
                        &f.source_staging.transaction_id,
                        &cancel,
                    )
                },
            )
            .unwrap_or_else(|e| panic!("fresh partial TEMP reopen: {e}"));
            assert_eq!(open_writers.get(), 0);
            assert_eq!(
                trees(&f),
                partial,
                "open cannot repair or truncate the partial TEMP"
            );
            require_owners(&f);
            let mut resumed_close = Close::new(&f, None);
            phase.next("fresh-checkout-partial-write.resume-copy-from-start");
            // The scoped sink fault has been removed. The production Owned route
            // must rewrite from zero and publish the original journal-owned inode.
            let completed = reopened
                .populate_checkout(&mut resumed_close, &cancel)
                .unwrap_or_else(|e| panic!("partial TEMP resume {:?}: {}", e.step, e.cause));
            assert_eq!(completed.effect(), CheckoutPopulationEffect::Updated);
            assert_eq!(resumed_close.counts, (3, 3, 3, 0));
            phase.next("fresh-checkout-partial-write.complete-witness");
            require_complete(&f, Some(completed.journal()));
            let complete = trees(&f);
            assert_eq!(
                &complete.2[&stage_path(&f)],
                &Node::Directory(stage_identity.clone())
            );
            let Node::File((final_inode, _, _)) = &complete.2[&final_path(&f)] else {
                panic!("actual published file required")
            };
            assert_eq!(final_inode, &temp_identity);
            drop(completed);
            require_released(&f);
            only_checkout_changed(
                &f,
                &before,
                &trees(&f),
                BTreeSet::from([stage_path(&f), final_path(&f)]),
            );
            phase.next("fresh-checkout-partial-write.complete-replay");
            replay_without_writer(&f, &cancel);
            assert_eq!(cause.to_string(), primary);
            require_copy_io_primary(&cause);
        },
    );
}

fn snapshot(path: &Path) -> Database {
    witness::read(path, witness::database)
}

/// Detached DATA only. No allocation, image, CAS or protection proof escapes.
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
    required: Vec<FileManifest>,
    source_rows: Database,
    destination_rows: Database,
    family_rows: Database,
    // B image, current CAS chunk, then every remaining exact CAS object.
    pins: Vec<PathBuf>,
}

fn close_after_protection(
    owner: OrdinaryManagedExport<'_>,
    f: &ProtectedCheckoutFixture<'_>,
    _: &Cancellation,
) -> Box<Fixture> {
    let mut phase = child::phase("fresh-checkout-setup.protected-witness");
    assert_eq!(f.source_staging.role, TransitionRole::Source);
    assert_eq!(f.source_staging.state, TransitionState::Staging);
    assert!(f.source_staging.receipt_id.is_none());
    assert_eq!(f.destination_staging.role, TransitionRole::Destination);
    assert_eq!(f.destination_staging.state, TransitionState::Staging);
    assert!(f.destination_staging.receipt_id.is_none());
    let _actual_setup_protection = f.protection;
    let original_ingress = witness::read(f.source_database, |c| ingress(c, f.source_marker));
    let source_rows = source(
        f.source_database,
        f.source_marker,
        f.source_staging,
        None,
        &original_ingress,
    );
    let destination_rows = witness::destination(f.destination_database, f.source_staging);
    assert!(destination_rows["projection_prejournal"].is_empty());
    assert_eq!(
        destination_rows["projection_provider_transition_receipts"],
        source_rows["projection_provider_transition_receipts"],
        "complete inherited ingress receipt table is copied to C"
    );
    assert_eq!(
        source_rows["projection_provider_transition_receipts"].len(),
        1
    );
    let family_rows = family(
        f.family_database,
        f.source_staging,
        f.required,
        "initializing",
    );
    assert_eq!(f.expected.files().len(), 1);
    assert!(f.expected.directories().is_empty());
    assert!(f.expected.deletions().is_empty());
    assert_eq!(&f.expected.files()[0].manifest_id, f.current.id());
    let mut pins = vec![
        f.source_root.join(f.expected.files()[0].path.as_str()),
        witness::object(
            f.family_root,
            "chunks",
            f.current.chunks()[0].chunk_id().as_str(),
        ),
    ];
    assert_eq!(fs::read(&pins[0]).unwrap(), CURRENT);
    assert_eq!(fs::read(&pins[1]).unwrap(), CURRENT);
    assert_ne!(
        crate::projection::provider::database_io::tests::support::witness(&pins[0]).0,
        crate::projection::provider::database_io::tests::support::witness(&pins[1]).0,
        "B image and protected CAS require independent identity witnesses"
    );
    for manifest in f.required {
        let objects = std::iter::once(witness::object(
            f.family_root,
            "manifests",
            manifest.id().as_str(),
        ))
        .chain(
            manifest
                .chunks()
                .iter()
                .map(|chunk| witness::object(f.family_root, "chunks", chunk.chunk_id().as_str())),
        );
        for path in objects {
            if !pins.contains(&path) {
                pins.push(path);
            }
        }
    }
    assert!(!f
        .destination_root
        .join(f.expected.stage_relative_path())
        .exists());
    assert!(!f
        .destination_root
        .join(f.expected.files()[0].path.as_str())
        .exists());
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
    phase.next("fresh-checkout-setup.close-original-b");
    owner.close().unwrap();
    phase.next("fresh-checkout-setup.detached-data");
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
        required: f.required.to_vec(),
        source_rows,
        destination_rows,
        family_rows,
        pins,
    })
}

fn released(path: &Path) {
    drop(
        OpenOptions::new()
            .write(true)
            .share_mode(7)
            .open(path)
            .expect("all former image/CAS/checkout pins must be released"),
    );
}

fn blocked(path: &Path) {
    let error = OpenOptions::new()
        .write(true)
        .share_mode(7)
        .open(path)
        .unwrap_err();
    assert_eq!(
        error.raw_os_error(),
        Some(32),
        "retained native pin {path:?}"
    );
}

fn require_released(f: &Fixture) {
    for path in &f.pins {
        released(path);
    }
    let final_path = final_path(f);
    if final_path.exists() {
        released(&final_path);
    }
    let temporary = temporary(f);
    if temporary.exists() {
        released(&temporary);
    }
}

fn fixture() -> Box<Fixture> {
    let _total = child::phase("fresh-checkout-setup.total");
    let f = with_protected_checkout_data(close_after_protection);
    let _witness = child::phase("fresh-checkout-setup.released-witness");
    require_released(&f);
    require_rows(&f);
    assert_eq!(snapshot(&f.destination_database), f.destination_rows);
    f
}

fn final_path(f: &Fixture) -> PathBuf {
    f.destination.join(f.expected.files()[0].path.as_str())
}

fn stage_path(f: &Fixture) -> PathBuf {
    f.destination.join(f.expected.stage_relative_path())
}

fn temporary(f: &Fixture) -> PathBuf {
    stage_path(f).join(f.expected.temporary_name(0).unwrap())
}

/// Every rowset is compared, including inherited receipts, grants and quotas.
/// Only checkout journal rows may change. C remains C8 Staging throughout.
fn require_rows(f: &Fixture) {
    assert_eq!(
        source(
            &f.source_database,
            &f.source_marker,
            &f.source_staging,
            None,
            &f.original_ingress,
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
    let current = witness::destination(&f.destination_database, &f.source_staging);
    assert_eq!(
        current.keys().collect::<Vec<_>>(),
        f.destination_rows.keys().collect::<Vec<_>>()
    );
    for (table, rows) in &f.destination_rows {
        if table != "projection_prejournal" {
            assert_eq!(&current[table], rows, "unexpected C mutation in {table}");
        }
    }
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
        assert_eq!(c.query_row(
            "SELECT count(*) FROM projection_provider_transition_receipts WHERE transaction_id=?1",
            [&f.source_staging.transaction_id], |row| row.get::<_, u64>(0),
        ).unwrap(), 0, "only the new B -> C receipt is absent");
    });
}

fn trees(f: &Fixture) -> (Tree, Tree, Tree) {
    let b_lock = f.source_database.parent().unwrap().join("layrs.lock");
    let c_lock = f.destination_database.parent().unwrap().join("layrs.lock");
    witness::trees(&f.source, &f.family, &f.destination, [&b_lock, &c_lock])
}

/// Preserve every preexisting inode and byte except C's journal database;
/// assert the complete set of new names, including the still-empty stage.
fn only_checkout_changed(
    f: &Fixture,
    before: &(Tree, Tree, Tree),
    after: &(Tree, Tree, Tree),
    new_paths: BTreeSet<PathBuf>,
) {
    assert_eq!(before.0, after.0);
    for (previous, current) in [
        (&before.0, &after.0),
        (&before.1, &after.1),
        (&before.2, &after.2),
    ] {
        for (path, node) in previous {
            let next = &current[path];
            if path == &f.destination_database {
                let (Node::File((old_id, _, _)), Node::File((new_id, _, _))) = (node, next) else {
                    panic!("C database remains a regular file")
                };
                assert_eq!(old_id, new_id);
            } else {
                assert_eq!(node, next, "checkout changed unrelated {path:?}");
            }
        }
        let actual_new = current
            .keys()
            .filter(|path| !previous.contains_key(*path))
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected_new = new_paths
            .iter()
            .filter(|path| current.contains_key(*path))
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(actual_new, expected_new);
    }
    for path in new_paths {
        assert!(
            after.2.contains_key(&path),
            "new checkout path {path:?} is observed"
        );
    }
}

fn require_complete(f: &Fixture, expected_journal: Option<&CheckoutJournal>) {
    require_rows(f);
    let complete = trees(f);
    witness::read(&f.destination_database, |c| {
        let journal = checkout_journal::read(c, &f.expected, None).unwrap();
        assert!(journal.is_complete());
        assert_eq!(journal.rows().len(), 3);
        assert_eq!(
            journal.row(RowKey::Header).unwrap(),
            &f.expected.declared_row(RowKey::Header).unwrap()
        );
        if let Some(expected) = expected_journal {
            assert_eq!(journal.rows(), expected.rows());
        }
        let CheckoutEntry::Stage {
            state: DirectoryState::Owned { identity },
        } = &journal.row(RowKey::Stage).unwrap().entry
        else {
            panic!("complete stage is durably Owned")
        };
        assert_eq!(
            &complete.2[&stage_path(f)],
            &Node::Directory(identity.clone())
        );
        let CheckoutEntry::File {
            state:
                FileState::Published {
                    inode,
                    parent_identity,
                },
            file,
            ..
        } = &journal.row(RowKey::File(0)).unwrap().entry
        else {
            panic!("complete file is durably Published")
        };
        assert_eq!(file, &f.expected.files()[0]);
        assert_eq!(parent_identity, &f.expected.header().checkout_identity);
        let Node::File((actual, bytes, _)) = &complete.2[&final_path(f)] else {
            panic!("published checkout must be an actual regular file")
        };
        assert_eq!(inode, actual);
        assert_eq!(*bytes, CURRENT.len() as u64);
        for pin in &f.pins {
            assert_ne!(
                inode,
                &crate::projection::provider::database_io::tests::support::witness(pin).0,
                "published checkout has independent physical identity"
            );
        }
    });
    assert_eq!(fs::read(final_path(f)).unwrap(), CURRENT);
    assert!(fs::read_dir(stage_path(f)).unwrap().next().is_none());
    assert!(!temporary(f).exists());
    witness::verify_objects(&f.family, &f.required);
}

fn require_owners(f: &Fixture) {
    for database in [&f.source_database, &f.destination_database] {
        for mode in [SessionMode::Shared, SessionMode::Exclusive] {
            let error =
                StoreLock::acquire_existing(&database.parent().unwrap().join("layrs.lock"), mode)
                    .unwrap_err();
            assert!(
                matches!(
                    error,
                    StoreError::LockBusy(_) | StoreError::ReentrantLock(_)
                ),
                "exclusive fresh owner required: {error}"
            );
        }
    }
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

fn refuse_open_without_writer(f: &Fixture, cancel: &Cancellation) -> Cause {
    let writers = Rc::new(Cell::new(0usize));
    let observed = writers.clone();
    let before = trees(f);
    let result = with_writer_probe(
        move |_, point| {
            if point == AdmissionPoint::BeforeWritableOpen {
                observed.set(observed.get() + 1);
                return Err(invalid(
                    "invalid fresh checkout must refuse before writable open",
                ));
            }
            Ok(())
        },
        || {
            OrdinaryCheckoutPopulation::open(
                &f.source,
                &f.destination,
                &f.source_staging.transaction_id,
                cancel,
            )
        },
    );
    let cause = result.err().expect("invalid fresh checkout must refuse");
    assert_eq!(writers.get(), 0);
    assert_eq!(
        trees(f),
        before,
        "refusal preserves every actual inode and byte"
    );
    require_rows(f);
    require_released(f);
    cause
}

struct Close {
    pins: Vec<PathBuf>,
    counts: (usize, usize, usize, usize),
    refuse_at: Option<usize>,
    withheld: Rc<Cell<bool>>,
}

impl Close {
    fn new(f: &Fixture, refuse_at: Option<usize>) -> Self {
        Self {
            pins: f.pins.clone(),
            counts: (0, 0, 0, 0),
            refuse_at,
            withheld: Rc::new(Cell::new(false)),
        }
    }
    fn require_pins(&self) {
        for path in &self.pins {
            blocked(path);
        }
    }
}

impl WriterCloseObserver for Close {
    fn open_point(&mut self, point: WriterOpenPoint, raw: *mut ffi::sqlite3) -> CauseResult<()> {
        assert!(
            !self.withheld.get(),
            "no writer probe after withheld close ACK"
        );
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
            "actual SQLite connection close success required; no native OS-close inference"
        );
        self.counts.2 += 1;
        self.require_pins();
        if self.refuse_at == Some(self.counts.2) {
            self.counts.3 += 1;
            self.withheld.set(true);
            Some(invalid(WITHHELD_ACK))
        } else {
            None
        }
    }
}

fn replay_without_writer(f: &Fixture, cancel: &Cancellation) {
    let mut phase = child::phase("fresh-checkout-replay.baseline-witness");
    let before = trees(f);
    let rows = snapshot(&f.destination_database);
    let writers = Rc::new(Cell::new(0usize));
    let observed = writers.clone();
    let mut close = Close::new(f, None);
    let progress = with_writer_probe(
        move |_, point| {
            if point == AdmissionPoint::BeforeWritableOpen {
                observed.set(observed.get() + 1);
                return Err(invalid("complete checkout replay forbids writable open"));
            }
            Ok(())
        },
        || {
            phase.next("fresh-checkout-replay.open");
            let held = OrdinaryCheckoutPopulation::open(
                &f.source,
                &f.destination,
                &f.source_staging.transaction_id,
                cancel,
            )
            .unwrap_or_else(|e| panic!("fresh complete checkout open: {e}"));
            require_owners(f);
            close.require_pins();
            phase.next("fresh-checkout-replay.populate");
            held.populate_checkout(&mut close, cancel)
        },
    )
    .unwrap_or_else(|e| panic!("complete checkout replay {:?}: {}", e.step, e.cause));
    phase.next("fresh-checkout-replay.complete-witness");
    assert_eq!(progress.effect(), CheckoutPopulationEffect::AlreadyComplete);
    assert_eq!(progress.expected().header(), f.expected.header());
    require_complete(f, Some(progress.journal()));
    assert_eq!(writers.get(), 0);
    assert_eq!(close.counts, (0, 0, 0, 0));
    drop(progress);
    assert_eq!(snapshot(&f.destination_database), rows);
    assert_eq!(trees(f), before);
    require_released(f);
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_checkout_refuses_drift_then_populates_absent_and_replays_without_writer()
{
    child::run(concat!(module_path!(), "::managed_ordinary_fresh_checkout_refuses_drift_then_populates_absent_and_replays_without_writer"), 3600, || {
        let _total = child::phase("fresh-checkout-success.total");
        let f = fixture();
        let mut phase = child::phase("fresh-checkout-success.baseline-witness");
        let before = trees(&f);
        let cancel = Cancellation::default();
        let cancelled = Cancellation::default();
        cancelled.cancel();
        phase.next("fresh-checkout-success.precancel");
        let _ = refuse_open_without_writer(&f, &cancelled);
        require_rows(&f);
        assert_eq!(trees(&f), before);
        phase.next("fresh-checkout-success.changed-b");
        fs::write(&f.pins[0], B_DRIFT).unwrap();
        let refused = refuse_open_without_writer(&f, &cancel);
        assert!(refused.to_string().contains("image"), "{refused}");
        require_rows(&f);
        assert_eq!(snapshot(&f.destination_database), f.destination_rows);
        fs::write(&f.pins[0], CURRENT).unwrap();
        assert_eq!(trees(&f), before);
        require_released(&f);
        phase.next("fresh-checkout-success.foreign-c-namespace");
        fs::write(final_path(&f), FOREIGN).unwrap();
        let _ = refuse_open_without_writer(&f, &cancel);
        assert_eq!(fs::read(final_path(&f)).unwrap(), FOREIGN);
        require_rows(&f);
        assert_eq!(snapshot(&f.destination_database), f.destination_rows);
        fs::remove_file(final_path(&f)).unwrap();
        assert_eq!(trees(&f), before);
        require_released(&f);
        phase.next("fresh-checkout-success.fresh-open");
        let held = OrdinaryCheckoutPopulation::open(&f.source, &f.destination, &f.source_staging.transaction_id, &cancel)
            .unwrap_or_else(|e| panic!("fresh protected checkout open: {e}"));
        require_owners(&f);
        let mut close = Close::new(&f, None);
        close.require_pins();
        phase.next("fresh-checkout-success.populate");
        let progress = held.populate_checkout(&mut close, &cancel)
            .unwrap_or_else(|e| panic!("fresh checkout populate {:?}: {}", e.step, e.cause));
        phase.next("fresh-checkout-success.complete-witness");
        assert_eq!(progress.effect(), CheckoutPopulationEffect::Updated);
        assert_eq!(progress.expected().header(), f.expected.header());
        assert_eq!(progress.expected().files(), f.expected.files());
        require_complete(&f, Some(progress.journal()));
        assert_eq!(close.counts.0, close.counts.1);
        assert_eq!(close.counts.1, close.counts.2);
        assert_eq!(close.counts.3, 0);
        assert!(close.counts.2 > 0);
        drop(progress);
        require_released(&f);
        only_checkout_changed(&f, &before, &trees(&f), BTreeSet::from([stage_path(&f), final_path(&f)]));
        phase.next("fresh-checkout-success.fresh-complete-replay");
        replay_without_writer(&f, &cancel);
    });
}

#[derive(Clone, Copy)]
enum Cut {
    Declare,
    Owned,
}

fn require_cut(f: &Fixture, cut: Cut) {
    require_rows(f);
    let physical = trees(f);
    witness::read(&f.destination_database, |c| {
        let journal = checkout_journal::read(c, &f.expected, None).unwrap();
        assert!(!journal.is_complete());
        assert_eq!(journal.rows().len(), 3);
        assert_eq!(
            journal.row(RowKey::Header).unwrap(),
            &f.expected.declared_row(RowKey::Header).unwrap()
        );
        match cut {
            Cut::Declare => {
                assert_eq!(
                    journal.row(RowKey::Stage).unwrap(),
                    &f.expected.declared_row(RowKey::Stage).unwrap()
                );
                assert_eq!(
                    journal.row(RowKey::File(0)).unwrap(),
                    &f.expected.declared_row(RowKey::File(0)).unwrap()
                );
                assert!(!stage_path(f).exists());
                assert!(!temporary(f).exists());
            }
            Cut::Owned => {
                let CheckoutEntry::Stage {
                    state: DirectoryState::Owned { identity },
                } = &journal.row(RowKey::Stage).unwrap().entry
                else {
                    panic!("stage Owned must be durably observed")
                };
                assert_eq!(
                    &physical.2[&stage_path(f)],
                    &Node::Directory(identity.clone())
                );
                let CheckoutEntry::File {
                    state: FileState::Owned { inode },
                    file,
                    ..
                } = &journal.row(RowKey::File(0)).unwrap().entry
                else {
                    panic!("file Owned must be durably observed")
                };
                assert_eq!(file, &f.expected.files()[0]);
                let Node::File((actual, bytes, _)) = &physical.2[&temporary(f)] else {
                    panic!("actual Owned TEMP required")
                };
                assert_eq!(inode, actual);
                assert_eq!(*bytes, 0, "no producer bytes before Owned close ACK");
                assert_eq!(fs::read_dir(stage_path(f)).unwrap().count(), 1);
            }
        }
        assert!(!final_path(f).exists());
    });
}

fn withheld_ack_recipe(cut: Cut) {
    let _total = child::phase("fresh-checkout-ack.total");
    let f = fixture(); // Each native recipe has its own genuine setup/parent.
    let mut phase = child::phase("fresh-checkout-ack.baseline-witness");
    let before = trees(&f);
    let cancel = Cancellation::default();
    phase.next("fresh-checkout-ack.open");
    let held = OrdinaryCheckoutPopulation::open(
        &f.source,
        &f.destination,
        &f.source_staging.transaction_id,
        &cancel,
    )
    .unwrap_or_else(|e| panic!("fresh protected checkout open: {e}"));
    require_owners(&f);
    let refuse_at = match cut {
        Cut::Declare => 1,
        Cut::Owned => 3,
    };
    let mut close = Close::new(&f, Some(refuse_at));
    close.require_pins();
    let withheld = close.withheld.clone();
    let reads_after_ack = Rc::new(Cell::new(0usize));
    let observed_reads = reads_after_ack.clone();
    let reads_before_ack = Rc::new(Cell::new(0usize));
    let observed_before_reads = reads_before_ack.clone();
    let read_events = Rc::new(RefCell::new(Vec::new()));
    let observed_events = read_events.clone();
    let writer_ack = withheld.clone();
    let writers_after_ack = Rc::new(Cell::new(0usize));
    let observed_writers = writers_after_ack.clone();
    let writers_before_ack = Rc::new(Cell::new(0usize));
    let observed_before_writers = writers_before_ack.clone();
    phase.next("fresh-checkout-ack.populate-until-withheld");
    let result = with_writer_probe(
        move |_, _| {
            if writer_ack.get() {
                observed_writers.set(observed_writers.get() + 1);
                return Err(invalid(
                    "withheld checkout ACK forbids subsequent writer admission probes",
                ));
            }
            observed_before_writers.set(observed_before_writers.get() + 1);
            Ok(())
        },
        || {
            with_observer(
                move |path, point: ReadBoundary| {
                    observed_events.borrow_mut().push((path.to_owned(), point));
                    if withheld.get() {
                        observed_reads.set(observed_reads.get() + 1);
                        return Err(invalid("withheld checkout ACK forbids subsequent read/identity/snapshot probes"));
                    }
                    observed_before_reads.set(observed_before_reads.get() + 1);
                    Ok(())
                },
                || held.populate_checkout(&mut close, &cancel),
            )
        },
    );
    phase.next("fresh-checkout-ack.retained-failure-witness");
    let OrdinaryCheckoutPopulationFailure { step, cause, held } = result
        .err()
        .expect("deliberately withheld close ACK must retain fresh authority");
    assert_eq!(step, AdmissionStep::CommitReturned);
    let primary = cause.to_string();
    assert!(primary.contains(WITHHELD_ACK), "{primary}");
    require_ack_primary(&cause, WITHHELD_ACK);
    assert_eq!(close.counts, (refuse_at, refuse_at, refuse_at, 1));
    assert_eq!(reads_after_ack.get(), 0);
    assert_eq!(writers_after_ack.get(), 0);
    assert!(
        reads_before_ack.get() > 0,
        "read guard observed the real driver before ACK"
    );
    assert!(
        writers_before_ack.get() > 0,
        "writer guard observed the real driver before ACK"
    );
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
                read_events
                    .borrow()
                    .iter()
                    .any(|(actual, point)| actual == path && *point == boundary),
                "actual {boundary:?} route {path:?} must be observed before the withheld ACK"
            );
        }
    }
    assert!(read_events
        .borrow()
        .iter()
        .any(|(path, point)| path == &f.source_database
            && *point == ReadBoundary::OrdinarySourceIdentity));
    close.require_pins();
    require_owners(&f);
    let retained_temp_identity = if let Cut::Owned = cut {
        let temp = temporary(&f);
        let identity = crate::projection::provider::database_io::tests::support::witness(&temp).0;
        // The mutable TEMP handle shares READ|WRITE|DELETE. Requesting a NEW
        // non-sharing read conflicts with its existing data access. This tests
        // handle retention, not an immutable or write-exclusive TEMP policy.
        // See Microsoft Learn, Creating and Opening Files: compatibility is
        // checked in both directions between desired access and share modes.
        let error = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&temp)
            .unwrap_err();
        assert_eq!(
            error.raw_os_error(),
            Some(32),
            "pending TEMP handle must remain retained"
        );
        Some(identity)
    } else {
        None
    };
    phase.next("fresh-checkout-ack.explicit-holder-release");
    drop(held);
    if let Some(identity) = retained_temp_identity {
        let temp = temporary(&f);
        drop(
            OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&temp)
                .expect("dropping the failure holder releases its mutable TEMP handle"),
        );
        assert_eq!(
            crate::projection::provider::database_io::tests::support::witness(&temp).0,
            identity,
            "handle release must preserve the exact TEMP inode"
        );
    }
    require_released(&f);
    // Exact durable SQL and native identities, not the close counter, prove
    // which progress survived. These are deliberate caller observations.
    phase.next("fresh-checkout-ack.durable-cut-witness");
    require_cut(&f, cut);
    let new_paths = match cut {
        Cut::Declare => BTreeSet::new(),
        Cut::Owned => BTreeSet::from([stage_path(&f), temporary(&f)]),
    };
    let partial = trees(&f);
    only_checkout_changed(&f, &before, &partial, new_paths);
    if let Cut::Declare = cut {
        phase.next("fresh-checkout-ack.declared-foreign-stage-refusal");
        // Deliberate fixture namespace mutation, with no SQL ownership edit.
        // This checks refusal only; it does not resolve/adopt an allocation
        // gap or represent a crash between native allocation and its journal.
        fs::create_dir(stage_path(&f)).unwrap();
        let _ = refuse_open_without_writer(&f, &cancel);
        require_cut_declared_rows_only(&f);
        fs::remove_dir(stage_path(&f)).unwrap();
        assert_eq!(trees(&f), partial);
    }
    phase.next("fresh-checkout-ack.fresh-resume-open");
    let reopened = OrdinaryCheckoutPopulation::open(
        &f.source,
        &f.destination,
        &f.source_staging.transaction_id,
        &cancel,
    )
    .unwrap_or_else(|e| panic!("fresh partial checkout reopen: {e}"));
    require_owners(&f);
    let mut resumed_close = Close::new(&f, None);
    resumed_close.require_pins();
    phase.next("fresh-checkout-ack.fresh-resume-populate");
    let completed = reopened
        .populate_checkout(&mut resumed_close, &cancel)
        .unwrap_or_else(|e| panic!("fresh partial checkout resume {:?}: {}", e.step, e.cause));
    phase.next("fresh-checkout-ack.resumed-complete-witness");
    assert_eq!(completed.effect(), CheckoutPopulationEffect::Updated);
    require_complete(&f, Some(completed.journal()));
    if let Cut::Owned = cut {
        let Node::Directory(previous_stage) = &partial.2[&stage_path(&f)] else {
            panic!("partial stage identity")
        };
        let Node::File((previous_temp, _, _)) = &partial.2[&temporary(&f)] else {
            panic!("partial TEMP identity")
        };
        let complete = trees(&f);
        assert_eq!(
            &complete.2[&stage_path(&f)],
            &Node::Directory(previous_stage.clone())
        );
        let Node::File((final_inode, _, _)) = &complete.2[&final_path(&f)] else {
            panic!("resumed published inode")
        };
        assert_eq!(
            final_inode, previous_temp,
            "resumed Owned TEMP is published without adopting another inode"
        );
    }
    drop(completed);
    require_released(&f);
    only_checkout_changed(
        &f,
        &before,
        &trees(&f),
        BTreeSet::from([stage_path(&f), final_path(&f)]),
    );
    phase.next("fresh-checkout-ack.complete-replay");
    replay_without_writer(&f, &cancel);
    assert_eq!(
        cause.to_string(),
        primary,
        "fresh progress cannot replace the original ACK error"
    );
    require_ack_primary(&cause, WITHHELD_ACK);
    assert_eq!(close.counts, (refuse_at, refuse_at, refuse_at, 1));
}

fn require_cut_declared_rows_only(f: &Fixture) {
    witness::read(&f.destination_database, |c| {
        let journal = checkout_journal::read(c, &f.expected, None).unwrap();
        assert_eq!(journal.rows().len(), 3);
        for key in [RowKey::Header, RowKey::Stage, RowKey::File(0)] {
            assert_eq!(
                journal.row(key).unwrap(),
                &f.expected.declared_row(key).unwrap()
            );
        }
    });
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_checkout_withheld_declare_ack_releases_then_reopens_and_completes() {
    child::run(concat!(module_path!(), "::managed_ordinary_fresh_checkout_withheld_declare_ack_releases_then_reopens_and_completes"), 3600, || withheld_ack_recipe(Cut::Declare));
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_checkout_withheld_owned_ack_releases_then_reopens_and_completes() {
    child::run(concat!(module_path!(), "::managed_ordinary_fresh_checkout_withheld_owned_ack_releases_then_reopens_and_completes"), 3600, || withheld_ack_recipe(Cut::Owned));
}

// Later cutpoints below are a SOURCE-ONLY proposal. Their distinct native
// recipes each create a new genuine parent; none reuses an earlier fixture.
const BEFORE_PUBLISHED_WRITER: &str =
    "fresh checkout Published writable admission deliberately refused";
const BEFORE_STAGE_OWNED_WRITER: &str =
    "fresh checkout stage Owned writable admission deliberately refused";
const BEFORE_FILE_OWNED_WRITER: &str =
    "fresh checkout file Owned writable admission deliberately refused";

#[derive(Clone, Copy)]
enum LateCut {
    Verified,
    Publishing,
    RenameGap,
}

impl LateCut {
    fn ack_number(self) -> Option<usize> {
        match self {
            Self::Verified => Some(4),
            Self::Publishing => Some(5),
            Self::RenameGap => None,
        }
    }
    fn resumed_writers(self) -> usize {
        match self {
            Self::Verified => 2,
            Self::Publishing | Self::RenameGap => 1,
        }
    }
    fn expected_error(self) -> &'static str {
        match self {
            Self::Verified | Self::Publishing => WITHHELD_ACK,
            Self::RenameGap => BEFORE_PUBLISHED_WRITER,
        }
    }
}

/// A non-sharing read proves retention of an already-open mutable handle.
/// It makes no assertion that the mutable TEMP denies other writable opens.
/// Directory access needs FILE_FLAG_BACKUP_SEMANTICS; no mutation is requested.
/// For the Stage creator specifically, privacy::create_new_private_child keeps
/// windows::create_new_private_directory_at's handle with FILE_ADD_FILE (0x2,
/// the FILE_WRITE_DATA bit) and FILE_ADD_SUBDIRECTORY (0x4) desired access.
/// Its existing write access conflicts with a new share0 open. An attributes-
/// only traversal pin would not justify this same retention assertion.
fn require_mutable_handle(path: &Path, directory: bool, retained: bool) {
    let opened = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .custom_flags(if directory { 0x0200_0000 } else { 0 })
        .open(path);
    if retained {
        let error = opened.expect_err("pending mutable handle must remain retained");
        assert_eq!(error.raw_os_error(), Some(32));
    } else {
        drop(opened.expect("explicit holder drop releases the pending mutable handle"));
    }
}

fn require_late_read_controls(events: &[(PathBuf, ReadBoundary)], f: &Fixture) {
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
                "actual {boundary:?} route {path:?} precedes the deliberate cut"
            );
        }
    }
    assert!(events.iter().any(|(path, point)| path == &f.source_database
        && *point == ReadBoundary::OrdinarySourceIdentity));
}

/// Observed identity DATA after all failed-driver capabilities were dropped.
/// This does not grant permission to rewrite a Verified or Publishing TEMP.
struct LateWitness {
    stage_identity: String,
    file_identity: String,
}

fn require_late_cut(f: &Fixture, cut: LateCut) -> LateWitness {
    require_rows(f);
    let physical = trees(f);
    let Node::Directory(stage_identity) = &physical.2[&stage_path(f)] else {
        panic!("actual retained stage directory required")
    };
    let observed_path = match cut {
        LateCut::RenameGap => final_path(f),
        _ => temporary(f),
    };
    let Node::File((file_identity, bytes, _)) = &physical.2[&observed_path] else {
        panic!("actual complete TEMP/final inode required")
    };
    assert_eq!(*bytes, CURRENT.len() as u64);
    assert_eq!(fs::read(&observed_path).unwrap(), CURRENT);
    witness::read(&f.destination_database, |c| {
        let journal = checkout_journal::read(c, &f.expected, None).unwrap();
        assert!(!journal.is_complete());
        assert_eq!(journal.rows().len(), 3);
        assert_eq!(
            journal.row(RowKey::Header).unwrap(),
            &f.expected.declared_row(RowKey::Header).unwrap()
        );
        assert_eq!(
            journal.row(RowKey::Stage).unwrap(),
            &f.expected
                .declared_row(RowKey::Stage)
                .unwrap()
                .owned_directory(stage_identity)
                .unwrap()
        );
        let verified = f
            .expected
            .declared_row(RowKey::File(0))
            .unwrap()
            .owned_file(file_identity)
            .unwrap()
            .verified_file()
            .unwrap();
        let wanted = match cut {
            LateCut::Verified => verified,
            LateCut::Publishing | LateCut::RenameGap => verified
                .publishing_file(&f.expected.header().checkout_identity)
                .unwrap(),
        };
        assert_eq!(
            journal.row(RowKey::File(0)).unwrap(),
            &wanted,
            "durable full row binds exact inventory, inode, and publication parent"
        );
    });
    match cut {
        LateCut::RenameGap => {
            assert!(!temporary(f).exists());
            assert!(fs::read_dir(stage_path(f)).unwrap().next().is_none());
        }
        LateCut::Verified | LateCut::Publishing => {
            assert!(!final_path(f).exists());
            assert_eq!(fs::read_dir(stage_path(f)).unwrap().count(), 1);
        }
    }
    for pin in &f.pins {
        assert_ne!(
            file_identity,
            &crate::projection::provider::database_io::tests::support::witness(pin).0
        );
    }
    LateWitness {
        stage_identity: stage_identity.clone(),
        file_identity: file_identity.clone(),
    }
}

fn late_cut_recipe(cut: LateCut) {
    let _total = child::phase("fresh-checkout-late.total");
    let f = fixture();
    let mut phase = child::phase("fresh-checkout-late.baseline-witness");
    let before = trees(&f);
    let cancel = Cancellation::default();
    phase.next("fresh-checkout-late.open");
    let held = OrdinaryCheckoutPopulation::open(
        &f.source,
        &f.destination,
        &f.source_staging.transaction_id,
        &cancel,
    )
    .unwrap_or_else(|e| panic!("fresh late checkout open: {e}"));
    require_owners(&f);
    let mut close = Close::new(&f, cut.ack_number());
    close.require_pins();
    let stopped = close.withheld.clone();
    let writer_stopped = stopped.clone();
    let writer_attempts = Rc::new(Cell::new(0usize));
    let observed_attempts = writer_attempts.clone();
    let writers_after_cut = Rc::new(Cell::new(0usize));
    let observed_after_writers = writers_after_cut.clone();
    let reads_after_cut = Rc::new(Cell::new(0usize));
    let observed_after_reads = reads_after_cut.clone();
    let read_events = Rc::new(RefCell::new(Vec::new()));
    let observed_events = read_events.clone();
    phase.next("fresh-checkout-late.populate-to-cut");
    let result = with_writer_probe(
        move |_, point| {
            if writer_stopped.get() {
                observed_after_writers.set(observed_after_writers.get() + 1);
                return Err(invalid(
                    "late checkout cut forbids subsequent writer admission probes",
                ));
            }
            if point == AdmissionPoint::BeforeWritableOpen {
                let attempt = observed_attempts.get() + 1;
                observed_attempts.set(attempt);
                if matches!(cut, LateCut::RenameGap) && attempt == 6 {
                    writer_stopped.set(true);
                    return Err(invalid(BEFORE_PUBLISHED_WRITER));
                }
            }
            Ok(())
        },
        || {
            with_observer(
                move |path, point| {
                    observed_events.borrow_mut().push((path.to_owned(), point));
                    if stopped.get() {
                        observed_after_reads.set(observed_after_reads.get() + 1);
                        return Err(invalid(
                            "late checkout cut forbids subsequent read/identity/snapshot probes",
                        ));
                    }
                    Ok(())
                },
                || held.populate_checkout(&mut close, &cancel),
            )
        },
    );
    phase.next("fresh-checkout-late.failure-and-retained-witness");
    let OrdinaryCheckoutPopulationFailure { step, cause, held } = result
        .err()
        .expect("late checkout cut must retain fresh authority");
    let primary = cause.to_string();
    require_ack_primary(&cause, cut.expected_error());
    let expected_counts = match cut {
        LateCut::Verified => {
            assert_eq!(step, AdmissionStep::CommitReturned);
            (4, 4, 4, 1)
        }
        LateCut::Publishing => {
            assert_eq!(step, AdmissionStep::CommitReturned);
            (5, 5, 5, 1)
        }
        LateCut::RenameGap => {
            // The Published writer has not opened or attempted COMMIT. The
            // acknowledged Publishing row precedes the real rename and sync.
            assert_eq!(step, AdmissionStep::WriterOpenAttempted);
            (5, 5, 5, 0)
        }
    };
    assert_eq!(close.counts, expected_counts);
    assert_eq!(
        writer_attempts.get(),
        match cut {
            LateCut::Verified => 4,
            LateCut::Publishing => 5,
            LateCut::RenameGap => 6,
        }
    );
    assert_eq!(writers_after_cut.get(), 0);
    assert_eq!(reads_after_cut.get(), 0);
    require_late_read_controls(&read_events.borrow(), &f);
    close.require_pins();
    require_owners(&f);
    // Verified failure retains the mutable piece used for actual copying.
    // Publishing and rename-gap retain B/CAS pins, without claiming a TEMP
    // or final immutable pin that the driver has not constructed.
    let mutable_identity = if matches!(cut, LateCut::Verified) {
        require_mutable_handle(&temporary(&f), false, true);
        Some(crate::projection::provider::database_io::tests::support::witness(&temporary(&f)).0)
    } else {
        None
    };
    phase.next("fresh-checkout-late.explicit-holder-release");
    drop(held);
    if let Some(identity) = mutable_identity {
        require_mutable_handle(&temporary(&f), false, false);
        assert_eq!(
            crate::projection::provider::database_io::tests::support::witness(&temporary(&f)).0,
            identity
        );
    }
    require_released(&f);
    phase.next("fresh-checkout-late.actual-sql-and-native-cut-witness");
    let actual = Box::new(require_late_cut(&f, cut));
    let cut_path = match cut {
        LateCut::RenameGap => final_path(&f),
        _ => temporary(&f),
    };
    only_checkout_changed(
        &f,
        &before,
        &trees(&f),
        BTreeSet::from([stage_path(&f), cut_path]),
    );
    phase.next("fresh-checkout-late.fresh-resume-open");
    let reopen_writers = Rc::new(Cell::new(0usize));
    let observed_reopen_writers = reopen_writers.clone();
    let reopened = with_writer_probe(
        move |_, point| {
            if point == AdmissionPoint::BeforeWritableOpen {
                observed_reopen_writers.set(observed_reopen_writers.get() + 1);
                return Err(invalid("partial checkout reopen must not open a writer"));
            }
            Ok(())
        },
        || {
            OrdinaryCheckoutPopulation::open(
                &f.source,
                &f.destination,
                &f.source_staging.transaction_id,
                &cancel,
            )
        },
    )
    .unwrap_or_else(|e| panic!("fresh late-cut reopen: {e}"));
    assert_eq!(reopen_writers.get(), 0);
    require_owners(&f);
    let mut resumed_close = Close::new(&f, None);
    resumed_close.require_pins();
    phase.next("fresh-checkout-late.fresh-resume-populate");
    let completed = reopened
        .populate_checkout(&mut resumed_close, &cancel)
        .unwrap_or_else(|e| panic!("fresh late-cut resume {:?}: {}", e.step, e.cause));
    phase.next("fresh-checkout-late.complete-witness");
    assert_eq!(completed.effect(), CheckoutPopulationEffect::Updated);
    let writers = cut.resumed_writers();
    assert_eq!(resumed_close.counts, (writers, writers, writers, 0));
    require_complete(&f, Some(completed.journal()));
    let complete = trees(&f);
    assert_eq!(
        &complete.2[&stage_path(&f)],
        &Node::Directory(actual.stage_identity.clone())
    );
    let Node::File((final_identity, _, _)) = &complete.2[&final_path(&f)] else {
        panic!("actual final inode required")
    };
    assert_eq!(
        final_identity, &actual.file_identity,
        "fresh continuation publishes the exact pre-cut inode"
    );
    drop(completed);
    require_released(&f);
    only_checkout_changed(
        &f,
        &before,
        &trees(&f),
        BTreeSet::from([stage_path(&f), final_path(&f)]),
    );
    phase.next("fresh-checkout-late.fresh-complete-replay");
    replay_without_writer(&f, &cancel);
    assert_eq!(cause.to_string(), primary);
    require_ack_primary(&cause, cut.expected_error());
    assert_eq!(close.counts, expected_counts);
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_checkout_withheld_verified_ack_reopens_exact_temp_and_finishes_with_two_writers(
) {
    child::run(concat!(module_path!(), "::managed_ordinary_fresh_checkout_withheld_verified_ack_reopens_exact_temp_and_finishes_with_two_writers"), 3600, || late_cut_recipe(LateCut::Verified));
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_checkout_withheld_publishing_ack_reopens_exact_temp_and_finishes_with_one_writer(
) {
    child::run(concat!(module_path!(), "::managed_ordinary_fresh_checkout_withheld_publishing_ack_reopens_exact_temp_and_finishes_with_one_writer"), 3600, || late_cut_recipe(LateCut::Publishing));
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_checkout_rename_gap_reopens_exact_final_and_finishes_with_one_writer() {
    child::run(concat!(module_path!(), "::managed_ordinary_fresh_checkout_rename_gap_reopens_exact_final_and_finishes_with_one_writer"), 3600, || late_cut_recipe(LateCut::RenameGap));
}

fn require_unowned_namespace_refusal(mut cause: &Cause) {
    while let Cause::Cleanup { primary, .. } = cause {
        cause = primary;
    }
    let Cause::Validation(error) = cause else {
        panic!("allocation gap must reach the namespace validation: {cause:?}")
    };
    let crate::projection::ProjectionStoreError::Invalid(message) = error.as_ref() else {
        panic!("allocation gap must refuse an unowned namespace entry: {error:?}")
    };
    assert!(
        matches!(
            message.as_str(),
            "checkout namespace entry count or duplicate differs"
                | "checkout namespace has unowned entry"
        ),
        "unrelated refusal cannot qualify the allocation-gap protection: {message}"
    );
}

#[derive(Clone, Copy)]
enum AllocationGap {
    Stage,
    File,
}

impl AllocationGap {
    fn writer(self) -> usize {
        match self {
            Self::Stage => 2,
            Self::File => 3,
        }
    }
    fn expected_error(self) -> &'static str {
        match self {
            Self::Stage => BEFORE_STAGE_OWNED_WRITER,
            Self::File => BEFORE_FILE_OWNED_WRITER,
        }
    }
}

/// The object is genuinely created by the driver, then its first ownership
/// writer is refused before RW open. SQL still Declared cannot authorize a
/// later opener to adopt, replace or delete this actual allocation gap.
fn allocation_gap_recipe(gap: AllocationGap) {
    let _total = child::phase("fresh-checkout-allocation-gap.total");
    let f = fixture();
    let mut phase = child::phase("fresh-checkout-allocation-gap.baseline-witness");
    let before = trees(&f);
    let cancel = Cancellation::default();
    let held = OrdinaryCheckoutPopulation::open(
        &f.source,
        &f.destination,
        &f.source_staging.transaction_id,
        &cancel,
    )
    .unwrap_or_else(|e| panic!("fresh allocation-gap open: {e}"));
    require_owners(&f);
    let mut close = Close::new(&f, None);
    let attempts = Rc::new(Cell::new(0usize));
    let observed_attempts = attempts.clone();
    let stopped = close.withheld.clone();
    let read_stopped = stopped.clone();
    let after_reads = Rc::new(Cell::new(0usize));
    let observed_after_reads = after_reads.clone();
    let after_writers = Rc::new(Cell::new(0usize));
    let observed_after_writers = after_writers.clone();
    let read_events = Rc::new(RefCell::new(Vec::new()));
    let observed_events = read_events.clone();
    phase.next("fresh-checkout-allocation-gap.refuse-first-ownership-writer");
    let result = with_writer_probe(
        move |_, point| {
            if stopped.get() {
                observed_after_writers.set(observed_after_writers.get() + 1);
                return Err(invalid("allocation gap forbids subsequent writer probes"));
            }
            if point == AdmissionPoint::BeforeWritableOpen {
                observed_attempts.set(observed_attempts.get() + 1);
                if observed_attempts.get() == gap.writer() {
                    stopped.set(true);
                    return Err(invalid(gap.expected_error()));
                }
            }
            Ok(())
        },
        || {
            with_observer(
                move |path, point| {
                    observed_events.borrow_mut().push((path.to_owned(), point));
                    if read_stopped.get() {
                        observed_after_reads.set(observed_after_reads.get() + 1);
                        return Err(invalid(
                            "allocation gap forbids subsequent read/identity/snapshot probes",
                        ));
                    }
                    Ok(())
                },
                || held.populate_checkout(&mut close, &cancel),
            )
        },
    );
    phase.next("fresh-checkout-allocation-gap.retained-failure-witness");
    let OrdinaryCheckoutPopulationFailure { step, cause, held } = result
        .err()
        .expect("ownership admission cut must retain the created object");
    assert_eq!(step, AdmissionStep::WriterOpenAttempted);
    require_ack_primary(&cause, gap.expected_error());
    let primary = cause.to_string();
    assert_eq!(attempts.get(), gap.writer());
    let successful = gap.writer() - 1;
    assert_eq!(close.counts, (successful, successful, successful, 0));
    assert_eq!(after_reads.get(), 0);
    assert_eq!(after_writers.get(), 0);
    require_late_read_controls(&read_events.borrow(), &f);
    close.require_pins();
    require_owners(&f);
    let (pending_path, directory) = match gap {
        AllocationGap::Stage => (stage_path(&f), true),
        AllocationGap::File => (temporary(&f), false),
    };
    require_mutable_handle(&pending_path, directory, true);
    let pending_identity = if directory {
        crate::secure_fs::PinnedDirectory::open(&pending_path)
            .unwrap()
            .identity_token()
    } else {
        crate::projection::provider::database_io::tests::support::witness(&pending_path).0
    };
    phase.next("fresh-checkout-allocation-gap.explicit-holder-release");
    drop(held);
    require_mutable_handle(&pending_path, directory, false);
    require_released(&f);
    phase.next("fresh-checkout-allocation-gap.actual-sql-and-native-witness");
    require_rows(&f);
    let actual = trees(&f);
    let Node::Directory(stage_identity) = &actual.2[&stage_path(&f)] else {
        panic!("actual allocated stage exists")
    };
    match gap {
        AllocationGap::Stage => assert_eq!(stage_identity, &pending_identity),
        AllocationGap::File => {
            let Node::File((inode, _, _)) = &actual.2[&pending_path] else {
                panic!("released pending TEMP must remain the same regular file")
            };
            assert_eq!(inode, &pending_identity);
        }
    }
    witness::read(&f.destination_database, |c| {
        let journal = checkout_journal::read(c, &f.expected, None).unwrap();
        assert_eq!(journal.rows().len(), 3);
        assert!(!journal.is_complete());
        assert_eq!(
            journal.row(RowKey::Header).unwrap(),
            &f.expected.declared_row(RowKey::Header).unwrap()
        );
        assert_eq!(
            journal.row(RowKey::File(0)).unwrap(),
            &f.expected.declared_row(RowKey::File(0)).unwrap()
        );
        let wanted_stage = f.expected.declared_row(RowKey::Stage).unwrap();
        let wanted_stage = match gap {
            AllocationGap::Stage => wanted_stage,
            AllocationGap::File => wanted_stage.owned_directory(stage_identity).unwrap(),
        };
        assert_eq!(journal.row(RowKey::Stage).unwrap(), &wanted_stage);
    });
    assert!(!final_path(&f).exists());
    let names = match gap {
        AllocationGap::Stage => {
            assert!(!temporary(&f).exists());
            assert!(fs::read_dir(stage_path(&f)).unwrap().next().is_none());
            BTreeSet::from([stage_path(&f)])
        }
        AllocationGap::File => {
            let Node::File((inode, bytes, _)) = &actual.2[&temporary(&f)] else {
                panic!("actual allocated TEMP exists")
            };
            assert_eq!(*bytes, 0);
            assert!(fs::read(temporary(&f)).unwrap().is_empty());
            assert_ne!(
                inode,
                &crate::projection::provider::database_io::tests::support::witness(&f.pins[0]).0
            );
            assert_eq!(fs::read_dir(stage_path(&f)).unwrap().count(), 1);
            BTreeSet::from([stage_path(&f), temporary(&f)])
        }
    };
    only_checkout_changed(&f, &before, &actual, names);
    let rows = snapshot(&f.destination_database);
    phase.next("fresh-checkout-allocation-gap.fresh-refusal-without-adoption-or-cleanup");
    let refused = refuse_open_without_writer(&f, &cancel);
    require_unowned_namespace_refusal(&refused);
    assert_eq!(snapshot(&f.destination_database), rows);
    assert_eq!(
        trees(&f),
        actual,
        "fresh refusal preserves every allocated object, inode and byte"
    );
    // No deletion, SQL ownership edit, synthetic replacement, or successful
    // recovery is attempted: this recipe leaves the genuine gap untouched.
    assert_eq!(cause.to_string(), primary);
    require_ack_primary(&cause, gap.expected_error());
    assert_eq!(close.counts, (successful, successful, successful, 0));
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_checkout_stage_allocation_gap_retains_then_refuses_without_adoption_or_cleanup(
) {
    child::run(concat!(module_path!(), "::managed_ordinary_fresh_checkout_stage_allocation_gap_retains_then_refuses_without_adoption_or_cleanup"), 3600, || allocation_gap_recipe(AllocationGap::Stage));
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required; proposed unmeasured"]
fn managed_ordinary_fresh_checkout_file_allocation_gap_retains_then_refuses_without_adoption_or_cleanup(
) {
    child::run(concat!(module_path!(), "::managed_ordinary_fresh_checkout_file_allocation_gap_retains_then_refuses_without_adoption_or_cleanup"), 3600, || allocation_gap_recipe(AllocationGap::File));
}
