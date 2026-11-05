//! Immutable named references and publication records.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerKind {
    Checkpoint,
    Milestone,
    Decision,
    Baseline,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    schema_version: &'static str,
    marker_id: MarkerId,
    space_id: SpaceId,
    target_step_id: StepId,
    kind: MarkerKind,
    name: String,
    title: String,
    notes: String,
    created_by: Option<ActorId>,
    created_at: Timestamp,
    proof_ids: Vec<ProofId>,
    metadata: Metadata,
}
impl Marker {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        marker_id: MarkerId,
        space_id: SpaceId,
        target_step_id: StepId,
        kind: MarkerKind,
        name: impl Into<String>,
        title: impl Into<String>,
        notes: impl Into<String>,
        created_by: Option<ActorId>,
        created_at: Timestamp,
        mut proof_ids: Vec<ProofId>,
        metadata: Metadata,
    ) -> Result<Self> {
        let name = name.into();
        let title = title.into();
        let notes = notes.into();
        validate_text("name", &name, 1, 255)?;
        validate_text("title", &title, 0, 1024)?;
        validate_text("notes", &notes, 0, 1_048_576)?;
        sort_unique(&mut proof_ids, "Proof", |v| v.as_str())?;
        Ok(Self {
            schema_version: "layrs.marker.v1",
            marker_id,
            space_id,
            target_step_id,
            kind,
            name,
            title,
            notes,
            created_by,
            created_at,
            proof_ids,
            metadata,
        })
    }
    pub fn id(&self) -> &MarkerId {
        &self.marker_id
    }
    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }
    pub fn target_step_id(&self) -> &StepId {
        &self.target_step_id
    }
    pub fn kind(&self) -> MarkerKind {
        self.kind
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn notes(&self) -> &str {
        &self.notes
    }
    pub fn created_by(&self) -> Option<&ActorId> {
        self.created_by.as_ref()
    }
    pub fn created_at(&self) -> &Timestamp {
        &self.created_at
    }
    pub fn proof_ids(&self) -> &[ProofId] {
        &self.proof_ids
    }
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MarkerWire {
    schema_version: String,
    marker_id: MarkerId,
    space_id: SpaceId,
    target_step_id: StepId,
    kind: MarkerKind,
    name: String,
    title: String,
    notes: String,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    created_by: Option<Option<ActorId>>,
    created_at: Timestamp,
    proof_ids: Vec<ProofId>,
    metadata: Metadata,
}
impl<'de> Deserialize<'de> for Marker {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let w = MarkerWire::deserialize(d)?;
        if w.schema_version != "layrs.marker.v1" {
            return Err(serde::de::Error::custom("unsupported Marker schemaVersion"));
        }
        Self::new(
            w.marker_id,
            w.space_id,
            w.target_step_id,
            w.kind,
            w.name,
            w.title,
            w.notes,
            w.created_by
                .ok_or_else(|| serde::de::Error::missing_field("createdBy"))?,
            w.created_at,
            w.proof_ids,
            w.metadata,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    schema_version: &'static str,
    release_id: ReleaseId,
    space_id: SpaceId,
    target_step_id: StepId,
    version: String,
    title: String,
    notes: String,
    created_by: Option<ActorId>,
    created_at: Timestamp,
    proof_ids: Vec<ProofId>,
    metadata: Metadata,
}
impl Release {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        release_id: ReleaseId,
        space_id: SpaceId,
        target_step_id: StepId,
        version: impl Into<String>,
        title: impl Into<String>,
        notes: impl Into<String>,
        created_by: Option<ActorId>,
        created_at: Timestamp,
        mut proof_ids: Vec<ProofId>,
        metadata: Metadata,
    ) -> Result<Self> {
        let version = version.into();
        let title = title.into();
        let notes = notes.into();
        validate_text("version", &version, 1, 255)?;
        validate_text("title", &title, 0, 1024)?;
        validate_text("notes", &notes, 0, 1_048_576)?;
        sort_unique(&mut proof_ids, "Proof", |v| v.as_str())?;
        Ok(Self {
            schema_version: "layrs.release.v1",
            release_id,
            space_id,
            target_step_id,
            version,
            title,
            notes,
            created_by,
            created_at,
            proof_ids,
            metadata,
        })
    }
    pub fn id(&self) -> &ReleaseId {
        &self.release_id
    }
    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }
    pub fn target_step_id(&self) -> &StepId {
        &self.target_step_id
    }
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn notes(&self) -> &str {
        &self.notes
    }
    pub fn created_by(&self) -> Option<&ActorId> {
        self.created_by.as_ref()
    }
    pub fn created_at(&self) -> &Timestamp {
        &self.created_at
    }
    pub fn proof_ids(&self) -> &[ProofId] {
        &self.proof_ids
    }
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseWire {
    schema_version: String,
    release_id: ReleaseId,
    space_id: SpaceId,
    target_step_id: StepId,
    version: String,
    title: String,
    notes: String,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    created_by: Option<Option<ActorId>>,
    created_at: Timestamp,
    proof_ids: Vec<ProofId>,
    metadata: Metadata,
}
impl<'de> Deserialize<'de> for Release {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let w = ReleaseWire::deserialize(d)?;
        if w.schema_version != "layrs.release.v1" {
            return Err(serde::de::Error::custom(
                "unsupported Release schemaVersion",
            ));
        }
        Self::new(
            w.release_id,
            w.space_id,
            w.target_step_id,
            w.version,
            w.title,
            w.notes,
            w.created_by
                .ok_or_else(|| serde::de::Error::missing_field("createdBy"))?,
            w.created_at,
            w.proof_ids,
            w.metadata,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleasePublication {
    schema_version: &'static str,
    release_publication_id: ReleasePublicationId,
    release_id: ReleaseId,
    space_id: SpaceId,
    operation_id: Option<OperationId>,
    published_by: Option<ActorId>,
    device_id: Option<DeviceId>,
    published_at: Timestamp,
    metadata: Metadata,
}
impl ReleasePublication {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        release_publication_id: ReleasePublicationId,
        release_id: ReleaseId,
        space_id: SpaceId,
        operation_id: Option<OperationId>,
        published_by: Option<ActorId>,
        device_id: Option<DeviceId>,
        published_at: Timestamp,
        metadata: Metadata,
    ) -> Self {
        Self {
            schema_version: "layrs.release-publication.v1",
            release_publication_id,
            release_id,
            space_id,
            operation_id,
            published_by,
            device_id,
            published_at,
            metadata,
        }
    }
    pub fn id(&self) -> &ReleasePublicationId {
        &self.release_publication_id
    }
    pub fn release_id(&self) -> &ReleaseId {
        &self.release_id
    }
    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }
    pub fn operation_id(&self) -> Option<&OperationId> {
        self.operation_id.as_ref()
    }
    pub fn published_by(&self) -> Option<&ActorId> {
        self.published_by.as_ref()
    }
    pub fn device_id(&self) -> Option<&DeviceId> {
        self.device_id.as_ref()
    }
    pub fn published_at(&self) -> &Timestamp {
        &self.published_at
    }
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleasePublicationWire {
    schema_version: String,
    release_publication_id: ReleasePublicationId,
    release_id: ReleaseId,
    space_id: SpaceId,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    operation_id: Option<Option<OperationId>>,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    published_by: Option<Option<ActorId>>,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    device_id: Option<Option<DeviceId>>,
    published_at: Timestamp,
    metadata: Metadata,
}
impl<'de> Deserialize<'de> for ReleasePublication {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let w = ReleasePublicationWire::deserialize(d)?;
        if w.schema_version != "layrs.release-publication.v1" {
            return Err(serde::de::Error::custom(
                "unsupported ReleasePublication schemaVersion",
            ));
        }
        Ok(Self::new(
            w.release_publication_id,
            w.release_id,
            w.space_id,
            w.operation_id
                .ok_or_else(|| serde::de::Error::missing_field("operationId"))?,
            w.published_by
                .ok_or_else(|| serde::de::Error::missing_field("publishedBy"))?,
            w.device_id
                .ok_or_else(|| serde::de::Error::missing_field("deviceId"))?,
            w.published_at,
            w.metadata,
        ))
    }
}
