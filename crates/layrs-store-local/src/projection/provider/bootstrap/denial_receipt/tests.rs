//! Component recipes only. Pure A/P/funding inputs are comparison DATA; these
//! filesystem tests do not construct a Provider owner, SQL proof or recovery
//! capability. ROOT qualifies the real driver and its observable boundaries.
use super::*;
use crate::projection::provider::{owned_test as child, recovery::error::ProviderBootstrapCause};

const TX: &str = "00000000000000000000000000000002";

#[test]
fn denial_receipt_temporary_names_are_bounded_nontraversing_and_cancellation_precedes_io() {
    let name = temporary_name(TX, "1-2-3").unwrap();
    assert!(name.starts_with("pd-start-"));
    assert!(name.ends_with(".tmp"));
    assert!(!name.contains('/'));
    assert!(!name.contains('\\'));
    assert!(
        temporary_name(
            TX,
            &format!("{}-{}-{}", "1".repeat(39), "2".repeat(10), "3".repeat(20))
        )
        .unwrap()
        .len()
            <= 122
    );
    for nonce in ["", "1-2", "1-2-3-4", "1--3", "1-2-../3", "a-2-3"] {
        assert!(temporary_name(TX, nonce).is_err(), "accepted {nonce}");
    }
    assert!(temporary_name("invalid", "1-2-3").is_err());
    let (allocation, prepared, funding, _) =
        crate::projection::shared::source_denial::release_test_data();
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(matches!(
        publish(&allocation, &prepared, &funding, Some(&cancel)),
        Err(ProviderBootstrapCause::Vfs(layrs_vfs::Error::Cancelled))
    ));
    assert!(matches!(
        reopen(&allocation, &prepared, Some(&cancel)),
        Err(ProviderBootstrapCause::Vfs(layrs_vfs::Error::Cancelled))
    ));
}

/// Exercise the actual file/selfbinding/verification primitive under a fresh
/// owned directory. Public publish/reopen additionally authenticate A's actual
/// metadata/control directory binding, supplied by the separate real driver.
fn component_receipt() -> (PinnedDirectory, PublishedDeniedStartReceipt) {
    let root = child::owned_root("pdreceipt");
    let root = PinnedDirectory::open(&root).unwrap();
    let parent = root
        .create_new_private_child(OsStr::new("receipt"))
        .unwrap();
    let mut temporary = parent
        .create_new_private_file(OsStr::new("component.tmp"))
        .unwrap();
    let (allocation, prepared, funding, _) =
        crate::projection::shared::source_denial::release_test_data();
    let record = PendingDeniedCoverageReleaseRecord::new(
        &allocation,
        &prepared,
        &funding,
        file::identity(temporary.file()).unwrap(),
    )
    .unwrap();
    let bytes = record.encode().unwrap();
    assert!(bytes.len() <= PendingDeniedCoverageReleaseRecord::MAX_BYTES);
    temporary.file_mut().write_all(&bytes).unwrap();
    temporary.file().sync_all().unwrap();
    assert_eq!(read_pinned(&temporary).unwrap(), record);
    assert!(!temporary.rename_to(&parent, OsStr::new(FILE_NAME)).unwrap());
    parent.sync().unwrap();
    let (file, actual) = read_slot(&parent, &allocation, &prepared, None)
        .unwrap()
        .unwrap();
    assert!(temporary.same_file(&file));
    assert_eq!(actual, record);
    let published = PublishedDeniedStartReceipt {
        parent: parent.try_clone().unwrap(),
        file,
        record,
    };
    published.verify(None).unwrap();
    (parent, published)
}

#[test]
#[ignore = "ROOT release and owned audited filesystem child120/outer180 required; unmeasured"]
fn denial_receipt_pinned_cleanup_removes_only_the_verified_final_inode() {
    child::run(
        concat!(
            module_path!(),
            "::denial_receipt_pinned_cleanup_removes_only_the_verified_final_inode"
        ),
        120,
        || {
            child::declare();
            let (parent, published) = component_receipt();
            let mut foreign = parent
                .create_new_private_file(OsStr::new("unrelated.tmp"))
                .unwrap();
            foreign
                .file_mut()
                .write_all(b"unrelated retained bytes")
                .unwrap();
            foreign.file().sync_all().unwrap();
            let foreign_id = file::identity(foreign.file()).unwrap();
            // The same no-replace primitive used by publish must leave an
            // authoritative final inode and the colliding temporary untouched.
            let mut collision = parent
                .create_new_private_file(OsStr::new("collision.tmp"))
                .unwrap();
            collision
                .file_mut()
                .write_all(b"retained collision")
                .unwrap();
            collision.file().sync_all().unwrap();
            assert!(collision.rename_to(&parent, OsStr::new(FILE_NAME)).unwrap());
            assert_eq!(
                read_bounded(collision.file()).unwrap(),
                b"retained collision"
            );
            published.verify(None).unwrap();
            published.cleanup(None).unwrap();
            assert_eq!(
                parent
                    .open_readonly_file(OsStr::new(FILE_NAME))
                    .unwrap_err()
                    .raw_os_error(),
                Some(2)
            );
            let retained = parent
                .open_readonly_file(OsStr::new("unrelated.tmp"))
                .unwrap();
            assert_eq!(file::identity(retained.file()).unwrap(), foreign_id);
            assert_eq!(
                read_bounded(retained.file()).unwrap(),
                b"unrelated retained bytes"
            );
        },
    );
}

#[test]
#[ignore = "ROOT release and owned audited filesystem child120/outer180 required; unmeasured"]
fn denial_receipt_replacement_with_identical_bytes_is_never_verified_or_unlinked() {
    child::run(
        concat!(
            module_path!(),
            "::denial_receipt_replacement_with_identical_bytes_is_never_verified_or_unlinked"
        ),
        120,
        || {
            child::declare();
            let (parent, published) = component_receipt();
            let bytes = published.record().encode().unwrap();
            let original_id = file::identity(published.file.file()).unwrap();
            assert!(
                !published
                    .file
                    .rename_to(&parent, OsStr::new("retained-original.json"))
                    .unwrap()
            );
            let mut replacement = parent
                .create_new_private_file(OsStr::new(FILE_NAME))
                .unwrap();
            replacement.file_mut().write_all(&bytes).unwrap();
            replacement.file().sync_all().unwrap();
            let replacement_id = file::identity(replacement.file()).unwrap();
            assert_ne!(original_id, replacement_id);
            assert!(read_pinned(&replacement).is_err());
            assert!(published.verify(None).is_err());
            assert!(published.cleanup(None).is_err());
            let final_file = parent.open_readonly_file(OsStr::new(FILE_NAME)).unwrap();
            assert_eq!(file::identity(final_file.file()).unwrap(), replacement_id);
            assert_eq!(read_bounded(final_file.file()).unwrap(), bytes);
            let original = parent
                .open_readonly_file(OsStr::new("retained-original.json"))
                .unwrap();
            assert_eq!(file::identity(original.file()).unwrap(), original_id);
        },
    );
}

#[test]
#[ignore = "ROOT release and owned audited filesystem child120/outer180 required; unmeasured"]
fn denial_receipt_byte_and_length_tamper_refuse_without_cleanup() {
    child::run(
        concat!(
            module_path!(),
            "::denial_receipt_byte_and_length_tamper_refuse_without_cleanup"
        ),
        120,
        || {
            child::declare();
            for length_tamper in [false, true] {
                let (parent, published) = component_receipt();
                let original_id = file::identity(published.file.file()).unwrap();
                let mut mutation = parent.open_file_for_sync(OsStr::new(FILE_NAME)).unwrap();
                if length_tamper {
                    mutation
                        .file()
                        .set_len(PendingDeniedCoverageReleaseRecord::MAX_BYTES as u64 + 1)
                        .unwrap();
                } else {
                    mutation.file_mut().seek(SeekFrom::Start(0)).unwrap();
                    mutation.file_mut().write_all(b"[").unwrap();
                }
                mutation.file().sync_all().unwrap();
                assert!(published.verify(None).is_err());
                assert!(published.cleanup(None).is_err());
                let retained = parent.open_readonly_file(OsStr::new(FILE_NAME)).unwrap();
                assert_eq!(file::identity(retained.file()).unwrap(), original_id);
            }
        },
    );
}
