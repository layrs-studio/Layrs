use layrs_client_core::native_scm::*;
use layrs_core::scm::*;
use layrs_store::scm::ScmReader;
use layrs_store_local::scm_v1::{self, NativeScmReader};
use layrs_weave::{
    graph_merge::{ConflictReplacement, GraphMergeLimits},
    reconciliation::*,
};
use std::{collections::BTreeMap, fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new(nested: bool) -> Self {
        let path = std::env::temp_dir().join(format!("layrs-rename-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        if nested {
            fs::create_dir(path.join("old")).unwrap();
        }
        fs::write(
            path.join(if nested { "old/a" } else { "old" }),
            b"exact\0\xff",
        )
        .unwrap();
        fs::write(path.join("keep"), b"untouched").unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn head(w: &NativeWorkspace) -> StepId {
    w.state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|s| s.worktree_id() == w.worktree_id())
        .unwrap()
        .base_step_id()
        .unwrap()
        .clone()
}
fn layer(w: &NativeWorkspace) -> LayerId {
    w.state()
        .unwrap()
        .layers
        .iter()
        .find(|l| l.name() == "main")
        .unwrap()
        .id()
        .clone()
}
fn policy() -> FileMergePolicy {
    FileMergePolicy {
        unsupported: FailureAction::StructuralFallback,
        quota: FailureAction::Reject,
        limits: FileMergeLimits::default(),
    }
}

#[test]
fn rename_terms_apply_all_placements_after_reopen_and_undo_preserves_history() {
    for (nested, scenario) in [
        (false, "modify"),
        (false, "delete"),
        (false, "collision"),
        (false, "divergent"),
        (true, "modify"),
    ] {
        let f = Fixture::new(nested);
        let w = NativeWorkspace::initialize(&f.0, "rename").unwrap();
        w.create_layer("topic").unwrap();
        w.switch_layer("topic").unwrap();
        fs::rename(f.0.join("old"), f.0.join("new")).unwrap();
        w.capture().unwrap();
        let source = head(&w);
        w.switch_layer("main").unwrap();
        match scenario {
            "delete" => fs::remove_file(f.0.join("old")).unwrap(),
            "collision" => fs::write(f.0.join("new"), b"collision").unwrap(),
            "divergent" => fs::rename(f.0.join("old"), f.0.join("elsewhere")).unwrap(),
            _ => fs::write(f.0.join(if nested { "old/a" } else { "old" }), b"modified").unwrap(),
        }
        w.capture().unwrap();
        let before = head(&w);
        let preview = w
            .preview_structural_merge(
                &[source.clone()],
                &layer(&w),
                policy(),
                GraphMergeLimits::default(),
            )
            .unwrap();
        assert_eq!(preview.plan().conflicts().len(), 1);
        let conflict = preview.plan().conflicts().values().next().unwrap();
        assert!(conflict.is_multi_placement());
        let id = conflict.id();
        let term = ObjectId::parse(
            conflict.document()["sides"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["provenance"]["stepId"] == source.as_str())
                .unwrap()["termId"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let accepted = w.accept_merge(&preview).unwrap();
        assert!(!accepted.materialized);
        assert_eq!(head(&w), before);
        drop(w);
        let w = NativeWorkspace::open(&f.0).unwrap();
        assert_eq!(w.layer_conflicts(&layer(&w)).unwrap().len(), 1);
        assert!(w.conflict_variant(&layer(&w), &id, &term).is_err());
        assert!(w
            .compare_conflict_variants(&layer(&w), &id, &term, &term)
            .is_err());
        assert!(w
            .preview_conflict_replacements(
                &layer(&w),
                &BTreeMap::from([(
                    id.clone(),
                    ConflictReplacement {
                        bytes: b"unsafe".to_vec(),
                        executable: false
                    }
                )]),
                GraphMergeLimits::default()
            )
            .is_err());
        let selected = w
            .preview_conflict_resolution(
                &layer(&w),
                &BTreeMap::from([(id.clone(), term)]),
                GraphMergeLimits::default(),
            )
            .unwrap();
        // An edit after preview is refused before Capture or any placement moves.
        let old_operation = w.state().unwrap().operation.id().clone();
        fs::write(f.0.join("keep"), b"dirty").unwrap();
        assert!(w.accept_conflict_resolution(&selected).is_err());
        assert_eq!(w.state().unwrap().operation.id(), &old_operation);
        fs::write(f.0.join("keep"), b"untouched").unwrap();
        let resolved = w.accept_conflict_resolution(&selected).unwrap();
        assert!(resolved.materialized && resolved.conflict_ids.is_empty());
        assert!(!f.0.join(if nested { "old/a" } else { "old" }).exists());
        // Checkout intentionally retains harmless empty untracked directories.
        if nested {
            assert!(fs::read_dir(f.0.join("old")).unwrap().next().is_none());
        }
        assert!(!f.0.join("elsewhere").exists());
        assert_eq!(
            fs::read(f.0.join(if nested { "new/a" } else { "new" })).unwrap(),
            b"exact\0\xff"
        );
        assert_eq!(fs::read(f.0.join("keep")).unwrap(), b"untouched");
        let store = scm_v1::open(w.store_root()).unwrap();
        let reader = NativeScmReader::new(&store).unwrap();
        assert_eq!(
            reader
                .get_step(&resolved.step_id)
                .unwrap()
                .unwrap()
                .parent_step_ids(),
            [accepted.step_id]
        );
        assert!(reader.get_conflict(&id).unwrap().is_some());
        drop(reader);
        drop(store);
        w.undo().unwrap();
        assert_eq!(w.layer_conflicts(&layer(&w)).unwrap().len(), 1);
        assert_eq!(head(&w), before);
        w.redo(None).unwrap();
        assert_eq!(head(&w), resolved.step_id);
        assert_eq!(
            fs::read(f.0.join(if nested { "new/a" } else { "new" })).unwrap(),
            b"exact\0\xff"
        );
    }
}
