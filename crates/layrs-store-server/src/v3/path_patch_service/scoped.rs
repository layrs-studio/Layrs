use super::*;
use bytes::Bytes;
use futures_util::stream;
use layrs_core::scm::{Chunk, RepoPath, SpaceId};
use sqlx::Row;

fn session_id(request: &ScopedPathPatchRequest) -> String {
    format!("scoped-patch:{}", &request.request_digest.as_str()[7..])
}

pub(super) fn sync_patch(request: &ScopedPathPatchRequest) -> V3StoreResult<PathPatchRequest> {
    Ok(PathPatchRequest {
        schema_version: PathPatchSchema::V1,
        space_id: request.space_id.clone(),
        client_id: request.client_id.clone(),
        idempotency_key: IdempotencyKey::parse(format!(
            "scoped-v2-{}",
            &request.request_digest.as_str()[7..]
        ))
        .map_err(invalid)?,
        request_digest: request.request_digest.clone(),
        layer_id: request.layer_id.clone(),
        base_step_id: request.base_step_id.clone(),
        expected_repository_view_id: request.expected_repository_view_id.clone(),
        expected_generation: request.expected_generation,
        expected_policy_set_id: request.expected_policy_set_id.clone(),
        expected_authorization_epoch: request.expected_authorization_epoch,
        changes: request.changes.clone(),
    })
}

fn chunk_descriptor(chunk: &layrs_core::scm::FileChunk) -> V3StoreResult<ObjectDescriptor> {
    Ok(ObjectDescriptor {
        kind: ObjectKind::Chunk,
        object_id: ObjectId::parse(chunk.chunk_id().as_str()).map_err(invalid)?,
        canonical_byte_len: chunk.byte_length(),
        encoded_byte_len: chunk.byte_length(),
        compression: Compression::Identity,
    })
}

fn manifest_descriptor(file: &ScopedPatchFile) -> V3StoreResult<(ObjectDescriptor, Vec<u8>)> {
    let bytes = serde_jcs::to_vec(&file.manifest).map_err(invalid)?;
    Ok((
        ObjectDescriptor {
            kind: ObjectKind::FileManifest,
            object_id: ObjectId::parse(file.manifest.id().as_str()).map_err(invalid)?,
            canonical_byte_len: bytes.len() as u64,
            encoded_byte_len: bytes.len() as u64,
            compression: Compression::Identity,
        },
        bytes,
    ))
}

impl PgPathPatchService {
    async fn scoped_is_stale(&self, request: &ScopedPathPatchRequest) -> V3StoreResult<bool> {
        let authority = PgBootstrapStore::new(self.repository.clone(), self.cursors.clone())
            .load_authority(&request.space_id)
            .await?;
        Ok(
            authority.repository_view_id != request.expected_repository_view_id
                || authority.repository_view_generation != request.expected_generation
                || authority.policy_set_id != request.expected_policy_set_id
                || authority.authorization_epoch != request.expected_authorization_epoch,
        )
    }

    pub(super) async fn scoped_replay_authority(
        &self,
        principal: &PrincipalContext,
        request: &ScopedPathPatchRequest,
    ) -> V3StoreResult<()> {
        let authority = PgBootstrapStore::new(self.repository.clone(), self.cursors.clone())
            .load_authority(&request.space_id)
            .await?;
        require_allowed(
            self.repository
                .authorizer()
                .authorize(&AuthorizationCheck {
                    principal: principal.clone(),
                    action: V3Action::ReadProjection,
                    target: AuthorizationTarget::Space(request.space_id.clone()),
                    policy_set_id: authority.policy_set_id.clone(),
                    authorization_epoch: authority.authorization_epoch,
                })
                .await?,
        )?;
        for change in &request.changes {
            require_allowed(
                self.repository
                    .authorizer()
                    .authorize(&AuthorizationCheck {
                        principal: principal.clone(),
                        action: V3Action::ReadProjection,
                        target: AuthorizationTarget::ScopedPath {
                            space_id: request.space_id.clone(),
                            base_step_id: request.base_step_id.clone(),
                            root_tree_id: request.base_root_tree_id.clone(),
                            repo_path: change.path.clone(),
                        },
                        policy_set_id: authority.policy_set_id.clone(),
                        authorization_epoch: authority.authorization_epoch,
                    })
                    .await?,
            )?;
        }
        Ok(())
    }

    async fn replay_scoped_store(
        &self,
        principal: &PrincipalContext,
        source: &ScopedPathPatchRequest,
    ) -> V3StoreResult<Option<ScopedPathPatchReceipt>> {
        match self.replay_scoped(principal, source).await {
            Ok(receipt) => Ok(receipt),
            Err(TransactionServiceError::Store(error)) => Err(error),
            Err(_) => Err(V3StoreError::IdentityMismatch),
        }
    }

    async fn scoped_path_authority(
        &self,
        principal: &PrincipalContext,
        request: &ScopedPathPatchRequest,
        path: &RepoPath,
    ) -> V3StoreResult<()> {
        let authority = PgBootstrapStore::new(self.repository.clone(), self.cursors.clone())
            .load_authority(&request.space_id)
            .await?;
        if authority.repository_view_id != request.expected_repository_view_id
            || authority.repository_view_generation != request.expected_generation
            || authority.policy_set_id != request.expected_policy_set_id
            || authority.authorization_epoch != request.expected_authorization_epoch
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        require_allowed(
            self.repository
                .authorizer()
                .authorize(&AuthorizationCheck {
                    principal: principal.clone(),
                    action: V3Action::ScopedPathWrite,
                    target: AuthorizationTarget::ScopedPath {
                        space_id: request.space_id.clone(),
                        base_step_id: request.base_step_id.clone(),
                        root_tree_id: request.base_root_tree_id.clone(),
                        repo_path: path.clone(),
                    },
                    policy_set_id: authority.policy_set_id,
                    authorization_epoch: authority.authorization_epoch,
                })
                .await?,
        )
    }

    async fn scoped_authority(
        &self,
        principal: &PrincipalContext,
        request: &ScopedPathPatchRequest,
    ) -> V3StoreResult<()> {
        let authority = PgBootstrapStore::new(self.repository.clone(), self.cursors.clone())
            .load_authority(&request.space_id)
            .await?;
        if authority.repository_view_id != request.expected_repository_view_id
            || authority.repository_view_generation != request.expected_generation
            || authority.policy_set_id != request.expected_policy_set_id
            || authority.authorization_epoch != request.expected_authorization_epoch
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        require_allowed(
            self.repository
                .authorizer()
                .authorize(&AuthorizationCheck {
                    principal: principal.clone(),
                    action: V3Action::ReadProjection,
                    target: AuthorizationTarget::Space(request.space_id.clone()),
                    policy_set_id: authority.policy_set_id.clone(),
                    authorization_epoch: authority.authorization_epoch,
                })
                .await?,
        )?;
        let patch = sync_patch(request)?;
        let (transaction, scope, _) = self
            .prepare_request_with_intent(&patch, Some(request))
            .await?;
        if scope
            .decisions(&self.repository, principal, &authority, &transaction, &[])
            .await?
            .iter()
            .any(|decision| !decision.allowed)
        {
            return Err(V3StoreError::InvalidCapability);
        }
        Ok(())
    }

    async fn scoped_session(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        id: &str,
    ) -> V3StoreResult<ScopedPathPatchRequest> {
        let row = sqlx::query("SELECT principal_id,request_digest,request_jcs FROM layrs_collaboration.scoped_patch_sessions WHERE session_id=$1 AND space_id=$2")
            .bind(id).bind(space.as_str()).fetch_optional(self.repository.pool()).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
        if row.get::<String, _>("principal_id") != principal.principal_id.as_str() {
            return Err(V3StoreError::InvalidCapability);
        }
        let canonical: String = row.get("request_jcs");
        if canonical.len() > 4 * 1024 * 1024 {
            return Err(V3StoreError::IdentityMismatch);
        }
        let request: ScopedPathPatchRequest = serde_json::from_str(&canonical).map_err(invalid)?;
        request.validate().map_err(invalid)?;
        if request.space_id != *space
            || session_id(&request) != id
            || request.request_digest.as_str() != row.get::<String, _>("request_digest")
            || serde_jcs::to_string(&request).map_err(invalid)? != canonical
        {
            return Err(V3StoreError::IdentityMismatch);
        }
        Ok(request)
    }

    pub async fn begin_scoped(
        &self,
        principal: &PrincipalContext,
        request: &ScopedPathPatchRequest,
    ) -> V3StoreResult<ScopedPatchProgress> {
        request.validate().map_err(invalid)?;
        let canonical = serde_jcs::to_string(request).map_err(invalid)?;
        if canonical.len() > 4 * 1024 * 1024 {
            return Err(invalid("scoped patch metadata exceeds 4 MiB"));
        }
        let id = session_id(request);
        match self.scoped_session(principal, &request.space_id, &id).await {
            Ok(existing) => {
                if existing != *request {
                    return Err(V3StoreError::IdentityMismatch);
                }
                return self
                    .scoped_progress(principal, &request.space_id, &id)
                    .await;
            }
            Err(V3StoreError::NotFound) => {}
            Err(error) => return Err(error),
        }
        if self.scoped_is_stale(request).await? {
            self.scoped_replay_authority(principal, request).await?;
            // A stale request can only reach a rejected receipt. Still validate
            // its immutable base and patch shape before recording a session.
            self.prepare_request_with_intent(&sync_patch(request)?, Some(request))
                .await?;
        } else {
            self.scoped_authority(principal, request).await?;
        }
        let mut descriptors = BTreeMap::new();
        for file in &request.files {
            let (manifest, _) = manifest_descriptor(file)?;
            descriptors.insert(manifest.object_id.clone(), manifest);
            for chunk in file.manifest.chunks() {
                let descriptor = chunk_descriptor(chunk)?;
                if let Some(old) =
                    descriptors.insert(descriptor.object_id.clone(), descriptor.clone())
                {
                    if old != descriptor {
                        return Err(V3StoreError::IdentityMismatch);
                    }
                }
            }
        }
        let _writer = self.repository.cas_budget_writer().await;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        self.repository.lock_cas_budget(&mut tx).await?;
        transaction_store::lock_identity(&mut tx, principal, &request.space_id).await?;
        let row = sqlx::query("SELECT current_repository_view_id,current_repository_view_generation,current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1 AND lifecycle_state='ready' FOR UPDATE")
            .bind(request.space_id.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(V3StoreError::InvalidCapability)?;
        let stale_locked = row.get::<String, _>("current_repository_view_id")
            != request.expected_repository_view_id.as_str()
            || row.get::<i64, _>("current_repository_view_generation")
                != i64_from_u64(request.expected_generation)?
            || row.get::<String, _>("current_policy_set_id")
                != request.expected_policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch")
                != i64_from_u64(request.expected_authorization_epoch)?;
        if let Some(row) = sqlx::query("SELECT session_id,principal_id,request_digest,request_jcs FROM layrs_collaboration.scoped_patch_sessions WHERE space_id=$1 AND client_id=$2 AND idempotency_key=$3")
            .bind(request.space_id.as_str()).bind(request.client_id.as_str()).bind(request.idempotency_key.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)? {
            if row.get::<String,_>("session_id") != id || row.get::<String,_>("principal_id") != principal.principal_id.as_str()
                || row.get::<String,_>("request_digest") != request.request_digest.as_str() || row.get::<String,_>("request_jcs") != canonical {
                return Err(V3StoreError::InvalidCapability);
            }
        } else {
            let additions: Vec<_> = descriptors.into_values().collect();
            if !stale_locked {
                self.repository.check_cas_budget(&mut tx, None, &additions).await?;
                self.repository.check_cas_budget(&mut tx, Some(&request.space_id), &additions).await?;
            }
            sqlx::query("INSERT INTO layrs_collaboration.scoped_patch_sessions(session_id,space_id,principal_id,client_id,idempotency_key,request_digest,capture_digest,intent,request_jcs,base_step_id,base_root_tree_id,expected_view_id,expected_policy_set_id,expected_authorization_epoch) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)")
                .bind(&id).bind(request.space_id.as_str()).bind(principal.principal_id.as_str()).bind(request.client_id.as_str()).bind(request.idempotency_key.as_str()).bind(request.request_digest.as_str()).bind(request.capture_digest.as_str())
                .bind(match request.intent { ScopedPathPatchIntent::Advance => "advance", ScopedPathPatchIntent::Candidate => "candidate" })
                .bind(&canonical).bind(request.base_step_id.as_str()).bind(request.base_root_tree_id.as_str()).bind(request.expected_repository_view_id.as_str()).bind(request.expected_policy_set_id.as_str()).bind(i64_from_u64(request.expected_authorization_epoch)?)
                .execute(&mut *tx).await.map_err(pg_error)?;
            for descriptor in additions.iter().filter(|_| !stale_locked) {
                sqlx::query("INSERT INTO layrs_collaboration.scoped_patch_reserved_objects(space_id,object_id,object_kind,canonical_byte_length,encoded_byte_length,compression) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
                    .bind(request.space_id.as_str()).bind(descriptor.object_id.as_str()).bind(super::super::object_kind(descriptor.kind)).bind(i64_from_u64(descriptor.canonical_byte_len)?).bind(i64_from_u64(descriptor.encoded_byte_len)?).bind(super::super::compression(descriptor.compression))
                    .execute(&mut *tx).await.map_err(pg_error)?;
            }
            for file in &request.files { for (ordinal, chunk) in file.manifest.chunks().iter().enumerate() {
                sqlx::query("INSERT INTO layrs_collaboration.scoped_patch_chunks(session_id,repo_path,ordinal,object_id,byte_length) VALUES($1,$2,$3,$4,$5)")
                    .bind(&id).bind(file.path.as_str()).bind(ordinal as i32).bind(chunk.chunk_id().as_str()).bind(i64_from_u64(chunk.byte_length())?)
                    .execute(&mut *tx).await.map_err(pg_error)?;
            }}
        }
        tx.commit().await.map_err(pg_error)?;
        self.scoped_progress(principal, &request.space_id, &id)
            .await
    }

    pub async fn scoped_progress(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        id: &str,
    ) -> V3StoreResult<ScopedPatchProgress> {
        let request = self.scoped_session(principal, space, id).await?;
        if self
            .replay_scoped_store(principal, &request)
            .await?
            .is_some()
        {
            self.scoped_replay_authority(principal, &request).await?;
            return Ok(ScopedPatchProgress {
                session_id: id.into(),
                request_digest: request.request_digest,
                missing: vec![],
            });
        }
        self.scoped_replay_authority(principal, &request).await?;
        if self.scoped_is_stale(&request).await? {
            return Ok(ScopedPatchProgress {
                session_id: id.into(),
                request_digest: request.request_digest,
                missing: vec![],
            });
        }
        let rows = sqlx::query("SELECT repo_path,ordinal FROM layrs_collaboration.scoped_patch_chunks WHERE session_id=$1 AND supplied_at IS NULL ORDER BY repo_path,ordinal")
            .bind(id).fetch_all(self.repository.pool()).await.map_err(pg_error)?;
        let missing = rows
            .into_iter()
            .map(|row| {
                let path = RepoPath::parse(row.get::<String, _>("repo_path")).map_err(invalid)?;
                let file_index = request
                    .files
                    .iter()
                    .position(|file| file.path == path)
                    .ok_or(V3StoreError::IdentityMismatch)?;
                Ok(ScopedMissingChunk {
                    path,
                    file_index: u32::try_from(file_index).map_err(invalid)?,
                    ordinal: u32::try_from(row.get::<i32, _>("ordinal")).map_err(invalid)?,
                })
            })
            .collect::<V3StoreResult<Vec<_>>>()?;
        let stale = self.scoped_is_stale(&request).await?;
        Ok(ScopedPatchProgress {
            session_id: id.into(),
            request_digest: request.request_digest,
            missing: if stale { vec![] } else { missing },
        })
    }

    pub async fn upload_scoped_chunk(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        id: &str,
        file_index: usize,
        ordinal: usize,
        bytes: Vec<u8>,
    ) -> V3StoreResult<()> {
        if bytes.is_empty() || bytes.len() > SCOPED_CHUNK_BYTES as usize {
            return Err(invalid("scoped chunk exceeds 1 MiB"));
        }
        let request = self.scoped_session(principal, space, id).await?;
        let file = request
            .files
            .get(file_index)
            .ok_or(V3StoreError::InvalidCapability)?;
        let path = &file.path;
        self.scoped_path_authority(principal, &request, path)
            .await?;
        let entry = file
            .manifest
            .chunks()
            .get(ordinal)
            .ok_or(V3StoreError::InvalidCapability)?;
        if entry.byte_length() != bytes.len() as u64 {
            return Err(V3StoreError::IdentityMismatch);
        }
        let chunk = Chunk::new(bytes.clone()).map_err(invalid)?;
        if chunk.id() != entry.chunk_id() {
            return Err(V3StoreError::IdentityMismatch);
        }
        let descriptor = chunk_descriptor(entry)?;
        // The entire supplied body is hashed above, even if this object already exists in CAS.
        self.repository
            .cas()
            .put_if_absent(
                &descriptor,
                Box::pin(stream::once(async move { Ok(Bytes::from(bytes)) })),
            )
            .await?;
        let object = self.repository.cas().verify_existing(&descriptor).await?;
        let _writer = self.repository.cas_budget_writer().await;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        self.repository.lock_cas_budget(&mut tx).await?;
        transaction_store::lock_identity(&mut tx, principal, space).await?;
        let row = sqlx::query("SELECT current_repository_view_id,current_repository_view_generation,current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1 AND lifecycle_state='ready' FOR UPDATE")
            .bind(space.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(V3StoreError::InvalidCapability)?;
        if row.get::<String, _>("current_repository_view_id")
            != request.expected_repository_view_id.as_str()
            || row.get::<i64, _>("current_repository_view_generation")
                != i64_from_u64(request.expected_generation)?
            || row.get::<String, _>("current_policy_set_id")
                != request.expected_policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch")
                != i64_from_u64(request.expected_authorization_epoch)?
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        let row = sqlx::query("SELECT object_id,byte_length FROM layrs_collaboration.scoped_patch_chunks WHERE session_id=$1 AND repo_path=$2 AND ordinal=$3 FOR UPDATE")
            .bind(id).bind(path.as_str()).bind(ordinal as i32).fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(V3StoreError::InvalidCapability)?;
        if row.get::<String, _>("object_id") != descriptor.object_id.as_str()
            || row.get::<i64, _>("byte_length") != i64_from_u64(descriptor.canonical_byte_len)?
        {
            return Err(V3StoreError::IdentityMismatch);
        }
        let catalog = PgCatalogStore::new(self.repository.clone());
        catalog.install_prepared(&mut tx, &object).await?;
        catalog
            .make_available(&mut tx, space, &descriptor.object_id)
            .await?;
        sqlx::query("INSERT INTO chunks(chunk_id,raw_byte_length,backend_etag) VALUES($1,$2,$3) ON CONFLICT DO NOTHING")
            .bind(descriptor.object_id.as_str()).bind(i64_from_u64(descriptor.canonical_byte_len)?).bind(&object.backend_etag).execute(&mut *tx).await.map_err(pg_error)?;
        sqlx::query("UPDATE layrs_collaboration.scoped_patch_chunks SET supplied_at=COALESCE(supplied_at,transaction_timestamp()) WHERE session_id=$1 AND repo_path=$2 AND ordinal=$3")
            .bind(id).bind(path.as_str()).bind(ordinal as i32).execute(&mut *tx).await.map_err(pg_error)?;
        tx.commit().await.map_err(pg_error)?;
        Ok(())
    }

    pub async fn finalize_scoped(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        id: &str,
    ) -> Result<ScopedPathPatchReceipt, TransactionServiceError> {
        let source = self.scoped_session(principal, space, id).await?;
        let patch = sync_patch(&source)?;
        if let Some(receipt) = self.replay_scoped(principal, &source).await? {
            self.scoped_replay_authority(principal, &source).await?;
            return Ok(receipt);
        }
        self.scoped_replay_authority(principal, &source).await?;
        let (transaction, scope, trees) = self
            .prepare_request_with_intent(&patch, Some(&source))
            .await?;
        if !self.scoped_is_stale(&source).await? {
            let missing: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM layrs_collaboration.scoped_patch_chunks WHERE session_id=$1 AND supplied_at IS NULL",
            )
            .bind(id)
            .fetch_one(self.repository.pool())
            .await
            .map_err(pg_error)?;
            if missing != 0 {
                if !self.scoped_is_stale(&source).await? {
                    return Err(invalid("scoped patch has missing chunks").into());
                }
            } else {
                for file in &source.files {
                    let mut verifier = file.manifest.verifier();
                    for entry in file.manifest.chunks() {
                        let descriptor = chunk_descriptor(entry)?;
                        let bytes = read_canonical_object(
                            self.repository.cas().as_ref(),
                            &descriptor,
                            SCOPED_CHUNK_BYTES,
                        )
                        .await?;
                        verifier.push(&bytes).map_err(invalid)?;
                    }
                    verifier.finish().map_err(invalid)?;
                }
                if let Err(error) = self
                    .install_scoped_metadata(principal, &source, &trees)
                    .await
                {
                    if !self.scoped_is_stale(&source).await? {
                        return Err(error.into());
                    }
                }
            }
        }
        let receipt = PgTransactionStore::new(self.repository.clone(), self.cursors.clone())
            .transact_scoped(principal, &transaction, &scope)
            .await?
            .receipt;
        Ok(ScopedPathPatchReceipt {
            request_digest: source.request_digest.clone(),
            client_id: source.client_id.clone(),
            idempotency_key: source.idempotency_key.clone(),
            capture_digest: source.capture_digest.clone(),
            layer_id: source.layer_id.clone(),
            base_step_id: source.base_step_id.clone(),
            intent: source.intent,
            candidate_id: (source.intent == ScopedPathPatchIntent::Candidate
                && receipt.status == ReceiptStatus::Applied)
                .then(|| format!("candidate:{}", &source.request_digest.as_str()[7..])),
            receipt,
        })
    }

    async fn replay_scoped(
        &self,
        principal: &PrincipalContext,
        source: &ScopedPathPatchRequest,
    ) -> Result<Option<ScopedPathPatchReceipt>, TransactionServiceError> {
        let patch = sync_patch(source)?;
        let row = sqlx::query("SELECT t.principal_id,t.receipt_canonical_jcs,t.request_digest,t.receipt_digest,a.canonical_record_jcs AS snapshot_jcs FROM sync_transactions t JOIN authorization_snapshots a ON a.space_id=t.space_id AND a.authorization_snapshot_id=t.authorization_snapshot_id WHERE t.space_id=$1 AND t.client_id=$2 AND t.idempotency_key=$3")
            .bind(source.space_id.as_str()).bind(source.client_id.as_str()).bind(patch.idempotency_key.as_str()).fetch_optional(self.repository.pool()).await.map_err(pg_error)?;
        let Some(row) = row else { return Ok(None) };
        if row.get::<String, _>("principal_id") != principal.principal_id.as_str() {
            return Err(V3StoreError::InvalidCapability.into());
        }
        let bytes: String = row.get("receipt_canonical_jcs");
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(V3StoreError::IdentityMismatch.into());
        }
        let snapshot_jcs: String = row.get("snapshot_jcs");
        if snapshot_jcs.len() > 16 * 1024 * 1024 {
            return Err(V3StoreError::IdentityMismatch.into());
        }
        let snapshot: AuthorizationSnapshot =
            serde_json::from_str(&snapshot_jcs).map_err(invalid)?;
        snapshot.validate_v3().map_err(invalid)?;
        if snapshot.subject
            != (AuthorizationSubject::Principal {
                principal_id: principal.principal_id.clone(),
            })
            || snapshot.request_binding.path_patch_request_digest.as_ref()
                != Some(&source.request_digest)
            || snapshot.request_binding.client_id != source.client_id
            || snapshot.request_binding.idempotency_key != patch.idempotency_key
            || snapshot.request_binding.request_digest.as_str()
                != row.get::<String, _>("request_digest")
            || serde_jcs::to_string(&snapshot).map_err(invalid)? != snapshot_jcs
        {
            return Err(V3StoreError::IdentityMismatch.into());
        }
        let receipt: SyncTransactionReceipt = serde_json::from_str(&bytes).map_err(invalid)?;
        receipt.validate_v3().map_err(invalid)?;
        if receipt.request_digest.as_str() != row.get::<String, _>("request_digest")
            || receipt.idempotency_key != patch.idempotency_key
            || receipt.authorization_snapshot_id != snapshot.authorization_snapshot_id
            || receipt.computed_storage_digest().map_err(invalid)?.as_str()
                != row.get::<String, _>("receipt_digest")
            || receipt.canonical_storage_bytes().map_err(invalid)? != bytes.as_bytes()
        {
            return Err(V3StoreError::IdentityMismatch.into());
        }
        // The session row is the immutable source binding; no base Tree or prior CAS body is needed.
        Ok(Some(ScopedPathPatchReceipt {
            request_digest: source.request_digest.clone(),
            client_id: source.client_id.clone(),
            idempotency_key: source.idempotency_key.clone(),
            capture_digest: source.capture_digest.clone(),
            layer_id: source.layer_id.clone(),
            base_step_id: source.base_step_id.clone(),
            intent: source.intent,
            candidate_id: (source.intent == ScopedPathPatchIntent::Candidate
                && receipt.status == ReceiptStatus::Applied)
                .then(|| format!("candidate:{}", &source.request_digest.as_str()[7..])),
            receipt,
        }))
    }

    async fn install_scoped_metadata(
        &self,
        principal: &PrincipalContext,
        source: &ScopedPathPatchRequest,
        trees: &[Tree],
    ) -> V3StoreResult<()> {
        let mut objects = BTreeMap::new();
        for file in &source.files {
            let (descriptor, bytes) = manifest_descriptor(file)?;
            objects.insert(descriptor.object_id.clone(), (descriptor, bytes));
        }
        for tree in trees {
            let bytes = serde_jcs::to_vec(tree).map_err(invalid)?;
            let descriptor = ObjectDescriptor {
                kind: ObjectKind::Tree,
                object_id: ObjectId::parse(tree.id().as_str()).map_err(invalid)?,
                canonical_byte_len: bytes.len() as u64,
                encoded_byte_len: bytes.len() as u64,
                compression: Compression::Identity,
            };
            objects.insert(descriptor.object_id.clone(), (descriptor, bytes));
        }
        let descriptors: Vec<_> = objects
            .values()
            .map(|(descriptor, _)| descriptor.clone())
            .collect();
        // Reserve generated Trees before their CAS writes. Manifests were reserved at session start.
        {
            let _writer = self.repository.cas_budget_writer().await;
            let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
            self.repository.lock_cas_budget(&mut tx).await?;
            transaction_store::lock_identity(&mut tx, principal, &source.space_id).await?;
            self.repository
                .check_cas_budget(&mut tx, None, &descriptors)
                .await?;
            self.repository
                .check_cas_budget(&mut tx, Some(&source.space_id), &descriptors)
                .await?;
            for descriptor in &descriptors {
                sqlx::query("INSERT INTO layrs_collaboration.scoped_patch_reserved_objects(space_id,object_id,object_kind,canonical_byte_length,encoded_byte_length,compression) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
                    .bind(source.space_id.as_str()).bind(descriptor.object_id.as_str()).bind(super::super::object_kind(descriptor.kind)).bind(i64_from_u64(descriptor.canonical_byte_len)?).bind(i64_from_u64(descriptor.encoded_byte_len)?).bind(super::super::compression(descriptor.compression))
                    .execute(&mut *tx).await.map_err(pg_error)?;
            }
            tx.commit().await.map_err(pg_error)?;
        }
        let mut prepared = Vec::new();
        for (descriptor, bytes) in objects.into_values() {
            self.repository
                .cas()
                .put_if_absent(
                    &descriptor,
                    Box::pin(stream::once(async move { Ok(Bytes::from(bytes)) })),
                )
                .await?;
            prepared.push(self.repository.cas().verify_existing(&descriptor).await?);
        }
        let _writer = self.repository.cas_budget_writer().await;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        self.repository.lock_cas_budget(&mut tx).await?;
        transaction_store::lock_identity(&mut tx, principal, &source.space_id).await?;
        let row = sqlx::query("SELECT current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1 FOR UPDATE")
            .bind(source.space_id.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if row.get::<String, _>("current_policy_set_id") != source.expected_policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch")
                != i64_from_u64(source.expected_authorization_epoch)?
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        let catalog = PgCatalogStore::new(self.repository.clone());
        for object in &prepared {
            catalog.install_prepared(&mut tx, object).await?;
            catalog
                .make_available(&mut tx, &source.space_id, &object.descriptor.object_id)
                .await?;
        }
        for file in &source.files {
            super::super::closure::insert_manifest(&mut tx, &file.manifest).await?;
        }
        tx.commit().await.map_err(pg_error)?;
        Ok(())
    }
}
