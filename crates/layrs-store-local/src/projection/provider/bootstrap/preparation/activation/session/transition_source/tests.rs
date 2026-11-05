//! Unrun native integration recipe: real source reads and a genuine intervening
//! Capture. This is not a fabricated publication proof or network authority.
use super::*;
use crate::projection::provider::{
    bootstrap::tests::capture_family::active_holder_with_writable_file_private,
    capture_lock::CaptureAttemptLock,
    database_io::tests::writer_support::{self, AdmissionPoint},
    process_profile::tests::child,
};
use std::{cell::Cell, fs, rc::Rc};

fn without_writer<T>(action: impl FnOnce() -> T) -> T {
    let writes = Rc::new(Cell::new(0));
    let observed = writes.clone();
    writer_support::observe(move |_, point| {
        if point == AdmissionPoint::BeforeWritableOpen {
            observed.set(observed.get() + 1);
            return Err(invalid("transition source read attempted a writer"));
        }
        Ok(())
    });
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            writer_support::clear_observer();
        }
    }
    let _reset = Reset;
    let result = action();
    assert_eq!(writes.get(), 0);
    result
}

#[test]
#[ignore = "UNQUALIFIED: ROOT source release and owned native child3600/outer3660 required; bounds unmeasured"]
fn transition_source_is_readonly_releases_gates_and_refuses_intervening_capture() {
    child::run(
        concat!(
            module_path!(),
            "::transition_source_is_readonly_releases_gates_and_refuses_intervening_capture"
        ),
        3600,
        || {
            child::declare();
            let (fixture, holder, _, _, _) = active_holder_with_writable_file_private();
            let mut session = holder
                .into_disconnected_session()
                .unwrap_or_else(|error| panic!("active session withheld: {}", error.cause()));
            let cancel = Cancellation::default();
            assert!(without_writer(|| session.transition_source(&cancel)).is_err());
            session.connect().unwrap();
            let original_requests = session.hydration_requests().len();
            let view = without_writer(|| session.transition_source(&cancel)).unwrap();
            assert_eq!(
                view.identity(),
                &session.source.held.artifacts.allocation.format.identity
            );
            assert_eq!(view.binding(), &session.source.held.binding);
            assert_eq!(view.capture_count(), 0);
            assert!(view.capture_head().is_none());
            assert!(view.captures().is_empty());
            assert_eq!(view.descriptors().len(), 1);
            assert_eq!(view.descriptors().next().unwrap().0.as_str(), "README.md");
            assert_eq!(
                view.versioned_snapshot().generation,
                session.source.activation.activated_generation
            );
            assert_eq!(
                view.versioned_snapshot().digest,
                session.source.activation.activated_digest
            );
            assert_eq!(
                view.selection_digest(),
                session.source.activation.selection_digest
            );
            assert_eq!(
                view.selection_snapshot_digest(),
                view.snapshot_parent_digest()
            );
            without_writer(|| view.revalidate(&cancel)).unwrap();
            assert_eq!(session.hydration_requests().len(), original_requests);

            let cancelled = Cancellation::default();
            cancelled.cancel();
            let refused = without_writer(|| session.transition_source(&cancelled))
                .err()
                .expect("precancel must refuse observation");
            assert!(matches!(
                refused.cause,
                Cause::Vfs(layrs_vfs::Error::Cancelled)
            ));
            let refused = without_writer(|| view.revalidate(&cancelled))
                .err()
                .expect("precancel must refuse revalidation");
            assert!(matches!(
                refused.cause,
                Cause::Vfs(layrs_vfs::Error::Cancelled)
            ));

            let path = fixture.checkout.join("README.md");
            if fs::remove_file(&path).is_err() {
                let mut permissions = fs::metadata(&path).unwrap().permissions();
                permissions.set_readonly(false);
                fs::set_permissions(&path, permissions).unwrap();
                fs::remove_file(&path).unwrap();
            }
            let changed = b"real locally authorized transition source Capture\n";
            fs::write(&path, changed).unwrap();
            // Local metadata did not change. This success deliberately proves
            // that callers still need a separate complete native-image witness.
            without_writer(|| view.revalidate(&cancel)).unwrap();

            // Exercise the real capture engine with the same retained source,
            // modelling another actor's durable change while our DATA view
            // remains borrowed. Its successful lock acquisition also proves
            // that transition_source did not retain the catalog/operation gates.
            let ack = {
                let held = &session.source.held;
                let attempt = CaptureAttemptLock::acquire(&held.artifacts, &held.family)
                    .unwrap_or_else(|_| panic!("capture attempt unexpectedly contended"));
                let mut checkpoint = session.session.checkpoint().unwrap();
                capture::run(
                    held,
                    &session.source.root,
                    &session.source.record,
                    &session.source.activation,
                    &mut checkpoint,
                    &attempt,
                    "transition-source-intervening-capture",
                    &cancel,
                )
                .unwrap()
            };
            assert_eq!(ack.patch.sequence, 1);
            let refused = without_writer(|| view.revalidate(&cancel))
                .err()
                .expect("old source predecessor must refuse the accepted capture");
            assert_eq!(refused.operation, "revalidate local transition predecessor");
            drop(view);
            let current = without_writer(|| session.transition_source(&cancel)).unwrap();
            assert_eq!(current.capture_count(), 1);
            assert_eq!(current.capture_head(), Some(ack.patch.digest.as_str()));
            assert_eq!(current.captures(), &[ack.patch]);
            without_writer(|| current.revalidate(&cancel)).unwrap();
            assert_eq!(fs::read(&path).unwrap(), changed);
            assert_eq!(session.hydration_requests().len(), original_requests);
            drop(current);
            session.disconnect().unwrap();
        },
    );
}
