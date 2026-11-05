//! Memory SQL DATA only; no native checkpoint or physical Ready authority.
use super::*;
use crate::projection::provider::{
    database_io::BareConnection,
    process_profile::tests::child,
    records::{
        ProviderBootstrapBinding, Receipt, Record,
        activation::ProviderActivationRecord,
        import::{ImportVersion, ProviderImportKind},
        transition::{
            ProviderTransitionIntentRecord, TransitionObservation, TransitionPlan, TransitionRole,
            TransitionRowset, TransitionState,
        },
    },
    validation::{
        active_fixture, private_active_capture_with_source, private_active_publication_with_source,
    },
};
use layrs_core::scm::{ObjectId, StepId, TreeId, WorktreeId};
use rusqlite::params;

const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
const RAW: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn prefix(
    c: &Connection,
    fixture: &crate::projection::provider::validation::tests::fixture::Fixture,
    activation: &ProviderActivationRecord,
) -> ProviderImportPrefixRecord {
    let marker = &fixture.allocation.format;
    let mut destination = marker.identity.clone();
    destination.worktree_id = WorktreeId::parse("worktree:publication-target").unwrap();
    let mut inherited = blake3::Hasher::new();
    inherited.update(b"layrs-projection-provider-import-segments-v1\0");
    inherited.update(&0u64.to_be_bytes());
    let row = |count| ImportRowset {
        count,
        digest: TOKEN.into(),
    };
    let mut prefix = ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v1".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Provider {
            identity: marker.identity.clone(),
            format_digest: crate::projection::provider::records::codec::hash(
                "layrs-projection-provider-import-format-v1",
                &marker.bytes().unwrap(),
            ),
            private_schema: 8,
            allocation: fixture.allocation.data_reference().unwrap(),
            prepared: fixture.prepared.data_reference().unwrap(),
            binding: ProviderBootstrapBinding::decode(&activation.binding.encode().unwrap())
                .unwrap(),
            activation_map_id: activation.digest().unwrap(),
            version: ImportVersion {
                generation: 1,
                snapshot_digest: activation.activated_digest.clone(),
                selection_digest: activation.selection_digest.clone(),
                capture_count: 0,
                capture_head: None,
                effective_image_digest: RAW.into(),
            },
        },
        destination_identity: destination,
        inherited_segments: ImportRowset {
            count: 0,
            digest: format!("blake3:{}", inherited.finalize()),
        },
        ancestry_root_receipt: None,
        closure: ImportClosure {
            denial_observations: None,
            restore_history: None,
            snapshots: row(2),
            selections: row(1),
            captures: row(0),
            publications: row(0),
            catalogs: row(1),
            descriptors: row(1),
            activation_maps: row(1),
            transition_receipts: row(0),
            companions: row(0),
            companion_files: row(0),
            companion_deletions: row(0),
            content_mappings: row(0),
        },
    };
    let metadata = read_source_metadata(
        c,
        marker,
        ImportDatabaseKind::Provider,
        &prefix,
        ImportSourceReferences::Provider {
            allocation: &fixture.allocation,
            prepared: &fixture.prepared,
            activation,
        },
        RAW,
        None,
    )
    .unwrap();
    prefix.closure =
        compute_scoped_closure(c, ImportDatabaseKind::Provider, &prefix, &metadata, None).unwrap();
    prefix
}

#[test]
#[ignore = "owned audited Windows SQLite process; publication SQL proof binding DATA"]
fn publication_closure_keeps_modes_distinct_and_refuses_stale_terminal_scope() {
    child::run(
        concat!(
            module_path!(),
            "::publication_closure_keeps_modes_distinct_and_refuses_stale_terminal_scope"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (fixture, _, _, activation, _) = active_fixture(c, false);
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            let prefix = prefix(c, &fixture, &activation);
            let refs = || ImportSourceReferences::Provider {
                allocation: &fixture.allocation,
                prepared: &fixture.prepared,
                activation: &activation,
            };
            let capture = private_active_capture_with_source(
                c,
                &fixture.allocation,
                &fixture.prepared,
                &activation.binding,
                None,
                None,
            )
            .unwrap();
            let publication = private_active_publication_with_source(
                c,
                &fixture.allocation,
                &fixture.prepared,
                &activation.binding,
                None,
                None,
            )
            .unwrap();
            let changes = c.total_changes();
            let strict = verify_source(
                c,
                &fixture.allocation.format,
                ImportDatabaseKind::Provider,
                &prefix,
                refs(),
                Some(&capture),
                RAW,
                None,
            )
            .unwrap();
            strict.require_provider_inputs(&capture).unwrap();
            assert!(strict.require_publication_inputs(&publication).is_err());
            let full = verify_source_publication(
                c,
                &fixture.allocation.format,
                &prefix,
                refs(),
                &publication,
                RAW,
                None,
            )
            .unwrap();
            full.require_publication_inputs(&publication).unwrap();
            assert!(full.require_provider_inputs(&capture).is_err());
            assert_eq!(strict.closure(), full.closure());
            assert_eq!(c.total_changes(), changes);
            let row = || TransitionRowset {
                count: 0,
                digest: TOKEN.into(),
            };
            let planned = ProviderTransitionIntentRecord {
                schema_version: "layrs.projection-provider-transition-intent.v1".into(),
                transaction_id: "00000000000000000000000000000002".into(),
                role: TransitionRole::Source,
                state: TransitionState::Planned,
                local_generation: 1,
                plan: TransitionPlan {
                    denial_coverage: None,
                    kind: ProviderImportKind::ManagedOrdinary,
                    source_prefix_id: prefix.digest().unwrap(),
                    source_prefix: ProviderImportPrefixRecord::decode(&prefix.encode().unwrap())
                        .unwrap(),
                    destination_parent: "C:\\publication-parent".into(),
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
                    working_overlay: row(),
                    companion_id: None,
                    companion_files: row(),
                    companion_deletions: row(),
                    protected_manifests: row(),
                    quota_bytes: 0,
                },
                destination: None,
                reservation: None,
                receipt_id: None,
            };
            c.execute("INSERT INTO projection_provider_transitions VALUES(?1,'source','planned',1,NULL,NULL,?2,?3)",
            params![planned.transaction_id,planned.encode().unwrap(),planned.digest().unwrap()]).unwrap();
            let current = private_active_publication_with_source(
                c,
                &fixture.allocation,
                &fixture.prepared,
                &activation.binding,
                None,
                None,
            )
            .unwrap();
            assert!(full.require_publication_inputs(&current).is_err());
            let changes = c.total_changes();
            assert!(
                verify_source_publication(
                    c,
                    &fixture.allocation.format,
                    &prefix,
                    refs(),
                    &publication,
                    RAW,
                    None
                )
                .is_err()
            );
            let fresh = verify_source_publication(
                c,
                &fixture.allocation.format,
                &prefix,
                refs(),
                &current,
                RAW,
                None,
            )
            .unwrap();
            fresh.require_publication_inputs(&current).unwrap();
            assert_eq!(
                fresh.closure(),
                strict.closure(),
                "outgoing intent is not incoming ancestry"
            );
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}
