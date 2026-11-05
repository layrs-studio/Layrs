//! Metadata-only runner namespace fixtures; no network or file-content reads.
use super::*;

fn entry(name: &str, target: TreeEntryTarget) -> TreeEntry {
    TreeEntry::new(RepoPathComponent::parse(name).unwrap(), target, false)
}

fn tree(graph: &mut GraphObjects, entries: Vec<TreeEntry>) -> TreeId {
    let tree = Tree::new(entries).unwrap();
    let id = tree.id().clone();
    graph.insert(CanonicalObject::Tree(tree)).unwrap();
    id
}

fn step(graph: &mut GraphObjects, root: TreeId) -> StepId {
    let step = Step::new(vec![], root).unwrap();
    let id = step.id().clone();
    graph.insert(CanonicalObject::Step(step)).unwrap();
    id
}

fn empty_file(graph: &mut GraphObjects) -> TreeEntryTarget {
    let manifest = FileManifest::empty(ChunkingDescriptor::single()).unwrap();
    let target = TreeEntryTarget::file(manifest.file_id().clone());
    graph
        .insert(CanonicalObject::FileManifest(manifest))
        .unwrap();
    target
}

fn tombstone(graph: &mut GraphObjects) -> TreeEntryTarget {
    let tombstone = Tombstone::new(None).unwrap();
    let target = TreeEntryTarget::tombstone(tombstone.id().clone());
    graph.insert(CanonicalObject::Tombstone(tombstone)).unwrap();
    target
}

fn assert_empty_directory(result: RemoteResult<()>) {
    assert!(matches!(
        result,
        Err(NativeRemoteError::Invalid(message)) if message.contains("empty director")
    ));
}

#[test]
fn empty_root_and_tombstone_only_root_are_supported() {
    let mut graph = GraphObjects::default();
    let empty = tree(&mut graph, vec![]);
    let deleted = tombstone(&mut graph);
    let tombstones = tree(&mut graph, vec![entry("deleted", deleted)]);
    for root in [empty, tombstones] {
        let target = step(&mut graph, root);
        graph.validate_runner_namespace(&target).unwrap();
        assert!(graph.entries(&target).unwrap().is_empty());
    }
}

#[test]
fn named_empty_and_tombstone_only_directories_are_refused() {
    let mut graph = GraphObjects::default();
    let empty = tree(&mut graph, vec![]);
    let deleted = tombstone(&mut graph);
    let tombstones = tree(&mut graph, vec![entry("deleted", deleted)]);
    for directory in [empty, tombstones] {
        let root = tree(
            &mut graph,
            vec![entry("named", TreeEntryTarget::tree(directory))],
        );
        let target = step(&mut graph, root);
        // The ordinary checkout projection remains unchanged; only the runner
        // boundary refuses to silently lose this namespace entry.
        assert!(graph.projection(&target).unwrap().is_empty());
        assert_empty_directory(graph.validate_runner_namespace(&target));
    }
}

#[test]
fn nested_empty_descendant_is_refused_even_with_nonempty_siblings_and_aliases() {
    let mut graph = GraphObjects::default();
    let file = empty_file(&mut graph);
    let empty = tree(&mut graph, vec![]);
    let branch = tree(
        &mut graph,
        vec![
            entry("empty", TreeEntryTarget::tree(empty)),
            entry("file", file.clone()),
        ],
    );
    let root = tree(
        &mut graph,
        vec![
            entry("a", TreeEntryTarget::tree(branch.clone())),
            entry("b", TreeEntryTarget::tree(branch)),
            entry("file", file),
        ],
    );
    let target = step(&mut graph, root);
    assert_empty_directory(graph.validate_runner_namespace(&target));
}

#[test]
fn nonempty_directory_aliases_preserve_all_runner_paths() {
    let mut graph = GraphObjects::default();
    let file = empty_file(&mut graph);
    let deleted = tombstone(&mut graph);
    let link = Symlink::new("file").unwrap();
    let link_target = TreeEntryTarget::symlink(link.id().clone());
    graph.insert(CanonicalObject::Symlink(link)).unwrap();
    let branch = tree(
        &mut graph,
        vec![
            entry("file", file),
            entry("link", link_target),
            entry("deleted", deleted),
        ],
    );
    let parent = tree(
        &mut graph,
        vec![entry("nested", TreeEntryTarget::tree(branch))],
    );
    let root = tree(
        &mut graph,
        vec![
            entry("a", TreeEntryTarget::tree(parent.clone())),
            entry("b", TreeEntryTarget::tree(parent)),
        ],
    );
    let target = step(&mut graph, root);
    graph.validate_runner_namespace(&target).unwrap();
    assert_eq!(
        graph
            .entries(&target)
            .unwrap()
            .into_iter()
            .map(|entry| entry.path)
            .collect::<Vec<_>>(),
        [
            "a/nested/file",
            "a/nested/link",
            "b/nested/file",
            "b/nested/link"
        ]
    );
}

#[test]
fn validation_is_scoped_to_the_requested_step() {
    let mut graph = GraphObjects::default();
    let empty = tree(&mut graph, vec![]);
    let unsupported = tree(
        &mut graph,
        vec![entry("empty", TreeEntryTarget::tree(empty.clone()))],
    );
    let other = step(&mut graph, unsupported);
    let target = Step::new(vec![other.clone()], empty).unwrap();
    let target_id = target.id().clone();
    graph.insert(CanonicalObject::Step(target)).unwrap();
    graph.validate_runner_namespace(&target_id).unwrap();
    assert_empty_directory(graph.validate_runner_namespace(&other));
}

#[test]
fn depth_limit_is_preserved_for_nonempty_directory_chains() {
    let mut graph = GraphObjects::default();
    let file = empty_file(&mut graph);
    let mut root = tree(&mut graph, vec![entry("file", file)]);
    for _ in 0..128 {
        root = tree(&mut graph, vec![entry("d", TreeEntryTarget::tree(root))]);
    }
    let allowed = step(&mut graph, root.clone());
    graph.validate_runner_namespace(&allowed).unwrap();
    let root = tree(&mut graph, vec![entry("d", TreeEntryTarget::tree(root))]);
    let excessive = step(&mut graph, root);
    assert!(matches!(
        graph.validate_runner_namespace(&excessive),
        Err(NativeRemoteError::Invalid("checkout exceeds limits"))
    ));
}

#[test]
fn logical_entry_limit_counts_alias_placements_and_tombstones() {
    let mut graph = GraphObjects::default();
    let file = empty_file(&mut graph);
    let deleted = tombstone(&mut graph);
    let mut entries = vec![entry("file", file)];
    entries.extend((0..98).map(|i| entry(&format!("deleted-{i}"), deleted.clone())));
    let branch = tree(&mut graph, entries);
    for (placements, allowed) in [(1_000, true), (1_001, false)] {
        let root = tree(
            &mut graph,
            (0..placements)
                .map(|i| entry(&format!("dir-{i}"), TreeEntryTarget::tree(branch.clone())))
                .collect(),
        );
        let target = step(&mut graph, root);
        // Each placement contributes its directory plus 99 entries. Only a few
        // canonical objects represent 100,000 (or 100,100) logical entries.
        assert!(graph.objects.len() < 10);
        if allowed {
            graph.validate_runner_namespace(&target).unwrap();
        } else {
            assert!(matches!(
                graph.validate_runner_namespace(&target),
                Err(NativeRemoteError::Invalid("checkout exceeds limits"))
            ));
        }
    }
}

#[test]
fn missing_reachable_tree_is_never_treated_as_empty() {
    let mut graph = GraphObjects::default();
    let missing = Tree::new(vec![]).unwrap();
    let root = tree(
        &mut graph,
        vec![entry(
            "missing",
            TreeEntryTarget::tree(missing.id().clone()),
        )],
    );
    let target = step(&mut graph, root);
    assert!(matches!(
        graph.validate_runner_namespace(&target),
        Err(NativeRemoteError::Invalid("missing Tree"))
    ));
}
