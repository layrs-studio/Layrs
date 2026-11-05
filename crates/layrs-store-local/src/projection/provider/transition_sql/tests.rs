use super::*;
use crate::projection::provider::{
    database_io::BareConnection,
    import_validation::{compute_scoped_closure, read_source_metadata},
    process_profile::tests::child,
    records::{
        Receipt,
        import::{
            ImportClosure, ImportRowset, ImportSource, ImportVersion, ProviderImportKind,
            ProviderImportPrefixRecord,
        },
        transition::{TransitionObservation, TransitionPlan, TransitionRowset},
    },
    validation::{active_fixture, private_active_capture_with_source},
};
use layrs_core::scm::{ObjectId, StepId, TreeId, WorktreeId};

#[path = "publication_tests.rs"]
mod publication_tests;

const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
const RAW: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TX: &str = "00000000000000000000000000000002";

fn rowset(count: u64) -> ImportRowset {
    ImportRowset {
        count,
        digest: TOKEN.into(),
    }
}
fn trowset() -> TransitionRowset {
    TransitionRowset {
        count: 0,
        digest: TOKEN.into(),
    }
}

pub(super) fn planned(
    c: &Connection,
    fixture: &crate::projection::provider::validation::tests::fixture::Fixture,
    activation: &crate::projection::provider::records::activation::ProviderActivationRecord,
    activated: &crate::projection::VersionedProjection,
) -> ProviderTransitionIntentRecord {
    let a = &fixture.allocation;
    let p = &fixture.prepared;
    let mut destination_identity = a.format.identity.clone();
    destination_identity.worktree_id =
        WorktreeId::parse("worktree:transition-destination").unwrap();
    let mut inherited = blake3::Hasher::new();
    inherited.update(b"layrs-projection-provider-import-segments-v1\0");
    inherited.update(&0u64.to_be_bytes());
    let mut prefix = ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v1".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Provider {
            identity: a.format.identity.clone(),
            format_digest: super::super::records::codec::hash(
                "layrs-projection-provider-import-format-v1",
                &a.format.bytes().unwrap(),
            ),
            private_schema: 8,
            allocation: a.data_reference().unwrap(),
            prepared: p.data_reference().unwrap(),
            binding: ProviderBootstrapBinding::decode(&activation.binding.encode().unwrap())
                .unwrap(),
            activation_map_id: activation.digest().unwrap(),
            version: ImportVersion {
                generation: activated.generation,
                snapshot_digest: activated.digest.clone(),
                selection_digest: activation.selection_digest.clone(),
                capture_count: 0,
                capture_head: None,
                effective_image_digest: RAW.into(),
            },
        },
        destination_identity,
        inherited_segments: ImportRowset {
            count: 0,
            digest: format!("blake3:{}", inherited.finalize()),
        },
        ancestry_root_receipt: None,
        closure: ImportClosure {
            denial_observations: None,
            restore_history: None,
            snapshots: rowset(2),
            selections: rowset(1),
            captures: rowset(0),
            publications: rowset(0),
            catalogs: rowset(1),
            descriptors: rowset(1),
            activation_maps: rowset(1),
            transition_receipts: rowset(0),
            companions: rowset(0),
            companion_files: rowset(0),
            companion_deletions: rowset(0),
            content_mappings: rowset(0),
        },
    };
    let metadata = read_source_metadata(
        c,
        &a.format,
        ImportDatabaseKind::Provider,
        &prefix,
        ImportSourceReferences::Provider {
            allocation: a,
            prepared: p,
            activation,
        },
        RAW,
        None,
    )
    .unwrap();
    prefix.closure =
        compute_scoped_closure(c, ImportDatabaseKind::Provider, &prefix, &metadata, None).unwrap();
    let prefix_id = prefix.digest().unwrap();
    ProviderTransitionIntentRecord {
        schema_version: "layrs.projection-provider-transition-intent.v1".into(),
        transaction_id: TX.into(),
        role: TransitionRole::Source,
        state: TransitionState::Planned,
        local_generation: activated.generation,
        plan: TransitionPlan {
            denial_coverage: None,
            kind: ProviderImportKind::ManagedOrdinary,
            source_prefix_id: prefix_id,
            source_prefix: prefix,
            destination_parent: "C:\\transition-parent".into(),
            destination_parent_identity: "fs:v1:1111111111111111:2222222222222222".into(),
            destination_leaf_nonce: "00000000000000000000000000000001".into(),
            observation: TransitionObservation {
                step_id: StepId::parse(TOKEN).unwrap(),
                root_tree_id: TreeId::parse(TOKEN).unwrap(),
                policy_set_id: ObjectId::parse(TOKEN).unwrap(),
                authorization_epoch: 1,
                generation: 2,
                snapshot_digest: RAW.into(),
                selection_digest: RAW.into(),
            },
            placement_generation: 3,
            placement_digest: RAW.into(),
            capture_sequence: 1,
            capture_digest: TOKEN.into(),
            working_overlay: trowset(),
            companion_id: None,
            companion_files: trowset(),
            companion_deletions: trowset(),
            protected_manifests: trowset(),
            quota_bytes: 0,
        },
        destination: None,
        reservation: None,
        receipt_id: None,
    }
}

#[test]
#[ignore = "owned audited Windows SQLite process; caller-owned transition SQL"]
fn source_intent_begin_retry_cas_cancel_and_rollback() {
    child::run(
        concat!(
            module_path!(),
            "::source_intent_begin_retry_cas_cancel_and_rollback"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (f, _, _, activation, activated) = active_fixture(c, false);
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            let initial = planned(c, &f, &activation, &activated);
            let binding = &activation.binding;
            let apply = |previous, next, cancel| {
                apply_source_intent(
                    c,
                    &f.allocation,
                    &f.prepared,
                    binding,
                    [0x41; 16],
                    previous,
                    next,
                    cancel,
                )
            };
            assert_eq!(
                apply(None, &initial, None).unwrap(),
                SourceIntentEffect::Inserted
            );
            assert_eq!(
                apply(None, &initial, None).unwrap(),
                SourceIntentEffect::AlreadyPresent
            );
            let mut forged =
                ProviderTransitionIntentRecord::decode(&initial.encode().unwrap()).unwrap();
            forged.plan.source_prefix.closure.selections.digest =
                "blake3:2222222222222222222222222222222222222222222222222222222222222222".into();
            forged.plan.source_prefix_id = forged.plan.source_prefix.digest().unwrap();
            c.execute(
                "UPDATE projection_provider_transitions SET payload=?1,digest=?2 WHERE id=?3",
                rusqlite::params![
                    forged.encode().unwrap(),
                    forged.digest().unwrap(),
                    forged.transaction_id
                ],
            )
            .unwrap();
            assert!(apply(None, &forged, None).is_err());
            c.execute(
                "UPDATE projection_provider_transitions SET payload=?1,digest=?2 WHERE id=?3",
                rusqlite::params![
                    initial.encode().unwrap(),
                    initial.digest().unwrap(),
                    initial.transaction_id
                ],
            )
            .unwrap();
            assert_eq!(
                apply(None, &initial, None).unwrap(),
                SourceIntentEffect::AlreadyPresent
            );
            assert!(
                private_active_capture_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    binding,
                    Some([0x41; 16]),
                    None
                )
                .is_err()
            );
            let mut reserved =
                ProviderTransitionIntentRecord::decode(&initial.encode().unwrap()).unwrap();
            reserved.reservation = Some(reserved.reservation_operation().unwrap());
            let mut wrong =
                ProviderTransitionIntentRecord::decode(&initial.encode().unwrap()).unwrap();
            wrong.plan.quota_bytes = 1;
            assert!(apply(Some(&wrong), &reserved, None).is_err());
            assert_eq!(
                apply(Some(&initial), &reserved, None).unwrap(),
                SourceIntentEffect::Updated
            );
            assert_eq!(
                apply(Some(&initial), &reserved, None).unwrap(),
                SourceIntentEffect::AlreadyPresent
            );
            let mut cancelled =
                ProviderTransitionIntentRecord::decode(&reserved.encode().unwrap()).unwrap();
            cancelled.state = TransitionState::Cancelled;
            let token = Cancellation::default();
            token.cancel();
            assert!(apply(Some(&reserved), &cancelled, Some(&token)).is_err());
            assert_eq!(
                apply(Some(&reserved), &cancelled, None).unwrap(),
                SourceIntentEffect::Updated
            );
            c.execute_batch("ROLLBACK").unwrap();
            let rows: u64 = c
                .query_row(
                    "SELECT count(*) FROM projection_provider_transitions",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(rows, 0);
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            assert_eq!(
                apply(None, &initial, None).unwrap(),
                SourceIntentEffect::Inserted
            );
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}
