//! Owned in-memory schema8 interpretation of the exact Active transition.
use super::*;
use crate::projection::{
    RecoveryBinding,
    provider::{database_io::BareConnection, process_profile::tests::child, records::Receipt},
};
use layrs_core::scm::FileId;
use rusqlite::params;

fn binding(f: &tests::fixture::Fixture) -> ProviderBootstrapBinding {
    let a = &f.allocation;
    let p = &f.prepared;
    ProviderBootstrapBinding {
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
        prepared: Some(records::PreparedBinding {
            receipt: p.data_reference().unwrap(),
            control_identity_hex: p.control_identity_hex.clone(),
        }),
        abandoned: None,
    }
}

fn ticket(f: &tests::fixture::Fixture) -> Vec<u8> {
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
    let mut wire = b"LYRTICK2".to_vec();
    wire.extend_from_slice(&[0x41; 16]);
    wire.extend_from_slice(f.source.commitment().vfs_catalog_id.as_str().as_bytes());
    wire.extend_from_slice(&volume.to_le_bytes());
    wire.extend_from_slice(&index.to_le_bytes());
    wire.extend_from_slice(digest.as_str().as_bytes());
    wire.extend_from_slice(&1u32.to_le_bytes());
    wire.extend_from_slice(&(root.len() as u32).to_le_bytes());
    wire.extend_from_slice(root);
    layrs_vfs::Registration::decode_control_directory(&wire)
        .unwrap()
        .matches_control_identity(&control)
        .unwrap();
    wire
}

fn case() -> (tests::fixture::Fixture, ProviderPreparationRecord) {
    let f = tests::fixture::Fixture::new("G1", false);
    let mut record = ProviderPreparationRecord {
        schema_version: "layrs.projection-provider-mount.v2".into(),
        bootstrap: f.mount(MountPhase::ControlPublished),
        binding: binding(&f),
        source: serde_json::from_slice(&serde_json::to_vec(f.source.commitment()).unwrap())
            .unwrap(),
        root_id_hex: "41".repeat(16),
        registration_ticket_hex: ticket(&f)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        state: records::preparation::PreparationState::RegistrationPending,
    };
    let pending_digest = record.digest().unwrap();
    record.state = records::preparation::PreparationState::ProviderPrepared { pending_digest };
    record
        .require_references(&f.allocation, &f.prepared, &f.source, [0x41; 16])
        .unwrap();
    (f, record)
}

fn install(
    c: &Connection,
    f: &tests::fixture::Fixture,
    predecessor: &ProviderPreparationRecord,
) -> (
    ProviderActiveMountRecord,
    ProviderActivationRecord,
    VersionedProjection,
) {
    c.execute(
        "UPDATE projection_provider_mounts SET phase=?1,payload=?2,digest=?3",
        params![
            predecessor.sql_phase(),
            predecessor.encode().unwrap(),
            predecessor.digest().unwrap()
        ],
    )
    .unwrap();
    let (map, after) =
        ProviderActivationRecord::derive(&f.allocation, &f.prepared, predecessor, &f.source)
            .unwrap();
    let payload = after.canonical_bytes().unwrap();
    c.execute(
        "INSERT INTO projection_snapshots VALUES(1,?1,?2,?3)",
        params![map.activated_digest, map.source_digest, payload],
    )
    .unwrap();
    assert_eq!(
        c.execute(
            "UPDATE projection_head SET generation=1 WHERE singleton=1",
            []
        )
        .unwrap(),
        1
    );
    c.execute(
        "INSERT INTO projection_provider_activations VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            map.digest().unwrap(),
            map.owner_identity_digest,
            map.mount_id,
            map.catalog_id,
            map.source_generation,
            map.source_digest,
            map.activated_generation,
            map.activated_digest,
            map.encode().unwrap()
        ],
    )
    .unwrap();
    let mount = ProviderActiveMountRecord::derive(predecessor, &map).unwrap();
    assert_eq!(
        c.execute(
            "UPDATE projection_provider_mounts SET phase='active',payload=?1,digest=?2",
            params![mount.encode().unwrap(), mount.digest().unwrap()],
        )
        .unwrap(),
        1
    );
    (
        mount,
        map,
        VersionedProjection {
            generation: 1,
            digest: database::digest(1, Some(&predecessor.source.snapshot_digest), &payload),
            snapshot: after,
        },
    )
}

fn check(
    c: &Connection,
    f: &tests::fixture::Fixture,
    predecessor: &ProviderPreparationRecord,
    mount: &ProviderActiveMountRecord,
    map: &ProviderActivationRecord,
    activated: &VersionedProjection,
) {
    let result = private_active_with_source(
        c,
        &f.allocation,
        &f.prepared,
        &predecessor.binding,
        Some([0x41; 16]),
        None,
    )
    .unwrap();
    assert_eq!(&result.mount, mount);
    assert_eq!(&result.activation, map);
    assert_eq!(result.source.commitment(), f.source.commitment());
    assert_eq!(&result.activated, activated);
    let derived = private_active_with_source(
        c,
        &f.allocation,
        &f.prepared,
        &predecessor.binding,
        None,
        None,
    )
    .unwrap();
    assert_eq!(derived.mount, *mount);
    assert!(matches!(
        private_activation_state_with_source(
            c, &f.allocation, &f.prepared, &predecessor.binding, [0x41; 16]
        ).unwrap(),
        PrivateActivationStateValidation::Active(active) if active.mount == *mount
    ));
}

#[test]
#[ignore = "owned audited Windows SQLite process; full Active schema8 interpretation"]
fn exact_active_chain_and_v1_v2_mutual_refusal() {
    child::run(
        concat!(
            module_path!(),
            "::exact_active_chain_and_v1_v2_mutual_refusal"
        ),
        120,
        || {
            child::declare();
            let (f, predecessor) = case();
            let database = BareConnection::reference().unwrap();
            let c = database.connection();
            f.populate(c, MountPhase::ControlPublished);
            assert!(
                private_active_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    None,
                    None
                )
                .is_err()
            );
            assert!(
                private_activation_state_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    [0x41; 16]
                )
                .is_err()
            );
            c.execute(
                "UPDATE projection_provider_mounts SET phase=?1,payload=?2,digest=?3",
                params![
                    predecessor.sql_phase(),
                    predecessor.encode().unwrap(),
                    predecessor.digest().unwrap()
                ],
            )
            .unwrap();
            assert!(
                private_active_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    None,
                    None
                )
                .is_err()
            );
            assert!(matches!(
                private_activation_state_with_source(
                    c, &f.allocation, &f.prepared, &predecessor.binding, [0x41; 16]
                ).unwrap(),
                PrivateActivationStateValidation::Prepared(ready) if ready.mount == predecessor
            ));
            let (mount, map, activated) = install(c, &f, &predecessor);
            check(c, &f, &predecessor, &mount, &map, &activated);
            assert!(private(c, &f.allocation, Some(&f.prepared)).is_err());
            assert!(
                private_preparation_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    [0x41; 16]
                )
                .is_err()
            );
            assert!(
                private_active_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    Some([0x42; 16]),
                    None
                )
                .is_err()
            );
            database.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; Active row and generation contradictions"]
fn active_map_generation_head_owner_phase_bounds_and_extra_rows_refuse() {
    child::run(
        concat!(
            module_path!(),
            "::active_map_generation_head_owner_phase_bounds_and_extra_rows_refuse"
        ),
        120,
        || {
            child::declare();
            let (f, predecessor) = case();
            let database = BareConnection::reference().unwrap();
            let c = database.connection();
            f.populate(c, MountPhase::ControlPublished);
            let (mount, map, activated) = install(c, &f, &predecessor);
            check(c, &f, &predecessor, &mount, &map, &activated);
            let altered = [
                "UPDATE projection_provider_activations SET id='blake3:0000000000000000000000000000000000000000000000000000000000000000'",
                "UPDATE projection_provider_activations SET owner_identity_digest='blake3:0000000000000000000000000000000000000000000000000000000000000000'",
                "UPDATE projection_head SET generation=0",
                "UPDATE projection_snapshots SET digest='0000000000000000000000000000000000000000000000000000000000000000' WHERE generation=1",
                "UPDATE projection_selections SET generation=1",
                "UPDATE projection_provider_mounts SET phase='provider_prepared'",
                "UPDATE projection_provider_mounts SET payload=zeroblob(262145)",
                "INSERT INTO projection_provider_activations SELECT 'blake3:0000000000000000000000000000000000000000000000000000000000000000','blake3:0000000000000000000000000000000000000000000000000000000000000000',mount_id,catalog_id,source_generation,source_digest,activated_generation,activated_digest,payload FROM projection_provider_activations",
                "INSERT INTO projection_snapshots VALUES(2,'0000000000000000000000000000000000000000000000000000000000000000',NULL,x'7b7d')",
            ];
            for statement in altered {
                c.execute_batch("SAVEPOINT altered").unwrap();
                c.execute_batch(statement).unwrap();
                assert!(
                    private_active_with_source(
                        c,
                        &f.allocation,
                        &f.prepared,
                        &predecessor.binding,
                        None,
                        None
                    )
                    .is_err(),
                    "accepted {statement}"
                );
                c.execute_batch("ROLLBACK TO altered; RELEASE altered")
                    .unwrap();
                check(c, &f, &predecessor, &mount, &map, &activated);
            }
            let foreign_keys: u64 = c
                .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
                .unwrap();
            c.execute_batch("PRAGMA foreign_keys=OFF; SAVEPOINT orphan")
                .unwrap();
            c.execute(
                "UPDATE projection_provider_activations SET catalog_id=?1",
                [format!("blake3:{}", "0".repeat(64))],
            )
            .unwrap();
            assert!(
                private_active_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &predecessor.binding,
                    None,
                    None
                )
                .is_err()
            );
            c.execute_batch("ROLLBACK TO orphan; RELEASE orphan")
                .unwrap();
            if foreign_keys != 0 {
                c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
            }
            check(c, &f, &predecessor, &mount, &map, &activated);
            database.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; Active schema-view fence"]
fn active_refuses_mount_view_before_document_decode() {
    child::run(
        concat!(
            module_path!(),
            "::active_refuses_mount_view_before_document_decode"
        ),
        120,
        || {
            child::declare();
            let (f, predecessor) = case();
            let database = BareConnection::reference().unwrap();
            let c = database.connection();
            f.populate(c, MountPhase::ControlPublished);
            let (mount, map, activated) = install(c, &f, &predecessor);
            check(c, &f, &predecessor, &mount, &map, &activated);
            c.execute_batch("SAVEPOINT changed; ALTER TABLE projection_provider_mounts RENAME TO retained_mounts; CREATE VIEW projection_provider_mounts AS SELECT * FROM retained_mounts;").unwrap();
            let error = private_active_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &predecessor.binding,
                None,
                None,
            )
            .err()
            .unwrap();
            assert!(format!("{error:?}").contains("schema fingerprint differs"));
            let error = private_activation_state_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &predecessor.binding,
                [0x41; 16],
            )
            .err()
            .unwrap();
            assert!(format!("{error:?}").contains("schema fingerprint differs"));
            c.execute_batch("ROLLBACK TO changed; RELEASE changed")
                .unwrap();
            check(c, &f, &predecessor, &mount, &map, &activated);
            database.finish(Ok(())).unwrap();
        },
    );
}
