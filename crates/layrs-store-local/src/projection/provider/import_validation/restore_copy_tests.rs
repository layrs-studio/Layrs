//! Complete schema8 SQL DATA copy regression. Synthetic FORMAT/native tokens
//! and the supplied image digest grant no native or content-byte authority.
use super::*;
use crate::projection::{
    capture::{CapturedFile, CapturedPatch},
    provider::{
        database_io::BareConnection,
        process_profile::tests::child,
        records::{
            import::{ImportVersion, ProviderImportKind},
            Record,
        },
        validation::tests::fixture::Fixture,
    },
    restore::{RestoreDestination, RestorePreview, RestoreSource},
};
use layrs_core::{
    projection::EntryState,
    scm::{Chunk, FileManifest, WorktreeId},
};
use rusqlite::params;
use std::collections::BTreeMap;

const IMAGE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ZERO: &str = "blake3:0000000000000000000000000000000000000000000000000000000000000000";

fn rowset(count: u64) -> ImportRowset {
    ImportRowset {
        count,
        digest: ZERO.into(),
    }
}

fn schema8(c: &Connection, marker: &format::Marker) {
    c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    c.execute_batch(
        &[
            database::SQL,
            selection::SQL,
            retention::SQL,
            super::super::schema::SQL,
            "PRAGMA user_version=8;",
        ]
        .concat(),
    )
    .unwrap();
    c.execute(
        "INSERT INTO projection_meta VALUES(1,?1)",
        [marker.bytes().unwrap()],
    )
    .unwrap();
    c.execute(
        "INSERT INTO projection_backend VALUES(1,'ordinary',NULL)",
        [],
    )
    .unwrap();
}

fn journal(preview: &RestorePreview) -> Vec<u8> {
    let mut preview = preview.clone();
    preview.digest.clear();
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-projection-restore-preview-v1\0");
    hash.update(&canonical_json(&preview).unwrap());
    preview.digest = format!("blake3:{}", hash.finalize());
    let old: BTreeMap<_, _> = preview
        .before
        .iter()
        .filter(|(_, file)| file.is_some())
        .map(|(path, _)| (path.clone(), "1:0:21"))
        .collect();
    let new: BTreeMap<_, _> = preview
        .after
        .iter()
        .filter(|(_, file)| file.is_some())
        .map(|(path, _)| (path.clone(), "1:0:22"))
        .collect();
    canonical_json(&serde_json::json!({
        "schemaVersion": "layrs.projection-restore.v1", "preview": preview,
        "directory": "restore-1-1", "directoryIdentity": "fs:v1:0000000000000001:0000000000000002",
        "oldIdentity": old, "newIdentity": new,
    }))
    .unwrap()
}

fn source_fixture(c: &Connection) -> (format::Marker, ProviderImportPrefixRecord, Vec<u8>) {
    let data = Fixture::new("G1", false);
    let mut snapshot = data.source.initial().clone();
    snapshot.entries[0].state = EntryState::Materialized;
    snapshot.entries[0].writable = true;
    snapshot.validate().unwrap();
    let payload = snapshot.canonical_bytes().unwrap();
    let snapshot_digest = database::digest(0, None, &payload);
    let mut marker = data.allocation.format.clone();
    marker.initial_snapshot_digest = snapshot_digest.clone();
    schema8(c, &marker);
    c.execute(
        "INSERT INTO projection_snapshots VALUES(0,?1,NULL,?2)",
        params![snapshot_digest, payload],
    )
    .unwrap();
    c.execute("INSERT INTO projection_head VALUES(1,0)", [])
        .unwrap();
    selection::insert(c, 0, data.source.selection()).unwrap();
    let mut patch = CapturedPatch {
        schema_version: "layrs.projection-capture.v1".into(),
        sequence: 1,
        previous_patch_digest: None,
        projection_generation: 0,
        projection_digest: snapshot_digest.clone(),
        identity: marker.identity.clone(),
        changes: Vec::new(),
        digest: String::new(),
    };
    patch.digest = patch.computed_digest().unwrap();
    c.execute(
        "INSERT INTO projection_captures VALUES(1,?1,0,?2)",
        params![patch.digest, canonical_json(&patch).unwrap()],
    )
    .unwrap();
    let after_manifest = &data.source.descriptors()[0].manifest;
    let before_manifest = FileManifest::from_chunks(
        after_manifest.chunking().clone(),
        &[Chunk::new(b"restore-only preimage".to_vec()).unwrap()],
    )
    .unwrap();
    let file = |manifest: &FileManifest| CapturedFile {
        file_id: manifest.file_id().clone(),
        bytes: manifest.byte_length(),
        executable: false,
    };
    for manifest in [&before_manifest, after_manifest] {
        c.execute(
            "INSERT INTO projection_content VALUES(?1,?2,?3)",
            params![
                manifest.file_id().as_str(),
                manifest.id().as_str(),
                manifest.byte_length()
            ],
        )
        .unwrap();
    }
    let path = snapshot.entries[0].path.clone();
    let preview = RestorePreview {
        operation_id: "1-1".into(),
        destination: Some(RestoreDestination {
            identity: marker.identity.clone(),
            checkout_identity: marker.checkout_identity().into(),
        }),
        source: RestoreSource::Capture {
            sequence: 1,
            paths: vec![path.clone()],
        },
        projection_generation: 0,
        projection_digest: snapshot_digest.clone(),
        checkout_digest: ZERO.into(),
        after_checkout_digest: ZERO.into(),
        before: BTreeMap::from([(path.clone(), Some(file(&before_manifest)))]),
        after: BTreeMap::from([(path, Some(file(after_manifest)))]),
        digest: String::new(),
    };
    let history = journal(&preview);
    c.execute(
        "INSERT INTO projection_restore_history VALUES('1-1',1,?1)",
        [&history],
    )
    .unwrap();
    let mut destination = marker.identity.clone();
    destination.worktree_id = WorktreeId::parse("worktree:restore-copy").unwrap();
    let selection_digest: String = c
        .query_row(
            "SELECT digest FROM projection_selections WHERE generation=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut segments = blake3::Hasher::new();
    segments.update(b"layrs-projection-provider-import-segments-v1\0");
    segments.update(&0u64.to_be_bytes());
    let prefix = ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v2".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Ordinary {
            identity: marker.identity.clone(),
            format_digest: super::super::records::codec::hash(
                "layrs-projection-provider-import-format-v1",
                &marker.bytes().unwrap(),
            ),
            private_schema: 8,
            checkout_identity: Some(marker.checkout_identity().into()),
            admission: None,
            version: ImportVersion {
                generation: 0,
                snapshot_digest,
                selection_digest,
                capture_count: 1,
                capture_head: Some(patch.digest),
                effective_image_digest: IMAGE.into(),
            },
        },
        destination_identity: destination,
        inherited_segments: ImportRowset {
            count: 0,
            digest: format!("blake3:{}", segments.finalize()),
        },
        ancestry_root_receipt: None,
        closure: ImportClosure {
            denial_observations: None,
            snapshots: rowset(1),
            selections: rowset(1),
            captures: rowset(1),
            publications: rowset(0),
            catalogs: rowset(0),
            descriptors: rowset(0),
            activation_maps: rowset(0),
            transition_receipts: rowset(0),
            companions: rowset(0),
            companion_files: rowset(0),
            companion_deletions: rowset(0),
            content_mappings: rowset(2),
            restore_history: Some(rowset(1)),
        },
    };
    (marker, prefix, history)
}

fn verify(
    c: &Connection,
    marker: &format::Marker,
    prefix: &mut ProviderImportPrefixRecord,
) -> VerifiedImportClosure {
    let metadata = read_source_metadata(
        c,
        marker,
        ImportDatabaseKind::Provider,
        prefix,
        ImportSourceReferences::Ordinary(marker),
        IMAGE,
        None,
    )
    .unwrap();
    prefix.closure =
        scoped::compute_scoped_closure(c, ImportDatabaseKind::Provider, prefix, &metadata, None)
            .unwrap();
    verify_source(
        c,
        marker,
        ImportDatabaseKind::Provider,
        prefix,
        ImportSourceReferences::Ordinary(marker),
        None,
        IMAGE,
        None,
    )
    .unwrap()
}

fn count(c: &Connection, table: &str) -> u64 {
    // All callers supply static test-owned table names.
    c.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
#[ignore = "owned audited Windows SQLite process; complete schema8 restore-prefix copy"]
fn verified_restore_prefix_copy_is_exact_idempotent_and_refuses_collision_before_inserts() {
    child::run(concat!(module_path!(), "::verified_restore_prefix_copy_is_exact_idempotent_and_refuses_collision_before_inserts"), 120, || {
        child::declare();
        let source_handle = BareConnection::reference().unwrap();
        let source = source_handle.connection();
        let (marker, mut prefix, history) = source_fixture(source);
        let mut destination_marker = marker.clone();
        destination_marker.identity = prefix.destination_identity.clone();
        let destination_handle = BareConnection::reference().unwrap();
        let destination = destination_handle.connection();
        schema8(destination, &destination_marker);
        for connection in [source, destination] {
            assert_eq!(connection.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, u8>(0)).unwrap(), 1);
        }
        source.execute_batch("BEGIN DEFERRED").unwrap();
        destination.execute_batch("BEGIN IMMEDIATE").unwrap();
        let proof = verify(source, &marker, &mut prefix);
        assert_eq!(proof.closure().restore_history.as_ref().unwrap().count, 1);
        copy_verified_prefix(source, destination, &destination_marker, &prefix, &proof, None).unwrap();
        let copied: (u64, Vec<u8>) = destination.query_row(
            "SELECT capture_sequence,payload FROM projection_restore_history WHERE operation_id='1-1'", [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(copied, (1, history.clone()));
        require_exact_copied_rows(destination, &destination_marker, &prefix, &proof, None).unwrap();
        let changes = destination.total_changes();
        copy_verified_prefix(source, destination, &destination_marker, &prefix, &proof, None).unwrap();
        assert_eq!(destination.total_changes(), changes);
        assert_eq!(count(destination, "projection_restore_history"), 1);
        assert_eq!(count(destination, "projection_content"), 2);

        destination.execute_batch("SAVEPOINT operation_collision").unwrap();
        let mut collision: serde_json::Value = serde_json::from_slice(&history).unwrap();
        collision["directory"] = "restore-2-1".into();
        let collision = canonical_json(&collision).unwrap();
        destination.execute("UPDATE projection_restore_history SET payload=?1 WHERE operation_id='1-1'", [&collision]).unwrap();
        // These earlier rowsets now need inserts. The later operation collision
        // must still be rejected before even the first such insertion.
        destination.execute("DELETE FROM projection_selections", []).unwrap();
        destination.execute("DELETE FROM projection_content", []).unwrap();
        let changes = destination.total_changes();
        let refusal = copy_verified_prefix(source, destination, &destination_marker, &prefix, &proof, None).unwrap_err();
        assert!(refusal.to_string().contains("collision"), "{refusal}");
        assert_eq!(destination.total_changes(), changes);
        assert_eq!(count(destination, "projection_selections"), 0);
        assert_eq!(count(destination, "projection_content"), 0);
        let unchanged: Vec<u8> = destination.query_row("SELECT payload FROM projection_restore_history WHERE operation_id='1-1'", [], |row| row.get(0)).unwrap();
        assert_eq!(unchanged, collision);
        destination.execute_batch("ROLLBACK TO operation_collision; RELEASE operation_collision").unwrap();
        require_exact_copied_rows(destination, &destination_marker, &prefix, &proof, None).unwrap();
        destination.execute_batch("ROLLBACK").unwrap();
        for table in ["projection_snapshots", "projection_selections", "projection_captures", "projection_restore_history", "projection_content"] {
            assert_eq!(count(destination, table), 0, "{table}");
        }
        assert_eq!(count(source, "projection_restore_history"), 1);
        assert_eq!(destination.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0)).unwrap(), 8);

        // The legacy twelve-rowset path remains valid without an optional
        // restore spec. This is a separate valid source proof, not a relabelled9.
        source.execute_batch("SAVEPOINT legacy_source").unwrap();
        source.execute("DELETE FROM projection_restore_history", []).unwrap();
        let mut legacy = ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap();
        legacy.schema_version = "layrs.projection-provider-import-prefix.v1".into();
        legacy.closure.restore_history = None;
        if let ImportSource::Ordinary { checkout_identity, .. } = &mut legacy.source { *checkout_identity = None; }
        let legacy_proof = verify(source, &marker, &mut legacy);
        destination.execute_batch("BEGIN IMMEDIATE").unwrap();
        copy_verified_prefix(source, destination, &destination_marker, &legacy, &legacy_proof, None).unwrap();
        require_exact_copied_rows(destination, &destination_marker, &legacy, &legacy_proof, None).unwrap();
        assert_eq!(count(destination, "projection_restore_history"), 0);
        destination.execute_batch("ROLLBACK").unwrap();
        source.execute_batch("ROLLBACK TO legacy_source; RELEASE legacy_source; COMMIT").unwrap();
        source_handle.finish(Ok(())).unwrap();
        destination_handle.finish(Ok(())).unwrap();
    });
}
