//! Authored DATA/SQL recipes only; not executed. SQL models release of a family
//! row after a caller has separately retained complete source absence proof.
use super::*;
use crate::projection::provider::records::denial_start::PendingDeniedCoverageReleaseRecord;

pub(in crate::projection) fn release_fixture() -> (
    AllocationRecord,
    PreparedRecord,
    SourceDeniedCoverageReservation,
    PendingDeniedCoverageReleaseRecord,
) {
    let (a, mut intent, mut body) = fixture();
    let p = PreparedRecord {
        schema_version: "layrs.projection-provider-prepared.v1".into(),
        transaction_id: a.transaction_id.clone(),
        receipt_file_identity: "1:0:25".into(),
        allocation: a.data_reference().unwrap(),
        control_identity_hex: literal("LYRCDIR1")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    };
    p.require_allocation(&a).unwrap();
    let ImportSource::Provider {
        prepared, binding, ..
    } = &mut intent.plan.source_prefix.source
    else {
        unreachable!()
    };
    *prepared = p.data_reference().unwrap();
    binding.prepared.as_mut().unwrap().receipt = prepared.clone();
    intent.plan.source_prefix_id = intent.plan.source_prefix.digest().unwrap();
    body.source_prefix_id = intent.plan.source_prefix_id.clone();
    intent.plan.denial_coverage = Some(body.reference().unwrap());
    let funding = SourceDeniedCoverageReservation::new(&a, &intent, &body, None).unwrap();
    let record =
        PendingDeniedCoverageReleaseRecord::new(&a, &p, &funding, "1:0:88".into()).unwrap();
    (a, p, funding, record)
}

#[test]
fn compact_release_roundtrips_exact_preimage_charge_and_ap_without_full_body() {
    let (a, p, funding, record) = release_fixture();
    let bytes = record.encode().unwrap();
    assert!(bytes.len() < PendingDeniedCoverageReleaseRecord::MAX_BYTES);
    let decoded = PendingDeniedCoverageReleaseRecord::decode(&bytes).unwrap();
    assert_eq!(decoded, record);
    decoded.require_references(&a, &p).unwrap();
    decoded.require_reservation(&funding).unwrap();
    assert_eq!(decoded.allocation(), &a.data_reference().unwrap());
    assert_eq!(decoded.prepared(), &p.data_reference().unwrap());
    assert_eq!(decoded.binding(), funding.binding());
    assert_eq!(decoded.document_bytes(), funding.document_bytes());
    assert_eq!(decoded.intent_bytes(), funding.intent_bytes());
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
    assert!(value.get("deniedPaths").is_none());
    assert!(value.get("body").is_none());
    assert_eq!(value["documentBytes"], funding.document_bytes());
    let token = SourceDeniedCoverageRelease::new(&a, &p, &decoded).unwrap();
    assert_eq!(token.binding(), funding.binding());
    assert_eq!(token.operation(), funding.operation());
    assert_eq!(token.reservation_bytes(), funding.reservation_bytes());
    let moved = record.with_file_identity("1:0:89".into()).unwrap();
    assert_eq!(moved.operation(), record.operation());
    assert_eq!(moved.file_identity(), "1:0:89");
    assert_ne!(
        moved.data_reference().unwrap(),
        record.data_reference().unwrap()
    );
    let mut wrong_p = PreparedRecord::decode(&p.encode().unwrap()).unwrap();
    wrong_p.receipt_file_identity = "1:0:99".into();
    assert!(
        PendingDeniedCoverageReleaseRecord::new(&a, &wrong_p, &funding, "1:0:88".into()).is_err()
    );
    assert!(SourceDeniedCoverageRelease::new(&a, &wrong_p, &record).is_err());
}

#[test]
fn compact_release_codec_rejects_unknown_duplicate_noncanonical_and_changed_charges() {
    let (_, _, _, record) = release_fixture();
    let bytes = record.encode().unwrap();
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for field in ["documentBytes", "intentBytes", "reservationBytes"] {
        let mut changed = original.clone();
        changed[field] = (changed[field].as_u64().unwrap() + 1).into();
        assert!(
            PendingDeniedCoverageReleaseRecord::decode(&canonical_json(&changed).unwrap()).is_err()
        );
    }
    for (field, value) in [
        ("operation", "provider-denial:bad"),
        ("receiptFileIdentity", "1:00:88"),
        ("schemaVersion", "unknown"),
    ] {
        let mut changed = original.clone();
        changed[field] = value.into();
        assert!(
            PendingDeniedCoverageReleaseRecord::decode(&canonical_json(&changed).unwrap()).is_err()
        );
    }
    for nested in [false, true] {
        let mut changed = original.clone();
        let object = if nested {
            &mut changed["binding"]
        } else {
            &mut changed
        };
        object["unknown"] = true.into();
        assert!(
            PendingDeniedCoverageReleaseRecord::decode(&canonical_json(&changed).unwrap()).is_err()
        );
    }
    let text = std::str::from_utf8(&bytes).unwrap();
    let duplicate = format!(
        "{{\"documentBytes\":{},{}",
        record.document_bytes(),
        &text[1..]
    );
    assert!(PendingDeniedCoverageReleaseRecord::decode(duplicate.as_bytes()).is_err());
    let whitespace = format!(" {text}");
    assert!(PendingDeniedCoverageReleaseRecord::decode(whitespace.as_bytes()).is_err());
    assert!(PendingDeniedCoverageReleaseRecord::decode(&vec![b' '; 64 * 1024 + 1]).is_err());
    let mut changed = original;
    changed["documentBytes"] = 0.into();
    assert!(
        PendingDeniedCoverageReleaseRecord::decode(&canonical_json(&changed).unwrap()).is_err()
    );
}

#[test]
#[ignore = "unrun owned SQL recipe; compact exact release after source absence proof"]
fn denial_release_exact_row_replays_and_survives_quota_reduction() {
    child::run(
        concat!(
            module_path!(),
            "::denial_release_exact_row_replays_and_survives_quota_reduction"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                schema_v2::initialize_reference(c)?;
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let (a, p, funding, record) = release_fixture();
                let release = SourceDeniedCoverageRelease::new(&a, &p, &record)?;
                seed(c, &funding, funding.reservation_bytes() + 31)?;
                funding.reserve(c, None)?;
                c.execute(
                    "INSERT INTO reservations VALUES('foreign','worktree:foreign','foreign',31)",
                    [],
                )?;
                c.execute_batch("UPDATE quota SET bytes=0")?;
                assert!(funding.require_present(c, None).is_err());
                assert_eq!(
                    release.release(c, None)?,
                    SourceDeniedCoverageReleaseEffect::Released
                );
                assert_eq!(
                    release.read_presence(c, None)?,
                    SourceDeniedCoveragePresence::Absent
                );
                assert_eq!(
                    c.query_row(
                        "SELECT remaining FROM reservations WHERE id='foreign'",
                        [],
                        |r| r.get::<_, u64>(0)
                    )?,
                    31
                );
                let changes = c.total_changes();
                assert_eq!(
                    release.release(c, None)?,
                    SourceDeniedCoverageReleaseEffect::AlreadyAbsent
                );
                assert_eq!(c.total_changes(), changes);
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
        },
    );
}

#[test]
#[ignore = "unrun owned SQL recipe; release collision and installation refusals"]
fn denial_release_refuses_conflicts_installations_changed_owner_or_amount_without_repair() {
    child::run(
        concat!(module_path!(), "::denial_release_refuses_conflicts_installations_changed_owner_or_amount_without_repair"),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                schema_v2::initialize_reference(c)?;
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let (a, p, funding, record) = release_fixture();
                let release = SourceDeniedCoverageRelease::new(&a, &p, &record)?;
                seed(c, &funding, MAX_METADATA)?;
                funding.reserve(c, None)?;
                for sql in [
                    "UPDATE reservations SET owner='worktree:foreign' WHERE id=?1",
                    "UPDATE reservations SET remaining=remaining-1 WHERE id=?1",
                    "UPDATE reservations SET operation='collision' WHERE id=?1",
                    "INSERT INTO reservations VALUES('collision','worktree:foreign',?1,1)",
                    "UPDATE worktrees SET state='initializing' WHERE id=(SELECT owner FROM reservations WHERE id=?1)",
                    "UPDATE worktrees SET database_identity='1:0:99' WHERE id=(SELECT owner FROM reservations WHERE id=?1)",
                ] {
                    c.execute_batch("SAVEPOINT corrupt")?;
                    c.execute(sql, [funding.operation()])?;
                    let changes = c.total_changes();
                    assert!(release.release(c, None).is_err());
                    assert_eq!(changes, c.total_changes());
                    c.execute_batch("ROLLBACK TO corrupt; RELEASE corrupt")?;
                }
                c.execute_batch("SAVEPOINT corrupt; PRAGMA ignore_check_constraints=ON")?;
                c.execute("UPDATE reservations SET remaining=-1 WHERE id=?1", [funding.operation()])?;
                let changes = c.total_changes();
                assert!(release.release(c, None).is_err());
                assert_eq!(changes, c.total_changes());
                c.execute_batch("ROLLBACK TO corrupt; RELEASE corrupt; PRAGMA ignore_check_constraints=OFF")?;
                for exact in [false, true] {
                    c.execute_batch("SAVEPOINT corrupt")?;
                    if exact {
                        c.execute("INSERT INTO exact_installations VALUES(?1,?2,?2,0,?3,NULL,0)", params![funding.operation(), TOKEN, b"journal".as_slice()])?;
                    } else {
                        c.execute("INSERT INTO installations VALUES(?1,?2,?3,NULL,0)", params![funding.operation(), TOKEN, b"journal".as_slice()])?;
                    }
                    let changes = c.total_changes();
                    assert!(release.release(c, None).is_err());
                    assert_eq!(changes, c.total_changes());
                    c.execute_batch("ROLLBACK TO corrupt; RELEASE corrupt")?;
                }
                c.execute_batch("ROLLBACK")?;
                Ok(())
            }).unwrap();
        },
    );
}

#[test]
#[ignore = "unrun owned SQL recipe; release transaction FK query-only and cancellation fences"]
fn denial_release_requires_main_write_fk_and_writable_connection_and_stops_on_cancel() {
    child::run(
        concat!(
            module_path!(),
            "::denial_release_requires_main_write_fk_and_writable_connection_and_stops_on_cancel"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                schema_v2::initialize_reference(c)?;
                let (a, p, funding, record) = release_fixture();
                let release = SourceDeniedCoverageRelease::new(&a, &p, &record)?;
                assert!(release.release(c, None).is_err());
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                seed(c, &funding, MAX_METADATA)?;
                funding.reserve(c, None)?;
                c.execute_batch("COMMIT; BEGIN DEFERRED")?;
                let changes = c.total_changes();
                assert!(release.release(c, None).is_err());
                assert_eq!(changes, c.total_changes());
                c.execute_batch("ROLLBACK; PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE")?;
                assert!(release.release(c, None).is_err());
                assert_eq!(changes, c.total_changes());
                c.execute_batch(
                    "ROLLBACK; PRAGMA foreign_keys=ON; BEGIN IMMEDIATE; PRAGMA query_only=ON",
                )?;
                assert!(release.release(c, None).is_err());
                assert_eq!(changes, c.total_changes());
                c.execute_batch("PRAGMA query_only=OFF")?;
                let cancel = Cancellation::default();
                cancel.cancel();
                assert!(release.release(c, Some(&cancel)).is_err());
                assert_eq!(changes, c.total_changes());
                assert_eq!(
                    release.read_presence(c, None)?,
                    SourceDeniedCoveragePresence::Reserved
                );
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
        },
    );
}
