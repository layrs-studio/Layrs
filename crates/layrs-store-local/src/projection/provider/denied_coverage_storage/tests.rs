//! Scalar SQL recipes using production retention DDL. No manufactured plan,
//! Ready, live authority, native recovery, or lifecycle acceptance is claimed.
use super::*;
use crate::projection::{
    provider::{
        database_io::with_reference, owned_test as child, records::denied_coverage::test_data,
        recovery::error::ProviderBootstrapCause,
    },
    retention,
};

fn document() -> DerivedDeniedCoverage {
    let (prefix, normalized) = test_data::observation();
    DerivedDeniedCoverage::from_observation(
        &prefix,
        "00000000000000000000000000000001",
        &normalized,
        None,
    )
    .unwrap()
}

#[test]
#[ignore = "ROOT release and owned audited SQL child120/outer180 required; unmeasured"]
fn denied_document_insert_replays_without_writes_and_obeys_caller_rollback() {
    child::run(
        concat!(
            module_path!(),
            "::denied_document_insert_replays_without_writes_and_obeys_caller_rollback"
        ),
        120,
        || {
            child::declare();
            with_reference(retention::SQL, |c| {
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let value = document();
                let reference = value.record().reference()?;
                assert!(read_required(c, &reference, None).is_err());
                let before = c.total_changes();
                assert_eq!(
                    insert_exact(c, &value, &reference, None)?,
                    InsertedDocument::Inserted
                );
                assert_eq!(c.total_changes(), before + 1);
                assert_eq!(read_required(c, &reference, None)?, *value.record());
                let before = c.total_changes();
                assert_eq!(
                    insert_exact(c, &value, &reference, None)?,
                    InsertedDocument::AlreadyPresent
                );
                assert_eq!(c.total_changes(), before);
                // Document participates in the caller's transaction. There is
                // intentionally no fake Planned-v2 link in this primitive test.
                c.execute_batch("ROLLBACK; BEGIN IMMEDIATE")?;
                assert!(read_required(c, &reference, None).is_err());
                assert_eq!(
                    insert_exact(c, &value, &reference, None)?,
                    InsertedDocument::Inserted
                );
                c.execute_batch("COMMIT; PRAGMA query_only=ON; BEGIN DEFERRED")?;
                let before = c.total_changes();
                assert_eq!(read_required(c, &reference, None)?, *value.record());
                assert_eq!(c.total_changes(), before);
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
        },
    );
}

#[test]
#[ignore = "ROOT release and owned audited SQL child120/outer180 required; unmeasured"]
fn denied_document_conflicting_or_noncanonical_body_is_never_repaired() {
    child::run(
        concat!(
            module_path!(),
            "::denied_document_conflicting_or_noncanonical_body_is_never_repaired"
        ),
        120,
        || {
            child::declare();
            with_reference(retention::SQL, |c| {
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let value = document();
                let reference = value.record().reference()?;
                let mut changed =
                    TransitionDeniedCoverageRecord::decode(&value.record().encode()?)?;
                changed.source_version.effective_image_digest = "f".repeat(64);
                let canonical = value.record().encode()?;
                let pretty = serde_json::to_vec_pretty(
                    &serde_json::from_slice::<serde_json::Value>(&canonical).unwrap(),
                )
                .unwrap();
                for payload in [changed.encode()?, pretty, b"{}".to_vec()] {
                    c.execute_batch("SAVEPOINT bad_body")?;
                    c.execute(
                        "INSERT INTO projection_observations(digest,payload) VALUES(?1,?2)",
                        params![reference.id, payload],
                    )?;
                    let before = c.total_changes();
                    assert!(read_required(c, &reference, None).is_err());
                    assert!(insert_exact(c, &value, &reference, None).is_err());
                    assert_eq!(c.total_changes(), before);
                    let retained: Vec<u8> = c.query_row(
                        "SELECT payload FROM projection_observations WHERE digest=?1",
                        [&reference.id],
                        |r| r.get(0),
                    )?;
                    assert_eq!(retained, payload);
                    c.execute_batch("ROLLBACK TO bad_body; RELEASE bad_body")?;
                }
                // A false reference cannot cause an initial insertion either.
                let mut wrong = reference.clone();
                wrong.path_bytes += 1;
                let before = c.total_changes();
                assert!(insert_exact(c, &value, &wrong, None).is_err());
                assert_eq!(c.total_changes(), before);
                assert!(read_optional(c, &reference, None)?.is_none());
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
        },
    );
}

#[test]
#[ignore = "ROOT release and owned audited SQL child120/outer180 required; unmeasured"]
fn denied_document_requires_transaction_fk_and_live_cancellation_without_effects() {
    child::run(
        concat!(
            module_path!(),
            "::denied_document_requires_transaction_fk_and_live_cancellation_without_effects"
        ),
        120,
        || {
            child::declare();
            with_reference(retention::SQL, |c| {
                let value = document();
                let reference = value.record().reference()?;
                let before = c.total_changes();
                assert!(insert_exact(c, &value, &reference, None).is_err());
                assert!(read_required(c, &reference, None).is_err());
                assert_eq!(c.total_changes(), before);
                c.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE")?;
                assert!(insert_exact(c, &value, &reference, None).is_err());
                assert_eq!(c.total_changes(), before);
                c.execute_batch("ROLLBACK; PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let cancel = Cancellation::default();
                cancel.cancel();
                assert!(matches!(
                    insert_exact(c, &value, &reference, Some(&cancel)),
                    Err(ProviderBootstrapCause::Vfs(layrs_vfs::Error::Cancelled))
                ));
                assert!(matches!(
                    read_required(c, &reference, Some(&cancel)),
                    Err(ProviderBootstrapCause::Vfs(layrs_vfs::Error::Cancelled))
                ));
                assert_eq!(c.total_changes(), before);
                // Another legacy/foreign observation remains untouched; this
                // targeted reader does not certify the enclosing inventory.
                c.execute(
                    "INSERT INTO projection_observations(digest,payload) VALUES('foreign',?1)",
                    [b"legacy".as_slice()],
                )?;
                insert_exact(c, &value, &reference, None)?;
                let retained: Vec<u8> = c.query_row(
                    "SELECT payload FROM projection_observations WHERE digest='foreign'",
                    [],
                    |r| r.get(0),
                )?;
                assert_eq!(retained, b"legacy");
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
        },
    );
}

#[test]
#[ignore = "ROOT release and owned audited SQL child120/outer180 required; unmeasured"]
fn denied_writer_refuses_deferred_savepoint_and_query_only_before_body_work() {
    child::run(
        concat!(
            module_path!(),
            "::denied_writer_refuses_deferred_savepoint_and_query_only_before_body_work"
        ),
        120,
        || {
            child::declare();
            with_reference(retention::SQL, |c| {
                let value = document();
                let reference = value.record().reference()?;
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                insert_exact(c, &value, &reference, None)?;
                c.execute_batch("COMMIT")?;
                let before = c.total_changes();
                for begin in ["BEGIN DEFERRED", "SAVEPOINT outer_denial"] {
                    c.execute_batch(begin)?;
                    assert!(!c.is_autocommit());
                    // Invalid reference would fail validation if the writer
                    // reached body work before checking main transaction state.
                    let mut invalid_reference = reference.clone();
                    invalid_reference.id = "invalid".into();
                    let error = insert_exact(c, &value, &invalid_reference, None).unwrap_err();
                    assert!(error.to_string().contains("active main write transaction"));
                    // Establishing a read snapshot still cannot authorize a
                    // replay through the writer; the reader remains usable.
                    assert_eq!(read_required(c, &reference, None)?, *value.record());
                    let error = insert_exact(c, &value, &reference, None).unwrap_err();
                    assert!(error.to_string().contains("active main write transaction"));
                    assert_eq!(c.total_changes(), before);
                    c.execute_batch("ROLLBACK")?;
                }
                c.execute_batch("PRAGMA query_only=ON; BEGIN DEFERRED")?;
                assert_eq!(read_required(c, &reference, None)?, *value.record());
                assert!(insert_exact(c, &value, &reference, None).is_err());
                c.execute_batch(
                    "ROLLBACK; PRAGMA query_only=OFF; BEGIN IMMEDIATE; PRAGMA query_only=ON",
                )?;
                // query_only may be enabled after acquiring the write state.
                assert_eq!(
                    c.transaction_state(Some(DatabaseName::Main))?,
                    TransactionState::Write
                );
                let error = insert_exact(c, &value, &reference, None).unwrap_err();
                assert!(error.to_string().contains("query-only"));
                assert_eq!(read_required(c, &reference, None)?, *value.record());
                assert_eq!(c.total_changes(), before);
                c.execute_batch("ROLLBACK; PRAGMA query_only=OFF")?;
                Ok(())
            })
            .unwrap();
        },
    );
}

#[test]
#[ignore = "ROOT release and owned audited SQL child120/outer180 required; unmeasured"]
fn denied_document_rejects_oversized_existing_blob_before_decoding_without_repair() {
    child::run(
        concat!(
            module_path!(),
            "::denied_document_rejects_oversized_existing_blob_before_decoding_without_repair"
        ),
        120,
        || {
            child::declare();
            with_reference(retention::SQL, |c| {
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let value = document();
                let reference = value.record().reference()?;
                let too_large = TransitionDeniedCoverageRecord::MAX_BYTES as i64 + 1;
                // First prove the production CHECK, then inject corruption in
                // this disposable scalar DB only. No native fixture is edited.
                assert!(c.execute("INSERT INTO projection_observations(digest,payload) VALUES(?1,zeroblob(?2))", params![reference.id, too_large]).is_err());
                c.execute_batch("PRAGMA ignore_check_constraints=ON")?;
                c.execute("INSERT INTO projection_observations(digest,payload) VALUES(?1,zeroblob(?2))", params![reference.id, too_large])?;
                c.execute_batch("PRAGMA ignore_check_constraints=OFF")?;
                let before = c.total_changes();
                let error = read_required(c, &reference, None).unwrap_err();
                assert!(error.to_string().contains("stored body type/length"));
                assert!(insert_exact(c, &value, &reference, None).is_err());
                assert_eq!(c.total_changes(), before);
                let retained: i64 = c.query_row("SELECT length(payload) FROM projection_observations WHERE digest=?1", [&reference.id], |r| r.get(0))?;
                assert_eq!(retained, too_large);
                c.execute_batch("ROLLBACK")?;
                Ok(())
            }).unwrap();
        },
    );
}
