use super::canonical::{canonical_json, domain_hash, domain_separated_bytes};
use super::error::{Result, ScmError};
use super::ids::*;
use super::serde_support::deserialize_present_nullable;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

fn validate_text(field: &'static str, value: &str, min: usize, max: usize) -> Result<()> {
    let length = value.chars().count();
    if !(min..=max).contains(&length) {
        return Err(ScmError::InvalidField {
            field,
            reason: format!("length {length} is outside {min}..={max}"),
        });
    }
    Ok(())
}

fn sort_unique<T, F>(values: &mut [T], kind: &'static str, key: F) -> Result<()>
where
    F: Fn(&T) -> &str,
{
    let mut seen = HashSet::with_capacity(values.len());
    for value in values.iter() {
        if !seen.insert(key(value).to_owned()) {
            return Err(ScmError::Duplicate {
                kind,
                value: key(value).to_owned(),
            });
        }
    }
    values.sort_by(|left, right| key(left).as_bytes().cmp(key(right).as_bytes()));
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Timestamp(String);

impl Timestamp {
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.len() > 64 || OffsetDateTime::parse(&value, &Rfc3339).is_err() {
            return Err(ScmError::InvalidField {
                field: "timestamp",
                reason: "expected a valid RFC 3339 date-time".into(),
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetadataEntry {
    key: String,
    value: String,
}

impl MetadataEntry {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Result<Self> {
        let key = key.into();
        let value = value.into();
        validate_text("metadata.key", &key, 1, 128)?;
        let mut chars = key.chars();
        if !chars.next().is_some_and(|ch| ch.is_ascii_alphanumeric())
            || !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':' | '-'))
        {
            return Err(ScmError::InvalidField {
                field: "metadata.key",
                reason: "invalid key syntax".into(),
            });
        }
        validate_text("metadata.value", &value, 0, 16_384)?;
        Ok(Self { key, value })
    }

    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn value(&self) -> &str {
        &self.value
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MetadataEntryWire {
    key: String,
    value: String,
}

impl<'de> Deserialize<'de> for MetadataEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = MetadataEntryWire::deserialize(deserializer)?;
        Self::new(wire.key, wire.value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Metadata(Vec<MetadataEntry>);

impl Metadata {
    pub fn new(mut entries: Vec<MetadataEntry>) -> Result<Self> {
        sort_unique(&mut entries, "metadata key", |entry| entry.key())?;
        Ok(Self(entries))
    }

    pub fn empty() -> Self {
        Self::default()
    }
    pub fn entries(&self) -> &[MetadataEntry] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Metadata {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::new(Vec::<MetadataEntry>::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureReason {
    AutomaticIdle,
    BeforeLayerSwitch,
    BeforeWorktreeSwitch,
    BeforeOperation,
    ManualSnapshot,
    WeaveResult,
    ConflictResolution,
    Rewrite,
    Received,
    Automation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Space {
    schema_version: &'static str,
    space_id: SpaceId,
    name: String,
    created_at: Timestamp,
    metadata: Metadata,
}

impl Space {
    pub fn new(
        space_id: SpaceId,
        name: impl Into<String>,
        created_at: Timestamp,
        metadata: Metadata,
    ) -> Result<Self> {
        let name = name.into();
        validate_text("name", &name, 1, 255)?;
        Ok(Self {
            schema_version: "layrs.space.v1",
            space_id,
            name,
            created_at,
            metadata,
        })
    }
    pub fn id(&self) -> &SpaceId {
        &self.space_id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn created_at(&self) -> &Timestamp {
        &self.created_at
    }
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SpaceWire {
    schema_version: String,
    space_id: SpaceId,
    name: String,
    created_at: Timestamp,
    metadata: Metadata,
}

impl<'de> Deserialize<'de> for Space {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = SpaceWire::deserialize(deserializer)?;
        if wire.schema_version != "layrs.space.v1" {
            return Err(serde::de::Error::custom("unsupported Space schemaVersion"));
        }
        Self::new(wire.space_id, wire.name, wire.created_at, wire.metadata)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerKind {
    Base,
    Exploration,
    Proposal,
    AutomatedResult,
    ReleaseCandidate,
    Published,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerStatus {
    Draft,
    Active,
    Archived,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerSettings {
    automatic_capture: bool,
    automatic_capture_idle_ms: u64,
}

impl LayerSettings {
    pub fn new(automatic_capture: bool, automatic_capture_idle_ms: u64) -> Result<Self> {
        if automatic_capture_idle_ms > 600_000 {
            return Err(ScmError::InvalidField {
                field: "automaticCaptureIdleMs",
                reason: "must be <= 600000".into(),
            });
        }
        Ok(Self {
            automatic_capture,
            automatic_capture_idle_ms,
        })
    }
    pub fn automatic_capture(&self) -> bool {
        self.automatic_capture
    }
    pub fn automatic_capture_idle_ms(&self) -> u64 {
        self.automatic_capture_idle_ms
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LayerSettingsWire {
    automatic_capture: bool,
    automatic_capture_idle_ms: u64,
}

impl<'de> Deserialize<'de> for LayerSettings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = LayerSettingsWire::deserialize(deserializer)?;
        Self::new(wire.automatic_capture, wire.automatic_capture_idle_ms)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    schema_version: &'static str,
    layer_id: LayerId,
    space_id: SpaceId,
    name: String,
    kind: LayerKind,
    status: LayerStatus,
    parent_layer_ids: Vec<LayerId>,
    settings: LayerSettings,
    created_at: Timestamp,
    metadata: Metadata,
}

impl Layer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        layer_id: LayerId,
        space_id: SpaceId,
        name: impl Into<String>,
        kind: LayerKind,
        status: LayerStatus,
        mut parent_layer_ids: Vec<LayerId>,
        settings: LayerSettings,
        created_at: Timestamp,
        metadata: Metadata,
    ) -> Result<Self> {
        let name = name.into();
        validate_text("name", &name, 1, 255)?;
        sort_unique(&mut parent_layer_ids, "parent Layer", |id| id.as_str())?;
        if parent_layer_ids.binary_search(&layer_id).is_ok() {
            return Err(ScmError::InvalidField {
                field: "parentLayerIds",
                reason: "Layer cannot parent itself".into(),
            });
        }
        Ok(Self {
            schema_version: "layrs.layer.v1",
            layer_id,
            space_id,
            name,
            kind,
            status,
            parent_layer_ids,
            settings,
            created_at,
            metadata,
        })
    }
    pub fn id(&self) -> &LayerId {
        &self.layer_id
    }
    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn kind(&self) -> LayerKind {
        self.kind
    }
    pub fn status(&self) -> LayerStatus {
        self.status
    }
    pub fn parent_layer_ids(&self) -> &[LayerId] {
        &self.parent_layer_ids
    }
    pub fn settings(&self) -> &LayerSettings {
        &self.settings
    }
    pub fn created_at(&self) -> &Timestamp {
        &self.created_at
    }
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LayerWire {
    schema_version: String,
    layer_id: LayerId,
    space_id: SpaceId,
    name: String,
    kind: LayerKind,
    status: LayerStatus,
    parent_layer_ids: Vec<LayerId>,
    settings: LayerSettings,
    created_at: Timestamp,
    metadata: Metadata,
}

impl<'de> Deserialize<'de> for Layer {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let w = LayerWire::deserialize(deserializer)?;
        if w.schema_version != "layrs.layer.v1" {
            return Err(serde::de::Error::custom("unsupported Layer schemaVersion"));
        }
        Self::new(
            w.layer_id,
            w.space_id,
            w.name,
            w.kind,
            w.status,
            w.parent_layer_ids,
            w.settings,
            w.created_at,
            w.metadata,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerHeadSet {
    layer_id: LayerId,
    step_ids: Vec<StepId>,
}

impl LayerHeadSet {
    pub fn new(layer_id: LayerId, mut step_ids: Vec<StepId>) -> Result<Self> {
        sort_unique(&mut step_ids, "Layer head", |id| id.as_str())?;
        Ok(Self { layer_id, step_ids })
    }
    pub fn layer_id(&self) -> &LayerId {
        &self.layer_id
    }
    pub fn step_ids(&self) -> &[StepId] {
        &self.step_ids
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LayerHeadSetWire {
    layer_id: LayerId,
    step_ids: Vec<StepId>,
}
impl<'de> Deserialize<'de> for LayerHeadSet {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let w = LayerHeadSetWire::deserialize(d)?;
        Self::new(w.layer_id, w.step_ids).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeStatus {
    Ready,
    Conflicted,
    NeedsRepair,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeViewState {
    worktree_id: WorktreeId,
    active_layer_id: Option<LayerId>,
    base_step_id: Option<StepId>,
    materialized_tree_id: Option<TreeId>,
    status: WorktreeStatus,
}

impl WorktreeViewState {
    pub fn new(
        worktree_id: WorktreeId,
        active_layer_id: Option<LayerId>,
        base_step_id: Option<StepId>,
        materialized_tree_id: Option<TreeId>,
        status: WorktreeStatus,
    ) -> Self {
        Self {
            worktree_id,
            active_layer_id,
            base_step_id,
            materialized_tree_id,
            status,
        }
    }
    pub fn worktree_id(&self) -> &WorktreeId {
        &self.worktree_id
    }
    pub fn active_layer_id(&self) -> Option<&LayerId> {
        self.active_layer_id.as_ref()
    }
    pub fn base_step_id(&self) -> Option<&StepId> {
        self.base_step_id.as_ref()
    }
    pub fn materialized_tree_id(&self) -> Option<&TreeId> {
        self.materialized_tree_id.as_ref()
    }
    pub fn status(&self) -> WorktreeStatus {
        self.status
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorktreeViewStateWire {
    worktree_id: WorktreeId,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    active_layer_id: Option<Option<LayerId>>,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    base_step_id: Option<Option<StepId>>,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    materialized_tree_id: Option<Option<TreeId>>,
    status: WorktreeStatus,
}
impl<'de> Deserialize<'de> for WorktreeViewState {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let w = WorktreeViewStateWire::deserialize(d)?;
        Ok(Self::new(
            w.worktree_id,
            w.active_layer_id
                .ok_or_else(|| serde::de::Error::missing_field("activeLayerId"))?,
            w.base_step_id
                .ok_or_else(|| serde::de::Error::missing_field("baseStepId"))?,
            w.materialized_tree_id
                .ok_or_else(|| serde::de::Error::missing_field("materializedTreeId"))?,
            w.status,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryView {
    schema_version: &'static str,
    repository_view_id: RepositoryViewId,
    space_id: SpaceId,
    layer_heads: Vec<LayerHeadSet>,
    marker_ids: Vec<MarkerId>,
    release_ids: Vec<ReleaseId>,
    worktrees: Vec<WorktreeViewState>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryViewPayload<'a> {
    schema_version: &'static str,
    space_id: &'a SpaceId,
    layer_heads: &'a [LayerHeadSet],
    marker_ids: &'a [MarkerId],
    release_ids: &'a [ReleaseId],
    worktrees: &'a [WorktreeViewState],
}

impl RepositoryView {
    pub fn new(
        space_id: SpaceId,
        mut layer_heads: Vec<LayerHeadSet>,
        mut marker_ids: Vec<MarkerId>,
        mut release_ids: Vec<ReleaseId>,
        mut worktrees: Vec<WorktreeViewState>,
    ) -> Result<Self> {
        sort_unique(&mut layer_heads, "Layer head set", |v| v.layer_id.as_str())?;
        sort_unique(&mut marker_ids, "Marker", |v| v.as_str())?;
        sort_unique(&mut release_ids, "Release", |v| v.as_str())?;
        sort_unique(&mut worktrees, "Worktree state", |v| v.worktree_id.as_str())?;
        let payload = canonical_json(&RepositoryViewPayload {
            schema_version: "layrs.repository-view.v1",
            space_id: &space_id,
            layer_heads: &layer_heads,
            marker_ids: &marker_ids,
            release_ids: &release_ids,
            worktrees: &worktrees,
        })?;
        let repository_view_id =
            RepositoryViewId::from_digest(domain_hash("layrs-repository-view-v1", &payload));
        Ok(Self {
            schema_version: "layrs.repository-view.v1",
            repository_view_id,
            space_id,
            layer_heads,
            marker_ids,
            release_ids,
            worktrees,
        })
    }
    pub fn id(&self) -> &RepositoryViewId {
        &self.repository_view_id
    }
    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }
    pub fn layer_heads(&self) -> &[LayerHeadSet] {
        &self.layer_heads
    }
    pub fn marker_ids(&self) -> &[MarkerId] {
        &self.marker_ids
    }
    pub fn release_ids(&self) -> &[ReleaseId] {
        &self.release_ids
    }
    pub fn worktrees(&self) -> &[WorktreeViewState] {
        &self.worktrees
    }
    pub fn canonical_payload(&self) -> Result<Vec<u8>> {
        canonical_json(&RepositoryViewPayload {
            schema_version: "layrs.repository-view.v1",
            space_id: &self.space_id,
            layer_heads: &self.layer_heads,
            marker_ids: &self.marker_ids,
            release_ids: &self.release_ids,
            worktrees: &self.worktrees,
        })
    }
    pub fn identity_bytes(&self) -> Result<Vec<u8>> {
        Ok(domain_separated_bytes(
            "layrs-repository-view-v1",
            &self.canonical_payload()?,
        ))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RepositoryViewWire {
    schema_version: String,
    repository_view_id: RepositoryViewId,
    space_id: SpaceId,
    layer_heads: Vec<LayerHeadSet>,
    marker_ids: Vec<MarkerId>,
    release_ids: Vec<ReleaseId>,
    worktrees: Vec<WorktreeViewState>,
}
impl<'de> Deserialize<'de> for RepositoryView {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let w = RepositoryViewWire::deserialize(d)?;
        if w.schema_version != "layrs.repository-view.v1" {
            return Err(serde::de::Error::custom(
                "unsupported RepositoryView schemaVersion",
            ));
        }
        let claimed = w.repository_view_id;
        let v = Self::new(
            w.space_id,
            w.layer_heads,
            w.marker_ids,
            w.release_ids,
            w.worktrees,
        )
        .map_err(serde::de::Error::custom)?;
        if v.repository_view_id != claimed {
            return Err(serde::de::Error::custom(ScmError::IdentityMismatch {
                claimed: claimed.to_string(),
                computed: v.repository_view_id.to_string(),
            }));
        }
        Ok(v)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RewriteTarget {
    old_step_id: StepId,
    new_step_ids: Vec<StepId>,
}
impl RewriteTarget {
    pub fn new(old_step_id: StepId, mut new_step_ids: Vec<StepId>) -> Result<Self> {
        sort_unique(&mut new_step_ids, "rewrite target", |v| v.as_str())?;
        Ok(Self {
            old_step_id,
            new_step_ids,
        })
    }
    pub fn old_step_id(&self) -> &StepId {
        &self.old_step_id
    }
    pub fn new_step_ids(&self) -> &[StepId] {
        &self.new_step_ids
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RewriteTargetWire {
    old_step_id: StepId,
    new_step_ids: Vec<StepId>,
}
impl<'de> Deserialize<'de> for RewriteTarget {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let w = RewriteTargetWire::deserialize(d)?;
        Self::new(w.old_step_id, w.new_step_ids).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Initialize,
    CaptureWorktree,
    SwitchLayer,
    MoveLayerHeads,
    CreateMarker,
    HideMarker,
    CreateRelease,
    Weave,
    ResolveConflict,
    RewriteSteps,
    ReceiveRemoteState,
    IntegrateOperations,
    Undo,
    Redo,
    Restore,
    Revert,
    Prune,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    schema_version: &'static str,
    operation_id: OperationId,
    space_id: SpaceId,
    parent_operation_ids: Vec<OperationId>,
    resulting_view_id: RepositoryViewId,
    kind: OperationKind,
    summary: String,
    rewrite_map: Vec<RewriteTarget>,
    actor_id: Option<ActorId>,
    device_id: Option<DeviceId>,
    created_at: Timestamp,
    metadata: Metadata,
}
impl Operation {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        operation_id: OperationId,
        space_id: SpaceId,
        mut parent_operation_ids: Vec<OperationId>,
        resulting_view_id: RepositoryViewId,
        kind: OperationKind,
        summary: impl Into<String>,
        mut rewrite_map: Vec<RewriteTarget>,
        actor_id: Option<ActorId>,
        device_id: Option<DeviceId>,
        created_at: Timestamp,
        metadata: Metadata,
    ) -> Result<Self> {
        let summary = summary.into();
        validate_text("summary", &summary, 1, 4096)?;
        sort_unique(&mut parent_operation_ids, "Operation parent", |v| {
            v.as_str()
        })?;
        if parent_operation_ids.binary_search(&operation_id).is_ok() {
            return Err(ScmError::InvalidField {
                field: "parentOperationIds",
                reason: "Operation cannot parent itself".into(),
            });
        }
        sort_unique(&mut rewrite_map, "rewrite source", |v| {
            v.old_step_id.as_str()
        })?;
        Ok(Self {
            schema_version: "layrs.operation.v1",
            operation_id,
            space_id,
            parent_operation_ids,
            resulting_view_id,
            kind,
            summary,
            rewrite_map,
            actor_id,
            device_id,
            created_at,
            metadata,
        })
    }
    pub fn id(&self) -> &OperationId {
        &self.operation_id
    }
    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }
    pub fn parent_operation_ids(&self) -> &[OperationId] {
        &self.parent_operation_ids
    }
    pub fn resulting_view_id(&self) -> &RepositoryViewId {
        &self.resulting_view_id
    }
    pub fn kind(&self) -> OperationKind {
        self.kind
    }
    pub fn summary(&self) -> &str {
        &self.summary
    }
    pub fn rewrite_map(&self) -> &[RewriteTarget] {
        &self.rewrite_map
    }
    pub fn actor_id(&self) -> Option<&ActorId> {
        self.actor_id.as_ref()
    }
    pub fn device_id(&self) -> Option<&DeviceId> {
        self.device_id.as_ref()
    }
    pub fn created_at(&self) -> &Timestamp {
        &self.created_at
    }
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperationWire {
    schema_version: String,
    operation_id: OperationId,
    space_id: SpaceId,
    parent_operation_ids: Vec<OperationId>,
    resulting_view_id: RepositoryViewId,
    kind: OperationKind,
    summary: String,
    rewrite_map: Vec<RewriteTarget>,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    actor_id: Option<Option<ActorId>>,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    device_id: Option<Option<DeviceId>>,
    created_at: Timestamp,
    metadata: Metadata,
}
impl<'de> Deserialize<'de> for Operation {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let w = OperationWire::deserialize(d)?;
        if w.schema_version != "layrs.operation.v1" {
            return Err(serde::de::Error::custom(
                "unsupported Operation schemaVersion",
            ));
        }
        Self::new(
            w.operation_id,
            w.space_id,
            w.parent_operation_ids,
            w.resulting_view_id,
            w.kind,
            w.summary,
            w.rewrite_map,
            w.actor_id
                .ok_or_else(|| serde::de::Error::missing_field("actorId"))?,
            w.device_id
                .ok_or_else(|| serde::de::Error::missing_field("deviceId"))?,
            w.created_at,
            w.metadata,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    schema_version: &'static str,
    worktree_id: WorktreeId,
    store_id: StoreId,
    space_id: SpaceId,
    active_operation_id: OperationId,
    checkout_root: String,
    filesystem_identity: String,
    created_at: Timestamp,
    metadata: Metadata,
}
impl Worktree {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        worktree_id: WorktreeId,
        store_id: StoreId,
        space_id: SpaceId,
        active_operation_id: OperationId,
        checkout_root: impl Into<String>,
        filesystem_identity: impl Into<String>,
        created_at: Timestamp,
        metadata: Metadata,
    ) -> Result<Self> {
        let checkout_root = checkout_root.into();
        let filesystem_identity = filesystem_identity.into();
        validate_text("checkoutRoot", &checkout_root, 1, 32768)?;
        validate_text("filesystemIdentity", &filesystem_identity, 1, 1024)?;
        Ok(Self {
            schema_version: "layrs.worktree.v1",
            worktree_id,
            store_id,
            space_id,
            active_operation_id,
            checkout_root,
            filesystem_identity,
            created_at,
            metadata,
        })
    }
    pub fn id(&self) -> &WorktreeId {
        &self.worktree_id
    }
    pub fn store_id(&self) -> &StoreId {
        &self.store_id
    }
    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }
    pub fn active_operation_id(&self) -> &OperationId {
        &self.active_operation_id
    }
    pub fn checkout_root(&self) -> &str {
        &self.checkout_root
    }
    pub fn filesystem_identity(&self) -> &str {
        &self.filesystem_identity
    }
    pub fn created_at(&self) -> &Timestamp {
        &self.created_at
    }
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorktreeWire {
    schema_version: String,
    worktree_id: WorktreeId,
    store_id: StoreId,
    space_id: SpaceId,
    active_operation_id: OperationId,
    checkout_root: String,
    filesystem_identity: String,
    created_at: Timestamp,
    metadata: Metadata,
}
impl<'de> Deserialize<'de> for Worktree {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let w = WorktreeWire::deserialize(d)?;
        if w.schema_version != "layrs.worktree.v1" {
            return Err(serde::de::Error::custom(
                "unsupported Worktree schemaVersion",
            ));
        }
        Self::new(
            w.worktree_id,
            w.store_id,
            w.space_id,
            w.active_operation_id,
            w.checkout_root,
            w.filesystem_identity,
            w.created_at,
            w.metadata,
        )
        .map_err(serde::de::Error::custom)
    }
}

mod references;
pub use references::{Marker, MarkerKind, Release, ReleasePublication};
