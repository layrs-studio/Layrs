//! Owned, in-memory SQLite interpretation of the post-bootstrap mount row.
use super::*;
use crate::projection::provider::{process_profile::tests::child, records::Receipt};
use crate::projection::{RecoveryBinding, provider::database_io::BareConnection};
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
    let identity = stage.root_filesystem_identity();
    let parts: Vec<_> = identity.split(':').collect();
    assert_eq!(parts.len(), 4);
    let volume = u32::from_str_radix(parts[2], 16).unwrap();
    let index = u64::from_str_radix(parts[3], 16).unwrap();
    let mut wire = b"LYRTICK2".to_vec();
    wire.extend_from_slice(&[0x11; 16]);
    wire.extend_from_slice(f.source.commitment().vfs_catalog_id.as_str().as_bytes());
    wire.extend_from_slice(&volume.to_le_bytes());
    wire.extend_from_slice(&index.to_le_bytes());
    wire.extend_from_slice(digest.as_str().as_bytes());
    wire.extend_from_slice(&1u32.to_le_bytes());
    wire.extend_from_slice(&(root.len() as u32).to_le_bytes());
    wire.extend_from_slice(root);
    let parsed = layrs_vfs::Registration::decode_control_directory(&wire).unwrap();
    parsed.matches_control_identity(&control).unwrap();
    assert_eq!(parsed.encode(), wire);
    wire
}

fn case() -> (tests::fixture::Fixture, ProviderPreparationRecord) {
    let f = tests::fixture::Fixture::new("G1", false);
    let record = ProviderPreparationRecord {
        schema_version: "layrs.projection-provider-mount.v2".into(),
        bootstrap: f.mount(MountPhase::ControlPublished),
        binding: binding(&f),
        source: serde_json::from_slice(&serde_json::to_vec(f.source.commitment()).unwrap())
            .unwrap(),
        root_id_hex: "11".repeat(16),
        registration_ticket_hex: ticket(&f)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        state: records::preparation::PreparationState::RegistrationPending,
    };
    record
        .require_references(&f.allocation, &f.prepared, &f.source, [0x11; 16])
        .unwrap();
    (f, record)
}

fn install(c: &Connection, record: &ProviderPreparationRecord) {
    c.execute(
        "UPDATE projection_provider_mounts SET phase=?1,payload=?2,digest=?3 WHERE id=?4",
        params![
            record.sql_phase(),
            record.encode().unwrap(),
            record.digest().unwrap(),
            record.bootstrap.transaction_id
        ],
    )
    .unwrap();
}

fn check(
    c: &Connection,
    f: &tests::fixture::Fixture,
    record: &ProviderPreparationRecord,
) -> PrivatePreparationValidation {
    let result =
        private_preparation_with_source(c, &f.allocation, &f.prepared, &record.binding, [0x11; 16])
            .unwrap();
    assert_eq!(result.mount, *record);
    assert_eq!(result.source.commitment(), f.source.commitment());
    result
}

#[test]
#[ignore = "owned audited Windows SQLite process; full post-bootstrap private validator"]
fn exact_pending_prepared_and_legacy_mutual_refusal() {
    child::run(
        concat!(
            module_path!(),
            "::exact_pending_prepared_and_legacy_mutual_refusal"
        ),
        120,
        || {
            child::declare();
            let (f, pending) = case();
            let database = BareConnection::reference().unwrap();
            let c = database.connection();
            f.populate(c, MountPhase::ControlPublished);
            assert!(
                matches!(private(c, &f.allocation, Some(&f.prepared)).unwrap(), PrivateValidation::Initialized(m) if m.phase == MountPhase::ControlPublished)
            );
            assert!(
                private_preparation_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &pending.binding,
                    [0x11; 16]
                )
                .is_err()
            );
            install(c, &pending);
            check(c, &f, &pending);
            assert!(private(c, &f.allocation, Some(&f.prepared)).is_err());
            let mut prepared =
                ProviderPreparationRecord::decode(&pending.encode().unwrap()).unwrap();
            prepared.state = records::preparation::PreparationState::ProviderPrepared {
                pending_digest: pending.digest().unwrap(),
            };
            install(c, &prepared);
            check(c, &f, &prepared);
            assert!(private(c, &f.allocation, Some(&f.prepared)).is_err());
            database.finish(Ok(())).unwrap();

            let empty = BareConnection::reference().unwrap();
            assert!(
                private_preparation_with_source(
                    empty.connection(),
                    &f.allocation,
                    &f.prepared,
                    &pending.binding,
                    [0x11; 16]
                )
                .is_err()
            );
            empty.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; independent post-bootstrap projection refusals"]
fn binding_phase_payload_digest_and_source_contradictions_refuse() {
    child::run(
        concat!(
            module_path!(),
            "::binding_phase_payload_digest_and_source_contradictions_refuse"
        ),
        120,
        || {
            child::declare();
            let (f, pending) = case();
            let database = BareConnection::reference().unwrap();
            let c = database.connection();
            f.populate(c, MountPhase::ControlPublished);
            install(c, &pending);
            check(c, &f, &pending);
            let mut wrong = binding(&f);
            wrong.transaction_id = "00000000000000000000000000000002".into();
            assert!(
                private_preparation_with_source(c, &f.allocation, &f.prepared, &wrong, [0x11; 16])
                    .is_err()
            );
            assert!(
                private_preparation_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &pending.binding,
                    [0x22; 16]
                )
                .is_err()
            );

            for (column, value) in [
                ("phase", "'provider_prepared'"),
                (
                    "digest",
                    "'blake3:0000000000000000000000000000000000000000000000000000000000000000'",
                ),
            ] {
                c.execute_batch(&format!(
                    "SAVEPOINT altered; UPDATE projection_provider_mounts SET {column}={value};"
                ))
                .unwrap();
                assert!(
                    private_preparation_with_source(
                        c,
                        &f.allocation,
                        &f.prepared,
                        &pending.binding,
                        [0x11; 16]
                    )
                    .is_err()
                );
                c.execute_batch("ROLLBACK TO altered; RELEASE altered;")
                    .unwrap();
                check(c, &f, &pending);
            }
            c.execute_batch(
                "SAVEPOINT altered; UPDATE projection_provider_mounts SET payload=x'7b7d';",
            )
            .unwrap();
            assert!(
                private_preparation_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &pending.binding,
                    [0x11; 16]
                )
                .is_err()
            );
            c.execute_batch("ROLLBACK TO altered; RELEASE altered;")
                .unwrap();
            c.execute_batch("SAVEPOINT altered; UPDATE projection_snapshots SET digest='0000000000000000000000000000000000000000000000000000000000000000';").unwrap();
            assert!(
                private_preparation_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &pending.binding,
                    [0x11; 16]
                )
                .is_err()
            );
            c.execute_batch("ROLLBACK TO altered; RELEASE altered;")
                .unwrap();
            check(c, &f, &pending);
            database.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; bounds and future-table refusals"]
fn future_tables_and_bounds_refuse() {
    child::run(
        concat!(module_path!(), "::future_tables_and_bounds_refuse"),
        120,
        || {
            child::declare();
            let (f, pending) = case();
            let database = BareConnection::reference().unwrap();
            let c = database.connection();
            f.populate(c, MountPhase::ControlPublished);
            install(c, &pending);
            check(c, &f, &pending);

            c.execute_batch("SAVEPOINT altered; INSERT INTO projection_provider_companions VALUES('blake3:0000000000000000000000000000000000000000000000000000000000000000',0,0,'blake3:0000000000000000000000000000000000000000000000000000000000000000','blake3:0000000000000000000000000000000000000000000000000000000000000000',x'7b7d');").unwrap();
            let failure = private_preparation_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &pending.binding,
                [0x11; 16],
            )
            .err()
            .unwrap();
            assert!(format!("{failure:?}").contains("projection_provider_companions"));
            c.execute_batch("ROLLBACK TO altered; RELEASE altered;")
                .unwrap();

            c.execute_batch("SAVEPOINT altered; UPDATE projection_provider_mounts SET payload=zeroblob(131073);").unwrap();
            assert!(
                private_preparation_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &pending.binding,
                    [0x11; 16]
                )
                .is_err()
            );
            c.execute_batch("ROLLBACK TO altered; RELEASE altered;")
                .unwrap();
            c.execute_batch(
                "SAVEPOINT altered; UPDATE projection_provider_descriptors SET manifest=x'7b7d';",
            )
            .unwrap();
            assert!(
                private_preparation_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &pending.binding,
                    [0x11; 16]
                )
                .is_err()
            );
            c.execute_batch("ROLLBACK TO altered; RELEASE altered;")
                .unwrap();
            check(c, &f, &pending);
            database.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; reopen schema fence precedes mount query"]
fn reopen_rejects_mount_view_before_document_decode() {
    child::run(
        concat!(
            module_path!(),
            "::reopen_rejects_mount_view_before_document_decode"
        ),
        120,
        || {
            child::declare();
            let (f, pending) = case();
            let database = BareConnection::reference().unwrap();
            let c = database.connection();
            f.populate(c, MountPhase::ControlPublished);
            install(c, &pending);
            assert_eq!(
                private_preparation_reopen_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &pending.binding
                )
                .unwrap()
                .mount,
                pending,
            );
            c.execute_batch(
                "SAVEPOINT substituted;
                 ALTER TABLE projection_provider_mounts RENAME TO retained_mounts;
                 CREATE VIEW projection_provider_mounts AS SELECT * FROM retained_mounts;",
            )
            .unwrap();
            let error = private_preparation_reopen_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &pending.binding,
            )
            .err()
            .expect("substituted mount view must refuse");
            assert!(format!("{error:?}").contains("schema fingerprint differs"));
            c.execute_batch("ROLLBACK TO substituted; RELEASE substituted;")
                .unwrap();
            assert_eq!(
                private_preparation_reopen_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &pending.binding
                )
                .unwrap()
                .mount,
                pending,
            );
            database.finish(Ok(())).unwrap();
        },
    );
}
