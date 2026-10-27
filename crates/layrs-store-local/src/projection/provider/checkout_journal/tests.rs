use super::*;
use crate::projection::{
    provider::{
        database_io::BareConnection, process_profile::tests::child,
        recovery::error::ProviderBootstrapCause,
    },
    ProjectionStoreError,
};
#[path = "tests/fixture.rs"]
mod fixture;
use fixture::{Fixture, DIRECTORY, IMAGE, STAGE};

fn refuses<T>(result: Result<T>, message: &str) {
    let error = match result {
        Ok(_) => panic!("expected refusal: {message}"),
        Err(e) => e,
    };
    assert!(
        matches!(&error,ProviderBootstrapCause::Validation(e) if matches!(e.as_ref(),ProjectionStoreError::Invalid(actual) if actual==message)),
        "expected {message:?}, got {error:?}"
    );
}
fn memory(body: impl FnOnce(&Connection, &ExpectedCheckout)) {
    let db = BareConnection::reference().unwrap();
    let expected = Fixture::new().expected();
    fixture::schema(db.connection(), &expected);
    body(db.connection(), &expected);
    if !db.connection().is_autocommit() {
        db.connection().execute_batch("ROLLBACK").unwrap();
    }
    db.finish(Ok(())).unwrap();
}
fn move_row(
    c: &Connection,
    expected: &ExpectedCheckout,
    key: RowKey,
    change: impl FnOnce(&CheckoutRow) -> Result<CheckoutRow>,
) -> CheckoutRow {
    let before = read(c, expected, None).unwrap().row(key).unwrap().clone();
    let after = change(&before).unwrap();
    assert_eq!(
        advance(c, expected, &before, &after, None).unwrap(),
        AdvanceEffect::Updated
    );
    after
}
macro_rules! owned {
    ($name:ident,$body:block)=>{
        #[test]
        #[ignore="UNQUALIFIED until ROOT reviews/releases the exact owned checkout-journal SQL recipe"]
        fn $name(){child::run(concat!(module_path!(),"::",stringify!($name)),120,||{child::declare();$body});}
    }
}

#[test]
fn checkout_inventory_and_codec_refuse_ambiguous_unbounded_or_noncanonical_data() {
    let mut f = Fixture::new();
    let expected = f.expected();
    assert_eq!(expected.directories(), &[RepoPath::parse("dir").unwrap()]);
    assert_eq!(expected.temporary_name(1).unwrap(), "file-000001");
    assert!(expected.maximum_encoded_bytes() >= expected.declared_encoded_bytes());
    for database in [true, false] {
        let mut header = expected.header().clone();
        if database {
            header.database_identity = "1:0:256".into();
        } else {
            header.format_file_identity = "1:0:256".into();
        }
        refuses(codec::header(&header), "checkout header identities alias");
    }
    let row = expected.declared_row(RowKey::File(0)).unwrap();
    let encoded = row.encode().unwrap();
    assert_eq!(CheckoutRow::decode(&encoded).unwrap(), row);
    let pretty = serde_json::to_vec_pretty(&row).unwrap();
    assert!(CheckoutRow::decode(&pretty).is_err());
    let mut wire = serde_json::to_value(&row).unwrap();
    wire["unexpected"] = true.into();
    assert!(CheckoutRow::decode(&canonical_json(&wire).unwrap()).is_err());
    refuses(
        CheckoutRow::decode(&vec![b' '; MAX_ROW_BYTES + 1]),
        "checkout row bytes exceed bound",
    );
    let duplicate = format!(
        "{{\"schemaVersion\":\"{ROW_SCHEMA}\",{}",
        std::str::from_utf8(&encoded[1..]).unwrap()
    );
    assert!(CheckoutRow::decode(duplicate.as_bytes()).is_err());
    refuses(
        ExpectedCheckout::new(&f.staging, &f.files, &f.deletions, &"c".repeat(64), None),
        "checkout image digest differs from planned image",
    );
    f.files[0].executable = true;
    refuses(
        ExpectedCheckout::new(&f.staging, &f.files, &f.deletions, IMAGE, None),
        "Windows cannot faithfully materialize executable checkout mode",
    );
    f.files[0].executable = false;
    f.files[1].path = RepoPath::parse("DIR/a").unwrap();
    refuses(
        ExpectedCheckout::new(&f.staging, &f.files, &f.deletions, IMAGE, None),
        "checkout inventory has duplicate or aliased leaves",
    );
    f.files[1].path = RepoPath::parse("DIR/b").unwrap();
    refuses(
        ExpectedCheckout::new(&f.staging, &f.files, &f.deletions, IMAGE, None),
        "checkout directory spelling aliases another parent",
    );
    f.files[1].path = RepoPath::parse("dir/a/child").unwrap();
    refuses(
        ExpectedCheckout::new(&f.staging, &f.files, &f.deletions, IMAGE, None),
        "checkout leaf contains another checkout leaf",
    );
    f.files[1].path = RepoPath::parse(".LaYrS/private").unwrap();
    assert!(ExpectedCheckout::new(&f.staging, &f.files, &f.deletions, IMAGE, None).is_err());
    f.files[1].path = RepoPath::parse("dir/b:stream").unwrap();
    assert!(ExpectedCheckout::new(&f.staging, &f.files, &f.deletions, IMAGE, None).is_err());
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(matches!(
        ExpectedCheckout::new(&f.staging, &f.files, &f.deletions, IMAGE, Some(&cancelled)),
        Err(ProviderBootstrapCause::Vfs(layrs_vfs::Error::Cancelled))
    ));
}

owned!(
    checkout_journal_exact_progress_replays_read_only_and_never_rewrites_published,
    {
        memory(|c, expected| {
            assert_eq!(declare(c, expected, None).unwrap(), DeclareEffect::Inserted);
            assert!(!read(c, expected, None).unwrap().is_complete());
            move_row(c, expected, RowKey::Stage, |r| r.owned_directory(STAGE));
            move_row(c, expected, RowKey::Directory(0), |r| {
                r.owned_directory(DIRECTORY)
            });
            let mut last = None;
            for ordinal in 0..2 {
                let key = RowKey::File(ordinal);
                let owned = move_row(c, expected, key, |r| {
                    r.owned_file(&format!("1:0:{}", 2001 + ordinal))
                });
                assert!(
                    matches!(&owned.entry,CheckoutEntry::File{state,..} if state.owned_temp_inode().is_some())
                );
                let verified = move_row(c, expected, key, CheckoutRow::verified_file);
                assert!(
                    matches!(&verified.entry,CheckoutEntry::File{state,..} if state.owned_temp_inode().is_none())
                );
                let publishing = move_row(c, expected, key, |r| r.publishing_file(DIRECTORY));
                let published = move_row(c, expected, key, CheckoutRow::published_file);
                assert!(
                    matches!(&published.entry,CheckoutEntry::File{state,..} if state.owned_temp_inode().is_none())
                );
                refuses(
                    advance(c, expected, &published, &owned, None),
                    "checkout phase transition or inode changed",
                );
                last = Some((publishing, published));
            }
            assert!(read(c, expected, None).unwrap().is_complete());
            c.execute_batch(
                "COMMIT; PRAGMA foreign_keys=OFF; PRAGMA query_only=ON; BEGIN DEFERRED",
            )
            .unwrap();
            let before = c.total_changes();
            assert_eq!(
                declare(c, expected, None).unwrap(),
                DeclareEffect::AlreadyPresent
            );
            let (previous, next) = last.unwrap();
            assert_eq!(
                advance(c, expected, &previous, &next, None).unwrap(),
                AdvanceEffect::AlreadyPresent
            );
            assert!(read(c, expected, None).unwrap().is_complete());
            assert_eq!(c.total_changes(), before);
        });
    }
);

owned!(checkout_journal_rollback_and_ownership_parent_cas_fences, {
    memory(|c, expected| {
        declare(c, expected, None).unwrap();
        c.execute_batch("ROLLBACK; BEGIN IMMEDIATE").unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM projection_prejournal", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
        declare(c, expected, None).unwrap();
        let file = read(c, expected, None)
            .unwrap()
            .row(RowKey::File(0))
            .unwrap()
            .clone();
        let owned = file.owned_file("1:0:2001").unwrap();
        let before = c.total_changes();
        refuses(
            advance(c, expected, &file, &owned, None),
            "checkout file has no durable stage directory",
        );
        assert_eq!(c.total_changes(), before);
        let stage = read(c, expected, None)
            .unwrap()
            .row(RowKey::Stage)
            .unwrap()
            .clone();
        for reserved in [
            &expected.header().database_identity,
            &expected.header().format_file_identity,
        ] {
            let alias = codec::file_as_directory_identity(reserved).unwrap();
            refuses(
                advance(
                    c,
                    expected,
                    &stage,
                    &stage.owned_directory(&alias).unwrap(),
                    None,
                ),
                "checkout directory identity aliases another owner",
            );
            assert_eq!(c.total_changes(), before);
        }
        move_row(c, expected, RowKey::Stage, |r| r.owned_directory(STAGE));
        let reserved = file
            .owned_file(&expected.header().database_identity)
            .unwrap();
        refuses(
            advance(c, expected, &file, &reserved, None),
            "checkout file inode aliases another owner",
        );
        advance(c, expected, &file, &owned, None).unwrap();
        let second = read(c, expected, None)
            .unwrap()
            .row(RowKey::File(1))
            .unwrap()
            .clone();
        refuses(
            advance(
                c,
                expected,
                &second,
                &second.owned_file("1:0:2001").unwrap(),
                None,
            ),
            "checkout file inode aliases another owner",
        );
        let dir = read(c, expected, None)
            .unwrap()
            .row(RowKey::Directory(0))
            .unwrap()
            .clone();
        refuses(
            advance(
                c,
                expected,
                &dir,
                &dir.owned_directory(STAGE).unwrap(),
                None,
            ),
            "checkout directory identity aliases another owner",
        );
        let verified = owned.verified_file().unwrap();
        advance(c, expected, &owned, &verified, None).unwrap();
        refuses(
            advance(
                c,
                expected,
                &verified,
                &verified.publishing_file(DIRECTORY).unwrap(),
                None,
            ),
            "checkout parent has no durable identity",
        );
        move_row(c, expected, RowKey::Directory(0), |r| {
            r.owned_directory(DIRECTORY)
        });
        refuses(
            advance(
                c,
                expected,
                &verified,
                &verified.publishing_file(STAGE).unwrap(),
                None,
            ),
            "checkout publication parent identity differs",
        );
        let mut substituted = verified.clone();
        if let CheckoutEntry::File { state, .. } = &mut substituted.entry {
            *state = FileState::Verified {
                inode: "1:0:2002".into(),
            };
        }
        refuses(
            advance(c, expected, &owned, &substituted, None),
            "checkout phase transition or inode changed",
        );
        refuses(
            advance(c, expected, &file, &owned, None),
            "checkout journal exact predecessor differs",
        );
        c.execute_batch("ROLLBACK; PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE")
            .unwrap();
        refuses(
            declare(c, expected, None),
            "checkout journal writer requires FK ON",
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM projection_prejournal", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
    });
});

owned!(
    checkout_journal_refuses_foreign_incomplete_oversized_and_canonical_tampering,
    {
        memory(|c, expected| {
            declare(c, expected, None).unwrap();
            for (sql,message) in [
            ("DELETE FROM projection_prejournal WHERE path LIKE '%:header'","checkout journal is incomplete or has foreign rows"),
            ("INSERT INTO projection_prejournal VALUES('foreign','foreign','foreign',x'7b7d','owned')","checkout journal is incomplete or has foreign rows"),
            ("UPDATE projection_prejournal SET state='cleaning' WHERE path LIKE '%:file:000000'","checkout journal SQL scalar or operation differs"),
            ("UPDATE projection_prejournal SET payload=zeroblob(8193) WHERE path LIKE '%:file:000000'","checkout journal SQL scalar or operation differs"),
            ("UPDATE projection_prejournal SET path=printf('%01000d',1) WHERE path LIKE '%:file:000000'","checkout journal SQL scalar or operation differs"),
            ("UPDATE projection_prejournal SET operation='another-operation' WHERE path LIKE '%:file:000000'","checkout journal SQL scalar or operation differs"),
        ] {
            c.execute_batch("SAVEPOINT tamper").unwrap();c.execute_batch(sql).unwrap();let before=c.total_changes();
            refuses(read(c,expected,None),message);refuses(declare(c,expected,None),message);assert_eq!(c.total_changes(),before);
            c.execute_batch("ROLLBACK TO tamper; RELEASE tamper").unwrap();
        }
            c.execute_batch("SAVEPOINT canonical_tamper").unwrap();
            let mut row = expected.declared_row(RowKey::File(0)).unwrap();
            row.header_digest = format!("blake3:{}", "0".repeat(64));
            c.execute(
                "UPDATE projection_prejournal SET payload=?1,digest=?2 WHERE path=?3",
                params![
                    row.encode().unwrap(),
                    row.digest().unwrap(),
                    expected.sql_key(RowKey::File(0)).unwrap()
                ],
            )
            .unwrap();
            refuses(
                read(c, expected, None),
                "checkout row differs from expected inventory",
            );
            c.execute_batch("ROLLBACK TO canonical_tamper; RELEASE canonical_tamper")
                .unwrap();
            c.execute_batch("SAVEPOINT forged_progress").unwrap();
            let mut row = expected.declared_row(RowKey::File(0)).unwrap();
            if let CheckoutEntry::File { state, .. } = &mut row.entry {
                *state = FileState::Published {
                    inode: "1:0:2001".into(),
                    parent_identity: DIRECTORY.into(),
                };
            }
            c.execute(
                "UPDATE projection_prejournal SET payload=?1,digest=?2 WHERE path=?3",
                params![
                    row.encode().unwrap(),
                    row.digest().unwrap(),
                    expected.sql_key(RowKey::File(0)).unwrap()
                ],
            )
            .unwrap();
            refuses(
                read(c, expected, None),
                "checkout file has no durable stage directory",
            );
            c.execute_batch("ROLLBACK TO forged_progress; RELEASE forged_progress")
                .unwrap();
            assert_eq!(read(c, expected, None).unwrap().rows().len(), 5);
        });
    }
);
