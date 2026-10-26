use super::*;
use crate::projection::provider::{
    database_io::BareConnection,
    process_profile::tests::child,
    records::{
        Receipt,
        import::{
            ImportClosure, ImportRowset, ImportVersion, ProviderImportKind,
            ProviderImportPrefixRecord,
        },
        transition::{TransitionObservation, TransitionPlan, TransitionRowset},
    },
};
use layrs_core::scm::{ObjectId, StepId, TreeId, WorktreeId};
use rusqlite::params;

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
fn planned(
    f: &super::super::tests::fixture::Fixture,
    mount: &crate::projection::provider::records::activation::ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    activated: &VersionedProjection,
) -> ProviderTransitionIntentRecord {
    let mut destination = f.allocation.format.identity.clone();
    destination.worktree_id = WorktreeId::parse("worktree:transition-destination").unwrap();
    let binding =
        serde_json::from_slice(&serde_json::to_vec(&mount.prepared.binding).unwrap()).unwrap();
    let prefix = ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v1".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Provider {
            identity: f.allocation.format.identity.clone(),
            format_digest: records::codec::hash(
                "layrs-projection-provider-import-format-v1",
                &f.allocation.format.bytes().unwrap(),
            ),
            private_schema: 8,
            allocation: f.allocation.data_reference().unwrap(),
            prepared: f.prepared.data_reference().unwrap(),
            binding,
            activation_map_id: activation.digest().unwrap(),
            version: ImportVersion {
                generation: activated.generation,
                snapshot_digest: activated.digest.clone(),
                selection_digest: f.source.commitment().selection_digest.clone(),
                capture_count: 0,
                capture_head: None,
                effective_image_digest: RAW.into(),
            },
        },
        destination_identity: destination,
        inherited_segments: rowset(0),
        ancestry_root_receipt: None,
        closure: ImportClosure {
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
    let prefix_id = prefix.digest().unwrap();
    ProviderTransitionIntentRecord {
        schema_version: "layrs.projection-provider-transition-intent.v1".into(),
        transaction_id: TX.into(),
        role: TransitionRole::Source,
        state: TransitionState::Planned,
        local_generation: activated.generation,
        plan: TransitionPlan {
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

fn insert(c: &Connection, intent: &ProviderTransitionIntentRecord) {
    c.execute(
        "INSERT INTO projection_provider_transitions(id,role,state,local_generation,reservation,receipt_id,payload,digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![intent.transaction_id, "source", "planned", intent.local_generation,
            intent.reservation, intent.receipt_id, intent.encode().unwrap(), intent.digest().unwrap()],
    ).unwrap();
}

#[test]
#[ignore = "owned audited Windows SQLite process; typed source transition rows"]
fn planned_source_is_exact_and_legacy_active_capture_stays_strict() {
    child::run(
        concat!(
            module_path!(),
            "::planned_source_is_exact_and_legacy_active_capture_stays_strict"
        ),
        120,
        || {
            child::declare();
            let database = BareConnection::reference().unwrap();
            let c = database.connection();
            let (f, _, mount, activation, activated) = super::super::active_fixture(c, false);
            let intent = planned(&f, &mount, &activation, &activated);
            insert(c, &intent);
            let proof = super::super::private_active_transition_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &mount.prepared.binding,
                None,
                None,
            )
            .unwrap();
            assert_eq!(proof.transitions.len(), 1);
            assert_eq!(proof.active.activated.digest, activated.digest);
            assert!(proof.capture.patches.is_empty());
            assert!(
                super::super::private_active_capture_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &mount.prepared.binding,
                    None,
                    None,
                )
                .is_err()
            );
            let mut competing =
                ProviderTransitionIntentRecord::decode(&intent.encode().unwrap()).unwrap();
            competing.transaction_id = "00000000000000000000000000000003".into();
            assert!(c.execute(
                "INSERT INTO projection_provider_transitions(id,role,state,local_generation,reservation,receipt_id,payload,digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![competing.transaction_id, "source", "planned", competing.local_generation,
                    competing.reservation, competing.receipt_id, competing.encode().unwrap(), competing.digest().unwrap()],
            ).is_err());
            let retained = super::super::private_active_transition_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &mount.prepared.binding,
                None,
                None,
            )
            .unwrap();
            assert_eq!(retained.transitions.len(), 1);
            assert_eq!(retained.transitions[0], intent);
            let mut forged =
                ProviderTransitionIntentRecord::decode(&intent.encode().unwrap()).unwrap();
            if let ImportSource::Provider { version, .. } = &mut forged.plan.source_prefix.source {
                version.snapshot_digest = RAW.into();
            }
            forged.plan.source_prefix_id = forged.plan.source_prefix.digest().unwrap();
            c.execute(
                "UPDATE projection_provider_transitions SET payload=?1,digest=?2",
                params![forged.encode().unwrap(), forged.digest().unwrap()],
            )
            .unwrap();
            assert!(
                super::super::private_active_transition_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &mount.prepared.binding,
                    None,
                    None,
                )
                .is_err()
            );
            c.execute(
                "UPDATE projection_provider_transitions SET payload=?1,digest=?2",
                params![intent.encode().unwrap(), intent.digest().unwrap()],
            )
            .unwrap();
            c.execute(
                "UPDATE projection_provider_transitions SET role='destination'",
                [],
            )
            .unwrap();
            assert!(
                super::super::private_active_transition_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &mount.prepared.binding,
                    None,
                    None,
                )
                .is_err()
            );
            c.execute(
                "UPDATE projection_provider_transitions SET role='source',digest=?1",
                [TOKEN],
            )
            .unwrap();
            assert!(
                super::super::private_active_transition_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &mount.prepared.binding,
                    None,
                    None,
                )
                .is_err()
            );
            database.finish(Ok(())).unwrap();
        },
    );
}
