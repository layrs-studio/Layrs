//! Isolated exact SQL copy primitives; no fabricated VerifiedImportClosure,
//! Ready, destination ownership or remote authority is produced by these tests.
use super::*;
use crate::projection::provider::{database_io::BareConnection, process_profile::tests::child};

#[test]
fn denial_documents_precede_receipts_without_changing_legacy_copy_order() {
    let index = |kind| {
        COPY_ORDER
            .iter()
            .position(|actual| *actual == kind)
            .unwrap()
    };
    assert!(index(Kind::DenialObservations) < index(Kind::TransitionReceipts));
    assert!(index(Kind::Captures) < index(Kind::RestoreHistory));
    assert_eq!(
        COPY_ORDER
            .iter()
            .copied()
            .filter(|kind| !matches!(kind, Kind::RestoreHistory | Kind::DenialObservations))
            .collect::<Vec<_>>(),
        vec![
            Kind::Snapshots,
            Kind::Selections,
            Kind::Captures,
            Kind::Publications,
            Kind::Catalogs,
            Kind::Descriptors,
            Kind::ActivationMaps,
            Kind::Companions,
            Kind::CompanionFiles,
            Kind::CompanionDeletions,
            Kind::TransitionReceipts,
            Kind::ContentMappings
        ]
    );
}

#[test]
#[ignore = "owned audited SQLite memory process; exact denial document copy primitives"]
fn inherited_document_copy_is_exact_replayable_conflict_refusing_and_rollback_owned() {
    child::run(
        concat!(
            module_path!(),
            "::inherited_document_copy_is_exact_replayable_conflict_refusing_and_rollback_owned"
        ),
        120,
        || {
            child::declare();
            let source = BareConnection::reference().unwrap();
            let destination = BareConnection::reference().unwrap();
            let s = source.connection();
            let d = destination.connection();
            for c in [s, d] {
                c.execute_batch(crate::projection::retention::SQL).unwrap();
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                    .unwrap();
            }
            let id = format!("blake3:{}", "1".repeat(64));
            let unrelated = format!("blake3:{}", "2".repeat(64));
            let body = b"{\"canonicalFixtureData\":true}";
            s.execute(
                "INSERT INTO projection_observations VALUES(?1,?2)",
                params![id, body.as_slice()],
            )
            .unwrap();
            s.execute(
                "INSERT INTO projection_observations VALUES(?1,?2)",
                params![unrelated, b"unrelated".as_slice()],
            )
            .unwrap();
            let json = serde_json::to_string(&[&id]).unwrap();
            let bindings: [&dyn rusqlite::ToSql; 1] = [&json];
            let spec = Spec {
                kind: Kind::DenialObservations,
                preflight_sql: scoped::denial_history::PREFLIGHT_SQL,
                rows_sql: scoped::denial_history::ROWS_SQL,
                params: &bindings,
                max_rows: 1_000_000,
                max_bytes: 1024 * 1024 * 1024,
            };
            visit(s, d, &spec, false, false, None).unwrap();
            visit(s, d, &spec, true, false, None).unwrap();
            let count: u64 = d
                .query_row("SELECT count(*) FROM projection_observations", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(count, 1);
            let retained: Vec<u8> = d
                .query_row(
                    "SELECT payload FROM projection_observations WHERE digest=?1",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(retained, body);
            let before = d.total_changes();
            visit(s, d, &spec, true, false, None).unwrap();
            assert_eq!(before, d.total_changes());
            d.execute_batch("ROLLBACK; BEGIN IMMEDIATE").unwrap();
            let count: u64 = d
                .query_row("SELECT count(*) FROM projection_observations", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(count, 0);
            d.execute(
                "INSERT INTO projection_observations VALUES(?1,?2)",
                params![id, b"different".as_slice()],
            )
            .unwrap();
            let before = d.total_changes();
            assert!(visit(s, d, &spec, false, false, None).is_err());
            assert!(visit(s, d, &spec, true, false, None).is_err());
            assert_eq!(before, d.total_changes());
            let retained: Vec<u8> = d
                .query_row(
                    "SELECT payload FROM projection_observations WHERE digest=?1",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(retained, b"different");
            for c in [s, d] {
                c.execute_batch("ROLLBACK").unwrap();
            }
            source.finish(Ok(())).unwrap();
            destination.finish(Ok(())).unwrap();
        },
    );
}
