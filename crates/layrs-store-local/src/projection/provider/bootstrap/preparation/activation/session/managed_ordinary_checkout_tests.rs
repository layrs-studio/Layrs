//! Genuine B -> C protection and journal-first checkout under one retained
//! ordinary owner. Withheld successful close ACKs are deliberate observation
//! errors, not OS close failures, owner-loss recovery or crash qualification.
use super::{
    managed_ordinary_installing_tests::{self as witness, Cell, Database, Node, Tree},
    managed_ordinary_ready_tests::require_ready_read_controls,
};
use crate::projection::{
    ProjectionStore,
    provider::{
        checkout_journal::{
            self, CheckoutEntry, CheckoutFile, DirectoryState, ExpectedCheckout, FileState, RowKey,
        },
        database_io::{
            WriterCloseObserver, WriterOpenPoint, tests::fresh_ordinary_support::with_observer,
        },
        managed_export::allocation,
        ordinary_destination::{CopyEffect, staging::StageEffect},
        process_profile::tests::child,
        records::{
            Record,
            transition::{ProviderTransitionIntentRecord, TransitionState},
        },
        recovery::error::{ProviderBootstrapCause as Cause, Result as CauseResult, invalid},
        transition_sql::SourceIntentEffect,
    },
    shared::{
        admitted_catalog::ExportReservationEffect,
        exact_protection::{ExactProtection, ProtectionPresence},
        provider_access::ImportRegistrationEffect,
    },
};
use layrs_core::scm::{FileId, FileManifest, RepoPath, TreeEntryTarget, WorktreeId};
use layrs_vfs::Cancellation;
use rusqlite::ffi;
use std::{
    cell::{Cell as Flag, RefCell},
    collections::BTreeSet,
    fs::{self, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    rc::Rc,
};

const LEAF: &str = "000000000000000000000000000000f1";
const TX: &str = "000000000000000000000000000000f2";
const HISTORY: &[u8] = b"ordinary checkout historical-only captured content";
const CURRENT: &[u8] = b"ordinary checkout current uncaptured bytes";
const WITHHELD_ACK: &str = "checkout Declare successful close acknowledgement withheld";

#[derive(Default)]
struct Close {
    opens: usize,
    raw: usize,
    closes: usize,
    withhold: bool,
    withheld: usize,
    protected_chunk: Option<PathBuf>,
    pin_checks: usize,
    withheld_namespace: Option<(PathBuf, PathBuf)>,
    after_withheld_ack: Rc<Flag<bool>>,
}
impl Close {
    fn counts(&self) -> (usize, usize, usize, usize) {
        (self.opens, self.raw, self.closes, self.withheld)
    }
}
fn require_write_blocked(path: &Path) {
    let error = OpenOptions::new()
        .write(true)
        .share_mode(7)
        .open(path)
        .unwrap_err();
    assert_eq!(
        error.raw_os_error(),
        Some(32),
        "actual C CAS immutable pins must refuse WRITE"
    );
}

pub(super) fn require_ack_primary(mut cause: &Cause, expected: &str) {
    while let Cause::Cleanup { primary, .. } = cause {
        cause = primary;
    }
    let Cause::Validation(error) = cause else {
        panic!("expected the withheld ACK as the primary validation cause: {cause:?}")
    };
    let crate::projection::ProjectionStoreError::Invalid(message) = error.as_ref() else {
        panic!("expected exact withheld ACK validation: {error:?}")
    };
    assert_eq!(message, expected);
}
impl WriterCloseObserver for Close {
    fn open_point(&mut self, point: WriterOpenPoint, raw: *mut ffi::sqlite3) -> CauseResult<()> {
        match point {
            WriterOpenPoint::BeforeRaw => {
                assert!(raw.is_null());
                self.opens += 1;
                if let Some(path) = &self.protected_chunk {
                    require_write_blocked(path);
                    self.pin_checks += 1;
                }
            }
            WriterOpenPoint::AfterRaw => {
                assert!(!raw.is_null());
                self.raw += 1;
            }
            _ => {}
        }
        Ok(())
    }
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        assert!(
            succeeded,
            "observer requires SQLite connection close success; no native xClose/OS-close inference"
        );
        self.closes += 1;
        if std::mem::take(&mut self.withhold) {
            let (stage, final_file) = self.withheld_namespace.as_ref().unwrap();
            assert!(
                !stage.exists(),
                "no checkout stage before Declare close acknowledgement"
            );
            assert!(
                !final_file.exists(),
                "no checkout file before Declare close acknowledgement"
            );
            self.withheld += 1;
            self.after_withheld_ack.set(true);
            Some(invalid(WITHHELD_ACK))
        } else {
            None
        }
    }
}
fn snapshot(path: &Path) -> Database {
    witness::read(path, witness::database)
}
fn remaining(path: &Path, stage: &ProviderTransitionIntentRecord) -> u64 {
    witness::read(path, |c| {
        c.query_row(
            "SELECT remaining FROM reservations WHERE id=?1",
            [stage.reservation_operation().unwrap()],
            |r| r.get(0),
        )
        .unwrap()
    })
}
fn expected_protection(
    stage: &ProviderTransitionIntentRecord,
    required: &[FileManifest],
) -> ExactProtection {
    let proof = stage.destination.as_ref().unwrap();
    let shared = proof.format.shared.as_ref().unwrap();
    ExactProtection::new(
        &shared.incarnation,
        &proof.format.identity.worktree_id,
        &proof.private_database_identity,
        &stage.transaction_id,
        required,
        None,
    )
    .unwrap()
}
fn require_protected(
    path: &Path,
    stage: &ProviderTransitionIntentRecord,
    required: &[FileManifest],
) {
    witness::read(path, |c| {
        assert_eq!(
            expected_protection(stage, required)
                .inspect(c, None)
                .unwrap(),
            ProtectionPresence::Present
        );
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM roots WHERE owner=?1",
                [stage
                    .destination
                    .as_ref()
                    .unwrap()
                    .format
                    .identity
                    .worktree_id
                    .as_str()],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
            1
        );
    });
}
fn root_only(
    before: &Database,
    after: &Database,
    stage: &ProviderTransitionIntentRecord,
    required: &[FileManifest],
) {
    let root = expected_protection(stage, required);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    for (table, rows) in before {
        if table == "roots" || table == "root_manifests" {
            for row in rows {
                assert!(after[table].contains(row), "retained root changed");
            }
        } else {
            assert_eq!(&after[table], rows, "protection changed {table}");
        }
    }
    assert_eq!(after["roots"].len(), before["roots"].len() + 1);
    assert_eq!(
        after["root_manifests"].len(),
        before["root_manifests"].len() + required.len()
    );
    let id = Cell::Text(root.id().as_bytes().to_vec());
    for row in &after["roots"] {
        if !before["roots"].contains(row) {
            assert_eq!(row[0], id);
            assert_eq!(row[5], Cell::Blob(root.payload().to_vec()));
        }
    }
    let expected: BTreeSet<_> = required.iter().map(|m| m.id().as_str()).collect();
    let actual: BTreeSet<_> = after["root_manifests"]
        .iter()
        .filter(|r| r[0] == id)
        .map(|r| match &r[1] {
            Cell::Text(v) => std::str::from_utf8(v).unwrap(),
            _ => panic!("manifest id must be text"),
        })
        .collect();
    assert_eq!(actual, expected);
}
fn database_only(before: &(Tree, Tree, Tree), after: &(Tree, Tree, Tree), path: &Path) {
    assert_eq!(before.0, after.0);
    assert_eq!(before.2, after.2);
    assert_eq!(
        before.1.keys().collect::<Vec<_>>(),
        after.1.keys().collect::<Vec<_>>()
    );
    assert!(before.1.contains_key(path));
    for (name, node) in &before.1 {
        let next = &after.1[name];
        if name == path {
            let (Node::File((old, _, a)), Node::File((new, _, b))) = (node, next) else {
                panic!("bound database must remain file")
            };
            assert_eq!(old, new);
            assert_ne!(a, b);
        } else {
            assert_eq!(node, next, "unexpected filesystem effect {name:?}");
        }
    }
}
fn checkout_only(before: &Database, after: &Database) {
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    for (table, rows) in before {
        if table != "projection_prejournal" {
            assert_eq!(
                &after[table], rows,
                "checkout changed copied history/table {table}"
            );
        }
    }
}
fn require_unready(
    family_database: &Path,
    destination_database: &Path,
    stage: &ProviderTransitionIntentRecord,
) {
    witness::destination(destination_database, stage);
    witness::read(family_database, |c| {
        let owner = stage
            .destination
            .as_ref()
            .unwrap()
            .format
            .identity
            .worktree_id
            .as_str();
        assert_eq!(
            c.query_row("SELECT state FROM worktrees WHERE id=?1", [owner], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            "initializing"
        );
        assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",[owner],|r|r.get::<_,bool>(0)).unwrap());
    });
}

#[test]
#[ignore = "ROOT source release and audited owned native child2400/outer2460 required"]
fn managed_ordinary_checkout_protects_complete_union_and_replays_declared_close_ack() {
    // One genuine admission, one capture, complete exact installation, protection
    // and journal-first checkout. r35 alone measured 694.993s; Installing r38 is
    // still being measured. child2400/outer2460 is a proposed qualification bound.
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_checkout_protects_complete_union_and_replays_declared_close_ack"
        ),
        2400,
        || {
            child::declare();
            let root = super::managed_restore_recovery_tests::admitted_destination();
            let store = ProjectionStore::open(&root).unwrap();
            let state = store.state().unwrap();
            let path = RepoPath::parse("README.md").unwrap();
            let base_id = match state
                .snapshot
                .entries
                .iter()
                .find(|e| e.path == path)
                .unwrap()
                .target
                .as_ref()
                .unwrap()
            {
                TreeEntryTarget::File { file_id } => file_id.clone(),
                _ => panic!("selected README file required"),
            };
            fs::write(root.join(path.as_str()), HISTORY).unwrap();
            store.capture_patch(state.generation).unwrap();
            drop(store);
            let store = ProjectionStore::open(&root).unwrap();
            assert_eq!(store.state().unwrap(), state);
            fs::write(root.join(path.as_str()), CURRENT).unwrap();
            let family = store.marker.shared.as_ref().unwrap().root.clone();
            let source_database = store.marker.metadata_path(&root).join("projection.sqlite3");
            let original_b = witness::source(&source_database, &store.marker, None);
            let source_lock = source_database.parent().unwrap().join("layrs.lock");
            let mut identity = store.identity().clone();
            identity.worktree_id =
                WorktreeId::parse("worktree:ordinary-checkout-destination").unwrap();
            let cancel = Cancellation::default();
            let mut owner = store
                .begin_managed_export(state.generation, identity, &cancel)
                .unwrap();
            let current = owner.current_manifest(&path).unwrap().clone();
            let history = owner
                .retained_manifest(&FileId::from_bytes(HISTORY))
                .unwrap()
                .clone();
            let base = owner.retained_manifest(&base_id).unwrap().clone();
            assert!(owner.retained_manifest(current.file_id()).is_none());
            let (planned, effect) = owner
                .begin_plan(root.parent().unwrap(), LEAF, TX, &cancel)
                .unwrap();
            assert_eq!(effect, SourceIntentEffect::Inserted);
            let derived = owner
                .derive_plan(
                    &planned.plan.destination_parent,
                    &planned.plan.destination_parent_identity,
                    LEAF,
                    TX,
                    &cancel,
                )
                .unwrap();
            assert_eq!(derived.plan, planned.plan);
            let required: Vec<_> = derived
                .required_manifests
                .iter()
                .map(|row| {
                    let manifest = if row.manifest_id == *current.id() {
                        current.clone()
                    } else {
                        owner.retained_manifest(&row.file_id).unwrap().clone()
                    };
                    assert_eq!(*manifest.id(), row.manifest_id);
                    manifest
                })
                .collect();
            assert_eq!(
                required
                    .iter()
                    .map(FileManifest::id)
                    .collect::<BTreeSet<_>>(),
                [base.id(), history.id(), current.id()]
                    .into_iter()
                    .collect::<BTreeSet<_>>()
            );
            assert_eq!(required.len(), 3);
            let files = [CheckoutFile {
                path: path.clone(),
                manifest_id: current.id().clone(),
                file_id: current.file_id().clone(),
                bytes: current.byte_length(),
                executable: false,
            }];
            let image = owner.image_digest().to_owned();
            let mut allocated = allocation::allocate(&mut owner, &planned, &cancel)
                .unwrap_or_else(|e| panic!("checkout allocation {e:?}"));
            let (copying, effect) = owner
                .commit_allocation(&planned, &allocated, &cancel)
                .unwrap();
            assert_eq!(effect, SourceIntentEffect::Updated);
            allocated
                .publish_format(&mut owner, &copying, &cancel)
                .unwrap();
            assert_eq!(
                owner
                    .register_allocation(&allocated, &copying, &cancel)
                    .unwrap(),
                ImportRegistrationEffect::Inserted
            );
            let mut setup = Close::default();
            assert_eq!(
                owner
                    .copy_metadata(&allocated, &copying, &mut setup, &cancel)
                    .unwrap(),
                CopyEffect::Copied
            );
            let outcome = owner
                .stage_metadata(&allocated, &copying, &mut setup, &cancel)
                .unwrap();
            assert_eq!(outcome.source, SourceIntentEffect::Updated);
            assert_eq!(outcome.destination, StageEffect::Updated);
            let staging = outcome.intent;
            assert_eq!(staging.state, TransitionState::Staging);
            assert_eq!(
                owner
                    .reserve_content(&allocated, &staging, &mut setup, &cancel)
                    .unwrap(),
                ExportReservationEffect::Inserted
            );
            for manifest in &required {
                assert_eq!(
                    owner
                        .install_content(&allocated, &staging, manifest.id(), &mut setup, &cancel)
                        .unwrap(),
                    *manifest
                );
            }
            let destination_root = allocated.root().to_owned();
            let destination_database = allocated.destination_database().path().to_owned();
            let destination_lock = destination_database.parent().unwrap().join("layrs.lock");
            let locks = [source_lock.as_path(), destination_lock.as_path()];
            let family_database = family.join("metadata/catalog.sqlite3");
            let before_family = snapshot(&family_database);
            let operation = Cell::Text(staging.reservation_operation().unwrap().into_bytes());
            let installed: Vec<_> = before_family["exact_installations"]
                .iter()
                .filter(|row| row[0] == operation)
                .collect();
            assert_eq!(installed.len(), required.len());
            assert!(
                installed.iter().all(|row| row[6] == Cell::Integer(1)),
                "full C union must already be Published"
            );
            let before_c = witness::destination(&destination_database, &staging);
            let before_b = witness::source(&source_database, &store.marker, Some(&staging));
            for table in [
                "projection_managed_admission",
                "projection_captures",
                "projection_restore_history",
            ] {
                assert_eq!(before_b[table], original_b[table]);
            }
            witness::verify_objects(&family, &required);
            let expected = ExpectedCheckout::ordinary(&staging, &files, &[], &image, None).unwrap();
            assert!(
                expected.deletions().is_empty(),
                "single selected-leaf fixture has no deletion scenario"
            );
            let before = witness::trees(&root, &family, &destination_root, locks);
            let q = remaining(&family_database, &staging);
            assert!(q > 0 && q < staging.plan.quota_bytes);
            let current_chunk =
                witness::object(&family, "chunks", current.chunks()[0].chunk_id().as_str());
            let mut close = Close {
                protected_chunk: Some(current_chunk.clone()),
                ..Close::default()
            };
            let cancelled = Cancellation::default();
            cancelled.cancel();
            assert!(
                owner
                    .protect_content(&allocated, &staging, &mut close, &cancelled)
                    .is_err()
            );
            assert_eq!(close.counts(), (0, 0, 0, 0));
            assert_eq!(
                witness::trees(&root, &family, &destination_root, locks),
                before
            );
            let protected = owner
                .protect_content(&allocated, &staging, &mut close, &cancel)
                .unwrap();
            assert_eq!(close.counts(), (1, 1, 1, 0));
            assert_eq!(close.pin_checks, 1);
            require_write_blocked(&current_chunk);
            require_protected(&family_database, &staging, &required);
            let protected_family = snapshot(&family_database);
            root_only(&before_family, &protected_family, &staging, &required);
            assert_eq!(remaining(&family_database, &staging), q);
            let protected_trees = witness::trees(&root, &family, &destination_root, locks);
            database_only(&before, &protected_trees, &family_database);
            assert_eq!(
                witness::source(&source_database, &store.marker, Some(&staging)),
                before_b
            );
            assert_eq!(
                witness::destination(&destination_database, &staging),
                before_c
            );
            let replay = owner
                .protect_content(&allocated, &staging, &mut close, &cancel)
                .unwrap();
            assert_eq!(close.counts(), (1, 1, 1, 0));
            assert_eq!(
                witness::trees(&root, &family, &destination_root, locks),
                protected_trees
            );
            drop(replay);
            assert!(
                owner
                    .copy_checkout(&allocated, &staging, &protected, &mut close, &cancelled)
                    .is_err()
            );
            assert_eq!(close.counts(), (1, 1, 1, 0));
            assert_eq!(
                witness::trees(&root, &family, &destination_root, locks),
                protected_trees
            );
            close.withhold = true;
            close.withheld_namespace = Some((
                destination_root.join(expected.stage_relative_path()),
                destination_root.join(path.as_str()),
            ));
            let after_ack = close.after_withheld_ack.clone();
            let later_probes = Rc::new(Flag::new(0usize));
            let observed_later = later_probes.clone();
            let events = Rc::new(RefCell::new(Vec::new()));
            let observed_events = events.clone();
            let failed = with_observer(
                move |path, point| {
                    observed_events.borrow_mut().push((path.to_owned(), point));
                    if after_ack.get() {
                        observed_later.set(observed_later.get() + 1);
                        return Err(invalid(
                            "failed checkout Declare ACK forbids later observed SQL/identity/snapshot probes",
                        ));
                    }
                    Ok(())
                },
                || owner.copy_checkout(&allocated, &staging, &protected, &mut close, &cancel),
            );
            let error = match failed {
                Ok(_) => panic!("Declare close ACK was not withheld"),
                Err(e) => e,
            };
            assert!(error.to_string().contains(WITHHELD_ACK), "{error}");
            let crate::projection::ProjectionStoreError::ProviderContent(cause) = &error else {
                panic!("retained checkout must preserve its structured cause: {error:?}")
            };
            require_ack_primary(cause, WITHHELD_ACK);
            assert_eq!(
                later_probes.get(),
                0,
                "only mandatory cleanup may follow the withheld ACK"
            );
            require_ready_read_controls(&events.borrow(), &destination_database, &family_database);
            assert_eq!(close.counts(), (2, 2, 2, 1));
            assert_eq!(close.pin_checks, 2);
            let declared_trees = witness::trees(&root, &family, &destination_root, locks);
            database_only(&protected_trees, &declared_trees, &destination_database);
            assert_eq!(snapshot(&family_database), protected_family);
            assert_eq!(remaining(&family_database, &staging), q);
            let declared = witness::destination(&destination_database, &staging);
            checkout_only(&before_c, &declared);
            witness::read(&destination_database, |c| {
                let journal = checkout_journal::read(c, &expected, None).unwrap();
                assert_eq!(journal.rows().len(), 3);
                assert!(!journal.is_complete());
                for row in journal.rows() {
                    match &row.entry {
                        CheckoutEntry::Header { .. } => {}
                        CheckoutEntry::Stage { state } | CheckoutEntry::Directory { state, .. } => {
                            assert_eq!(*state, DirectoryState::Declared)
                        }
                        CheckoutEntry::File { state, .. } => {
                            assert_eq!(*state, FileState::Declared)
                        }
                    }
                }
            });
            assert!(
                !destination_root
                    .join(expected.stage_relative_path())
                    .exists()
            );
            assert!(!destination_root.join(path.as_str()).exists());
            assert_eq!(
                witness::source(&source_database, &store.marker, Some(&staging)),
                before_b
            );
            // The explicit same-owner read-only reconstruction accepts the durable
            // journal; it neither opens a writer nor tries owner-loss recovery.
            let reopened = owner
                .reopen_content_protection(&allocated, &staging, &mut close, &cancel)
                .unwrap();
            assert_eq!(close.counts(), (2, 2, 2, 1));
            assert_eq!(
                witness::trees(&root, &family, &destination_root, locks),
                declared_trees
            );
            drop(protected);
            require_write_blocked(&current_chunk);
            let checkout = owner
                .copy_checkout(&allocated, &staging, &reopened, &mut close, &cancel)
                .unwrap();
            assert!(close.opens > 2);
            assert_eq!(close.opens, close.raw);
            assert_eq!(close.raw, close.closes);
            assert_eq!(close.withheld, 1);
            assert_eq!(close.pin_checks, close.opens);
            assert_eq!(snapshot(&family_database), protected_family);
            assert_eq!(remaining(&family_database, &staging), q);
            require_protected(&family_database, &staging, &required);
            require_unready(&family_database, &destination_database, &staging);
            assert_eq!(
                fs::read(destination_root.join(path.as_str())).unwrap(),
                CURRENT
            );
            assert_eq!(
                witness::source(&source_database, &store.marker, Some(&staging)),
                before_b
            );
            let complete_c = witness::destination(&destination_database, &staging);
            checkout_only(&before_c, &complete_c);
            let complete = witness::trees(&root, &family, &destination_root, locks);
            assert_eq!(complete.0, declared_trees.0);
            // C's private DB is the only changed preexisting family file. The
            // family CAS, protections and every other node remain byte-identical.
            assert_eq!(
                complete.1.keys().collect::<Vec<_>>(),
                declared_trees.1.keys().collect::<Vec<_>>()
            );
            for (name, node) in &declared_trees.1 {
                if name != &destination_database {
                    assert_eq!(&complete.1[name], node);
                } else {
                    let (Node::File((a, _, _)), Node::File((b, _, _))) = (node, &complete.1[name])
                    else {
                        panic!("private C DB must remain file")
                    };
                    assert_eq!(a, b);
                }
            }
            let stage_path = destination_root.join(expected.stage_relative_path());
            let final_path = destination_root.join(path.as_str());
            let expected_new = BTreeSet::from([stage_path.clone(), final_path.clone()]);
            let actual_new = complete
                .2
                .keys()
                .filter(|p| !declared_trees.2.contains_key(*p))
                .cloned()
                .collect::<BTreeSet<_>>();
            assert_eq!(actual_new, expected_new);
            for (name, node) in &declared_trees.2 {
                assert_eq!(&complete.2[name], node);
            }
            witness::read(&destination_database, |c| {
                let journal = checkout_journal::read(c, &expected, None).unwrap();
                assert!(journal.is_complete());
                let CheckoutEntry::Stage {
                    state: DirectoryState::Owned { identity },
                } = &journal.row(RowKey::Stage).unwrap().entry
                else {
                    panic!("owned checkout stage required")
                };
                let Node::Directory(actual) = &complete.2[&stage_path] else {
                    panic!("stage must be directory")
                };
                assert_eq!(identity, actual);
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
                    panic!("Published checkout file required")
                };
                assert_eq!(file, &files[0]);
                assert_eq!(parent_identity, &expected.header().checkout_identity);
                let Node::File((actual, bytes, _)) = &complete.2[&final_path] else {
                    panic!("final file required")
                };
                assert_eq!(inode, actual);
                assert_eq!(*bytes, CURRENT.len() as u64);
            });
            assert!(
                fs::read_dir(&stage_path).unwrap().next().is_none(),
                "complete single-file checkout leaves no TEMP"
            );
            witness::verify_objects(&family, &required);
            require_write_blocked(&current_chunk);
            let counts = close.counts();
            let pins = close.pin_checks;
            let replay = owner
                .copy_checkout(&allocated, &staging, &reopened, &mut close, &cancel)
                .unwrap();
            let replay_protection = owner
                .reopen_content_protection(&allocated, &staging, &mut close, &cancel)
                .unwrap();
            assert_eq!(close.counts(), counts);
            assert_eq!(close.pin_checks, pins);
            assert_eq!(
                witness::trees(&root, &family, &destination_root, locks),
                complete
            );
            assert_eq!(snapshot(&family_database), protected_family);
            assert_eq!(
                witness::destination(&destination_database, &staging),
                complete_c
            );
            assert_eq!(
                witness::source(&source_database, &store.marker, Some(&staging)),
                before_b
            );
            drop(replay);
            drop(checkout);
            drop(replay_protection);
            drop(reopened);
            owner.close().unwrap();
            assert!(ProjectionStore::open(&root).is_err());
            allocated.require_published(&staging.plan).unwrap();
            drop(allocated);
            assert!(
                ProjectionStore::open(&destination_root).is_err(),
                "C remains unready C8 despite complete bytes"
            );
            assert_eq!(
                witness::trees(&root, &family, &destination_root, locks),
                complete
            );
            drop(store);
        },
    );
}
