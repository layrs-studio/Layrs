use super::*;
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
};

// Observe the actual public caller's policy, then clamp only its test wait.
// Thread-local scope leaves parallel tests and all production builds untouched.
std::thread_local! {
    static POLICY_PROBE: RefCell<Option<Vec<(&'static str, Duration)>>> = const { RefCell::new(None) };
}

pub(super) fn budget_for_test(stage: &'static str, budget: Duration) -> Duration {
    POLICY_PROBE.with(|probe| match probe.borrow_mut().as_mut() {
        Some(selections) => {
            selections.push((stage, budget));
            Duration::from_millis(1)
        }
        None => budget,
    })
}

fn assert_caller_policy<T, E: std::fmt::Display>(
    stage: &'static str,
    seconds: u64,
    call: impl FnOnce() -> std::result::Result<T, E>,
) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            POLICY_PROBE.with(|probe| {
                probe.replace(None);
            });
        }
    }
    POLICY_PROBE.with(|probe| assert!(probe.replace(Some(vec![])).is_none()));
    let _reset = Reset;
    let error = call()
        .err()
        .expect("real held Store must refuse acquisition")
        .to_string();
    POLICY_PROBE.with(|probe| {
        assert_eq!(
            probe.borrow().as_ref().unwrap(),
            &[(stage, Duration::from_secs(seconds))],
        )
    });
    assert!(error.contains("Store acquisition timed out"), "{error}");
    assert!(error.contains("incompatible process-local lock"), "{error}");
}

#[test]
fn public_read_and_repository_callers_keep_ordinary_and_short_policies_distinct() {
    use super::super::NativeScmRepository;
    use layrs_store::scm::{CanonicalObject, ScmRepository, TransactionId, WriteBatch};
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "layrs-acquisition-policy-{}-{nonce}",
        std::process::id()
    ));
    let id = layrs_core::scm::StoreId::parse(format!("store:policy-{nonce}")).unwrap();
    drop(super::super::init(&root, id).unwrap());
    let held = super::super::open_exclusive(&root).unwrap();
    // Canonical preflight succeeds; no payload can be published while held.
    let batch = WriteBatch::new(
        TransactionId::new("transaction:policy-probe").unwrap(),
        vec![CanonicalObject::Tree(
            layrs_core::scm::Tree::new(vec![]).unwrap(),
        )],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    assert_caller_policy("ordinary Store read", 60, || {
        open_for_read_with_retry(&root)
    });
    assert_caller_policy("registration Store read", 5, || {
        open_for_registration_with_retry(&root)
    });
    assert_caller_policy("repository acceptance", 60, || {
        NativeScmRepository::new(&root).apply_batch(&batch)
    });
    let guard_calls = Cell::new(0);
    assert_caller_policy("repository acceptance", 5, || {
        NativeScmRepository::new(&root).apply_batch_guarded(&batch, || {
            guard_calls.set(guard_calls.get() + 1);
            Ok(())
        })
    });
    assert_eq!(guard_calls.get(), 0);
    assert_caller_policy("repository acceptance", 5, || {
        NativeScmRepository::for_materialization(&root, "recovery:probe", "owner:probe")
            .apply_batch(&batch)
    });
    assert_caller_policy("repository acceptance", 5, || {
        NativeScmRepository::new(&root)
            .with_short_acquisition()
            .apply_batch(&batch)
    });
    assert_caller_policy("short repository read", 5, || {
        super::super::repository::open_read_retry(&root)
    });
    assert_caller_policy("short repository write", 5, || {
        super::super::repository::open_exclusive_retry(&root)
    });
    drop(held);
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn timeout_preserves_stage_budget_elapsed_attempts_and_exact_contention() {
    for local in [false, true] {
        let calls = Cell::new(0u64);
        let path = PathBuf::from("fixture/metadata/layrs.lock");
        let budget = Duration::from_millis(35);
        let error = acquire::<()>("bounded test acceptance", budget, || {
            calls.set(calls.get() + 1);
            Err(NativeStoreError::Lock(if local {
                crate::StoreError::ReentrantLock(path.clone())
            } else {
                crate::StoreError::LockBusy(path.clone())
            }))
        })
        .unwrap_err();
        let description = error.to_string();
        let NativeStoreError::Io(error) = error else {
            panic!("{description}")
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let timeout = error
            .get_ref()
            .unwrap()
            .downcast_ref::<AcquisitionTimeout>()
            .unwrap();
        assert_eq!(timeout.stage, "bounded test acceptance");
        assert_eq!(timeout.budget, budget);
        assert!(timeout.elapsed >= budget);
        assert_eq!(timeout.attempts, calls.get());
        assert!(
            (1..=5).contains(&timeout.attempts),
            "backoff attempt count: {timeout:?}"
        );
        let source = std::error::Error::source(timeout)
            .unwrap()
            .downcast_ref::<NativeStoreError>()
            .unwrap();
        match source {
            NativeStoreError::Lock(crate::StoreError::LockBusy(actual)) if !local => {
                assert_eq!(actual, &path)
            }
            NativeStoreError::Lock(crate::StoreError::ReentrantLock(actual)) if local => {
                assert_eq!(actual, &path)
            }
            other => panic!("contention source changed: {other:?}"),
        }
        for field in [
            "bounded test acceptance",
            "budget=35ms",
            "elapsed=",
            "attempts=",
            "layrs.lock",
        ] {
            assert!(description.contains(field), "{description}");
        }
    }
}

#[test]
fn exact_contention_retries_the_acquisition_then_returns_its_success() {
    let mut calls = 0;
    let value = acquire("transient test read", Duration::from_millis(200), || {
        calls += 1;
        match calls {
            1 => Err(NativeStoreError::Lock(crate::StoreError::LockBusy(
                "lock".into(),
            ))),
            2 => Err(NativeStoreError::Lock(crate::StoreError::ReentrantLock(
                "lock".into(),
            ))),
            _ => Ok("validated capability"),
        }
    })
    .unwrap();
    assert_eq!(calls, 3);
    assert_eq!(value, "validated capability");
}

#[test]
fn non_contention_and_identity_errors_escape_on_the_first_attempt() {
    let expected = layrs_core::scm::StoreId::parse("store:expected").unwrap();
    let actual = layrs_core::scm::StoreId::parse("store:actual").unwrap();
    for error in [
        NativeStoreError::InvalidMarker("not ready".into()),
        NativeStoreError::StoreIdMismatch { expected, actual },
        NativeStoreError::Io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "guard refused",
        )),
        // The underlying OS kind alone must not trigger a retry.
        NativeStoreError::Lock(crate::StoreError::Io(io::Error::from(
            io::ErrorKind::WouldBlock,
        ))),
    ] {
        let description = error.to_string();
        let mut error = Some(error);
        let mut calls = 0;
        let returned = acquire::<()>("refusal", ORDINARY_WAIT, || {
            calls += 1;
            Err(error.take().expect("non-contention must not retry"))
        })
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(returned.to_string(), description);
    }
}

#[test]
fn existing_store_root_and_metadata_binding_refusals_are_not_retried() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("layrs-acquisition-{}-{nonce}", std::process::id()));
    let id = layrs_core::scm::StoreId::parse(format!("store:acquisition-{nonce}")).unwrap();
    drop(super::super::init(&root, id).unwrap());
    for metadata in [false, true] {
        let mut calls = 0;
        let error = acquire("bound read refusal", ORDINARY_WAIT, || {
            calls += 1;
            let store = super::super::open(&root)?;
            if metadata {
                store.with_metadata_identity("fs:v1:wrong")
            } else {
                store.with_filesystem_identity("fs:v1:wrong")
            }
            .map_err(NativeStoreError::Io)
        })
        .unwrap_err();
        assert_eq!(calls, 1);
        assert!(
            error
                .to_string()
                .contains("Store filesystem identity changed"),
            "{error}"
        );
        let NativeStoreError::Io(error) = error else {
            panic!("expected identity error")
        };
        assert_ne!(error.kind(), io::ErrorKind::TimedOut);
    }
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(root).unwrap();
}
