//! Receipt, hot-journal and following-effect assertions of the owned-process cuts.
use super::*;

pub(super) fn require_receipt_ack(f: &Fixture, r: &ReceiptAck) {
    let parent = f.metadata.join("provider-control");
    assert_eq!(
        crate::secure_fs::PinnedDirectory::open(&parent)
            .unwrap()
            .identity_token(),
        r.parent_identity
    );
    let temporary = parent.join(&r.leaf);
    if r.after {
        assert!(matches!(fs::symlink_metadata(&temporary),Err(e) if e.raw_os_error()==Some(2)));
        assert_eq!(FileAck::read(&f.receipt(receipt(r.kind))), r.file);
    } else {
        assert_eq!(FileAck::read(&temporary), r.file);
        assert!(!f.receipt(receipt(r.kind)).try_exists().unwrap());
    }
    assert_eq!(r.file.digest, r.expected_digest);
}
pub(super) fn require_hot(i: &ProviderBootstrapInspection, kind: u8, ack: &SqlAck) {
    let scope = purpose(kind).scope();
    let db = if scope == ProviderDatabaseScope::FamilyCatalog {
        &i.family
    } else {
        &i.private
    };
    assert_eq!(db.database, scope);
    assert_eq!(db.status, ProviderDatabaseStatus::RollbackRequired);
    let trigger = match db.cause.as_ref().expect("typed hot cause") {
        Cause::ReadonlyRollback(e) => e,
        e => panic!("wrong hot cause {e:?}"),
    };
    assert!(
        matches!(trigger.as_ref(),rusqlite::Error::SqliteFailure(e,_) if e.extended_code==rusqlite::ffi::SQLITE_READONLY_ROLLBACK)
    );
    let original = i
        .rollback_triggers
        .iter()
        .find(|t| t.database == scope)
        .expect("scoped original");
    assert!(std::ptr::eq(original.error(), trigger.as_ref()));
    assert!(ack.journal.is_some());
    assert!(i.phase.is_none());
    assert_eq!(
        i.state,
        if kind == 4 {
            ProviderBootstrapState::TerminalPendingValidation
        } else {
            ProviderBootstrapState::SqlValidationPending
        }
    );
}
pub(super) fn assert_previous(
    f: &Fixture,
    kind: u8,
    i: &ProviderBootstrapInspection,
    ack: &SqlAck,
) {
    let scope = purpose(kind).scope();
    let status = if scope == ProviderDatabaseScope::FamilyCatalog {
        i.family.status
    } else {
        i.private.status
    };
    if status == ProviderDatabaseStatus::RollbackRequired {
        require_hot(i, kind, ack);
        return;
    }
    if kind == 1 && status == ProviderDatabaseStatus::Unavailable {
        let cause = i.private.cause.as_ref().expect("zero-origin typed cause");
        assert!(
            matches!(cause,Cause::Validation(e) if matches!(e.as_ref(),ProjectionStoreError::Invalid(s) if s=="zero database has a sidecar; SQLite open forbidden"))
        );
        assert_eq!(ack.database.bytes, 0);
        assert!(ack.journal.is_some());
        assert!(i.phase.is_none());
        assert_eq!(i.state, ProviderBootstrapState::SqlValidationPending);
        return;
    }
    let a = stages(f);
    let family = FamilyAccess::open(f.family.root()).unwrap();
    let _locks = family.locks(&f.worktree, false).unwrap();
    if kind == 0 || kind == 4 {
        assert_eq!(status, ProviderDatabaseStatus::Validated);
        let state = database_io::read(&family.catalog, |c| family.validate_catalog(c, &a)).unwrap();
        let expected = if kind == 0 {
            Registration::Absent
        } else {
            Registration::Initializing
        };
        assert!(
            matches!(state,database_io::ReadState::Valid(v) if v==expected),
            "precommit must retain prior registry row/absence"
        );
    } else {
        assert_eq!(
            status,
            if kind == 1 {
                ProviderDatabaseStatus::Uninitialized
            } else {
                ProviderDatabaseStatus::Validated
            }
        );
        let db = database_io::DatabaseFile::open(
            f.metadata.join("projection.sqlite3"),
            &a.private.database_file_identity,
        )
        .unwrap();
        let p = if kind >= 2 {
            Some(
                PreparedRecord::decode(&read_bounded(
                    &f.receipt(ProviderBootstrapReceipt::Prepared),
                    PreparedRecord::MAX_BYTES as u64,
                ))
                .unwrap(),
            )
        } else {
            None
        };
        let state = database_io::read(&db, |c| validation::private(c, &a, p.as_ref())).unwrap();
        match kind {
            1 => assert!(
                matches!(
                    state,
                    database_io::ReadState::Uninitialized
                        | database_io::ReadState::Valid(
                            validation::PrivateValidation::Uninitialized
                        )
                ),
                "precommit initialization must not be accepted schema8"
            ),
            2 | 3 => {
                let expected = if kind == 2 {
                    MountPhase::Allocating
                } else {
                    MountPhase::ControlPrepared
                };
                assert!(
                    matches!(state,database_io::ReadState::Valid(validation::PrivateValidation::Initialized(m)) if m.phase==expected),
                    "precommit must reject committed phase"
                );
            }
            _ => unreachable!(),
        }
    }
}
pub(super) fn no_following_effects(f: &Fixture, kind: u8) {
    let a = stages(f);
    if kind <= 1 {
        assert_eq!(
            fs::metadata(a.stage_identity().unwrap().stage().join("FORMAT"))
                .unwrap()
                .len(),
            0
        );
        assert!(
            !f.receipt(ProviderBootstrapReceipt::Prepared)
                .try_exists()
                .unwrap()
        );
        assert!(!f.checkout.join(".layrs").try_exists().unwrap());
    }
    if kind == 2 {
        assert!(
            f.receipt(ProviderBootstrapReceipt::Prepared)
                .try_exists()
                .unwrap()
        );
        assert!(!f.checkout.join(".layrs").try_exists().unwrap());
    }
    if kind == 3 {
        assert!(f.checkout.join(".layrs/FORMAT").try_exists().unwrap());
    }
    if kind < 4 {
        assert!(
            !f.receipt(ProviderBootstrapReceipt::Abandoned)
                .try_exists()
                .unwrap()
        );
    }
    if kind == 4 {
        assert!(
            f.receipt(ProviderBootstrapReceipt::Abandoned)
                .try_exists()
                .unwrap()
        );
        assert!(f.metadata.join("projection.sqlite3").try_exists().unwrap());
        assert!(f.checkout.join(".layrs/FORMAT").try_exists().unwrap());
    }
}
