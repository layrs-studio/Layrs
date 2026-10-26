use super::rows::{Kind, Spec};
use super::*;
use layrs_core::scm::{SpaceId, StoreId, WorktreeId};
use rusqlite::ToSql;

fn identity() -> crate::projection::ProjectionIdentity {
    crate::projection::ProjectionIdentity {
        store_id: StoreId::parse("store:source").unwrap(),
        worktree_id: WorktreeId::parse("worktree:source").unwrap(),
        space_id: SpaceId::parse("space:source").unwrap(),
        origin: "https://example.invalid".into(),
        principal_id: "principal:test".into(),
    }
}

#[test]
fn provenance_refuses_zero_and_outside_capture_interval() {
    let index = ProvenanceIndex {
        segments: Vec::new(),
        receipts: Vec::new(),
        local_identity: identity(),
        capture_count: 2,
        local_min_generation: 1,
    };
    assert!(index.capture_source(0, 3).is_err());
    assert!(index.capture_source(3, 3).is_err());
    let (origin, first, last) = index.capture_source(2, 3).unwrap();
    assert_eq!(origin, &identity());
    assert_eq!((first, last), (1, 3));
}

#[test]
#[ignore = "owned audited Windows SQLite process; bounded rowset fixture"]
fn rowset_reader_limits_to_high_water_and_detects_change_between_passes() {
    use crate::projection::provider::{database_io::BareConnection, process_profile::tests::child};
    child::run(
        concat!(
            module_path!(),
            "::rowset_reader_limits_to_high_water_and_detects_change_between_passes"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            c.execute_batch(
                "CREATE TABLE evidence(id INTEGER PRIMARY KEY, value TEXT NOT NULL) STRICT;
        INSERT INTO evidence VALUES(1,'a'),(2,'later');",
            )
            .unwrap();
            let high_water: i64 = 1;
            let params: [&dyn ToSql; 1] = [&high_water];
            let kinds = [
                Kind::Snapshots,
                Kind::Selections,
                Kind::Captures,
                Kind::Publications,
                Kind::Catalogs,
                Kind::Descriptors,
                Kind::ActivationMaps,
                Kind::TransitionReceipts,
                Kind::Companions,
                Kind::CompanionFiles,
                Kind::CompanionDeletions,
                Kind::ContentMappings,
            ];
            let specs: Vec<_> = kinds.into_iter().map(|kind| Spec {
        kind,
        preflight_sql: "SELECT count(*),coalesce(sum(8+length(CAST(value AS BLOB))),0) FROM evidence WHERE id<=?1",
        rows_sql: "SELECT id,value FROM evidence WHERE id<=?1 ORDER BY id",
        params: &params,
        max_rows: 2,
        max_bytes: 32,
    }).collect();
            let before = rows::preflight(&c, &specs, None).unwrap();
            let first = rows::stream(&c, &specs, &before, None).unwrap();
            c.execute("UPDATE evidence SET value='tail changed' WHERE id=2", [])
                .unwrap();
            let again = rows::preflight(&c, &specs, None).unwrap();
            assert_eq!(first, rows::stream(&c, &specs, &again, None).unwrap());
            c.execute("UPDATE evidence SET value='changed' WHERE id=1", [])
                .unwrap();
            assert!(rows::stream(&c, &specs, &again, None).is_err());
            bare.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; full Active source SQL closure"]
fn active_source_closure_is_exact_and_map_tamper_is_refused() {
    use crate::projection::provider::{
        database_io::BareConnection,
        process_profile::tests::child,
        records::{
            ProviderBootstrapBinding, Receipt, Record,
            import::{
                ImportClosure, ImportRowset, ImportSource, ImportSourceReferences, ImportVersion,
                ProviderImportKind, ProviderImportPrefixRecord,
            },
        },
        validation::{active_fixture, private_active_capture_with_source},
    };
    const ZERO: &str = "blake3:0000000000000000000000000000000000000000000000000000000000000000";
    const RAW: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    fn row(count: u64) -> ImportRowset {
        ImportRowset {
            count,
            digest: ZERO.into(),
        }
    }
    child::run(
        concat!(
            module_path!(),
            "::active_source_closure_is_exact_and_map_tamper_is_refused"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (f, _, _, activation, activated) = active_fixture(c, false);
            c.execute_batch("BEGIN DEFERRED").unwrap();
            let binding =
                ProviderBootstrapBinding::decode(&activation.binding.encode().unwrap()).unwrap();
            let marker = &f.allocation.format;
            let mut destination = marker.identity.clone();
            destination.worktree_id = WorktreeId::parse("worktree:destination").unwrap();
            let mut h = blake3::Hasher::new();
            h.update(b"layrs-projection-provider-import-segments-v1\0");
            h.update(&0u64.to_be_bytes());
            let mut prefix = ProviderImportPrefixRecord {
                schema_version: "layrs.projection-provider-import-prefix.v1".into(),
                kind: ProviderImportKind::ExpandedProvider,
                source: ImportSource::Provider {
                    identity: marker.identity.clone(),
                    format_digest: crate::projection::provider::records::codec::hash(
                        "layrs-projection-provider-import-format-v1",
                        &marker.bytes().unwrap(),
                    ),
                    private_schema: 8,
                    allocation: f.allocation.data_reference().unwrap(),
                    prepared: f.prepared.data_reference().unwrap(),
                    binding,
                    activation_map_id: activation.digest().unwrap(),
                    version: ImportVersion {
                        generation: 1,
                        snapshot_digest: activated.digest.clone(),
                        selection_digest: activation.selection_digest.clone(),
                        capture_count: 0,
                        capture_head: None,
                        effective_image_digest: RAW.into(),
                    },
                },
                destination_identity: destination,
                inherited_segments: ImportRowset {
                    count: 0,
                    digest: format!("blake3:{}", h.finalize()),
                },
                ancestry_root_receipt: None,
                closure: ImportClosure {
                    snapshots: row(2),
                    selections: row(1),
                    captures: row(0),
                    publications: row(0),
                    catalogs: row(1),
                    descriptors: row(1),
                    activation_maps: row(1),
                    transition_receipts: row(0),
                    companions: row(0),
                    companion_files: row(0),
                    companion_deletions: row(0),
                    content_mappings: row(0),
                },
            };
            let metadata = read_source_metadata(
                c,
                marker,
                ImportDatabaseKind::Provider,
                &prefix,
                ImportSourceReferences::Provider {
                    allocation: &f.allocation,
                    prepared: &f.prepared,
                    activation: &activation,
                },
                RAW,
                None,
            )
            .unwrap();
            prefix.closure = scoped::compute_scoped_closure(
                c,
                ImportDatabaseKind::Provider,
                &prefix,
                &metadata,
                None,
            )
            .unwrap();
            let proof = private_active_capture_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &activation.binding,
                Some([0x41; 16]),
                None,
            )
            .unwrap();
            let verified = verify_source(
                c,
                marker,
                ImportDatabaseKind::Provider,
                &prefix,
                ImportSourceReferences::Provider {
                    allocation: &f.allocation,
                    prepared: &f.prepared,
                    activation: &activation,
                },
                Some(&proof),
                RAW,
                None,
            )
            .unwrap();
            assert_eq!(verified.closure(), &prefix.closure);
            assert_eq!(verified.prefix_id(), prefix.digest().unwrap());
            c.execute_batch("SAVEPOINT tamper").unwrap();
            c.execute(
                "UPDATE projection_provider_activations SET activated_digest=?1 WHERE id=?2",
                rusqlite::params!["0".repeat(64), activation.digest().unwrap()],
            )
            .unwrap();
            assert!(
                verify_source(
                    c,
                    marker,
                    ImportDatabaseKind::Provider,
                    &prefix,
                    ImportSourceReferences::Provider {
                        allocation: &f.allocation,
                        prepared: &f.prepared,
                        activation: &activation,
                    },
                    Some(&proof),
                    RAW,
                    None
                )
                .is_err()
            );
            c.execute_batch("ROLLBACK TO tamper; RELEASE tamper; COMMIT")
                .unwrap();
            c.execute_batch("PRAGMA foreign_keys=OFF; BEGIN DEFERRED")
                .unwrap();
            c.execute(
                "UPDATE projection_snapshots SET digest=?1 WHERE generation=0",
                ["x".repeat(65_536)],
            )
            .unwrap();
            let oversized = read_source_metadata(
                c,
                marker,
                ImportDatabaseKind::Provider,
                &prefix,
                ImportSourceReferences::Provider {
                    allocation: &f.allocation,
                    prepared: &f.prepared,
                    activation: &activation,
                },
                RAW,
                None,
            )
            .err()
            .expect("oversized digest must refuse");
            assert!(oversized.to_string().contains("snapshot/selection prefix"));
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}
