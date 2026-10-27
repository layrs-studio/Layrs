//! Owned SQL DATA only. The genuine private9 interpreter supplies the source
//! fixture; declared image/manifests and synthetic C identities grant no bytes,
//! physical destination ownership, family authorization or ordinary admission.
use super::*;

use crate::projection::{
    capture::{CapturedFile, CapturedPatch},
    offline,
    provider::{
        bootstrap::{derive_ordinary_data, ImageFile, RequiredManifest},
        database_io::BareConnection,
        managed_admission::tests::with_admitted_source_sql,
        ordinary_transition,
        process_profile::tests::child,
        records::transition::TransitionDestinationProof,
        transition_sql::SourceIntentEffect,
    },
    restore::{RestoreDestination, RestorePreview, RestoreSource},
};
use layrs_core::scm::{
    canonical_json, Chunk, ChunkingDescriptor, FileManifest, TreeEntryTarget, WorktreeId,
};
use std::collections::BTreeMap;

const IMAGE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN: &str = "blake3:0000000000000000000000000000000000000000000000000000000000000000";
const TX: &str = "000000000000000000000000000000a1";
const LEAF: &str = "000000000000000000000000000000a2";
const PARENT: &str = "C:\\ordinary-copied-parent";
const OPERATION: &str = "8-1";

macro_rules! owned {
    ($name:ident, $body:block) => {
        #[test]
        #[ignore = "ROOT source release and audited owned SQL child120/outer180 required"]
        fn $name() {
            child::run(
                concat!(module_path!(), "::", stringify!($name)),
                120,
                || {
                    child::declare();
                    $body
                },
            );
        }
    };
}

#[path = "tests/staging.rs"]
mod staging;

fn append_restore(
    c: &Connection,
    marker: &format::Marker,
    manifests: &[FileManifest],
) -> (FileManifest, Vec<u8>) {
    let generation: u64 = c
        .query_row("SELECT generation FROM projection_head", [], |row| {
            row.get(0)
        })
        .unwrap();
    let current = offline::snapshot_at(c, generation, None).unwrap();
    let entry = &current.snapshot.entries[0];
    let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
        panic!("fixture base file required")
    };
    let after = manifests
        .iter()
        .find(|manifest| manifest.file_id() == file_id)
        .unwrap();
    let before = FileManifest::from_chunks(
        ChunkingDescriptor::single(),
        &[Chunk::new(b"ordinary copied restore-only preimage".to_vec()).unwrap()],
    )
    .unwrap();
    assert_ne!(before.file_id(), after.file_id());
    let (last_sequence, last_digest): (u64, String) = c
        .query_row(
            "SELECT sequence,digest FROM projection_captures ORDER BY sequence DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let mut patch = CapturedPatch {
        schema_version: "layrs.projection-capture.v1".into(),
        sequence: last_sequence + 1,
        previous_patch_digest: Some(last_digest),
        projection_generation: generation,
        projection_digest: current.digest.clone(),
        identity: marker.identity.clone(),
        changes: vec![],
        digest: String::new(),
    };
    patch.digest = patch.computed_digest().unwrap();
    c.execute(
        "INSERT INTO projection_captures VALUES(?1,?2,?3,?4)",
        params![
            patch.sequence,
            patch.digest,
            generation,
            canonical_json(&patch).unwrap()
        ],
    )
    .unwrap();
    c.execute(
        "INSERT INTO projection_content VALUES(?1,?2,?3)",
        params![
            before.file_id().as_str(),
            before.id().as_str(),
            before.byte_length()
        ],
    )
    .unwrap();
    let file = |manifest: &FileManifest, executable| CapturedFile {
        file_id: manifest.file_id().clone(),
        bytes: manifest.byte_length(),
        executable,
    };
    let mut preview = RestorePreview {
        operation_id: OPERATION.into(),
        destination: Some(RestoreDestination {
            identity: marker.identity.clone(),
            checkout_identity: marker.checkout_identity().into(),
        }),
        source: RestoreSource::Capture {
            sequence: patch.sequence,
            paths: vec![entry.path.clone()],
        },
        projection_generation: generation,
        projection_digest: current.digest,
        checkout_digest: TOKEN.into(),
        after_checkout_digest: TOKEN.into(),
        before: BTreeMap::from([(entry.path.clone(), Some(file(&before, false)))]),
        after: BTreeMap::from([(entry.path.clone(), Some(file(after, entry.executable)))]),
        digest: String::new(),
    };
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-projection-restore-preview-v1\0");
    hash.update(&canonical_json(&preview).unwrap());
    preview.digest = format!("blake3:{}", hash.finalize());
    let old = BTreeMap::from([(entry.path.clone(), "1:0:21")]);
    let new = BTreeMap::from([(entry.path.clone(), "1:0:22")]);
    let history = canonical_json(&serde_json::json!({
        "schemaVersion": "layrs.projection-restore.v1", "preview": preview,
        "directory": "restore-8-1", "directoryIdentity": "fs:v1:0000000000000001:0000000000000002",
        "oldIdentity": old,
        "newIdentity": new,
    }))
    .unwrap();
    c.execute(
        "INSERT INTO projection_restore_history VALUES(?1,?2,?3)",
        params![OPERATION, patch.sequence, &history],
    )
    .unwrap();
    (before, history)
}

fn source_copying(
    c: &Connection,
    marker: &format::Marker,
    known: &[FileManifest],
) -> (
    ProviderTransitionIntentRecord,
    VerifiedImportClosure,
    format::Marker,
    Vec<u8>,
) {
    let (preimage, history) = append_restore(c, marker, known);
    let mut supplied: BTreeMap<_, _> = known
        .iter()
        .cloned()
        .map(|manifest| {
            (
                (manifest.file_id().clone(), manifest.id().clone()),
                manifest,
            )
        })
        .collect();
    supplied.insert(
        (preimage.file_id().clone(), preimage.id().clone()),
        preimage.clone(),
    );
    let known: Vec<_> = supplied.into_values().collect();
    let mut destination_identity = marker.identity.clone();
    destination_identity.worktree_id = WorktreeId::parse("worktree:ordinary-copied-sql").unwrap();
    let (prefix, closure) = import_validation::plan_admitted_import(
        c,
        marker,
        destination_identity.clone(),
        IMAGE,
        None,
    )
    .unwrap();
    assert_eq!(prefix.closure.restore_history.as_ref().unwrap().count, 1);
    let mappings = closure.copied_content_mappings(c, &prefix, None).unwrap();
    assert!(
        mappings.contains_key(preimage.file_id()),
        "restore-only preimage must be in copied scope"
    );
    let historical: Vec<_> = mappings
        .iter()
        .map(|(file, (manifest, bytes))| RequiredManifest {
            file_id: file.clone(),
            manifest_id: manifest.clone(),
            bytes: *bytes,
        })
        .collect();
    let ImportSource::Ordinary { version, .. } = &prefix.source else {
        unreachable!()
    };
    let current =
        offline::snapshot_at(c, version.generation, Some(&version.snapshot_digest)).unwrap();
    let selected = selection::at(c, version.generation).unwrap();
    // The fixture exposes every manifest it knows, including the original
    // Provider descriptor. Only exact historical mappings and current selected
    // bases belong to this ordinary export; a spare known representation is
    // correctly rejected by the production complete-union check.
    let mut supplied = BTreeMap::new();
    for (file, (id, bytes)) in &mappings {
        let manifest = known
            .iter()
            .find(|manifest| {
                manifest.file_id() == file
                    && manifest.id() == id
                    && manifest.byte_length() == *bytes
            })
            .expect("every authenticated historical mapping has a known manifest");
        supplied.insert((file.clone(), id.clone()), manifest.clone());
    }
    for entry in current
        .snapshot
        .entries
        .iter()
        .filter(|entry| selected.includes(&entry.path))
    {
        if let Some(TreeEntryTarget::File { file_id }) = &entry.target {
            let manifest = known
                .iter()
                .find(|manifest| manifest.file_id() == file_id)
                .expect("every selected base has a known manifest");
            supplied.insert(
                (manifest.file_id().clone(), manifest.id().clone()),
                manifest.clone(),
            );
        }
    }
    let supplied: Vec<_> = supplied.into_values().collect();
    let bases: Vec<_> = current
        .snapshot
        .entries
        .iter()
        .filter(|entry| selected.includes(&entry.path))
        .filter_map(|entry| match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => {
                Some(supplied.iter().find(|m| m.file_id() == file_id).unwrap())
            }
            _ => None,
        })
        .collect();
    let files: Vec<_> = current
        .snapshot
        .entries
        .iter()
        .filter(|entry| selected.includes(&entry.path))
        .filter_map(|entry| match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some(ImageFile {
                path: &entry.path,
                manifest: supplied.iter().find(|m| m.file_id() == file_id).unwrap(),
                executable: entry.executable,
                expected: Some((file_id.clone(), entry.executable)),
            }),
            _ => None,
        })
        .collect();
    let data = derive_ordinary_data(
        &prefix,
        &current.snapshot,
        &selected,
        &version.selection_digest,
        &bases,
        &files,
        &[],
        destination_identity.clone(),
        PARENT,
        "fs:v1:1111111111111111:2222222222222222",
        LEAF,
        TX,
        &historical,
        &supplied,
        0,
    )
    .unwrap();
    let planned = ProviderTransitionIntentRecord {
        schema_version: "layrs.projection-provider-transition-intent.v1".into(),
        transaction_id: TX.into(),
        role: TransitionRole::Source,
        state: TransitionState::Planned,
        local_generation: version.generation,
        plan: data.plan,
        destination: None,
        reservation: None,
        receipt_id: None,
    };
    assert_eq!(
        ordinary_transition::apply_planned(c, marker, &prefix, &planned, None).unwrap(),
        SourceIntentEffect::Inserted
    );
    let shared = marker.shared.as_ref().unwrap();
    let mut value = serde_json::to_value(marker).unwrap();
    value["identity"] = serde_json::to_value(&destination_identity).unwrap();
    value["checkoutRoot"] = std::path::Path::new(PARENT)
        .join(LEAF)
        .to_str()
        .unwrap()
        .into();
    value["checkoutIdentity"] = "fs:v1:3333333333333333:4444444444444444".into();
    value["storeIdentity"] = "fs:v1:3333333333333333:5555555555555555".into();
    value["metadataIdentity"] = "fs:v1:3333333333333333:6666666666666666".into();
    value["importDigest"] = prefix.digest().unwrap().into();
    value["shared"]["metadataPath"] = shared
        .root
        .join("worktrees")
        .join(
            blake3::hash(destination_identity.worktree_id.as_str().as_bytes())
                .to_hex()
                .to_string(),
        )
        .join("metadata")
        .to_str()
        .unwrap()
        .into();
    value["shared"]["databaseIdentity"] = "1:0:881".into();
    let target = format::parse_marker_bytes(&canonical_json(&value).unwrap()).unwrap();
    let admitted = read_admitted_import(c, marker, None).unwrap();
    let mut copying = ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
    copying.state = TransitionState::Copying;
    copying.destination = Some(TransitionDestinationProof {
        backend: TransitionBackendKind::Ordinary,
        format: target.clone(),
        private_database_identity: "1:0:881".into(),
        format_file_identity: "1:0:882".into(),
        family_marker_digest: admitted.record().binding.family_marker_digest.clone(),
        native_control_identity_hex: None,
        native_root_identity: None,
    });
    assert_eq!(
        ordinary_transition::apply_copying(c, marker, &prefix, &copying, None).unwrap(),
        SourceIntentEffect::Updated
    );
    let (fresh_prefix, fresh_closure) =
        import_validation::plan_admitted_import(c, marker, destination_identity, IMAGE, None)
            .unwrap();
    assert_eq!(fresh_prefix, copying.plan.source_prefix);
    (copying, fresh_closure, target, history)
}

owned!(
    ordinary_destination_copies_restore_history_and_replays_without_changes,
    {
        with_admitted_source_sql(|source, marker, supplied| {
            let (copying, closure, target, history) = source_copying(source, marker, supplied);
            let inputs = OrdinaryCopyInputs::new(source, marker, &copying, &closure);
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            let source_changes = source.total_changes();
            assert_eq!(
                initialize_or_replay_copied(c, &target, &inputs, None)
                    .unwrap()
                    .1,
                CopyEffect::Copied
            );
            let copied: Vec<u8> = c
                .query_row(
                    "SELECT payload FROM projection_restore_history WHERE operation_id=?1",
                    [OPERATION],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(copied, history);
            assert_eq!(
                c.query_row("SELECT count(*) FROM projection_restore_history", [], |r| r
                    .get::<_, u64>(0))
                    .unwrap(),
                1
            );
            assert_eq!(
                c.query_row("SELECT generation FROM projection_head", [], |r| r
                    .get::<_, u64>(0))
                    .unwrap(),
                copying.local_generation
            );
            let changes = c.total_changes();
            assert_eq!(
                initialize_or_replay_copied(c, &target, &inputs, None)
                    .unwrap()
                    .1,
                CopyEffect::AlreadyPresent
            );
            assert_eq!(c.total_changes(), changes);
            assert_eq!(source.total_changes(), source_changes);
            read_copied(c, &target, &inputs, None).unwrap();
            c.execute_batch("ROLLBACK").unwrap();
            assert!(database_io::schema_inventory(c).unwrap().is_empty());
            assert_eq!(
                c.query_row("PRAGMA user_version", [], |r| r.get::<_, u64>(0))
                    .unwrap(),
                0
            );
            // A later readonly opener does not enable FK enforcement. Exact
            // schema, copied rows and pragma_foreign_key_check still validate
            // the committed prefix; the writer keeps its separate FK-ON gate.
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            assert_eq!(
                initialize_or_replay_copied(c, &target, &inputs, None)
                    .unwrap()
                    .1,
                CopyEffect::Copied
            );
            c.execute_batch("COMMIT; PRAGMA foreign_keys=OFF; BEGIN DEFERRED")
                .unwrap();
            assert_eq!(
                c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u64>(0))
                    .unwrap(),
                0
            );
            let committed_changes = c.total_changes();
            read_copied(c, &target, &inputs, None).unwrap();
            assert_eq!(c.total_changes(), committed_changes);
            let error = initialize_or_replay_copied(c, &target, &inputs, None)
                .err()
                .expect("writer must refuse FK-OFF even for an exact Copied retry");
            assert!(error.to_string().contains("FK-enabled"), "{error}");
            assert_eq!(c.total_changes(), committed_changes);
            assert_eq!(source.total_changes(), source_changes);
            read_copied(c, &target, &inputs, None).unwrap();
            c.execute_batch("ROLLBACK; PRAGMA foreign_keys=ON").unwrap();
            bare.finish(Ok(())).unwrap();
        });
    }
);

owned!(
    ordinary_destination_refuses_foreign_pending_restore_tail_head_and_ingress,
    {
        with_admitted_source_sql(|source, marker, supplied| {
            let (copying, closure, target, _) = source_copying(source, marker, supplied);
            let inputs = OrdinaryCopyInputs::new(source, marker, &copying, &closure);
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            initialize_or_replay_copied(c, &target, &inputs, None).unwrap();
            for mutation in [
                "pending",
                "restore-tail",
                "restore-payload",
                "head",
                "prefix",
                "intent",
            ] {
                c.execute_batch("SAVEPOINT substituted_copy").unwrap();
                match mutation {
                    "pending" => {
                        c.execute("INSERT INTO projection_restore_pending VALUES(1,x'00')", [])
                            .unwrap();
                    }
                    "restore-tail" => {
                        c.execute("INSERT INTO projection_restore_history SELECT '9-1',capture_sequence,payload FROM projection_restore_history WHERE operation_id=?1", [OPERATION]).unwrap();
                    }
                    "restore-payload" => {
                        let mut payload: Vec<u8> = c
                            .query_row("SELECT payload FROM projection_restore_history", [], |r| {
                                r.get(0)
                            })
                            .unwrap();
                        payload.push(b' ');
                        c.execute(
                            "UPDATE projection_restore_history SET payload=?1",
                            [payload],
                        )
                        .unwrap();
                    }
                    "head" => {
                        c.execute("UPDATE projection_head SET generation=0", [])
                            .unwrap();
                    }
                    "prefix" => {
                        c.execute("UPDATE projection_imports SET payload=x'00'", [])
                            .unwrap();
                    }
                    "intent" => {
                        c.execute(
                            "UPDATE projection_provider_transitions SET digest=?1",
                            [TOKEN],
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
                let changes = c.total_changes();
                assert!(
                    read_copied(c, &target, &inputs, None).is_err(),
                    "accepted {mutation}"
                );
                assert!(
                    initialize_or_replay_copied(c, &target, &inputs, None).is_err(),
                    "replay accepted {mutation}"
                );
                assert_eq!(c.total_changes(), changes, "refusal wrote for {mutation}");
                c.execute_batch("ROLLBACK TO substituted_copy; RELEASE substituted_copy")
                    .unwrap();
                read_copied(c, &target, &inputs, None).unwrap();
            }
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        });
    }
);

owned!(
    ordinary_destination_refuses_precancel_wrong_source_and_partial_ingress_before_writes,
    {
        with_admitted_source_sql(|source, marker, supplied| {
            let (copying, closure, target, _) = source_copying(source, marker, supplied);
            let inputs = OrdinaryCopyInputs::new(source, marker, &copying, &closure);
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            let cancelled = Cancellation::default();
            cancelled.cancel();
            let changes = c.total_changes();
            assert!(initialize_or_replay_copied(c, &target, &inputs, Some(&cancelled)).is_err());
            assert!(database_io::schema_inventory(c).unwrap().is_empty());
            assert_eq!(c.total_changes(), changes);
            let mut foreign =
                ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
            foreign.transaction_id = "000000000000000000000000000000a3".into();
            foreign.encode().unwrap();
            let wrong = OrdinaryCopyInputs::new(source, marker, &foreign, &closure);
            assert!(initialize_or_replay_copied(c, &target, &wrong, None).is_err());
            assert!(database_io::schema_inventory(c).unwrap().is_empty());
            assert_eq!(c.total_changes(), changes);
            c.execute_batch(
                &[
                    database::SQL,
                    selection::SQL,
                    retention::SQL,
                    schema::SQL,
                    "PRAGMA user_version=8;",
                ]
                .concat(),
            )
            .unwrap();
            c.execute(
                "INSERT INTO projection_meta VALUES(1,?1)",
                [target.bytes().unwrap()],
            )
            .unwrap();
            c.execute(
                "INSERT INTO projection_backend VALUES(1,'ordinary',NULL)",
                [],
            )
            .unwrap();
            c.execute(
                "INSERT INTO projection_imports VALUES(1,?1,?2)",
                params![
                    copying.plan.source_prefix_id,
                    copying.plan.source_prefix.encode().unwrap()
                ],
            )
            .unwrap();
            let changes = c.total_changes();
            assert!(
                initialize_or_replay_copied(c, &target, &inputs, None).is_err(),
                "must not adopt partial EmptyIngress"
            );
            assert_eq!(c.total_changes(), changes);
            assert_eq!(
                c.query_row("SELECT count(*) FROM projection_snapshots", [], |r| r
                    .get::<_, u64>(0))
                    .unwrap(),
                0
            );
            c.execute_batch("ROLLBACK").unwrap();
            assert!(database_io::schema_inventory(c).unwrap().is_empty());
            bare.finish(Ok(())).unwrap();
        });
    }
);
