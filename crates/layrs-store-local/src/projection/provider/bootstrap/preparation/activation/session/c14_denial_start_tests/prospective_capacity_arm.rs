//! Prospective-capacity arm of `run_start_configuration`: capacity refusals before funding, against a foreign destination parent. Borrows the caller's state; the caller tears the owners down after it returns.
use super::super::super::{
    managed_image::complete::VerifiedCompleteManagedImage,
    managed_journal::denial_start::PendingDenialStart,
};
use super::*;
use crate::projection::provider::records::{
    AllocationRecord, PreparedRecord, ProviderBootstrapBinding,
};
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    preimage_case: PreimageCase,
    cancel: &Cancellation,
    f: &Fixture,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    binding: &ProviderBootstrapBinding,
    session: &ProviderActiveSession,
    attempt: &CaptureAttemptLock,
    range_watch: &RangeWatch,
    image: &mut VerifiedCompleteManagedImage<'_, '_>,
    planned: &ProviderTransitionIntentRecord,
    plan_parent: &Path,
    observed: &TransitionObservationData,
    coverage: &DerivedDeniedCoverage,
    private_path: &Path,
    family_path: &Path,
    required: &[FileManifest],
    preimage: &FileManifest,
    immutable: &FileManifest,
) {
    let held = &session.source.held;
    use crate::projection::SharedQuota;
    let funding = crate::projection::shared::source_denial::SourceDeniedCoverageReservation::new(
        &allocation,
        &planned,
        coverage.record(),
        Some(&cancel),
    )
    .unwrap();
    let usage = f.family.usage().unwrap();
    // Only source denial d and this plan's future C quota q are tested;
    // a later source-companion charge is outside this boundary.
    let d = funding.reservation_bytes();
    let q = planned.plan.quota_bytes;
    assert!(d > 0 && q > 0);
    let used_and_reserved = usage
        .content_bytes
        .checked_add(usage.reserved_bytes)
        .unwrap();
    let threshold = used_and_reserved
        .checked_add(d)
        .unwrap()
        .checked_add(q)
        .unwrap();
    let insufficient = threshold - 1;
    assert!(
        used_and_reserved + d <= insufficient,
        "B alone must fit this negative quota"
    );
    let target = std::path::Path::new(&planned.plan.destination_parent)
        .join(&planned.plan.destination_leaf_nonce);
    let control = f.metadata.join("provider-control");
    let control_inventory = || {
        fs::read_dir(&control)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                assert!(entry.file_type().unwrap().is_file());
                (
                    entry.file_name(),
                    database_io::tests::support::witness(&entry.path()),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    let original_control = control_inventory();
    let private_before = database_io::tests::support::witness(&private_path);
    let family_before = database_io::tests::support::witness(&family_path);
    let other_parent = f.outer.join("r1-other-parent");
    fs::create_dir(&other_parent).unwrap();
    let error = super::super::super::managed_journal::denial_start::PendingDenialStart::prepare(
        held,
        session.session.guard_export_parent(&other_parent).unwrap(),
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        &attempt,
        &planned,
        &coverage,
        &observed,
        image,
        &required,
        &cancel,
    )
    .err()
    .expect("different real parent guard admitted");
    require_validation_message(error, "denial start retained destination parent differs");
    assert_eq!(
        database_io::tests::support::witness(&private_path),
        private_before
    );
    assert_eq!(
        database_io::tests::support::witness(&family_path),
        family_before
    );
    assert_eq!(control_inventory(), original_control);
    assert!(!target.exists());

    // Replace only the dedicated empty destination parent. The new guard
    // has the exact spelling but another inode, so only identity can reject.
    let retained_parent = f.outer.join("r1-original-parent");
    assert_eq!(plan_parent, f.outer.join("r1-destination-parent"));
    fs::rename(&plan_parent, &retained_parent).unwrap();
    fs::create_dir(&plan_parent).unwrap();
    let replacement = session.session.guard_export_parent(&plan_parent).unwrap();
    assert_ne!(
        replacement.filesystem_identity(),
        planned.plan.destination_parent_identity
    );
    let error = super::super::super::managed_journal::denial_start::PendingDenialStart::prepare(
        held,
        replacement,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        &attempt,
        &planned,
        &coverage,
        &observed,
        image,
        &required,
        &cancel,
    )
    .err()
    .expect("same-path replacement parent admitted");
    require_validation_message(error, "denial start retained destination parent differs");
    assert_eq!(
        database_io::tests::support::witness(&private_path),
        private_before
    );
    assert_eq!(
        database_io::tests::support::witness(&family_path),
        family_before
    );
    assert_eq!(control_inventory(), original_control);
    assert!(!target.exists());
    fs::remove_dir(&plan_parent).unwrap(); // empty fixture directory only
    fs::rename(&retained_parent, &plan_parent).unwrap();

    // A real guard from another Provider Session cannot stand in for the
    // source owner merely because its parent path and inode are correct.
    let (_foreign_fixture, foreign_holder, _, _, _) =
        capture_family::active_holder_with_source(None);
    let foreign_session = foreign_holder
        .into_disconnected_session()
        .unwrap_or_else(|error| panic!("foreign session: {}", error.cause()));
    let foreign_parent = foreign_session
        .session
        .guard_export_parent(&plan_parent)
        .unwrap();
    assert_eq!(
        foreign_parent.filesystem_identity(),
        planned.plan.destination_parent_identity
    );
    let error = super::super::super::managed_journal::denial_start::PendingDenialStart::prepare(
        held,
        foreign_parent,
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        &attempt,
        &planned,
        &coverage,
        &observed,
        image,
        &required,
        &cancel,
    )
    .err()
    .expect("foreign source Session parent admitted");
    assert!(
        matches!(error, Cause::Vfs(layrs_vfs::Error::IdentityMismatch)),
        "{error}"
    );
    assert_eq!(
        database_io::tests::support::witness(&private_path),
        private_before
    );
    assert_eq!(
        database_io::tests::support::witness(&family_path),
        family_before
    );
    assert_eq!(control_inventory(), original_control);
    assert!(!target.exists());

    f.family
        .set_quota(SharedQuota {
            bytes: insufficient,
        })
        .unwrap();
    let before_low = catalog_snapshot(&f, &cancel);
    let family_before_low = database_io::tests::support::witness(&family_path);
    let error = super::super::super::managed_journal::denial_start::PendingDenialStart::prepare(
        held,
        session
            .session
            .guard_export_parent(std::path::Path::new(&planned.plan.destination_parent))
            .unwrap(),
        &session.source.root,
        &session.source.record,
        &session.source.activation,
        &attempt,
        &planned,
        &coverage,
        &observed,
        image,
        &required,
        &cancel,
    )
    .err()
    .expect("unfundable d+q admitted because d alone fits");
    require_validation_message(
        error,
        "partial_store_quota: denial source and future destination unfunded",
    );
    assert_eq!(
        database_io::tests::support::witness(&private_path),
        private_before
    );
    assert_eq!(
        database_io::tests::support::witness(&family_path),
        family_before_low
    );
    assert_eq!(catalog_snapshot(&f, &cancel), before_low);
    assert_eq!(control_inventory(), original_control);
    assert!(
        denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
            .unwrap()
            .is_none()
    );
    assert!(!target.exists());
    assert_eq!(
        f.family.usage().unwrap().reserved_bytes,
        usage.reserved_bytes
    );

    f.family
        .set_quota(SharedQuota { bytes: threshold })
        .unwrap();
    let family_at_threshold = database_io::tests::support::witness(&family_path);
    let mut pending =
        super::super::super::managed_journal::denial_start::PendingDenialStart::prepare(
            held,
            session
                .session
                .guard_export_parent(std::path::Path::new(&planned.plan.destination_parent))
                .unwrap(),
            &session.source.root,
            &session.source.record,
            &session.source.activation,
            &attempt,
            &planned,
            &coverage,
            &observed,
            image,
            &required,
            &cancel,
        )
        .unwrap();
    assert_eq!(
        database_io::tests::support::witness(&private_path),
        private_before
    );
    assert_eq!(
        database_io::tests::support::witness(&family_path),
        family_at_threshold
    );
    assert_eq!(control_inventory(), original_control);
    assert!(!target.exists());

    // A quota change after prepare must be rechecked before receipt/B.
    f.family
        .set_quota(SharedQuota {
            bytes: insufficient,
        })
        .unwrap();
    let family_before_begin = database_io::tests::support::witness(&family_path);
    let catalog_before_begin = catalog_snapshot(&f, &cancel);
    let mut family_close = Close::default();
    let mut source_close = Close::default();
    let error = pending
        .begin(&mut family_close, &mut source_close, &cancel)
        .unwrap_err();
    require_validation_message(
        error,
        "partial_store_quota: denial source and future destination unfunded",
    );
    assert_eq!(
        (
            family_close.opened,
            family_close.closed,
            source_close.opened,
            source_close.closed
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        database_io::tests::support::witness(&private_path),
        private_before
    );
    assert_eq!(
        database_io::tests::support::witness(&family_path),
        family_before_begin
    );
    assert_eq!(catalog_snapshot(&f, &cancel), catalog_before_begin);
    assert_eq!(control_inventory(), original_control);
    assert!(
        denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
            .unwrap()
            .is_none()
    );
    assert!(!target.exists());

    f.family
        .set_quota(SharedQuota { bytes: threshold })
        .unwrap();
    // Stop at the real family COMMIT to exercise capacity with B already
    // reserved but Planned absent. Re-adding d would exceed this exact limit.
    let refused = Rc::new(Cell::new(false));
    let mut family_close = Close::default();
    family_close.funded = Some(range_watch.funded.clone());
    family_close.refused = Some(refused.clone());
    let mut source_close = Close::default();
    let error = pending
        .begin(&mut family_close, &mut source_close, &cancel)
        .unwrap_err();
    require_validation_message(error, "C14 start SQLite close ACK withheld");
    assert!(refused.get());
    assert_eq!((family_close.opened, family_close.closed), (1, 1));
    assert_eq!((source_close.opened, source_close.closed), (0, 0));
    assert_eq!(
        database_io::tests::support::witness(&private_path),
        private_before
    );
    assert!(!target.exists());
    let funded_usage = f.family.usage().unwrap();
    assert_eq!(funded_usage.content_bytes, usage.content_bytes);
    assert_eq!(funded_usage.reserved_bytes, usage.reserved_bytes + d);
    assert_eq!(
        funded_usage.content_bytes + funded_usage.reserved_bytes + q,
        threshold
    );
    assert!(
        denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
            .unwrap()
            .is_some()
    );
    let family_funded = database_io::tests::support::witness(&family_path);
    let mut family_close = Close::default();
    let mut source_close = Close::default();
    let source_refused = Rc::new(Cell::new(false));
    source_close.refused = Some(source_refused.clone());
    require_validation_message(
        pending
            .begin(&mut family_close, &mut source_close, &cancel)
            .unwrap_err(),
        "C14 start SQLite close ACK withheld",
    );
    assert!(source_refused.get());
    assert_eq!((family_close.opened, family_close.closed), (0, 0));
    assert_eq!((source_close.opened, source_close.closed), (1, 1));
    assert_eq!(
        database_io::tests::support::witness(&family_path),
        family_funded
    );
    let actual = super::super::super::managed_destination::valid(
        database_io::read_cancellable_strict(&held.artifacts.database, &cancel, |c| {
            validation::private_active_transition_with_source(
                c,
                &allocation,
                &prepared,
                &binding,
                Some(session.source.root.registration().root_id()),
                Some(&cancel),
            )
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(actual.transitions, std::slice::from_ref(planned));
    assert!(
        denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
            .unwrap()
            .is_some()
    );
    let private_committed = database_io::tests::support::witness(&private_path);
    // Planned is already durable. Lowering prospective C capacity must not
    // prevent this exact no-writer replay or cleanup of the compact receipt.
    f.family
        .set_quota(SharedQuota {
            bytes: insufficient,
        })
        .unwrap();
    let family_lowered = database_io::tests::support::witness(&family_path);
    let usage_lowered = f.family.usage().unwrap();
    let mut family_close = Close::default();
    let mut source_close = Close::default();
    assert_eq!(
        pending
            .begin(&mut family_close, &mut source_close, &cancel)
            .unwrap(),
        SourceIntentEffect::AlreadyPresent
    );
    assert_eq!(
        (
            family_close.opened,
            family_close.closed,
            source_close.opened,
            source_close.closed
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        database_io::tests::support::witness(&private_path),
        private_committed
    );
    assert_eq!(
        database_io::tests::support::witness(&family_path),
        family_lowered
    );
    assert_eq!(f.family.usage().unwrap(), usage_lowered);
    assert_eq!(control_inventory(), original_control);
    assert!(
        denial_receipt::reopen(&allocation, &prepared, Some(&cancel))
            .unwrap()
            .is_none()
    );
    assert!(!target.exists());
    f.family
        .set_quota(SharedQuota { bytes: threshold })
        .unwrap();
    assert_eq!(f.family.usage().unwrap(), funded_usage);
    let private_before_foreign = database_io::tests::support::witness(&private_path);
    let family_before_foreign = database_io::tests::support::witness(&family_path);
    let failure = super::super::super::managed_allocation::allocate_denial(
        &foreign_session.session,
        held,
        &mut pending,
        &planned,
        &cancel,
    )
    .err()
    .expect("foreign allocation Session created C");
    assert!(
        matches!(
            &failure.cause,
            Cause::Vfs(layrs_vfs::Error::IdentityMismatch)
        ),
        "{}",
        failure.cause
    );
    assert!(failure.effects.is_empty());
    assert!(!target.exists());
    assert_eq!(
        database_io::tests::support::witness(&private_path),
        private_before_foreign
    );
    assert_eq!(
        database_io::tests::support::witness(&family_path),
        family_before_foreign
    );
    drop(failure);
    drop(foreign_session);
    drop(pending);
    require_retained_preimage(&image, preimage_case, preimage, immutable);
    assert_eq!(range_watch.late.load(Ordering::SeqCst), 0);
}
