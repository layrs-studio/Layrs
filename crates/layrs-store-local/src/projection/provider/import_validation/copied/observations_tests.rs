//! Private inventory gate after inherited authentication. Actual normalized SQL
//! commitments and canonical bodies are exercised; no VerifiedImportClosure,
//! physical ingress, Ready or source-authority proof is manufactured here.
use super::*;
use crate::projection::provider::{
    database_io::BareConnection,
    process_profile::tests::child,
    records::{
        codec,
        denied_coverage::DeniedAuthorityData,
        import::{ImportVersion, ProviderImportKind, imported_segments_rowset},
    },
};
use layrs_core::scm::{ObjectId, SpaceId, StepId, StoreId, TreeId, WorktreeId};
use rusqlite::{ToSql, params};
use std::collections::BTreeSet;
const ID: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";

fn with_rows<T>(
    c: &Connection,
    ids: &BTreeSet<String>,
    version: usize,
    after: impl FnOnce(&[rows::Spec<'_>], &ImportClosure) -> CauseResult<T>,
) -> CauseResult<T> {
    let json = serde_json::to_string(ids).unwrap();
    let bound: [&dyn ToSql; 1] = [&json];
    let mut specs = Vec::new();
    for kind in [
        rows::Kind::Snapshots,
        rows::Kind::Selections,
        rows::Kind::Captures,
        rows::Kind::Publications,
        rows::Kind::Catalogs,
        rows::Kind::Descriptors,
        rows::Kind::ActivationMaps,
        rows::Kind::TransitionReceipts,
        rows::Kind::Companions,
        rows::Kind::CompanionFiles,
        rows::Kind::CompanionDeletions,
        rows::Kind::ContentMappings,
        rows::Kind::RestoreHistory,
    ] {
        // The other rowsets are bounded scalar projections, deliberately not
        // source-history fixtures. These tests qualify only this private gate.
        let nonempty = matches!(kind, rows::Kind::Snapshots | rows::Kind::Selections);
        specs.push(rows::Spec {
            kind,
            preflight_sql: if nonempty { "SELECT 1,8" } else { "SELECT 0,0" },
            rows_sql: if nonempty {
                "SELECT 1"
            } else {
                "SELECT 0 WHERE 0"
            },
            params: &[],
            max_rows: 1_000_000,
            max_bytes: 1024 * 1024 * 1024,
        });
    }
    specs.push(rows::Spec {
        kind: rows::Kind::DenialObservations,
        preflight_sql: scoped::denial_history::PREFLIGHT_SQL,
        rows_sql: scoped::denial_history::ROWS_SQL,
        params: &bound,
        max_rows: 1_000_000,
        max_bytes: 1024 * 1024 * 1024,
    });
    specs.truncate(version);
    let preflight = rows::preflight(c, &specs, None)?;
    let closure = rows::stream(c, &specs, &preflight, None)?;
    after(&specs, &closure)
}
fn prefix(closure: ImportClosure) -> ProviderImportPrefixRecord {
    let source = crate::projection::ProjectionIdentity {
        store_id: StoreId::parse("store:copied-observations").unwrap(),
        worktree_id: WorktreeId::parse("worktree:copied-source").unwrap(),
        space_id: SpaceId::parse("space:copied-observations").unwrap(),
        origin: "https://example.invalid".into(),
        principal_id: "principal:copied-observations".into(),
    };
    let mut destination = source.clone();
    destination.worktree_id = WorktreeId::parse("worktree:copied-destination").unwrap();
    let version = if closure.denial_observations.is_some() {
        3
    } else if closure.restore_history.is_some() {
        2
    } else {
        1
    };
    let value = ProviderImportPrefixRecord {
        schema_version: format!("layrs.projection-provider-import-prefix.v{version}"),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Ordinary {
            identity: source,
            format_digest: ID.into(),
            private_schema: 8,
            checkout_identity: closure
                .restore_history
                .as_ref()
                .map(|_| "fs:v1:0000000000000001:0000000000000002".into()),
            admission: None,
            version: ImportVersion {
                generation: 0,
                snapshot_digest: "a".repeat(64),
                selection_digest: "b".repeat(64),
                capture_count: 0,
                capture_head: None,
                effective_image_digest: "c".repeat(64),
            },
        },
        destination_identity: destination,
        inherited_segments: imported_segments_rowset(&[]).unwrap(),
        ancestry_root_receipt: None,
        closure,
    };
    value.validate_fields().unwrap();
    value
}
fn document(
    prefix: &ProviderImportPrefixRecord,
    transaction: &str,
) -> TransitionDeniedCoverageRecord {
    let source = match &prefix.source {
        ImportSource::Ordinary { identity, .. } | ImportSource::Provider { identity, .. } => {
            identity
        }
    };
    let authority = DeniedAuthorityData::ReobservedExpandedScope {
        policy_set_id: ObjectId::parse(ID).unwrap(),
        authorization_epoch: 2,
    };
    let mut rows = blake3::Hasher::new();
    rows.update(b"layrs-provider-transition-denied-paths-data-v1\0");
    rows.update(&0u64.to_be_bytes());
    TransitionDeniedCoverageRecord {
        schema_version: "layrs.projection-transition-denied-coverage.v1".into(),
        transaction_id: transaction.into(),
        source_prefix_id: prefix.digest().unwrap(),
        source_identity: source.clone(),
        destination_identity: prefix.destination_identity.clone(),
        source_version: prefix.source.version().clone(),
        step_id: StepId::parse(ID).unwrap(),
        root_tree_id: TreeId::parse(ID).unwrap(),
        path_semantics: layrs_core::projection::PathSemantics::PortableWindows,
        authority_data_digest: codec::hash(
            "layrs-provider-transition-authority-data-v1",
            &codec::encode(&authority, 8192).unwrap(),
        ),
        authority,
        observation_generation: 1,
        observation_snapshot_digest: "d".repeat(64),
        observation_selection_digest: "e".repeat(64),
        denied_path_count: 0,
        path_bytes: 0,
        rows_digest: format!("blake3:{}", rows.finalize()),
        denied_paths: Vec::new(),
    }
}
fn insert(c: &Connection, body: &TransitionDeniedCoverageRecord) {
    c.execute(
        "INSERT INTO projection_observations VALUES(?1,?2)",
        params![body.reference().unwrap().id, body.encode().unwrap()],
    )
    .unwrap();
}
fn committed(c: &Connection, ids: &BTreeSet<String>, version: usize) -> ProviderImportPrefixRecord {
    with_rows(c, ids, version, |_, closure| Ok(prefix(closure.clone()))).unwrap()
}
fn check(
    c: &Connection,
    prefix: &ProviderImportPrefixRecord,
    ids: &BTreeSet<String>,
    current: Option<&TransitionDeniedCoverageRecord>,
) -> CauseResult<()> {
    let version = if prefix.closure.denial_observations.is_some() {
        14
    } else if prefix.closure.restore_history.is_some() {
        13
    } else {
        12
    };
    with_rows(c, ids, version, |specs, closure| {
        require_observation_inventory(c, prefix, specs, closure, current, None)
    })
}
fn setup(
    c: &Connection,
) -> (
    ProviderImportPrefixRecord,
    BTreeSet<String>,
    TransitionDeniedCoverageRecord,
) {
    c.execute_batch(retention::SQL).unwrap();
    c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
        .unwrap();
    let empty = BTreeSet::new();
    let first = committed(c, &empty, 12);
    let inherited = document(&first, "00000000000000000000000000000001");
    insert(c, &inherited);
    let ids = BTreeSet::from([inherited.reference().unwrap().id]);
    let prefix = committed(c, &ids, 14);
    let current = document(&prefix, "00000000000000000000000000000002");
    insert(c, &current);
    (prefix, ids, current)
}

#[test]
#[ignore = "owned audited SQLite child120; exact copied observation inventory primitive"]
fn copied_observations_accept_exact_union_and_keep_legacy_none_strict() {
    child::run(
        concat!(
            module_path!(),
            "::copied_observations_accept_exact_union_and_keep_legacy_none_strict"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            c.execute_batch(retention::SQL).unwrap();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            let empty = BTreeSet::new();
            for version in [12, 13, 14] {
                let prefix = committed(c, &empty, version);
                check(c, &prefix, &empty, None).unwrap();
                let current = document(&prefix, "00000000000000000000000000000002");
                insert(c, &current);
                // Zero denied paths still occupy one required current document.
                assert_eq!(current.denied_path_count, 0);
                check(c, &prefix, &empty, Some(&current)).unwrap();
                let before = c.total_changes();
                assert!(check(c, &prefix, &empty, None).is_err());
                assert_eq!(before, c.total_changes());
                c.execute("DELETE FROM projection_observations", [])
                    .unwrap();
            }
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited SQLite child120; inherited/current hash and inventory refusal primitive"]
fn copied_observations_refuse_extras_missing_and_substituted_bodies_without_repair() {
    child::run(
        concat!(
            module_path!(),
            "::copied_observations_refuse_extras_missing_and_substituted_bodies_without_repair"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (prefix, ids, current) = setup(c);
            let current_id = current.reference().unwrap().id;
            let inherited_id = ids.first().unwrap();
            let other = document(&prefix, "00000000000000000000000000000003");
            let before = c.total_changes();
            check(c, &prefix, &ids, Some(&current)).unwrap();
            assert_eq!(before, c.total_changes());
            for variant in 0..4 {
                c.execute_batch("SAVEPOINT refused").unwrap();
                match variant {
                    0 => insert(c, &other),
                    1 => {
                        c.execute(
                            "DELETE FROM projection_observations WHERE digest=?1",
                            [&current_id],
                        )
                        .unwrap();
                        insert(c, &other);
                    }
                    2 => {
                        c.execute(
                            "UPDATE projection_observations SET payload=?1 WHERE digest=?2",
                            params![other.encode().unwrap(), current_id],
                        )
                        .unwrap();
                    }
                    _ => {
                        c.execute(
                            "UPDATE projection_observations SET payload=?1 WHERE digest=?2",
                            params![other.encode().unwrap(), inherited_id],
                        )
                        .unwrap();
                    }
                }
                let before = c.total_changes();
                assert!(check(c, &prefix, &ids, Some(&current)).is_err());
                assert_eq!(before, c.total_changes());
                c.execute_batch("ROLLBACK TO refused; RELEASE refused")
                    .unwrap();
            }
            check(c, &prefix, &ids, Some(&current)).unwrap();
            let mut wrong =
                TransitionDeniedCoverageRecord::decode(&current.encode().unwrap()).unwrap();
            wrong.source_prefix_id = ID.into();
            let before = c.total_changes();
            assert!(
                check(c, &prefix, &ids, Some(&wrong))
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("prefix/destination")
            );
            assert_eq!(before, c.total_changes());
            wrong = TransitionDeniedCoverageRecord::decode(&current.encode().unwrap()).unwrap();
            wrong.destination_identity.worktree_id =
                WorktreeId::parse("worktree:other-destination").unwrap();
            let before = c.total_changes();
            assert!(
                check(c, &prefix, &ids, Some(&wrong))
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("prefix/destination")
            );
            assert_eq!(before, c.total_changes());
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited SQLite child120; explicit inherited/current selector collision guard"]
fn copied_observations_refuse_current_in_inherited_selector_despite_matching_global_count() {
    child::run(
        concat!(
            module_path!(),
            "::copied_observations_refuse_current_in_inherited_selector_despite_matching_global_count"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (prefix, ids, current) = setup(c);
            let current_id = current.reference().unwrap().id;
            let selector = serde_json::to_string(&[&current_id]).unwrap();
            let bound: [&dyn ToSql; 1] = [&selector];
            let collision = rows::Spec {
                kind: rows::Kind::DenialObservations,
                preflight_sql: scoped::denial_history::PREFLIGHT_SQL,
                rows_sql: scoped::denial_history::ROWS_SQL,
                params: &bound,
                max_rows: 1_000_000,
                max_bytes: 1024 * 1024 * 1024,
            };
            // Deliberately inconsistent selector DATA targets this defensive guard;
            // no source proof is manufactured. A real with_scope selector is sealed
            // by its incoming receipts and was already checked by the outer reader.
            let before = c.total_changes();
            let error = require_observation_inventory(
                c,
                &prefix,
                &[collision],
                &prefix.closure,
                Some(&current),
                None,
            )
            .err()
            .unwrap();
            assert!(error.to_string().contains("aliases inherited scope"));
            assert_eq!(before, c.total_changes());
            check(c, &prefix, &ids, Some(&current)).unwrap();
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}
