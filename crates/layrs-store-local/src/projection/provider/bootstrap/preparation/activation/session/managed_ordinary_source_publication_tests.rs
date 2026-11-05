//! Same-owner physical B SourceReady/SourcePublished and exact phase replay.
//! Progress is DATA until the retained B owner closes positively. C remains
//! private C8 Ready and unadmitted. No close-failure, ACK-withholding, crash,
//! owner-loss, historical handoff or autonomous C9 admission is qualified.
use super::{
    managed_ordinary_family_ready_tests::{Close, copy_cell, family, text},
    managed_ordinary_installing_tests::{self as witness, Cell, Database, Node, Tree},
    managed_ordinary_ready_tests::{self as ready_witness, CURRENT, Physical},
};
use crate::projection::{
    ProjectionStore, database, format,
    provider::{
        bootstrap::ManagedPlanData,
        checkout_journal::{CheckoutFile, ExpectedCheckout},
        destination_ready::ReadyEffect,
        managed_admission::read_admitted_import,
        managed_export::{
            OrdinaryCheckoutProof, OrdinaryImportProtection, OrdinaryManagedExport,
            allocation::{self, ManagedDestinationAllocation},
        },
        ordinary_destination::{CopyEffect, staging::StageEffect},
        process_profile::tests::child,
        records::{
            Record,
            import::ProviderTransitionReceiptRecord,
            transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
        },
        transition_sql::SourceIntentEffect,
    },
    selection,
    shared::{
        admitted_catalog::{ExportPublicationEffect, ExportReservationEffect},
        provider_access::ImportRegistrationEffect,
    },
};
use layrs_core::scm::{FileId, FileManifest, RepoPath, TreeEntryTarget, WorktreeId};
use layrs_vfs::Cancellation;
use rusqlite::Connection;
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::Path,
};

const LEAF: &str = "00000000000000000000000000000121";
const TX: &str = "00000000000000000000000000000122";
pub(super) const HISTORY: &[u8] = b"ordinary source publication historical-only capture";

fn snapshot(path: &Path) -> Database {
    witness::read(path, witness::database)
}

pub(super) fn ingress(c: &Connection, marker: &format::Marker) -> Vec<Vec<u8>> {
    let admitted = read_admitted_import(c, marker, None).unwrap();
    vec![
        admitted.record().encode().unwrap(),
        admitted.source_published().encode().unwrap(),
        admitted.prefix().encode().unwrap(),
        admitted.ready().encode().unwrap(),
        admitted.receipt().encode().unwrap(),
    ]
}

fn transition_row(intent: &ProviderTransitionIntentRecord) -> Vec<Cell> {
    let phase = match intent.state {
        TransitionState::Staging => "staging",
        TransitionState::Ready => "ready",
        TransitionState::Published => "published",
        _ => panic!("source publication fixture requires actual Staging/Ready/Published"),
    };
    assert_eq!(intent.role, TransitionRole::Source);
    vec![
        text(&intent.transaction_id),
        text("source"),
        text(phase),
        Cell::Integer(i64::try_from(intent.local_generation).unwrap()),
        intent.reservation.as_deref().map_or(Cell::Null, text),
        intent.receipt_id.as_deref().map_or(Cell::Null, text),
        Cell::Blob(intent.encode().unwrap()),
        text(&intent.digest().unwrap()),
    ]
}

pub(super) fn source(
    path: &Path,
    marker: &format::Marker,
    expected: &ProviderTransitionIntentRecord,
    receipt: Option<&ProviderTransitionReceiptRecord>,
    original_ingress: &[Vec<u8>],
) -> Database {
    witness::read(path, |c| {
        assert_eq!(ingress(c, marker), original_ingress);
        let admitted = read_admitted_import(c, marker, None).unwrap();
        assert_eq!(admitted.outgoing(), std::slice::from_ref(expected));
        if let Some(receipt) = receipt {
            assert_eq!(
                admitted
                    .outgoing_receipt(&receipt.digest().unwrap())
                    .unwrap(),
                receipt
            );
        } else {
            assert!(expected.receipt_id.is_none());
        }
        // Decode the stored actual phase independently; never rewrite Staging
        // in SQL or present reconstructed Staging DATA as its observation.
        let payload: Vec<u8> = c
            .query_row(
                "SELECT payload FROM projection_provider_transitions WHERE id=?1",
                [&expected.transaction_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            ProviderTransitionIntentRecord::decode(&payload).unwrap(),
            *expected
        );
        let idle = database::require_idle(c);
        if expected.state == TransitionState::Published {
            idle.unwrap();
        } else {
            assert!(idle.is_err(), "SourceReady must block ordinary idle work");
        }
        let rows = witness::database(c);
        let expected_row = transition_row(expected);
        assert_eq!(
            rows["projection_provider_transitions"]
                .iter()
                .filter(|row| row.first() == expected_row.first())
                .collect::<Vec<_>>(),
            vec![&expected_row]
        );
        rows
    })
}

pub(super) fn only_source_rows_changed(
    before: &Database,
    after: &Database,
    previous: &ProviderTransitionIntentRecord,
    next: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
) {
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    let previous_row = transition_row(previous);
    let next_row = transition_row(next);
    assert_eq!(
        before["projection_provider_transitions"]
            .iter()
            .filter(|row| row.first() == previous_row.first())
            .collect::<Vec<_>>(),
        vec![&previous_row]
    );
    for (table, rows) in before {
        let mut expected: Vec<_> = rows
            .iter()
            .map(|row| row.iter().map(copy_cell).collect::<Vec<_>>())
            .collect();
        if table == "projection_provider_transitions" {
            let row = expected
                .iter_mut()
                .find(|row| row.first() == previous_row.first())
                .unwrap();
            *row = next_row.iter().map(copy_cell).collect();
        } else if table == "projection_provider_transition_receipts"
            && next.state == TransitionState::Ready
        {
            let id = text(&receipt.digest().unwrap());
            assert!(rows.iter().all(|row| row.first() != Some(&id)));
            expected.push(vec![
                id,
                text(&receipt.owner_identity_digest),
                text(&receipt.transaction_id),
                receipt.companion_id.as_deref().map_or(Cell::Null, text),
                Cell::Blob(receipt.encode().unwrap()),
            ]);
        }
        expected.sort();
        assert_eq!(after[table], expected, "unexpected B mutation in {table}");
    }
}

pub(super) fn only_source_database_changed(
    before: &(Tree, Tree, Tree),
    after: &(Tree, Tree, Tree),
    database: &Path,
) {
    let mut changes = 0;
    for (before, after) in [
        (&before.0, &after.0),
        (&before.1, &after.1),
        (&before.2, &after.2),
    ] {
        assert_eq!(
            before.keys().collect::<Vec<_>>(),
            after.keys().collect::<Vec<_>>()
        );
        for (path, previous) in before {
            let next = &after[path];
            if path == database {
                let (Node::File((old_id, _, old_hash)), Node::File((new_id, _, new_hash))) =
                    (previous, next)
                else {
                    panic!("B database must remain a regular file")
                };
                assert_eq!(old_id, new_id);
                assert_ne!(old_hash, new_hash, "source phase must be durable");
                changes += 1;
            } else {
                assert_eq!(previous, next, "source publication changed {path:?}");
            }
        }
    }
    assert!(changes > 0, "complete trees must include actual B database");
}

fn require_source_pin(path: &Path) {
    assert_eq!(fs::read(path).unwrap(), CURRENT);
    let error = OpenOptions::new()
        .write(true)
        .share_mode(7)
        .open(path)
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(32), "retained source image pin");
}

#[test]
#[ignore = "ROOT source release and audited owned native child3600/outer3660 required"]
fn managed_ordinary_source_publication_commits_ready_then_published_and_replays_exact_phases() {
    // Proposed, not measured: the genuine B9/C8/family Ready setup is followed
    // by four complete source-publication calls and their physical witnesses.
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_source_publication_commits_ready_then_published_and_replays_exact_phases"
        ),
        3600,
        source_publication_recipe,
    );
}

/// Optional continuation of this fresh same-owner fixture. The original
/// publication test supplies no hooks and keeps its exact C8 assertions.
pub(super) struct PublicationFixture<'a> {
    pub destination: &'a ManagedDestinationAllocation,
    pub protection: &'a OrdinaryImportProtection,
    pub checkout: &'a OrdinaryCheckoutProof,
    pub ready: &'a ProviderTransitionIntentRecord,
    pub receipt: &'a ProviderTransitionReceiptRecord,
    pub required: &'a [FileManifest],
    pub expected: &'a ExpectedCheckout,
    pub source_marker: &'a format::Marker,
    pub provider_source_root: &'a Path,
    pub source_root: &'a Path,
    pub source_database: &'a Path,
    pub destination_root: &'a Path,
    pub destination_database: &'a Path,
    pub family_root: &'a Path,
    pub family_database: &'a Path,
}

/// Actual complete checkout at C Staging. No Ready record or receipt exists
/// on this route; these cannot be mistaken for PublicationFixture evidence.
pub(super) struct CheckoutFixture<'a> {
    pub source_staging: &'a ProviderTransitionIntentRecord,
    pub destination_staging: &'a ProviderTransitionIntentRecord,
    pub protection: &'a OrdinaryImportProtection,
    pub checkout: &'a OrdinaryCheckoutProof,
    pub expected: &'a ExpectedCheckout,
    pub derived: &'a ManagedPlanData,
    pub selection_payload: &'a [u8],
    pub base: &'a FileManifest,
    pub history: &'a FileManifest,
    pub current: &'a FileManifest,
    pub required: &'a [FileManifest],
    pub source_marker: &'a format::Marker,
    pub source_root: &'a Path,
    pub source_database: &'a Path,
    pub destination_root: &'a Path,
    pub destination_database: &'a Path,
    pub family_root: &'a Path,
    pub family_database: &'a Path,
}

/// Genuine completed content protection before any checkout declaration or
/// filesystem population. No checkout proof or new transaction Ready/receipt
/// exists; inherited ingress receipts remain intact.
pub(super) struct ProtectedCheckoutFixture<'a> {
    pub source_staging: &'a ProviderTransitionIntentRecord,
    pub destination_staging: &'a ProviderTransitionIntentRecord,
    pub protection: &'a OrdinaryImportProtection,
    pub expected: &'a ExpectedCheckout,
    pub current: &'a FileManifest,
    pub required: &'a [FileManifest],
    pub source_marker: &'a format::Marker,
    pub source_root: &'a Path,
    pub source_database: &'a Path,
    pub destination_root: &'a Path,
    pub destination_database: &'a Path,
    pub family_root: &'a Path,
    pub family_database: &'a Path,
}

pub(super) struct AdmissionHooks {
    pub before_published:
        fn(&mut OrdinaryManagedExport<'_>, &PublicationFixture<'_>, &Cancellation),
    pub after_published: fn(OrdinaryManagedExport<'_>, &PublicationFixture<'_>, &Cancellation),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PublicationStop {
    PrivateReady,
    FamilyReady,
    SourceReady,
    Published,
}

struct PublicationHooks<F> {
    before_published: fn(&mut OrdinaryManagedExport<'_>, &PublicationFixture<'_>, &Cancellation),
    continuation: F,
    stop: PublicationStop,
}

pub(super) fn with_admission_hooks(hooks: AdmissionHooks) {
    with_admission_data(hooks.before_published, hooks.after_published);
}

/// The continuation must close its B owner. Its owned DATA escapes only after
/// the helper has dropped every C checkout/CAS/allocation and store holder.
pub(super) fn with_admission_data<T>(
    before_published: fn(&mut OrdinaryManagedExport<'_>, &PublicationFixture<'_>, &Cancellation),
    after_published: impl FnOnce(OrdinaryManagedExport<'_>, &PublicationFixture<'_>, &Cancellation) -> T,
) -> T {
    source_publication_inner(
        Some(PublicationHooks {
            before_published,
            continuation: after_published,
            stop: PublicationStop::Published,
        }),
        None::<fn(OrdinaryManagedExport<'_>, &CheckoutFixture<'_>, &Cancellation) -> T>,
        None::<fn(OrdinaryManagedExport<'_>, &ProtectedCheckoutFixture<'_>, &Cancellation) -> T>,
    )
    .unwrap()
}

/// Stop after the real SourceReady commit, closed readback and exact Ready
/// replay. The callback consumes and must positively close the original B
/// owner; only its owned DATA escapes after all C/store holders are dropped.
pub(super) fn with_source_ready_data<T>(
    after_ready: impl FnOnce(OrdinaryManagedExport<'_>, &PublicationFixture<'_>, &Cancellation) -> T,
) -> T {
    source_publication_inner(
        Some(PublicationHooks {
            before_published: |_, _, _| {},
            continuation: after_ready,
            stop: PublicationStop::SourceReady,
        }),
        None::<fn(OrdinaryManagedExport<'_>, &CheckoutFixture<'_>, &Cancellation) -> T>,
        None::<fn(OrdinaryManagedExport<'_>, &ProtectedCheckoutFixture<'_>, &Cancellation) -> T>,
    )
    .unwrap()
}

/// Stop with genuine C8/family Ready while actual B remains Staging and has
/// no outgoing Ready receipt. The callback must positively close B; only its
/// owned DATA returns after all original source/C/store holders are released.
pub(super) fn with_source_staging_data<T>(
    after_family_ready: impl FnOnce(
        OrdinaryManagedExport<'_>,
        &PublicationFixture<'_>,
        &Cancellation,
    ) -> T,
) -> T {
    source_publication_inner(
        Some(PublicationHooks {
            before_published: |_, _, _| {},
            continuation: after_family_ready,
            stop: PublicationStop::FamilyReady,
        }),
        None::<fn(OrdinaryManagedExport<'_>, &CheckoutFixture<'_>, &Cancellation) -> T>,
        None::<fn(OrdinaryManagedExport<'_>, &ProtectedCheckoutFixture<'_>, &Cancellation) -> T>,
    )
    .unwrap()
}

/// Stop after genuine private C8 Ready, before any family publication. Actual
/// B remains Staging and C's family member remains Initializing. The callback
/// must positively close B; only its owned DATA escapes after all old holders.
pub(super) fn with_private_ready_data<T>(
    after_private_ready: impl FnOnce(
        OrdinaryManagedExport<'_>,
        &PublicationFixture<'_>,
        &Cancellation,
    ) -> T,
) -> T {
    source_publication_inner(
        Some(PublicationHooks {
            before_published: |_, _, _| {},
            continuation: after_private_ready,
            stop: PublicationStop::PrivateReady,
        }),
        None::<fn(OrdinaryManagedExport<'_>, &CheckoutFixture<'_>, &Cancellation) -> T>,
        None::<fn(OrdinaryManagedExport<'_>, &ProtectedCheckoutFixture<'_>, &Cancellation) -> T>,
    )
    .unwrap()
}

/// Stop after the genuine checkout has completed, before private Ready. The
/// callback must positively close original B. All other original owners and
/// image/allocation/CAS/checkout holders drop before its owned DATA escapes.
pub(super) fn with_complete_checkout_data<T>(
    after_checkout: impl FnOnce(OrdinaryManagedExport<'_>, &CheckoutFixture<'_>, &Cancellation) -> T,
) -> T {
    source_publication_inner(
        None::<
            PublicationHooks<
                fn(OrdinaryManagedExport<'_>, &PublicationFixture<'_>, &Cancellation) -> T,
            >,
        >,
        Some(after_checkout),
        None::<fn(OrdinaryManagedExport<'_>, &ProtectedCheckoutFixture<'_>, &Cancellation) -> T>,
    )
    .unwrap()
}

/// The callback positively closes B. Only owned DATA may escape, after this
/// helper drops every original protection/allocation/image/store holder.
pub(super) fn with_protected_checkout_data<T>(
    after_protection: impl FnOnce(
        OrdinaryManagedExport<'_>,
        &ProtectedCheckoutFixture<'_>,
        &Cancellation,
    ) -> T,
) -> T {
    source_publication_inner(
        None::<
            PublicationHooks<
                fn(OrdinaryManagedExport<'_>, &PublicationFixture<'_>, &Cancellation) -> T,
            >,
        >,
        None::<fn(OrdinaryManagedExport<'_>, &CheckoutFixture<'_>, &Cancellation) -> T>,
        Some(after_protection),
    )
    .unwrap()
}

fn source_publication_recipe() {
    let _: Option<()> = source_publication_inner(
        None::<
            PublicationHooks<fn(OrdinaryManagedExport<'_>, &PublicationFixture<'_>, &Cancellation)>,
        >,
        None::<fn(OrdinaryManagedExport<'_>, &CheckoutFixture<'_>, &Cancellation)>,
        None::<fn(OrdinaryManagedExport<'_>, &ProtectedCheckoutFixture<'_>, &Cancellation)>,
    );
}

fn source_publication_inner<T>(
    mut hooks: Option<
        PublicationHooks<
            impl FnOnce(OrdinaryManagedExport<'_>, &PublicationFixture<'_>, &Cancellation) -> T,
        >,
    >,
    mut checkout_hook: Option<
        impl FnOnce(OrdinaryManagedExport<'_>, &CheckoutFixture<'_>, &Cancellation) -> T,
    >,
    mut protection_hook: Option<
        impl FnOnce(OrdinaryManagedExport<'_>, &ProtectedCheckoutFixture<'_>, &Cancellation) -> T,
    >,
) -> Option<T> {
    child::declare();
    let _total = child::phase("source-route.total");
    let mut phase = child::phase("source-setup.provider-to-admitted-b");

    // Independent fresh fixture: no r47/earlier database is reopened or reset.
    // The family recipe owns its capabilities through teardown, so its setup
    // cannot be shared by visibility alone; only its witnesses are reused.
    let (root, provider_source_root) =
        super::managed_restore_recovery_tests::admitted_destination_with_source();
    phase.next("source-setup.prepare-b");
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
    let original_ingress = witness::read(&source_database, |c| ingress(c, &store.marker));
    let mut identity = store.identity().clone();
    identity.worktree_id =
        WorktreeId::parse("worktree:ordinary-source-publication-destination").unwrap();
    let cancel = Cancellation::default();
    phase.next("source-setup.begin-export");
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

    phase.next("source-setup.plan");
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
    phase.next("source-setup.allocate");
    let mut allocated = allocation::allocate(&mut owner, &planned, &cancel)
        .unwrap_or_else(|e| panic!("source publication allocation {e:?}"));
    phase.next("source-setup.commit-allocation");
    let (copying, effect) = owner
        .commit_allocation(&planned, &allocated, &cancel)
        .unwrap();
    assert_eq!(effect, SourceIntentEffect::Updated);
    phase.next("source-setup.publish-format");
    allocated
        .publish_format(&mut owner, &copying, &cancel)
        .unwrap();
    phase.next("source-setup.register-allocation");
    assert_eq!(
        owner
            .register_allocation(&allocated, &copying, &cancel)
            .unwrap(),
        ImportRegistrationEffect::Inserted
    );
    let mut setup = Close::default();
    phase.next("source-setup.copy-metadata");
    assert_eq!(
        owner
            .copy_metadata(&allocated, &copying, &mut setup, &cancel)
            .unwrap(),
        CopyEffect::Copied
    );
    phase.next("source-setup.stage-metadata");
    let staged = owner
        .stage_metadata(&allocated, &copying, &mut setup, &cancel)
        .unwrap();
    assert_eq!(staged.source, SourceIntentEffect::Updated);
    assert_eq!(staged.destination, StageEffect::Updated);
    let staging = staged.intent;
    assert_eq!(staging.state, TransitionState::Staging);
    phase.next("source-setup.reserve-content");
    assert_eq!(
        owner
            .reserve_content(&allocated, &staging, &mut setup, &cancel)
            .unwrap(),
        ExportReservationEffect::Inserted
    );
    phase.next("source-setup.install-content");
    for manifest in &required {
        assert_eq!(
            owner
                .install_content(&allocated, &staging, manifest.id(), &mut setup, &cancel)
                .unwrap(),
            *manifest
        );
    }
    phase.next("source-setup.protect-content");
    let protection = owner
        .protect_content(&allocated, &staging, &mut setup, &cancel)
        .unwrap();
    if let Some(after_protection) = protection_hook.take() {
        assert!(hooks.is_none() && checkout_hook.is_none());
        let destination_root = allocated.root().to_owned();
        let destination_database = allocated.destination_database().path().to_owned();
        let family_database = family_root.join("metadata/catalog.sqlite3");
        let expected = ExpectedCheckout::ordinary(&staging, &files, &[], &image, None).unwrap();
        let destination_staging = witness::read(&destination_database, |c| {
            let payload: Vec<u8> = c
                .query_row(
                    "SELECT payload FROM projection_provider_transitions WHERE id=?1",
                    [&staging.transaction_id],
                    |row| row.get(0),
                )
                .unwrap();
            ProviderTransitionIntentRecord::decode(&payload).unwrap()
        });
        assert_eq!(destination_staging.role, TransitionRole::Destination);
        assert_eq!(destination_staging.state, TransitionState::Staging);
        assert!(destination_staging.receipt_id.is_none());
        let fixture = ProtectedCheckoutFixture {
            source_staging: &staging,
            destination_staging: &destination_staging,
            protection: &protection,
            expected: &expected,
            current: &current,
            required: &required,
            source_marker: &store.marker,
            source_root: &root,
            source_database: &source_database,
            destination_root: &destination_root,
            destination_database: &destination_database,
            family_root: &family_root,
            family_database: &family_database,
        };
        phase.next("source-route.protected-checkout-continuation");
        let result = after_protection(owner, &fixture, &cancel);
        phase.next("source-route.protected-checkout-teardown");
        allocated.require_published(&staging.plan).unwrap();
        drop(protection);
        drop(allocated);
        drop(store);
        return Some(result);
    }
    phase.next("source-setup.copy-checkout");
    let checkout = owner
        .copy_checkout(&allocated, &staging, &protection, &mut setup, &cancel)
        .unwrap();
    phase.next("source-setup.checkout-witness");
    let destination_root = allocated.root().to_owned();
    let destination_database = allocated.destination_database().path().to_owned();
    let destination_lock = destination_database.parent().unwrap().join("layrs.lock");
    let locks = [source_lock.as_path(), destination_lock.as_path()];
    let family_database = family_root.join("metadata/catalog.sqlite3");
    let expected = ExpectedCheckout::ordinary(&staging, &files, &[], &image, None).unwrap();
    assert!(
        expected.deletions().is_empty(),
        "README fixture has no positive deletion/nested checkout"
    );
    let staged_c = witness::destination(&destination_database, &staging);
    let selection_payload = witness::read(&source_database, |c| {
        selection::encode(&selection::at(c, staging.local_generation).unwrap()).unwrap()
    });
    if let Some(after_checkout) = checkout_hook.take() {
        assert!(
            hooks.is_none(),
            "checkout stop and later phase hooks are exclusive"
        );
        let destination_staging = witness::read(&destination_database, |c| {
            let payload: Vec<u8> = c
                .query_row(
                    "SELECT payload FROM projection_provider_transitions WHERE id=?1",
                    [&staging.transaction_id],
                    |row| row.get(0),
                )
                .unwrap();
            ProviderTransitionIntentRecord::decode(&payload).unwrap()
        });
        assert_eq!(destination_staging.role, TransitionRole::Destination);
        assert_eq!(destination_staging.state, TransitionState::Staging);
        assert!(destination_staging.receipt_id.is_none());
        let fixture = CheckoutFixture {
            source_staging: &staging,
            destination_staging: &destination_staging,
            protection: &protection,
            checkout: &checkout,
            expected: &expected,
            derived: &derived,
            selection_payload: &selection_payload,
            base: &base,
            history: &history,
            current: &current,
            required: &required,
            source_marker: &store.marker,
            source_root: &root,
            source_database: &source_database,
            destination_root: &destination_root,
            destination_database: &destination_database,
            family_root: &family_root,
            family_database: &family_database,
        };
        phase.next("source-route.checkout-complete-continuation");
        let result = after_checkout(owner, &fixture, &cancel);
        phase.next("source-route.checkout-complete-teardown");
        allocated.require_published(&staging.plan).unwrap();
        drop(checkout);
        drop(protection);
        drop(allocated);
        drop(store);
        return Some(result);
    }
    phase.next("source-setup.private-ready");
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
    phase.next("source-setup.private-ready-witness");
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

    let before_b = source(
        &source_database,
        &store.marker,
        &staging,
        None,
        &original_ingress,
    );
    let before_family = family(&family_database, &staging, &required, "initializing");
    let before_trees = witness::trees(&root, &family_root, &destination_root, locks);
    let final_path = destination_root.join(path.as_str());
    let Node::File((inode, _, _)) = &before_trees.2[&final_path] else {
        panic!("complete C checkout required")
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
    require_source_pin(&root.join(path.as_str()));
    witness::verify_objects(&family_root, &required);
    // C8 Ready alone cannot publish B while the exact family member is still
    // Initializing. The retained owner survives this ordinary refusal.
    phase.next("source-setup.early-source-ready-refusal");
    assert!(
        owner
            .publish_source_ready(
                &allocated,
                &staging,
                &protection,
                &checkout,
                &durable.ready,
                &durable.receipt,
                &cancel,
            )
            .is_err()
    );
    phase.next("source-setup.early-refusal-witness");
    assert_eq!(
        source(
            &source_database,
            &store.marker,
            &staging,
            None,
            &original_ingress
        ),
        before_b
    );
    assert_eq!(snapshot(&destination_database), durable.rows);
    assert_eq!(snapshot(&family_database), before_family);
    assert_eq!(
        witness::trees(&root, &family_root, &destination_root, locks),
        before_trees
    );
    physical.require();
    require_source_pin(&root.join(path.as_str()));

    if hooks
        .as_ref()
        .is_some_and(|hooks| hooks.stop == PublicationStop::PrivateReady)
    {
        let fixture = PublicationFixture {
            destination: &allocated,
            protection: &protection,
            checkout: &checkout,
            ready: &durable.ready,
            receipt: &durable.receipt,
            required: &required,
            expected: &expected,
            source_marker: &store.marker,
            provider_source_root: &provider_source_root,
            source_root: &root,
            source_database: &source_database,
            destination_root: &destination_root,
            destination_database: &destination_database,
            family_root: &family_root,
            family_database: &family_database,
        };
        let hooks = hooks.take().unwrap();
        phase.next("source-route.private-ready-continuation");
        let result = (hooks.continuation)(owner, &fixture, &cancel);
        phase.next("source-route.private-ready-teardown");
        allocated.require_published(&staging.plan).unwrap();
        drop(checkout);
        drop(protection);
        drop(allocated);
        drop(store);
        return Some(result);
    }

    phase.next("source-setup.family-ready");
    let family_ready = owner
        .publish_destination_family_ready(
            &allocated,
            &staging,
            &protection,
            &checkout,
            &mut setup,
            &cancel,
        )
        .unwrap();
    phase.next("source-setup.family-ready-witness");
    assert_eq!(family_ready.effect(), ExportPublicationEffect::Updated);
    assert_eq!(family_ready.ready(), &durable.ready);
    assert_eq!(family_ready.receipt(), &durable.receipt);
    drop(family_ready);
    assert_eq!(
        source(
            &source_database,
            &store.marker,
            &staging,
            None,
            &original_ingress
        ),
        before_b
    );
    assert_eq!(snapshot(&destination_database), durable.rows);
    let family_rows = family(&family_database, &staging, &required, "ready");

    let fixture = PublicationFixture {
        destination: &allocated,
        protection: &protection,
        checkout: &checkout,
        ready: &durable.ready,
        receipt: &durable.receipt,
        required: &required,
        expected: &expected,
        source_marker: &store.marker,
        provider_source_root: &provider_source_root,
        source_root: &root,
        source_database: &source_database,
        destination_root: &destination_root,
        destination_database: &destination_database,
        family_root: &family_root,
        family_database: &family_database,
    };

    if hooks
        .as_ref()
        .is_some_and(|hooks| hooks.stop == PublicationStop::FamilyReady)
    {
        // The genuine family Ready publication above is the last setup call.
        // Neither SourceReady nor SourcePublished is reached on this route.
        let hooks = hooks.take().unwrap();
        phase.next("source-route.family-ready-continuation");
        let result = (hooks.continuation)(owner, &fixture, &cancel);
        phase.next("source-route.family-ready-teardown");
        allocated.require_published(&staging.plan).unwrap();
        drop(checkout);
        drop(protection);
        drop(allocated);
        drop(store);
        return Some(result);
    }

    // Independently derive exact B DATA from the witnessed complete C Ready.
    // Only role and source-local generation change; Published then changes
    // only state. None of these expected records rewrite the stored B phase.
    phase.next("source-setup.source-publication-witness");
    let mut expected_ready =
        ProviderTransitionIntentRecord::decode(&durable.ready.encode().unwrap()).unwrap();
    expected_ready.role = TransitionRole::Source;
    expected_ready.local_generation = expected_ready
        .plan
        .source_prefix
        .source
        .version()
        .generation;
    assert_eq!(expected_ready.local_generation, staging.local_generation);
    expected_ready.require_transition(&staging).unwrap();
    expected_ready.require_receipt(&durable.receipt).unwrap();
    let mut expected_published =
        ProviderTransitionIntentRecord::decode(&expected_ready.encode().unwrap()).unwrap();
    expected_published.state = TransitionState::Published;
    expected_published
        .require_transition(&expected_ready)
        .unwrap();

    let mut previous = &staging;
    let mut previous_rows = before_b;
    let mut previous_trees = witness::trees(&root, &family_root, &destination_root, locks);
    for next in [&expected_ready, &expected_published] {
        phase.next(if next.state == TransitionState::Ready {
            "source-setup.source-ready-preflight"
        } else {
            "source-setup.source-published-preflight"
        });
        if next.state == TransitionState::Published {
            if let Some(hooks) = &hooks {
                (hooks.before_published)(&mut owner, &fixture, &cancel);
            }
        }
        let mut publish = |cancel: &Cancellation| {
            if next.state == TransitionState::Ready {
                owner.publish_source_ready(
                    &allocated,
                    &staging,
                    &protection,
                    &checkout,
                    &durable.ready,
                    &durable.receipt,
                    cancel,
                )
            } else {
                owner.publish_source_published(
                    &allocated,
                    &staging,
                    &protection,
                    &checkout,
                    &durable.ready,
                    &durable.receipt,
                    cancel,
                )
            }
        };
        physical.require();
        require_source_pin(&root.join(path.as_str()));
        let cancelled = Cancellation::default();
        cancelled.cancel();
        phase.next(if next.state == TransitionState::Ready {
            "source-setup.source-ready-precancel"
        } else {
            "source-setup.source-published-precancel"
        });
        assert!(publish(&cancelled).is_err());
        phase.next("source-setup.source-precancel-witness");
        assert_eq!(snapshot(&source_database), previous_rows);
        assert_eq!(snapshot(&destination_database), durable.rows);
        assert_eq!(
            family(&family_database, &staging, &required, "ready"),
            family_rows
        );
        assert_eq!(
            witness::trees(&root, &family_root, &destination_root, locks),
            previous_trees
        );
        physical.require();
        require_source_pin(&root.join(path.as_str()));

        phase.next(if next.state == TransitionState::Ready {
            "source-setup.source-ready"
        } else {
            "source-setup.source-published"
        });
        let progress = publish(&cancel).unwrap();
        phase.next("source-setup.source-publication-result-witness");
        assert_eq!(progress.effect(), SourceIntentEffect::Updated);
        assert_eq!(progress.intent(), next);
        assert_eq!(progress.receipt(), &durable.receipt);
        drop(progress); // Committed DATA, not an enclosing B owner-close ACK.
        let rows = source(
            &source_database,
            &store.marker,
            next,
            Some(&durable.receipt),
            &original_ingress,
        );
        only_source_rows_changed(&previous_rows, &rows, previous, next, &durable.receipt);
        assert_eq!(snapshot(&destination_database), durable.rows);
        assert_eq!(
            family(&family_database, &staging, &required, "ready"),
            family_rows
        );
        let trees = witness::trees(&root, &family_root, &destination_root, locks);
        only_source_database_changed(&previous_trees, &trees, &source_database);
        physical.require();
        require_source_pin(&root.join(path.as_str()));
        witness::verify_objects(&family_root, &required);

        phase.next(if next.state == TransitionState::Ready {
            "source-setup.source-ready-replay"
        } else {
            "source-setup.source-published-replay"
        });
        let replay = publish(&cancel).unwrap();
        phase.next("source-setup.source-replay-witness");
        assert_eq!(replay.effect(), SourceIntentEffect::AlreadyPresent);
        assert_eq!(replay.intent(), next);
        assert_eq!(replay.receipt(), &durable.receipt);
        drop(replay);
        // Exact same-phase replay must preserve all rows, database bytes,
        // checkout/CAS trees and native identities, including B's original
        // ingress/admission/archive/head/captures/content and C's C8 journal.
        assert_eq!(
            source(
                &source_database,
                &store.marker,
                next,
                Some(&durable.receipt),
                &original_ingress
            ),
            rows
        );
        assert_eq!(snapshot(&destination_database), durable.rows);
        assert_eq!(
            family(&family_database, &staging, &required, "ready"),
            family_rows
        );
        assert_eq!(
            witness::trees(&root, &family_root, &destination_root, locks),
            trees
        );
        physical.require();
        require_source_pin(&root.join(path.as_str()));
        witness::verify_objects(&family_root, &required);
        previous = next;
        previous_rows = rows;
        previous_trees = trees;
        if next.state == TransitionState::Ready
            && hooks
                .as_ref()
                .is_some_and(|hooks| hooks.stop == PublicationStop::SourceReady)
        {
            // This is a true terminal branch of setup. In particular the
            // Published callback/driver below is never called on this route.
            let hooks = hooks.take().unwrap();
            phase.next("source-route.source-ready-continuation");
            let result = (hooks.continuation)(owner, &fixture, &cancel);
            phase.next("source-route.source-ready-teardown");
            allocated.require_published(&staging.plan).unwrap();
            drop(checkout);
            drop(protection);
            drop(allocated);
            drop(store);
            return Some(result);
        }
    }

    if let Some(hooks) = hooks {
        // The continuation owns the original B owner through its explicit
        // positive close. C allocation and byte proofs stay borrowed here.
        phase.next("source-route.source-published-continuation");
        let result = (hooks.continuation)(owner, &fixture, &cancel);
        phase.next("source-route.source-published-teardown");
        allocated.require_published(&staging.plan).unwrap();
        drop(checkout);
        drop(protection);
        drop(allocated);
        drop(store);
        return Some(result);
    }

    // All four successful calls used the same live B owner and C protections.
    // Only its positive native close completes that boundary; the progress
    // objects above conferred neither a close witness nor C9 admission.
    phase.next("source-route.final-close-preflight");
    physical.require();
    require_source_pin(&root.join(path.as_str()));
    phase.next("source-route.close-b-owner");
    owner.close().unwrap();
    phase.next("source-route.final-reopen-and-witness");
    let reopened = ProjectionStore::open(&root).unwrap();
    assert_eq!(reopened.state().unwrap(), state);
    assert_eq!(
        source(
            &source_database,
            &reopened.marker,
            &expected_published,
            Some(&durable.receipt),
            &original_ingress
        ),
        previous_rows
    );
    drop(reopened);
    allocated.require_published(&staging.plan).unwrap();
    drop(checkout);
    drop(protection);
    drop(allocated);
    assert!(
        ProjectionStore::open(&destination_root).is_err(),
        "C remains private C8 Ready without C9 admission"
    );
    assert_eq!(snapshot(&source_database), previous_rows);
    assert_eq!(snapshot(&destination_database), durable.rows);
    assert_eq!(
        family(&family_database, &staging, &required, "ready"),
        family_rows
    );
    assert_eq!(
        witness::trees(&root, &family_root, &destination_root, locks),
        previous_trees
    );
    witness::verify_objects(&family_root, &required);
    drop(store);
    None
}
