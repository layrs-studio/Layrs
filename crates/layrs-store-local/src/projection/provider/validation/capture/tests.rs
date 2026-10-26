use super::*;
use crate::projection::{
    RecoveryBinding,
    capture::{CapturedFile, CapturedTarget},
    provider::validation::tests::fixture::Fixture,
    provider::{
        database_io::BareConnection,
        process_profile::tests::child,
        records::{
            MountPhase, PreparedBinding, ProviderBootstrapBinding, ProviderBootstrapSource,
            ProviderSourceDescriptor, Receipt,
            activation::{ProviderActivationRecord, ProviderActiveMountRecord},
            capture::{ProviderCaptureReceiptRecord, ProviderCaptureStage},
            preparation::{PreparationState, ProviderPreparationRecord},
        },
    },
};
use layrs_core::scm::{FileId, RepoPath};
use rusqlite::params;

fn active_fixture(
    c: &Connection,
    writable: bool,
) -> (
    Fixture,
    ProviderPreparationRecord,
    ProviderActiveMountRecord,
    ProviderActivationRecord,
    VersionedProjection,
) {
    let mut f = Fixture::new("G1", false);
    if writable {
        let mut snapshot = f.source.initial().clone();
        snapshot.entries[0].writable = true;
        snapshot.validate().unwrap();
        let descriptors = f
            .source
            .descriptors()
            .iter()
            .map(|descriptor| ProviderSourceDescriptor {
                path: descriptor.path.clone(),
                manifest: descriptor.manifest.clone(),
            })
            .collect();
        let source =
            ProviderBootstrapSource::validate(snapshot, f.source.selection().clone(), descriptors)
                .unwrap();
        f.allocation.format.initial_snapshot_digest = source.commitment().snapshot_digest.clone();
        let format = f.allocation.format.bytes().unwrap();
        f.allocation.format_bytes = format.len() as u64;
        f.allocation.format_file_id = FileId::from_bytes(&format);
        f.allocation.source =
            serde_json::from_slice(&serde_json::to_vec(source.commitment()).unwrap()).unwrap();
        let stage = f.allocation.stage_identity().unwrap().encode();
        let mut control = b"LYRCDIR1".to_vec();
        control.extend_from_slice(&(stage.len() as u32).to_le_bytes());
        control.extend_from_slice(&stage);
        control.extend_from_slice(&(format.len() as u32).to_le_bytes());
        control.extend_from_slice(f.allocation.format_file_id.as_str().as_bytes());
        f.prepared.allocation = f.allocation.data_reference().unwrap();
        f.prepared.control_identity_hex =
            control.iter().map(|byte| format!("{byte:02x}")).collect();
        f.prepared.require_allocation(&f.allocation).unwrap();
        f.source = source;
    }
    let control = f.prepared.control_identity().unwrap();
    let mut digest_input = b"layrs-vfs-control-directory-registration-v2\0".to_vec();
    digest_input.extend_from_slice(&control.encode());
    let digest = FileId::from_bytes(&digest_input);
    let stage = control.stage_identity();
    let root = stage.root().to_str().unwrap().as_bytes();
    let parts: Vec<_> = stage
        .root_filesystem_identity()
        .split(':')
        .map(str::to_owned)
        .collect();
    let volume = u32::from_str_radix(&parts[2], 16).unwrap();
    let index = u64::from_str_radix(&parts[3], 16).unwrap();
    let mut ticket = b"LYRTICK2".to_vec();
    ticket.extend_from_slice(&[0x41; 16]);
    ticket.extend_from_slice(f.source.commitment().vfs_catalog_id.as_str().as_bytes());
    ticket.extend_from_slice(&volume.to_le_bytes());
    ticket.extend_from_slice(&index.to_le_bytes());
    ticket.extend_from_slice(digest.as_str().as_bytes());
    ticket.extend_from_slice(&1u32.to_le_bytes());
    ticket.extend_from_slice(&(root.len() as u32).to_le_bytes());
    ticket.extend_from_slice(root);
    let a = &f.allocation;
    let p = &f.prepared;
    let binding = ProviderBootstrapBinding {
        schema_version: "layrs.projection-provider-bootstrap-binding.v1".into(),
        recovery: RecoveryBinding {
            identity: a.format.identity.clone(),
            checkout_identity: a.format.checkout_identity().into(),
            marker_digest: records::codec::hash(
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
    };
    let mut predecessor = ProviderPreparationRecord {
        schema_version: "layrs.projection-provider-mount.v2".into(),
        bootstrap: f.mount(MountPhase::ControlPublished),
        binding,
        source: serde_json::from_slice(&serde_json::to_vec(f.source.commitment()).unwrap())
            .unwrap(),
        root_id_hex: "41".repeat(16),
        registration_ticket_hex: ticket.iter().map(|byte| format!("{byte:02x}")).collect(),
        state: PreparationState::RegistrationPending,
    };
    let pending_digest = predecessor.digest().unwrap();
    predecessor.state = PreparationState::ProviderPrepared { pending_digest };
    predecessor
        .require_references(a, p, &f.source, [0x41; 16])
        .unwrap();
    f.populate(c, MountPhase::ControlPublished);
    c.execute(
        "UPDATE projection_provider_mounts SET phase=?1,payload=?2,digest=?3",
        params![
            predecessor.sql_phase(),
            predecessor.encode().unwrap(),
            predecessor.digest().unwrap()
        ],
    )
    .unwrap();
    let (activation, snapshot) =
        ProviderActivationRecord::derive(a, p, &predecessor, &f.source).unwrap();
    let payload = snapshot.canonical_bytes().unwrap();
    c.execute(
        "INSERT INTO projection_snapshots VALUES(1,?1,?2,?3)",
        params![
            activation.activated_digest,
            activation.source_digest,
            payload
        ],
    )
    .unwrap();
    c.execute(
        "UPDATE projection_head SET generation=1 WHERE singleton=1",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO projection_provider_activations VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            activation.digest().unwrap(),
            activation.owner_identity_digest,
            activation.mount_id,
            activation.catalog_id,
            activation.source_generation,
            activation.source_digest,
            activation.activated_generation,
            activation.activated_digest,
            activation.encode().unwrap(),
        ],
    )
    .unwrap();
    let mount = ProviderActiveMountRecord::derive(&predecessor, &activation).unwrap();
    c.execute(
        "UPDATE projection_provider_mounts SET phase='active',payload=?1,digest=?2",
        params![mount.encode().unwrap(), mount.digest().unwrap()],
    )
    .unwrap();
    let activated = VersionedProjection {
        generation: 1,
        digest: activation.activated_digest.clone(),
        snapshot,
    };
    (f, predecessor, mount, activation, activated)
}

fn insert_transaction(c: &Connection, tx: &ProviderCaptureTransactionRecord) {
    c.execute(
        "INSERT INTO projection_provider_capture_transactions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            tx.transaction_id, tx.mount_id, tx.capture_generation, tx.capture_digest,
            tx.expected_capture_sequence, tx.expected_capture_digest, tx.sql_state(),
            tx.reservation(), tx.encode().unwrap(), tx.digest().unwrap()
        ],
    ).unwrap();
}

fn update_transaction(c: &Connection, tx: &ProviderCaptureTransactionRecord) {
    assert_eq!(
        c.execute(
            "UPDATE projection_provider_capture_transactions SET state=?1,reservation=?2,payload=?3,record_digest=?4 WHERE id=?5",
            params![
                tx.sql_state(), tx.reservation(), tx.encode().unwrap(), tx.digest().unwrap(),
                tx.transaction_id,
            ],
        ).unwrap(),
        1
    );
}

fn set_patch_digest(patch: &mut CapturedPatch) {
    patch.digest.clear();
    let bytes = canonical_json(patch).unwrap();
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-projection-capture-v1\0");
    hash.update(&bytes);
    patch.digest = format!("blake3:{}", hash.finalize());
}

fn write_accepted(c: &Connection, tx: &ProviderCaptureTransactionRecord) {
    let ProviderCaptureState::Accepted { patch, .. } = &tx.state else {
        panic!("accepted test record required");
    };
    let receipt = ProviderCaptureReceiptRecord::derive(tx).unwrap();
    update_transaction(c, tx);
    c.execute(
        "INSERT INTO projection_captures VALUES(?1,?2,?3,?4) ON CONFLICT(sequence) DO UPDATE SET digest=excluded.digest,projection_generation=excluded.projection_generation,payload=excluded.payload",
        params![patch.sequence, patch.digest, patch.projection_generation, canonical_json(patch).unwrap()],
    ).unwrap();
    c.execute(
        "INSERT INTO projection_provider_capture_receipts VALUES(?1,?2,?3,?4,?5) ON CONFLICT(transaction_id) DO UPDATE SET capture_sequence=excluded.capture_sequence,capture_digest=excluded.capture_digest,payload=excluded.payload,digest=excluded.digest",
        params![
            receipt.transaction_id, receipt.capture_sequence, receipt.capture_digest,
            receipt.encode().unwrap(), receipt.digest().unwrap()
        ],
    ).unwrap();
}

#[test]
#[ignore = "owned audited Windows SQLite process; bounded Active capture SQL interpretation"]
fn active_capture_rows_are_separate_from_initial_active_admission() {
    child::run(
        concat!(
            module_path!(),
            "::active_capture_rows_are_separate_from_initial_active_admission"
        ),
        120,
        || {
            child::declare();
            let database = BareConnection::reference().unwrap();
            let c = database.connection();
            let (f, predecessor, mount, activation, activated) = active_fixture(c, false);
            let clean = private_active_capture_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &predecessor.binding,
                Some([0x41; 16]),
                None,
            )
            .unwrap();
            assert!(clean.capture.transactions.is_empty());
            assert_eq!(clean.active.activated, activated);
            let tx = ProviderCaptureTransactionRecord::begin(
                &f.allocation,
                &mount,
                &activation,
                "attempt-one".into(),
                0,
                None,
            )
            .unwrap();
            insert_transaction(c, &tx);
            assert!(
                private_active_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    Some([0x41; 16]),
                    None
                )
                .is_err()
            );
            let proof = private_active_capture_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &predecessor.binding,
                Some([0x41; 16]),
                None,
            )
            .unwrap();
            assert_eq!(proof.capture.transactions, vec![tx.clone()]);
            c.execute(
                "UPDATE projection_provider_capture_transactions SET record_digest=?1",
                [format!("blake3:{}", "0".repeat(64))],
            )
            .unwrap();
            assert!(
                private_active_capture_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    Some([0x41; 16]),
                    None
                )
                .is_err()
            );
            c.execute(
                "UPDATE projection_provider_capture_transactions SET record_digest=?1",
                [tx.digest().unwrap()],
            )
            .unwrap();
            c.execute("INSERT INTO projection_prejournal VALUES('outside','capture','invalid',x'7b7d','owned')", []).unwrap();
            assert!(
                private_active_capture_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    Some([0x41; 16]),
                    None
                )
                .is_err()
            );
            database.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; canonical accepted capture SQL history"]
fn accepted_capture_requires_exact_writable_history_content_claim_and_receipt() {
    child::run(
        concat!(
            module_path!(),
            "::accepted_capture_requires_exact_writable_history_content_claim_and_receipt"
        ),
        120,
        || {
            child::declare();
            let database = BareConnection::reference().unwrap();
            let c = database.connection();
            let (f, predecessor, mount, activation, activated) = active_fixture(c, true);
            let entry = activated
                .snapshot
                .entries
                .iter()
                .find(|entry| entry.path.as_str() == "README.md")
                .unwrap();
            assert!(entry.writable && entry.state == EntryState::Materialized);
            let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
                panic!("fixture README is not a file");
            };
            let replacement = CapturedFile {
                file_id: FileId::from_bytes(b"accepted provider replacement"),
                executable: !entry.executable,
                bytes: 20,
            };
            let change = CapturedChange {
                path: RepoPath::parse("README.md").unwrap(),
                expected: Some(CapturedTarget {
                    file_id: file_id.clone(),
                    executable: entry.executable,
                }),
                replacement: Some(replacement.clone()),
            };
            let stage = ProviderCaptureStage {
                image_digest: "a".repeat(64),
                image_path_count: 1,
                changes: vec![change.clone()],
                reservation: "owned-reservation-one".into(),
            };
            let mut tx = ProviderCaptureTransactionRecord::begin(
                &f.allocation,
                &mount,
                &activation,
                "attempt-one".into(),
                0,
                None,
            )
            .unwrap();
            tx.state = ProviderCaptureState::Staging {
                stage: stage.clone(),
            };
            insert_transaction(c, &tx);
            let staged = private_active_capture_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &predecessor.binding,
                Some([0x41; 16]),
                None,
            )
            .unwrap();
            assert!(staged.capture.content.is_empty() && staged.capture.claims.is_empty());
            let manifest_id = format!("blake3:{}", "0".repeat(64));
            c.execute(
                "INSERT INTO projection_content VALUES(?1,?2,?3)",
                params![replacement.file_id.as_str(), manifest_id, replacement.bytes],
            )
            .unwrap();
            let mut patch = CapturedPatch {
                schema_version: "layrs.projection-capture.v1".into(),
                sequence: 1,
                previous_patch_digest: None,
                projection_generation: 1,
                projection_digest: activated.digest.clone(),
                identity: f.allocation.format.identity.clone(),
                changes: vec![change],
                digest: String::new(),
            };
            set_patch_digest(&mut patch);
            tx.state = ProviderCaptureState::Accepted {
                stage: stage.clone(),
                patch: patch.clone(),
            };
            write_accepted(c, &tx);
            assert!(
                private_active_capture_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    Some([0x41; 16]),
                    None
                )
                .is_err(),
                "accepted state must have private claim"
            );
            let root = "a".repeat(64);
            c.execute(
                "INSERT INTO projection_claims VALUES(?1,?2,'provider-capture',?3)",
                params![root, root, tx.transaction_id],
            )
            .unwrap();
            let proof = private_active_capture_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &predecessor.binding,
                Some([0x41; 16]),
                None,
            )
            .unwrap();
            assert_eq!(proof.capture.transactions, vec![tx.clone()]);
            assert_eq!(proof.capture.patches, vec![patch.clone()]);
            assert_eq!(
                proof.capture.receipts,
                vec![ProviderCaptureReceiptRecord::derive(&tx).unwrap()]
            );
            assert_eq!(proof.capture.content.len(), 1);
            assert_eq!(proof.capture.claims.len(), 1);
            assert!(
                private_active_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    Some([0x41; 16]),
                    None
                )
                .is_err(),
                "initial Active admission remains strict"
            );

            let stale = ProviderCaptureTransactionRecord::begin(
                &f.allocation,
                &mount,
                &activation,
                "stale-attempt".into(),
                0,
                None,
            )
            .unwrap();
            insert_transaction(c, &stale);
            assert!(
                private_active_capture_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    Some([0x41; 16]),
                    None,
                )
                .is_err(),
                "active attempt cannot claim a historical capture head"
            );
            c.execute(
                "DELETE FROM projection_provider_capture_transactions WHERE id=?1",
                [&stale.transaction_id],
            )
            .unwrap();

            // A sequence gap is refused before any later history is trusted.
            c.execute(
                "INSERT INTO projection_captures VALUES(3,?1,1,?2)",
                params![
                    format!("blake3:{}", "0".repeat(64)),
                    canonical_json(&patch).unwrap()
                ],
            )
            .unwrap();
            assert!(
                private_active_capture_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    Some([0x41; 16]),
                    None
                )
                .is_err()
            );
            c.execute("DELETE FROM projection_captures WHERE sequence=3", [])
                .unwrap();

            // All documents remain canonical and mutually consistent, but the
            // forged expected target is outside the activated base authority.
            let mut wrong = tx.clone();
            if let ProviderCaptureState::Accepted { stage, patch } = &mut wrong.state {
                stage.changes[0].expected.as_mut().unwrap().file_id =
                    FileId::from_bytes(b"wrong expected target");
                patch.changes = stage.changes.clone();
                set_patch_digest(patch);
            }
            write_accepted(c, &wrong);
            assert!(
                private_active_capture_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    Some([0x41; 16]),
                    None
                )
                .is_err()
            );
            write_accepted(c, &tx);

            c.execute(
                "UPDATE projection_provider_capture_receipts SET capture_digest=?1",
                [format!("blake3:{}", "0".repeat(64))],
            )
            .unwrap();
            assert!(
                private_active_capture_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    Some([0x41; 16]),
                    None
                )
                .is_err()
            );
            write_accepted(c, &tx);

            c.execute(
                "INSERT INTO projection_content VALUES('short',?1,1)",
                [format!("blake3:{}", "1".repeat(64))],
            )
            .unwrap();
            assert!(
                private_active_capture_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    Some([0x41; 16]),
                    None
                )
                .is_err()
            );
            c.execute("DELETE FROM projection_content WHERE file_id='short'", [])
                .unwrap();

            let unbound = "b".repeat(64);
            c.execute(
                "INSERT INTO projection_claims VALUES(?1,?2,'provider-capture','unknown-attempt')",
                params![unbound, unbound],
            )
            .unwrap();
            assert!(
                private_active_capture_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    Some([0x41; 16]),
                    None
                )
                .is_err()
            );
            database.finish(Ok(())).unwrap();
        },
    );
}

#[test]
fn capture_change_authority_is_exactly_the_activated_writable_base() {
    let f = Fixture::new("G1", false);
    let mut snapshot = f.source.initial().clone();
    let writable = snapshot
        .entries
        .iter_mut()
        .find(|entry| entry.path.as_str() == "README.md")
        .unwrap();
    writable.writable = true;
    writable.state = EntryState::Materialized;
    snapshot.validate().unwrap();
    let activated = VersionedProjection {
        generation: 1,
        digest: "a".repeat(64),
        snapshot,
    };
    let entry = activated
        .snapshot
        .entries
        .iter()
        .find(|entry| entry.path.as_str() == "README.md")
        .unwrap();
    let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
        panic!("fixture README is not a file");
    };
    assert!(entry.writable);
    let mut change = CapturedChange {
        path: RepoPath::parse("README.md").unwrap(),
        expected: Some(CapturedTarget {
            file_id: file_id.clone(),
            executable: entry.executable,
        }),
        replacement: Some(CapturedFile {
            file_id: FileId::from_bytes(b"provider replacement"),
            executable: !entry.executable,
            bytes: 20,
        }),
    };
    let reserved = activated.snapshot.reserved_paths().unwrap();
    captured_change_authority(std::slice::from_ref(&change), &activated, &reserved, None).unwrap();
    change.path = RepoPath::parse("new.txt").unwrap();
    assert!(
        captured_change_authority(std::slice::from_ref(&change), &activated, &reserved, None)
            .is_err()
    );
    change.path = RepoPath::parse("README.md").unwrap();
    change.expected.as_mut().unwrap().executable = !entry.executable;
    assert!(
        captured_change_authority(std::slice::from_ref(&change), &activated, &reserved, None)
            .is_err()
    );
}
