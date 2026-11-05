use super::*;
use crate::graph_merge::renames::{PlannedRenameConflict, analyze_renames};

fn snapshot(objects: &mut MergeObjects, files: &[(&str, &[u8])], parents: &[StepId]) -> StepId {
    let entries = files
        .iter()
        .map(|(path, bytes)| file(objects, path, bytes))
        .collect();
    step(objects, entries, parents)
}

fn fixture(
    left: &[(&str, &[u8])],
    right: &[(&str, &[u8])],
) -> (MergeObjects, StepId, StepId, StepId) {
    let mut objects = MergeObjects::default();
    let base = snapshot(&mut objects, &[("a.txt", b"exact\0\xff")], &[]);
    let left = snapshot(&mut objects, left, std::slice::from_ref(&base));
    // Distinct parent chain allows equal final snapshots with distinct heads.
    let middle = snapshot(
        &mut objects,
        &[("a.txt", b"exact\0\xff")],
        std::slice::from_ref(&base),
    );
    let right = snapshot(&mut objects, right, &[middle]);
    (objects, base, left, right)
}

#[test]
fn exact_moves_unilateral_and_same_destination_remain_clean_without_changing_sources() {
    for right in [
        vec![("a.txt", b"exact\0\xff".as_slice())],
        vec![("b.txt", b"exact\0\xff".as_slice())],
    ] {
        let (objects, base, left, right) = fixture(&[("b.txt", b"exact\0\xff")], &right);
        let analysis = analyze_renames(
            &objects,
            &base,
            &[left.clone(), right.clone()],
            GraphMergeLimits::default(),
        )
        .unwrap();
        assert!(!analysis.exact.is_empty());
        assert!(analysis.ambiguous.is_empty());
        assert!(analysis.conflicts.is_empty());
        let merged = plan(
            &objects,
            std::slice::from_ref(&left),
            std::slice::from_ref(&right),
        );
        let tree = &merged.trees[merged.step.root_tree_id()];
        assert_eq!(tree.entries().len(), 1);
        assert_eq!(tree.entries()[0].name().as_str(), "b.txt");
        assert_eq!(
            tree.entries()[0].target().file_id().unwrap(),
            &FileId::from_bytes(b"exact\0\xff")
        );
        assert_eq!(merged.step.parent_step_ids().len(), 2);
        assert!(
            objects.trees[objects.steps[&base].root_tree_id()]
                .entries()
                .iter()
                .any(|e| e.name().as_str() == "a.txt")
        );
    }
}

#[test]
fn concurrent_move_modify_delete_and_destination_divergence_are_one_complete_conflict() {
    for (right, expected_paths) in [
        (vec![("a.txt", b"changed".as_slice())], 2),
        (vec![], 2),
        (vec![("c.txt", b"exact\0\xff".as_slice())], 3),
        (
            vec![
                ("a.txt", b"exact\0\xff".as_slice()),
                ("b.txt", b"collision".as_slice()),
            ],
            2,
        ),
    ] {
        let (objects, base, left, right) = fixture(&[("b.txt", b"exact\0\xff")], &right);
        let analysis = analyze_renames(
            &objects,
            &base,
            &[left.clone(), right.clone()],
            GraphMergeLimits::default(),
        )
        .unwrap();
        assert_eq!(analysis.conflicts.len(), 1);
        let conflict = &analysis.conflicts[0];
        let doc = conflict.document();
        assert_eq!(doc["kind"], "rename");
        assert_eq!(doc["placements"].as_array().unwrap().len(), expected_paths);
        for term in doc["bases"]
            .as_array()
            .unwrap()
            .iter()
            .chain(doc["sides"].as_array().unwrap())
        {
            assert_eq!(term["values"].as_array().unwrap().len(), expected_paths);
            let selected = conflict
                .select_term(&ObjectId::parse(term["termId"].as_str().unwrap()).unwrap())
                .unwrap();
            assert_eq!(selected.values.len(), expected_paths);
            let provenance = StepId::parse(term["provenance"]["stepId"].as_str().unwrap()).unwrap();
            let tree = &objects.trees[objects.steps[&provenance].root_tree_id()];
            for (path, value) in &selected.values {
                assert_eq!(
                    value.as_ref(),
                    tree.entries()
                        .iter()
                        .find(|e| e.name().as_str() == path.as_str())
                );
            }
        }
        let reversed = analyze_renames(
            &objects,
            &base,
            &[right.clone(), left.clone()],
            GraphMergeLimits::default(),
        )
        .unwrap();
        assert_eq!(analysis, reversed);
        assert_eq!(
            PlannedRenameConflict::from_canonical_bytes(
                &conflict.canonical_bytes().unwrap(),
                &objects,
                GraphMergeLimits::default()
            )
            .unwrap(),
            *conflict
        );
        let merged = plan(&objects, &[left], &[right]);
        assert_eq!(merged.conflicts.len(), 1);
        let parsed =
            PlannedConflict::from_canonical_bytes(&conflict.canonical_bytes().unwrap()).unwrap();
        parsed
            .validate_rename_context(&objects, GraphMergeLimits::default())
            .unwrap();
        assert_eq!(merged.conflicts.values().next().unwrap(), &parsed);
        assert_eq!(
            merged.trees[merged.step.root_tree_id()].entries().len(),
            expected_paths
        );
        for entry in merged.trees[merged.step.root_tree_id()].entries() {
            assert_eq!(entry.target().conflict_id(), Some(&parsed.id()));
        }
        let mut installed = objects.clone();
        install(&mut installed, &merged);
        for term in doc["bases"]
            .as_array()
            .unwrap()
            .iter()
            .chain(doc["sides"].as_array().unwrap())
        {
            let origin = StepId::parse(term["provenance"]["stepId"].as_str().unwrap()).unwrap();
            let chosen = ObjectId::parse(term["termId"].as_str().unwrap()).unwrap();
            let resolved = resolve_conflict_terms(
                &installed,
                merged.step.id(),
                &BTreeMap::from([(parsed.id(), chosen)]),
                GraphMergeLimits::default(),
            )
            .unwrap();
            assert_eq!(
                resolved.step.root_tree_id(),
                objects.steps[&origin].root_tree_id()
            );
            assert!(resolved.conflicts.is_empty());
            assert_eq!(resolved.step.parent_step_ids(), [merged.step.id().clone()]);
        }
        assert!(
            resolve_conflict_replacements(
                &installed,
                merged.step.id(),
                &BTreeMap::from([(
                    parsed.id(),
                    ConflictReplacement {
                        bytes: b"manual".to_vec(),
                        executable: false
                    }
                )]),
                GraphMergeLimits::default()
            )
            .is_err()
        );
        let mut entries = merged.trees[merged.step.root_tree_id()].entries().to_vec();
        entries.pop();
        let incomplete = step(&mut installed, entries, &[merged.step.id().clone()]);
        let chosen = ObjectId::parse(doc["bases"][0]["termId"].as_str().unwrap()).unwrap();
        assert!(
            resolve_conflict_terms(
                &installed,
                &incomplete,
                &BTreeMap::from([(parsed.id(), chosen)]),
                GraphMergeLimits::default()
            )
            .is_err()
        );
    }
}

#[test]
fn duplicate_values_are_explicit_ambiguities_and_never_matched_by_path_order() {
    let mut objects = MergeObjects::default();
    let base = snapshot(&mut objects, &[("a", b"x"), ("copy", b"x")], &[]);
    let left = snapshot(
        &mut objects,
        &[("b", b"x"), ("copy", b"x")],
        std::slice::from_ref(&base),
    );
    let right = snapshot(
        &mut objects,
        &[("a", b"y"), ("copy", b"x")],
        std::slice::from_ref(&base),
    );
    let analysis =
        analyze_renames(&objects, &base, &[left, right], GraphMergeLimits::default()).unwrap();
    assert!(analysis.exact.is_empty());
    assert_eq!(analysis.ambiguous.len(), 1);
    assert_eq!(analysis.ambiguous[0].base_candidates.len(), 2);
    assert_eq!(analysis.ambiguous[0].surviving_occurrences.len(), 1);
    assert_eq!(analysis.conflicts.len(), 1);
    assert_eq!(
        analysis.conflicts[0].document()["placements"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn copies_modified_content_and_mode_changes_are_not_invented_as_exact_moves() {
    let mut objects = MergeObjects::default();
    let base = snapshot(&mut objects, &[("a", b"same")], &[]);
    let retained = snapshot(
        &mut objects,
        &[("a", b"same"), ("b", b"same")],
        std::slice::from_ref(&base),
    );
    let modified = snapshot(
        &mut objects,
        &[("b", b"similar")],
        std::slice::from_ref(&base),
    );
    for left in [retained, modified] {
        let analysis = analyze_renames(
            &objects,
            &base,
            &[left, base.clone()],
            GraphMergeLimits::default(),
        )
        .unwrap();
        assert!(
            analysis.exact.is_empty()
                && analysis.ambiguous.is_empty()
                && analysis.conflicts.is_empty()
        );
    }
    let entry = file(&mut objects, "b", b"same");
    let changed_mode = TreeEntry::new(entry.name().clone(), entry.target().clone(), true);
    let left = step(
        &mut objects,
        vec![changed_mode],
        std::slice::from_ref(&base),
    );
    assert!(
        analyze_renames(
            &objects,
            &base,
            &[left, base.clone()],
            GraphMergeLimits::default()
        )
        .unwrap()
        .exact
        .is_empty()
    );
}

#[test]
fn two_distinct_sources_competing_for_one_destination_form_one_component() {
    let mut objects = MergeObjects::default();
    let base = snapshot(&mut objects, &[("a", b"x"), ("c", b"y")], &[]);
    let left = snapshot(
        &mut objects,
        &[("b", b"x"), ("c", b"y")],
        std::slice::from_ref(&base),
    );
    let right = snapshot(
        &mut objects,
        &[("a", b"x"), ("b", b"y")],
        std::slice::from_ref(&base),
    );
    let analysis =
        analyze_renames(&objects, &base, &[left, right], GraphMergeLimits::default()).unwrap();
    assert_eq!(analysis.exact.len(), 2);
    assert_eq!(analysis.conflicts.len(), 1);
    assert_eq!(
        analysis.conflicts[0].document()["placements"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn nested_move_preserves_created_and_removed_ancestor_values() {
    let mut objects = MergeObjects::default();
    let entry = file(&mut objects, "a", b"same");
    let dir = Tree::new(vec![entry]).unwrap();
    objects.trees.insert(dir.id().clone(), dir.clone());
    let parent_entry = |name| {
        TreeEntry::new(
            RepoPathComponent::parse(name).unwrap(),
            TreeEntryTarget::tree(dir.id().clone()),
            false,
        )
    };
    let base = step(&mut objects, vec![parent_entry("old")], &[]);
    let left = step(
        &mut objects,
        vec![parent_entry("new")],
        std::slice::from_ref(&base),
    );
    let changed = file(&mut objects, "a", b"modified");
    let changed_dir = Tree::new(vec![changed]).unwrap();
    let id = changed_dir.id().clone();
    objects.trees.insert(id.clone(), changed_dir);
    let right = step(
        &mut objects,
        vec![TreeEntry::new(
            RepoPathComponent::parse("old").unwrap(),
            TreeEntryTarget::tree(id),
            false,
        )],
        std::slice::from_ref(&base),
    );
    let analysis = analyze_renames(
        &objects,
        &base,
        &[left.clone(), right],
        GraphMergeLimits::default(),
    )
    .unwrap();
    let conflict = &analysis.conflicts[0];
    let paths: BTreeSet<_> = conflict.document()["placements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, BTreeSet::from(["old", "old/a", "new", "new/a"]));
    let term = conflict.document()["sides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["provenance"]["stepId"] == left.as_str())
        .unwrap();
    let resolved = conflict
        .select_term(&ObjectId::parse(term["termId"].as_str().unwrap()).unwrap())
        .unwrap();
    assert!(resolved.values[&RepoPath::parse("old").unwrap()].is_none());
    assert!(
        resolved.values[&RepoPath::parse("new").unwrap()]
            .as_ref()
            .unwrap()
            .target()
            .tree_id()
            .is_some()
    );
}

#[test]
fn proposed_rename_rejects_tampered_or_missing_context_and_bounded_output() {
    let (mut objects, base, left, right) =
        fixture(&[("b.txt", b"exact\0\xff")], &[("a.txt", b"changed")]);
    let analysis = analyze_renames(
        &objects,
        &base,
        &[left.clone(), right.clone()],
        GraphMergeLimits::default(),
    )
    .unwrap();
    let conflict = &analysis.conflicts[0];
    // Even a self-consistent, fully rehashed document is not proof that the
    // declared base really held those bytes at that placement.
    let mut forged = conflict.document().clone();
    let value = forged["bases"][0]["values"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|v| v["value"]["kind"] == "file")
        .unwrap();
    value["value"]["objectId"] = json!(FileId::from_bytes(b"changed"));
    forged["bases"][0].as_object_mut().unwrap().remove("termId");
    forged["bases"][0]["termId"] =
        json!(digest("layrs-conflict-term-v1", &forged["bases"][0]).unwrap());
    forged.as_object_mut().unwrap().remove("conflictId");
    forged["conflictId"] = json!(digest("layrs-conflict-v1", &forged).unwrap());
    let parsed = PlannedConflict::from_canonical_bytes(&canonical_json(&forged).unwrap()).unwrap();
    assert!(
        parsed
            .validate_rename_context(&objects, GraphMergeLimits::default())
            .is_err()
    );
    let mut tampered = conflict.document().clone();
    tampered["sides"][0]["values"].as_array_mut().unwrap().pop();
    assert!(
        PlannedRenameConflict::from_canonical_bytes(
            &canonical_json(&tampered).unwrap(),
            &objects,
            GraphMergeLimits::default()
        )
        .is_err()
    );
    assert!(
        conflict
            .select_term(&ObjectId::parse(base.as_str()).unwrap())
            .is_err()
    );
    assert!(matches!(
        analyze_renames(
            &objects,
            &base,
            &[left.clone(), right.clone()],
            GraphMergeLimits {
                max_output_bytes: 10,
                ..GraphMergeLimits::default()
            }
        ),
        Err(GraphMergeError::Limit(_))
    ));
    objects.files.remove(&FileId::from_bytes(b"exact\0\xff"));
    assert!(analyze_renames(&objects, &base, &[left, right], GraphMergeLimits::default()).is_err());
}

#[test]
fn case_and_unicode_paths_keep_exact_canonical_identity_on_every_os() {
    let mut ids = vec![];
    for destination in ["A.txt", "é.txt", "É.txt"] {
        let (objects, base, left, right) =
            fixture(&[(destination, b"exact\0\xff")], &[("a.txt", b"modified")]);
        let analysis =
            analyze_renames(&objects, &base, &[left, right], GraphMergeLimits::default()).unwrap();
        let conflict = &analysis.conflicts[0];
        assert!(
            conflict.document()["placements"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["path"] == destination)
        );
        ids.push(conflict.id().to_string());
    }
    assert_eq!(
        ids,
        [
            "blake3:ccee92225f29ea7bf077e4d17d3ee46acc6f3a1cafb86147721486ba8db8d4fd",
            "blake3:6127616f1f8fdc47a031e90f2c91850e217a9805912413fb66dc3cb94a28a309",
            "blake3:1a4f5eee92e3412269e3bd60bdf2c6819e12b643567dc417012de0b36c3148c0",
        ]
    );
}

#[test]
fn symlink_moves_keep_exact_target_objects_and_do_not_become_files() {
    let mut objects = MergeObjects::default();
    let original = Symlink::new("../exact-target").unwrap();
    let modified = Symlink::new("../changed-target").unwrap();
    objects
        .symlinks
        .insert(original.id().clone(), original.clone());
    objects
        .symlinks
        .insert(modified.id().clone(), modified.clone());
    let link = |name, id: SymlinkId| {
        TreeEntry::new(
            RepoPathComponent::parse(name).unwrap(),
            TreeEntryTarget::symlink(id),
            false,
        )
    };
    let base = step(&mut objects, vec![link("old", original.id().clone())], &[]);
    let left = step(
        &mut objects,
        vec![link("new", original.id().clone())],
        std::slice::from_ref(&base),
    );
    let right = step(
        &mut objects,
        vec![link("old", modified.id().clone())],
        std::slice::from_ref(&base),
    );
    let analysis =
        analyze_renames(&objects, &base, &[left, right], GraphMergeLimits::default()).unwrap();
    assert_eq!(analysis.exact.len(), 1);
    let doc = analysis.conflicts[0].document();
    for term in doc["bases"]
        .as_array()
        .unwrap()
        .iter()
        .chain(doc["sides"].as_array().unwrap())
    {
        for placed in term["values"].as_array().unwrap() {
            let value = &placed["value"];
            assert!(
                value["kind"] == "absent"
                    || (value["kind"] == "symlink" && value["executable"].is_null())
            );
        }
    }
}
