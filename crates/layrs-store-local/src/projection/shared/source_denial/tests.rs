//! Authored source-only recipes, NOT EXECUTED. Synthetic DATA is neither an
//! authenticated observation nor a physical C Ready/publication capability.
use super::*;
use crate::projection::{
    RecoveryBinding,
    provider::{
        database_io::with_reference,
        owned_test as child,
        records::{
            PreparedBinding, ReceiptRef,
            denied_coverage::DeniedAuthorityData,
            import::{
                ImportClosure, ImportRowset, ImportVersion, ProviderImportKind,
                ProviderImportPrefixRecord,
            },
            transition::{
                TransitionBackendKind, TransitionDestinationProof, TransitionObservation,
                TransitionPlan, TransitionRowset, TransitionState,
            },
        },
    },
    shared::schema_v2,
};
use serde::Deserialize;

#[path = "tests/release.rs"]
pub(super) mod release_recipes;

#[path = "tests/capacity.rs"]
mod capacity_recipes;

const TX: &str = "00000000000000000000000000000002";
const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";

fn literal(field: &str) -> Vec<u8> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Golden {
        label: String,
        field: String,
        byte_length: usize,
        hex: String,
    }
    let bytes = include_bytes!("../../provider/records/golden-r4.json");
    let rows: Vec<Golden> =
        serde_json::from_slice(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes)).unwrap();
    let row = rows
        .into_iter()
        .find(|r| r.label == "G0" && r.field == field)
        .unwrap();
    let bytes: Vec<_> = row
        .hex
        .as_bytes()
        .chunks_exact(2)
        .map(|v| u8::from_str_radix(std::str::from_utf8(v).unwrap(), 16).unwrap())
        .collect();
    assert_eq!(bytes.len(), row.byte_length);
    bytes
}

fn fixture() -> (
    AllocationRecord,
    ProviderTransitionIntentRecord,
    TransitionDeniedCoverageRecord,
) {
    let marker: crate::projection::format::Marker =
        serde_json::from_slice(&literal("FORMAT")).unwrap();
    let metadata = marker
        .shared
        .as_ref()
        .unwrap()
        .metadata_path
        .to_str()
        .unwrap();
    let bytes = marker.bytes().unwrap();
    let a: AllocationRecord = serde_json::from_value(serde_json::json!({
        "schemaVersion":"layrs.projection-provider-allocation.v1", "transactionId":"00000000000000000000000000000001", "receiptFileIdentity":"1:0:24",
        "family":{"root":"C:\\family","canonicalRoot":"C:\\family","incarnation":"test-incarnation",
            "rootIdentity":"fs:v1:0000000000000001:0000000000000014","markerFileIdentity":"1:0:23",
            "markerDigest":"blake3:0000000000000000000000000000000000000000000000000000000000000000",
            "metadataIdentity":"fs:v1:0000000000000001:0000000000000015","catalogFileIdentity":"1:0:22"},
        "private":{"metadataPath":metadata,"canonicalMetadataPath":metadata,
            "metadataIdentity":"fs:v1:0000000000000001:0000000000000009",
            "providerControlIdentity":"fs:v1:0000000000000001:000000000000000a","databaseFileIdentity":"1:0:11"},
        "stageIdentityHex":literal("LYRCSTG1").iter().map(|b| format!("{b:02x}")).collect::<String>(),
        "format":marker,"formatBytes":bytes.len(),"formatFileId":layrs_core::scm::FileId::from_bytes(&bytes),
        "source":serde_json::from_slice::<serde_json::Value>(&literal("SourceCommitment")).unwrap()
    })).unwrap();
    a.validate_fields().unwrap();
    let allocation = a.data_reference().unwrap();
    let prepared = ReceiptRef {
        file_identity: "1:0:25".into(),
        byte_length: 100,
        digest: TOKEN.into(),
    };
    let binding = crate::projection::provider::records::ProviderBootstrapBinding {
        schema_version: "layrs.projection-provider-bootstrap-binding.v1".into(),
        recovery: RecoveryBinding {
            identity: a.format.identity.clone(),
            checkout_identity: a.format.checkout_identity().into(),
            marker_digest: format!(
                "blake3:{}",
                digest("layrs-projection-recovery-marker-v1", &bytes)
            ),
            database_identity: a.private.database_file_identity.clone(),
            family_marker_digest: Some(a.family.marker_digest.clone()),
        },
        transaction_id: a.transaction_id.clone(),
        allocation: allocation.clone(),
        prepared: Some(PreparedBinding {
            receipt: prepared.clone(),
            control_identity_hex: literal("LYRCDIR1")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        }),
        abandoned: None,
    };
    let mut destination = a.format.identity.clone();
    destination.worktree_id = WorktreeId::parse("worktree:source-denial-destination").unwrap();
    let row = |count| ImportRowset {
        count,
        digest: TOKEN.into(),
    };
    let version = ImportVersion {
        generation: 0,
        snapshot_digest: a.source.snapshot_digest.clone(),
        selection_digest: a.source.selection_digest.clone(),
        capture_count: 0,
        capture_head: None,
        effective_image_digest: "a".repeat(64),
    };
    let prefix = ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v1".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Provider {
            identity: a.format.identity.clone(),
            format_digest: format!(
                "blake3:{}",
                digest("layrs-projection-provider-import-format-v1", &bytes)
            ),
            private_schema: 8,
            allocation,
            prepared,
            binding,
            activation_map_id: TOKEN.into(),
            version: version.clone(),
        },
        destination_identity: destination.clone(),
        inherited_segments: row(0),
        ancestry_root_receipt: None,
        closure: ImportClosure {
            snapshots: row(1),
            selections: row(1),
            captures: row(0),
            publications: row(0),
            catalogs: row(0),
            descriptors: row(0),
            activation_maps: row(0),
            transition_receipts: row(0),
            companions: row(0),
            companion_files: row(0),
            companion_deletions: row(0),
            content_mappings: row(0),
            restore_history: None,
            denial_observations: None,
        },
    };
    let snapshot: layrs_core::projection::ProjectionSnapshot =
        serde_json::from_slice(&literal("S")).unwrap();
    let authority = DeniedAuthorityData::PreservedEpoch {
        policy_set_id: snapshot.observed_policy_set_id.clone(),
        authorization_epoch: 1,
    };
    let mut rows = blake3::Hasher::new();
    rows.update(b"layrs-provider-transition-denied-paths-data-v1\0");
    rows.update(&0u64.to_be_bytes());
    let body = TransitionDeniedCoverageRecord {
        schema_version: "layrs.projection-transition-denied-coverage.v1".into(),
        transaction_id: TX.into(),
        source_prefix_id: prefix.digest().unwrap(),
        source_identity: a.format.identity.clone(),
        destination_identity: destination,
        source_version: version,
        step_id: snapshot.base_step_id.clone(),
        root_tree_id: snapshot.base_root_tree_id.clone(),
        path_semantics: snapshot.path_semantics,
        authority_data_digest: format!(
            "blake3:{}",
            digest(
                "layrs-provider-transition-authority-data-v1",
                &canonical_json(&authority).unwrap()
            )
        ),
        authority,
        observation_generation: 1,
        observation_snapshot_digest: "b".repeat(64),
        observation_selection_digest: "c".repeat(64),
        denied_path_count: 0,
        path_bytes: 0,
        rows_digest: format!("blake3:{}", rows.finalize()),
        denied_paths: vec![],
    };
    let empty = || TransitionRowset {
        count: 0,
        digest: TOKEN.into(),
    };
    let intent = ProviderTransitionIntentRecord {
        schema_version: "layrs.projection-provider-transition-intent.v2".into(),
        transaction_id: TX.into(),
        role: TransitionRole::Source,
        state: TransitionState::Planned,
        local_generation: 0,
        plan: TransitionPlan {
            kind: ProviderImportKind::ManagedOrdinary,
            source_prefix_id: prefix.digest().unwrap(),
            source_prefix: prefix,
            destination_parent: "C:\\source-denial-parent".into(),
            destination_parent_identity: "fs:v1:1111111111111111:2222222222222222".into(),
            destination_leaf_nonce: "00000000000000000000000000000003".into(),
            observation: TransitionObservation {
                step_id: body.step_id.clone(),
                root_tree_id: body.root_tree_id.clone(),
                policy_set_id: snapshot.observed_policy_set_id,
                authorization_epoch: 1,
                generation: 1,
                snapshot_digest: body.observation_snapshot_digest.clone(),
                selection_digest: body.observation_selection_digest.clone(),
            },
            placement_generation: 2,
            placement_digest: "d".repeat(64),
            capture_sequence: 1,
            capture_digest: TOKEN.into(),
            working_overlay: empty(),
            companion_id: None,
            companion_files: empty(),
            companion_deletions: empty(),
            protected_manifests: empty(),
            quota_bytes: 0,
            denial_coverage: Some(body.reference().unwrap()),
        },
        destination: None,
        reservation: None,
        receipt_id: None,
    };
    intent.validate_fields().unwrap();
    (a, intent, body)
}

#[test]
fn denial_funding_binds_full_body_allocation_and_exact_sql_cost_before_c_exists() {
    let (a, intent, body) = fixture();
    assert!(intent.destination.is_none());
    let funding = SourceDeniedCoverageReservation::new(&a, &intent, &body, None).unwrap();
    assert_eq!(
        funding.document_bytes(),
        body.encode().unwrap().len() as u64 + 71
    );
    assert_eq!(
        funding.intent_bytes(),
        ProviderTransitionIntentRecord::MAX_BYTES as u64
            + TX.len() as u64
            + 6
            + 9
            + 8
            + 128
            + 71
            + 71
    );
    assert_eq!(
        funding.reservation_bytes(),
        funding.document_bytes()
            + funding.intent_bytes()
            + 2 * funding.operation().len() as u64
            + a.format.identity.worktree_id.as_str().len() as u64
            + 8
    );
    assert!(funding.operation().starts_with(NAMESPACE));
    let mut foreign = AllocationRecord::decode(&a.encode().unwrap()).unwrap();
    foreign.receipt_file_identity = "1:0:99".into();
    foreign.validate_fields().unwrap();
    assert!(SourceDeniedCoverageReservation::new(&foreign, &intent, &body, None).is_err());
    let mut altered = TransitionDeniedCoverageRecord::decode(&body.encode().unwrap()).unwrap();
    altered.observation_snapshot_digest = "f".repeat(64);
    assert!(SourceDeniedCoverageReservation::new(&a, &intent, &altered, None).is_err());
    let mut linked = ProviderTransitionIntentRecord::decode(&intent.encode().unwrap()).unwrap();
    linked.plan.denial_coverage = Some(altered.reference().unwrap());
    assert!(
        SourceDeniedCoverageReservation::new(&a, &linked, &altered, None).is_err(),
        "rehashing does not adopt a foreign body"
    );
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(SourceDeniedCoverageReservation::new(&a, &intent, &body, Some(&cancel)).is_err());
    assert!(add(i64::MAX as u64, 1).is_err());
}

#[test]
fn denial_operation_separates_family_catalog_database_transaction_plan_and_document() {
    let (a, intent, body) = fixture();
    let original = SourceDeniedCoverageReservation::new(&a, &intent, &body, None).unwrap();
    for changed in [
        "incarnation",
        "root",
        "catalog",
        "marker",
        "database",
        "transaction",
        "plan",
        "document",
    ] {
        let mut a = AllocationRecord::decode(&a.encode().unwrap()).unwrap();
        let mut intent = ProviderTransitionIntentRecord::decode(&intent.encode().unwrap()).unwrap();
        let mut body = TransitionDeniedCoverageRecord::decode(&body.encode().unwrap()).unwrap();
        match changed {
            "incarnation" => {
                a.family.incarnation = "other-incarnation".into();
                a.format.shared.as_mut().unwrap().incarnation = a.family.incarnation.clone();
            }
            "root" => {
                a.family.root_identity = "fs:v1:0000000000000001:0000000000000099".into();
                a.format.shared.as_mut().unwrap().root_identity = a.family.root_identity.clone();
            }
            "catalog" => a.family.catalog_file_identity = "1:0:99".into(),
            "marker" => a.family.marker_digest = TOKEN.into(),
            "database" => {
                a.private.database_file_identity = "1:0:99".into();
                a.format.shared.as_mut().unwrap().database_identity =
                    a.private.database_file_identity.clone();
            }
            "transaction" => {
                intent.transaction_id = "00000000000000000000000000000004".into();
                body.transaction_id = intent.transaction_id.clone();
            }
            "plan" => {
                intent.plan.destination_leaf_nonce = "00000000000000000000000000000004".into()
            }
            "document" => {
                body.observation_snapshot_digest = "f".repeat(64);
                intent.plan.observation.snapshot_digest = body.observation_snapshot_digest.clone();
            }
            _ => unreachable!(),
        }
        let bytes = a.format.bytes().unwrap();
        a.format_bytes = bytes.len() as u64;
        a.format_file_id = layrs_core::scm::FileId::from_bytes(&bytes);
        let ImportSource::Provider {
            allocation,
            binding,
            format_digest,
            ..
        } = &mut intent.plan.source_prefix.source
        else {
            unreachable!()
        };
        *allocation = a.data_reference().unwrap();
        binding.allocation = allocation.clone();
        binding.recovery.database_identity = a.private.database_file_identity.clone();
        binding.recovery.family_marker_digest = Some(a.family.marker_digest.clone());
        binding.recovery.marker_digest = format!(
            "blake3:{}",
            digest("layrs-projection-recovery-marker-v1", &bytes)
        );
        *format_digest = format!(
            "blake3:{}",
            digest("layrs-projection-provider-import-format-v1", &bytes)
        );
        intent.plan.source_prefix_id = intent.plan.source_prefix.digest().unwrap();
        body.source_prefix_id = intent.plan.source_prefix_id.clone();
        intent.plan.denial_coverage = Some(body.reference().unwrap());
        let other = SourceDeniedCoverageReservation::new(&a, &intent, &body, None).unwrap();
        assert_ne!(
            other.operation(),
            original.operation(),
            "operation adopted different {changed}"
        );
    }
}

#[test]
fn denial_operation_and_funding_survive_every_source_phase_as_data_only() {
    let (a, mut intent, body) = fixture();
    let original = SourceDeniedCoverageReservation::new(&a, &intent, &body, None).unwrap();
    let mut marker = serde_json::to_value(&a.format).unwrap();
    marker["identity"] =
        serde_json::to_value(&intent.plan.source_prefix.destination_identity).unwrap();
    marker["checkoutRoot"] = std::path::Path::new(&intent.plan.destination_parent)
        .join(&intent.plan.destination_leaf_nonce)
        .to_str()
        .unwrap()
        .into();
    marker["importDigest"] = intent.plan.source_prefix_id.clone().into();
    let owner = intent
        .plan
        .source_prefix
        .destination_identity
        .worktree_id
        .as_str();
    marker["shared"]["metadataPath"] = format!(
        "C:\\family\\worktrees\\{}\\metadata",
        blake3::hash(owner.as_bytes()).to_hex()
    )
    .into();
    marker["shared"]["databaseIdentity"] = "1:0:98".into();
    let proof = TransitionDestinationProof {
        backend: TransitionBackendKind::Ordinary,
        format: serde_json::from_value(marker).unwrap(),
        format_file_identity: "1:0:97".into(),
        private_database_identity: "1:0:98".into(),
        family_marker_digest: a.family.marker_digest.clone(),
        native_control_identity_hex: None,
        native_root_identity: None,
    };
    // These are codec-valid synthetic DATA states only; no SQL Ready row,
    // private C database, filesystem proof or publication is manufactured.
    intent.destination = Some(proof);
    for state in [
        TransitionState::Copying,
        TransitionState::Staging,
        TransitionState::Ready,
        TransitionState::Published,
        TransitionState::Cancelled,
    ] {
        intent.state = state;
        intent.reservation = if matches!(
            state,
            TransitionState::Staging | TransitionState::Ready | TransitionState::Published
        ) {
            Some(intent.reservation_operation().unwrap())
        } else {
            None
        };
        intent.receipt_id = if matches!(state, TransitionState::Ready | TransitionState::Published)
        {
            Some(TOKEN.into())
        } else {
            None
        };
        assert_eq!(
            SourceDeniedCoverageReservation::new(&a, &intent, &body, None).unwrap(),
            original
        );
    }
    intent.state = TransitionState::Cancelled;
    intent.destination = None;
    assert_eq!(
        SourceDeniedCoverageReservation::new(&a, &intent, &body, None).unwrap(),
        original
    );
}

fn seed(c: &Connection, funding: &SourceDeniedCoverageReservation, limit: u64) -> Result<()> {
    c.execute("INSERT INTO family VALUES(1,?1)", [b"family".as_slice()])?;
    c.execute("INSERT INTO quota VALUES(1,?1)", [limit])?;
    c.execute(
        "INSERT INTO worktrees VALUES(?1,'source',?2,?3,'ready')",
        params![
            funding.binding().source_owner.as_str(),
            funding.binding().source_database,
            b"source".as_slice()
        ],
    )?;
    c.execute(
        "INSERT INTO worktrees VALUES('worktree:foreign','foreign','1:0:55',?1,'ready')",
        [b"foreign".as_slice()],
    )?;
    Ok(())
}

#[test]
#[ignore = "unrun owned SQL recipe; dedicated metadata B capacity, no live C14 driver"]
fn denial_sql_exact_capacity_replay_counts_all_metadata_and_foreign_reservations() {
    child::run(
        concat!(
            module_path!(),
            "::denial_sql_exact_capacity_replay_counts_all_metadata_and_foreign_reservations"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                schema_v2::initialize_reference(c)?;
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let (a, intent, body) = fixture();
                let funding = SourceDeniedCoverageReservation::new(&a, &intent, &body, None)?;
                let extra = 11 + 13 + 17 + 19 + 8192 + 23;
                seed(c, &funding, funding.reservation_bytes() + extra - 1)?;
                c.execute("INSERT INTO chunks VALUES('chunk',11)", [])?;
                c.execute("INSERT INTO sweep VALUES('chunks','retired',13)", [])?;
                c.execute("INSERT INTO files VALUES(?1,0)", [TOKEN])?;
                c.execute(
                    "INSERT INTO manifests VALUES(?1,?1,0,?2)",
                    params![TOKEN, vec![0u8; 17]],
                )?;
                c.execute(
                    "INSERT INTO reservations VALUES('foreign','worktree:foreign','foreign',19)",
                    [],
                )?;
                c.execute(
                    "INSERT INTO exact_installations VALUES('foreign',?1,?1,0,?2,?3,0)",
                    params![TOKEN, b"journal".as_slice(), vec![0u8; 23]],
                )?;
                let changes = c.total_changes();
                assert!(funding.reserve(c, None).is_err());
                assert_eq!(changes, c.total_changes());
                assert_eq!(
                    funding.read_presence(c, None)?,
                    SourceDeniedCoveragePresence::Absent
                );
                c.execute("UPDATE quota SET bytes=bytes+1", [])?;
                assert_eq!(funding.reserve(c, None)?, ProtectionEffect::Inserted);
                funding.require_present(c, None)?;
                let changes = c.total_changes();
                assert_eq!(funding.reserve(c, None)?, ProtectionEffect::AlreadyPresent);
                assert_eq!(changes, c.total_changes());
                c.execute("UPDATE quota SET bytes=bytes-1", [])?;
                let changes = c.total_changes();
                assert!(funding.reserve(c, None).is_err());
                assert!(funding.require_present(c, None).is_err());
                funding.require_retained(c, None)?;
                assert_eq!(changes, c.total_changes());
                c.execute_batch("ROLLBACK; BEGIN IMMEDIATE")?;
                let count: u64 =
                    c.query_row("SELECT count(*) FROM reservations", [], |r| r.get(0))?;
                assert_eq!(count, 0);
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
        },
    );
}

#[test]
#[ignore = "unrun owned SQL recipe; immutable reservation collisions and installation fences"]
fn denial_sql_rejects_conflicts_malformed_remaining_and_installations_without_repair() {
    child::run(
        concat!(
            module_path!(),
            "::denial_sql_rejects_conflicts_malformed_remaining_and_installations_without_repair"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                schema_v2::initialize_reference(c)?;
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let (a, intent, body) = fixture();
                let funding = SourceDeniedCoverageReservation::new(&a, &intent, &body, None)?;
                seed(c, &funding, MAX_METADATA)?;
                for sql in [
                    "INSERT INTO reservations VALUES(?1,'worktree:foreign',?1,?2)",
                    "INSERT INTO reservations VALUES('collision','worktree:foreign',?1,?2)",
                    "INSERT INTO reservations VALUES(?1,?3,'different',?2)",
                    "INSERT INTO reservations VALUES(?1,?3,?1,?2-1)",
                    "INSERT INTO reservations VALUES(?1,?3,?1,?2+1)",
                ] {
                    c.execute_batch("SAVEPOINT corrupt")?;
                    let mut statement = c.prepare(sql)?;
                    statement.raw_bind_parameter(1, funding.operation())?;
                    statement.raw_bind_parameter(2, funding.reservation_bytes())?;
                    if statement.parameter_count() == 3 {
                        statement.raw_bind_parameter(3, funding.binding().source_owner.as_str())?;
                    }
                    statement.raw_execute()?;
                    drop(statement);
                    let changes = c.total_changes();
                    assert!(funding.reserve(c, None).is_err());
                    assert_eq!(changes, c.total_changes());
                    c.execute_batch("ROLLBACK TO corrupt; RELEASE corrupt")?;
                }
                funding.reserve(c, None)?;
                c.execute_batch("SAVEPOINT corrupt; PRAGMA ignore_check_constraints=ON")?;
                c.execute(
                    "UPDATE reservations SET remaining=-1 WHERE id=?1",
                    [funding.operation()],
                )?;
                let changes = c.total_changes();
                assert!(funding.reserve(c, None).is_err());
                assert_eq!(changes, c.total_changes());
                c.execute_batch(
                    "ROLLBACK TO corrupt; RELEASE corrupt; PRAGMA ignore_check_constraints=OFF",
                )?;
                for exact in [false, true] {
                    c.execute_batch("SAVEPOINT corrupt")?;
                    if exact {
                        c.execute(
                            "INSERT INTO exact_installations VALUES(?1,?2,?2,0,?3,NULL,0)",
                            params![funding.operation(), TOKEN, b"journal".as_slice()],
                        )?;
                    } else {
                        c.execute(
                            "INSERT INTO installations VALUES(?1,?2,?3,NULL,0)",
                            params![funding.operation(), TOKEN, b"journal".as_slice()],
                        )?;
                    }
                    let changes = c.total_changes();
                    assert!(funding.reserve(c, None).is_err());
                    assert_eq!(changes, c.total_changes());
                    c.execute_batch("ROLLBACK TO corrupt; RELEASE corrupt")?;
                }
                funding.require_present(c, None)?;
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
        },
    );
}

#[test]
#[ignore = "unrun owned SQL recipe; transaction/FK/query-only/cancellation preconditions"]
fn denial_sql_requires_main_write_fk_writable_and_cancellation_before_effects() {
    child::run(
        concat!(
            module_path!(),
            "::denial_sql_requires_main_write_fk_writable_and_cancellation_before_effects"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                schema_v2::initialize_reference(c)?;
                let (a, intent, body) = fixture();
                let funding = SourceDeniedCoverageReservation::new(&a, &intent, &body, None)?;
                let changes = c.total_changes();
                assert!(funding.reserve(c, None).is_err());
                assert_eq!(changes, c.total_changes());
                for profile in [
                    "PRAGMA foreign_keys=ON; BEGIN DEFERRED",
                    "PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE",
                    "PRAGMA foreign_keys=ON; BEGIN IMMEDIATE; PRAGMA query_only=ON",
                ] {
                    c.execute_batch(profile)?;
                    let changes = c.total_changes();
                    assert!(funding.reserve(c, None).is_err());
                    assert_eq!(changes, c.total_changes());
                    c.execute_batch("ROLLBACK; PRAGMA query_only=OFF")?;
                }
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                seed(c, &funding, MAX_METADATA)?;
                let cancel = Cancellation::default();
                cancel.cancel();
                let changes = c.total_changes();
                assert!(funding.reserve(c, Some(&cancel)).is_err());
                assert!(funding.read_presence(c, Some(&cancel)).is_err());
                assert_eq!(changes, c.total_changes());
                funding.reserve(c, None)?;
                c.execute(
                    "UPDATE worktrees SET database_identity='1:0:99' WHERE id=?1",
                    [funding.binding().source_owner.as_str()],
                )?;
                let changes = c.total_changes();
                assert!(funding.reserve(c, None).is_err());
                assert_eq!(changes, c.total_changes());
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
        },
    );
}

#[test]
#[ignore = "unrun owned SQL classifier recipe; no physical FamilyAccess or private proof"]
fn pending_denial_classifier_accepts_only_exact_absent_or_reserved_operation() {
    child::run(
        concat!(
            module_path!(),
            "::pending_denial_classifier_accepts_only_exact_absent_or_reserved_operation"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                use crate::projection::shared::provider_access::provider_capture::classify_denial_reservations_for_test as classify;
                schema_v2::initialize_reference(c)?;
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let (a, intent, body) = fixture();
                let funding = SourceDeniedCoverageReservation::new(&a, &intent, &body, None)?;
                seed(c, &funding, MAX_METADATA)?;
                let changes = c.total_changes();
                assert!(classify(c, &a, &[], Some(&funding), None)?.is_empty());
                assert!(classify(c, &a, &[], None, None)?.is_empty());
                assert_eq!(changes, c.total_changes());
                funding.reserve(c, None)?;
                let expected = std::collections::BTreeSet::from([funding.operation().to_owned()]);
                let changes = c.total_changes();
                assert_eq!(classify(c, &a, &[], Some(&funding), None)?, expected);
                assert!(classify(c, &a, &[], None, None).is_err(), "strict admission must refuse an unproven reservation");
                assert_eq!(classify(c, &a, std::slice::from_ref(&funding), None, None)?, expected);
                assert_eq!(classify(c, &a, std::slice::from_ref(&funding), Some(&funding), None)?, expected);
                assert_eq!(changes, c.total_changes());

                c.execute_batch("SAVEPOINT unknown_denial")?;
                let unknown = format!("{NAMESPACE}{}", "f".repeat(64));
                assert_ne!(unknown, funding.operation());
                c.execute(
                    "INSERT INTO reservations VALUES(?1,?2,?1,1)",
                    params![unknown, funding.binding().source_owner.as_str()],
                )?;
                let changes = c.total_changes();
                assert!(classify(c, &a, &[], Some(&funding), None).is_err(), "namespace matching must not authorize another operation");
                assert_eq!(changes, c.total_changes());
                c.execute_batch("ROLLBACK TO unknown_denial; RELEASE unknown_denial")?;

                c.execute("UPDATE quota SET bytes=?1", [funding.reservation_bytes() - 1])?;
                let changes = c.total_changes();
                assert!(classify(c, &a, &[], Some(&funding), None).is_err(), "durable replay must recheck total capacity");
                assert_eq!(classify(c, &a, std::slice::from_ref(&funding), None, None)?, expected,
                    "actual document funding remains readable below quota");
                assert_eq!(changes, c.total_changes());
                let cancel = Cancellation::default();
                cancel.cancel();
                assert!(classify(c, &a, &[], Some(&funding), Some(&cancel)).is_err());
                assert_eq!(changes, c.total_changes());
                c.execute_batch("ROLLBACK")?;
                Ok(())
            }).unwrap();
        },
    );
}

#[test]
#[ignore = "unrun owned SQL classifier recipe; prospective owner/family and exact-row fences"]
fn pending_denial_classifier_refuses_foreign_bindings_and_changed_reservation() {
    child::run(
        concat!(
            module_path!(),
            "::pending_denial_classifier_refuses_foreign_bindings_and_changed_reservation"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                use crate::projection::shared::provider_access::provider_capture::classify_denial_reservations_for_test as classify;
                schema_v2::initialize_reference(c)?;
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let (a, intent, body) = fixture();
                let funding = SourceDeniedCoverageReservation::new(&a, &intent, &body, None)?;
                seed(c, &funding, MAX_METADATA)?;
                for field in ["owner", "database", "incarnation", "root", "catalog", "marker"] {
                    // Deliberately contradictory DATA for the classifier seam;
                    // this does not construct an authenticated family holder.
                    let mut other = AllocationRecord::decode(&a.encode().unwrap()).unwrap();
                    match field {
                        "owner" => other.format.identity.worktree_id = WorktreeId::parse("worktree:foreign").unwrap(),
                        "database" => other.private.database_file_identity = "1:0:99".into(),
                        "incarnation" => other.family.incarnation = "foreign".into(),
                        "root" => other.family.root_identity = "fs:v1:0000000000000001:0000000000000099".into(),
                        "catalog" => other.family.catalog_file_identity = "1:0:99".into(),
                        "marker" => other.family.marker_digest = TOKEN.into(),
                        _ => unreachable!(),
                    }
                    let changes = c.total_changes();
                    assert!(classify(c, &other, &[], Some(&funding), None).is_err(), "accepted changed {field}");
                    assert_eq!(changes, c.total_changes());
                }
                for corrupt in ["owner", "operation", "remaining_less", "remaining_more", "negative", "foreign_id"] {
                    c.execute_batch("SAVEPOINT changed_denial")?;
                    let owner = if corrupt == "owner" { "worktree:foreign" } else { funding.binding().source_owner.as_str() };
                    let id = if corrupt == "foreign_id" { "other-id" } else { funding.operation() };
                    let operation = if corrupt == "operation" { "other-operation" } else { funding.operation() };
                    let remaining = match corrupt {
                        "remaining_less" => funding.reservation_bytes() as i64 - 1,
                        "remaining_more" => funding.reservation_bytes() as i64 + 1,
                        "negative" => -1,
                        _ => funding.reservation_bytes() as i64,
                    };
                    if corrupt == "negative" { c.execute_batch("PRAGMA ignore_check_constraints=ON")?; }
                    c.execute("INSERT INTO reservations VALUES(?1,?2,?3,?4)", params![id, owner, operation, remaining])?;
                    let changes = c.total_changes();
                    assert!(classify(c, &a, &[], Some(&funding), None).is_err(), "accepted changed {corrupt}");
                    assert_eq!(changes, c.total_changes());
                    c.execute_batch("ROLLBACK TO changed_denial; RELEASE changed_denial; PRAGMA ignore_check_constraints=OFF")?;
                }
                assert!(classify(c, &a, &[], Some(&funding), None)?.is_empty());
                c.execute_batch("ROLLBACK")?;
                Ok(())
            }).unwrap();
        },
    );
}

#[test]
#[ignore = "unrun owned SQL classifier recipe; actual DATA never replaced by pending hashes"]
fn pending_denial_classifier_refuses_conflicting_actual_plan_or_document() {
    child::run(
        concat!(
            module_path!(),
            "::pending_denial_classifier_refuses_conflicting_actual_plan_or_document"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                use crate::projection::shared::provider_access::provider_capture::classify_denial_reservations_for_test as classify;
                schema_v2::initialize_reference(c)?;
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let (a, intent, body) = fixture();
                let funding = SourceDeniedCoverageReservation::new(&a, &intent, &body, None)?;
                seed(c, &funding, MAX_METADATA)?;
                funding.reserve(c, None)?;
                for document in [false, true] {
                    let mut changed = ProviderTransitionIntentRecord::decode(&intent.encode().unwrap()).unwrap();
                    let mut changed_body = TransitionDeniedCoverageRecord::decode(&body.encode().unwrap()).unwrap();
                    if document {
                        changed_body.observation_snapshot_digest = "f".repeat(64);
                        changed.plan.observation.snapshot_digest = changed_body.observation_snapshot_digest.clone();
                        changed.plan.denial_coverage = Some(changed_body.reference()?);
                    } else {
                        changed.plan.destination_leaf_nonce = "00000000000000000000000000000004".into();
                    }
                    let pending = SourceDeniedCoverageReservation::new(&a, &changed, &changed_body, None)?;
                    assert_ne!(pending.operation(), funding.operation());
                    let changes = c.total_changes();
                    assert!(classify(c, &a, std::slice::from_ref(&funding), Some(&pending), None).is_err());
                    assert!(classify(c, &a, &[], Some(&pending), None).is_err(), "pending must not hide the different durable operation");
                    assert_eq!(changes, c.total_changes());
                    c.execute_batch("SAVEPOINT second_denial")?;
                    pending.reserve(c, None)?;
                    let changes = c.total_changes();
                    assert!(classify(c, &a, std::slice::from_ref(&funding), Some(&pending), None).is_err(), "two exact rows do not reconcile conflicting transaction bindings");
                    assert_eq!(changes, c.total_changes());
                    c.execute_batch("ROLLBACK TO second_denial; RELEASE second_denial")?;
                }
                let changes = c.total_changes();
                assert!(classify(c, &a, &[funding.clone(), funding.clone()], None, None).is_err());
                assert_eq!(changes, c.total_changes());
                c.execute_batch("ROLLBACK")?;
                Ok(())
            }).unwrap();
        },
    );
}
