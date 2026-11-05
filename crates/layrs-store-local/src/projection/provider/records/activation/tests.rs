use super::*;
use crate::projection::{RecoveryBinding, provider::validation::tests::fixture::Fixture};
use layrs_core::{
    projection::{EntryState, PathSemantics, ProjectionEntry, ProjectionSelection},
    scm::{FileId, RepoPath},
};

fn binding(f: &Fixture) -> ProviderBootstrapBinding {
    use super::super::Receipt;
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
        prepared: Some(super::super::PreparedBinding {
            receipt: p.data_reference().unwrap(),
            control_identity_hex: p.control_identity_hex.clone(),
        }),
        abandoned: None,
    }
}

fn ticket(f: &Fixture) -> Vec<u8> {
    let control = f.prepared.control_identity().unwrap();
    let mut digest_input = b"layrs-vfs-control-directory-registration-v2\0".to_vec();
    digest_input.extend_from_slice(&control.encode());
    let digest = FileId::from_bytes(&digest_input);
    let stage = control.stage_identity();
    let root = stage.root().to_str().unwrap().as_bytes();
    let identity = stage.root_filesystem_identity();
    let parts: Vec<_> = identity.split(':').collect();
    let volume = u32::from_str_radix(parts[2], 16).unwrap();
    let root_index = u64::from_str_radix(parts[3], 16).unwrap();
    let mut wire = b"LYRTICK2".to_vec();
    wire.extend_from_slice(&[0x31; 16]);
    wire.extend_from_slice(f.source.commitment().vfs_catalog_id.as_str().as_bytes());
    wire.extend_from_slice(&volume.to_le_bytes());
    wire.extend_from_slice(&root_index.to_le_bytes());
    wire.extend_from_slice(digest.as_str().as_bytes());
    wire.extend_from_slice(&1u32.to_le_bytes());
    wire.extend_from_slice(&(root.len() as u32).to_le_bytes());
    wire.extend_from_slice(root);
    let parsed = layrs_vfs::Registration::decode_control_directory(&wire).unwrap();
    parsed.matches_control_identity(&control).unwrap();
    wire
}

fn prepared(f: &Fixture) -> ProviderPreparationRecord {
    let mut record = ProviderPreparationRecord {
        schema_version: "layrs.projection-provider-mount.v2".into(),
        bootstrap: f.mount(super::super::MountPhase::ControlPublished),
        binding: binding(f),
        source: serde_json::from_slice(&serde_json::to_vec(f.source.commitment()).unwrap())
            .unwrap(),
        root_id_hex: "31".repeat(16),
        registration_ticket_hex: codec::encode_hex(&ticket(f)),
        state: PreparationState::RegistrationPending,
    };
    let pending_digest = record.digest().unwrap();
    record.state = PreparationState::ProviderPrepared { pending_digest };
    record
        .require_references(&f.allocation, &f.prepared, &f.source, [0x31; 16])
        .unwrap();
    record
}

fn copy<T: Record>(value: &T) -> T {
    T::decode(&value.encode().unwrap()).unwrap()
}

#[test]
fn activation_changes_only_selected_placeholder_state_and_has_exact_rows() {
    let f = Fixture::new("G1", false);
    let predecessor = prepared(&f);
    let (map, after) =
        ProviderActivationRecord::derive(&f.allocation, &f.prepared, &predecessor, &f.source)
            .unwrap();
    let before = f.source.initial();
    assert_eq!(before.entries.len(), 1);
    assert_eq!(before.entries[0].state, EntryState::Unavailable);
    assert_eq!(after.entries[0].state, EntryState::Materialized);
    let mut expected = after.clone();
    expected.entries[0].state = EntryState::Unavailable;
    assert_eq!(&expected, before);
    assert_eq!(map.source_generation, 0);
    assert_eq!(map.activated_generation, 1);
    assert_eq!(
        map.activated_digest,
        database::digest(
            1,
            Some(&map.source_digest),
            &after.canonical_bytes().unwrap()
        )
    );
    map.require_references(&f.allocation, &f.prepared, &predecessor, &f.source, &after)
        .unwrap();
    assert_eq!(
        ProviderActivationRecord::decode(&map.encode().unwrap()).unwrap(),
        map
    );
    let payload = map.encode().unwrap();
    let id = map.digest().unwrap();
    map.require_row(ActivationRowData {
        id: &id,
        owner_identity_digest: &map.owner_identity_digest,
        mount_id: &map.mount_id,
        catalog_id: &map.catalog_id,
        source_generation: 0,
        source_digest: &map.source_digest,
        activated_generation: 1,
        activated_digest: &map.activated_digest,
        payload: &payload,
    })
    .unwrap();
    let mount = ProviderActiveMountRecord::derive(&predecessor, &map).unwrap();
    assert_eq!(mount.sql_phase(), "active");
    mount
        .require_references(&f.allocation, &f.prepared, &f.source, &map, &after)
        .unwrap();
    let old_payload = predecessor.encode().unwrap();
    let old_digest = predecessor.digest().unwrap();
    mount
        .require_predecessor_row(MountRowData {
            id: &map.mount_id,
            catalog_id: &map.catalog_id,
            phase: "provider_prepared",
            payload: &old_payload,
            digest: &old_digest,
        })
        .unwrap();
    let new_payload = mount.encode().unwrap();
    let new_digest = mount.digest().unwrap();
    mount
        .require_row(MountRowData {
            id: &map.mount_id,
            catalog_id: &map.catalog_id,
            phase: "active",
            payload: &new_payload,
            digest: &new_digest,
        })
        .unwrap();
    assert_eq!(
        ProviderActiveMountRecord::decode(&new_payload).unwrap(),
        mount
    );
    assert!(
        mount
            .require_row(MountRowData {
                id: &map.mount_id,
                catalog_id: &map.catalog_id,
                phase: "provider_prepared",
                payload: &new_payload,
                digest: &new_digest,
            })
            .is_err()
    );
}

#[test]
fn explicit_absence_and_unselected_cold_file_are_preserved() {
    let f = Fixture::new("G1", false);
    let mut initial = f.source.initial().clone();
    initial.entries.push(ProjectionEntry {
        path: RepoPath::parse("UNSELECTED.md").unwrap(),
        target: initial.entries[0].target.clone(),
        executable: false,
        writable: false,
        state: EntryState::Unavailable,
    });
    initial.entries.push(ProjectionEntry {
        path: RepoPath::parse("Z-absent.md").unwrap(),
        target: None,
        executable: false,
        writable: false,
        state: EntryState::Absent,
    });
    let selection = ProjectionSelection::selected(
        vec![
            RepoPath::parse("README.md").unwrap(),
            RepoPath::parse("Z-absent.md").unwrap(),
        ],
        vec![],
        PathSemantics::PortableWindows,
    )
    .unwrap();
    let source = ProviderBootstrapSource::validate(
        initial.clone(),
        selection,
        vec![super::super::ProviderSourceDescriptor {
            path: RepoPath::parse("README.md").unwrap(),
            manifest: f.source.descriptors()[0].manifest.clone(),
        }],
    )
    .unwrap();
    let after = activation_snapshot(&source).unwrap();
    assert_eq!(after.entries[0].state, EntryState::Materialized);
    assert_eq!(after.entries[1], initial.entries[1]);
    assert_eq!(after.entries[2], initial.entries[2]);
    assert_eq!(after.base_step_id, initial.base_step_id);
    assert_eq!(after.base_root_tree_id, initial.base_root_tree_id);
    assert_eq!(
        after.observed_authorization_epoch,
        initial.observed_authorization_epoch
    );
    assert_eq!(after.observed_policy_set_id, initial.observed_policy_set_id);
}

#[test]
fn map_and_v3_reject_cross_owner_generation_selection_and_observation_changes() {
    let f = Fixture::new("G1", false);
    let other = Fixture::new("G2", false);
    let predecessor = prepared(&f);
    let (map, after) =
        ProviderActivationRecord::derive(&f.allocation, &f.prepared, &predecessor, &f.source)
            .unwrap();
    assert_ne!(
        owner_identity_digest(&f.allocation).unwrap(),
        owner_identity_digest(&other.allocation).unwrap()
    );
    assert!(
        map.require_references(
            &other.allocation,
            &other.prepared,
            &predecessor,
            &other.source,
            &after
        )
        .is_err()
    );
    let mut wrong = copy(&map);
    wrong.owner_identity_digest = codec::hash(OWNER_DOMAIN, b"other owner");
    wrong.validate_fields().unwrap();
    assert!(
        wrong
            .require_references(&f.allocation, &f.prepared, &predecessor, &f.source, &after)
            .is_err()
    );
    wrong = copy(&map);
    wrong.activated_generation = 2;
    assert!(wrong.encode().is_err());
    wrong = copy(&map);
    wrong.selection_digest = "0".repeat(64);
    assert!(wrong.encode().is_err());
    let mut observed = after.clone();
    observed.observed_authorization_epoch += 1;
    assert!(
        map.require_references(
            &f.allocation,
            &f.prepared,
            &predecessor,
            &f.source,
            &observed
        )
        .is_err()
    );
    observed = after.clone();
    observed.entries[0].writable = !observed.entries[0].writable;
    assert!(
        map.require_references(
            &f.allocation,
            &f.prepared,
            &predecessor,
            &f.source,
            &observed
        )
        .is_err()
    );
    let mut mount = ProviderActiveMountRecord::derive(&predecessor, &map).unwrap();
    mount.activation_digest = codec::hash(ACTIVE_MOUNT_DOMAIN, b"another map");
    assert!(
        mount
            .require_references(&f.allocation, &f.prepared, &f.source, &map, &after)
            .is_err()
    );
}
