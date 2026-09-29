use super::*;

fn selected(names: &[&str]) -> ExportMode {
    ExportMode::SelectedHistory {
        references: names.iter().map(|name| (*name).into()).collect(),
    }
}

fn fixture(temp: &Temp) -> (NativeWorkspace, ImportPlan) {
    let source = temp.directory("source");
    let git = bare(&source);
    let base_tree = tree(&git, &[(b"file", b"base\n", "100644")]);
    let base = commit(&git, &base_tree, &[], "base");
    let left_tree = tree(&git, &[(b"file", b"left\n", "100644")]);
    let left = commit(&git, &left_tree, &[&base], "left");
    let right_tree = tree(&git, &[(b"file", b"right\n", "100644")]);
    let right = commit(&git, &right_tree, &[&base], "right");
    let merged_tree = tree(&git, &[(b"file", b"merged\n", "100755")]);
    let merge = commit(&git, &merged_tree, &[&left, &right], "merge");
    let orphan = commit(&git, &base_tree, &[], "omitted provenance");
    reference(&git, "refs/heads/main", &merge);
    reference(&git, "refs/heads/left", &left);
    reference(&git, "refs/heads/right", &right);
    reference(&git, "refs/heads/omitted", &orphan);
    let tag = git.write_object("tag", format!("object {base}\ntype commit\ntag base\ntagger Example <example@example.invalid> 1000000002 +0000\n\nannotation\n").as_bytes()).unwrap();
    reference(&git, "refs/tags/base", &tag);
    let plan = scan_git(&source, GitBridgeLimits::default()).unwrap();
    let native = NativeWorkspace::initialize(temp.directory("native"), "Selected export").unwrap();
    plan.install(&native).unwrap();
    (native, plan)
}

#[test]
fn selected_export_preserves_complete_merge_history_and_exact_resume_scope() {
    let temp = Temp::new();
    let (native, plan) = fixture(&temp);
    let before = native.state().unwrap();
    let mode = selected(&["refs/tags/base", "refs/heads/main"]);
    let destination = temp.0.join("selected");
    assert!(matches!(
        export_workspace_with_progress(
            &native,
            &destination,
            mode.clone(),
            GitBridgeLimits::default(),
            &mut |event| event.phase != BridgePhase::ExportCommits || event.completed < 1
        ),
        Err(BridgeError::Cancelled)
    ));
    let checkpoint_path = destination.join("layrs-export.incomplete");
    let checkpoint = std::fs::read(&checkpoint_path).unwrap();
    for changed in [
        selected(&["refs/heads/main"]),
        selected(&["refs/heads/main", "refs/heads/omitted"]),
        ExportMode::History,
    ] {
        let error = resume_export_workspace(
            &native,
            &destination,
            changed,
            GitBridgeLimits::default(),
            &mut |_| true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("checkpoint does not match"));
        assert_eq!(std::fs::read(&checkpoint_path).unwrap(), checkpoint);
    }
    // Same set in another order is the same scope and resumes successfully.
    let report = resume_export_workspace(
        &native,
        &destination,
        selected(&["refs/heads/main", "refs/tags/base"]),
        GitBridgeLimits::default(),
        &mut |_| true,
    )
    .unwrap();
    assert_eq!(report.references.len(), 2);
    assert_eq!(report.commits.len(), 4);
    let git = process::Git::new(&destination, GitBridgeLimits::default()).unwrap();
    assert_eq!(
        utf8(
            &git.run(&["rev-list", "--count", "refs/heads/main"], None, 128)
                .unwrap()
        )
        .unwrap()
        .trim(),
        "4"
    );
    let merge = git
        .object(
            "commit",
            &report.references["refs/heads/main"],
            GitBridgeLimits::default(),
        )
        .unwrap();
    assert_eq!(
        import::parse_headers(&merge)
            .unwrap()
            .iter()
            .filter(|(key, _)| key == "parent")
            .count(),
        2
    );
    assert_eq!(
        git.run(&["show", "refs/heads/main:file"], None, 128)
            .unwrap(),
        b"merged\n"
    );
    let source_tag = plan
        .report
        .references
        .iter()
        .find(|reference| reference.git_ref == "refs/tags/base")
        .unwrap();
    let original_tag = base64_decode(source_tag.annotated_tag_base64.as_ref().unwrap());
    let original_tag = utf8(&original_tag).unwrap();
    let expected_tag = format!(
        "object {}\n{}",
        report.commits[&source_tag.step_id],
        original_tag.split_once('\n').unwrap().1
    );
    assert_eq!(
        git.object(
            "tag",
            &report.references["refs/tags/base"],
            GitBridgeLimits::default()
        )
        .unwrap(),
        expected_tag.as_bytes()
    );
    let refs = git
        .run(&["for-each-ref", "--format=%(refname)"], None, 1024)
        .unwrap();
    assert_eq!(utf8(&refs).unwrap(), "refs/heads/main\nrefs/tags/base\n");
    git.run(&["fsck", "--strict", "--full"], None, 4096)
        .unwrap();
    // Lost response after refs publication: same checkpoint replay is exact.
    std::fs::write(&checkpoint_path, &checkpoint).unwrap();
    let replay = resume_export_workspace(
        &native,
        &destination,
        mode,
        GitBridgeLimits::default(),
        &mut |_| true,
    )
    .unwrap();
    assert_eq!(report.references, replay.references);
    assert_eq!(report.commits, replay.commits);
    assert_eq!(
        native.state().unwrap().operation.id(),
        before.operation.id()
    );
}

#[test]
fn selected_export_rejects_missing_ambiguous_and_over_budget_scope_without_destination() {
    let temp = Temp::new();
    let (native, plan) = fixture(&temp);
    let destination = temp.0.join("rejected");
    for mode in [
        selected(&[]),
        selected(&["refs/heads/missing"]),
        selected(&["refs/heads/main", "refs/heads/main"]),
        selected(&["HEAD"]),
        selected(&["refs/heads/main\ninvalid"]),
        selected(&[&format!("refs/heads/{}", "a".repeat(16385))]),
    ] {
        assert!(export_workspace(&native, &destination, mode, GitBridgeLimits::default()).is_err());
        assert!(!destination.exists());
    }
    for limits in [
        GitBridgeLimits {
            max_commits: 3,
            ..GitBridgeLimits::default()
        },
        GitBridgeLimits {
            max_objects: 1,
            ..GitBridgeLimits::default()
        },
    ] {
        assert!(
            export_workspace(
                &native,
                &destination,
                selected(&["refs/heads/main", "refs/tags/base"]),
                limits
            )
            .is_err()
        );
        assert!(!destination.exists());
    }
    let state = native.state().unwrap();
    let layer = state
        .layers
        .iter()
        .find(|layer| layer.name() == "git/main")
        .unwrap()
        .id()
        .clone();
    let step = |name| {
        plan.report
            .references
            .iter()
            .find(|reference| reference.git_ref == name)
            .unwrap()
            .step_id
            .clone()
    };
    let left = step("refs/heads/left");
    let right = step("refs/heads/right");
    publish_heads(&native, layer, vec![left.clone(), right], vec![]);
    assert!(
        export_workspace(
            &native,
            &destination,
            selected(&["refs/heads/main"]),
            GitBridgeLimits::default()
        )
        .is_err()
    );
    assert!(!destination.exists());
    let head = format!("refs/heads/main/head-{}", &left.as_str()[7..]);
    let report = export_workspace(
        &native,
        &destination,
        selected(&[&head]),
        GitBridgeLimits::default(),
    )
    .unwrap();
    assert_eq!(report.references.len(), 1);
    assert_eq!(report.commits.len(), 2);
    assert!(report.commits.contains_key(&left));
}

#[test]
fn selected_clean_history_omits_unselected_conflict_but_never_truncates_selected_ancestors() {
    let temp = Temp::new();
    let (native, plan) = fixture(&temp);
    let state = native.state().unwrap();
    let layer = state
        .layers
        .iter()
        .find(|layer| layer.name() == "git/left")
        .unwrap()
        .id()
        .clone();
    let right = plan
        .report
        .references
        .iter()
        .find(|reference| reference.git_ref == "refs/heads/right")
        .unwrap()
        .step_id
        .clone();
    use layrs_weave::reconciliation::{FailureAction, FileMergeLimits, FileMergePolicy};
    let preview = native
        .preview_merge(
            &[right],
            &layer,
            FileMergePolicy {
                unsupported: FailureAction::StructuralFallback,
                quota: FailureAction::Reject,
                limits: FileMergeLimits::default(),
            },
            layrs_weave::graph_merge::GraphMergeLimits::default(),
        )
        .unwrap();
    assert_eq!(preview.plan().conflicts().len(), 1);
    publish_heads(
        &native,
        layer,
        vec![preview.plan().step().id().clone()],
        preview.canonical_objects().unwrap(),
    );
    let report = export_workspace(
        &native,
        temp.0.join("clean"),
        selected(&["refs/heads/main"]),
        GitBridgeLimits::default(),
    )
    .unwrap();
    assert_eq!(report.commits.len(), 4);
    let bad = temp.0.join("conflicted");
    assert!(
        export_workspace(
            &native,
            &bad,
            selected(&["refs/heads/left"]),
            GitBridgeLimits::default()
        )
        .unwrap_err()
        .to_string()
        .contains("conflicted Steps")
    );
    assert!(!bad.exists());
}

fn base64_decode(value: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .unwrap()
}
