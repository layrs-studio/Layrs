use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bytes::Bytes;
use futures_util::stream;
use layrs_core::scm::{Chunk, ChunkingDescriptor, FileId, FileManifest};
use sqlx::Row;

const MAX_BYTES: usize = 16 * 1024 * 1024;
pub(super) struct PreparedContent {
    objects: Vec<(ObjectDescriptor, Vec<u8>)>,
    manifests: Vec<FileManifest>,
}
impl PreparedContent {
    fn add(&mut self, kind: ObjectKind, id: &str, bytes: Vec<u8>) -> V3StoreResult<()> {
        self.objects.push((
            ObjectDescriptor {
                kind,
                object_id: ObjectId::parse(id).map_err(invalid)?,
                canonical_byte_len: bytes.len() as u64,
                encoded_byte_len: bytes.len() as u64,
                compression: Compression::Identity,
            },
            bytes,
        ));
        Ok(())
    }
    pub(super) fn add_trees(&mut self, trees: Vec<Tree>) -> V3StoreResult<()> {
        for tree in trees {
            self.add(
                ObjectKind::Tree,
                tree.id().as_str(),
                serde_jcs::to_vec(&tree).map_err(invalid)?,
            )?;
        }
        Ok(())
    }
}
pub(super) fn decode(submission: &PathPatchSubmission) -> V3StoreResult<PreparedContent> {
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    for file in &submission.files {
        total = total
            .checked_add(file.bytes_base64.len())
            .ok_or_else(|| invalid("patch size overflow"))?;
        if total > (MAX_BYTES / 3 + 1) * 4 || files.insert(&file.path, &file.bytes_base64).is_some()
        {
            return Err(invalid("duplicate or oversized patch contents"));
        }
    }
    let mut result = PreparedContent {
        objects: vec![],
        manifests: vec![],
    };
    let mut decoded_total = 0usize;
    for change in &submission.request.changes {
        if change
            .path
            .as_str()
            .split('/')
            .any(|part| part.eq_ignore_ascii_case(".layrs") || part.eq_ignore_ascii_case(".git"))
        {
            return Err(invalid("patch touches reserved metadata namespace"));
        }
        // Structural conflicts and symlinks require a separately qualified
        // multi-origin contract. This endpoint never treats them as plain files.
        if change
            .expected
            .0
            .iter()
            .chain(change.replacement.0.iter())
            .any(|entry| !matches!(entry.target, TreeEntryTarget::File { .. }))
        {
            return Err(invalid(
                "path patch currently supports ordinary file entries",
            ));
        }
        let Some(replacement) = &change.replacement.0 else {
            continue;
        };
        let encoded = files
            .remove(&change.path)
            .ok_or_else(|| invalid("replacement requires complete supplied file bytes"))?;
        let bytes = STANDARD.decode(encoded).map_err(invalid)?;
        decoded_total = decoded_total
            .checked_add(bytes.len())
            .ok_or_else(|| invalid("patch size overflow"))?;
        if decoded_total > MAX_BYTES || STANDARD.encode(&bytes) != *encoded {
            return Err(invalid("oversized or noncanonical patch contents"));
        }
        if replacement.target.file_id() != Some(&FileId::from_bytes(&bytes)) {
            return Err(V3StoreError::IdentityMismatch);
        }
        let chunks = bytes
            .chunks(1024 * 1024)
            .map(|bytes| Chunk::new(bytes.to_vec()).map_err(invalid))
            .collect::<V3StoreResult<Vec<_>>>()?;
        let manifest = FileManifest::from_chunks(
            ChunkingDescriptor::deterministic_fixed(1024 * 1024).map_err(invalid)?,
            &chunks,
        )
        .map_err(invalid)?;
        for chunk in chunks {
            result.add(
                ObjectKind::Chunk,
                chunk.id().as_str(),
                chunk.bytes().to_vec(),
            )?;
        }
        result.add(
            ObjectKind::FileManifest,
            manifest.id().as_str(),
            serde_jcs::to_vec(&manifest).map_err(invalid)?,
        )?;
        result.manifests.push(manifest);
    }
    if !files.is_empty() {
        return Err(invalid("unsolicited file bytes in patch"));
    }
    Ok(result)
}

impl PgPathPatchService {
    pub(super) async fn install_content(
        &self,
        principal: &PrincipalContext,
        scope: &ScopedPatch,
        request: &SyncTransactionRequest,
        content: &PreparedContent,
    ) -> V3StoreResult<()> {
        if content.objects.is_empty() {
            return Ok(());
        }
        let patch = &scope.request;
        let request_id = RequestId::parse(format!(
            "request:path-patch-{}",
            &patch.request_digest.as_str()[7..]
        ))
        .map_err(invalid)?;
        let now =
            u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp()).map_err(invalid)?;
        let context = PlanContext {
            actor: CapabilityActor {
                principal_id: principal.principal_id.clone(),
                space_id: patch.space_id.clone(),
                client_id: patch.client_id.clone(),
                plan_id: deterministic_plan_id(&patch.space_id, &patch.client_id, &request_id)?,
                request_id,
                authorization_epoch: patch.expected_authorization_epoch,
            },
            now_unix_seconds: now,
            expires_at_unix_seconds: now + 900,
            transfer_base_url: self.transfer_base_url.clone(),
        };
        let descriptors: Vec<_> = content
            .objects
            .iter()
            .map(|(descriptor, _)| (descriptor.object_id.clone(), descriptor.clone()))
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect();
        // Durable ordinary upload reservations account for even crash orphans.
        // Their capabilities remain private to this service; never returned.
        PgPlanStore::new(self.repository.clone(), PlanLimits::default())
            .create_scoped_object_plan(principal, &context, descriptors, scope, request)
            .await?;
        let mut prepared = BTreeMap::new();
        for (descriptor, bytes) in &content.objects {
            let body = Bytes::copy_from_slice(bytes);
            self.repository
                .cas()
                .put_if_absent(descriptor, Box::pin(stream::once(async move { Ok(body) })))
                .await?;
            let object = self.repository.cas().verify_existing(descriptor).await?;
            prepared.insert(descriptor.object_id.clone(), object);
        }
        let _writer = self.repository.cas_budget_writer().await;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        self.repository.lock_cas_budget(&mut tx).await?;
        transaction_store::lock_identity(&mut tx, principal, &patch.space_id).await?;
        let row = sqlx::query("SELECT current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1 FOR UPDATE")
            .bind(patch.space_id.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if row.get::<String, _>("current_policy_set_id") != patch.expected_policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch")
                != i64_from_u64(patch.expected_authorization_epoch)?
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        let catalog = PgCatalogStore::new(self.repository.clone());
        for object in prepared.values() {
            catalog.install_prepared(&mut tx, object).await?;
            catalog
                .make_available(&mut tx, &patch.space_id, &object.descriptor.object_id)
                .await?;
        }
        for object in prepared
            .values()
            .filter(|object| object.descriptor.kind == ObjectKind::Chunk)
        {
            sqlx::query("INSERT INTO chunks(chunk_id,raw_byte_length,backend_etag) VALUES($1,$2,$3) ON CONFLICT DO NOTHING")
                .bind(object.descriptor.object_id.as_str()).bind(i64_from_u64(object.descriptor.canonical_byte_len)?).bind(&object.backend_etag)
                .execute(&mut *tx).await.map_err(pg_error)?;
        }
        // Manifests were built and hashed from the supplied bytes, rather than
        // borrowing an existing manifest as evidence of content possession.
        for manifest in &content.manifests {
            super::super::closure::insert_manifest(&mut tx, manifest).await?;
        }
        tx.commit().await.map_err(pg_error)?;
        Ok(())
    }
}
