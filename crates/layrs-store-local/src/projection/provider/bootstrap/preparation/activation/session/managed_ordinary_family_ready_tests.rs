//! Genuine ordinary family publication after private C8 Ready. Only C's family
//! member changes Initializing -> Ready; B remains Staging and C unadmitted.
//! Separate fresh fixtures cover acknowledged Updated with durable readback and
//! withheld ACK replay. ACK withholding follows SQLite connection-close success;
//! native xClose/OS-close outcomes, positive deletion/nested checkout, crashes
//! and owner-loss recovery remain unqualified.
use super::{
    managed_ordinary_installing_tests::{self as witness, Cell, Database, Node, Tree},
    managed_ordinary_ready_tests::{self as ready_witness, CURRENT, Physical},
};
use crate::projection::{
    ProjectionStore,
    provider::{
        checkout_journal::{CheckoutFile, ExpectedCheckout},
        database_io::{
            WriterCloseObserver, WriterOpenPoint,
            tests::fresh_ordinary_support::{ReadBoundary, with_observer},
        },
        destination_ready::ReadyEffect,
        managed_export::allocation,
        ordinary_destination::{CopyEffect, staging::StageEffect},
        process_profile::tests::child,
        records::transition::{ProviderTransitionIntentRecord, TransitionState},
        recovery::error::{ProviderBootstrapCause as Cause, Result as CauseResult, invalid},
        transition_sql::SourceIntentEffect,
    },
    selection,
    shared::{
        admitted_catalog::{ExportPublicationEffect, ExportReservationEffect},
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
    fs,
    path::Path,
    rc::Rc,
};

const LEAF: &str = "00000000000000000000000000000111";
const TX: &str = "00000000000000000000000000000112";
const HISTORY: &[u8] = b"ordinary family publication historical-only capture";
const WITHHELD_ACK: &str = "ordinary family Ready successful close acknowledgement withheld";

#[derive(Default)]
pub(super) struct Close {
    opens: usize,
    raw: usize,
    closes: usize,
    withheld: usize,
    withhold: bool,
    physical: Option<Physical>,
    physical_checks: usize,
    after_close: Rc<Flag<bool>>,
}
impl Close {
    fn counts(&self) -> (usize, usize, usize, usize) {
        (self.opens, self.raw, self.closes, self.withheld)
    }
}
impl WriterCloseObserver for Close {
    fn open_point(&mut self, point: WriterOpenPoint, raw: *mut ffi::sqlite3) -> CauseResult<()> {
        match point {
            WriterOpenPoint::BeforeRaw => {
                assert!(raw.is_null());
                self.opens += 1;
                if let Some(physical) = &self.physical {
                    physical.require();
                    self.physical_checks += 1;
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
            "family observer requires SQLite connection-close success, without native xClose/OS-close inference"
        );
        self.closes += 1;
        if let Some(physical) = &self.physical {
            physical.require();
            self.physical_checks += 1;
        }
        self.after_close.set(true);
        if std::mem::take(&mut self.withhold) {
            self.withheld += 1;
            Some(invalid(WITHHELD_ACK))
        } else {
            None
        }
    }
}

fn snapshot(path: &Path) -> Database {
    witness::read(path, witness::database)
}
pub(super) fn text(value: &str) -> Cell {
    Cell::Text(value.as_bytes().to_vec())
}
pub(super) fn copy_cell(cell: &Cell) -> Cell {
    match cell {
        Cell::Null => Cell::Null,
        Cell::Integer(v) => Cell::Integer(*v),
        Cell::Real(v) => Cell::Real(*v),
        Cell::Text(v) => Cell::Text(v.clone()),
        Cell::Blob(v) => Cell::Blob(v.clone()),
    }
}

pub(super) fn family(
    path: &Path,
    stage: &ProviderTransitionIntentRecord,
    required: &[FileManifest],
    state: &str,
) -> Database {
    witness::read(path, |c| {
        let proof = stage.destination.as_ref().unwrap();
        let shared = proof.format.shared.as_ref().unwrap();
        let registration: (String, String, Vec<u8>, String) = c
            .query_row(
                "SELECT metadata_path,database_identity,marker,state FROM worktrees WHERE id=?1",
                [proof.format.identity.worktree_id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            registration,
            (
                shared.metadata_path.to_str().unwrap().into(),
                proof.private_database_identity.clone(),
                proof.format.bytes().unwrap(),
                state.into()
            )
        );
        let protection = ExactProtection::new(
            &shared.incarnation,
            &proof.format.identity.worktree_id,
            &proof.private_database_identity,
            &stage.transaction_id,
            required,
            None,
        )
        .unwrap();
        assert_eq!(
            protection.inspect(c, None).unwrap(),
            ProtectionPresence::Present
        );
        assert!(
            !c.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
        );
        witness::database(c)
    })
}

fn only_member_state_changed(
    before: &Database,
    after: &Database,
    stage: &ProviderTransitionIntentRecord,
) {
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    let proof = stage.destination.as_ref().unwrap();
    let shared = proof.format.shared.as_ref().unwrap();
    let mut expected_member = vec![
        text(proof.format.identity.worktree_id.as_str()),
        text(shared.metadata_path.to_str().unwrap()),
        text(&proof.private_database_identity),
        Cell::Blob(proof.format.bytes().unwrap()),
        text("initializing"),
    ];
    assert_eq!(
        before["worktrees"]
            .iter()
            .filter(|row| row.first() == expected_member.first())
            .collect::<Vec<_>>(),
        vec![&expected_member]
    );
    for (table, rows) in before {
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
            "only exact C registry state may change"
        );
    }
}

fn only_catalog_changed(before: &(Tree, Tree, Tree), after: &(Tree, Tree, Tree), catalog: &Path) {
    assert_eq!(
        before.0, after.0,
        "B checkout changed during family publication"
    );
    assert_eq!(
        before.2, after.2,
        "C checkout changed during family publication"
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

#[test]
#[ignore = "ROOT source release and audited owned native child3000/outer3060 required"]
fn managed_ordinary_family_ready_replays_withheld_catalog_close_ack_without_promoting_b_or_admitting_c()
 {
    // Same owned child3000/outer3060 proposal as private Ready. This recipe
    // reaches that frontier normally and withholds only the new family ACK.
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_family_ready_replays_withheld_catalog_close_ack_without_promoting_b_or_admitting_c"
        ),
        3000,
        || family_ready_recipe(true),
    );
}

#[test]
#[ignore = "ROOT source release and audited owned native child3000/outer3060 required"]
fn managed_ordinary_family_ready_updates_then_replays_read_only_without_promoting_b_or_admitting_c()
{
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_family_ready_updates_then_replays_read_only_without_promoting_b_or_admitting_c"
        ),
        3000,
        || family_ready_recipe(false),
    );
}

fn family_ready_recipe(withhold_close_ack: bool) {
    child::declare();

    // This helper creates a fresh owned fixture and genuinely admits B9.
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
        _ => panic!("selected README required"),
    };

    fs::write(root.join(path.as_str()), HISTORY).unwrap();
    store.capture_patch(state.generation).unwrap();
    drop(store);
    let store = ProjectionStore::open(&root).unwrap();
    assert_eq!(store.state().unwrap(), state);
    fs::write(root.join(path.as_str()), CURRENT).unwrap();

    let family_root = store.marker.shared.as_ref().unwrap().root.clone();
    let source_database = store.marker.metadata_path(&root).join("projection.sqlite3");
    let source_lock = source_database.parent().unwrap().join("layrs.lock");
    let mut identity = store.identity().clone();
    identity.worktree_id = WorktreeId::parse("worktree:ordinary-family-ready-destination").unwrap();
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
    assert_eq!(current.chunks().len(), 1);

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
    let required: Vec<FileManifest> = derived
        .required_manifests
        .iter()
        .map(|row| {
            let m = if row.manifest_id == *current.id() {
                current.clone()
            } else {
                owner.retained_manifest(&row.file_id).unwrap().clone()
            };
            assert_eq!(*m.id(), row.manifest_id);
            assert_eq!(m.byte_length(), row.bytes);
            m
        })
        .collect();
    assert_eq!(required.len(), 3);
    assert_eq!(
        required
            .iter()
            .map(FileManifest::id)
            .collect::<BTreeSet<_>>(),
        [base.id(), history.id(), current.id()]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    let image = owner.image_digest().to_owned();
    let files = [CheckoutFile {
        path: path.clone(),
        manifest_id: current.id().clone(),
        file_id: current.file_id().clone(),
        bytes: current.byte_length(),
        executable: false,
    }];

    let mut allocated = allocation::allocate(&mut owner, &planned, &cancel)
        .unwrap_or_else(|e| panic!("family Ready allocation {e:?}"));
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
    let staged = owner
        .stage_metadata(&allocated, &copying, &mut setup, &cancel)
        .unwrap();
    assert_eq!(staged.source, SourceIntentEffect::Updated);
    assert_eq!(staged.destination, StageEffect::Updated);
    let staging = staged.intent;
    assert_eq!(staging.state, TransitionState::Staging);
    assert_eq!(
        owner
            .reserve_content(&allocated, &staging, &mut setup, &cancel)
            .unwrap(),
        ExportReservationEffect::Inserted
    );
    for m in &required {
        assert_eq!(
            owner
                .install_content(&allocated, &staging, m.id(), &mut setup, &cancel)
                .unwrap(),
            *m
        );
    }
    let protection = owner
        .protect_content(&allocated, &staging, &mut setup, &cancel)
        .unwrap();
    let checkout = owner
        .copy_checkout(&allocated, &staging, &protection, &mut setup, &cancel)
        .unwrap();

    let destination_root = allocated.root().to_owned();
    let destination_database = allocated.destination_database().path().to_owned();
    let destination_lock = destination_database.parent().unwrap().join("layrs.lock");
    let locks = [source_lock.as_path(), destination_lock.as_path()];
    let family_database = family_root.join("metadata/catalog.sqlite3");
    let expected = ExpectedCheckout::ordinary(&staging, &files, &[], &image, None).unwrap();
    assert!(
        expected.deletions().is_empty(),
        "single README fixture does not qualify positive deletion/nested checkout"
    );
    let staged_c = witness::destination(&destination_database, &staging);
    let selection_payload = witness::read(&source_database, |c| {
        selection::encode(&selection::at(c, staging.local_generation).unwrap()).unwrap()
    });

    // Normal setup only: the new target boundary starts after actual private C
    // Ready, without replaying private Ready's separate ACK-withholding test.
    let private_ready = owner
        .finalize_destination_ready(
            &allocated,
            &staging,
            &protection,
            &checkout,
            &mut setup,
            &cancel,
        )
        .unwrap();
    assert_eq!(private_ready.effect(), ReadyEffect::Finalized);
    let durable = ready_witness::require_ready_rows(
        &destination_database,
        &staged_c,
        &staging,
        &derived,
        &selection_payload,
        ready_witness::ReadyContent {
            base: &base,
            history: &history,
            current: &current,
        },
        &required,
        &expected,
    );
    assert_eq!(private_ready.ready(), &durable.ready);
    assert_eq!(private_ready.receipt(), &durable.receipt);
    drop(private_ready);

    let before_b = witness::source(&source_database, &store.marker, Some(&staging));
    let before_c = durable.rows;
    let before_family = family(&family_database, &staging, &required, "initializing");
    let before = witness::trees(&root, &family_root, &destination_root, locks);
    let final_path = destination_root.join(path.as_str());
    let Node::File((inode, _, _)) = &before.2[&final_path] else {
        panic!("complete checkout final file required")
    };
    let physical = Physical {
        file: final_path,
        inode: inode.clone(),
        cas: witness::object(
            &family_root,
            "chunks",
            current.chunks()[0].chunk_id().as_str(),
        ),
    };
    physical.require();
    witness::verify_objects(&family_root, &required);
    let mut close = Close {
        physical: Some(physical),
        ..Close::default()
    };
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(
        owner
            .publish_destination_family_ready(
                &allocated,
                &staging,
                &protection,
                &checkout,
                &mut close,
                &cancelled,
            )
            .is_err()
    );
    assert_eq!(close.counts(), (0, 0, 0, 0));
    assert_eq!(close.physical_checks, 0);
    assert_eq!(
        witness::trees(&root, &family_root, &destination_root, locks),
        before
    );
    assert_eq!(snapshot(&source_database), before_b);
    assert_eq!(snapshot(&destination_database), before_c);
    assert_eq!(snapshot(&family_database), before_family);

    close.withhold = withhold_close_ack;
    let after_close = close.after_close.clone();
    let after_close_probes = Rc::new(Flag::new(0usize));
    let observed_after_close = after_close_probes.clone();
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    let post_close_events = Rc::new(RefCell::new(Vec::new()));
    let observed_post_close = post_close_events.clone();
    let publication = with_observer(
        move |path, point| {
            observed.borrow_mut().push((path.to_owned(), point));
            if after_close.get() {
                observed_post_close
                    .borrow_mut()
                    .push((path.to_owned(), point));
                if withhold_close_ack {
                    observed_after_close.set(observed_after_close.get() + 1);
                    return Err(invalid(
                        "failed retained family Ready ACK forbids later observed SQL/identity/snapshot probes",
                    ));
                }
            }
            Ok(())
        },
        || {
            owner.publish_destination_family_ready(
                &allocated,
                &staging,
                &protection,
                &checkout,
                &mut close,
                &cancel,
            )
        },
    );
    ready_witness::require_ready_read_controls(
        &events.borrow(),
        &destination_database,
        &family_database,
    );
    let original_error = if withhold_close_ack {
        let error = match publication {
            Ok(_) => panic!("family Ready close ACK was not withheld"),
            Err(error) => error,
        };
        assert!(error.to_string().contains(WITHHELD_ACK), "{error}");
        assert_eq!(
            after_close_probes.get(),
            0,
            "only mandatory rollback/reader closes follow the failed ACK"
        );
        assert!(post_close_events.borrow().is_empty());
        let primary = error.to_string();
        Some((error, primary))
    } else {
        // Returning Updated requires the production path's fresh catalog
        // readback after COMMIT and positively acknowledged SQLite close.
        let outcome = publication.unwrap();
        assert_eq!(outcome.effect(), ExportPublicationEffect::Updated);
        assert_eq!(outcome.ready(), &durable.ready);
        assert_eq!(outcome.receipt(), &durable.receipt);
        assert!(post_close_events.borrow().iter().any(|(path, point)| {
            path == &family_database && *point == ReadBoundary::BeforeSnapshot
        }));
        assert!(post_close_events.borrow().iter().any(|(path, point)| {
            path == &destination_database && *point == ReadBoundary::DatabaseSnapshot
        }));
        None
    };
    let expected_closes = (1, 1, 1, usize::from(withhold_close_ack));
    assert_eq!(close.counts(), expected_closes);
    assert_eq!(close.physical_checks, 2);

    let committed = witness::trees(&root, &family_root, &destination_root, locks);
    only_catalog_changed(&before, &committed, &family_database);
    assert_eq!(
        witness::source(&source_database, &store.marker, Some(&staging)),
        before_b
    );
    assert_eq!(snapshot(&destination_database), before_c);
    let published = family(&family_database, &staging, &required, "ready");
    // Every other row, including exact journals, charged bitsets, debits,
    // reservations and the Protected C root, must remain byte-for-byte equal.
    only_member_state_changed(&before_family, &published, &staging);
    close.physical.as_ref().unwrap().require();
    witness::verify_objects(&family_root, &required);

    let outcome = owner
        .publish_destination_family_ready(
            &allocated,
            &staging,
            &protection,
            &checkout,
            &mut close,
            &cancel,
        )
        .unwrap();
    assert_eq!(outcome.effect(), ExportPublicationEffect::AlreadyReady);
    assert_eq!(outcome.ready(), &durable.ready);
    assert_eq!(outcome.receipt(), &durable.receipt);
    assert_eq!(close.counts(), expected_closes);
    assert_eq!(close.physical_checks, 2);
    if let Some((error, primary)) = &original_error {
        assert_eq!(
            error.to_string(),
            *primary,
            "replay preserves original family ACK error"
        );
    }
    assert_eq!(
        witness::trees(&root, &family_root, &destination_root, locks),
        committed
    );
    assert_eq!(snapshot(&source_database), before_b);
    assert_eq!(snapshot(&destination_database), before_c);
    assert_eq!(
        family(&family_database, &staging, &required, "ready"),
        published
    );
    close.physical.as_ref().unwrap().require();

    drop(outcome);
    drop(checkout);
    drop(protection);
    owner.close().unwrap();
    assert!(ProjectionStore::open(&root).is_err());
    allocated.require_published(&staging.plan).unwrap();
    drop(allocated);
    assert!(
        ProjectionStore::open(&destination_root).is_err(),
        "family Ready and private C8 Ready remain unadmitted without C9 admission"
    );
    assert_eq!(
        witness::trees(&root, &family_root, &destination_root, locks),
        committed
    );
    assert_eq!(snapshot(&source_database), before_b);
    assert_eq!(snapshot(&destination_database), before_c);
    assert_eq!(snapshot(&family_database), published);
    drop(store);
}
