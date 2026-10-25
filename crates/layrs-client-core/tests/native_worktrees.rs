use layrs_client_core::native_scm::*;
use layrs_core::scm::*;
use layrs_store::scm::ScmReader;
use layrs_store_local::scm_v1::{self, NativeScmRepository};
use std::{fs, io, path::PathBuf, sync::Barrier};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("layrs-worktrees-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn directory(&self, name: &str) -> PathBuf {
        let root = self.0.join(name);
        fs::create_dir(&root).unwrap();
        root
    }
    fn primary(&self) -> NativeWorkspace {
        let root = self.directory("primary");
        fs::write(root.join("same.txt"), b"base\0bytes").unwrap();
        NativeWorkspace::initialize(&root, "Shared Worktrees").unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn own_state(workspace: &NativeWorkspace) -> WorktreeViewState {
    workspace
        .state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == workspace.worktree_id())
        .unwrap()
        .clone()
}
fn captured(workspace: &NativeWorkspace) -> StepId {
    match workspace.capture().unwrap() {
        CapturePlan::Change(change) => change.step_id().clone(),
        CapturePlan::Unchanged { step_id } => step_id,
    }
}
fn assert_clean(workspace: &NativeWorkspace, protected: &[StepId]) {
    let report =
        scm_v1::fsck_existing(workspace.store_root(), scm_v1::FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{:?}", report.findings);
    assert!(!report.is_truncated());
    for step in protected {
        assert!(!report
            .gc_reachability
            .unreachable_objects
            .contains(&step.to_string()));
    }
}

#[test]
fn independent_captures_views_layers_and_retention_share_one_store() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let initial = own_state(&primary);
    let base = initial.base_step_id().unwrap().clone();
    let main = initial.active_layer_id().unwrap().clone();
    let alt = primary.create_layer("alternate").unwrap();
    let primary_operation = primary.state().unwrap().operation.id().clone();
    let secondary_root = fixture.directory("secondary");
    let secondary = primary
        .create_worktree(&secondary_root, &main, &base)
        .unwrap();
    assert_ne!(primary.worktree_id(), secondary.worktree_id());
    assert_eq!(primary.store_root(), secondary.store_root());
    assert_ne!(
        primary.worktree_id().as_str().strip_prefix("worktree:"),
        primary
            .state()
            .unwrap()
            .worktree
            .store_id()
            .as_str()
            .strip_prefix("store:")
    );
    assert_eq!(primary.state().unwrap().operation.id(), &primary_operation);
    assert_eq!(own_state(&secondary).base_step_id(), Some(&base));
    assert_eq!(primary.list_worktrees().unwrap().len(), 2);
    fs::write(primary.checkout_root().unwrap().join("same.txt"), b"left").unwrap();
    let left = captured(&primary);
    assert_eq!(
        fs::read(secondary_root.join("same.txt")).unwrap(),
        b"base\0bytes"
    );
    fs::write(secondary_root.join("same.txt"), b"right").unwrap();
    let right = captured(&secondary);
    let state = secondary.state().unwrap();
    let heads = state
        .view
        .layer_heads()
        .iter()
        .find(|h| h.layer_id() == &main)
        .unwrap();
    assert_eq!(heads.step_ids().len(), 2);
    assert!(heads.step_ids().contains(&left) && heads.step_ids().contains(&right));
    let repository = NativeScmRepository::new(primary.store_root());
    assert_eq!(
        repository
            .get_step(&left)
            .unwrap()
            .unwrap()
            .parent_step_ids(),
        &[base.clone()]
    );
    assert_eq!(
        repository
            .get_step(&right)
            .unwrap()
            .unwrap()
            .parent_step_ids(),
        &[base.clone()]
    );
    let primary_before_switch = primary.state().unwrap().operation.id().clone();
    secondary.switch_layer(alt.id().as_str()).unwrap();
    assert_eq!(own_state(&secondary).active_layer_id(), Some(alt.id()));
    assert_eq!(own_state(&primary).active_layer_id(), Some(&main));
    assert_eq!(
        primary.state().unwrap().operation.id(),
        &primary_before_switch
    );
    assert_eq!(
        fs::read(primary.checkout_root().unwrap().join("same.txt")).unwrap(),
        b"left"
    );
    assert_eq!(
        fs::read(secondary_root.join("same.txt")).unwrap(),
        b"base\0bytes"
    );
    assert!(
        secondary.switch_layer(main.as_str()).is_err(),
        "divergent Layer requires explicit integration"
    );
    assert_clean(&primary, &[base, left, right]);
    assert_eq!(diagnose_checkout(&secondary_root).unwrap()["clean"], true);
}

#[test]
fn concurrent_different_worktrees_preserve_both_operation_histories() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own_state(&primary);
    let secondary = primary
        .create_worktree(
            fixture.directory("secondary"),
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
        )
        .unwrap();
    let root_a = primary.checkout_root().unwrap();
    let root_b = secondary.checkout_root().unwrap();
    fs::write(root_a.join("same.txt"), b"concurrent left").unwrap();
    fs::write(root_b.join("same.txt"), b"concurrent right").unwrap();
    // Prepare from exactly the same global frontier to force a CAS conflict.
    let a = primary
        .client()
        .prepare_disk_capture(
            primary.worktree_id(),
            SnapshotLimits::streaming(),
            MutationStamp::fresh().unwrap(),
        )
        .unwrap();
    let b = secondary
        .client()
        .prepare_disk_capture(
            secondary.worktree_id(),
            SnapshotLimits::streaming(),
            MutationStamp::fresh().unwrap(),
        )
        .unwrap();
    let (CapturePlan::Change(a), CapturePlan::Change(b)) = (a, b) else {
        panic!("expected captures");
    };
    let barrier = Barrier::new(2);
    let outcomes = std::thread::scope(|scope| {
        let a_job = scope.spawn(|| {
            barrier.wait();
            primary.client().accept(&a)
        });
        let b_job = scope.spawn(|| {
            barrier.wait();
            secondary.client().accept(&b)
        });
        (a_job.join().unwrap(), b_job.join().unwrap())
    });
    assert_ne!(outcomes.0.is_ok(), outcomes.1.is_ok());
    let error = if let Err(e) = &outcomes.0 {
        e
    } else {
        outcomes.1.as_ref().unwrap_err()
    };
    assert_eq!(error.code(), "cas_mismatch");
    // Re-read and recapture the rejected Worktree, preserving the accepted one.
    let left = captured(&primary);
    let right = captured(&secondary);
    assert_ne!(left, right);
    let repository = NativeScmRepository::new(primary.store_root());
    let heads = repository
        .operation_heads(primary.state().unwrap().space.id())
        .unwrap();
    assert_eq!(heads.len(), 2);
    assert!(heads.contains(primary.state().unwrap().operation.id()));
    assert!(heads.contains(secondary.state().unwrap().operation.id()));
    assert_eq!(
        NativeWorkspace::open(&root_a).unwrap().worktree_id(),
        primary.worktree_id()
    );
    assert_eq!(
        NativeWorkspace::open(&root_b).unwrap().worktree_id(),
        secondary.worktree_id()
    );
    assert_clean(&primary, &[left, right]);
}

#[test]
fn locator_spoof_and_replaced_root_are_rejected_without_touching_bytes() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own_state(&primary);
    let root = fixture.directory("secondary");
    let secondary = primary
        .create_worktree(
            &root,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
        )
        .unwrap();
    let locator = fs::read(root.join(".layrs")).unwrap();
    let spoof = fixture.directory("spoof");
    fs::write(spoof.join(".layrs"), &locator).unwrap();
    assert!(NativeWorkspace::open(&spoof).is_err());
    let mut edited: serde_json::Value = serde_json::from_slice(&locator).unwrap();
    edited["worktreeId"] = serde_json::to_value(primary.worktree_id()).unwrap();
    fs::write(root.join(".layrs"), serde_json::to_vec(&edited).unwrap()).unwrap();
    assert!(NativeWorkspace::open(&root).is_err());
    fs::write(root.join(".layrs"), &locator).unwrap();
    let old = fixture.0.join("preserved-original");
    drop(secondary);
    fs::rename(&root, &old).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(root.join(".layrs"), &locator).unwrap();
    fs::write(root.join("same.txt"), b"external replacement").unwrap();
    assert!(NativeWorkspace::open(&root).is_err());
    assert_eq!(
        fs::read(root.join("same.txt")).unwrap(),
        b"external replacement"
    );
    assert_eq!(fs::read(old.join("same.txt")).unwrap(), b"base\0bytes");
}

#[test]
fn nonempty_or_nested_roots_are_refused_and_discovery_validates_locator() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own_state(&primary);
    let occupied = fixture.directory("occupied");
    fs::write(occupied.join("valuable"), b"external").unwrap();
    assert!(primary
        .create_worktree(
            &occupied,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap()
        )
        .is_err());
    assert_eq!(fs::read(occupied.join("valuable")).unwrap(), b"external");
    let nested = primary.checkout_root().unwrap().join("nested");
    fs::create_dir(&nested).unwrap();
    assert!(primary
        .create_worktree(
            &nested,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap()
        )
        .is_err());
    #[cfg(windows)]
    assert!(
        primary
            .create_worktree(
                PathBuf::from(nested.to_string_lossy().to_uppercase()),
                state.active_layer_id().unwrap(),
                state.base_step_id().unwrap(),
            )
            .is_err(),
        "case aliases must not bypass filesystem ancestry validation"
    );
    assert_eq!(primary.list_worktrees().unwrap().len(), 1);
    let root = fixture.directory("secondary");
    let secondary = primary
        .create_worktree(
            &root,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
        )
        .unwrap();
    fs::create_dir(root.join("child")).unwrap();
    assert_eq!(
        NativeWorkspace::discover(root.join("child"))
            .unwrap()
            .worktree_id(),
        secondary.worktree_id()
    );
    #[cfg(unix)]
    {
        let link = fixture.0.join("alias");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert!(NativeWorkspace::open(&link).is_err());
        assert!(NativeWorkspace::discover(link.join("child")).is_err());
    }
}

struct Stop(scm_v1::materialize::MaterializationPoint);
impl scm_v1::materialize::MaterializationFaults for Stop {
    fn check(&self, point: scm_v1::materialize::MaterializationPoint) -> io::Result<()> {
        if point == self.0 {
            Err(io::Error::other("creation interrupted"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn failed_creation_is_stale_and_recoverable_without_claiming_ready() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own_state(&primary);
    let root = fixture.directory("secondary");
    assert!(primary
        .create_worktree_with_faults(
            &root,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
            &Stop(scm_v1::materialize::MaterializationPoint::AfterJournal)
        )
        .is_err());
    let secondary = NativeWorkspace::open(&root).unwrap();
    assert_eq!(own_state(&secondary).status(), WorktreeStatus::Stale);
    assert!(secondary.needs_repair().unwrap());
    assert!(secondary.capture().is_err());
    secondary.recover().unwrap();
    secondary.resume_worktree_creation().unwrap();
    assert_eq!(own_state(&secondary).status(), WorktreeStatus::Ready);
    assert_eq!(fs::read(root.join("same.txt")).unwrap(), b"base\0bytes");
    assert_clean(&primary, &[state.base_step_id().unwrap().clone()]);
}

#[test]
fn reopen_after_an_independent_process_exits() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own_state(&primary);
    let root = fixture.directory("secondary");
    let secondary = primary
        .create_worktree(
            &root,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
        )
        .unwrap();
    let id = secondary.worktree_id().clone();
    drop(secondary);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "process_worker", "--nocapture"])
        .env("LAYRS_WORKTREE_TEST_ROOT", &root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let reopened = NativeWorkspace::open(&root).unwrap();
    assert_eq!(reopened.worktree_id(), &id);
    assert_eq!(
        fs::read(root.join("same.txt")).unwrap(),
        b"child process capture"
    );
    assert!(matches!(
        reopened.capture().unwrap(),
        CapturePlan::Unchanged { .. }
    ));
    assert_clean(
        &primary,
        &[own_state(&reopened).base_step_id().unwrap().clone()],
    );
}
#[test]
fn process_worker() {
    let Some(root) = std::env::var_os("LAYRS_WORKTREE_TEST_ROOT") else {
        return;
    };
    let workspace = NativeWorkspace::open(PathBuf::from(root)).unwrap();
    fs::write(
        workspace.checkout_root().unwrap().join("same.txt"),
        b"child process capture",
    )
    .unwrap();
    workspace.capture().unwrap();
}

struct StopAfterLocator;

#[test]
fn explicit_same_space_layer_can_be_selected_from_another_worktrees_history() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own_state(&primary);
    let secondary = primary
        .create_worktree(
            fixture.directory("secondary"),
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
        )
        .unwrap();
    let layer = primary.create_layer("later layer").unwrap();
    assert!(!secondary
        .state()
        .unwrap()
        .layers
        .iter()
        .any(|l| l.id() == layer.id()));
    let third = secondary
        .create_worktree(
            fixture.directory("third"),
            layer.id(),
            state.base_step_id().unwrap(),
        )
        .unwrap();
    assert_eq!(own_state(&third).active_layer_id(), Some(layer.id()));
    assert_eq!(own_state(&third).base_step_id(), state.base_step_id());
    assert_eq!(primary.list_worktrees().unwrap().len(), 3);
    assert_clean(&primary, &[state.base_step_id().unwrap().clone()]);
}

impl WorktreeCreationFaults for StopAfterLocator {
    fn after_locator(&self) -> io::Result<()> {
        Err(io::Error::other("reservation interrupted"))
    }
}
#[test]
fn reservation_before_registration_resumes_exact_identity_and_base() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own_state(&primary);
    let root = fixture.directory("secondary");
    assert!(primary
        .create_worktree_with_creation_faults(
            &root,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
            &scm_v1::materialize::NoMaterializationFaults,
            &StopAfterLocator
        )
        .is_err());
    assert!(NativeWorkspace::open(&root).is_err());
    assert_eq!(primary.list_worktrees().unwrap().len(), 1);
    let locator = scm_v1::worktrees::read_locator(&root).unwrap();
    fs::write(
        primary.checkout_root().unwrap().join("same.txt"),
        b"source advanced after reservation",
    )
    .unwrap();
    let advanced = captured(&primary);
    let resumed = NativeWorkspace::resume_worktree(&root).unwrap();
    assert_eq!(resumed.worktree_id(), &locator.worktree_id);
    assert_eq!(own_state(&resumed).base_step_id(), state.base_step_id());
    assert_eq!(fs::read(root.join("same.txt")).unwrap(), b"base\0bytes");
    assert_eq!(
        fs::read(primary.checkout_root().unwrap().join("same.txt")).unwrap(),
        b"source advanced after reservation"
    );
    assert_eq!(
        NativeWorkspace::resume_worktree(&root)
            .unwrap()
            .worktree_id(),
        resumed.worktree_id()
    );
    assert_clean(&primary, &[advanced, state.base_step_id().unwrap().clone()]);
}

struct CaptureAfterLocator<'a>(&'a NativeWorkspace);
impl WorktreeCreationFaults for CaptureAfterLocator<'_> {
    fn after_locator(&self) -> io::Result<()> {
        fs::write(
            self.0
                .checkout_root()
                .map_err(io::Error::other)?
                .join("same.txt"),
            b"source raced",
        )?;
        self.0.capture().map_err(io::Error::other)?;
        Ok(())
    }
}
#[test]
fn source_advance_between_preflight_and_registration_is_cas_rejected() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own_state(&primary);
    let root = fixture.directory("secondary");
    let result = primary.create_worktree_with_creation_faults(
        &root,
        state.active_layer_id().unwrap(),
        state.base_step_id().unwrap(),
        &scm_v1::materialize::NoMaterializationFaults,
        &CaptureAfterLocator(&primary),
    );
    assert_eq!(result.err().unwrap().code(), "cas_mismatch");
    assert_eq!(primary.list_worktrees().unwrap().len(), 1);
    assert_ne!(own_state(&primary).base_step_id(), state.base_step_id());
    let resumed = NativeWorkspace::resume_worktree(&root).unwrap();
    assert_eq!(own_state(&resumed).base_step_id(), state.base_step_id());
    assert_eq!(fs::read(root.join("same.txt")).unwrap(), b"base\0bytes");
}

struct RegistrationBarrier(Barrier);
impl WorktreeCreationFaults for RegistrationBarrier {
    fn after_locator(&self) -> io::Result<()> {
        self.0.wait();
        Ok(())
    }
    fn after_registration(&self) -> io::Result<()> {
        Err(io::Error::other("stop after registered creation"))
    }
}
#[test]
fn concurrent_registrations_preserve_capture_cas_and_supported_resumption() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own_state(&primary);
    let source_operation = primary.state().unwrap().operation.id().clone();
    let unrelated = primary
        .create_worktree(
            fixture.directory("unrelated"),
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
        )
        .unwrap();
    fs::write(
        unrelated.checkout_root().unwrap().join("same.txt"),
        b"unrelated history",
    )
    .unwrap();
    let unrelated_step = captured(&unrelated);
    let unrelated_operation = unrelated.state().unwrap().operation.id().clone();
    fs::write(
        unrelated.checkout_root().unwrap().join("same.txt"),
        b"pending independent capture",
    )
    .unwrap();
    let CapturePlan::Change(pending_capture) = unrelated
        .client()
        .prepare_disk_capture(
            unrelated.worktree_id(),
            SnapshotLimits::streaming(),
            MutationStamp::fresh().unwrap(),
        )
        .unwrap()
    else {
        panic!("expected independent capture");
    };
    let a = fixture.directory("a");
    let b = fixture.directory("b");
    let barrier = RegistrationBarrier(Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let a_job = scope.spawn(|| {
            primary.create_worktree_with_creation_faults(
                &a,
                state.active_layer_id().unwrap(),
                state.base_step_id().unwrap(),
                &scm_v1::materialize::NoMaterializationFaults,
                &barrier,
            )
        });
        let b_job = scope.spawn(|| {
            primary.create_worktree_with_creation_faults(
                &b,
                state.active_layer_id().unwrap(),
                state.base_step_id().unwrap(),
                &scm_v1::materialize::NoMaterializationFaults,
                &barrier,
            )
        });
        (a_job.join().unwrap(), b_job.join().unwrap())
    });
    let errors = [&results.0, &results.1].map(|result| result.as_ref().err());
    eprintln!("concurrent registration results: {errors:?}");
    assert!(
        errors.iter().all(Option::is_some),
        "both creations stop after registration: {errors:?}"
    );
    for error in errors.into_iter().flatten() {
        assert!(
            matches!(error, NativeClientError::Filesystem(e) if e.to_string() == "stop after registered creation"),
            "unexpected creation error: {error:?}"
        );
    }
    let locators = [&a, &b].map(|root| scm_v1::worktrees::read_locator(root).unwrap());
    let store = primary.open_store().unwrap();
    let reader = scm_v1::NativeScmReader::new(&store).unwrap();
    let registered = locators
        .iter()
        .filter_map(|locator| reader.get_worktree(&locator.worktree_id).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        registered.len(),
        2,
        "both namespace-serialized registrations committed before materialization"
    );
    assert_eq!(
        scm_v1::worktrees::registered_worktrees(&store)
            .unwrap()
            .len(),
        4
    );
    let heads = reader.operation_heads(registered[0].space_id()).unwrap();
    assert_eq!(heads.len(), 3);
    assert!(heads.contains(&unrelated_operation));
    for worktree in &registered {
        assert!(heads.contains(worktree.active_operation_id()));
    }
    drop(reader);
    drop(store);
    let stale_capture = unrelated.client().accept(&pending_capture).unwrap_err();
    assert!(
        matches!(
            stale_capture,
            NativeClientError::Store(layrs_store::scm::StoreError::CasMismatch { .. })
        ),
        "{stale_capture:?}"
    );
    assert_eq!(primary.state().unwrap().operation.id(), &source_operation);
    assert_eq!(own_state(&primary), state);
    assert_eq!(
        unrelated.state().unwrap().operation.id(),
        &unrelated_operation
    );

    // Both exact locators resume their registered Stale state. The independently
    // prepared capture above must be explicitly recaptured after losing CAS.
    for (root, locator) in [&a, &b].into_iter().zip(&locators) {
        let workspace = NativeWorkspace::resume_worktree(root).unwrap();
        assert_eq!(workspace.worktree_id(), &locator.worktree_id);
        assert_eq!(
            workspace.state().unwrap().worktree.filesystem_identity(),
            locator.filesystem_identity
        );
        assert_eq!(own_state(&workspace).status(), WorktreeStatus::Ready);
        assert_eq!(own_state(&workspace).base_step_id(), state.base_step_id());
        let store = workspace.open_store().unwrap();
        let reader = scm_v1::NativeScmReader::new(&store).unwrap();
        let registration = reader
            .get_operation(locator.creation.worktree.active_operation_id())
            .unwrap()
            .unwrap();
        assert_eq!(
            registration.parent_operation_ids(),
            &[source_operation.clone()]
        );
        assert_eq!(fs::read(root.join("same.txt")).unwrap(), b"base\0bytes");
    }
    assert_eq!(primary.list_worktrees().unwrap().len(), 4);
    let store = primary.open_store().unwrap();
    let reader = scm_v1::NativeScmReader::new(&store).unwrap();
    let heads = reader.operation_heads(registered[0].space_id()).unwrap();
    assert_eq!(heads.len(), 3);
    assert!(heads.contains(&unrelated_operation));
    for locator in &locators {
        let worktree = reader.get_worktree(&locator.worktree_id).unwrap().unwrap();
        assert!(heads.contains(worktree.active_operation_id()));
    }
    drop(reader);
    drop(store);
    assert_eq!(own_state(&primary), state);
    assert_eq!(
        unrelated.state().unwrap().operation.id(),
        &unrelated_operation
    );
    assert_eq!(
        fs::read(unrelated.checkout_root().unwrap().join("same.txt")).unwrap(),
        b"pending independent capture"
    );
    let recaptured = captured(&unrelated);
    assert_ne!(recaptured, unrelated_step);
    assert_clean(
        &primary,
        &[
            state.base_step_id().unwrap().clone(),
            unrelated_step,
            recaptured,
        ],
    );
}

struct ExternalCollision(PathBuf);
impl scm_v1::materialize::MaterializationFaults for ExternalCollision {
    fn check(&self, point: scm_v1::materialize::MaterializationPoint) -> io::Result<()> {
        if point == scm_v1::materialize::MaterializationPoint::AfterJournal {
            fs::write(self.0.join("same.txt"), b"valuable external collision")?;
        }
        Ok(())
    }
}
#[test]
fn external_collision_during_creation_is_preserved_and_never_ready() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own_state(&primary);
    let root = fixture.directory("secondary");
    assert!(primary
        .create_worktree_with_faults(
            &root,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
            &ExternalCollision(root.clone())
        )
        .is_err());
    assert_eq!(
        fs::read(root.join("same.txt")).unwrap(),
        b"valuable external collision"
    );
    let secondary = NativeWorkspace::open(&root).unwrap();
    assert_eq!(own_state(&secondary).status(), WorktreeStatus::Stale);
    assert!(secondary.capture().is_err());
    assert!(secondary.resume_worktree_creation().is_err());
}

#[test]
fn shared_worktree_divergence_produces_durable_explicit_conflicts() {
    use layrs_weave::{
        graph_merge::GraphMergeLimits,
        reconciliation::{FailureAction, FileMergeLimits, FileMergePolicy},
    };
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own_state(&primary);
    let root = fixture.directory("secondary");
    let secondary = primary
        .create_worktree(
            &root,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
        )
        .unwrap();
    fs::write(primary.checkout_root().unwrap().join("same.txt"), b"left").unwrap();
    let left = captured(&primary);
    fs::write(root.join("same.txt"), b"right").unwrap();
    let right = captured(&secondary);
    let secondary_operation = secondary.state().unwrap().operation.id().clone();
    let preview = primary
        .preview_structural_merge(
            &[right.clone()],
            state.active_layer_id().unwrap(),
            FileMergePolicy {
                unsupported: FailureAction::StructuralFallback,
                quota: FailureAction::Reject,
                limits: FileMergeLimits::default(),
            },
            GraphMergeLimits::default(),
        )
        .unwrap();
    let accepted = primary.accept_merge(&preview).unwrap();
    assert!(!accepted.conflict_ids.is_empty());
    assert!(!accepted.materialized);
    assert_eq!(own_state(&primary).status(), WorktreeStatus::Conflicted);
    assert_eq!(
        secondary.state().unwrap().operation.id(),
        &secondary_operation
    );
    assert_eq!(fs::read(root.join("same.txt")).unwrap(), b"right");
    assert_eq!(
        fs::read(primary.checkout_root().unwrap().join("same.txt")).unwrap(),
        b"left"
    );
    let repository = NativeScmRepository::new(primary.store_root());
    for conflict in accepted.conflict_ids {
        assert!(repository.get_conflict(&conflict).unwrap().is_some());
    }
    assert_eq!(
        repository
            .operation_heads(primary.state().unwrap().space.id())
            .unwrap()
            .len(),
        2
    );
    assert_clean(&primary, &[left, right, accepted.step_id]);
}
