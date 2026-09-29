use super::*;

fn fixture() -> (MergeObjects, GraphMergePlan) {
    let mut objects = MergeObjects::default();
    let base = file_step(&mut objects, b"prefix\r\nzero\r\nsame\r\nzero\r\nfin", &[]);
    let a = file_step(
        &mut objects,
        "prefix\r\né\r\nsame\r\né\r\nfin".as_bytes(),
        std::slice::from_ref(&base),
    );
    let b = file_step(&mut objects, b"prefix\r\nB\r\nsame\r\nB\r\nfin", &[base]);
    let merged = plan(&objects, std::slice::from_ref(&a), std::slice::from_ref(&b));
    assert_eq!(merged.conflicts(), plan(&objects, &[b], &[a]).conflicts());
    assert_eq!(
        merged.conflicts.values().next().unwrap().0["blocks"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    install(&mut objects, &merged);
    (objects, merged)
}

fn install_bundle(objects: &mut MergeObjects, bundle: &InstallationBundle) {
    objects
        .steps
        .insert(bundle.step.id().clone(), bundle.step.clone());
    objects
        .trees
        .extend(bundle.trees.iter().map(|t| (t.id().clone(), t.clone())));
    objects
        .conflicts
        .extend(bundle.conflicts.iter().map(|c| (c.id(), c.clone())));
    for chunk in &bundle.chunks {
        objects
            .files
            .insert(FileId::from_bytes(chunk.bytes()), chunk.bytes().to_vec());
    }
    for file in &bundle.file_manifests {
        if file.byte_length() == 0 {
            objects.files.insert(file.file_id().clone(), vec![]);
        }
    }
}

#[test]
fn blocks_partial_reopen_final_preserves_originals_anchors_and_exact_bytes() {
    let (mut objects, merged) = fixture();
    let original = merged.conflicts.values().next().unwrap().clone();
    original.validate_payload(&objects.files).unwrap();
    let blocks = original.0["blocks"].as_array().unwrap();
    let ids: Vec<_> = blocks
        .iter()
        .map(|b| ObjectId::parse(b["blockId"].as_str().unwrap()).unwrap())
        .collect();
    assert_ne!(
        ids[0], ids[1],
        "equal text at distinct positions has distinct canonical IDs"
    );
    let selected = ObjectId::parse(blocks[0]["sideValues"][0]["termId"].as_str().unwrap()).unwrap();
    let choices = BTreeMap::from([(
        original.id(),
        BTreeMap::from([(
            ids[0].clone(),
            ConflictBlockResolution::SelectTerm(selected),
        )]),
    )]);
    let partial = resolve_conflict_blocks(
        &objects,
        merged.step.id(),
        &choices,
        GraphMergeLimits::default(),
    )
    .unwrap();
    assert_eq!(partial.step.parent_step_ids(), [merged.step.id().clone()]);
    let updated = &partial.conflicts[0];
    assert_ne!(updated.id(), original.id());
    assert_eq!(updated.0["resolutionState"], "partial");
    assert_eq!(updated.0["bases"], original.0["bases"]);
    assert_eq!(updated.0["sides"], original.0["sides"]);
    for (before, after) in blocks.iter().zip(updated.0["blocks"].as_array().unwrap()) {
        assert_eq!(before["blockId"], after["blockId"]);
        assert_eq!(before["occurrenceAnchor"], after["occurrenceAnchor"]);
    }
    let reopened =
        PlannedConflict::from_canonical_bytes(&updated.canonical_bytes().unwrap()).unwrap();
    assert_eq!(&reopened, updated);
    install_bundle(&mut objects, &partial);
    reopened.validate_payload(&objects.files).unwrap();
    let sources = original.0["bases"]
        .as_array()
        .unwrap()
        .iter()
        .chain(original.0["sides"].as_array().unwrap())
        .map(|t| ObjectId::parse(t["termId"].as_str().unwrap()).unwrap())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let choices = BTreeMap::from([(
        updated.id(),
        BTreeMap::from([(
            ids[1].clone(),
            ConflictBlockResolution::Manual {
                bytes: "manuel 🦀\r\n".as_bytes().to_vec(),
                source_term_ids: sources,
            },
        )]),
    )]);
    let final_bundle = resolve_conflict_blocks(
        &objects,
        partial.step.id(),
        &choices,
        GraphMergeLimits::default(),
    )
    .unwrap();
    assert!(final_bundle.conflicts.is_empty());
    assert_eq!(
        final_bundle.step.parent_step_ids(),
        [partial.step.id().clone()]
    );
    install_bundle(&mut objects, &final_bundle);
    let tree = &objects.trees[final_bundle.step.root_tree_id()];
    let final_bytes = &objects.files[tree.entries()[0].target().file_id().unwrap()];
    let mut expected = vec![];
    for segment in original.0["segments"].as_array().unwrap() {
        if segment["kind"] == "content" {
            expected.extend_from_slice(
                &objects.files[&FileId::parse(segment["objectId"].as_str().unwrap()).unwrap()],
            );
        } else if segment["blockId"] == ids[1].as_str() {
            expected.extend_from_slice("manuel 🦀\r\n".as_bytes());
        } else {
            let result = &updated.0["blocks"][0]["resolution"]["result"];
            expected.extend_from_slice(
                &objects.files[&FileId::parse(result["objectId"].as_str().unwrap()).unwrap()],
            );
        }
    }
    assert_eq!(*final_bytes, expected);
    assert_eq!(objects.conflicts[&original.id()], original);
    // No automatic re-merge may flatten retained semantic choices.
    let third = file_step(&mut objects, b"another\n", merged.merge_bases());
    assert!(matches!(
        plan_graph_merge(
            &objects,
            &[partial.step.id().clone()],
            &[third],
            policy(),
            GraphMergeLimits::default()
        ),
        Err(GraphMergeError::Unsupported(_))
    ));
}

#[test]
fn blocks_reject_unknown_stale_corruption_missing_payload_and_small_budget() {
    let (objects, merged) = fixture();
    let conflict = merged.conflicts.values().next().unwrap();
    let block = &conflict.0["blocks"][0];
    let id = ObjectId::parse(block["blockId"].as_str().unwrap()).unwrap();
    let term = ObjectId::parse(block["sideValues"][0]["termId"].as_str().unwrap()).unwrap();
    let choices = BTreeMap::from([(
        conflict.id(),
        BTreeMap::from([(
            id.clone(),
            ConflictBlockResolution::SelectTerm(term.clone()),
        )]),
    )]);
    let partial = resolve_conflict_blocks(
        &objects,
        merged.step.id(),
        &choices,
        GraphMergeLimits::default(),
    )
    .unwrap();
    let mut changed = objects.clone();
    install_bundle(&mut changed, &partial);
    assert!(
        resolve_conflict_blocks(
            &changed,
            partial.step.id(),
            &choices,
            GraphMergeLimits::default()
        )
        .is_err()
    );
    let again = BTreeMap::from([(partial.conflicts[0].id(), choices[&conflict.id()].clone())]);
    assert!(
        resolve_conflict_blocks(
            &changed,
            partial.step.id(),
            &again,
            GraphMergeLimits::default()
        )
        .is_err()
    );
    let mut missing = objects.files.clone();
    missing.remove(conflict.payload_file_ids().unwrap().iter().next().unwrap());
    assert!(conflict.validate_payload(&missing).is_err());
    let unknown = BTreeMap::from([(
        conflict.id(),
        BTreeMap::from([(
            term.clone(),
            ConflictBlockResolution::SelectTerm(term.clone()),
        )]),
    )]);
    assert!(
        resolve_conflict_blocks(
            &objects,
            merged.step.id(),
            &unknown,
            GraphMergeLimits::default()
        )
        .is_err()
    );
    let mut corrupt = conflict.0.clone();
    corrupt["blocks"][0]["occurrenceAnchor"]["spans"][0]["startByte"] = json!(999);
    semantic_blocks::reidentify(&mut corrupt).unwrap();
    assert!(PlannedConflict::from_canonical_bytes(&canonical_json(&corrupt).unwrap()).is_err());
    // Rehashing a malformed occurrence does not bypass contextual validation.
    let mut corrupt = conflict.0.clone();
    let old = corrupt["blocks"][0]["blockId"].clone();
    for span in corrupt["blocks"][0]["occurrenceAnchor"]["spans"]
        .as_array_mut()
        .unwrap()
    {
        span["startByte"] = json!(span["startByte"].as_u64().unwrap() + 1);
        span["endByte"] = json!(span["endByte"].as_u64().unwrap() + 1);
    }
    let mut projection = corrupt["blocks"][0].clone();
    projection.as_object_mut().unwrap().remove("blockId");
    projection.as_object_mut().unwrap().remove("resolution");
    projection["lensPin"] = corrupt["lensPin"].clone();
    projection["lensImplementationDigest"] = corrupt["lensImplementationDigest"].clone();
    let new_id = digest("layrs-conflict-block-v1", &projection).unwrap();
    corrupt["blocks"][0]["blockId"] = json!(new_id);
    for segment in corrupt["segments"].as_array_mut().unwrap() {
        if segment["blockId"] == old {
            segment["blockId"] = json!(new_id);
        }
    }
    corrupt["blocks"]
        .as_array_mut()
        .unwrap()
        .sort_by(|a, b| a["blockId"].as_str().cmp(&b["blockId"].as_str()));
    semantic_blocks::reidentify(&mut corrupt).unwrap();
    let corrupt =
        PlannedConflict::from_canonical_bytes(&canonical_json(&corrupt).unwrap()).unwrap();
    assert!(corrupt.validate_payload(&objects.files).is_err());
    let choices = BTreeMap::from([(
        conflict.id(),
        BTreeMap::from([(
            id,
            ConflictBlockResolution::Manual {
                bytes: vec![b'a'; 4096],
                source_term_ids: vec![term],
            },
        )]),
    )]);
    assert!(matches!(
        resolve_conflict_blocks(
            &objects,
            merged.step.id(),
            &choices,
            GraphMergeLimits {
                max_input_bytes: 100,
                ..GraphMergeLimits::default()
            }
        ),
        Err(GraphMergeError::Limit(_))
    ));
}

#[test]
fn auto_merged_content_that_cannot_reconstruct_every_original_stays_structural() {
    let mut objects = MergeObjects::default();
    let base = file_step(&mut objects, b"base\nsame\nbase\n", &[]);
    let a = file_step(
        &mut objects,
        b"A\nsame\nbase\n",
        std::slice::from_ref(&base),
    );
    let b = file_step(&mut objects, b"B\nsame\nnew\n", &[base]);
    let merged = plan(&objects, &[a], &[b]);
    let conflict = merged.conflicts.values().next().unwrap();
    assert!(conflict.0["blocks"].as_array().unwrap().is_empty());
    assert!(merged.files.is_empty());
}
