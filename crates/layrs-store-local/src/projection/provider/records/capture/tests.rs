use super::*;
use crate::projection::{
    RecoveryBinding,
    capture::{CapturedFile, CapturedTarget},
    provider::{
        records::{
            MountPhase, PreparedBinding, ProviderBootstrapBinding, Receipt,
            preparation::{PreparationState, ProviderPreparationRecord},
        },
        validation::tests::fixture::Fixture,
    },
};
use layrs_core::scm::{FileId, RepoPath, TreeEntryTarget};

fn binding(f: &Fixture) -> ProviderBootstrapBinding {
    let a = &f.allocation;
    let p = &f.prepared;
    ProviderBootstrapBinding {
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
        prepared: Some(PreparedBinding {
            receipt: p.data_reference().unwrap(),
            control_identity_hex: p.control_identity_hex.clone(),
        }),
        abandoned: None,
    }
}

fn prepared(f: &Fixture) -> ProviderPreparationRecord {
    let control = f.prepared.control_identity().unwrap();
    let mut digest_input = b"layrs-vfs-control-directory-registration-v2\0".to_vec();
    digest_input.extend_from_slice(&control.encode());
    let digest = FileId::from_bytes(&digest_input);
    let stage = control.stage_identity();
    let root = stage.root().to_str().unwrap().as_bytes();
    let root_identity = stage.root_filesystem_identity();
    let parts: Vec<_> = root_identity.split(':').collect();
    let volume = u32::from_str_radix(parts[2], 16).unwrap();
    let root_index = u64::from_str_radix(parts[3], 16).unwrap();
    let mut ticket = b"LYRTICK2".to_vec();
    ticket.extend_from_slice(&[0x31; 16]);
    ticket.extend_from_slice(f.source.commitment().vfs_catalog_id.as_str().as_bytes());
    ticket.extend_from_slice(&volume.to_le_bytes());
    ticket.extend_from_slice(&root_index.to_le_bytes());
    ticket.extend_from_slice(digest.as_str().as_bytes());
    ticket.extend_from_slice(&1u32.to_le_bytes());
    ticket.extend_from_slice(&(root.len() as u32).to_le_bytes());
    ticket.extend_from_slice(root);
    let mut record = ProviderPreparationRecord {
        schema_version: "layrs.projection-provider-mount.v2".into(),
        bootstrap: f.mount(MountPhase::ControlPublished),
        binding: binding(f),
        source: serde_json::from_slice(&serde_json::to_vec(f.source.commitment()).unwrap())
            .unwrap(),
        root_id_hex: "31".repeat(16),
        registration_ticket_hex: codec::encode_hex(&ticket),
        state: PreparationState::RegistrationPending,
    };
    let pending_digest = record.digest().unwrap();
    record.state = PreparationState::ProviderPrepared { pending_digest };
    record
        .require_references(&f.allocation, &f.prepared, &f.source, [0x31; 16])
        .unwrap();
    record
}

fn base() -> (Fixture, ProviderActiveMountRecord, ProviderActivationRecord) {
    let f = Fixture::new("G1", false);
    let prepared = prepared(&f);
    let (activation, after) =
        ProviderActivationRecord::derive(&f.allocation, &f.prepared, &prepared, &f.source).unwrap();
    let mount = ProviderActiveMountRecord::derive(&prepared, &activation).unwrap();
    mount
        .require_references(&f.allocation, &f.prepared, &f.source, &activation, &after)
        .unwrap();
    (f, mount, activation)
}

fn change(f: &Fixture) -> CapturedChange {
    let entry = &f.source.initial().entries[0];
    let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
        panic!("G1 has no file")
    };
    CapturedChange {
        path: RepoPath::parse("README.md").unwrap(),
        expected: Some(CapturedTarget {
            file_id: file_id.clone(),
            executable: entry.executable,
        }),
        replacement: Some(CapturedFile {
            file_id: FileId::from_bytes(b"provider-capture-replacement"),
            executable: !entry.executable,
            bytes: 13,
        }),
    }
}

fn stage(f: &Fixture) -> ProviderCaptureStage {
    ProviderCaptureStage {
        image_digest: "a".repeat(64),
        image_path_count: 1,
        changes: vec![change(f)],
        reservation: "reservation-one".into(),
    }
}

fn patch(
    f: &Fixture,
    activation: &ProviderActivationRecord,
    changes: Vec<CapturedChange>,
) -> CapturedPatch {
    let mut patch = CapturedPatch {
        schema_version: "layrs.projection-capture.v1".into(),
        sequence: 1,
        previous_patch_digest: None,
        projection_generation: 1,
        projection_digest: activation.activated_digest.clone(),
        identity: f.allocation.format.identity.clone(),
        changes,
        digest: String::new(),
    };
    patch.digest = patch_digest(&patch).unwrap();
    patch
}

fn row<'a>(
    record: &'a ProviderCaptureTransactionRecord,
    payload: &'a [u8],
    digest: &'a str,
) -> ProviderCaptureTransactionRowData<'a> {
    ProviderCaptureTransactionRowData {
        id: &record.transaction_id,
        mount_id: &record.mount_id,
        capture_generation: record.capture_generation,
        digest: &record.capture_digest,
        expected_capture_sequence: record.expected_capture_sequence,
        expected_capture_digest: record.expected_capture_digest.as_deref(),
        state: record.sql_state(),
        reservation: record.reservation(),
        payload,
        record_digest: digest,
    }
}

#[test]
fn complete_stage_patch_and_receipt_are_canonical_and_exact() {
    let (f, mount, activation) = base();
    let hashing = ProviderCaptureTransactionRecord::begin(
        &f.allocation,
        &mount,
        &activation,
        "attempt-one".into(),
        0,
        None,
    )
    .unwrap();
    let image = stage(&f);
    let mut staging = hashing.clone();
    staging.state = ProviderCaptureState::Staging {
        stage: image.clone(),
    };
    staging.require_transition(&hashing).unwrap();
    let mut accepted = staging.clone();
    accepted.state = ProviderCaptureState::Accepted {
        stage: image.clone(),
        patch: patch(&f, &activation, image.changes.clone()),
    };
    accepted.require_transition(&staging).unwrap();
    accepted
        .require_references(&f.allocation, &mount, &activation)
        .unwrap();
    let payload = accepted.encode().unwrap();
    let digest = accepted.digest().unwrap();
    assert_eq!(
        ProviderCaptureTransactionRecord::decode(&payload).unwrap(),
        accepted
    );
    accepted
        .require_row(row(&accepted, &payload, &digest))
        .unwrap();
    let receipt = ProviderCaptureReceiptRecord::derive(&accepted).unwrap();
    receipt.require_references(&accepted).unwrap();
    let receipt_payload = receipt.encode().unwrap();
    let receipt_digest = receipt.digest().unwrap();
    receipt
        .require_row(ProviderCaptureReceiptRowData {
            transaction_id: &receipt.transaction_id,
            capture_sequence: receipt.capture_sequence,
            capture_digest: &receipt.capture_digest,
            payload: &receipt_payload,
            digest: &receipt_digest,
        })
        .unwrap();
    assert_eq!(
        ProviderCaptureReceiptRecord::decode(&receipt_payload).unwrap(),
        receipt
    );
    let mut substituted = accepted.clone();
    substituted.state = ProviderCaptureState::Accepted {
        stage: image,
        patch: patch(&f, &activation, vec![]),
    };
    assert!(substituted.encode().is_err());
}

#[test]
fn immutable_attempt_and_terminal_transitions_refuse_substitution() {
    let (f, mount, activation) = base();
    let hashing = ProviderCaptureTransactionRecord::begin(
        &f.allocation,
        &mount,
        &activation,
        "attempt-one".into(),
        0,
        None,
    )
    .unwrap();
    let image = stage(&f);
    let mut staging = hashing.clone();
    staging.state = ProviderCaptureState::Staging {
        stage: image.clone(),
    };
    let mut accepted = staging.clone();
    accepted.state = ProviderCaptureState::Accepted {
        stage: image.clone(),
        patch: patch(&f, &activation, image.changes.clone()),
    };
    accepted.require_transition(&staging).unwrap();
    let mut changed = accepted.clone();
    changed.mount_digest = codec::hash("other-mount", b"other");
    assert!(changed.require_transition(&staging).is_err());
    changed = accepted.clone();
    changed.transaction_id = "another-attempt".into();
    assert!(changed.require_transition(&staging).is_err());
    changed = accepted.clone();
    if let ProviderCaptureState::Accepted { stage, .. } = &mut changed.state {
        stage.reservation = "another-reservation".into();
    }
    assert!(changed.require_transition(&staging).is_err());
    assert!(hashing.require_transition(&accepted).is_err());
    assert!(staging.require_transition(&accepted).is_err());
    let mut cancelled = staging.clone();
    cancelled.state = ProviderCaptureState::Cancelled {
        stage: Some(image.clone()),
    };
    cancelled.require_transition(&staging).unwrap();
    assert_eq!(cancelled.reservation(), Some("reservation-one"));
    assert!(accepted.require_transition(&cancelled).is_err());
    let mut early_cancelled = hashing.clone();
    early_cancelled.state = ProviderCaptureState::Cancelled { stage: None };
    early_cancelled.require_transition(&hashing).unwrap();
    assert!(early_cancelled.require_transition(&staging).is_err());
}

#[test]
fn patch_history_mode_and_bounds_refuse_alternate_or_truncated_data() {
    let (f, mount, activation) = base();
    let hashing = ProviderCaptureTransactionRecord::begin(
        &f.allocation,
        &mount,
        &activation,
        "attempt-one".into(),
        0,
        None,
    )
    .unwrap();
    let image = stage(&f);
    let mut accepted = hashing.clone();
    accepted.state = ProviderCaptureState::Accepted {
        stage: image.clone(),
        patch: patch(&f, &activation, image.changes.clone()),
    };
    assert!(accepted.require_transition(&hashing).is_err());
    if let ProviderCaptureState::Accepted { patch, .. } = &mut accepted.state {
        patch.changes[0].expected.as_mut().unwrap().executable ^= true;
    }
    assert!(accepted.encode().is_err());
    let mut staged = hashing.clone();
    let mut too_many = image.clone();
    too_many.image_path_count = 100_001;
    staged.state = ProviderCaptureState::Staging { stage: too_many };
    assert!(staged.encode().is_err());
    let mut oversized = image.clone();
    oversized.changes = vec![image.changes[0].clone(); 1025];
    staged.state = ProviderCaptureState::Staging { stage: oversized };
    assert!(staged.encode().is_err());
    let mut wrong_head = hashing.clone();
    wrong_head.expected_capture_sequence = 1;
    assert!(wrong_head.encode().is_err());
    let mut wrong_map = hashing.clone();
    wrong_map.activation_digest = codec::hash("other-map", b"other");
    assert!(
        wrong_map
            .require_references(&f.allocation, &mount, &activation)
            .is_err()
    );
    let receipt = ProviderCaptureReceiptRecord::derive(&{
        let mut valid = hashing;
        valid.state = ProviderCaptureState::Accepted {
            stage: image.clone(),
            patch: patch(&f, &activation, image.changes),
        };
        valid
    })
    .unwrap();
    let mut altered = receipt.clone();
    altered.capture_digest = codec::hash("other-capture", b"other");
    assert!(
        altered
            .require_row(ProviderCaptureReceiptRowData {
                transaction_id: &receipt.transaction_id,
                capture_sequence: receipt.capture_sequence,
                capture_digest: &receipt.capture_digest,
                payload: &receipt.encode().unwrap(),
                digest: &receipt.digest().unwrap(),
            })
            .is_err()
    );
}

#[test]
fn copied_owner_digest_cannot_rebind_another_allocation_mount() {
    let (a, _, _) = base();
    let (b, mut b_activation) = {
        let b = Fixture::new("G2", false);
        let predecessor = prepared(&b);
        let (activation, _) =
            ProviderActivationRecord::derive(&b.allocation, &b.prepared, &predecessor, &b.source)
                .unwrap();
        (b, activation)
    };
    assert_ne!(
        a.allocation.data_reference().unwrap(),
        b.allocation.data_reference().unwrap()
    );
    b_activation.owner_identity_digest = owner_identity_digest(&a.allocation).unwrap();
    let b_mount = ProviderActiveMountRecord::derive(&prepared(&b), &b_activation).unwrap();
    assert!(
        ProviderCaptureTransactionRecord::begin(
            &a.allocation,
            &b_mount,
            &b_activation,
            "attempt-mixed".into(),
            0,
            None,
        )
        .is_err()
    );
}

#[test]
fn nonzero_head_allows_only_exact_dedup_or_next_ancestor() {
    let (f, mount, activation) = base();
    let image = stage(&f);
    let mut existing = patch(&f, &activation, image.changes.clone());
    existing.sequence = 7;
    existing.previous_patch_digest = Some(codec::hash("prior-capture", b"six"));
    existing.digest = patch_digest(&existing).unwrap();
    let hashing = ProviderCaptureTransactionRecord::begin(
        &f.allocation,
        &mount,
        &activation,
        "attempt-nonzero".into(),
        7,
        Some(existing.digest.clone()),
    )
    .unwrap();
    let mut dedup = hashing.clone();
    dedup.state = ProviderCaptureState::Accepted {
        stage: image.clone(),
        patch: existing.clone(),
    };
    dedup
        .require_references(&f.allocation, &mount, &activation)
        .unwrap();
    let receipt = ProviderCaptureReceiptRecord::derive(&dedup).unwrap();
    assert_eq!(receipt.capture_sequence, 7);
    assert_eq!(receipt.capture_digest, existing.digest);

    let mut next_patch = patch(&f, &activation, image.changes.clone());
    next_patch.sequence = 8;
    next_patch.previous_patch_digest = Some(existing.digest.clone());
    next_patch.digest = patch_digest(&next_patch).unwrap();
    let mut next = hashing.clone();
    next.state = ProviderCaptureState::Accepted {
        stage: image.clone(),
        patch: next_patch.clone(),
    };
    next.require_references(&f.allocation, &mount, &activation)
        .unwrap();
    let mut gap = next.clone();
    if let ProviderCaptureState::Accepted { patch, .. } = &mut gap.state {
        patch.sequence = 9;
        patch.digest = patch_digest(patch).unwrap();
    }
    assert!(gap.encode().is_err());
    let mut wrong_ancestor = next;
    if let ProviderCaptureState::Accepted { patch, .. } = &mut wrong_ancestor.state {
        patch.previous_patch_digest = Some(codec::hash("unrelated-capture", b"other"));
        patch.digest = patch_digest(patch).unwrap();
    }
    assert!(wrong_ancestor.encode().is_err());
}
