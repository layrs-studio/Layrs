use super::*;
use layrs_core::scm::{FileId, RepoPath};
use layrs_sync::v3::Nullable;

fn file(letter: char) -> PathPatchEntry {
    PathPatchEntry {
        target: TreeEntryTarget::file(
            FileId::parse(format!("blake3:{}", letter.to_string().repeat(64))).unwrap(),
        ),
        executable: false,
    }
}
fn entry(name: &str, value: &PathPatchEntry) -> TreeEntry {
    TreeEntry::new(
        RepoPathComponent::parse(name).unwrap(),
        value.target.clone(),
        value.executable,
    )
}
fn change(
    path: &str,
    expected: Option<PathPatchEntry>,
    replacement: Option<PathPatchEntry>,
) -> PathPatchChange {
    PathPatchChange {
        path: RepoPath::parse(path).unwrap(),
        expected: Nullable(expected),
        replacement: Nullable(replacement),
    }
}

#[test]
fn edit_preserves_opaque_siblings_and_rebuilds_only_affected_ancestors() {
    let secret = TreeId::parse(format!("blake3:{}", "f".repeat(64))).unwrap();
    let visible = Tree::new(vec![entry("a", &file('a')), entry("untouched", &file('c'))]).unwrap();
    let root = Tree::new(vec![
        TreeEntry::new(
            RepoPathComponent::parse("hidden").unwrap(),
            TreeEntryTarget::tree(secret.clone()),
            false,
        ),
        TreeEntry::new(
            RepoPathComponent::parse("visible").unwrap(),
            TreeEntryTarget::tree(visible.id().clone()),
            true,
        ),
    ])
    .unwrap();
    let source = BTreeMap::from([
        (root.id().clone(), root.clone()),
        (visible.id().clone(), visible.clone()),
    ]);
    let mut seen = vec![];
    let result = rebuild_path_patch(
        root.id(),
        &[change("visible/a", Some(file('a')), Some(file('b')))],
        &mut |id| {
            seen.push(id.clone());
            source.get(id).cloned().ok_or(V3StoreError::NotFound)
        },
    )
    .unwrap();
    assert_eq!(seen, [root.id().clone(), visible.id().clone()]);
    assert!(!seen.contains(&secret));
    assert_eq!(result.regenerated_trees.len(), 2);
    let trees: BTreeMap<_, _> = result
        .regenerated_trees
        .iter()
        .map(|t| (t.id(), t))
        .collect();
    let after = trees[&result.root_tree_id];
    assert_eq!(&after.entries()[0], &root.entries()[0]);
    assert!(after.entries()[1].executable());
    let child = after.entries()[1].target().tree_id().unwrap();
    assert_eq!(trees[child].entries()[0].target(), &file('b').target);
    assert_eq!(&trees[child].entries()[1], &visible.entries()[1]);
    // No mutation of source objects.
    assert_eq!(source[root.id()], root);
}

#[test]
fn rename_pair_creation_deletion_and_modes_are_exact_and_deterministic() {
    let root = Tree::new(vec![entry("old", &file('a'))]).unwrap();
    let mut moved = file('a');
    moved.executable = true;
    let changes = [
        change("new/nested", None, Some(moved.clone())),
        change("old", Some(file('a')), None),
    ];
    let run = || {
        rebuild_path_patch(root.id(), &changes, &mut |id| {
            assert_eq!(id, root.id());
            Ok(root.clone())
        })
        .unwrap()
    };
    let a = run();
    let b = run();
    assert_eq!(a.root_tree_id, b.root_tree_id);
    assert_eq!(a.regenerated_trees, b.regenerated_trees);
    let newroot = a
        .regenerated_trees
        .iter()
        .find(|t| t.id() == &a.root_tree_id)
        .unwrap();
    assert_eq!(newroot.entries().len(), 1);
    let nested = a
        .regenerated_trees
        .iter()
        .find(|t| Some(t.id()) == newroot.entries()[0].target().tree_id())
        .unwrap();
    assert_eq!(nested.entries(), &[entry("nested", &moved)]);
}

#[test]
fn stale_collision_wrong_loader_and_opaque_directory_changes_fail() {
    let child = Tree::new(vec![entry("secret", &file('b'))]).unwrap();
    let root = Tree::new(vec![
        entry("file", &file('a')),
        TreeEntry::new(
            RepoPathComponent::parse("dir").unwrap(),
            TreeEntryTarget::tree(child.id().clone()),
            false,
        ),
    ])
    .unwrap();
    for changes in [
        vec![change("file", Some(file('b')), Some(file('c')))],
        vec![change("file", None, Some(file('b')))],
        vec![change("missing", Some(file('a')), None)],
        vec![change("file/child", None, Some(file('b')))],
        vec![change("dir", None, Some(file('b')))],
        vec![change(
            "dir",
            Some(PathPatchEntry {
                target: TreeEntryTarget::tree(child.id().clone()),
                executable: false,
            }),
            None,
        )],
    ] {
        assert!(rebuild_path_patch(root.id(), &changes, &mut |_| Ok(root.clone())).is_err());
    }
    assert!(matches!(
        rebuild_path_patch(
            root.id(),
            &[change("file", Some(file('a')), None)],
            &mut |_| Ok(child.clone())
        ),
        Err(V3StoreError::IdentityMismatch)
    ));
}

#[test]
fn duplicate_ancestor_overlap_and_noop_fail_before_loading() {
    let root = Tree::new(vec![]).unwrap();
    for changes in [
        vec![],
        vec![
            change("a", None, Some(file('a'))),
            change("a", None, Some(file('b'))),
        ],
        vec![
            change("a", None, Some(file('a'))),
            change("a-b", None, Some(file('b'))),
            change("a/child", None, Some(file('c'))),
        ],
        vec![change("a", Some(file('a')), Some(file('a')))],
    ] {
        assert!(
            rebuild_path_patch(root.id(), &changes, &mut |_| panic!(
                "invalid patch must fail before CAS read"
            ))
            .is_err()
        );
    }
}

#[test]
fn metadata_work_is_bounded_before_regeneration() {
    let tree = Tree::new(vec![entry("a", &file('a'))]).unwrap();
    let mut budget = Budget {
        trees: MAX_LOADED_TREES,
        ..Budget::default()
    };
    assert!(matches!(
        budget.charge(&tree),
        Err(V3StoreError::LimitExceeded { .. })
    ));
    let mut budget = Budget {
        entries: MAX_VISITED_ENTRIES,
        ..Budget::default()
    };
    assert!(budget.charge_entries(tree.entries()).is_err());
    let mut budget = Budget {
        bytes: MAX_METADATA_BYTES,
        ..Budget::default()
    };
    assert!(budget.charge_entries(tree.entries()).is_err());
}
