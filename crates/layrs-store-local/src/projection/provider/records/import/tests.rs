use super::*;
use crate::projection::{provider::validation::tests::fixture::Fixture as Pure, RecoveryBinding};
use layrs_core::scm::{SpaceId, StoreId, WorktreeId};

const ID0: &str = "blake3:0000000000000000000000000000000000000000000000000000000000000000";
const ID1: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
const ID2: &str = "blake3:2222222222222222222222222222222222222222222222222222222222222222";
const RAW: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn identity(name: &str) -> ProjectionIdentity {
    ProjectionIdentity {
        store_id: StoreId::parse("store:source").unwrap(),
        worktree_id: WorktreeId::parse(format!("worktree:{name}")).unwrap(),
        space_id: SpaceId::parse("space:source").unwrap(),
        origin: "https://example.invalid".into(),
        principal_id: "principal:test".into(),
    }
}
fn rowset(count: u64) -> ImportRowset {
    ImportRowset {
        count,
        digest: ID0.into(),
    }
}
fn closure() -> ImportClosure {
    ImportClosure {
        snapshots: rowset(3),
        selections: rowset(3),
        captures: rowset(0),
        publications: rowset(0),
        catalogs: rowset(0),
        descriptors: rowset(0),
        activation_maps: rowset(0),
        transition_receipts: rowset(0),
        companions: rowset(0),
        companion_files: rowset(0),
        companion_deletions: rowset(0),
        content_mappings: rowset(0),
    }
}
fn prefix() -> ProviderImportPrefixRecord {
    ProviderImportPrefixRecord {
        schema_version: PREFIX_SCHEMA.into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Ordinary {
            identity: identity("source"),
            format_digest: ID0.into(),
            private_schema: 8,
            version: ImportVersion {
                generation: 2,
                snapshot_digest: RAW.into(),
                selection_digest: RAW.into(),
                capture_count: 0,
                capture_head: None,
                effective_image_digest: RAW.into(),
            },
        },
        destination_identity: identity("destination"),
        inherited_segments: rowset(0),
        ancestry_root_receipt: None,
        closure: closure(),
    }
}

fn provider_prefix() -> (ProviderImportPrefixRecord, Pure, ProviderActivationRecord) {
    let f = Pure::new("G1", false);
    let a = &f.allocation;
    let p = &f.prepared;
    let binding = ProviderBootstrapBinding {
        schema_version: "layrs.projection-provider-bootstrap-binding.v1".into(),
        recovery: RecoveryBinding {
            identity: a.format.identity.clone(),
            checkout_identity: a.format.checkout_identity().into(),
            marker_digest: codec::hash(
                "layrs-projection-recovery-marker-v1",
                &a.format.bytes().unwrap(),
            ),
            database_identity: a.private.database_file_identity.clone(),
            family_marker_digest: Some(a.family.marker_digest.clone()),
        },
        transaction_id: a.transaction_id.clone(),
        allocation: a.data_reference().unwrap(),
        prepared: Some(super::super::PreparedBinding {
            receipt: p.data_reference().unwrap(),
            control_identity_hex: p.control_identity_hex.clone(),
        }),
        abandoned: None,
    };
    let activation = ProviderActivationRecord {
        schema_version: "layrs.projection-provider-activation.v1".into(),
        owner_identity_digest: owner_identity_digest(a).unwrap(),
        mount_id: a.transaction_id.clone(),
        catalog_id: a.source.catalog_id.clone(),
        binding: ProviderBootstrapBinding::decode(&binding.encode().unwrap()).unwrap(),
        source: serde_json::from_slice(&serde_json::to_vec(&a.source).unwrap()).unwrap(),
        prepared_digest: ID0.into(),
        source_generation: 0,
        source_digest: a.source.snapshot_digest.clone(),
        selection_digest: a.source.selection_digest.clone(),
        activated_generation: 1,
        activated_digest: RAW.into(),
    };
    activation.validate_fields().unwrap();
    let mut prefix = prefix();
    prefix.kind = ProviderImportKind::ExpandedProvider;
    prefix.source = ImportSource::Provider {
        identity: a.format.identity.clone(),
        format_digest: codec::hash(
            "layrs-projection-provider-import-format-v1",
            &a.format.bytes().unwrap(),
        ),
        private_schema: 8,
        allocation: a.data_reference().unwrap(),
        prepared: p.data_reference().unwrap(),
        binding,
        activation_map_id: activation.digest().unwrap(),
        version: ImportVersion {
            generation: 1,
            snapshot_digest: RAW.into(),
            selection_digest: a.source.selection_digest.clone(),
            capture_count: 0,
            capture_head: None,
            effective_image_digest: RAW.into(),
        },
    };
    prefix.closure.snapshots.count = 2;
    prefix.closure.selections.count = 2;
    prefix.closure.catalogs.count = 1;
    prefix.closure.descriptors.count = 1;
    prefix.closure.activation_maps.count = 1;
    prefix.validate_fields().unwrap();
    (prefix, f, activation)
}

#[test]
fn provider_import_rejects_rehashed_family_and_control_forgery() {
    let (prefix, f, activation) = provider_prefix();
    prefix
        .require_references(ImportSourceReferences::Provider {
            allocation: &f.allocation,
            prepared: &f.prepared,
            activation: &activation,
        })
        .unwrap();

    let mut forged = ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap();
    let mut forged_map = ProviderActivationRecord::decode(&activation.encode().unwrap()).unwrap();
    let ImportSource::Provider {
        binding,
        activation_map_id,
        ..
    } = &mut forged.source
    else {
        panic!("provider source expected");
    };
    binding.recovery.family_marker_digest = Some(ID1.into());
    forged_map.binding = ProviderBootstrapBinding::decode(&binding.encode().unwrap()).unwrap();
    *activation_map_id = forged_map.digest().unwrap();
    forged.validate_fields().unwrap();
    assert!(forged
        .require_references(ImportSourceReferences::Provider {
            allocation: &f.allocation,
            prepared: &f.prepared,
            activation: &forged_map,
        })
        .is_err());

    let alternate = Pure::new("G0", false);
    assert_ne!(
        alternate.prepared.control_identity_hex,
        f.prepared.control_identity_hex
    );
    let mut forged = ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap();
    let mut forged_map = ProviderActivationRecord::decode(&activation.encode().unwrap()).unwrap();
    let ImportSource::Provider {
        binding,
        activation_map_id,
        ..
    } = &mut forged.source
    else {
        panic!("provider source expected");
    };
    binding.prepared.as_mut().unwrap().control_identity_hex =
        alternate.prepared.control_identity_hex;
    forged_map.binding = ProviderBootstrapBinding::decode(&binding.encode().unwrap()).unwrap();
    *activation_map_id = forged_map.digest().unwrap();
    forged.validate_fields().unwrap();
    assert!(forged
        .require_references(ImportSourceReferences::Provider {
            allocation: &f.allocation,
            prepared: &f.prepared,
            activation: &forged_map,
        })
        .is_err());
}
fn segments_digest(segments: &[ImportedSegment]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"layrs-projection-provider-import-segments-v1\0");
    h.update(&(segments.len() as u64).to_be_bytes());
    for segment in segments {
        let bytes = canonical_json(segment).unwrap();
        h.update(&(bytes.len() as u64).to_be_bytes());
        h.update(&bytes);
    }
    format!("blake3:{}", h.finalize())
}

fn receipt_from(prefix: &ProviderImportPrefixRecord, tx: &str) -> ProviderTransitionReceiptRecord {
    let version = prefix.source.version();
    ProviderTransitionReceiptRecord {
        schema_version: RECEIPT_SCHEMA.into(),
        owner_identity_digest: ID0.into(),
        transaction_id: tx.into(),
        source_prefix_id: prefix.digest().unwrap(),
        source_prefix: ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap(),
        parent_receipt_id: prefix.ancestry_root_receipt.clone(),
        source_identity: prefix.source.identity().clone(),
        destination_identity: prefix.destination_identity.clone(),
        imported_max_generation: version.generation,
        destination_observation_generation: version.generation + 1,
        destination_observation_digest: RAW.into(),
        destination_placement_generation: version.generation + 2,
        destination_placement_digest: RAW.into(),
        imported_capture_count: version.capture_count,
        destination_capture_sequence: version.capture_count + 1,
        destination_capture_digest: ID1.into(),
        protected_manifests: rowset(0),
        content_mappings: prefix.closure.content_mappings.clone(),
        companion_id: None,
        companion_files: rowset(0),
        companion_deletions: rowset(0),
    }
}

#[test]
fn two_imports_reconstruct_parent_and_preserve_source_identity() {
    let mut first_prefix = prefix();
    if let ImportSource::Ordinary {
        identity: source_identity,
        ..
    } = &mut first_prefix.source
    {
        *source_identity = identity("original");
    }
    first_prefix.destination_identity = identity("source");
    let first_receipt = receipt_from(&first_prefix, "first");
    let first = ImportedSegment::from_receipt(&first_receipt, None).unwrap();
    let mut second_prefix = prefix();
    if let ImportSource::Ordinary { version, .. } = &mut second_prefix.source {
        version.generation = 7;
        version.capture_count = 4;
        version.capture_head = Some(ID2.into());
    }
    second_prefix.closure.captures.count = 4;
    second_prefix.closure.snapshots.count = 8;
    second_prefix.closure.transition_receipts.count = 1;
    second_prefix.inherited_segments = ImportRowset {
        count: 1,
        digest: segments_digest(std::slice::from_ref(&first)),
    };
    second_prefix.ancestry_root_receipt = Some(first_receipt.digest().unwrap());
    second_prefix
        .validate_segments(std::slice::from_ref(&first))
        .unwrap();
    let second_receipt = receipt_from(&second_prefix, "second");
    let second = ImportedSegment::from_receipt(&second_receipt, Some(&first_receipt)).unwrap();
    assert_ne!(first.source_identity, second.source_identity);
    assert_eq!(
        second.parent_receipt_id.as_deref(),
        Some(first.receipt_id.as_str())
    );
    assert_eq!(second.first_generation, first.last_generation + 1);
    assert_eq!(second.capture_count_before, first.capture_count_through);
    assert!(ImportedSegment::from_receipt(&second_receipt, None).is_err());
}

#[test]
fn two_import_segments_preserve_identity_and_reject_cycle_overlap() {
    let first = ImportedSegment {
        receipt_id: ID1.into(),
        parent_receipt_id: None,
        source_identity: identity("original"),
        first_generation: 0,
        last_generation: 2,
        capture_count_before: 0,
        capture_count_through: 0,
    };
    let second = ImportedSegment {
        receipt_id: ID2.into(),
        parent_receipt_id: Some(ID1.into()),
        source_identity: identity("source"),
        first_generation: 3,
        last_generation: 7,
        capture_count_before: 0,
        capture_count_through: 4,
    };
    let mut prefix = prefix();
    if let ImportSource::Ordinary { version, .. } = &mut prefix.source {
        version.generation = 7;
        version.capture_count = 4;
        version.capture_head = Some(ID2.into());
    }
    prefix.closure.captures.count = 4;
    prefix.closure.snapshots.count = 8;
    let segments = vec![first.clone(), second.clone()];
    prefix.inherited_segments = ImportRowset {
        count: 2,
        digest: segments_digest(&segments),
    };
    prefix.ancestry_root_receipt = Some(ID2.into());
    prefix.closure.transition_receipts.count = 2;
    prefix.validate_segments(&segments).unwrap();
    assert_eq!(
        ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap(),
        prefix
    );
    assert_ne!(segments[0].source_identity, segments[1].source_identity);

    let mut cycle = segments.clone();
    cycle[0].parent_receipt_id = Some(ID2.into());
    assert!(prefix.validate_segments(&cycle).is_err());
    let mut overlap = segments;
    overlap[1].first_generation = 2;
    assert!(prefix.validate_segments(&overlap).is_err());
    let mut capture_overlap = vec![first, second];
    capture_overlap[1].capture_count_before = 1;
    assert!(prefix.validate_segments(&capture_overlap).is_err());
}

#[test]
fn prefix_cumulative_rows_and_compactness_are_fenced() {
    let mut prefix = prefix();
    let bytes = prefix.encode().unwrap();
    assert!(bytes.len() < 64 * 1024);
    prefix
        .require_row(&prefix.digest().unwrap(), &bytes)
        .unwrap();
    prefix.closure.descriptors.count = MAX_ROWS;
    assert!(prefix.encode().is_err());
    prefix.closure.descriptors.count = 0;
    prefix.source = ImportSource::Ordinary {
        identity: identity("destination"),
        format_digest: ID0.into(),
        private_schema: 8,
        version: ImportVersion {
            generation: 2,
            snapshot_digest: RAW.into(),
            selection_digest: RAW.into(),
            capture_count: 0,
            capture_head: None,
            effective_image_digest: RAW.into(),
        },
    };
    assert!(prefix.encode().is_err());
}

#[test]
fn companion_rows_project_exact_ids_modes_reason_and_order() {
    let file = ProviderCompanionFileRecord {
        schema_version: FILE_SCHEMA.into(),
        path: RepoPath::parse("a.txt").unwrap(),
        file_id: FileId::parse(ID1).unwrap(),
        bytes: 7,
        executable: false,
        manifest_id: FileManifestId::parse(ID2).unwrap(),
        base_file_id: Some(FileId::parse(ID0).unwrap()),
        base_executable: Some(true),
        reason: CompanionReason::RevokedRights,
    };
    let mut deletion = ProviderCompanionDeletionRecord {
        schema_version: DELETION_SCHEMA.into(),
        path: RepoPath::parse("z.txt").unwrap(),
        base_file_id: FileId::parse(ID0).unwrap(),
        base_executable: true,
        base_manifest_id: Some(FileManifestId::parse(ID1).unwrap()),
        reason: CompanionReason::ExcludedPath,
    };
    let file_digest = file.digest().unwrap();
    let projection = CompanionFileRowData {
        companion_id: ID0,
        path: "a.txt",
        file_id: ID1,
        bytes: 7,
        executable: false,
        manifest_id: ID2,
        base_file_id: Some(ID0),
        base_executable: Some(true),
        reason: "revokedRights",
        digest: &file_digest,
    };
    file.require_row(ID0, projection).unwrap();
    assert!(file
        .require_row(
            ID0,
            CompanionFileRowData {
                companion_id: ID0,
                path: "a.txt",
                file_id: RAW,
                bytes: 7,
                executable: false,
                manifest_id: ID2,
                base_file_id: Some(ID0),
                base_executable: Some(true),
                reason: "revokedRights",
                digest: &file_digest,
            }
        )
        .is_err());
    deletion
        .require_row(
            ID0,
            CompanionDeletionRowData {
                companion_id: ID0,
                path: "z.txt",
                base_file_id: ID0,
                base_executable: true,
                base_manifest_id: Some(ID1),
                reason: "excludedPath",
                digest: &deletion.digest().unwrap(),
            },
        )
        .unwrap();
    let mut header = ProviderCompanionHeaderRecord {
        schema_version: COMPANION_SCHEMA.into(),
        source_identity: identity("source"),
        source_private_schema: 8,
        source_generation: 3,
        source_capture_count: 0,
        source_capture_head: None,
        import_prefix_id: ID0.into(),
        catalog_id: Some(ID1.into()),
        content_mappings: rowset(2),
        file_count: 1,
        deletion_count: 1,
        files_digest: ordered_companion_rowset(
            FILES_DOMAIN,
            &[(file.path.as_str(), file.digest().unwrap())],
        )
        .unwrap(),
        deletions_digest: ordered_companion_rowset(
            DELETIONS_DOMAIN,
            &[(deletion.path.as_str(), deletion.digest().unwrap())],
        )
        .unwrap(),
    };
    header
        .validate_rows(std::slice::from_ref(&file), std::slice::from_ref(&deletion))
        .unwrap();
    assert!(header
        .require_row(CompanionHeaderRowData {
            id: &header.digest().unwrap(),
            file_count: 1,
            deletion_count: 1,
            files_digest: &header.files_digest,
            deletions_digest: &header.deletions_digest,
            payload: &header.encode().unwrap(),
        })
        .is_ok());
    deletion.path = RepoPath::parse("a.txt").unwrap();
    assert!(header
        .validate_rows(std::slice::from_ref(&file), std::slice::from_ref(&deletion))
        .is_err());
    header.files_digest = RAW.into(); // raw64 is never a schema8 rowset digest
    assert!(header.validate_fields().is_err());
    assert!(
        ordered_companion_rowset(FILES_DOMAIN, &[("z", ID0.into()), ("a", ID1.into())]).is_err()
    );
    let unknown = br#"{"schemaVersion":"layrs.projection-provider-companion-deletion.v1","path":"x","baseFileId":"blake3:0000000000000000000000000000000000000000000000000000000000000000","baseExecutable":false,"baseManifestId":null,"reason":"unknownFuture"}"#;
    assert!(ProviderCompanionDeletionRecord::decode(unknown).is_err());
}

#[test]
fn receipt_binds_prefix_and_new_destination_generation() {
    let prefix = prefix();
    let mut receipt = ProviderTransitionReceiptRecord {
        schema_version: RECEIPT_SCHEMA.into(),
        owner_identity_digest: ID0.into(),
        transaction_id: "transition-one".into(),
        source_prefix_id: prefix.digest().unwrap(),
        source_prefix: ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap(),
        parent_receipt_id: None,
        source_identity: identity("source"),
        destination_identity: identity("destination"),
        imported_max_generation: 2,
        destination_observation_generation: 3,
        destination_observation_digest: RAW.into(),
        destination_placement_generation: 4,
        destination_placement_digest: RAW.into(),
        imported_capture_count: 0,
        destination_capture_sequence: 1,
        destination_capture_digest: ID1.into(),
        protected_manifests: rowset(0),
        content_mappings: rowset(0),
        companion_id: None,
        companion_files: rowset(0),
        companion_deletions: rowset(0),
    };
    receipt.require_prefix(&prefix).unwrap();
    receipt
        .require_row(TransitionReceiptRowData {
            id: &receipt.digest().unwrap(),
            owner_identity_digest: ID0,
            transaction_id: "transition-one",
            companion_id: None,
            payload: &receipt.encode().unwrap(),
        })
        .unwrap();
    receipt.destination_observation_generation = 2;
    assert!(receipt.encode().is_err());
}
