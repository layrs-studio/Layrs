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
        restore_history: None,
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
            checkout_identity: None,
            admission: None,
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

fn prefix_v2() -> ProviderImportPrefixRecord {
    let mut prefix = prefix();
    prefix.schema_version = PREFIX_SCHEMA_V2.into();
    prefix.closure.restore_history = Some(rowset(0));
    let ImportSource::Ordinary {
        checkout_identity, ..
    } = &mut prefix.source
    else {
        unreachable!();
    };
    *checkout_identity = Some("fs:v1:0000000000000001:0000000000000002".into());
    prefix
}

#[test]
fn prefix_v1_preserves_legacy_bytes_and_omits_restore_extensions() {
    let prefix = prefix();
    let empty = serde_json::json!({ "count": 0, "digest": ID0 });
    let legacy = serde_json::json!({
        "schemaVersion": "layrs.projection-provider-import-prefix.v1",
        "kind": "managedOrdinary",
        "source": {
            "kind": "ordinary",
            "identity": identity("source"),
            "format_digest": ID0,
            "private_schema": 8,
            "version": {
                "generation": 2,
                "snapshotDigest": RAW,
                "selectionDigest": RAW,
                "captureCount": 0,
                "captureHead": null,
                "effectiveImageDigest": RAW
            }
        },
        "destinationIdentity": identity("destination"),
        "inheritedSegments": empty,
        "ancestryRootReceipt": null,
        "closure": {
            "snapshots": { "count": 3, "digest": ID0 },
            "selections": { "count": 3, "digest": ID0 },
            "captures": empty,
            "publications": empty,
            "catalogs": empty,
            "descriptors": empty,
            "activationMaps": empty,
            "transitionReceipts": empty,
            "companions": empty,
            "companionFiles": empty,
            "companionDeletions": empty,
            "contentMappings": empty
        }
    });
    let bytes = canonical_json(&legacy).unwrap();
    assert_eq!(prefix.encode().unwrap(), bytes);
    assert_eq!(ProviderImportPrefixRecord::decode(&bytes).unwrap(), prefix);
    assert_eq!(prefix.digest().unwrap(), codec::hash(PREFIX_DOMAIN, &bytes));
    assert!(!prefix.supports_restore_history());
    assert_eq!(prefix.authenticated_checkout_identity().unwrap(), None);
    assert_eq!(prefix.source_admission(), None);

    let mut legacy_v2 = legacy.clone();
    legacy_v2["schemaVersion"] = PREFIX_SCHEMA_V2.into();
    legacy_v2["closure"]["restoreHistory"] = serde_json::to_value(rowset(0)).unwrap();
    legacy_v2["source"]["checkout_identity"] = "fs:v1:0000000000000001:0000000000000002".into();
    assert_eq!(
        prefix_v2().encode().unwrap(),
        canonical_json(&legacy_v2).unwrap()
    );

    // Canonical decoding rejects explicit nulls as well as populated v1 extensions.
    for (section, field) in [
        ("closure", "restoreHistory"),
        ("source", "checkout_identity"),
        ("source", "admission"),
    ] {
        let mut extended = legacy.clone();
        extended[section][field] = serde_json::Value::Null;
        assert!(ProviderImportPrefixRecord::decode(&canonical_json(&extended).unwrap()).is_err());
    }
}

#[test]
fn prefix_v2_roundtrips_and_commits_restore_history_and_checkout() {
    let mut prefix = prefix_v2();
    let bytes = prefix.encode().unwrap();
    assert_eq!(ProviderImportPrefixRecord::decode(&bytes).unwrap(), prefix);
    assert!(prefix.supports_restore_history());
    assert_eq!(
        prefix.authenticated_checkout_identity().unwrap(),
        Some("fs:v1:0000000000000001:0000000000000002")
    );
    let digest = prefix.digest().unwrap();
    assert_eq!(digest, codec::hash(PREFIX_DOMAIN_V2, &bytes));
    assert_ne!(digest, codec::hash(PREFIX_DOMAIN, &bytes));
    prefix.closure.restore_history.as_mut().unwrap().digest = ID1.into();
    assert_ne!(prefix.digest().unwrap(), digest);
    prefix.closure.restore_history.as_mut().unwrap().digest = ID0.into();
    if let ImportSource::Ordinary {
        checkout_identity, ..
    } = &mut prefix.source
    {
        *checkout_identity = Some("fs:v1:0000000000000001:0000000000000003".into());
    }
    assert_ne!(prefix.digest().unwrap(), digest);
}

#[test]
fn prefix_versions_reject_missing_mixed_and_unknown_extensions() {
    let mut missing_history = prefix_v2();
    missing_history.closure.restore_history = None;
    assert!(missing_history.encode().is_err());
    assert!(missing_history.authenticated_checkout_identity().is_err());
    let mut missing_checkout = prefix_v2();
    if let ImportSource::Ordinary {
        checkout_identity, ..
    } = &mut missing_checkout.source
    {
        *checkout_identity = None;
    }
    assert!(missing_checkout.encode().is_err());
    let mut mixed = prefix_v2();
    mixed.schema_version = PREFIX_SCHEMA.into();
    assert!(mixed.encode().is_err());
    mixed.closure.restore_history = None;
    assert!(mixed.encode().is_err());
    let mut history_only = prefix();
    history_only.closure.restore_history = Some(rowset(0));
    assert!(history_only.encode().is_err());
    let mut unknown = prefix_v2();
    unknown.schema_version = "layrs.projection-provider-import-prefix.v3".into();
    assert!(unknown.encode().is_err());
    assert!(unknown.digest().is_err());

    let mut bytes: serde_json::Value =
        serde_json::from_slice(&prefix_v2().encode().unwrap()).unwrap();
    bytes["source"]["private_schema"] = 9.into();
    assert!(ProviderImportPrefixRecord::decode(&canonical_json(&bytes).unwrap()).is_err());
}

#[test]
fn prefix_v2_restore_rows_share_total_bound_and_validate_commitment() {
    let mut prefix = prefix_v2();
    prefix.closure.restore_history = Some(rowset(MAX_ROWS - 6));
    prefix.encode().unwrap();
    prefix.closure.restore_history.as_mut().unwrap().count += 1;
    assert!(prefix.encode().is_err());
    prefix.closure.restore_history.as_mut().unwrap().count = MAX_ROWS + 1;
    assert!(prefix.encode().is_err());
    prefix.closure.restore_history = Some(ImportRowset {
        count: 0,
        digest: RAW.into(),
    });
    assert!(prefix.encode().is_err());
}

#[test]
fn prefix_v2_checkout_requires_canonical_native_directory_identity() {
    for checkout in [
        "",
        "1:2:3",
        "fs:v1:0000000000000001:000000000000000A",
        "fs:v1:1:2",
        &"x".repeat(513),
    ] {
        let mut prefix = prefix_v2();
        if let ImportSource::Ordinary {
            checkout_identity, ..
        } = &mut prefix.source
        {
            *checkout_identity = Some(checkout.into());
        }
        assert!(prefix.encode().is_err());
    }
}

#[test]
fn prefix_v2_ordinary_checkout_must_match_exact_source_marker() {
    let fixture = Pure::new("G1", false);
    let marker = &fixture.allocation.format;
    let mut prefix = prefix_v2();
    if let ImportSource::Ordinary {
        identity,
        format_digest,
        checkout_identity,
        ..
    } = &mut prefix.source
    {
        *identity = marker.identity.clone();
        *format_digest = codec::hash(
            "layrs-projection-provider-import-format-v1",
            &marker.bytes().unwrap(),
        );
        *checkout_identity = Some(marker.checkout_identity().into());
    }
    prefix
        .require_references(ImportSourceReferences::Ordinary(marker))
        .unwrap();
    if let ImportSource::Ordinary {
        checkout_identity, ..
    } = &mut prefix.source
    {
        *checkout_identity = Some("fs:v1:ffffffffffffffff:ffffffffffffffff".into());
        assert_ne!(
            checkout_identity.as_deref(),
            Some(marker.checkout_identity())
        );
    }
    prefix.validate_fields().unwrap();
    assert!(prefix
        .require_references(ImportSourceReferences::Ordinary(marker))
        .is_err());
}

#[test]
fn prefix_v2_provider_uses_binding_checkout_and_v1_has_no_restore_authority() {
    let (mut prefix, fixture, activation) = provider_prefix();
    assert_eq!(prefix.authenticated_checkout_identity().unwrap(), None);
    prefix.schema_version = PREFIX_SCHEMA_V2.into();
    assert!(prefix.encode().is_err());
    prefix.closure.restore_history = Some(rowset(0));
    assert_eq!(
        prefix.authenticated_checkout_identity().unwrap(),
        Some(fixture.allocation.format.checkout_identity())
    );
    prefix
        .require_references(ImportSourceReferences::Provider {
            allocation: &fixture.allocation,
            prepared: &fixture.prepared,
            activation: &activation,
        })
        .unwrap();
    let bytes = prefix.encode().unwrap();
    assert_eq!(ProviderImportPrefixRecord::decode(&bytes).unwrap(), prefix);
    let mut mixed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    mixed["source"]["checkout_identity"] = fixture.allocation.format.checkout_identity().into();
    assert!(ProviderImportPrefixRecord::decode(&canonical_json(&mixed).unwrap()).is_err());
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
        transition_plan_digest: ID1.into(),
        destination_proof_digest: ID1.into(),
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

/// Detached codec DATA: no admission row, SQL store or native identity is
/// authenticated by constructing this fixture.
fn admitted_prefix() -> (
    ProviderImportPrefixRecord,
    format::Marker,
    ProviderTransitionReceiptRecord,
) {
    let pure = Pure::new("G1", false);
    let mut marker = pure.allocation.format.clone();
    let mut incoming = prefix();
    if let ImportSource::Ordinary {
        identity: source, ..
    } = &mut incoming.source
    {
        *source = identity("original");
    }
    incoming.destination_identity = marker.identity.clone();
    let parent = receipt_from(&incoming, "incoming-admission");
    let segment = ImportedSegment::from_receipt(&parent, None).unwrap();
    let parent_id = parent.digest().unwrap();
    marker.import_digest = Some(parent.source_prefix_id.clone());
    marker.validate().unwrap();
    let admission = AdmittedSourceBinding {
        admission_digest: ID2.into(),
        incoming_prefix_id: parent.source_prefix_id.clone(),
        incoming_receipt_id: parent_id.clone(),
        database_identity: marker.shared.as_ref().unwrap().database_identity.clone(),
    };
    let mut prefix = prefix_v2();
    prefix.source = ImportSource::Ordinary {
        identity: marker.identity.clone(),
        format_digest: codec::hash(
            "layrs-projection-provider-import-format-v1",
            &marker.bytes().unwrap(),
        ),
        private_schema: 9,
        version: ImportVersion {
            generation: parent.destination_placement_generation,
            snapshot_digest: parent.destination_placement_digest.clone(),
            selection_digest: RAW.into(),
            capture_count: parent.destination_capture_sequence,
            capture_head: Some(parent.destination_capture_digest.clone()),
            effective_image_digest: RAW.into(),
        },
        checkout_identity: Some(marker.checkout_identity().into()),
        admission: Some(admission),
    };
    prefix.inherited_segments = ImportRowset {
        count: 1,
        digest: segments_digest(std::slice::from_ref(&segment)),
    };
    prefix.ancestry_root_receipt = Some(parent_id);
    prefix.closure.snapshots.count = parent.destination_placement_generation + 1;
    prefix.closure.captures.count = parent.destination_capture_sequence;
    prefix.closure.transition_receipts.count = 1;
    prefix.validate_segments(&[segment]).unwrap();
    (prefix, marker, parent)
}

fn set_format_digest(prefix: &mut ProviderImportPrefixRecord, marker: &format::Marker) {
    let ImportSource::Ordinary { format_digest, .. } = &mut prefix.source else {
        unreachable!()
    };
    *format_digest = codec::hash(
        "layrs-projection-provider-import-format-v1",
        &marker.bytes().unwrap(),
    );
}

#[test]
fn admitted_source_binding_roundtrips_with_canonical_bounded_fields() {
    let (prefix, marker, parent) = admitted_prefix();
    let bytes = prefix.encode().unwrap();
    assert_eq!(ProviderImportPrefixRecord::decode(&bytes).unwrap(), prefix);
    let binding = prefix.source_admission().unwrap();
    let fields = serde_json::to_value(binding).unwrap();
    assert_eq!(fields["admissionDigest"], ID2);
    assert_eq!(fields["incomingPrefixId"], parent.source_prefix_id);
    assert_eq!(fields["incomingReceiptId"], parent.digest().unwrap());
    assert_eq!(
        fields["databaseIdentity"],
        marker.shared.as_ref().unwrap().database_identity
    );
    let mut unknown = fields;
    unknown["unexpected"] = true.into();
    assert!(serde_json::from_value::<AdmittedSourceBinding>(unknown).is_err());
    let mut changed = ProviderImportPrefixRecord::decode(&bytes).unwrap();
    if let ImportSource::Ordinary {
        admission: Some(admission),
        ..
    } = &mut changed.source
    {
        admission.admission_digest = ID1.into();
    }
    assert_ne!(changed.digest().unwrap(), prefix.digest().unwrap());

    for name in ["admissionDigest", "incomingPrefixId", "incomingReceiptId"] {
        let mut malformed = serde_json::to_value(&prefix).unwrap();
        malformed["source"]["admission"][name] = RAW.into();
        assert!(ProviderImportPrefixRecord::decode(&canonical_json(&malformed).unwrap()).is_err());
    }
    for database in [
        "",
        "1:2",
        "01:0:1",
        "1:0:4294967296",
        "fs:v1:0000000000000001:0000000000000002",
        &"1".repeat(513),
    ] {
        let mut malformed = serde_json::to_value(&prefix).unwrap();
        malformed["source"]["admission"]["databaseIdentity"] = database.into();
        assert!(ProviderImportPrefixRecord::decode(&canonical_json(&malformed).unwrap()).is_err());
    }
}

#[test]
fn admitted_source_requires_v2_schema9_checkout_and_incoming_ancestry() {
    let (prefix, _, _) = admitted_prefix();
    let bytes = prefix.encode().unwrap();
    for schema in [6, 7, 8, 10] {
        let mut wrong = ProviderImportPrefixRecord::decode(&bytes).unwrap();
        if let ImportSource::Ordinary { private_schema, .. } = &mut wrong.source {
            *private_schema = schema;
        }
        assert!(wrong.encode().is_err());
    }
    let mut no_admission = ProviderImportPrefixRecord::decode(&bytes).unwrap();
    if let ImportSource::Ordinary { admission, .. } = &mut no_admission.source {
        *admission = None;
    }
    assert!(no_admission.encode().is_err());
    let mut no_checkout = ProviderImportPrefixRecord::decode(&bytes).unwrap();
    if let ImportSource::Ordinary {
        checkout_identity, ..
    } = &mut no_checkout.source
    {
        *checkout_identity = None;
    }
    assert!(no_checkout.encode().is_err());
    let mut legacy = ProviderImportPrefixRecord::decode(&bytes).unwrap();
    legacy.schema_version = PREFIX_SCHEMA.into();
    assert!(legacy.encode().is_err());
    legacy.closure.restore_history = None;
    if let ImportSource::Ordinary {
        checkout_identity, ..
    } = &mut legacy.source
    {
        *checkout_identity = None;
    }
    assert!(legacy.encode().is_err());
    let mut wrong_root = ProviderImportPrefixRecord::decode(&bytes).unwrap();
    wrong_root.ancestry_root_receipt = Some(ID1.into());
    assert!(wrong_root.encode().is_err());
    let mut no_ancestry = ProviderImportPrefixRecord::decode(&bytes).unwrap();
    no_ancestry.inherited_segments.count = 0;
    no_ancestry.ancestry_root_receipt = None;
    assert!(no_ancestry.encode().is_err());
    let (mut provider, _, _) = provider_prefix();
    if let ImportSource::Provider { private_schema, .. } = &mut provider.source {
        *private_schema = 9;
    }
    assert!(provider.encode().is_err());
}

#[test]
fn admitted_source_references_require_exact_marker_database_checkout_and_binding() {
    let (mut prefix, marker, _) = admitted_prefix();
    let binding = prefix.source_admission().unwrap().clone();
    prefix
        .require_references(ImportSourceReferences::AdmittedOrdinary {
            marker: &marker,
            admission: &binding,
        })
        .unwrap();
    assert!(prefix
        .require_references(ImportSourceReferences::Ordinary(&marker))
        .is_err());
    let mut wrong_binding = binding.clone();
    wrong_binding.admission_digest = ID1.into();
    assert!(prefix
        .require_references(ImportSourceReferences::AdmittedOrdinary {
            marker: &marker,
            admission: &wrong_binding
        })
        .is_err());
    let original = prefix.encode().unwrap();
    for field in ["database", "incoming", "identity", "checkout", "format"] {
        prefix = ProviderImportPrefixRecord::decode(&original).unwrap();
        let mut wrong_marker = marker.clone();
        match field {
            "database" => {
                wrong_marker.shared.as_mut().unwrap().database_identity = "1:0:123".into()
            }
            "incoming" => wrong_marker.import_digest = Some(ID1.into()),
            "identity" => {
                wrong_marker.identity.worktree_id = WorktreeId::parse("worktree:wrong").unwrap()
            }
            "checkout" => {
                if let ImportSource::Ordinary {
                    checkout_identity, ..
                } = &mut prefix.source
                {
                    *checkout_identity = Some("fs:v1:ffffffffffffffff:ffffffffffffffff".into());
                }
            }
            "format" => {}
            _ => unreachable!(),
        }
        if field != "format" {
            set_format_digest(&mut prefix, &wrong_marker);
        } else if let ImportSource::Ordinary { format_digest, .. } = &mut prefix.source {
            *format_digest = ID1.into();
        }
        assert!(
            prefix
                .require_references(ImportSourceReferences::AdmittedOrdinary {
                    marker: &wrong_marker,
                    admission: &binding
                })
                .is_err(),
            "{field}"
        );
    }
    assert!(prefix_v2()
        .require_references(ImportSourceReferences::AdmittedOrdinary {
            marker: &marker,
            admission: &binding
        })
        .is_err());
}

#[test]
fn admitted_source_parent_requires_exact_receipt_prefix_and_destination() {
    let (prefix, _, parent) = admitted_prefix();
    prefix.require_admission_parent(Some(&parent)).unwrap();
    assert!(prefix.require_admission_parent(None).is_err());
    let mut other = ProviderTransitionReceiptRecord::decode(&parent.encode().unwrap()).unwrap();
    other.transaction_id = "other-incoming".into();
    other.validate_fields().unwrap();
    assert!(prefix.require_admission_parent(Some(&other)).is_err());
    let bytes = prefix.encode().unwrap();
    let mut wrong_prefix = ProviderImportPrefixRecord::decode(&bytes).unwrap();
    if let ImportSource::Ordinary {
        admission: Some(admission),
        ..
    } = &mut wrong_prefix.source
    {
        admission.incoming_prefix_id = ID1.into();
    }
    wrong_prefix.validate_fields().unwrap();
    assert!(wrong_prefix
        .require_admission_parent(Some(&parent))
        .is_err());
    let mut wrong_origin = ProviderImportPrefixRecord::decode(&bytes).unwrap();
    if let ImportSource::Ordinary { identity, .. } = &mut wrong_origin.source {
        identity.worktree_id = WorktreeId::parse("worktree:unrelated").unwrap();
    }
    wrong_origin.validate_fields().unwrap();
    assert!(wrong_origin
        .require_admission_parent(Some(&parent))
        .is_err());
    prefix_v2().require_admission_parent(None).unwrap();
    prefix_v2().require_admission_parent(Some(&other)).unwrap();
}

#[test]
fn receipt_companion_commits_header_local_domains() {
    let prefix = prefix();
    let files_digest = ordered_companion_rowset(FILES_DOMAIN, &[]).unwrap();
    let deletions_digest = ordered_companion_rowset(DELETIONS_DOMAIN, &[]).unwrap();
    let header = ProviderCompanionHeaderRecord {
        schema_version: COMPANION_SCHEMA.into(),
        source_identity: prefix.source.identity().clone(),
        source_private_schema: 8,
        source_generation: prefix.source.version().generation,
        source_capture_count: 0,
        source_capture_head: None,
        import_prefix_id: prefix.digest().unwrap(),
        catalog_id: None,
        content_mappings: prefix.closure.content_mappings.clone(),
        file_count: 0,
        deletion_count: 0,
        files_digest: files_digest.clone(),
        deletions_digest: deletions_digest.clone(),
    };
    header.validate_rows(&[], &[]).unwrap();
    let mut receipt = receipt_from(&prefix, "companion");
    receipt.companion_id = Some(header.digest().unwrap());
    receipt.companion_files = ImportRowset {
        count: 0,
        digest: files_digest,
    };
    receipt.companion_deletions = ImportRowset {
        count: 0,
        digest: deletions_digest,
    };
    receipt.require_companion(&header).unwrap();
    receipt.companion_files.digest = ID1.into();
    assert!(receipt.require_companion(&header).is_err());
}

#[test]
fn import_kind_names_destination_and_is_independent_of_source() {
    let mut ordinary = prefix();
    ordinary.kind = ProviderImportKind::ExpandedProvider;
    ordinary.validate_fields().unwrap();
    let (mut provider, fixture, activation) = provider_prefix();
    provider.kind = ProviderImportKind::ManagedOrdinary;
    provider.validate_fields().unwrap();
    provider
        .require_references(ImportSourceReferences::Provider {
            allocation: &fixture.allocation,
            prepared: &fixture.prepared,
            activation: &activation,
        })
        .unwrap();
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
        checkout_identity: None,
        admission: None,
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
        transition_plan_digest: ID1.into(),
        destination_proof_digest: ID1.into(),
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
