use super::*;
use crate::projection::provider::records::Receipt;
use crate::projection::provider::validation::tests::fixture::Fixture;
use layrs_core::scm::{Chunk, ChunkingDescriptor, WorktreeId};

fn path(value: &str) -> RepoPath {
    RepoPath::parse(value).unwrap()
}

#[test]
fn checkpoint_owner_metadata_refuses_foreign_root_control_format_and_ordinary() {
    let f = Fixture::new("G1", false);
    let bytes = f.allocation.format.bytes().unwrap();
    let token = format!("blake3:{}", "1".repeat(64));
    let version = serde_json::json!({"generation":1,"snapshotDigest":"a".repeat(64),
        "selectionDigest":"b".repeat(64),"captureCount":0,"captureHead":null,
        "effectiveImageDigest":"c".repeat(64)});
    let mut source: ImportSource = serde_json::from_value(serde_json::json!({
        "kind":"provider", "identity":f.allocation.format.identity,
        "format_digest":codec::hash("layrs-projection-provider-import-format-v1", &bytes),
        "private_schema":8, "allocation":f.allocation.data_reference().unwrap(),
        "prepared":f.prepared.data_reference().unwrap(), "activation_map_id":token,
        "version":version,
        "binding":{
            "schemaVersion":"layrs.projection-provider-bootstrap-binding.v1",
            "recovery":{"identity":f.allocation.format.identity,
                "checkoutIdentity":f.allocation.format.checkout_identity(),
                "markerDigest":codec::hash("layrs-projection-recovery-marker-v1", &bytes),
                "databaseIdentity":f.allocation.private.database_file_identity,
                "familyMarkerDigest":f.allocation.family.marker_digest},
            "transactionId":f.allocation.transaction_id,
            "allocation":f.allocation.data_reference().unwrap(),
            "prepared":{"receipt":f.prepared.data_reference().unwrap(),
                "controlIdentityHex":f.prepared.control_identity_hex},
            "abandoned":null
        }
    }))
    .unwrap();
    let control = &f.prepared.control_identity_hex;
    let fs = f.allocation.format.checkout_identity();
    require_owner(&source, control, fs, &bytes).unwrap();
    assert!(require_owner(&source, "foreign", fs, &bytes).is_err());
    assert!(require_owner(
        &source,
        control,
        "fs:v1:0000000000000001:000000000000ffff",
        &bytes
    )
    .is_err());
    let mut changed_bytes = bytes.clone();
    changed_bytes.push(b' ');
    assert!(require_owner(&source, control, fs, &changed_bytes).is_err());
    if let ImportSource::Provider { identity, .. } = &mut source {
        identity.worktree_id = WorktreeId::parse("worktree:foreign-checkpoint").unwrap();
    }
    assert!(require_owner(&source, control, fs, &bytes).is_err());
    let ordinary: ImportSource = serde_json::from_value(serde_json::json!({
        "kind":"ordinary","identity":f.allocation.format.identity,
        "format_digest":token,"private_schema":6,"version":version
    }))
    .unwrap();
    assert!(require_owner(&ordinary, control, fs, &bytes).is_err());
}

#[test]
fn checkpoint_image_commitment_refuses_later_bytes_modes_and_synthetic_absences() {
    let a = FileManifest::from_chunks(
        ChunkingDescriptor::single(),
        &[Chunk::new(b"image at import".to_vec()).unwrap()],
    )
    .unwrap();
    let b = FileManifest::from_chunks(
        ChunkingDescriptor::single(),
        &[Chunk::new(b"a later version".to_vec()).unwrap()],
    )
    .unwrap();
    let file_path = path("kept.txt");
    let deleted_path = path("removed.txt");
    let reports = [ProviderRecoveryExportFile {
        path: file_path.clone(),
        file_id: a.file_id().clone(),
        length: a.byte_length(),
        executable: Some(false),
        class: ProviderRecoveryExportClass::ManagedCandidate,
    }];
    let deleted = [ProviderRecoveryExportDeletion {
        path: deleted_path.clone(),
        base: b.file_id().clone(),
        expected_executable: Some(false),
        class: ProviderRecoveryExportClass::ManagedCandidate,
    }];
    let token = recovery_image_digest(&reports, &deleted, &[]).unwrap();
    let expected = token.strip_prefix("blake3:").unwrap();
    let mut entries = BTreeMap::from([
        (
            file_path.clone(),
            ImageEntryData {
                file: Some(PresentFileData {
                    manifest: &a,
                    executable: false,
                }),
                relocated: false,
            },
        ),
        (
            deleted_path.clone(),
            ImageEntryData {
                file: None,
                relocated: false,
            },
        ),
    ]);
    let mut bases = BTreeMap::from([(deleted_path.clone(), b.file_id())]);
    require_image(expected, &entries, &bases, None).unwrap();
    entries
        .get_mut(&file_path)
        .unwrap()
        .file
        .as_mut()
        .unwrap()
        .manifest = &b;
    assert!(require_image(expected, &entries, &bases, None).is_err());
    entries
        .get_mut(&file_path)
        .unwrap()
        .file
        .as_mut()
        .unwrap()
        .manifest = &a;
    entries
        .get_mut(&file_path)
        .unwrap()
        .file
        .as_mut()
        .unwrap()
        .executable = true;
    assert!(require_image(expected, &entries, &bases, None).is_err());
    entries
        .get_mut(&file_path)
        .unwrap()
        .file
        .as_mut()
        .unwrap()
        .executable = false;
    entries.insert(
        path("captured-then-missing.txt"),
        ImageEntryData {
            file: None,
            relocated: false,
        },
    );
    assert!(require_image(expected, &entries, &bases, None).is_err());
    entries.remove(&path("captured-then-missing.txt"));
    bases.insert(path("foreign-deletion.txt"), a.file_id());
    assert!(require_image(expected, &entries, &bases, None).is_err());
    bases.remove(&path("foreign-deletion.txt"));
    bases.insert(deleted_path, a.file_id());
    assert!(require_image(expected, &entries, &bases, None).is_err());
}
