//! Complete synthetic Active DATA, reconstructed before populating SQL so the
//! family marker's real canonical digest is bound by every source reference.
use super::*;
use crate::projection::provider::records::{
    activation::{ProviderActivationRecord, ProviderActiveMountRecord},
    preparation::{PreparationState, ProviderPreparationRecord},
    MountPhase, PreparedBinding, ProviderBootstrapBinding, ProviderSourceDescriptor,
};
use crate::projection::{RecoveryBinding, VersionedProjection};

pub(super) fn active(
    c: &Connection,
) -> (
    SourceFixture,
    ProviderActiveMountRecord,
    ProviderActivationRecord,
    VersionedProjection,
    Vec<u8>,
) {
    let mut f = SourceFixture::new("G1", false);
    let mut initial = f.source.initial().clone();
    initial.entries[0].writable = true;
    let descriptors = f
        .source
        .descriptors()
        .iter()
        .map(|d| ProviderSourceDescriptor {
            path: d.path.clone(),
            manifest: d.manifest.clone(),
        })
        .collect();
    f.source =
        ProviderBootstrapSource::validate(initial, f.source.selection().clone(), descriptors)
            .unwrap();
    f.allocation.source =
        serde_json::from_value(serde_json::to_value(f.source.commitment()).unwrap()).unwrap();
    f.allocation.format.initial_snapshot_digest = f.source.commitment().snapshot_digest.clone();
    let identity = &f.allocation.format.identity;
    let marker = family::FamilyMarker {
        schema_version: "layrs.partial-store-family.v1".into(),
        magic: "LAYRS_NATIVE_STORE".into(),
        format_version: 4,
        min_reader_version: 4,
        incarnation: f.allocation.family.incarnation.clone(),
        identity: crate::projection::shared::PartialStoreIdentity {
            store_id: identity.store_id.clone(),
            space_id: identity.space_id.clone(),
            origin: identity.origin.clone(),
            principal_id: identity.principal_id.clone(),
        },
        root_identity: f.allocation.family.root_identity.clone(),
        metadata_identity: f.allocation.family.metadata_identity.clone(),
        database_identity: f.allocation.family.catalog_file_identity.clone(),
    };
    let family_marker = canonical_json(&marker).unwrap();
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-projection-recovery-family-v1\0");
    hash.update(&family_marker);
    f.allocation.family.marker_digest = format!("blake3:{}", hash.finalize());
    let format = f.allocation.format.bytes().unwrap();
    f.allocation.format_bytes = format.len() as u64;
    f.allocation.format_file_id = FileId::from_bytes(&format);
    let stage = f.allocation.stage_identity().unwrap().encode();
    let mut control = b"LYRCDIR1".to_vec();
    control.extend_from_slice(&(stage.len() as u32).to_le_bytes());
    control.extend_from_slice(&stage);
    control.extend_from_slice(&(format.len() as u32).to_le_bytes());
    control.extend_from_slice(f.allocation.format_file_id.as_str().as_bytes());
    f.prepared.allocation = f.allocation.data_reference().unwrap();
    f.prepared.control_identity_hex = hex(&control);
    f.prepared.require_allocation(&f.allocation).unwrap();

    let control = f.prepared.control_identity().unwrap();
    let mut digest_input = b"layrs-vfs-control-directory-registration-v2\0".to_vec();
    digest_input.extend_from_slice(&control.encode());
    let stage = control.stage_identity();
    let parts: Vec<_> = stage
        .root_filesystem_identity()
        .split(':')
        .map(str::to_owned)
        .collect();
    let root = stage.root().to_str().unwrap().as_bytes();
    let mut ticket = b"LYRTICK2".to_vec();
    ticket.extend_from_slice(&[0x41; 16]);
    ticket.extend_from_slice(f.source.commitment().vfs_catalog_id.as_str().as_bytes());
    ticket.extend_from_slice(&u32::from_str_radix(&parts[2], 16).unwrap().to_le_bytes());
    ticket.extend_from_slice(&u64::from_str_radix(&parts[3], 16).unwrap().to_le_bytes());
    ticket.extend_from_slice(FileId::from_bytes(&digest_input).as_str().as_bytes());
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
            marker_digest: crate::projection::provider::records::codec::hash(
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
        source: serde_json::from_value(serde_json::to_value(f.source.commitment()).unwrap())
            .unwrap(),
        root_id_hex: "41".repeat(16),
        registration_ticket_hex: hex(&ticket),
        state: PreparationState::RegistrationPending,
    };
    predecessor.state = PreparationState::ProviderPrepared {
        pending_digest: predecessor.digest().unwrap(),
    };
    predecessor
        .require_references(a, p, &f.source, [0x41; 16])
        .unwrap();
    f.populate(c, MountPhase::ControlPublished);
    let (activation, snapshot) =
        ProviderActivationRecord::derive(a, p, &predecessor, &f.source).unwrap();
    c.execute(
        "INSERT INTO projection_snapshots VALUES(1,?1,?2,?3)",
        params![
            activation.activated_digest,
            activation.source_digest,
            snapshot.canonical_bytes().unwrap()
        ],
    )
    .unwrap();
    c.execute("UPDATE projection_head SET generation=1", [])
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
            activation.encode().unwrap()
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
    (f, mount, activation, activated, family_marker)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
