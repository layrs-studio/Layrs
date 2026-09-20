pub(crate) mod conflicts;
pub(crate) mod lineage;
mod renames;
use super::{
    PgV3Repository, V3StoreError, V3StoreResult, load_space_descriptor, pg_error,
    read_canonical_object,
};
use layrs_core::scm::{
    ConflictId, FileId, FileManifest, ObjectId, SpaceId, StepId, Tombstone, Tree, TreeEntryTarget,
};
use layrs_sync::v3::{ObjectDescriptor, ObjectKind};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const MAX_OBJECT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_STREAM_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_FILE_CHUNKS: usize = 16_384;

/// Reconstruct incrementally: memory is bounded by a single CAS chunk.
pub(crate) async fn verified_manifest(
    repo: &PgV3Repository,
    space: &SpaceId,
    descriptor: &ObjectDescriptor,
) -> V3StoreResult<FileManifest> {
    verified_manifest_budgeted(repo, space, descriptor, &mut 0).await
}

async fn verified_manifest_budgeted(
    repo: &PgV3Repository,
    space: &SpaceId,
    descriptor: &ObjectDescriptor,
    verification_bytes: &mut u64,
) -> V3StoreResult<FileManifest> {
    let bytes = read_canonical_object(repo.cas().as_ref(), descriptor, MAX_OBJECT_BYTES).await?;
    let manifest: FileManifest =
        serde_json::from_slice(&bytes).map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
    if manifest.id().as_str() != descriptor.object_id.as_str() {
        return Err(V3StoreError::IdentityMismatch);
    }
    if manifest.byte_length() > MAX_FILE_BYTES {
        return Err(V3StoreError::LimitExceeded {
            resource: "reconstructed service file",
            limit: MAX_FILE_BYTES,
        });
    }
    if manifest.chunks().len() > MAX_FILE_CHUNKS {
        return Err(V3StoreError::LimitExceeded {
            resource: "service file chunk references",
            limit: MAX_FILE_CHUNKS as u64,
        });
    }
    // Logical reconstruction work includes repeated chunks and failed manifest
    // candidates. Deduplication cannot turn a tiny CAS into unbounded hashing.
    charge_stream(verification_bytes, manifest.byte_length())?;
    let mut verifier = manifest.verifier();
    for chunk in manifest.chunks() {
        let id = ObjectId::parse(chunk.chunk_id().as_str())
            .map_err(|_| V3StoreError::IdentityMismatch)?;
        let descriptor = load_space_descriptor(repo.pool(), space, &id).await?;
        if descriptor.kind != ObjectKind::Chunk
            || descriptor.canonical_byte_len != chunk.byte_length()
        {
            return Err(V3StoreError::IdentityMismatch);
        }
        let bytes =
            read_canonical_object(repo.cas().as_ref(), &descriptor, MAX_OBJECT_BYTES).await?;
        verifier
            .push(&bytes)
            .map_err(|_| V3StoreError::IdentityMismatch)?;
    }
    verifier
        .finish()
        .map_err(|_| V3StoreError::IdentityMismatch)?;
    Ok(manifest)
}

pub(crate) async fn insert_manifest(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    manifest: &FileManifest,
) -> V3StoreResult<()> {
    let chunking = serde_jcs::to_string(manifest.chunking())
        .map_err(|e| V3StoreError::Backend(e.to_string()))?;
    sqlx::query("INSERT INTO file_manifests(file_manifest_id,file_id,file_byte_length,chunker_name,chunker_version,chunker_parameters_jcs,reconstruction_verified_at) VALUES($1,$2,$3,$4,'1',$5,transaction_timestamp()) ON CONFLICT(file_manifest_id) DO NOTHING")
        .bind(manifest.id().as_str()).bind(manifest.file_id().as_str()).bind(super::i64_from_u64(manifest.byte_length())?).bind(manifest.chunking().strategy()).bind(chunking).execute(&mut **tx).await.map_err(pg_error)?;
    for (ordinal, chunk) in manifest.chunks().iter().enumerate() {
        sqlx::query("INSERT INTO file_manifest_chunks(file_manifest_id,ordinal,chunk_id,byte_offset,byte_length) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
            .bind(manifest.id().as_str()).bind(ordinal as i32).bind(chunk.chunk_id().as_str()).bind(super::i64_from_u64(chunk.offset())?).bind(super::i64_from_u64(chunk.byte_length())?).execute(&mut **tx).await.map_err(pg_error)?;
        sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id) VALUES($1,$2,'manifest_chunk',$3) ON CONFLICT DO NOTHING")
            .bind(manifest.id().as_str()).bind(ordinal as i32).bind(chunk.chunk_id().as_str()).execute(&mut **tx).await.map_err(pg_error)?;
    }
    Ok(())
}

type ConflictEdgeTargets = Vec<(Option<ObjectId>, Option<FileId>)>;
pub(crate) struct VerifiedClosure {
    pub objects: BTreeMap<ObjectId, ObjectDescriptor>,
    pub trees: Vec<Tree>,
    pub conflicts: BTreeSet<ConflictId>,
    pub conflict_edges: Vec<(ObjectId, ConflictEdgeTargets)>,
    pub conflict_documents: BTreeMap<ConflictId, layrs_weave::graph_merge::PlannedConflict>,
}

pub(crate) async fn verify_tree_closure(
    repo: &PgV3Repository,
    space: &SpaceId,
    roots: Vec<ObjectId>,
    known_steps: &BTreeMap<StepId, layrs_core::scm::Step>,
    new_steps: &BTreeSet<StepId>,
) -> V3StoreResult<VerifiedClosure> {
    let validation_roots = roots
        .iter()
        .map(|id| {
            layrs_core::scm::TreeId::parse(id.as_str()).map_err(|_| V3StoreError::IdentityMismatch)
        })
        .collect::<V3StoreResult<BTreeSet<_>>>()?;
    let mut pending: Vec<_> = roots.into_iter().map(|id| (id, ObjectKind::Tree)).collect();
    let mut result = VerifiedClosure {
        objects: BTreeMap::new(),
        trees: vec![],
        conflicts: BTreeSet::new(),
        conflict_edges: vec![],
        conflict_documents: BTreeMap::new(),
    };
    let mut files = BTreeSet::new();
    let mut payload_ids = BTreeSet::new();
    let mut payload = BTreeMap::new();
    let mut payload_bytes = 0;
    let mut canonical_bytes = 0u64;
    let mut content_bytes = 0u64;
    let mut verification_bytes = 0u64;
    while let Some((id, kind)) = pending.pop() {
        if let Some(known) = result.objects.get(&id) {
            if known.kind != kind {
                return Err(V3StoreError::IdentityMismatch);
            }
            continue;
        }
        if result.objects.len() >= 100_000 {
            return Err(V3StoreError::LimitExceeded {
                resource: "transaction closure",
                limit: 100_000,
            });
        }
        let descriptor = load_space_descriptor(repo.pool(), space, &id).await?;
        if descriptor.kind != kind {
            return Err(V3StoreError::IdentityMismatch);
        }
        charge_closure(&mut canonical_bytes, descriptor.canonical_byte_len)?;
        let bytes =
            read_canonical_object(repo.cas().as_ref(), &descriptor, MAX_OBJECT_BYTES).await?;
        result.objects.insert(id.clone(), descriptor);
        if kind == ObjectKind::Conflict {
            let conflict = layrs_weave::graph_merge::PlannedConflict::from_canonical_bytes(&bytes)
                .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
            let document = conflict.document();
            conflicts::verify_origins(repo, space, &conflict, known_steps).await?;
            payload_ids.extend(conflicts::required_files(&conflict)?);
            let mut edges = Vec::new();
            for role in ["bases", "sides"] {
                for term in document[role].as_array().expect("validated terms") {
                    let step = StepId::parse(
                        term["provenance"]["stepId"]
                            .as_str()
                            .expect("validated Step"),
                    )
                    .map_err(|_| V3StoreError::IdentityMismatch)?;
                    if !known_steps.contains_key(&step) {
                        return Err(V3StoreError::NotFound);
                    }
                    if !new_steps.contains(&step) {
                        let origin = ObjectId::parse(step.as_str()).expect("typed Step ID");
                        if let std::collections::btree_map::Entry::Vacant(slot) =
                            result.objects.entry(origin)
                        {
                            let descriptor =
                                load_space_descriptor(repo.pool(), space, slot.key()).await?;
                            if descriptor.kind != ObjectKind::Step {
                                return Err(V3StoreError::IdentityMismatch);
                            }
                            charge_closure(&mut canonical_bytes, descriptor.canonical_byte_len)?;
                            let bytes = read_canonical_object(
                                repo.cas().as_ref(),
                                &descriptor,
                                MAX_OBJECT_BYTES,
                            )
                            .await?;
                            let record: layrs_core::scm::Step = serde_json::from_slice(&bytes)
                                .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
                            if record.id() != &step {
                                return Err(V3StoreError::IdentityMismatch);
                            }
                            slot.insert(descriptor);
                        }
                    }
                    for placed in term["values"].as_array().expect("validated placements") {
                        let value = &placed["value"];
                        let kind = value["kind"].as_str().expect("validated term kind");
                        if kind == "absent" {
                            continue;
                        }
                        let target = value["objectId"].as_str().expect("validated term object");
                        if kind == "file" {
                            let file = FileId::parse(target)
                                .map_err(|_| V3StoreError::IdentityMismatch)?;
                            files.insert(file.clone());
                            edges.push((None, Some(file)));
                        } else {
                            let target = ObjectId::parse(target)
                                .map_err(|_| V3StoreError::IdentityMismatch)?;
                            pending.push((
                                target.clone(),
                                if kind == "tree" {
                                    ObjectKind::Tree
                                } else {
                                    ObjectKind::Symlink
                                },
                            ));
                            edges.push((Some(target), None));
                        }
                    }
                }
            }
            for file in conflict
                .payload_file_ids()
                .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?
            {
                files.insert(file.clone());
                edges.push((None, Some(file)));
            }
            edges.sort();
            edges.dedup();
            result.conflicts.insert(conflict.id());
            result.conflict_documents.insert(conflict.id(), conflict);
            result.conflict_edges.push((id, edges));
            continue;
        }
        if kind == ObjectKind::Tombstone {
            let value: Tombstone = serde_json::from_slice(&bytes)
                .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
            if value.previous_object_id().is_some() {
                return Err(V3StoreError::InvalidDescriptor(
                    "Tombstone history closure admission is not yet supported".into(),
                ));
            }
        }
        if kind != ObjectKind::Tree {
            continue;
        }
        let tree: Tree = serde_json::from_slice(&bytes)
            .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
        for entry in tree.entries() {
            let (id, kind) = match entry.target() {
                TreeEntryTarget::Tree { tree_id } => (tree_id.as_str(), ObjectKind::Tree),
                TreeEntryTarget::Symlink { symlink_id } => {
                    (symlink_id.as_str(), ObjectKind::Symlink)
                }
                TreeEntryTarget::Tombstone { tombstone_id } => {
                    (tombstone_id.as_str(), ObjectKind::Tombstone)
                }
                TreeEntryTarget::Conflict { conflict_id } => {
                    (conflict_id.as_str(), ObjectKind::Conflict)
                }
                TreeEntryTarget::File { file_id } => {
                    files.insert(file_id.clone());
                    continue;
                }
            };
            pending.push((
                ObjectId::parse(id).map_err(|_| V3StoreError::IdentityMismatch)?,
                kind,
            ));
        }
        result.trees.push(tree);
        if pending.len() > 100_000 {
            return Err(V3StoreError::LimitExceeded {
                resource: "transaction closure queue",
                limit: 100_000,
            });
        }
    }
    for file in files {
        let ids: Vec<String> = sqlx::query_scalar("SELECT m.file_manifest_id FROM file_manifests m JOIN space_object_availability a ON a.object_id=m.file_manifest_id WHERE a.space_id=$1 AND m.file_id=$2 AND a.availability_state='available' ORDER BY m.file_manifest_id LIMIT 129")
            .bind(space.as_str()).bind(file.as_str()).fetch_all(repo.pool()).await.map_err(pg_error)?;
        if ids.len() > 128 {
            return Err(V3StoreError::LimitExceeded {
                resource: "manifests per File",
                limit: 128,
            });
        }
        let mut selected = None;
        for id in ids {
            let descriptor = load_space_descriptor(
                repo.pool(),
                space,
                &ObjectId::parse(id).map_err(|_| V3StoreError::IdentityMismatch)?,
            )
            .await?;
            match verified_manifest_budgeted(repo, space, &descriptor, &mut verification_bytes)
                .await
            {
                Ok(manifest) => {
                    if manifest.file_id() != &file {
                        return Err(V3StoreError::IdentityMismatch);
                    }
                    selected = Some((descriptor, manifest));
                    break;
                }
                Err(
                    V3StoreError::NotFound
                    | V3StoreError::IdentityMismatch
                    | V3StoreError::CorruptObject(_),
                ) => continue,
                Err(error) => return Err(error),
            }
        }
        let (descriptor, manifest) = selected.ok_or(V3StoreError::NotFound)?;
        if payload_ids.contains(&file) {
            let bytes =
                conflicts::read_payload_file(repo, space, &manifest, &mut payload_bytes).await?;
            payload.insert(file, bytes);
        }
        charge_closure(&mut canonical_bytes, descriptor.canonical_byte_len)?;
        result
            .objects
            .insert(descriptor.object_id.clone(), descriptor);
        if result.objects.len() > 100_000 {
            return Err(V3StoreError::LimitExceeded {
                resource: "transaction closure",
                limit: 100_000,
            });
        }
        for chunk in manifest.chunks() {
            let descriptor = load_space_descriptor(
                repo.pool(),
                space,
                &ObjectId::parse(chunk.chunk_id().as_str())
                    .map_err(|_| V3StoreError::IdentityMismatch)?,
            )
            .await?;
            if !result.objects.contains_key(&descriptor.object_id) {
                charge_stream(&mut content_bytes, descriptor.canonical_byte_len)?;
                result
                    .objects
                    .insert(descriptor.object_id.clone(), descriptor);
            }
            if result.objects.len() > 100_000 {
                return Err(V3StoreError::LimitExceeded {
                    resource: "transaction closure",
                    limit: 100_000,
                });
            }
        }
    }
    for conflict in result.conflict_documents.values() {
        conflict
            .validate_payload(&payload)
            .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
    }
    renames::validate(
        repo,
        space,
        &mut result,
        known_steps,
        &validation_roots,
        &mut canonical_bytes,
    )
    .await?;
    Ok(result)
}

fn charge_closure(total: &mut u64, bytes: u64) -> V3StoreResult<()> {
    const MAX_CLOSURE: u64 = 256 * 1024 * 1024;
    *total = total
        .checked_add(bytes)
        .ok_or(V3StoreError::LimitExceeded {
            resource: "transaction closure bytes",
            limit: MAX_CLOSURE,
        })?;
    if *total > MAX_CLOSURE {
        return Err(V3StoreError::LimitExceeded {
            resource: "transaction closure bytes",
            limit: MAX_CLOSURE,
        });
    }
    Ok(())
}

fn charge_stream(total: &mut u64, bytes: u64) -> V3StoreResult<()> {
    let next = total
        .checked_add(bytes)
        .filter(|next| *next <= MAX_STREAM_BYTES)
        .ok_or(V3StoreError::LimitExceeded {
            resource: "transaction streamed content bytes",
            limit: MAX_STREAM_BYTES,
        })?;
    *total = next;
    Ok(())
}

pub(crate) async fn insert_tree_edges(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    closure: &VerifiedClosure,
) -> V3StoreResult<()> {
    for tree in &closure.trees {
        for (ordinal, entry) in tree.entries().iter().enumerate() {
            let (object, file) = match entry.target() {
                TreeEntryTarget::Tree { tree_id } => (Some(tree_id.as_str()), None),
                TreeEntryTarget::Symlink { symlink_id } => (Some(symlink_id.as_str()), None),
                TreeEntryTarget::Tombstone { tombstone_id } => (Some(tombstone_id.as_str()), None),
                TreeEntryTarget::Conflict { conflict_id } => (Some(conflict_id.as_str()), None),
                TreeEntryTarget::File { file_id } => (None, Some(file_id.as_str())),
            };
            sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id,target_file_id,repo_path) VALUES($1,$2,'tree_entry',$3,$4,$5) ON CONFLICT DO NOTHING")
            .bind(tree.id().as_str()).bind(ordinal as i32).bind(object).bind(file).bind(entry.name().as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        }
    }
    for (source, edges) in &closure.conflict_edges {
        for (ordinal, (object, file)) in edges.iter().enumerate() {
            sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id,target_file_id) VALUES($1,$2,'conflict_side',$3,$4) ON CONFLICT DO NOTHING")
                .bind(source.as_str()).bind(ordinal as i32).bind(object.as_ref().map(ObjectId::as_str)).bind(file.as_ref().map(FileId::as_str)).execute(&mut **tx).await.map_err(pg_error)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn aggregate_closure_budget_rejects_overflow_and_one_byte_over_limit() {
        let mut total = 0;
        super::charge_closure(&mut total, 256 * 1024 * 1024).unwrap();
        assert!(super::charge_closure(&mut total, 1).is_err());
        let mut total = u64::MAX;
        assert!(super::charge_closure(&mut total, 1).is_err());
    }

    #[test]
    fn streamed_budget_does_not_expand_metadata_or_allow_reconstruction_amplification() {
        let mut streamed = 0;
        for _ in 0..4 {
            super::charge_stream(&mut streamed, super::MAX_FILE_BYTES).unwrap();
        }
        assert!(super::charge_stream(&mut streamed, 1).is_err());
        assert_eq!(streamed, super::MAX_STREAM_BYTES);
        let mut metadata = 0;
        assert!(super::charge_closure(&mut metadata, super::MAX_FILE_BYTES).is_err());
        let mut overflow = u64::MAX;
        assert!(super::charge_stream(&mut overflow, 1).is_err());
        assert_eq!(overflow, u64::MAX);
    }
}
