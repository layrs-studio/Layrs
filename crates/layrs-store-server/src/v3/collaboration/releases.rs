//! Append-only release publication lifecycle, scoped and checked against CAS.
use super::*;
use layrs_core::scm::{
    Metadata, Release, ReleaseId, ReleaseLifecycleAction, ReleaseLifecycleEvent,
    ReleasePublication, ReleasePublicationId,
};
use layrs_sync::v3::{ProductEnvelope, ProductRoot};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishReleaseRequest {
    pub request_id: String,
    pub expected_last_event_id: Option<ObjectId>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevokeReleaseRequest {
    pub request_id: String,
    pub expected_last_event_id: Option<ObjectId>,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseEventEntry {
    pub event_object_id: ObjectId,
    pub event: ReleaseLifecycleEvent,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleasePublicationHistory {
    pub space_id: SpaceId,
    pub release_id: ReleaseId,
    pub last_event_object_id: Option<ObjectId>,
    pub active_publication_id: Option<ReleasePublicationId>,
    pub events: Vec<ReleaseEventEntry>,
}
const MAX_EVENTS: usize = 1024;
fn validate_key(key: &str) -> V3StoreResult<()> {
    if !(16..=128).contains(&key.len())
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
    {
        return Err(bad("invalid release requestId"));
    }
    Ok(())
}
impl PgCollaborationStore {
    async fn release_manifest(
        &self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        release: &ReleaseId,
        action: V3Action,
    ) -> V3StoreResult<(ObjectId, Release)> {
        let raw=sqlx::query("SELECT p.product_object_id,p.canonical_envelope_jcs FROM products p JOIN space_object_availability a ON a.space_id=p.space_id AND a.object_id=p.product_object_id WHERE p.space_id=$1 AND p.product_kind='release' AND p.logical_id=$2 AND a.availability_state='available'")
            .bind(authority.space_id.as_str()).bind(release.as_str()).fetch_optional(self.repository.pool()).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
        let object_id: ObjectId = parse(raw.get("product_object_id"))?;
        require_allowed(
            self.repository
                .authorizer()
                .authorize(&AuthorizationCheck {
                    principal: principal.clone(),
                    action,
                    target: AuthorizationTarget::Product {
                        space_id: authority.space_id.clone(),
                        product: ProductRoot::Release {
                            logical_id: release.clone(),
                            object_id: object_id.clone(),
                        },
                    },
                    policy_set_id: authority.policy_set_id.clone(),
                    authorization_epoch: authority.authorization_epoch,
                })
                .await?,
        )?;
        let descriptor =
            load_space_descriptor(self.repository.pool(), &authority.space_id, &object_id).await?;
        let bytes =
            read_canonical_object(self.repository.cas().as_ref(), &descriptor, 1024 * 1024).await?;
        if descriptor.kind != ObjectKind::Product
            || bytes != raw.get::<String, _>("canonical_envelope_jcs").as_bytes()
        {
            return Err(V3StoreError::IdentityMismatch);
        }
        let envelope: ProductEnvelope = serde_json::from_slice(&bytes).map_err(bad)?;
        envelope.verify_identity().map_err(bad)?;
        let ProductEnvelope::Release {
            product_id,
            product_object_id,
            record,
            ..
        } = envelope
        else {
            return Err(V3StoreError::IdentityMismatch);
        };
        if &product_id != release
            || product_object_id != object_id
            || record.space_id() != &authority.space_id
            || record.id() != release
        {
            return Err(V3StoreError::IdentityMismatch);
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM space_steps WHERE space_id=$1 AND step_id=$2)",
        )
        .bind(authority.space_id.as_str())
        .bind(record.target_step_id().as_str())
        .fetch_one(self.repository.pool())
        .await
        .map_err(pg_error)?;
        if !exists {
            return Err(V3StoreError::NotFound);
        }
        Ok((object_id, record))
    }
    async fn release_lock<'a>(
        &'a self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        require_reviewers: bool,
    ) -> V3StoreResult<(
        Transaction<'a, Postgres>,
        std::collections::BTreeSet<String>,
    )> {
        let _budget_writer = self.repository.cas_budget_writer().await;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        self.repository.lock_cas_budget(&mut tx).await?;
        let reviewers = if require_reviewers {
            gates::lock_reviewers(&mut tx, &authority.space_id).await?
        } else {
            Default::default()
        };
        super::super::transaction_store::lock_identity(&mut tx, principal, &authority.space_id)
            .await?;
        let row=sqlx::query("SELECT current_repository_view_id,current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1 AND lifecycle_state='ready' FOR UPDATE").bind(authority.space_id.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(V3StoreError::InvalidCapability)?;
        if row.get::<String, _>("current_repository_view_id")
            != authority.repository_view_id.as_str()
            || row.get::<String, _>("current_policy_set_id") != authority.policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch") != authority.authorization_epoch as i64
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        Ok((tx, reviewers))
    }
    async fn pin_release_objects(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        space: &SpaceId,
        manifest: &ObjectId,
        step: &layrs_core::scm::StepId,
    ) -> V3StoreResult<()> {
        let ids = vec![manifest.as_str(), step.as_str()];
        let rows=sqlx::query("SELECT o.object_id,o.object_kind,o.canonical_byte_length,o.encoded_byte_length,o.compression FROM canonical_objects o JOIN space_object_availability a ON a.object_id=o.object_id WHERE a.space_id=$1 AND a.object_id=ANY($2) AND a.availability_state='available' AND o.verification_state='verified' ORDER BY o.object_id FOR SHARE OF o,a").bind(space.as_str()).bind(ids).fetch_all(&mut **tx).await.map_err(pg_error)?;
        if rows.len() != 2 {
            return Err(V3StoreError::NotFound);
        }
        for row in rows {
            let descriptor = super::super::plan_store::descriptor_from_row(&row)?;
            if descriptor.object_id.as_str() == step.as_str() {
                if descriptor.kind != ObjectKind::Step {
                    return Err(V3StoreError::IdentityMismatch);
                }
                let bytes = read_canonical_object(
                    self.repository.cas().as_ref(),
                    &descriptor,
                    4 * 1024 * 1024,
                )
                .await?;
                let record: layrs_core::scm::Step = serde_json::from_slice(&bytes).map_err(bad)?;
                if record.id() != step {
                    return Err(V3StoreError::IdentityMismatch);
                }
            } else if descriptor.kind != ObjectKind::Product {
                return Err(V3StoreError::IdentityMismatch);
            }
        }
        Ok(())
    }
    async fn release_history_locked(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        space: &SpaceId,
        release: &ReleaseId,
        manifest: &ObjectId,
        step: &layrs_core::scm::StepId,
    ) -> V3StoreResult<ReleasePublicationHistory> {
        self.pin_release_objects(tx, space, manifest, step).await?;
        let rows=sqlx::query("SELECT e.sequence,e.event_object_id,e.previous_event_object_id,e.release_product_object_id,e.step_id,e.publication_id,e.event_kind,p.actor_id,e.canonical_event_jcs FROM layrs_collaboration.release_events e JOIN principals p ON p.principal_id=e.actor_principal_id WHERE e.space_id=$1 AND e.release_id=$2 ORDER BY e.sequence LIMIT 1025")
            .bind(space.as_str()).bind(release.as_str()).fetch_all(&mut **tx).await.map_err(pg_error)?;
        if rows.len() > MAX_EVENTS {
            return Err(V3StoreError::LimitExceeded {
                resource: "release_events",
                limit: MAX_EVENTS as u64,
            });
        }
        let mut history = ReleasePublicationHistory {
            space_id: space.clone(),
            release_id: release.clone(),
            last_event_object_id: None,
            active_publication_id: None,
            events: vec![],
        };
        for (index, row) in rows.into_iter().enumerate() {
            let id: ObjectId = parse(row.get("event_object_id"))?;
            let descriptor_row=sqlx::query("SELECT o.object_id,o.object_kind,o.canonical_byte_length,o.encoded_byte_length,o.compression FROM canonical_objects o JOIN space_object_availability a ON a.object_id=o.object_id WHERE a.space_id=$1 AND a.object_id=$2 AND a.availability_state='available' AND o.verification_state='verified' FOR SHARE OF o,a").bind(space.as_str()).bind(id.as_str()).fetch_optional(&mut **tx).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
            let descriptor = super::super::plan_store::descriptor_from_row(&descriptor_row)?;
            let bytes =
                read_canonical_object(self.repository.cas().as_ref(), &descriptor, 16384).await?;
            let event: ReleaseLifecycleEvent = serde_json::from_slice(&bytes).map_err(bad)?;
            if descriptor.kind != ObjectKind::Evidence
                || event.object_id().map_err(bad)? != id
                || bytes != row.get::<String, _>("canonical_event_jcs").as_bytes()
                || event.space_id() != space
                || event.release_id() != release
                || event.release_product_object_id() != manifest
                || event.step_id() != step
                || row.get::<i64, _>("sequence") != index as i64 + 1
                || event.previous_event_object_id() != history.last_event_object_id.as_ref()
                || row
                    .get::<Option<String>, _>("previous_event_object_id")
                    .as_deref()
                    != event.previous_event_object_id().map(ObjectId::as_str)
                || row.get::<String, _>("release_product_object_id") != manifest.as_str()
                || row.get::<String, _>("step_id") != step.as_str()
                || row.get::<String, _>("publication_id") != event.publication_id().as_str()
                || event.actor_id().as_str() != row.get::<String, _>("actor_id")
            {
                return Err(V3StoreError::IdentityMismatch);
            }
            match event.action() {
                ReleaseLifecycleAction::Published { publication } => {
                    if history.active_publication_id.is_some()
                        || row.get::<String, _>("event_kind") != "published"
                    {
                        return Err(V3StoreError::IdentityMismatch);
                    }
                    history.active_publication_id = Some(publication.id().clone());
                }
                ReleaseLifecycleAction::Revoked { publication_id, .. } => {
                    if history.active_publication_id.as_ref() != Some(publication_id)
                        || row.get::<String, _>("event_kind") != "revoked"
                    {
                        return Err(V3StoreError::IdentityMismatch);
                    }
                    history.active_publication_id = None;
                }
            }
            history.last_event_object_id = Some(id.clone());
            history.events.push(ReleaseEventEntry {
                event_object_id: id,
                event,
            });
        }
        Ok(history)
    }
    pub async fn release_publications(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        release: &ReleaseId,
    ) -> V3StoreResult<ReleasePublicationHistory> {
        let authority = self
            .authority(principal, space, V3Action::ReadReleases)
            .await?;
        let (manifest, record) = self
            .release_manifest(principal, &authority, release, V3Action::ReadReleases)
            .await?;
        let (mut tx, _) = self.release_lock(principal, &authority, false).await?;
        let history = self
            .release_history_locked(&mut tx, space, release, &manifest, record.target_step_id())
            .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(history)
    }
    pub async fn list_releases(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
    ) -> V3StoreResult<serde_json::Value> {
        let authority = self
            .authority(principal, space, V3Action::ReadReleases)
            .await?;
        let ids:Vec<String>=sqlx::query_scalar("SELECT release_id FROM repository_view_releases WHERE repository_view_id=$1 AND space_id=$2 ORDER BY release_id LIMIT 1025").bind(authority.repository_view_id.as_str()).bind(space.as_str()).fetch_all(self.repository.pool()).await.map_err(pg_error)?;
        if ids.len() > 1024 {
            return Err(V3StoreError::LimitExceeded {
                resource: "release_list",
                limit: 1024,
            });
        }
        let mut releases = vec![];
        let mut bindings = vec![];
        let mut response_bytes = 0usize;
        for id in ids {
            let id = parse(id)?;
            let (object, record) = self
                .release_manifest(principal, &authority, &id, V3Action::ReadReleases)
                .await?;
            response_bytes = response_bytes.saturating_add(jcs(&record)?.len() + 128);
            if response_bytes > 4 * 1024 * 1024 {
                return Err(V3StoreError::LimitExceeded {
                    resource: "release_list_bytes",
                    limit: 4 * 1024 * 1024,
                });
            }
            bindings.push((object.clone(), record.target_step_id().clone()));
            releases.push(serde_json::json!({"productObjectId":object,"record":record}));
        }
        let mut tx = self.locked(principal, &authority).await?;
        for (manifest, step) in bindings {
            self.pin_release_objects(&mut tx, space, &manifest, &step)
                .await?;
        }
        tx.commit().await.map_err(pg_error)?;
        Ok(serde_json::json!({"releases":releases}))
    }
    pub async fn publish_release(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        release: &ReleaseId,
        request: PublishReleaseRequest,
    ) -> V3StoreResult<ReleaseEventEntry> {
        validate_key(&request.request_id)?;
        self.release_event(
            principal,
            space,
            release,
            &request.request_id,
            request.expected_last_event_id.as_ref(),
            None,
            digest(&("publish", release, &request))?,
        )
        .await
    }
    pub async fn revoke_release(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        release: &ReleaseId,
        publication: &ReleasePublicationId,
        request: RevokeReleaseRequest,
    ) -> V3StoreResult<ReleaseEventEntry> {
        validate_key(&request.request_id)?;
        if request.reason.trim().is_empty()
            || request.reason.len() > 4096
            || request.reason.contains('\0')
        {
            return Err(bad("invalid revocation reason"));
        }
        self.release_event(
            principal,
            space,
            release,
            &request.request_id,
            request.expected_last_event_id.as_ref(),
            Some((publication, &request.reason)),
            digest(&("revoke", release, publication, &request))?,
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    async fn release_event(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        release: &ReleaseId,
        key: &str,
        expected: Option<&ObjectId>,
        revocation: Option<(&ReleasePublicationId, &str)>,
        request_digest: String,
    ) -> V3StoreResult<ReleaseEventEntry> {
        let action = if revocation.is_some() {
            V3Action::RevokeRelease
        } else {
            V3Action::PublishRelease
        };
        let authority = self.authority(principal, space, action).await?;
        let (manifest, record) = self
            .release_manifest(principal, &authority, release, action)
            .await?;
        let (mut tx, reviewers) = self
            .release_lock(principal, &authority, revocation.is_none())
            .await?;
        let history = self
            .release_history_locked(&mut tx, space, release, &manifest, record.target_step_id())
            .await?;
        if let Some(row)=sqlx::query("SELECT request_digest,event_object_id FROM layrs_collaboration.release_events WHERE space_id=$1 AND release_id=$2 AND actor_principal_id=$3 AND request_id=$4").bind(space.as_str()).bind(release.as_str()).bind(principal.principal_id.as_str()).bind(key).fetch_optional(&mut *tx).await.map_err(pg_error)? {
            if row.get::<String,_>("request_digest")!=request_digest {return Err(V3StoreError::ReleaseStateConflict);}
            let id:String=row.get("event_object_id");
            let entry=history.events.into_iter().find(|e|e.event_object_id.as_str()==id).ok_or(V3StoreError::IdentityMismatch)?;
            tx.commit().await.map_err(pg_error)?;return Ok(entry);
        }
        if history.last_event_object_id.as_ref() != expected {
            return Err(V3StoreError::ReleaseStateConflict);
        }
        if history.events.len() >= MAX_EVENTS {
            return Err(V3StoreError::LimitExceeded {
                resource: "release_events",
                limit: MAX_EVENTS as u64,
            });
        }
        let actor = principal.actor_id.clone();
        let occurred = now()?;
        let body = if let Some((publication, reason)) = revocation {
            if history.active_publication_id.as_ref() != Some(publication) {
                return Err(V3StoreError::ReleaseStateConflict);
            }
            ReleaseLifecycleAction::Revoked {
                publication_id: publication.clone(),
                reason: reason.into(),
            }
        } else {
            if history.active_publication_id.is_some() {
                return Err(V3StoreError::ReleaseStateConflict);
            }
            let visible:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM repository_view_releases WHERE repository_view_id=$1 AND space_id=$2 AND release_id=$3 AND release_product_object_id=$4)").bind(authority.repository_view_id.as_str()).bind(space.as_str()).bind(release.as_str()).bind(manifest.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
            if !visible {
                return Err(V3StoreError::NotFound);
            }
            if gates::enforce_release(
                self.repository.cas().as_ref(),
                &mut tx,
                &authority,
                record.target_step_id(),
                &reviewers,
            )
            .await?
            .is_some()
            {
                return Err(V3StoreError::ReleaseGateRejected);
            }
            let publication = ReleasePublication::new(
                parse(key_id(
                    "release-publication",
                    principal,
                    space,
                    &format!("{release}:{key}"),
                ))?,
                release.clone(),
                space.clone(),
                None,
                Some(actor.clone()),
                None,
                occurred.clone(),
                Metadata::empty(),
            );
            ReleaseLifecycleAction::Published {
                publication: Box::new(publication),
            }
        };
        let event = ReleaseLifecycleEvent::new(
            space.clone(),
            release.clone(),
            manifest.clone(),
            record.target_step_id().clone(),
            history.last_event_object_id.clone(),
            actor,
            occurred,
            body,
        )
        .map_err(bad)?;
        let prepared = self.evidence(&event).await?;
        let id = prepared.descriptor.object_id.clone();
        if id != event.object_id().map_err(bad)? {
            return Err(V3StoreError::IdentityMismatch);
        }
        self.install(&mut tx, space, &prepared).await?;
        let mut dependencies = vec![manifest];
        if let Some(previous) = event.previous_event_object_id() {
            dependencies.push(previous.clone());
        }
        for (ordinal, target) in dependencies.iter().enumerate() {
            sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id) VALUES($1,$2,'evidence',$3) ON CONFLICT DO NOTHING").bind(id.as_str()).bind(ordinal as i32).bind(target.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        }
        let kind = if revocation.is_some() {
            "revoked"
        } else {
            "published"
        };
        sqlx::query("INSERT INTO layrs_collaboration.release_events(space_id,release_id,sequence,event_object_id,previous_event_object_id,release_product_object_id,step_id,publication_id,event_kind,actor_principal_id,request_id,request_digest,canonical_event_jcs) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)")
            .bind(space.as_str()).bind(release.as_str()).bind(history.events.len() as i64+1).bind(id.as_str()).bind(event.previous_event_object_id().map(ObjectId::as_str)).bind(event.release_product_object_id().as_str()).bind(record.target_step_id().as_str()).bind(event.publication_id().as_str()).bind(kind).bind(principal.principal_id.as_str()).bind(key).bind(request_digest).bind(jcs(&event)?).execute(&mut *tx).await.map_err(pg_error)?;
        self.notify(
            &mut tx,
            principal,
            space,
            if revocation.is_some() {
                "release.revoked"
            } else {
                "release.published"
            },
            release.as_str(),
            &id,
        )
        .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(ReleaseEventEntry {
            event_object_id: id,
            event,
        })
    }
}
