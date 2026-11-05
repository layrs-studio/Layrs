use super::rows::{Kind, Spec};
use super::*;
use layrs_core::scm::{SpaceId, StoreId, WorktreeId};
use rusqlite::ToSql;

impl VerifiedImportClosure {
    /// Exercise the historical walker one receipt beyond the real C10 export
    /// prefix. The added D receipt is declared DATA, never a live D admission
    /// or a physical transition proof. Every ancestor closure is checked against
    /// the unchanged, real C10 SQL rows, including its copied restore history.
    pub(in crate::projection::provider) fn assert_current_c10_as_historical_ancestor(
        mut self,
        c: &Connection,
        prefix: &ProviderImportPrefixRecord,
    ) {
        use super::super::records::Record;
        assert!(matches!(
            prefix.source,
            ImportSource::Ordinary {
                private_schema: 10,
                admission: Some(_),
                ..
            }
        ));
        self.require_source_connection(c).unwrap();
        let parent = self.metadata.lineage.receipts.last().unwrap();
        let mut next = ProviderTransitionReceiptRecord::decode(&parent.encode().unwrap()).unwrap();
        let version = prefix.source.version();
        next.source_prefix = ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap();
        next.source_prefix_id = prefix.digest().unwrap();
        next.parent_receipt_id = prefix.ancestry_root_receipt.clone();
        next.source_identity = match &prefix.source {
            ImportSource::Ordinary { identity, .. } => identity.clone(),
            _ => unreachable!("C10 ordinary source asserted above"),
        };
        next.destination_identity = prefix.destination_identity.clone();
        next.transaction_id = "declared-future-d-receipt".into();
        next.imported_max_generation = version.generation;
        next.destination_observation_generation = version.generation + 1;
        next.destination_placement_generation = version.generation + 2;
        next.imported_capture_count = version.capture_count;
        next.destination_capture_sequence = version.capture_count + 1;
        next.content_mappings = prefix.closure.content_mappings.clone();
        next.validate_fields().unwrap();
        prefix.require_admission_parent(Some(parent)).unwrap();
        let segment = ImportedSegment::from_receipt(&next, Some(parent)).unwrap();
        self.metadata.lineage.segments.push(segment);
        self.metadata.lineage.receipts.push(next);
        self.metadata.lineage.local_identity = prefix.destination_identity.clone();
        self.metadata.lineage.local_min_generation = version.generation + 1;
        ancestry::verify_historical_closures(c, prefix, &self.metadata, None).unwrap();

        // A valid ancestor schema does not waive its committed SQL contents.
        let last = self.metadata.lineage.receipts.last_mut().unwrap();
        last.source_prefix.closure.snapshots.digest = format!("blake3:{}", "0".repeat(64));
        last.source_prefix_id = last.source_prefix.digest().unwrap();
        let error =
            ancestry::verify_historical_closures(c, prefix, &self.metadata, None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("ancestor immutable SQL closure differs")
        );
    }
}

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
                    denial_observations: None,
                    restore_history: None,
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
            verified.require_provider_inputs(&proof).unwrap();
            let other_bare = BareConnection::reference().unwrap();
            let other = other_bare.connection();
            let (other_fixture, _, _, other_activation, _) = active_fixture(other, true);
            let other_proof = private_active_capture_with_source(
                other,
                &other_fixture.allocation,
                &other_fixture.prepared,
                &other_activation.binding,
                Some([0x41; 16]),
                None,
            )
            .unwrap();
            assert!(verified.require_provider_inputs(&other_proof).is_err());
            other_bare.finish(Ok(())).unwrap();
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

#[test]
#[ignore = "owned audited Windows SQLite process; exact provider prefix copy"]
fn verified_prefix_copy_is_idempotent_and_refuses_collision() {
    use crate::projection::provider::{
        database_io::BareConnection,
        process_profile::tests::child,
        records::{
            Receipt, Record,
            import::{
                ImportClosure, ImportRowset, ImportSource, ImportSourceReferences, ImportVersion,
                ProviderImportKind, ProviderImportPrefixRecord,
            },
        },
        validation::{active_fixture, private_active_capture_with_source},
    };
    const IMAGE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ZERO: &str = "blake3:0000000000000000000000000000000000000000000000000000000000000000";
    let empty = || ImportRowset {
        count: 0,
        digest: ZERO.into(),
    };
    child::run(
        concat!(
            module_path!(),
            "::verified_prefix_copy_is_idempotent_and_refuses_collision"
        ),
        120,
        || {
            child::declare();
            let source_handle = BareConnection::reference().unwrap();
            let source = source_handle.connection();
            let (fixture, _, _, activation, activated) = active_fixture(source, false);
            let original = fixture.allocation.format.clone();
            let mut destination_marker = original.clone();
            destination_marker.identity.worktree_id = WorktreeId::parse("worktree:copied").unwrap();
            let destination_handle = BareConnection::reference().unwrap();
            let destination = destination_handle.connection();
            let ddl = [
                crate::projection::database::SQL,
                crate::projection::selection::SQL,
                crate::projection::retention::SQL,
                crate::projection::provider::schema::SQL,
            ]
            .concat();
            destination.execute_batch(&ddl).unwrap();
            destination.pragma_update(None, "user_version", 8).unwrap();
            destination
                .execute(
                    "INSERT INTO projection_meta VALUES(1,?1)",
                    [destination_marker.bytes().unwrap()],
                )
                .unwrap();
            destination
                .execute(
                    "INSERT INTO projection_backend VALUES(1,'ordinary',NULL)",
                    [],
                )
                .unwrap();
            source.execute_batch("BEGIN DEFERRED").unwrap();
            destination.execute_batch("BEGIN IMMEDIATE").unwrap();
            let mut hash = blake3::Hasher::new();
            hash.update(b"layrs-projection-provider-import-segments-v1\0");
            hash.update(&0u64.to_be_bytes());
            let mut prefix = ProviderImportPrefixRecord {
                schema_version: "layrs.projection-provider-import-prefix.v1".into(),
                kind: ProviderImportKind::ManagedOrdinary,
                source: ImportSource::Provider {
                    identity: original.identity.clone(),
                    format_digest: crate::projection::provider::records::codec::hash(
                        "layrs-projection-provider-import-format-v1",
                        &original.bytes().unwrap(),
                    ),
                    private_schema: 8,
                    allocation: fixture.allocation.data_reference().unwrap(),
                    prepared: fixture.prepared.data_reference().unwrap(),
                    binding:
                        crate::projection::provider::records::ProviderBootstrapBinding::decode(
                            &activation.binding.encode().unwrap(),
                        )
                        .unwrap(),
                    activation_map_id: activation.digest().unwrap(),
                    version: ImportVersion {
                        generation: 1,
                        snapshot_digest: activated.digest,
                        selection_digest: activation.selection_digest.clone(),
                        capture_count: 0,
                        capture_head: None,
                        effective_image_digest: IMAGE.into(),
                    },
                },
                destination_identity: destination_marker.identity.clone(),
                inherited_segments: ImportRowset {
                    count: 0,
                    digest: format!("blake3:{}", hash.finalize()),
                },
                ancestry_root_receipt: None,
                closure: ImportClosure {
                    denial_observations: None,
                    restore_history: None,
                    snapshots: ImportRowset {
                        count: 2,
                        ..empty()
                    },
                    selections: ImportRowset {
                        count: 1,
                        ..empty()
                    },
                    captures: empty(),
                    publications: empty(),
                    catalogs: ImportRowset {
                        count: 1,
                        ..empty()
                    },
                    descriptors: ImportRowset {
                        count: 1,
                        ..empty()
                    },
                    activation_maps: ImportRowset {
                        count: 1,
                        ..empty()
                    },
                    transition_receipts: empty(),
                    companions: empty(),
                    companion_files: empty(),
                    companion_deletions: empty(),
                    content_mappings: empty(),
                },
            };
            let metadata = read_source_metadata(
                source,
                &original,
                ImportDatabaseKind::Provider,
                &prefix,
                ImportSourceReferences::Provider {
                    allocation: &fixture.allocation,
                    prepared: &fixture.prepared,
                    activation: &activation,
                },
                IMAGE,
                None,
            )
            .unwrap();
            prefix.closure = scoped::compute_scoped_closure(
                source,
                ImportDatabaseKind::Provider,
                &prefix,
                &metadata,
                None,
            )
            .unwrap();
            let active_proof = private_active_capture_with_source(
                source,
                &fixture.allocation,
                &fixture.prepared,
                &activation.binding,
                Some([0x41; 16]),
                None,
            )
            .unwrap();
            let proof = verify_source(
                source,
                &original,
                ImportDatabaseKind::Provider,
                &prefix,
                ImportSourceReferences::Provider {
                    allocation: &fixture.allocation,
                    prepared: &fixture.prepared,
                    activation: &activation,
                },
                Some(&active_proof),
                IMAGE,
                None,
            )
            .unwrap();
            copy_verified_prefix(
                source,
                destination,
                &destination_marker,
                &prefix,
                &proof,
                None,
            )
            .unwrap();
            copy_verified_prefix(
                source,
                destination,
                &destination_marker,
                &prefix,
                &proof,
                None,
            )
            .unwrap();
            let snapshots: u64 = destination
                .query_row("SELECT count(*) FROM projection_snapshots", [], |r| {
                    r.get(0)
                })
                .unwrap();
            let descriptors: u64 = destination
                .query_row(
                    "SELECT count(*) FROM projection_provider_descriptors",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let live_mounts: u64 = destination
                .query_row("SELECT count(*) FROM projection_provider_mounts", [], |r| {
                    r.get(0)
                })
                .unwrap();
            let backend: String = destination
                .query_row(
                    "SELECT backend FROM projection_backend WHERE singleton=1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                (snapshots, descriptors, live_mounts, backend.as_str()),
                (2, 1, 0, "ordinary")
            );
            require_exact_copied_rows(destination, &destination_marker, &prefix, &proof, None)
                .unwrap();
            destination.execute_batch("SAVEPOINT foreign_tail").unwrap();
            destination.execute(
                "INSERT INTO projection_snapshots(generation,digest,parent_digest,payload) SELECT 2,?1,digest,payload FROM projection_snapshots WHERE generation=1",
                ["9".repeat(64)],
            ).unwrap();
            assert!(
                require_exact_copied_rows(destination, &destination_marker, &prefix, &proof, None)
                    .is_err()
            );
            destination
                .execute_batch("ROLLBACK TO foreign_tail; RELEASE foreign_tail")
                .unwrap();
            destination.execute_batch("SAVEPOINT collision").unwrap();
            destination
                .execute(
                    "UPDATE projection_snapshots SET payload=x'00' WHERE generation=1",
                    [],
                )
                .unwrap();
            assert!(
                require_exact_copied_rows(destination, &destination_marker, &prefix, &proof, None)
                    .is_err()
            );
            let refusal = copy_verified_prefix(
                source,
                destination,
                &destination_marker,
                &prefix,
                &proof,
                None,
            )
            .err()
            .expect("same-key altered row must refuse");
            assert!(refusal.to_string().contains("collision"), "{refusal}");
            destination
                .execute_batch("ROLLBACK TO collision; RELEASE collision; COMMIT")
                .unwrap();
            source.execute_batch("COMMIT").unwrap();
            source_handle.finish(Ok(())).unwrap();
            destination_handle.finish(Ok(())).unwrap();
        },
    );
}
