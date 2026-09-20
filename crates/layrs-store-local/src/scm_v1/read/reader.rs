//! Durable typed reader implementation.
use super::*;
impl ScmReader for NativeScmReader<'_> {
    fn products_in_space(&self, space: &SpaceId) -> Result<Vec<layrs_core::scm::ObjectId>> {
        let connection = self.connection()?;
        let mut query=connection.prepare("SELECT p.product_object_id FROM products p JOIN space_object_admissions a ON a.object_id=p.product_object_id WHERE a.space_id=?1 ORDER BY p.product_object_id LIMIT 16385").map_err(sqlite_backend)?;
        let ids = query
            .query_map([space.as_str()], |row| row.get::<_, String>(0))
            .map_err(sqlite_backend)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_backend)?;
        if ids.len() > 16384 {
            return Err(backend("Product listing limit exceeded"));
        }
        ids.into_iter()
            .map(|id| layrs_core::scm::ObjectId::parse(id).map_err(|e| corruption(e.to_string())))
            .collect()
    }
    fn get_product(
        &self,
        id: &layrs_core::scm::ObjectId,
    ) -> Result<Option<layrs_store::scm::CanonicalProduct>> {
        match self.read_object(&StoreObjectId::Product(id.clone()))? {
            None => Ok(None),
            Some(CanonicalObject::Product(value)) => {
                let logical: Option<String> = self
                    .connection()?
                    .query_row(
                        "SELECT product_id FROM products WHERE product_object_id=?1",
                        [id.as_str()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(sqlite_backend)?;
                if logical.as_deref() != Some(value.envelope().logical_id()) {
                    return Err(corruption(format!("Product projection mismatch for {id}")));
                }
                let edges = value
                    .direct_dependencies()
                    .iter()
                    .enumerate()
                    .map(|(i, id)| ("product_dependency".to_owned(), i as u64, id.to_string()))
                    .collect();
                self.validate_object_edges(&StoreObjectId::Product(id.clone()), edges)?;
                Ok(Some(value))
            }
            Some(_) => Err(corruption(format!("CAS kind mismatch for Product {id}"))),
        }
    }
    fn get_chunk(&self, id: &ChunkId) -> Result<Option<Chunk>> {
        match self.read_object(&StoreObjectId::ChunkRaw(id.clone()))? {
            None => Ok(None),
            Some(CanonicalObject::ChunkRaw(value)) => {
                self.validate_object_edges(&StoreObjectId::ChunkRaw(id.clone()), vec![])?;
                Ok(Some(value))
            }
            Some(_) => Err(corruption(format!("CAS kind mismatch for Chunk {id}"))),
        }
    }

    fn get_file_manifest(&self, id: &FileManifestId) -> Result<Option<FileManifest>> {
        match self.read_object(&StoreObjectId::FileManifest(id.clone()))? {
            None => Ok(None),
            Some(CanonicalObject::FileManifest(value)) => {
                self.validate_manifest_projection(&value)?;
                Ok(Some(value))
            }
            Some(_) => Err(corruption(format!(
                "CAS kind mismatch for FileManifest {id}"
            ))),
        }
    }

    fn get_tree(&self, id: &TreeId) -> Result<Option<Tree>> {
        match self.read_object(&StoreObjectId::Tree(id.clone()))? {
            None => Ok(None),
            Some(CanonicalObject::Tree(value)) => {
                self.validate_tree_projection(&value)?;
                Ok(Some(value))
            }
            Some(_) => Err(corruption(format!("CAS kind mismatch for Tree {id}"))),
        }
    }

    fn get_symlink(&self, id: &SymlinkId) -> Result<Option<Symlink>> {
        match self.read_object(&StoreObjectId::Symlink(id.clone()))? {
            None => Ok(None),
            Some(CanonicalObject::Symlink(value)) => {
                self.validate_object_edges(&StoreObjectId::Symlink(id.clone()), vec![])?;
                Ok(Some(value))
            }
            Some(_) => Err(corruption(format!("CAS kind mismatch for Symlink {id}"))),
        }
    }

    fn get_tombstone(&self, id: &TombstoneId) -> Result<Option<Tombstone>> {
        match self.read_object(&StoreObjectId::Tombstone(id.clone()))? {
            None => Ok(None),
            Some(CanonicalObject::Tombstone(value)) => {
                self.validate_object_edges(&StoreObjectId::Tombstone(id.clone()), vec![])?;
                Ok(Some(value))
            }
            Some(_) => Err(corruption(format!("CAS kind mismatch for Tombstone {id}"))),
        }
    }

    fn get_step(&self, id: &StepId) -> Result<Option<Step>> {
        match self.read_object(&StoreObjectId::Step(id.clone()))? {
            None => Ok(None),
            Some(CanonicalObject::Step(value)) => {
                self.validate_step_projection(&value)?;
                Ok(Some(value))
            }
            Some(_) => Err(corruption(format!("CAS kind mismatch for Step {id}"))),
        }
    }

    fn get_repository_view(&self, id: &RepositoryViewId) -> Result<Option<RepositoryView>> {
        match self.read_object(&StoreObjectId::RepositoryView(id.clone()))? {
            None => Ok(None),
            Some(CanonicalObject::RepositoryView(value)) => {
                self.validate_view_projection(&value)?;
                Ok(Some(value))
            }
            Some(_) => Err(corruption(format!(
                "CAS kind mismatch for RepositoryView {id}"
            ))),
        }
    }

    fn get_conflict(&self, id: &ConflictId) -> Result<Option<CanonicalConflict>> {
        match self.read_object(&StoreObjectId::Conflict(id.clone()))? {
            None => Ok(None),
            Some(CanonicalObject::Conflict(value)) => {
                // Conflict structure remains opaque until Phase 5. Accepting a
                // conflict_side projection that cannot yet be reconstructed
                // from typed Core data would turn it into an authorization
                // bypass, so the only valid V1 projection is currently empty.
                self.validate_object_edges(&StoreObjectId::Conflict(id.clone()), vec![])?;
                Ok(Some(value))
            }
            Some(_) => Err(corruption(format!("CAS kind mismatch for Conflict {id}"))),
        }
    }

    fn get_space(&self, id: &SpaceId) -> Result<Option<Space>> {
        let value: Option<Space> =
            self.entity_payload("spaces", "space_id", id.as_str(), "Space")?;
        if let Some(value) = &value {
            self.validate_scalar_projections(
                "spaces",
                "space_id",
                id.as_str(),
                value,
                &[("space_id", "spaceId")],
            )?;
            self.validate_timestamp_ms(
                "spaces",
                "space_id",
                id.as_str(),
                "created_at_ms",
                value.created_at(),
            )?;
        }
        Ok(value)
    }

    fn get_layer(&self, id: &LayerId) -> Result<Option<Layer>> {
        let value = self.entity_payload("layers", "layer_id", id.as_str(), "Layer")?;
        if let Some(value) = &value {
            self.validate_layer_projection(value)?;
            self.validate_timestamp_ms(
                "layers",
                "layer_id",
                id.as_str(),
                "created_at_ms",
                value.created_at(),
            )?;
        }
        Ok(value)
    }

    fn get_step_observation(&self, id: &ObservationId) -> Result<Option<StepObservation>> {
        let value = self.entity_payload(
            "step_observations",
            "observation_id",
            id.as_str(),
            "StepObservation",
        )?;
        if let Some(value) = &value {
            self.validate_scalar_projections(
                "step_observations",
                "observation_id",
                id.as_str(),
                value,
                &[
                    ("observation_id", "observationId"),
                    ("step_id", "stepId"),
                    ("space_id", "spaceId"),
                    ("operation_id", "operationId"),
                    ("worktree_id", "worktreeId"),
                    ("actor_id", "actorId"),
                    ("device_id", "deviceId"),
                    ("observed_at", "observedAt"),
                    ("capture_reason", "captureReason"),
                ],
            )?;
        }
        Ok(value)
    }

    fn get_operation(&self, id: &OperationId) -> Result<Option<Operation>> {
        let value = self.entity_payload("operations", "operation_id", id.as_str(), "Operation")?;
        if let Some(value) = &value {
            self.validate_operation_projection(value)?;
        }
        Ok(value)
    }

    fn get_worktree(&self, id: &WorktreeId) -> Result<Option<Worktree>> {
        let value: Option<Worktree> =
            self.entity_payload("worktrees", "worktree_id", id.as_str(), "Worktree")?;
        if let Some(value) = &value {
            self.validate_scalar_projections(
                "worktrees",
                "worktree_id",
                id.as_str(),
                value,
                &[
                    ("worktree_id", "worktreeId"),
                    ("store_id", "storeId"),
                    ("space_id", "spaceId"),
                    ("active_operation_id", "activeOperationId"),
                    ("checkout_root", "checkoutRoot"),
                    ("filesystem_identity", "filesystemIdentity"),
                ],
            )?;
            self.validate_timestamp_ms(
                "worktrees",
                "worktree_id",
                id.as_str(),
                "created_at_ms",
                value.created_at(),
            )?;
        }
        Ok(value)
    }

    fn get_marker(&self, id: &MarkerId) -> Result<Option<Marker>> {
        let value: Option<Marker> =
            self.entity_payload("markers", "marker_id", id.as_str(), "Marker")?;
        if let Some(value) = &value {
            self.validate_scalar_projections(
                "markers",
                "marker_id",
                id.as_str(),
                value,
                &[
                    ("marker_id", "markerId"),
                    ("space_id", "spaceId"),
                    ("target_step_id", "targetStepId"),
                    ("marker_kind", "kind"),
                    ("created_at", "createdAt"),
                ],
            )?;
            self.validate_proofs(
                "marker_proofs",
                "marker_id",
                id.as_str(),
                value.proof_ids().iter().map(ToString::to_string),
            )?;
        }
        Ok(value)
    }

    fn get_release(&self, id: &ReleaseId) -> Result<Option<Release>> {
        let value: Option<Release> =
            self.entity_payload("releases", "release_id", id.as_str(), "Release")?;
        if let Some(value) = &value {
            self.validate_scalar_projections(
                "releases",
                "release_id",
                id.as_str(),
                value,
                &[
                    ("release_id", "releaseId"),
                    ("space_id", "spaceId"),
                    ("target_step_id", "targetStepId"),
                    ("version", "version"),
                    ("created_at", "createdAt"),
                ],
            )?;
            self.validate_proofs(
                "release_proofs",
                "release_id",
                id.as_str(),
                value.proof_ids().iter().map(ToString::to_string),
            )?;
        }
        Ok(value)
    }

    fn get_release_publication(
        &self,
        id: &ReleasePublicationId,
    ) -> Result<Option<ReleasePublication>> {
        let value = self.entity_payload(
            "release_publications",
            "release_publication_id",
            id.as_str(),
            "ReleasePublication",
        )?;
        if let Some(value) = &value {
            self.validate_scalar_projections(
                "release_publications",
                "release_publication_id",
                id.as_str(),
                value,
                &[
                    ("release_publication_id", "releasePublicationId"),
                    ("release_id", "releaseId"),
                    ("space_id", "spaceId"),
                    ("operation_id", "operationId"),
                    ("published_at", "publishedAt"),
                ],
            )?;
        }
        Ok(value)
    }

    fn file_availability(&self, id: &FileId) -> Result<FileAvailability> {
        self.file_availability_with_budget(
            id,
            DEFAULT_FILE_AVAILABILITY_ROWS,
            DEFAULT_FILE_AVAILABILITY_BYTES,
            DEFAULT_FILE_AVAILABILITY_MANIFESTS,
            DEFAULT_FILE_AVAILABILITY_CHUNKS,
        )
    }

    fn object_admitted_to_space(
        &self,
        object_id: &StoreObjectId,
        space_id: &SpaceId,
    ) -> Result<bool> {
        let admitted = self.exists(
            "SELECT 1 FROM space_object_admissions WHERE object_id = ?1 AND space_id = ?2",
            object_id.as_str(),
            Some(space_id.as_str()),
        )?;
        if !admitted {
            return Ok(false);
        }
        if self.get_space(space_id)?.is_none() {
            return Err(corruption(format!(
                "admission references missing Space {space_id}"
            )));
        }
        if !self.object_exists_typed(object_id)? {
            return Err(corruption(format!(
                "admission references missing or unverified object {object_id}"
            )));
        }
        Ok(true)
    }

    fn is_step_ancestor(&self, ancestor: &StepId, descendant: &StepId) -> Result<bool> {
        if ancestor == descendant {
            return Ok(false);
        }
        let descendant = match self.get_step(descendant)? {
            Some(value) => value,
            None => return Ok(false),
        };
        let mut pending: Vec<_> = descendant.parent_step_ids().to_vec();
        let mut visited = BTreeSet::new();
        while let Some(candidate) = pending.pop() {
            if !visited.insert(candidate.clone()) {
                continue;
            }
            let step = self.require_step(&candidate, "Step DAG")?;
            if &candidate == ancestor {
                return Ok(true);
            }
            pending.extend(step.parent_step_ids().iter().cloned());
        }
        Ok(false)
    }

    fn operation_reaches_step(&self, operation_id: &OperationId, step_id: &StepId) -> Result<bool> {
        let Some(operation) = self.get_operation(operation_id)? else {
            return Ok(false);
        };
        let view = self
            .get_repository_view(operation.resulting_view_id())?
            .ok_or_else(|| {
                corruption(format!(
                    "Operation {operation_id} references missing RepositoryView {}",
                    operation.resulting_view_id()
                ))
            })?;
        for head in view.layer_heads().iter().flat_map(|heads| heads.step_ids()) {
            if self.step_reaches(head, step_id)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn store_exists(&self, id: &StoreId) -> Result<bool> {
        let connection = self.connection()?;
        let payload: Option<String> = connection
            .query_row(
                "SELECT payload_json FROM stores WHERE store_id = ?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_backend)?;
        let Some(payload) = payload else {
            return Ok(false);
        };
        let value: Value = serde_json::from_str(&payload)
            .map_err(|error| corruption(format!("invalid Store payload {id}: {error}")))?;
        if canonical_json(&value).map_err(core_corruption)? != payload.as_bytes()
            || value.get("storeId").and_then(Value::as_str) != Some(id.as_str())
        {
            return Err(corruption(format!(
                "Store payload {id} disagrees with its table projection"
            )));
        }
        Ok(true)
    }

    fn conflict_exists(&self, id: &ConflictId) -> Result<bool> {
        self.get_conflict(id).map(|value| value.is_some())
    }

    fn proof_in_space(&self, id: &ProofId, space_id: &SpaceId) -> Result<bool> {
        let sources = self.tagged_ids(
            "SELECT 'marker', marker_id FROM marker_proofs WHERE proof_id = ?1 \
             UNION ALL SELECT 'release', release_id FROM release_proofs WHERE proof_id = ?1 \
             ORDER BY 1, 2",
            id.as_str(),
        )?;
        for (kind, source) in sources {
            let matches = match kind.as_str() {
                "marker" => {
                    let source = MarkerId::parse(source).map_err(core_corruption)?;
                    self.get_marker(&source)?.is_some_and(|value| {
                        value.space_id() == space_id && value.proof_ids().contains(id)
                    })
                }
                "release" => {
                    let source = ReleaseId::parse(source).map_err(core_corruption)?;
                    self.get_release(&source)?.is_some_and(|value| {
                        value.space_id() == space_id && value.proof_ids().contains(id)
                    })
                }
                _ => return Err(corruption("unknown Proof source kind")),
            };
            if matches {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn actor_exists(&self, id: &ActorId) -> Result<bool> {
        if self.identities.actor_exists(id) {
            return Ok(true);
        }
        let sources = self.tagged_ids(
            "SELECT 'operation', operation_id FROM operations WHERE actor_id = ?1 \
             UNION ALL SELECT 'observation', observation_id FROM step_observations WHERE actor_id = ?1 \
             UNION ALL SELECT 'marker', marker_id FROM markers WHERE json_extract(payload_json, '$.createdBy') = ?1 \
             UNION ALL SELECT 'release', release_id FROM releases WHERE json_extract(payload_json, '$.createdBy') = ?1 \
             UNION ALL SELECT 'publication', release_publication_id FROM release_publications \
                       WHERE json_extract(payload_json, '$.publishedBy') = ?1 \
             ORDER BY 1, 2",
            id.as_str(),
        )?;
        for (kind, source) in sources {
            let matches = match kind.as_str() {
                "operation" => self
                    .get_operation(&OperationId::parse(source).map_err(core_corruption)?)?
                    .is_some_and(|value| value.actor_id() == Some(id)),
                "observation" => self
                    .get_step_observation(&ObservationId::parse(source).map_err(core_corruption)?)?
                    .is_some_and(|value| value.actor_id() == Some(id)),
                "marker" => self
                    .get_marker(&MarkerId::parse(source).map_err(core_corruption)?)?
                    .is_some_and(|value| value.created_by() == Some(id)),
                "release" => self
                    .get_release(&ReleaseId::parse(source).map_err(core_corruption)?)?
                    .is_some_and(|value| value.created_by() == Some(id)),
                "publication" => self
                    .get_release_publication(
                        &ReleasePublicationId::parse(source).map_err(core_corruption)?,
                    )?
                    .is_some_and(|value| value.published_by() == Some(id)),
                _ => return Err(corruption("unknown Actor source kind")),
            };
            if matches {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn device_exists(&self, id: &DeviceId) -> Result<bool> {
        if self.identities.device_exists(id) {
            return Ok(true);
        }
        let sources = self.tagged_ids(
            "SELECT 'operation', operation_id FROM operations WHERE device_id = ?1 \
             UNION ALL SELECT 'observation', observation_id FROM step_observations WHERE device_id = ?1 \
             UNION ALL SELECT 'publication', release_publication_id FROM release_publications \
                       WHERE json_extract(payload_json, '$.deviceId') = ?1 \
             ORDER BY 1, 2",
            id.as_str(),
        )?;
        for (kind, source) in sources {
            let matches = match kind.as_str() {
                "operation" => self
                    .get_operation(&OperationId::parse(source).map_err(core_corruption)?)?
                    .is_some_and(|value| value.device_id() == Some(id)),
                "observation" => self
                    .get_step_observation(&ObservationId::parse(source).map_err(core_corruption)?)?
                    .is_some_and(|value| value.device_id() == Some(id)),
                "publication" => self
                    .get_release_publication(
                        &ReleasePublicationId::parse(source).map_err(core_corruption)?,
                    )?
                    .is_some_and(|value| value.device_id() == Some(id)),
                _ => return Err(corruption("unknown Device source kind")),
            };
            if matches {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn current_repository_view(&self, space_id: &SpaceId) -> Result<Option<CurrentRepositoryView>> {
        let connection = self.connection()?;
        let row: Option<(i64, String)> = connection
            .query_row(
                "SELECT generation, repository_view_id FROM current_repository_views WHERE space_id = ?1",
                [space_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(sqlite_backend)?;
        drop(connection);
        let Some((generation, view_id)) = row else {
            return Ok(None);
        };
        let generation = u64::try_from(generation)
            .map_err(|_| corruption("negative repository View generation"))?;
        let view_id = RepositoryViewId::parse(view_id).map_err(core_corruption)?;
        let view = self.get_repository_view(&view_id)?.ok_or_else(|| {
            corruption(format!(
                "current RepositoryView {view_id} is missing or unverified"
            ))
        })?;
        if view.space_id() != space_id {
            return Err(corruption(format!(
                "current RepositoryView {view_id} belongs to another Space"
            )));
        }
        Ok(Some(CurrentRepositoryView::new(generation, Some(view_id))))
    }

    fn operation_heads(&self, space_id: &SpaceId) -> Result<Vec<OperationId>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT operation_id, ordinal FROM operation_heads \
                 WHERE space_id = ?1 ORDER BY ordinal",
            )
            .map_err(sqlite_backend)?;
        let rows = statement
            .query_map([space_id.as_str()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(sqlite_backend)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_backend)?;
        drop(statement);
        drop(connection);
        let mut result = Vec::new();
        for (expected, (id, ordinal)) in rows.into_iter().enumerate() {
            if ordinal != i64::try_from(expected).unwrap_or(-1) {
                return Err(corruption("operation_heads ordinals are not contiguous"));
            }
            let id = OperationId::parse(id).map_err(core_corruption)?;
            let operation = self.get_operation(&id)?.ok_or_else(|| {
                corruption(format!("Operation head {id} is missing or unverified"))
            })?;
            if operation.space_id() != space_id {
                return Err(corruption(format!(
                    "Operation head {id} belongs to another Space"
                )));
            }
            result.push(id);
        }
        Ok(result)
    }

    fn initializer_in_space(&self, space_id: &SpaceId) -> Result<Option<Operation>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT operation_id FROM operations \
                 WHERE (space_id = ?1 AND operation_kind = 'initialize') \
                    OR (space_id = ?1 \
                        AND json_extract(payload_json, '$.kind') = 'initialize') \
                    OR (json_extract(payload_json, '$.spaceId') = ?1 \
                        AND operation_kind = 'initialize') \
                    OR (json_extract(payload_json, '$.spaceId') = ?1 \
                        AND json_extract(payload_json, '$.kind') = 'initialize') \
                 ORDER BY operation_id LIMIT 2",
            )
            .map_err(sqlite_backend)?;
        let ids = statement
            .query_map([space_id.as_str()], |row| row.get::<_, String>(0))
            .map_err(sqlite_backend)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_backend)?;
        drop(statement);
        drop(connection);
        if ids.len() > 1 {
            return Err(corruption(format!(
                "Space {space_id} has multiple durable Initialize Operations"
            )));
        }
        let Some(id) = ids.into_iter().next() else {
            return Ok(None);
        };
        let id = OperationId::parse(id).map_err(core_corruption)?;
        let operation = self.get_operation(&id)?.ok_or_else(|| {
            corruption(format!(
                "Initialize Operation {id} is missing or fails typed projection validation"
            ))
        })?;
        if operation.space_id() != space_id || operation.kind() != OperationKind::Initialize {
            return Err(corruption(format!(
                "Initialize index row {id} disagrees with its typed Operation payload"
            )));
        }
        Ok(Some(operation))
    }

    fn worktree_active_operation(&self, worktree_id: &WorktreeId) -> Result<Option<OperationId>> {
        self.get_worktree(worktree_id)
            .map(|value| value.map(|worktree| worktree.active_operation_id().clone()))
    }
}
