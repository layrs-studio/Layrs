use super::*;
use layrs_store::scm::*;
use layrs_store_local::scm_v1::{
    FsckOptions, NativeScmRepository, RepositoryFaultInjector, RepositoryFaultPoint, fsck_existing,
};
use std::sync::Arc;

struct Fault(RepositoryFaultPoint);
impl RepositoryFaultInjector for Fault {
    fn check(&self, point: RepositoryFaultPoint) -> std::result::Result<(), String> {
        if point == self.0 {
            Err("import fault recipe".into())
        } else {
            Ok(())
        }
    }
}

fn fixture(temp: &Temp) -> (PathBuf, ImportPlan) {
    let source = temp.directory("source");
    let git = bare(&source);
    let first_tree = tree(&git, &[(b"binary", &[0, 255, 1], "100644")]);
    let first = commit(&git, &first_tree, &[], "first");
    let last_tree = tree(&git, &[(b"binary", &[0, 254, 1], "100755")]);
    let last = commit(&git, &last_tree, &[&first], "last");
    reference(&git, "refs/heads/main", &last);
    reference(&git, "refs/tags/base", &first);
    let plan = scan_git(&source, GitBridgeLimits::default()).unwrap();
    (source, plan)
}

#[test]
fn import_faults_preserve_source_checkout_and_atomic_owned_receipt() {
    let temp = Temp::new();
    let (source, plan) = fixture(&temp);
    let source_before = tree_bytes(&source);
    use RepositoryFaultPoint::*;
    for point in [
        AfterEachCas,
        AfterAllCas,
        AfterWriterOpen,
        AfterBeginImmediate,
        AfterReplayCheck,
        AfterValidation,
        AfterObjects,
        AfterEntitiesAndProjections,
        AfterAdmissions,
        AfterRepositoryViewCas,
        AfterOperationHeadsCas,
        AfterWorktreeCas,
        BeforeReceipt,
        AfterReceiptBeforeCommit,
        AfterCommit,
    ] {
        let checkout = temp.directory(&format!("native-{point:?}"));
        let workspace = NativeWorkspace::initialize(&checkout, "Recovery").unwrap();
        std::fs::write(checkout.join("dirty"), [0, 251, 10]).unwrap();
        let before = workspace.state().unwrap();
        let mut batch = None;
        let result = plan.install_with(&workspace, |prepared| {
            batch = Some(prepared.clone());
            NativeScmRepository::with_fault_injector(workspace.store_root(), Arc::new(Fault(point)))
                .apply_batch(prepared)
                .map_err(invalid)
        });
        assert!(result.is_err(), "{point:?}");
        let batch = batch.unwrap();
        let reopened = NativeWorkspace::open(&checkout).unwrap();
        let state = reopened.state().unwrap();
        let committed = point == AfterCommit;
        assert_eq!(
            state.operation.id() == before.operation.id(),
            !committed,
            "{point:?}"
        );
        assert_eq!(state.view.id() == before.view.id(), !committed, "{point:?}");
        assert_eq!(state.view.worktrees(), before.view.worktrees());
        // The sole durable transaction journal belongs to the native writer.
        // Replay the exact production batch, not a freshly stamped lookalike.
        let mut writer = NativeScmRepository::new(reopened.store_root());
        let applied = writer.apply_batch(&batch).unwrap();
        assert_eq!(applied.was_replay(), committed, "{point:?}");
        assert_eq!(applied.request_digest(), batch.request_digest());
        assert!(writer.apply_batch(&batch).unwrap().was_replay());
        let retry = plan.install(&reopened).unwrap();
        assert!(retry.already_imported);
        assert_eq!(retry.layer_ids.len(), 1);
        assert_eq!(retry.marker_ids.len(), 1);
        let after = reopened.state().unwrap();
        assert_eq!(after.layers.len(), before.layers.len() + 1);
        assert_eq!(
            after.view.marker_ids().len(),
            before.view.marker_ids().len() + 1
        );
        assert_eq!(std::fs::read(checkout.join("dirty")).unwrap(), [0, 251, 10]);
        let report = fsck_existing(reopened.store_root(), FsckOptions::default()).unwrap();
        assert!(report.is_clean(), "{point:?}: {:?}", report.findings);
    }
    assert_eq!(tree_bytes(&source), source_before);
}

#[test]
fn public_import_restart_rescans_and_preserves_later_operations() {
    for point in [
        RepositoryFaultPoint::AfterReceiptBeforeCommit,
        RepositoryFaultPoint::AfterCommit,
    ] {
        let temp = Temp::new();
        let (source, plan) = fixture(&temp);
        let checkout = temp.directory("native");
        let workspace = NativeWorkspace::initialize(&checkout, "Restart").unwrap();
        assert!(
            plan.install_with(&workspace, |batch| {
                NativeScmRepository::with_fault_injector(
                    workspace.store_root(),
                    Arc::new(Fault(point)),
                )
                .apply_batch(batch)
                .map_err(invalid)
            })
            .is_err()
        );
        drop(plan);
        drop(workspace);
        let workspace = NativeWorkspace::open(&checkout).unwrap();
        let rescanned = scan_git(&source, GitBridgeLimits::default()).unwrap();
        let outcome = rescanned.install(&workspace).unwrap();
        assert_eq!(
            outcome.already_imported,
            point == RepositoryFaultPoint::AfterCommit
        );
        std::fs::write(checkout.join("later"), b"keep later capture").unwrap();
        workspace.capture().unwrap();
        let later = workspace.state().unwrap();
        assert!(rescanned.install(&workspace).unwrap().already_imported);
        assert_eq!(
            workspace.state().unwrap().operation.id(),
            later.operation.id()
        );
        assert_eq!(
            std::fs::read(checkout.join("later")).unwrap(),
            b"keep later capture"
        );
    }
}

#[test]
fn import_rejects_inconsistent_provenance_and_transaction_identity_reuse() {
    let temp = Temp::new();
    let (_, plan) = fixture(&temp);
    let workspace = NativeWorkspace::initialize(temp.directory("native"), "Mismatch").unwrap();
    let mut exact = None;
    plan.install_with(&workspace, |batch| {
        exact = Some(batch.clone());
        NativeScmRepository::new(workspace.store_root())
            .apply_batch(batch)
            .map_err(invalid)
    })
    .unwrap();
    let batch = exact.unwrap();
    let mut records = batch.entity_records().to_vec();
    let EntityRecord::Operation(operation) = records
        .iter_mut()
        .find(|record| matches!(record, EntityRecord::Operation(_)))
        .unwrap()
    else {
        unreachable!()
    };
    *operation = Operation::new(
        operation.id().clone(),
        operation.space_id().clone(),
        operation.parent_operation_ids().to_vec(),
        operation.resulting_view_id().clone(),
        operation.kind(),
        "different request under same transaction ID",
        vec![],
        None,
        None,
        operation.created_at().clone(),
        operation.metadata().clone(),
    )
    .unwrap();
    let changed = WriteBatch::new(
        batch.transaction_id().clone(),
        batch.canonical_objects().to_vec(),
        records,
        batch.admissions().to_vec(),
        batch.mutations().to_vec(),
    )
    .unwrap();
    let error = NativeScmRepository::new(workspace.store_root())
        .apply_batch(&changed)
        .unwrap_err();
    assert_eq!(error.code(), "transaction_replay_mismatch");
    let mut forged = ImportPlan {
        report: plan.report.clone(),
        objects: plan.objects.clone(),
    };
    forged.report.commits[0].raw_commit_base64 = "different provenance".into();
    assert!(forged.install(&workspace).is_err());
    assert!(plan.install(&workspace).unwrap().already_imported);

    // A structurally admissible Operation with the right ID but incomplete
    // provenance must not be mistaken for this bridge's completed import.
    let other = NativeWorkspace::initialize(temp.directory("inconsistent"), "Incomplete").unwrap();
    plan.install_with(&other, |batch| {
        let mut records = batch.entity_records().to_vec();
        for record in &mut records {
            if let EntityRecord::Operation(op) = record {
                *op = Operation::new(
                    op.id().clone(),
                    op.space_id().clone(),
                    op.parent_operation_ids().to_vec(),
                    op.resulting_view_id().clone(),
                    op.kind(),
                    op.summary(),
                    vec![],
                    None,
                    None,
                    op.created_at().clone(),
                    Metadata::new(vec![
                        MetadataEntry::new("git.importId", &plan.report.import_id).unwrap(),
                    ])
                    .unwrap(),
                )
                .unwrap();
            }
        }
        let incomplete = WriteBatch::new(
            batch.transaction_id().clone(),
            batch.canonical_objects().to_vec(),
            records,
            batch.admissions().to_vec(),
            batch.mutations().to_vec(),
        )
        .unwrap();
        NativeScmRepository::new(other.store_root())
            .apply_batch(&incomplete)
            .map_err(invalid)
    })
    .unwrap();
    assert!(
        plan.install(&NativeWorkspace::open(temp.0.join("inconsistent")).unwrap())
            .is_err()
    );
}

#[test]
fn explicit_import_refs_keep_ancestry_and_skip_unselected_incompatible_history() {
    let temp = Temp::new();
    let (source, all) = fixture(&temp);
    let git = process::Git::new(&source, GitBridgeLimits::default()).unwrap();
    let incompatible_tree = tree(&git, &[(b".layrs", b"reserved", "100644")]);
    let incompatible = commit(
        &git,
        &incompatible_tree,
        &[],
        "unselected incompatible branch",
    );
    reference(&git, "refs/heads/unselected", &incompatible);
    let before = tree_bytes(&source);
    assert!(scan_git(&source, GitBridgeLimits::default()).is_err());
    let selection = ImportSelection::References(vec!["refs/heads/main".into()]);
    let selected = scan_git_selected(&source, &selection, GitBridgeLimits::default()).unwrap();
    assert_eq!(selected.report.references.len(), 1);
    assert_eq!(selected.report.commits.len(), 2);
    assert_eq!(
        selected.report.commits[1].parent_git_ids,
        [selected.report.commits[0].git_id.clone()]
    );
    assert_ne!(selected.report.import_id, all.report.import_id);
    let native = NativeWorkspace::initialize(temp.directory("selected"), "Selected").unwrap();
    let outcome = selected.install(&native).unwrap();
    assert_eq!(outcome.layer_ids.len(), 1);
    assert!(outcome.marker_ids.is_empty());
    let tag = scan_git_selected(
        &source,
        &ImportSelection::References(vec!["refs/tags/base".into()]),
        GitBridgeLimits::default(),
    )
    .unwrap();
    assert_eq!(tag.report.commits.len(), 1);
    let both = |reverse: bool| {
        let mut refs = vec!["refs/tags/base".into(), "refs/heads/main".into()];
        if reverse {
            refs.reverse();
        }
        scan_git_selected(
            &source,
            &ImportSelection::References(refs),
            GitBridgeLimits::default(),
        )
        .unwrap()
    };
    assert_eq!(both(false).report.import_id, both(true).report.import_id);
    for refs in [
        vec![],
        vec!["HEAD"],
        vec!["refs/heads/missing"],
        vec!["refs/heads/main", "refs/heads/main"],
        vec!["refs/heads/main", "refs/heads/unselected"],
    ] {
        assert!(
            scan_git_selected(
                &source,
                &ImportSelection::References(refs.into_iter().map(String::from).collect()),
                GitBridgeLimits::default()
            )
            .is_err()
        );
    }
    assert!(
        scan_git_selected(
            &source,
            &selection,
            GitBridgeLimits {
                max_commits: 1,
                ..GitBridgeLimits::default()
            }
        )
        .is_err()
    );
    assert_eq!(tree_bytes(&source), before);
}

fn tree_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, at: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(at).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), result);
            } else {
                result.insert(
                    entry.path().strip_prefix(root).unwrap().to_owned(),
                    std::fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
