//! Immutable publication lifecycle evidence; Release manifests never change.
use super::*;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ReleaseLifecycleAction {
    Published {
        publication: Box<ReleasePublication>,
    },
    Revoked {
        publication_id: ReleasePublicationId,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseLifecycleEvent {
    schema_version: &'static str,
    space_id: SpaceId,
    release_id: ReleaseId,
    release_product_object_id: ObjectId,
    step_id: StepId,
    previous_event_object_id: Option<ObjectId>,
    actor_id: ActorId,
    occurred_at: Timestamp,
    #[serde(flatten)]
    action: ReleaseLifecycleAction,
}
impl ReleaseLifecycleEvent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        space_id: SpaceId,
        release_id: ReleaseId,
        release_product_object_id: ObjectId,
        step_id: StepId,
        previous_event_object_id: Option<ObjectId>,
        actor_id: ActorId,
        occurred_at: Timestamp,
        action: ReleaseLifecycleAction,
    ) -> Result<Self> {
        match &action {
            ReleaseLifecycleAction::Published { publication } => {
                if publication.space_id() != &space_id
                    || publication.release_id() != &release_id
                    || publication.published_by() != Some(&actor_id)
                    || publication.published_at() != &occurred_at
                {
                    return Err(invalid("publication binding differs from event"));
                }
            }
            ReleaseLifecycleAction::Revoked { reason, .. } => {
                if previous_event_object_id.is_none()
                    || reason.trim().is_empty()
                    || reason.len() > 4096
                    || reason.contains('\0')
                {
                    return Err(invalid(
                        "revocation requires preceding event and bounded reason",
                    ));
                }
            }
        }
        Ok(Self {
            schema_version: "layrs.release-lifecycle-event.v1",
            space_id,
            release_id,
            release_product_object_id,
            step_id,
            previous_event_object_id,
            actor_id,
            occurred_at,
            action,
        })
    }
    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }
    pub fn release_id(&self) -> &ReleaseId {
        &self.release_id
    }
    pub fn release_product_object_id(&self) -> &ObjectId {
        &self.release_product_object_id
    }
    pub fn step_id(&self) -> &StepId {
        &self.step_id
    }
    pub fn previous_event_object_id(&self) -> Option<&ObjectId> {
        self.previous_event_object_id.as_ref()
    }
    pub fn actor_id(&self) -> &ActorId {
        &self.actor_id
    }
    pub fn occurred_at(&self) -> &Timestamp {
        &self.occurred_at
    }
    pub fn action(&self) -> &ReleaseLifecycleAction {
        &self.action
    }
    pub fn publication_id(&self) -> &ReleasePublicationId {
        match &self.action {
            ReleaseLifecycleAction::Published { publication } => publication.id(),
            ReleaseLifecycleAction::Revoked { publication_id, .. } => publication_id,
        }
    }
    pub fn object_id(&self) -> Result<ObjectId> {
        let bytes = canonical_json(self)?;
        Ok(ObjectId::from_digest(blake3::hash(
            &domain_separated_bytes("layrs-evidence-v1", &bytes),
        )))
    }
}
fn invalid(reason: &str) -> ScmError {
    ScmError::InvalidField {
        field: "releaseLifecycleEvent",
        reason: reason.into(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Wire {
    schema_version: String,
    space_id: SpaceId,
    release_id: ReleaseId,
    release_product_object_id: ObjectId,
    step_id: StepId,
    previous_event_object_id: Option<ObjectId>,
    actor_id: ActorId,
    occurred_at: Timestamp,
    kind: String,
    publication: Option<ReleasePublication>,
    publication_id: Option<ReleasePublicationId>,
    reason: Option<String>,
}
impl<'de> Deserialize<'de> for ReleaseLifecycleEvent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        let wire: Wire = serde_json::from_value(value.clone()).map_err(serde::de::Error::custom)?;
        let object = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("event must be an object"))?;
        if wire.schema_version != "layrs.release-lifecycle-event.v1"
            || !object.contains_key("previousEventObjectId")
        {
            return Err(serde::de::Error::custom(
                "event version or previous event field missing",
            ));
        }
        let action = match wire.kind.as_str() {
            "published"
                if !object.contains_key("publicationId") && !object.contains_key("reason") =>
            {
                ReleaseLifecycleAction::Published {
                    publication: Box::new(
                        wire.publication
                            .ok_or_else(|| serde::de::Error::custom("publication missing"))?,
                    ),
                }
            }
            "revoked" if !object.contains_key("publication") => ReleaseLifecycleAction::Revoked {
                publication_id: wire
                    .publication_id
                    .ok_or_else(|| serde::de::Error::custom("publicationId missing"))?,
                reason: wire
                    .reason
                    .ok_or_else(|| serde::de::Error::custom("reason missing"))?,
            },
            _ => return Err(serde::de::Error::custom("invalid release event variant")),
        };
        Self::new(
            wire.space_id,
            wire.release_id,
            wire.release_product_object_id,
            wire.step_id,
            wire.previous_event_object_id,
            wire.actor_id,
            wire.occurred_at,
            action,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn publication() -> ReleaseLifecycleEvent {
        let space = SpaceId::parse("space:release-test").unwrap();
        let release = ReleaseId::parse("release:test").unwrap();
        let actor = ActorId::parse("actor:publisher").unwrap();
        let now = Timestamp::parse("2026-09-20T12:00:00Z").unwrap();
        let publication = ReleasePublication::new(
            ReleasePublicationId::parse("release-publication:test").unwrap(),
            release.clone(),
            space.clone(),
            None,
            Some(actor.clone()),
            None,
            now.clone(),
            Metadata::empty(),
        );
        ReleaseLifecycleEvent::new(
            space,
            release,
            ObjectId::parse(format!("blake3:{}", "1".repeat(64))).unwrap(),
            StepId::parse(format!("blake3:{}", "2".repeat(64))).unwrap(),
            None,
            actor,
            now,
            ReleaseLifecycleAction::Published {
                publication: Box::new(publication),
            },
        )
        .unwrap()
    }
    #[test]
    fn lifecycle_roundtrip_keeps_publication_immutable_and_hashes_evidence() {
        let event = publication();
        let bytes = canonical_json(&event).unwrap();
        assert_eq!(
            serde_json::from_slice::<ReleaseLifecycleEvent>(&bytes).unwrap(),
            event
        );
        let revoked = ReleaseLifecycleEvent::new(
            event.space_id().clone(),
            event.release_id().clone(),
            event.release_product_object_id().clone(),
            event.step_id().clone(),
            Some(event.object_id().unwrap()),
            event.actor_id().clone(),
            event.occurred_at().clone(),
            ReleaseLifecycleAction::Revoked {
                publication_id: event.publication_id().clone(),
                reason: "withdrawn".into(),
            },
        )
        .unwrap();
        assert_ne!(event.object_id().unwrap(), revoked.object_id().unwrap());
        assert_eq!(canonical_json(&event).unwrap(), bytes);
        assert_eq!(
            serde_json::from_slice::<ReleaseLifecycleEvent>(&canonical_json(&revoked).unwrap())
                .unwrap(),
            revoked
        );
    }
    #[test]
    fn lifecycle_rejects_forged_bindings_ambiguous_variants_and_unknown_schema() {
        let original = serde_json::to_value(publication()).unwrap();
        for (field, value) in [
            ("spaceId", serde_json::json!("space:other")),
            ("releaseId", serde_json::json!("release:other")),
            ("actorId", serde_json::json!("actor:other")),
            ("occurredAt", serde_json::json!("2026-09-21T12:00:00Z")),
            ("schemaVersion", serde_json::json!("other")),
            ("reason", serde_json::Value::Null),
            ("unexpected", serde_json::json!(true)),
        ] {
            let mut malformed = original.clone();
            malformed[field] = value;
            assert!(
                serde_json::from_value::<ReleaseLifecycleEvent>(malformed).is_err(),
                "{field}"
            );
        }
        let mut missing = original;
        missing
            .as_object_mut()
            .unwrap()
            .remove("previousEventObjectId");
        assert!(serde_json::from_value::<ReleaseLifecycleEvent>(missing).is_err());
    }
}
