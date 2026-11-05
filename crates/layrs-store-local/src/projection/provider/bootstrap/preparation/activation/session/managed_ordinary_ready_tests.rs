//! Genuine private C8 Ready while B remains Staging and C's family member is
//! Initializing. An ACK is withheld after SQLite connection-close success;
//! native xClose/OS close, family publication, admission and owner loss remain
//! outside the qualification of this recipe.
use super::managed_ordinary_installing_tests::{self as witness, Cell, Database, Node, Tree};
use crate::projection::{
    ProjectionStore,
    capture::scan::executable,
    database,
    provider::{
        bootstrap::ManagedPlanData,
        checkout_journal::{self, CheckoutFile, ExpectedCheckout},
        database_io::{
            self, WriterCloseObserver, WriterOpenPoint,
            tests::fresh_ordinary_support::{ReadBoundary, with_observer},
        },
        destination_ready::ReadyEffect,
        managed_export::allocation,
        ordinary_destination::{CopyEffect, staging::StageEffect},
        process_profile::tests::child,
        records::{
            Record,
            import::{ImportSource, ProviderTransitionReceiptRecord},
            transition::{
                ProviderTransitionIntentRecord, TransitionRole, TransitionRowData, TransitionState,
            },
        },
        recovery::error::{ProviderBootstrapCause as Cause, Result as CauseResult, invalid},
        transition_sql::SourceIntentEffect,
    },
    selection,
    shared::{
        admitted_catalog::ExportReservationEffect,
        exact_protection::{ExactProtection, ProtectionPresence},
        provider_access::ImportRegistrationEffect,
    },
};
use layrs_core::scm::{
    FileId, FileManifest, RepoPath, TreeEntryTarget, WorktreeId, canonical_json,
};
use layrs_vfs::Cancellation;
use rusqlite::ffi;
use std::{
    cell::{Cell as Flag, RefCell},
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    rc::Rc,
};

const LEAF: &str = "00000000000000000000000000000101";
const TX: &str = "00000000000000000000000000000102";
const HISTORY: &[u8] = b"ordinary Ready historical-only capture";
pub(super) const CURRENT: &[u8] = b"ordinary Ready uncaptured current bytes";
const WITHHELD_ACK: &str = "private C Ready successful close acknowledgement withheld";

pub(super) struct Physical {
    pub(super) file: PathBuf,
    pub(super) inode: String,
    pub(super) cas: PathBuf,
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
        "live immutable proof must forbid WRITE"
    );
}
impl Physical {
    pub(super) fn require(&self) {
        assert_eq!(fs::read(&self.file).unwrap(), CURRENT);
        let metadata = fs::symlink_metadata(&self.file).unwrap();
        assert!(metadata.is_file());
        assert!(!metadata.file_type().is_symlink());
        assert!(!executable(&metadata));
        let actual = database_io::tests::support::witness(&self.file);
        assert_eq!(actual.0, self.inode);
        assert_eq!(actual.1, CURRENT.len() as u64);
        require_write_blocked(&self.file);
        require_write_blocked(&self.cas);
    }
}
#[derive(Default)]
struct Close {
    opens: usize,
    raw: usize,
    closes: usize,
    withhold: bool,
    withheld: usize,
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
            "observer requires SQLite connection-close success, without native xClose/OS-close inference"
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

pub(super) fn require_ready_read_controls(
    events: &[(PathBuf, ReadBoundary)],
    destination: &Path,
    family: &Path,
) {
    for path in [destination, family] {
        for boundary in [
            ReadBoundary::BeforeSnapshot,
            ReadBoundary::DatabaseSnapshot,
            ReadBoundary::DatabaseIdentity,
        ] {
            assert!(
                events
                    .iter()
                    .any(|(actual, point)| actual == path && *point == boundary),
                "retained Ready must exercise actual {boundary:?} route {path:?}"
            );
        }
    }
}

fn snapshot(path: &Path) -> Database {
    witness::read(path, witness::database)
}
fn text(value: &str) -> Cell {
    Cell::Text(value.as_bytes().to_vec())
}
fn number(value: u64) -> Cell {
    Cell::Integer(value.try_into().unwrap())
}
fn only_c_database_changed(
    before: &(Tree, Tree, Tree),
    after: &(Tree, Tree, Tree),
    c_database: &Path,
) {
    assert_eq!(before.0, after.0, "B checkout changed during C Ready");
    assert_eq!(before.2, after.2, "C checkout changed during C Ready");
    assert_eq!(
        before.1.keys().collect::<Vec<_>>(),
        after.1.keys().collect::<Vec<_>>()
    );
    assert!(before.1.contains_key(c_database));
    for (path, node) in &before.1 {
        let next = &after.1[path];
        if path == c_database {
            let (Node::File((a, _, old)), Node::File((b, _, new))) = (node, next) else {
                panic!("C DB must stay regular file")
            };
            assert_eq!(a, b);
            assert_ne!(old, new);
        } else {
            assert_eq!(node, next, "Ready changed unrelated {path:?}");
        }
    }
}
fn family_initializing(
    path: &Path,
    stage: &ProviderTransitionIntentRecord,
    required: &[FileManifest],
) {
    witness::read(path, |c| {
        let proof = stage.destination.as_ref().unwrap();
        let shared = proof.format.shared.as_ref().unwrap();
        let owner = proof.format.identity.worktree_id.as_str();
        let registration: (String, String, Vec<u8>, String) = c
            .query_row(
                "SELECT metadata_path,database_identity,marker,state FROM worktrees WHERE id=?1",
                [owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            registration,
            (
                shared.metadata_path.to_str().unwrap().into(),
                proof.private_database_identity.clone(),
                proof.format.bytes().unwrap(),
                "initializing".into()
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
    });
}

pub(super) struct ReadyRows {
    pub(super) ready: ProviderTransitionIntentRecord,
    pub(super) receipt: ProviderTransitionReceiptRecord,
    pub(super) rows: Database,
}
pub(super) struct ReadyContent<'a> {
    pub(super) base: &'a FileManifest,
    pub(super) history: &'a FileManifest,
    pub(super) current: &'a FileManifest,
}
fn content_row(manifest: &FileManifest) -> Vec<Cell> {
    vec![
        text(manifest.file_id().as_str()),
        text(manifest.id().as_str()),
        number(manifest.byte_length()),
    ]
}
#[allow(clippy::too_many_arguments)]
pub(super) fn require_ready_rows(
    path: &Path,
    before: &Database,
    stage: &ProviderTransitionIntentRecord,
    derived: &ManagedPlanData,
    selection_payload: &[u8],
    content: ReadyContent<'_>,
    required: &[FileManifest],
    expected: &ExpectedCheckout,
) -> ReadyRows {
    // This fixture has three distinct contents, with only the historical
    // mapping in the copied prefix. Check the full manifest representations
    // independently of the planner's required rows and the Ready writer.
    let fixture = [content.base, content.history, content.current];
    assert_eq!(
        fixture
            .iter()
            .map(|manifest| manifest.file_id())
            .collect::<BTreeSet<_>>()
            .len(),
        3
    );
    assert_eq!(required.len(), fixture.len());
    assert_eq!(
        required
            .iter()
            .map(|manifest| ((manifest.file_id(), manifest.id()), manifest))
            .collect::<BTreeMap<_, _>>(),
        fixture
            .into_iter()
            .map(|manifest| ((manifest.file_id(), manifest.id()), manifest))
            .collect::<BTreeMap<_, _>>()
    );
    assert_eq!(
        before["projection_content"],
        vec![content_row(content.history)],
        "copied prefix must contain exactly the historical fixture mapping"
    );
    witness::read(path, |c| {
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            8
        );
        assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='projection_managed_admission')",[],|r|r.get::<_,bool>(0)).unwrap());
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM projection_provider_transitions",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
            1
        );
        let (id,role,state,generation,reservation,receipt_id,payload,digest):(String,String,String,u64,Option<String>,Option<String>,Vec<u8>,String)=c.query_row("SELECT id,role,state,local_generation,reservation,receipt_id,payload,digest FROM projection_provider_transitions",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).unwrap();
        let ready = ProviderTransitionIntentRecord::decode(&payload).unwrap();
        ready
            .require_row(TransitionRowData {
                id: &id,
                role: &role,
                state: &state,
                local_generation: generation,
                reservation: reservation.as_deref(),
                receipt_id: receipt_id.as_deref(),
                payload: &payload,
                digest: &digest,
            })
            .unwrap();
        assert_eq!(ready.role, TransitionRole::Destination);
        assert_eq!(ready.state, TransitionState::Ready);
        assert_eq!(ready.local_generation, 0);
        assert_eq!(ready.plan, stage.plan);
        assert_eq!(ready.destination, stage.destination);
        assert_eq!(ready.reservation, stage.reservation);
        let receipt_id = ready.receipt_id.as_ref().unwrap();
        let (stored_id,owner,transaction,companion,receipt_payload):(String,String,String,Option<String>,Vec<u8>)=c.query_row("SELECT id,owner_identity_digest,transaction_id,companion_id,payload FROM projection_provider_transition_receipts WHERE id=?1",[receipt_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
        let receipt = ProviderTransitionReceiptRecord::decode(&receipt_payload).unwrap();
        assert_eq!(stored_id, receipt.digest().unwrap());
        assert_eq!(stored_id, *receipt_id);
        assert_eq!(owner, receipt.owner_identity_digest);
        assert_eq!(transaction, stage.transaction_id);
        assert_eq!(companion, None);
        ready.require_receipt(&receipt).unwrap();
        receipt.require_prefix(&stage.plan.source_prefix).unwrap();
        let ImportSource::Ordinary {
            identity, version, ..
        } = &stage.plan.source_prefix.source
        else {
            panic!("actual ordinary prefix required")
        };
        assert_eq!(receipt.source_identity, *identity);
        assert_eq!(
            receipt.parent_receipt_id,
            stage.plan.source_prefix.ancestry_root_receipt
        );
        assert_eq!(receipt.imported_max_generation, version.generation);
        assert_eq!(receipt.imported_capture_count, version.capture_count);
        assert_eq!(
            receipt.destination_observation_generation,
            stage.plan.observation.generation
        );
        assert_eq!(
            receipt.destination_observation_digest,
            derived.observation_digest
        );
        assert_eq!(
            receipt.destination_placement_generation,
            stage.plan.placement_generation
        );
        assert_eq!(
            receipt.destination_placement_digest,
            derived.placement_digest
        );
        assert_eq!(
            receipt.destination_capture_sequence,
            stage.plan.capture_sequence
        );
        assert_eq!(receipt.destination_capture_digest, derived.capture.digest);
        assert_eq!(receipt.protected_manifests.count, required.len() as u64);
        assert_eq!(
            receipt.protected_manifests.digest,
            stage.plan.protected_manifests.digest
        );
        assert_eq!(
            receipt.content_mappings,
            stage.plan.source_prefix.closure.content_mappings
        );
        assert_eq!(receipt.companion_id, None);
        assert_eq!(receipt.companion_files.count, 0);
        assert_eq!(receipt.companion_deletions.count, 0);
        let mut expected_ready =
            ProviderTransitionIntentRecord::decode(&stage.encode().unwrap()).unwrap();
        expected_ready.role = TransitionRole::Destination;
        expected_ready.local_generation = 0;
        expected_ready.state = TransitionState::Ready;
        expected_ready.receipt_id = Some(receipt.digest().unwrap());
        assert_eq!(ready, expected_ready);
        let journal = checkout_journal::read(c, expected, None).unwrap();
        assert!(journal.is_complete());
        assert!(
            !c.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
        );
        let proof = stage.destination.as_ref().unwrap();
        let shared = proof.format.shared.as_ref().unwrap();
        let protection = ExactProtection::new(
            &shared.incarnation,
            &proof.format.identity.worktree_id,
            &proof.private_database_identity,
            &stage.transaction_id,
            required,
            None,
        )
        .unwrap();
        let mut suffix: BTreeMap<&str, Vec<Vec<Cell>>> = BTreeMap::new();
        for (generation, digest, parent, snapshot) in [
            (
                stage.plan.observation.generation,
                &derived.observation_digest,
                &version.snapshot_digest,
                &derived.observation,
            ),
            (
                stage.plan.placement_generation,
                &derived.placement_digest,
                &derived.observation_digest,
                &derived.placement,
            ),
        ] {
            let bytes = snapshot.canonical_bytes().unwrap();
            assert_eq!(database::digest(generation, Some(parent), &bytes), *digest);
            suffix.entry("projection_snapshots").or_default().push(vec![
                number(generation),
                text(digest),
                text(parent),
                Cell::Blob(bytes),
            ]);
            let selection_digest = database::digest(generation, Some(digest), selection_payload);
            suffix
                .entry("projection_selections")
                .or_default()
                .push(vec![
                    number(generation),
                    text(&selection_digest),
                    Cell::Blob(selection_payload.to_vec()),
                ]);
        }
        assert_eq!(
            derived.capture.computed_digest().unwrap(),
            derived.capture.digest
        );
        suffix.insert(
            "projection_captures",
            vec![vec![
                number(derived.capture.sequence),
                text(&derived.capture.digest),
                number(derived.capture.projection_generation),
                Cell::Blob(canonical_json(&derived.capture).unwrap()),
            ]],
        );
        suffix.insert(
            "projection_content",
            // The selected base is protected but absent from copied history,
            // just like current content: Ready must append both exact rows.
            vec![content_row(content.base), content_row(content.current)],
        );
        suffix.insert(
            "projection_claims",
            vec![vec![
                text(protection.id()),
                text(protection.digest()),
                text("provider-import"),
                text(&stage.transaction_id),
            ]],
        );
        suffix.insert(
            "projection_provider_transition_receipts",
            vec![vec![
                text(receipt_id),
                text(&receipt.owner_identity_digest),
                text(&stage.transaction_id),
                Cell::Null,
                Cell::Blob(receipt.encode().unwrap()),
            ]],
        );
        let rows = witness::database(c);
        assert_eq!(
            rows.keys().collect::<Vec<_>>(),
            before.keys().collect::<Vec<_>>()
        );
        for (table, previous) in before {
            let mut anticipated = if table == "projection_head" {
                vec![vec![number(1), number(stage.plan.placement_generation)]]
            } else if table == "projection_provider_transitions" {
                vec![vec![
                    text(&ready.transaction_id),
                    text("destination"),
                    text("ready"),
                    number(0),
                    text(ready.reservation.as_ref().unwrap()),
                    text(receipt_id),
                    Cell::Blob(ready.encode().unwrap()),
                    text(&ready.digest().unwrap()),
                ]]
            } else {
                // Existing immutable rows from every one of the thirteen
                // copied tables survive byte-for-byte, even in suffix tables.
                let mut expected_rows: Vec<_> = previous
                    .iter()
                    .map(|r| r.iter().map(copy_cell).collect::<Vec<_>>())
                    .collect();
                if let Some(appended) = suffix.remove(table.as_str()) {
                    expected_rows.extend(appended);
                }
                expected_rows
            };
            anticipated.sort();
            assert_eq!(
                rows[table], anticipated,
                "unexpected Ready change in {table}"
            );
        }
        assert!(suffix.is_empty());
        ReadyRows {
            ready,
            receipt,
            rows,
        }
    })
}
fn copy_cell(cell: &Cell) -> Cell {
    match cell {
        Cell::Null => Cell::Null,
        Cell::Integer(v) => Cell::Integer(*v),
        Cell::Real(v) => Cell::Real(*v),
        Cell::Text(v) => Cell::Text(v.clone()),
        Cell::Blob(v) => Cell::Blob(v.clone()),
    }
}

#[test]
#[ignore = "ROOT source release and audited owned native child3000/outer3060 required"]
fn managed_ordinary_ready_replays_withheld_private_close_ack_without_promoting_b_or_family() {
    // r38 Installing measured1193.900s; the r41 checkout recipe has a proposed
    // 2400s bound and remains active. Add600s for Ready validation/writer/replay:
    // child3000/outer3060 is a qualification proposal, not a measured runtime.
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_ready_replays_withheld_private_close_ack_without_promoting_b_or_family"
        ),
        3000,
        ready_recipe,
    );
}

fn ready_recipe() {
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
        _ => panic!("selected README required"),
    };

    fs::write(root.join(path.as_str()), HISTORY).unwrap();
    store.capture_patch(state.generation).unwrap();
    drop(store);

    let store = ProjectionStore::open(&root).unwrap();
    assert_eq!(store.state().unwrap(), state);
    fs::write(root.join(path.as_str()), CURRENT).unwrap();

    let family = store.marker.shared.as_ref().unwrap().root.clone();
    let source_database = store.marker.metadata_path(&root).join("projection.sqlite3");
    let source_lock = source_database.parent().unwrap().join("layrs.lock");

    let mut identity = store.identity().clone();
    identity.worktree_id =
        WorktreeId::parse("worktree:ordinary-private-ready-destination").unwrap();
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
        .unwrap_or_else(|e| panic!("Ready allocation {e:?}"));
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
    let family_database = family.join("metadata/catalog.sqlite3");

    let expected = ExpectedCheckout::ordinary(&staging, &files, &[], &image, None).unwrap();
    assert!(
        expected.deletions().is_empty(),
        "single README fixture does not qualify positive deletion/nested checkout"
    );

    let before_b = witness::source(&source_database, &store.marker, Some(&staging));
    let before_c = witness::destination(&destination_database, &staging);
    let before_family = snapshot(&family_database);
    let before = witness::trees(&root, &family, &destination_root, locks);

    let selection_payload = witness::read(&source_database, |c| {
        selection::encode(&selection::at(c, staging.local_generation).unwrap()).unwrap()
    });

    assert_eq!(
        before_c["projection_content"],
        vec![content_row(&history)],
        "copied prefix must contain exactly the historical fixture mapping"
    );

    let final_path = destination_root.join(path.as_str());
    let Node::File((inode, _, _)) = &before.2[&final_path] else {
        panic!("complete checkout final file required")
    };

    let physical = Physical {
        file: final_path,
        inode: inode.clone(),
        cas: witness::object(&family, "chunks", current.chunks()[0].chunk_id().as_str()),
    };
    physical.require();
    witness::verify_objects(&family, &required);
    family_initializing(&family_database, &staging, &required);

    let mut close = Close {
        physical: Some(physical),
        ..Close::default()
    };
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(
        owner
            .finalize_destination_ready(
                &allocated,
                &staging,
                &protection,
                &checkout,
                &mut close,
                &cancelled
            )
            .is_err()
    );
    assert_eq!(close.counts(), (0, 0, 0, 0));
    assert_eq!(close.physical_checks, 0);
    assert_eq!(
        witness::trees(&root, &family, &destination_root, locks),
        before
    );
    assert_eq!(snapshot(&destination_database), before_c);
    assert_eq!(snapshot(&family_database), before_family);

    close.withhold = true;
    let after_close = close.after_close.clone();
    let after_close_probes = Rc::new(Flag::new(0usize));
    let observed_after_close = after_close_probes.clone();
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    let failed = with_observer(
        move |path, point| {
            observed.borrow_mut().push((path.to_owned(), point));
            if after_close.get() {
                observed_after_close.set(observed_after_close.get() + 1);
                return Err(invalid(
                    "failed retained private Ready ACK forbids later observed SQL/identity/snapshot probes",
                ));
            }
            Ok(())
        },
        || {
            owner.finalize_destination_ready(
                &allocated,
                &staging,
                &protection,
                &checkout,
                &mut close,
                &cancel,
            )
        },
    );
    let error = match failed {
        Ok(_) => panic!("Ready close ACK was not withheld"),
        Err(e) => e,
    };
    assert!(error.to_string().contains(WITHHELD_ACK), "{error}");
    let primary = error.to_string();
    assert_eq!(
        after_close_probes.get(),
        0,
        "only mandatory rollback/reader closes follow the failed ACK"
    );
    require_ready_read_controls(&events.borrow(), &destination_database, &family_database);
    assert_eq!(close.counts(), (1, 1, 1, 1));
    assert_eq!(close.physical_checks, 2);

    let committed = witness::trees(&root, &family, &destination_root, locks);
    only_c_database_changed(&before, &committed, &destination_database);
    assert_eq!(
        witness::source(&source_database, &store.marker, Some(&staging)),
        before_b
    );
    assert_eq!(snapshot(&family_database), before_family);
    family_initializing(&family_database, &staging, &required);

    let durable = require_ready_rows(
        &destination_database,
        &before_c,
        &staging,
        &derived,
        &selection_payload,
        ReadyContent {
            base: &base,
            history: &history,
            current: &current,
        },
        &required,
        &expected,
    );
    close.physical.as_ref().unwrap().require();
    witness::verify_objects(&family, &required);

    let outcome = owner
        .finalize_destination_ready(
            &allocated,
            &staging,
            &protection,
            &checkout,
            &mut close,
            &cancel,
        )
        .unwrap();
    assert_eq!(outcome.effect(), ReadyEffect::AlreadyReady);
    assert_eq!(outcome.ready(), &durable.ready);
    assert_eq!(outcome.receipt(), &durable.receipt);
    assert_eq!(close.counts(), (1, 1, 1, 1));
    assert_eq!(close.physical_checks, 2);
    assert_eq!(
        error.to_string(),
        primary,
        "replay preserves the original ACK error"
    );

    assert_eq!(
        witness::trees(&root, &family, &destination_root, locks),
        committed
    );
    assert_eq!(snapshot(&destination_database), durable.rows);
    assert_eq!(snapshot(&family_database), before_family);
    assert_eq!(
        witness::source(&source_database, &store.marker, Some(&staging)),
        before_b
    );
    close.physical.as_ref().unwrap().require();
    family_initializing(&family_database, &staging, &required);

    drop(outcome);
    drop(checkout);
    drop(protection);
    owner.close().unwrap();
    assert!(ProjectionStore::open(&root).is_err());
    allocated.require_published(&staging.plan).unwrap();
    drop(allocated);
    assert!(
        ProjectionStore::open(&destination_root).is_err(),
        "private C8 Ready with Initializing member remains unadmitted"
    );
    assert_eq!(
        witness::trees(&root, &family, &destination_root, locks),
        committed
    );
    drop(store);
}
