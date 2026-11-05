//! Owned scalar SQL history, using production DDL and existing pure fixtures.
//! These recipes authenticate stored incoming receipt DATA, never a live source,
//! destination Ready, filesystem owner, or remote authorization capability.
use super::*;
use crate::projection::{
    capture::CapturedPatch,
    provider::{
        database_io::BareConnection,
        process_profile::tests::child,
        records::{
            codec,
            denied_coverage::DeniedAuthorityData,
            import::{imported_segments_rowset, ImportVersion, ProviderImportKind},
        },
        validation::tests::fixture::Fixture,
    },
};
use layrs_core::{
    projection::{PathSemantics, ProjectionSelection},
    scm::{RepoPath, WorktreeId},
};
use rusqlite::params;
const ID: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
fn row(count: u64) -> ImportRowset {
    ImportRowset {
        count,
        digest: ID.into(),
    }
}
fn closure(snapshots: u64, selections: u64, captures: u64, receipts: u64) -> ImportClosure {
    ImportClosure {
        snapshots: row(snapshots),
        selections: row(selections),
        captures: row(captures),
        publications: row(0),
        catalogs: row(0),
        descriptors: row(0),
        activation_maps: row(0),
        transition_receipts: row(receipts),
        companions: row(0),
        companion_files: row(0),
        companion_deletions: row(0),
        content_mappings: row(0),
        restore_history: None,
        denial_observations: None,
    }
}
fn fixture(c: &Connection) -> (ProviderImportPrefixRecord, TransitionDeniedCoverageRecord) {
    c.execute_batch(
        &[
            database::SQL,
            selection::SQL,
            retention::SQL,
            crate::projection::provider::schema::SQL,
            "PRAGMA user_version=8; PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;",
        ]
        .concat(),
    )
    .unwrap();
    let fixture = Fixture::new("G1", false);
    let initial = fixture.source.initial().clone();
    let source = fixture.allocation.format.identity.clone();
    let mut destination = source.clone();
    destination.worktree_id = WorktreeId::parse("worktree:denial-incoming-b").unwrap();
    let old_payload = initial.canonical_bytes().unwrap();
    let old_digest = database::digest(0, None, &old_payload);
    c.execute(
        "INSERT INTO projection_snapshots VALUES(0,?1,NULL,?2)",
        params![old_digest, old_payload],
    )
    .unwrap();
    selection::insert(c, 0, &ProjectionSelection::All).unwrap();
    let old_selection: String = c
        .query_row(
            "SELECT digest FROM projection_selections WHERE generation=0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let old = ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v1".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Ordinary {
            identity: source.clone(),
            format_digest: ID.into(),
            private_schema: 8,
            checkout_identity: None,
            admission: None,
            version: ImportVersion {
                generation: 0,
                snapshot_digest: old_digest.clone(),
                selection_digest: old_selection,
                capture_count: 0,
                capture_head: None,
                effective_image_digest: "a".repeat(64),
            },
        },
        destination_identity: destination.clone(),
        inherited_segments: imported_segments_rowset(&[]).unwrap(),
        ancestry_root_receipt: None,
        closure: closure(1, 1, 0, 0),
    };
    let mut observed = initial;
    observed.observed_authorization_epoch += 1;
    let payload = observed.canonical_bytes().unwrap();
    let observation = database::digest(1, Some(&old_digest), &payload);
    c.execute(
        "INSERT INTO projection_snapshots VALUES(1,?1,?2,?3)",
        params![observation, old_digest, payload],
    )
    .unwrap();
    selection::insert(c, 1, &ProjectionSelection::All).unwrap();
    let observed_selection: String = c
        .query_row(
            "SELECT digest FROM projection_selections WHERE generation=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let placement = database::digest(2, Some(&observation), &payload);
    c.execute(
        "INSERT INTO projection_snapshots VALUES(2,?1,?2,?3)",
        params![placement, observation, payload],
    )
    .unwrap();
    selection::insert(c, 2, &ProjectionSelection::All).unwrap();
    let mut patch = CapturedPatch {
        schema_version: "layrs.projection-capture.v1".into(),
        sequence: 1,
        previous_patch_digest: None,
        projection_generation: 2,
        projection_digest: placement.clone(),
        identity: destination.clone(),
        changes: Vec::new(),
        digest: String::new(),
    };
    patch.digest = patch.computed_digest().unwrap();
    c.execute(
        "INSERT INTO projection_captures VALUES(1,?1,2,?2)",
        params![patch.digest, canonical_json(&patch).unwrap()],
    )
    .unwrap();
    let authority = DeniedAuthorityData::ReobservedExpandedScope {
        policy_set_id: observed.observed_policy_set_id.clone(),
        authorization_epoch: observed.observed_authorization_epoch,
    };
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-transition-denied-paths-data-v1\0");
    hash.update(&1u64.to_be_bytes());
    hash.update(&12u64.to_be_bytes());
    hash.update(b"\"secret.txt\"");
    let body = TransitionDeniedCoverageRecord {
        schema_version: "layrs.projection-transition-denied-coverage.v1".into(),
        transaction_id: "00000000000000000000000000000001".into(),
        source_prefix_id: old.digest().unwrap(),
        source_identity: source.clone(),
        destination_identity: destination.clone(),
        source_version: old.source.version().clone(),
        step_id: observed.base_step_id.clone(),
        root_tree_id: observed.base_root_tree_id.clone(),
        path_semantics: observed.path_semantics,
        authority_data_digest: codec::hash(
            "layrs-provider-transition-authority-data-v1",
            &codec::encode(&authority, 8192).unwrap(),
        ),
        authority,
        observation_generation: 1,
        observation_snapshot_digest: observation.clone(),
        observation_selection_digest: observed_selection,
        denied_path_count: 1,
        path_bytes: 10,
        rows_digest: format!("blake3:{}", hash.finalize()),
        denied_paths: vec![RepoPath::parse("secret.txt").unwrap()],
    };
    let receipt = ProviderTransitionReceiptRecord {
        schema_version: "layrs.projection-provider-transition-receipt.v2".into(),
        owner_identity_digest: ID.into(),
        transaction_id: body.transaction_id.clone(),
        transition_plan_digest: ID.into(),
        destination_proof_digest: ID.into(),
        source_prefix_id: old.digest().unwrap(),
        source_prefix: old,
        parent_receipt_id: None,
        source_identity: source,
        destination_identity: destination.clone(),
        imported_max_generation: 0,
        destination_observation_generation: 1,
        destination_observation_digest: observation,
        destination_placement_generation: 2,
        destination_placement_digest: placement.clone(),
        imported_capture_count: 0,
        destination_capture_sequence: 1,
        destination_capture_digest: patch.digest.clone(),
        protected_manifests: row(0),
        content_mappings: row(0),
        companion_id: None,
        companion_files: row(0),
        companion_deletions: row(0),
        denial_coverage: Some(body.reference().unwrap()),
    };
    insert(c, &receipt, &body);
    let segment = ImportedSegment::from_receipt(&receipt, None).unwrap();
    let mut future = destination.clone();
    future.worktree_id = WorktreeId::parse("worktree:denial-outgoing-c").unwrap();
    let selection_digest: String = c
        .query_row(
            "SELECT digest FROM projection_selections WHERE generation=2",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mut prefix = ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v3".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Ordinary {
            identity: destination,
            format_digest: ID.into(),
            private_schema: 8,
            checkout_identity: Some("fs:v1:0000000000000001:0000000000000002".into()),
            admission: None,
            version: ImportVersion {
                generation: 2,
                snapshot_digest: placement,
                selection_digest,
                capture_count: 1,
                capture_head: Some(patch.digest),
                effective_image_digest: "a".repeat(64),
            },
        },
        destination_identity: future,
        inherited_segments: imported_segments_rowset(&[segment]).unwrap(),
        ancestry_root_receipt: Some(receipt.digest().unwrap()),
        closure: closure(3, 3, 1, 1),
    };
    prefix.closure.restore_history = Some(row(0));
    prefix.closure.denial_observations = Some(row(1));
    prefix.validate_fields().unwrap();
    (prefix, body)
}
fn insert(
    c: &Connection,
    receipt: &ProviderTransitionReceiptRecord,
    body: &TransitionDeniedCoverageRecord,
) {
    c.execute(
        "INSERT INTO projection_observations VALUES(?1,?2)",
        params![body.reference().unwrap().id, body.encode().unwrap()],
    )
    .unwrap();
    c.execute(
        "INSERT INTO projection_provider_transition_receipts VALUES(?1,?2,?3,NULL,?4)",
        params![
            receipt.digest().unwrap(),
            receipt.owner_identity_digest,
            receipt.transaction_id,
            receipt.encode().unwrap()
        ],
    )
    .unwrap();
}
fn scoped(c: &Connection, prefix: &ProviderImportPrefixRecord) -> CauseResult<ImportClosure> {
    scoped_as(c, prefix, ImportDatabaseKind::Provider)
}
fn scoped_as(
    c: &Connection,
    prefix: &ProviderImportPrefixRecord,
    kind: ImportDatabaseKind,
) -> CauseResult<ImportClosure> {
    let lineage = crate::projection::provider::import_validation::ancestry::read_ancestry_rows(
        c, kind, prefix, None,
    )?;
    let ids: BTreeSet<_> = lineage
        .receipts()
        .iter()
        .map(|r| r.digest().unwrap())
        .collect();
    let empty = BTreeSet::new();
    let lengths = BTreeMap::new();
    let prefix_id = prefix.digest()?;
    let view = ScopeView {
        kind,
        prefix_id: &prefix_id,
        catalog_ids: &empty,
        activation_ids: &empty,
        receipt_ids: &ids,
        companion_ids: &empty,
        content_file_ids: &empty,
        capture_lengths: &lengths,
        receipts: lineage.receipts(),
        lineage: &lineage,
    };
    compute_from_scope(c, kind, prefix, &view, None)
}

#[test]
#[ignore = "owned audited SQL child120; incoming receipt denial transport DATA only"]
fn inherited_denial_scope_uses_only_incoming_receipts_and_rejects_legacy_omission() {
    child::run(
        concat!(
            module_path!(),
            "::inherited_denial_scope_uses_only_incoming_receipts_and_rejects_legacy_omission"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (prefix, body) = fixture(c);
            let expected = scoped(c, &prefix).unwrap();
            assert_eq!(expected.denial_observations.as_ref().unwrap().count, 1);
            // A current/unrelated outgoing body and receipt in the same source DB
            // are not in the incoming ancestry root, so neither enters the closure.
            let mut current =
                TransitionDeniedCoverageRecord::decode(&body.encode().unwrap()).unwrap();
            current.transaction_id = "00000000000000000000000000000002".into();
            let payload: Vec<u8> = c
                .query_row(
                    "SELECT payload FROM projection_provider_transition_receipts WHERE id=?1",
                    [prefix.ancestry_root_receipt.as_ref().unwrap()],
                    |r| r.get(0),
                )
                .unwrap();
            let mut outgoing = ProviderTransitionReceiptRecord::decode(&payload).unwrap();
            outgoing.transaction_id = current.transaction_id.clone();
            outgoing.denial_coverage = Some(current.reference().unwrap());
            insert(c, &outgoing, &current);
            assert_eq!(scoped(c, &prefix).unwrap(), expected);
            let mut legacy = ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap();
            legacy.schema_version = "layrs.projection-provider-import-prefix.v2".into();
            legacy.closure.denial_observations = None;
            assert!(scoped(c, &legacy)
                .err()
                .unwrap()
                .to_string()
                .contains("denial"));
            // v3 also represents an empty incoming denial inventory. Existing
            // foreign documents stay outside its exact selector.
            let mut empty = ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap();
            empty.ancestry_root_receipt = None;
            empty.inherited_segments = imported_segments_rowset(&[]).unwrap();
            empty.closure.transition_receipts = row(0);
            assert_eq!(
                scoped(c, &empty)
                    .unwrap()
                    .denial_observations
                    .unwrap()
                    .count,
                0
            );
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited SQL child120; missing/substituted denial body DATA only"]
fn inherited_denial_missing_substituted_and_rehashed_semantics_are_not_repaired() {
    child::run(
        concat!(
            module_path!(),
            "::inherited_denial_missing_substituted_and_rehashed_semantics_are_not_repaired"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (mut prefix, body) = fixture(c);
            let id = body.reference().unwrap().id;
            c.execute_batch("SAVEPOINT missing").unwrap();
            c.execute("DELETE FROM projection_observations WHERE digest=?1", [&id])
                .unwrap();
            let before = c.total_changes();
            assert!(scoped(c, &prefix).is_err());
            assert_eq!(before, c.total_changes());
            c.execute_batch("ROLLBACK TO missing; RELEASE missing")
                .unwrap();
            c.execute_batch("SAVEPOINT substituted").unwrap();
            c.execute(
                "UPDATE projection_observations SET payload=?1 WHERE digest=?2",
                params![b"{}".as_slice(), id],
            )
            .unwrap();
            let before = c.total_changes();
            assert!(scoped(c, &prefix).is_err());
            assert_eq!(before, c.total_changes());
            c.execute_batch("ROLLBACK TO substituted; RELEASE substituted")
                .unwrap();
            let mut wrong =
                TransitionDeniedCoverageRecord::decode(&body.encode().unwrap()).unwrap();
            wrong.path_semantics = match body.path_semantics {
                PathSemantics::PortableWindows => PathSemantics::CaseSensitive,
                PathSemantics::CaseSensitive => PathSemantics::PortableWindows,
            };
            let old_receipt_id = prefix.ancestry_root_receipt.clone().unwrap();
            let payload: Vec<u8> = c
                .query_row(
                    "SELECT payload FROM projection_provider_transition_receipts WHERE id=?1",
                    [&old_receipt_id],
                    |r| r.get(0),
                )
                .unwrap();
            let mut receipt = ProviderTransitionReceiptRecord::decode(&payload).unwrap();
            receipt.denial_coverage = Some(wrong.reference().unwrap());
            c.execute(
                "DELETE FROM projection_provider_transition_receipts WHERE id=?1",
                [&old_receipt_id],
            )
            .unwrap();
            insert(c, &receipt, &wrong);
            prefix.ancestry_root_receipt = Some(receipt.digest().unwrap());
            prefix.inherited_segments =
                imported_segments_rowset(&[ImportedSegment::from_receipt(&receipt, None).unwrap()])
                    .unwrap();
            let before = c.total_changes();
            let error = scoped(c, &prefix).err().unwrap();
            assert!(error.to_string().contains("denied coverage"), "{error}");
            assert_eq!(before, c.total_changes());
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited SQL child120; empty ordinary6 prefix-v3 commitment DATA"]
fn ordinary6_empty_denial_scope_needs_no_observation_table() {
    child::run(
        concat!(
            module_path!(),
            "::ordinary6_empty_denial_scope_needs_no_observation_table"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            c.execute_batch(
                &[
                    database::SQL,
                    selection::SQL,
                    "PRAGMA user_version=6; PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;",
                ]
                .concat(),
            )
            .unwrap();
            let fixture = Fixture::new("G1", false);
            let snapshot = fixture.source.initial();
            let payload = snapshot.canonical_bytes().unwrap();
            let digest = database::digest(0, None, &payload);
            c.execute(
                "INSERT INTO projection_snapshots VALUES(0,?1,NULL,?2)",
                params![digest, payload],
            )
            .unwrap();
            selection::insert(c, 0, &ProjectionSelection::All).unwrap();
            let selected: String = c
                .query_row(
                    "SELECT digest FROM projection_selections WHERE generation=0",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let source = fixture.allocation.format.identity.clone();
            let mut destination = source.clone();
            destination.worktree_id = WorktreeId::parse("worktree:empty-v3").unwrap();
            let mut prefix = ProviderImportPrefixRecord {
                schema_version: "layrs.projection-provider-import-prefix.v3".into(),
                kind: ProviderImportKind::ManagedOrdinary,
                source: ImportSource::Ordinary {
                    identity: source,
                    format_digest: ID.into(),
                    private_schema: 6,
                    admission: None,
                    checkout_identity: Some("fs:v1:0000000000000001:0000000000000002".into()),
                    version: ImportVersion {
                        generation: 0,
                        snapshot_digest: digest,
                        selection_digest: selected,
                        capture_count: 0,
                        capture_head: None,
                        effective_image_digest: "a".repeat(64),
                    },
                },
                destination_identity: destination,
                inherited_segments: imported_segments_rowset(&[]).unwrap(),
                ancestry_root_receipt: None,
                closure: closure(1, 1, 0, 0),
            };
            prefix.closure.restore_history = Some(row(0));
            prefix.closure.denial_observations = Some(row(0));
            let computed = scoped_as(c, &prefix, ImportDatabaseKind::Ordinary).unwrap();
            assert_eq!(computed.denial_observations.unwrap().count, 0);
            assert!(computed.restore_history.is_some());
            let table:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='projection_observations')",[],|r|r.get(0)).unwrap();
            assert!(!table);
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}
