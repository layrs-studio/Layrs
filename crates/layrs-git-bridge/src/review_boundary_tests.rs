use super::*;
use layrs_store::scm::ScmReader;
use layrs_store_local::scm_v1::{
    NativeScmRepository, RepositoryFaultInjector, RepositoryFaultPoint,
};

fn plan(temp: &Temp, name: &str) -> ImportPlan {
    let source = temp.directory(name);
    let git = bare(&source);
    let tree = tree(&git, &[(b"imported", b"content", "100644")]);
    let commit = commit(&git, &tree, &[], name);
    reference(&git, &format!("refs/heads/{name}"), &commit);
    scan_git(&source, GitBridgeLimits::default()).unwrap()
}
fn pair(temp: &Temp) -> (NativeWorkspace, NativeWorkspace) {
    let first = NativeWorkspace::initialize(temp.directory("first"), "test").unwrap();
    let state = first.state().unwrap();
    let own = state
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == first.worktree_id())
        .unwrap();
    let second = first
        .create_worktree(
            temp.directory("second"),
            own.active_layer_id().unwrap(),
            own.base_step_id().unwrap(),
        )
        .unwrap();
    (first, second)
}
fn state(workspace: &NativeWorkspace) -> serde_json::Value {
    serde_json::to_value(workspace.state().unwrap()).unwrap()
}

#[test]
fn exact_snapshot_can_select_another_worktrees_admitted_step_but_not_a_foreign_space() {
    let temp = Temp::new();
    let (first, second) = pair(&temp);
    std::fs::write(
        second.checkout_root().unwrap().join("independent"),
        b"second worktree content",
    )
    .unwrap();
    second.capture().unwrap();
    let step = second
        .state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == second.worktree_id())
        .unwrap()
        .base_step_id()
        .unwrap()
        .clone();
    assert!(
        !first
            .state()
            .unwrap()
            .view
            .layer_heads()
            .iter()
            .any(|heads| heads.step_ids().contains(&step))
    );
    let destination = temp.0.join("explicit-foreign-worktree.git");
    let report = export_workspace(
        &first,
        &destination,
        ExportMode::Snapshot {
            step_id: step.clone(),
            reference: "refs/heads/exact".into(),
        },
        GitBridgeLimits::default(),
    )
    .unwrap();
    assert_eq!(report.commits.len(), 1);
    assert!(report.commits.contains_key(&step));
    let git = process::Git::for_import(&destination, GitBridgeLimits::default()).unwrap();
    assert_eq!(
        git.run(&["show", "refs/heads/exact:independent"], None, 1024)
            .unwrap(),
        b"second worktree content"
    );
    let foreign = NativeWorkspace::initialize(temp.directory("foreign-space"), "foreign").unwrap();
    std::fs::write(
        foreign.checkout_root().unwrap().join("foreign-only"),
        b"foreign content",
    )
    .unwrap();
    foreign.capture().unwrap();
    let foreign_step = foreign
        .state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == foreign.worktree_id())
        .unwrap()
        .base_step_id()
        .unwrap()
        .clone();
    let refused = temp.0.join("foreign-space.git");
    let error = export_workspace(
        &first,
        &refused,
        ExportMode::Snapshot {
            step_id: foreign_step,
            reference: "refs/heads/foreign".into(),
        },
        GitBridgeLimits::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("outside selected Space"));
    assert!(!refused.exists());
}

#[test]
fn corrupt_bare_resume_inside_another_repo_never_mutates_the_enclosing_repo() {
    let temp = Temp::new();
    let outer = temp.directory("outer.git");
    let outer_git = bare(&outer);
    let outer_tree = tree(&outer_git, &[(b"outer", b"preserved", "100644")]);
    let outer_commit = commit(&outer_git, &outer_tree, &[], "outer");
    reference(&outer_git, "refs/heads/outer", &outer_commit);
    let native = NativeWorkspace::initialize(temp.directory("native"), "native").unwrap();
    let destination = outer.join("export.git");
    assert!(matches!(
        export_workspace_with_progress(
            &native,
            &destination,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |e| e.phase != BridgePhase::ExportCommits || e.completed < 1
        ),
        Err(BridgeError::Cancelled)
    ));
    std::fs::write(destination.join("HEAD"), b"corrupt HEAD\n").unwrap();
    let head = std::fs::read(outer.join("HEAD")).unwrap();
    let refs = outer_git.run(&["show-ref"], None, 4096).unwrap();
    let objects = outer_git.run(&["count-objects", "-v"], None, 4096).unwrap();
    assert!(
        resume_export_workspace(
            &native,
            &destination,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .is_err()
    );
    assert_eq!(std::fs::read(outer.join("HEAD")).unwrap(), head);
    assert_eq!(outer_git.run(&["show-ref"], None, 4096).unwrap(), refs);
    assert_eq!(
        outer_git.run(&["count-objects", "-v"], None, 4096).unwrap(),
        objects
    );
    assert_eq!(
        std::fs::read(destination.join("HEAD")).unwrap(),
        b"corrupt HEAD\n"
    );
}

#[test]
fn import_requires_the_explicit_repo_root_and_accepts_a_linked_worktree() {
    let temp = Temp::new();
    let source = temp.directory("working");
    let git = process::Git::new(&source, GitBridgeLimits::default()).unwrap();
    git.run(&["init", "--template=", "."], None, 4096).unwrap();
    let root = tree(&git, &[(b"file", b"content", "100644")]);
    let tip = commit(&git, &root, &[], "working");
    reference(&git, "refs/heads/main", &tip);
    let nested = source.join("nested");
    std::fs::create_dir(&nested).unwrap();
    assert!(scan_git(&nested, GitBridgeLimits::default()).is_err());
    assert!(
        prepare_git_scan_checkpoint(
            &nested,
            temp.0.join("wrong.scan"),
            &ImportSelection::AllLocalReferences,
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .is_err()
    );
    assert!(!temp.0.join("wrong.scan").exists());
    let linked = temp.0.join("linked");
    git.run(
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().unwrap(),
            "refs/heads/main",
        ],
        None,
        4096,
    )
    .unwrap();
    assert!(linked.join(".git").is_file());
    let original = scan_git(&source, GitBridgeLimits::default()).unwrap();
    let linked_plan = scan_git(&linked, GitBridgeLimits::default()).unwrap();
    assert_eq!(
        original.canonical_objects(),
        linked_plan.canonical_objects()
    );
    assert_eq!(
        canonical_json(original.report()).unwrap(),
        canonical_json(linked_plan.report()).unwrap()
    );
    // Exercise the Rust binding with both relative gitdir and commondir, not
    // only Git's own path parser. Windows canonical bases use the verbatim prefix.
    std::fs::write(
        linked.join(".git"),
        b"gitdir: ../working/.git/worktrees/linked\n",
    )
    .unwrap();
    let relative_plan = scan_git(&linked, GitBridgeLimits::default()).unwrap();
    assert_eq!(
        original.canonical_objects(),
        relative_plan.canonical_objects()
    );

    let mut changed = false;
    assert!(
        scan_git_with_progress(&linked, GitBridgeLimits::default(), &mut |_| {
            if !changed {
                changed = true;
                std::fs::write(linked.join(".git"), b"gitdir: changed-after-binding\n").unwrap();
            }
            true
        })
        .is_err()
    );
    assert!(changed);
}

#[cfg(unix)]
#[test]
fn acquisition_keeps_the_binding_validated_before_source_replacement() {
    let temp = Temp::new();
    let source = temp.directory("bound-source");
    let replacement = temp.directory("replacement");
    for (path, text) in [(&source, "original"), (&replacement, "replacement")] {
        let git = bare(path);
        let root = tree(&git, &[(b"file", text.as_bytes(), "100644")]);
        let tip = commit(&git, &root, &[], text);
        reference(&git, "refs/heads/main", &tip);
    }
    let bound = process::Git::for_import(&source, GitBridgeLimits::default()).unwrap();
    std::fs::rename(&source, temp.0.join("retained-source")).unwrap();
    std::fs::rename(&replacement, &source).unwrap();
    assert!(
        import::acquire_with(
            &bound,
            &ImportSelection::AllLocalReferences,
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .is_err()
    );
    // The replacement is valid, so recapturing here would have silently changed
    // the source selected by the caller's earlier outside-source validation.
    assert!(scan_git(&source, GitBridgeLimits::default()).is_ok());
}

#[test]
fn source_root_replacement_during_acquisition_is_denied_or_refused() {
    let temp = Temp::new();
    let source = temp.directory("source");
    let git = bare(&source);
    let root = tree(&git, &[(b"file", b"original", "100644")]);
    let tip = commit(&git, &root, &[], "original");
    reference(&git, "refs/heads/main", &tip);
    let moved = temp.0.join("original-source");
    let mut attempted = false;
    let mut replaced = false;
    let outcome = scan_git_with_progress(&source, GitBridgeLimits::default(), &mut |_| {
        if !attempted {
            attempted = true;
            match std::fs::rename(&source, &moved) {
                Ok(()) => {
                    replaced = true;
                    std::fs::create_dir(&source).unwrap();
                }
                Err(error) => {
                    assert!(cfg!(windows), "unexpected rename refusal: {error}");
                    assert!(matches!(error.raw_os_error(), Some(5 | 32)));
                }
            }
        }
        true
    });
    assert!(attempted);
    if replaced {
        assert!(outcome.is_err());
        assert_eq!(std::fs::read_dir(source).unwrap().count(), 0);
    } else {
        assert_eq!(outcome.unwrap().report().commits.len(), 1);
    }
}

#[test]
fn marker_only_export_resumes_initialization_in_the_exact_destination() {
    let temp = Temp::new();
    let native = NativeWorkspace::initialize(temp.directory("native"), "native").unwrap();
    let state = native.state().unwrap();
    let destination = temp.directory("pre-init.git");
    let checkpoint = canonical_json(&serde_json::json!({
        "schemaVersion":"layrs.git-export-checkpoint.v1",
        "spaceId":state.space.id(),
        "operationId":state.operation.id(),
        "viewId":state.view.id(),
        "mode":{"kind":"history"}
    }))
    .unwrap();
    std::fs::write(destination.join("layrs-export.incomplete"), checkpoint).unwrap();
    let report = resume_export_workspace(
        &native,
        &destination,
        ExportMode::History,
        GitBridgeLimits::default(),
        &mut |_| true,
    )
    .unwrap();
    assert!(!report.commits.is_empty());
    assert!(!destination.join("layrs-export.incomplete").exists());
    let git = process::Git::for_import(&destination, GitBridgeLimits::default()).unwrap();
    assert_eq!(
        git.run(&["rev-parse", "--is-bare-repository"], None, 16)
            .unwrap(),
        b"true\n"
    );
}

#[test]
fn imports_advance_each_independent_worktree_and_preserve_foreign_frontier() {
    let temp = Temp::new();
    let (first, second) = pair(&temp);
    for (workspace, bytes) in [
        (&first, b"first".as_slice()),
        (&second, b"second".as_slice()),
    ] {
        std::fs::write(workspace.checkout_root().unwrap().join("local"), bytes).unwrap();
        workspace.capture().unwrap();
    }
    for (own, other, name) in [
        (&first, &second, "from-first"),
        (&second, &first, "from-second"),
    ] {
        let before_other = state(other);
        let expected = ExpectedBridgeState::capture(own).unwrap();
        let parent = expected.state().operation.id().clone();
        let old_heads = NativeScmRepository::new(own.store_root())
            .operation_heads(expected.state().space.id())
            .unwrap();
        assert_eq!(old_heads.len(), 2);
        let outcome = plan(&temp, name).install_expected(own, &expected).unwrap();
        assert!(!outcome.already_imported);
        let after = own.state().unwrap();
        assert_eq!(after.operation.parent_operation_ids(), &[parent.clone()]);
        assert_eq!(after.operation.id(), &outcome.operation_id);
        assert_eq!(state(other), before_other);
        let heads = NativeScmRepository::new(own.store_root())
            .operation_heads(after.space.id())
            .unwrap();
        let mut wanted = old_heads
            .into_iter()
            .filter(|id| id != &parent)
            .collect::<Vec<_>>();
        wanted.push(outcome.operation_id);
        wanted.sort();
        assert_eq!(heads, wanted);
    }
}

#[test]
fn authority_creation_change_and_removal_invalidate_review_without_operation_change() {
    let temp = Temp::new();
    let workspace = NativeWorkspace::initialize(temp.directory("native"), "test").unwrap();
    let plan = plan(&temp, "source");
    let policy = workspace.store_root().join("native-remote.json");
    for (index, bytes) in [
        Some(b"{\"observedPolicy\":1}".as_slice()),
        Some(b"{\"observedPolicy\":2,\"receipt\":\"revoked\"}".as_slice()),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        let expected = ExpectedBridgeState::capture(&workspace).unwrap();
        let before = state(&workspace);
        if let Some(bytes) = bytes {
            std::fs::write(&policy, bytes).unwrap();
        } else {
            std::fs::remove_file(&policy).unwrap();
        }
        assert_eq!(state(&workspace), before);
        assert!(plan.install_expected(&workspace, &expected).is_err());
        let destination = temp.0.join(format!("export-{index}"));
        assert!(
            export_workspace_expected(
                &workspace,
                &expected,
                &destination,
                ExportMode::History,
                GitBridgeLimits::default(),
                &mut |_| true
            )
            .is_err()
        );
        assert!(!destination.exists());
        assert_eq!(state(&workspace), before);
    }
}

#[test]
fn prepared_import_rechecks_secondary_checkout_and_locator_at_acceptance() {
    for replace_directory in [false, true] {
        let temp = Temp::new();
        let (_, workspace) = pair(&temp);
        let plan = plan(&temp, "source");
        let expected = ExpectedBridgeState::capture(&workspace).unwrap();
        let before = state(&workspace);
        let root = workspace.checkout_root().unwrap();
        let result = plan.install_expected_with(&workspace, &expected, |batch| {
            if replace_directory {
                let locator = std::fs::read(root.join(".layrs")).unwrap();
                std::fs::rename(&root, temp.0.join("retained-original-checkout")).unwrap();
                std::fs::create_dir(&root).unwrap();
                std::fs::write(root.join(".layrs"), locator).unwrap();
            } else {
                std::fs::write(root.join(".layrs"), b"changed locator").unwrap();
            }
            expected.apply_batch(&workspace, batch)
        });
        assert!(result.is_err());
        assert_eq!(state(&workspace), before);
    }
}

struct ChangeAuthority(PathBuf);
impl RepositoryFaultInjector for ChangeAuthority {
    fn check(&self, point: RepositoryFaultPoint) -> std::result::Result<(), String> {
        if point == RepositoryFaultPoint::AfterReceiptBeforeCommit {
            std::fs::write(&self.0, b"changed at commit").map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
#[test]
fn import_guard_rechecks_policy_at_commit_and_rolls_back_references() {
    let temp = Temp::new();
    let workspace = NativeWorkspace::initialize(temp.directory("native"), "test").unwrap();
    let expected = ExpectedBridgeState::capture(&workspace).unwrap();
    let before = state(&workspace);
    let result = plan(&temp, "source").install_expected_with(&workspace, &expected, |batch| {
        NativeScmRepository::with_fault_injector(
            workspace.store_root(),
            std::sync::Arc::new(ChangeAuthority(
                workspace.store_root().join("native-remote.json"),
            )),
        )
        .with_store_identity(workspace.store_identity())
        .with_metadata_identity(workspace.metadata_identity())
        .with_store_id(expected.state().worktree.store_id().clone())
        .apply_batch_guarded(batch, || expected.accept_guard())
        .map_err(invalid)
    });
    assert!(result.is_err());
    assert_eq!(state(&workspace), before);
}

#[test]
fn export_parent_replacement_and_copied_resume_directory_write_nothing() {
    let temp = Temp::new();
    let workspace = NativeWorkspace::initialize(temp.directory("native"), "test").unwrap();
    let expected = ExpectedBridgeState::capture(&workspace).unwrap();
    let parent = temp.directory("parent");
    let destination = parent.join("out.git");
    let reviewed = review_export_destination(&destination, GitBridgeLimits::default()).unwrap();
    std::fs::rename(&parent, temp.0.join("original-parent")).unwrap();
    std::fs::create_dir(&parent).unwrap();
    assert!(
        export_workspace_reviewed(
            &workspace,
            &expected,
            &reviewed,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .is_err()
    );
    assert!(!destination.exists());
    assert!(!temp.0.join("original-parent/out.git").exists());

    assert!(
        export_workspace(
            &workspace,
            &destination,
            ExportMode::History,
            GitBridgeLimits::default()
        )
        .is_ok()
    );
    // An exact checkpoint copy is still a different directory capability.
    let checkpoint = canonical_json(&serde_json::json!({"schemaVersion":"layrs.git-export-checkpoint.v1","spaceId":expected.state().space.id(),"operationId":expected.state().operation.id(),"viewId":expected.state().view.id(),"mode":{"kind":"history"}})).unwrap();
    std::fs::write(destination.join("layrs-export.incomplete"), &checkpoint).unwrap();
    let reviewed = review_export_destination(&destination, GitBridgeLimits::default()).unwrap();
    let files = super::expected_state::contents(&destination);
    std::fs::rename(&destination, parent.join("retained-export")).unwrap();
    std::fs::create_dir(&destination).unwrap();
    for (path, bytes) in &files {
        let output = destination.join(path.strip_prefix(&destination).unwrap());
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(output, bytes).unwrap();
    }
    let before = super::expected_state::contents(&destination);
    assert!(
        resume_export_workspace_reviewed(
            &workspace,
            &expected,
            &reviewed,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .is_err()
    );
    assert_eq!(super::expected_state::contents(&destination), before);
}
