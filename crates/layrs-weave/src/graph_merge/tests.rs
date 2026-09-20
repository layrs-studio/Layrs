use super::*;
use crate::reconciliation::{FailureAction, FileMergeLimits};
mod blocks;
mod renames;

fn policy() -> FileMergePolicy {
    FileMergePolicy {
        unsupported: FailureAction::StructuralFallback,
        quota: FailureAction::Reject,
        limits: FileMergeLimits::default(),
    }
}
fn file(objects: &mut MergeObjects, name: &str, bytes: &[u8]) -> TreeEntry {
    let id = FileId::from_bytes(bytes);
    objects.files.insert(id.clone(), bytes.to_vec());
    TreeEntry::new(
        RepoPathComponent::parse(name).unwrap(),
        TreeEntryTarget::file(id),
        false,
    )
}
fn step(objects: &mut MergeObjects, entries: Vec<TreeEntry>, parents: &[StepId]) -> StepId {
    let tree = Tree::new(entries).unwrap();
    let step = Step::new(parents.to_vec(), tree.id().clone()).unwrap();
    objects.trees.insert(tree.id().clone(), tree);
    let id = step.id().clone();
    objects.steps.insert(id.clone(), step);
    id
}
fn file_step(objects: &mut MergeObjects, bytes: &[u8], parents: &[StepId]) -> StepId {
    let entry = file(objects, "code.ts", bytes);
    step(objects, vec![entry], parents)
}
fn plan(objects: &MergeObjects, source: &[StepId], target: &[StepId]) -> GraphMergePlan {
    plan_graph_merge(
        objects,
        source,
        target,
        policy(),
        GraphMergeLimits::default(),
    )
    .unwrap()
}
fn branches() -> (MergeObjects, StepId, StepId, StepId) {
    let mut objects = MergeObjects::default();
    let base = file_step(&mut objects, b"a\nb\nc\n", &[]);
    let a = file_step(&mut objects, b"A\nb\nc\n", std::slice::from_ref(&base));
    let b = file_step(&mut objects, b"a\nb\nC\n", std::slice::from_ref(&base));
    (objects, base, a, b)
}

#[test]
fn dag_merge_creates_one_canonical_join_and_is_side_order_independent() {
    let (objects, base, a, b) = branches();
    let merged = plan(&objects, std::slice::from_ref(&a), std::slice::from_ref(&b));
    assert_eq!(merged.merge_bases(), &[base]);
    let mut expected = vec![a.clone(), b.clone()];
    expected.sort();
    assert_eq!(merged.step().parent_step_ids(), expected);
    assert!(merged.conflicts().is_empty());
    let root = &merged.trees()[merged.step().root_tree_id()];
    assert_eq!(
        merged.files()[root.entries()[0].target().file_id().unwrap()],
        b"A\nb\nC\n"
    );
    let reverse = plan(&objects, &[b], &[a]);
    assert_eq!(merged.step(), reverse.step());
    assert_eq!(merged.trees(), reverse.trees());
    assert_eq!(merged.files(), reverse.files());
}

#[test]
fn already_present_sources_still_join_a_multi_head_target() {
    let (objects, _, a, b) = branches();
    let forward = plan(&objects, &[a.clone(), b.clone()], &[a.clone(), b.clone()]);
    let reverse = plan(&objects, &[b.clone(), a.clone()], &[b.clone(), a.clone()]);
    let mut parents = vec![a, b];
    parents.sort();
    assert_eq!(forward.step().parent_step_ids(), parents);
    assert_eq!(forward.step(), reverse.step());
    assert_eq!(forward.merge_bases(), reverse.merge_bases());
}

#[test]
fn conflicts_have_canonical_terms_and_keep_every_original_value() {
    let (mut objects, base, a, _) = branches();
    let b = file_step(&mut objects, b"B\nb\nc\n", std::slice::from_ref(&base));
    let merged = plan(&objects, std::slice::from_ref(&a), std::slice::from_ref(&b));
    let conflict = merged.conflicts().values().next().unwrap();
    let value = conflict.document();
    assert_eq!(value["schemaVersion"], "layrs.scm.conflict.v1");
    assert_eq!(value["bases"].as_array().unwrap().len(), 1);
    assert_eq!(value["sides"].as_array().unwrap().len(), 2);
    for term in value["bases"]
        .as_array()
        .unwrap()
        .iter()
        .chain(value["sides"].as_array().unwrap())
    {
        let mut projection = term.clone();
        projection.as_object_mut().unwrap().remove("termId");
        assert_eq!(
            term["termId"],
            json!(digest("layrs-conflict-term-v1", &projection).unwrap())
        );
        let origin = StepId::parse(term["provenance"]["stepId"].as_str().unwrap()).unwrap();
        let original = &objects.trees[objects.steps[&origin].root_tree_id()];
        assert_eq!(
            term["values"][0]["value"]["objectId"],
            json!(original.entries()[0].target().file_id().unwrap())
        );
    }
    let mut projection = value.clone();
    projection.as_object_mut().unwrap().remove("conflictId");
    assert_eq!(
        value["conflictId"],
        json!(digest("layrs-conflict-v1", &projection).unwrap())
    );
    assert_eq!(merged.conflicts(), plan(&objects, &[b], &[a]).conflicts());
    assert_eq!(value["blocks"].as_array().unwrap().len(), 1);
    // Only immutable original fragments, never rendered marker text.
    assert!(
        merged
            .files()
            .values()
            .all(|bytes| !bytes.windows(7).any(|w| w == b"<<<<<<<"))
    );
}

#[test]
fn delete_modify_and_empty_file_are_distinct() {
    let (mut objects, base, a, _) = branches();
    let deleted = step(&mut objects, vec![], std::slice::from_ref(&base));
    let merged = plan(&objects, &[a], &[deleted]);
    let conflict = merged.conflicts().values().next().unwrap().document();
    assert_eq!(conflict["kind"], "delete_modify");
    assert!(
        conflict["sides"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["values"][0]["value"] == json!({"kind":"absent"}))
    );
    assert!(conflict["lensPin"].is_null());
}

#[test]
fn unrelated_roots_retain_explicit_empty_base_set() {
    let mut objects = MergeObjects::default();
    let a = file_step(&mut objects, b"first", &[]);
    let b = file_step(&mut objects, b"second", &[]);
    let merged = plan(&objects, &[a], &[b]);
    assert!(merged.merge_bases().is_empty());
    assert!(
        merged.conflicts().values().next().unwrap().document()["bases"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn criss_cross_keeps_all_bases_in_conflict_instead_of_choosing_one() {
    let (mut objects, _, a, b) = branches();
    let parents = vec![a.clone(), b.clone()];
    let left = file_step(&mut objects, b"left", &parents);
    let right = file_step(&mut objects, b"right", &parents);
    let merged = plan(&objects, &[left], &[right]);
    let mut expected = vec![a, b];
    expected.sort();
    assert_eq!(merged.merge_bases(), expected);
    let conflict = merged.conflicts().values().next().unwrap().document();
    assert_eq!(conflict["bases"].as_array().unwrap().len(), 2);
}

#[test]
fn graph_join_is_not_noop_when_tree_is_identical_but_parents_differ() {
    let (mut objects, base, a, _) = branches();
    let b = file_step(&mut objects, b"a\nb\nc\n", std::slice::from_ref(&a));
    let c = file_step(&mut objects, b"a\nb\nc\n", std::slice::from_ref(&base));
    let merged = plan(&objects, &[b], &[c]);
    assert_eq!(
        merged.step().root_tree_id(),
        objects.steps[&base].root_tree_id()
    );
    assert_eq!(merged.step().parent_step_ids().len(), 2);
    assert_eq!(
        plan_graph_merge(
            &objects,
            &[base],
            &[a],
            policy(),
            GraphMergeLimits::default()
        )
        .unwrap_err(),
        GraphMergeError::NoChanges
    );
}

#[test]
fn directory_file_symlink_and_binary_conflicts_keep_object_kinds() {
    for kind in ["directory_file", "type", "binary"] {
        let mut objects = MergeObjects::default();
        let base_entry = file(&mut objects, "asset.bin", &[0xff]);
        let base = step(&mut objects, vec![base_entry], &[]);
        let a_entry = file(&mut objects, "asset.bin", &[0xfe]);
        let a = step(&mut objects, vec![a_entry], std::slice::from_ref(&base));
        let target = match kind {
            "directory_file" => {
                let tree = Tree::new(vec![]).unwrap();
                let id = tree.id().clone();
                objects.trees.insert(id.clone(), tree);
                TreeEntryTarget::tree(id)
            }
            "type" => {
                let link = Symlink::new("target").unwrap();
                let id = link.id().clone();
                objects.symlinks.insert(id.clone(), link);
                TreeEntryTarget::symlink(id)
            }
            _ => file(&mut objects, "asset.bin", &[0xfd]).target().clone(),
        };
        let b = step(
            &mut objects,
            vec![TreeEntry::new(
                RepoPathComponent::parse("asset.bin").unwrap(),
                target,
                false,
            )],
            &[base],
        );
        let merged = plan(&objects, &[a], &[b]);
        assert_eq!(
            merged.conflicts().values().next().unwrap().document()["kind"],
            kind
        );
    }
}

#[test]
fn multi_heads_are_all_parents_and_permutations_are_deterministic() {
    let (mut objects, base, a, b) = branches();
    let c = file_step(&mut objects, b"third", &[base]);
    let first = plan(&objects, &[a.clone(), b.clone()], std::slice::from_ref(&c));
    let second = plan(&objects, &[b, a], &[c]);
    assert_eq!(first.step(), second.step());
    assert_eq!(first.step().parent_step_ids().len(), 3);
    assert_eq!(first.conflicts(), second.conflicts());
    assert_eq!(
        first.conflicts().values().next().unwrap().document()["sides"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn missing_and_corrupt_objects_reject_before_trivial_tree_reuse() {
    let (mut objects, _, a, b) = branches();
    let file_id = objects.files.keys().next().unwrap().clone();
    objects.files.get_mut(&file_id).unwrap().push(42);
    assert!(matches!(
        plan_graph_merge(
            &objects,
            std::slice::from_ref(&a),
            std::slice::from_ref(&b),
            policy(),
            GraphMergeLimits::default()
        ),
        Err(GraphMergeError::Invalid(_))
    ));
    objects.files.remove(&file_id);
    assert!(matches!(
        plan_graph_merge(&objects, &[a], &[b], policy(), GraphMergeLimits::default()),
        Err(GraphMergeError::Missing(_))
    ));
}

#[test]
fn graph_quotas_and_duplicate_heads_reject_without_partial_plan() {
    let (objects, _, a, b) = branches();
    for limits in [
        GraphMergeLimits {
            max_paths: 0,
            ..GraphMergeLimits::default()
        },
        GraphMergeLimits {
            max_input_bytes: 0,
            ..GraphMergeLimits::default()
        },
        GraphMergeLimits {
            max_output_bytes: 0,
            ..GraphMergeLimits::default()
        },
    ] {
        assert!(matches!(
            plan_graph_merge(
                &objects,
                std::slice::from_ref(&a),
                std::slice::from_ref(&b),
                policy(),
                limits
            ),
            Err(GraphMergeError::Limit(_))
        ));
    }
    assert!(matches!(
        plan_graph_merge(
            &objects,
            &[a.clone(), a],
            &[b],
            policy(),
            GraphMergeLimits::default()
        ),
        Err(GraphMergeError::Invalid(_))
    ));
}

fn profile(plan: &GraphMergePlan) -> layrs_sync::v3::DeterminismProfile {
    let sha = plan.execution_policy_digest().unwrap();
    let limits = plan.limits;
    let mut value = json!({
        "canonicalization":{"id":"rfc8785", "version":"RFC8785"},
        "contentHash":{"id":"blake3", "version":"1"},
        "mergeAlgorithm":{"id":"layrs.native-tree-merge", "version":"1"},
        "baseAlgorithm":{"id":"layrs.step-dag-bases", "version":"1"},
        "hostPolicy":{"id":"layrs.native", "version":"1", "digest":sha,
            "ambientCapabilities":{"network":false,"wallClock":false,"randomness":false,"locale":false,"hostFilesystem":false,"environment":false}},
        "resolutionPolicy":{"id":"layrs.structural", "version":"1", "digest":sha,
            "allowedMethods":["select_term"], "automaticSideSelection":false,
            "absentSemantics":"distinct_from_empty_file", "combineOrder":"term_id_ascii", "manualInput":"forbidden", "renameResolution":"forbidden",
            "onUnsupportedLens":"structural_fallback", "onUnavailableLens":"reject", "onTimeout":"reject", "onQuotaExceeded":"reject", "onTrap":"reject",
            "onInvalidOutput":"reject", "onInvalidSignature":"reject", "onDigestMismatch":"reject"},
        "limits":{"maxMemoryBytes":134217728, "maxExecutionMs":5000, "maxInputBytes":limits.max_input_bytes,
            "maxOutputBytes":limits.max_output_bytes, "maxFuel":null, "maxBlocks":plan.file_policy.limits.max_blocks,
            "maxSegments":plan.file_policy.limits.max_segments, "maxPaths":limits.max_paths, "maxRecursionDepth":limits.max_depth}
    });
    value["profileId"] = json!(digest("layrs-determinism-profile-v1", &value).unwrap());
    serde_json::from_value(value).unwrap()
}
fn context() -> ReceiptContext {
    ReceiptContext {
        space_id: SpaceId::parse("space:test").unwrap(),
        operation_id: OperationId::parse("operation:test").unwrap(),
        created_at: Timestamp::parse("2026-09-20T00:00:00Z").unwrap(),
        gate_snapshot_id: None,
    }
}

#[test]
fn receipt_hashes_profile_executions_and_result_conflict_closure() {
    let (mut objects, base, a, _) = branches();
    let b = file_step(&mut objects, b"B\nb\nc\n", &[base]);
    let plan = plan(&objects, &[a], &[b]);
    let profile = profile(&plan);
    let receipt = plan
        .receipt(
            context(),
            ReceiptHostAttestation {
                enforced_profile: profile.clone(),
            },
        )
        .unwrap();
    assert_eq!(
        receipt.conflict_ids,
        plan.conflicts.keys().cloned().collect::<Vec<_>>()
    );
    assert_eq!(receipt.lens_executions.len(), 1);
    assert_eq!(
        receipt.lens_executions[0].conflict_id.as_ref(),
        plan.conflicts.keys().next()
    );
    let mut value = serde_json::to_value(&receipt).unwrap();
    value.as_object_mut().unwrap().remove("weaveId");
    assert_eq!(
        receipt.weave_id,
        digest("layrs-weave-receipt-v1", &value).unwrap()
    );
    if let Ok(path) = std::env::var("LAYRS_WEAVE_SCHEMA_FIXTURE") {
        std::fs::write(
            path,
            canonical_json(
                &json!({"receipt":receipt, "bundle":plan.installation_bundle().unwrap()}),
            )
            .unwrap(),
        )
        .unwrap();
    }
    let mut bad = profile;
    bad.limits.max_paths += 1;
    assert!(
        plan.receipt(
            context(),
            ReceiptHostAttestation {
                enforced_profile: bad
            }
        )
        .is_err()
    );
}

fn install(objects: &mut MergeObjects, plan: &GraphMergePlan) {
    objects
        .steps
        .insert(plan.step.id().clone(), plan.step.clone());
    objects.trees.extend(plan.trees.clone());
    objects.files.extend(plan.files.clone());
    objects.conflicts.extend(plan.conflicts.clone());
}

#[test]
fn existing_conflict_can_merge_again_without_losing_any_term() {
    let (mut objects, base, a, _) = branches();
    let b = file_step(&mut objects, b"B\nb\nc\n", std::slice::from_ref(&base));
    let first =
        plan_structural_merge(&objects, &[a], &[b], policy(), GraphMergeLimits::default()).unwrap();
    install(&mut objects, &first);
    let another = file_step(&mut objects, b"third\nb\nc\n", &[base]);
    let next = plan(&objects, std::slice::from_ref(first.step.id()), &[another]);
    let conflict = next.conflicts.values().next().unwrap().document();
    assert_eq!(conflict["sides"].as_array().unwrap().len(), 3);
    for term in first.conflicts.values().next().unwrap().document()["sides"]
        .as_array()
        .unwrap()
    {
        assert!(conflict["sides"].as_array().unwrap().contains(term));
    }
    assert!(conflict["lensPin"].is_null());
}

#[test]
fn canonical_conflict_ingress_checks_nested_ids_and_never_drops_fields() {
    let (mut objects, base, a, _) = branches();
    let b = file_step(&mut objects, b"B\nb\nc\n", &[base]);
    let merged = plan(&objects, &[a], &[b]);
    let conflict = merged.conflicts.values().next().unwrap();
    assert_eq!(
        PlannedConflict::from_canonical_bytes(&conflict.canonical_bytes().unwrap()).unwrap(),
        *conflict
    );
    let mut corrupt = conflict.0.clone();
    corrupt["sides"][0]["values"][0]["value"]["executable"] = json!(true);
    assert!(PlannedConflict::from_canonical_bytes(&canonical_json(&corrupt).unwrap()).is_err());
    let mut extra = conflict.0.clone();
    extra["ignoredData"] = json!("must not disappear");
    assert!(PlannedConflict::from_canonical_bytes(&canonical_json(&extra).unwrap()).is_err());
}

#[test]
fn tombstone_is_absent_in_merge_and_missing_tombstone_rejects() {
    let (mut objects, base, a, _) = branches();
    let tombstone = Tombstone::new(None).unwrap();
    let id = tombstone.id().clone();
    objects.tombstones.insert(id.clone(), tombstone);
    let b = step(
        &mut objects,
        vec![TreeEntry::new(
            RepoPathComponent::parse("code.ts").unwrap(),
            TreeEntryTarget::tombstone(id.clone()),
            false,
        )],
        &[base],
    );
    let merged = plan(&objects, std::slice::from_ref(&a), std::slice::from_ref(&b));
    assert_eq!(
        merged.conflicts.values().next().unwrap().document()["kind"],
        "delete_modify"
    );
    objects.tombstones.remove(&id);
    assert!(matches!(
        plan_graph_merge(&objects, &[a], &[b], policy(), GraphMergeLimits::default()),
        Err(GraphMergeError::Missing(_))
    ));
}

#[test]
fn partial_file_resolution_produces_descendant_and_preserves_other_conflicts() {
    let mut objects = MergeObjects::default();
    let mut create = |bytes: &[u8], parents: &[StepId]| {
        let x = file(&mut objects, "x.txt", bytes);
        let y = file(&mut objects, "y.txt", bytes);
        step(&mut objects, vec![x, y], parents)
    };
    let base = create(b"base", &[]);
    let a = create(b"left", std::slice::from_ref(&base));
    let b = create(b"right", &[base]);
    let merged = plan(&objects, &[a], &[b]);
    assert_eq!(merged.conflicts.len(), 2);
    install(&mut objects, &merged);
    let conflict = merged.conflicts.values().next().unwrap();
    let term = ObjectId::parse(conflict.0["sides"][0]["termId"].as_str().unwrap()).unwrap();
    let resolved = resolve_conflict_terms(
        &objects,
        merged.step.id(),
        &BTreeMap::from([(conflict.id(), term)]),
        GraphMergeLimits::default(),
    )
    .unwrap();
    assert_eq!(
        resolved.step.parent_step_ids(),
        std::slice::from_ref(merged.step.id())
    );
    assert_eq!(resolved.conflicts.len(), 1);
    assert_eq!(objects.conflicts.len(), 2);
    assert_ne!(resolved.step.root_tree_id(), merged.step.root_tree_id());
    let bundle = merged.installation_bundle().unwrap();
    let persisted: Value = serde_json::from_slice(&bundle.canonical_bytes().unwrap()).unwrap();
    let recovered: Step = serde_json::from_value(persisted["step"].clone()).unwrap();
    assert_eq!(recovered, merged.step);
    for value in persisted["conflicts"].as_array().unwrap() {
        PlannedConflict::from_canonical_bytes(&canonical_json(value).unwrap()).unwrap();
    }
}

#[test]
fn selected_absent_term_deletes_instead_of_creating_empty_file() {
    let (mut objects, base, a, _) = branches();
    let deleted = step(&mut objects, vec![], &[base]);
    let merged = plan(&objects, &[a], &[deleted]);
    install(&mut objects, &merged);
    let conflict = merged.conflicts.values().next().unwrap();
    let term = conflict.0["sides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["values"][0]["value"]["kind"] == "absent")
        .unwrap();
    let resolved = resolve_conflict_terms(
        &objects,
        merged.step.id(),
        &BTreeMap::from([(
            conflict.id(),
            ObjectId::parse(term["termId"].as_str().unwrap()).unwrap(),
        )]),
        GraphMergeLimits::default(),
    )
    .unwrap();
    assert!(resolved.conflicts.is_empty());
    assert!(
        resolved
            .trees
            .iter()
            .find(|t| t.id() == resolved.step.root_tree_id())
            .unwrap()
            .entries()
            .is_empty()
    );
}

#[test]
fn conflicts_nested_inside_retained_directory_variants_remain_in_closure() {
    fn nested(objects: &mut MergeObjects, bytes: &[u8], parents: &[StepId]) -> StepId {
        let entry = file(objects, "x.txt", bytes);
        let child = Tree::new(vec![entry]).unwrap();
        let id = child.id().clone();
        objects.trees.insert(id.clone(), child);
        step(
            objects,
            vec![TreeEntry::new(
                RepoPathComponent::parse("dir").unwrap(),
                TreeEntryTarget::tree(id),
                false,
            )],
            parents,
        )
    }
    let mut objects = MergeObjects::default();
    let base = nested(&mut objects, b"base", &[]);
    let a = nested(&mut objects, b"left", std::slice::from_ref(&base));
    let b = nested(&mut objects, b"right", std::slice::from_ref(&base));
    let first = plan(&objects, &[a], &[b]);
    install(&mut objects, &first);
    let replacement = file(&mut objects, "dir", b"now a file");
    let other = step(&mut objects, vec![replacement], &[base]);
    let merged = plan(&objects, std::slice::from_ref(first.step.id()), &[other]);
    assert_eq!(merged.conflicts.len(), 2);
    assert!(
        first
            .conflicts
            .keys()
            .all(|id| merged.conflicts.contains_key(id))
    );
    install(&mut objects, &merged);
    let outer = merged
        .conflicts
        .values()
        .find(|c| c.document()["kind"] == "directory_file")
        .unwrap();
    let selected = outer.document()["sides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["values"][0]["value"]["kind"] == "tree")
        .unwrap();
    let bundle = resolve_conflict_terms(
        &objects,
        merged.step.id(),
        &BTreeMap::from([(
            outer.id(),
            ObjectId::parse(selected["termId"].as_str().unwrap()).unwrap(),
        )]),
        GraphMergeLimits::default(),
    )
    .unwrap();
    assert_eq!(bundle.conflicts.len(), 1);
    assert_eq!(
        bundle.conflicts[0].id(),
        first.conflicts.values().next().unwrap().id()
    );
}
#[test]
fn manual_replacements_preserve_exact_bytes_modes_and_parent_conflict() {
    let (mut objects, base, a, _) = branches();
    let b = file_step(&mut objects, b"OTHER\nb\nc\n", &[base]);
    let merged = plan(&objects, &[a], &[b]);
    install(&mut objects, &merged);
    let id = merged.conflicts.keys().next().unwrap().clone();
    let original = objects.conflicts[&id].clone();
    for bytes in [vec![0, 255, 1, 13, 10], vec![]] {
        let replacements = BTreeMap::from([(
            id.clone(),
            ConflictReplacement {
                bytes: bytes.clone(),
                executable: true,
            },
        )]);
        let bundle = resolve_conflict_replacements(
            &objects,
            merged.step.id(),
            &replacements,
            GraphMergeLimits::default(),
        )
        .unwrap();
        assert_eq!(bundle.step.parent_step_ids(), [merged.step.id().clone()]);
        assert!(bundle.conflicts.is_empty());
        let root = bundle
            .trees
            .iter()
            .find(|t| t.id() == bundle.step.root_tree_id())
            .unwrap();
        let entry = &root.entries()[0];
        assert!(entry.executable());
        assert_eq!(entry.target().file_id(), Some(&FileId::from_bytes(&bytes)));
        assert_eq!(
            bundle
                .chunks
                .iter()
                .flat_map(|c| c.bytes().iter())
                .copied()
                .collect::<Vec<_>>(),
            bytes
        );
        assert_eq!(bundle.file_manifests.len(), 1);
        assert_eq!(objects.conflicts[&id], original);
        assert!(
            resolve_conflict_replacements(
                &objects,
                merged.step.id(),
                &replacements,
                GraphMergeLimits {
                    max_output_bytes: 1,
                    ..GraphMergeLimits::default()
                }
            )
            .is_err()
        );
    }
    let unknown = ConflictId::parse(format!("blake3:{}", "0".repeat(64))).unwrap();
    assert!(
        resolve_conflict_replacements(
            &objects,
            merged.step.id(),
            &BTreeMap::from([(
                unknown,
                ConflictReplacement {
                    bytes: vec![],
                    executable: false
                }
            )]),
            GraphMergeLimits::default()
        )
        .is_err()
    );
}
