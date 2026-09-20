use super::{V3StoreError, V3StoreResult};
use layrs_core::scm::{ConflictId, ObjectId, RepoPath, RepoPathComponent, StepId};
use layrs_sync::v3::ObjectDescriptor;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeSet;

mod references;
pub use references::conflict_object_references;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
struct Sha256Id(String);

impl Sha256Id {
    fn validate(&self, field: &str) -> Result<(), String> {
        let digest = self
            .0
            .strip_prefix("sha256:")
            .ok_or_else(|| format!("{field} must use the sha256: prefix"))?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(format!(
                "{field} must contain exactly 64 lowercase hexadecimal digits"
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ConflictKind {
    Content,
    DeleteModify,
    Rename,
    Type,
    Mode,
    DirectoryFile,
    Binary,
    Reference,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PlacementRole {
    Primary,
    Source,
    Destination,
    Affected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Placement {
    placement_id: ObjectId,
    path: RepoPath,
    role: PlacementRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum BaseProvenance {
    Step {
        step_id: StepId,
    },
    Virtual {
        source_step_ids: Vec<StepId>,
        algorithm: AlgorithmVersion,
        result: VirtualBaseResult,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AlgorithmVersion {
    id: String,
    version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VirtualBaseResultKind {
    Tree,
    Object,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VirtualBaseResult {
    kind: VirtualBaseResultKind,
    object_id: ObjectId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StepProvenance {
    kind: StepProvenanceKind,
    step_id: StepId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StepProvenanceKind {
    Step,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BaseTermKind {
    Base,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SideTermKind {
    Side,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BaseTerm {
    term_id: ObjectId,
    term_kind: BaseTermKind,
    provenance: BaseProvenance,
    values: Vec<PlacedValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SideTerm {
    term_id: ObjectId,
    term_kind: SideTermKind,
    provenance: StepProvenance,
    values: Vec<PlacedValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlacedValue {
    placement_id: ObjectId,
    value: ConflictValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
struct NullableExecutable(Option<bool>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ConflictValue {
    Absent,
    File {
        object_id: ObjectId,
        executable: NullableExecutable,
    },
    Tree {
        object_id: ObjectId,
        executable: NullableExecutable,
    },
    Symlink {
        object_id: ObjectId,
        executable: NullableExecutable,
    },
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ConflictValueWire {
    Absent,
    File {
        object_id: ObjectId,
        #[serde(default, deserialize_with = "deserialize_present_nullable")]
        executable: Option<Option<bool>>,
    },
    Tree {
        object_id: ObjectId,
        #[serde(default, deserialize_with = "deserialize_present_nullable")]
        executable: Option<Option<bool>>,
    },
    Symlink {
        object_id: ObjectId,
        #[serde(default, deserialize_with = "deserialize_present_nullable")]
        executable: Option<Option<bool>>,
    },
}

impl<'de> Deserialize<'de> for ConflictValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ConflictValueWire::deserialize(deserializer)?;
        let required = |value: Option<Option<bool>>| {
            value
                .map(NullableExecutable)
                .ok_or_else(|| serde::de::Error::missing_field("executable"))
        };
        match wire {
            ConflictValueWire::Absent => Ok(Self::Absent),
            ConflictValueWire::File {
                object_id,
                executable,
            } => Ok(Self::File {
                object_id,
                executable: required(executable)?,
            }),
            ConflictValueWire::Tree {
                object_id,
                executable,
            } => Ok(Self::Tree {
                object_id,
                executable: required(executable)?,
            }),
            ConflictValueWire::Symlink {
                object_id,
                executable,
            } => Ok(Self::Symlink {
                object_id,
                executable: required(executable)?,
            }),
        }
    }
}

fn deserialize_present_nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

fn deserialize_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ResolutionMethod {
    SelectTerm,
    CombineTerms,
    Manual,
    Rename,
}

impl ResolutionMethod {
    const fn as_str(self) -> &'static str {
        match self {
            Self::SelectTerm => "select_term",
            Self::CombineTerms => "combine_terms",
            Self::Manual => "manual",
            Self::Rename => "rename",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Resolution {
    method: ResolutionMethod,
    source_term_ids: Vec<ObjectId>,
    result: ConflictValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlacementResolution {
    placement_id: ObjectId,
    resolution: Resolution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BlockTermValue {
    term_id: ObjectId,
    value: ConflictValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TermSpan {
    term_id: ObjectId,
    start_byte: u64,
    end_byte: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum OccurrenceAnchor {
    Text {
        placement_id: ObjectId,
        spans: Vec<TermSpan>,
    },
    Structural {
        placement_id: ObjectId,
        node_path: Vec<RepoPathComponent>,
        term_ids: Vec<ObjectId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConflictBlock {
    block_id: ObjectId,
    base_values: Vec<BlockTermValue>,
    side_values: Vec<BlockTermValue>,
    supported_methods: Vec<ResolutionMethod>,
    occurrence_anchor: OccurrenceAnchor,
    #[serde(deserialize_with = "deserialize_nullable")]
    resolution: Option<Resolution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Segment {
    Content { object_id: ObjectId },
    Block { block_id: ObjectId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LensPin {
    lens_id: String,
    version: String,
    digest: Sha256Id,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ResolutionState {
    Unresolved,
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConflictObject {
    schema_version: String,
    conflict_id: ConflictId,
    kind: ConflictKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    custom_kind: Option<String>,
    placements: Vec<Placement>,
    bases: Vec<BaseTerm>,
    sides: Vec<SideTerm>,
    #[serde(deserialize_with = "deserialize_nullable")]
    lens_pin: Option<LensPin>,
    #[serde(deserialize_with = "deserialize_nullable")]
    lens_implementation_digest: Option<Sha256Id>,
    #[serde(deserialize_with = "deserialize_nullable")]
    rendered_object_id: Option<ObjectId>,
    resolution_state: ResolutionState,
    placement_resolutions: Vec<PlacementResolution>,
    blocks: Vec<ConflictBlock>,
    segments: Vec<Segment>,
    origin_step_ids: Vec<StepId>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConflictIdentity<'a> {
    schema_version: &'a str,
    kind: ConflictKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    custom_kind: &'a Option<String>,
    placements: &'a [Placement],
    bases: &'a [BaseTerm],
    sides: &'a [SideTerm],
    lens_pin: &'a Option<LensPin>,
    lens_implementation_digest: &'a Option<Sha256Id>,
    rendered_object_id: &'a Option<ObjectId>,
    resolution_state: ResolutionState,
    placement_resolutions: &'a [PlacementResolution],
    blocks: &'a [ConflictBlock],
    segments: &'a [Segment],
    origin_step_ids: &'a [StepId],
}

impl ConflictObject {
    fn identity(&self) -> ConflictIdentity<'_> {
        ConflictIdentity {
            schema_version: &self.schema_version,
            kind: self.kind,
            custom_kind: &self.custom_kind,
            placements: &self.placements,
            bases: &self.bases,
            sides: &self.sides,
            lens_pin: &self.lens_pin,
            lens_implementation_digest: &self.lens_implementation_digest,
            rendered_object_id: &self.rendered_object_id,
            resolution_state: self.resolution_state,
            placement_resolutions: &self.placement_resolutions,
            blocks: &self.blocks,
            segments: &self.segments,
            origin_step_ids: &self.origin_step_ids,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.schema_version != "layrs.scm.conflict.v1" {
            return Err("unsupported ConflictObject schemaVersion".into());
        }
        match (self.kind, self.custom_kind.as_deref()) {
            (ConflictKind::Custom, Some(value)) => identifier(value, 128, "customKind")?,
            (ConflictKind::Custom, None) => {
                return Err("custom conflict requires customKind".into());
            }
            (_, Some(_)) => return Err("customKind is forbidden for non-custom conflict".into()),
            (_, None) => {}
        }

        bounded(&self.placements, 1, 4_096, "placements")?;
        sorted_by(
            &self.placements,
            |value| value.placement_id.as_str(),
            "placements",
        )?;
        let placement_ids = self
            .placements
            .iter()
            .map(|value| value.placement_id.as_str())
            .collect::<BTreeSet<_>>();

        bounded(&self.bases, 0, 64, "bases")?;
        sorted_by(&self.bases, |value| value.term_id.as_str(), "bases")?;
        bounded(&self.sides, 2, 64, "sides")?;
        sorted_by(&self.sides, |value| value.term_id.as_str(), "sides")?;

        let base_ids = self
            .bases
            .iter()
            .map(|value| value.term_id.as_str())
            .collect::<BTreeSet<_>>();
        let side_ids = self
            .sides
            .iter()
            .map(|value| value.term_id.as_str())
            .collect::<BTreeSet<_>>();
        if !base_ids.is_disjoint(&side_ids) {
            return Err("a termId occurs in both bases and sides".into());
        }
        let term_ids = base_ids.union(&side_ids).copied().collect::<BTreeSet<_>>();

        for base in &self.bases {
            validate_values(&base.values, &placement_ids, "bases.values")?;
            if let BaseProvenance::Virtual {
                source_step_ids,
                algorithm,
                ..
            } = &base.provenance
            {
                bounded(source_step_ids, 2, 128, "bases.provenance.sourceStepIds")?;
                sorted_by(
                    source_step_ids,
                    StepId::as_str,
                    "bases.provenance.sourceStepIds",
                )?;
                identifier(&algorithm.id, 128, "bases.provenance.algorithm.id")?;
                string_len(
                    &algorithm.version,
                    1,
                    64,
                    "bases.provenance.algorithm.version",
                )?;
            }
        }
        for side in &self.sides {
            validate_values(&side.values, &placement_ids, "sides.values")?;
        }

        match (&self.lens_pin, &self.lens_implementation_digest) {
            (None, None) => {}
            (Some(pin), Some(digest)) => {
                identifier(&pin.lens_id, 128, "lensPin.lensId")?;
                if !valid_semver(&pin.version) {
                    return Err("lensPin.version is not a semantic version".into());
                }
                pin.digest.validate("lensPin.digest")?;
                digest.validate("lensImplementationDigest")?;
            }
            _ => {
                return Err(
                    "lensPin and lensImplementationDigest must both be null or present".into(),
                );
            }
        }

        bounded(
            &self.placement_resolutions,
            0,
            4_096,
            "placementResolutions",
        )?;
        sorted_by(
            &self.placement_resolutions,
            |value| value.placement_id.as_str(),
            "placementResolutions",
        )?;
        for resolution in &self.placement_resolutions {
            if !placement_ids.contains(resolution.placement_id.as_str()) {
                return Err("placement resolution references an unknown placementId".into());
            }
            validate_resolution(&resolution.resolution, &term_ids, "placementResolutions")?;
        }

        bounded(&self.blocks, 0, 100_000, "blocks")?;
        sorted_by(&self.blocks, |value| value.block_id.as_str(), "blocks")?;
        let block_ids = self
            .blocks
            .iter()
            .map(|value| value.block_id.as_str())
            .collect::<BTreeSet<_>>();
        let mut resolved_blocks = 0_usize;
        for block in &self.blocks {
            bounded(&block.base_values, 0, 64, "blocks.baseValues")?;
            sorted_by(
                &block.base_values,
                |value| value.term_id.as_str(),
                "blocks.baseValues",
            )?;
            validate_block_values(&block.base_values, &base_ids, "blocks.baseValues")?;
            bounded(&block.side_values, 2, 64, "blocks.sideValues")?;
            sorted_by(
                &block.side_values,
                |value| value.term_id.as_str(),
                "blocks.sideValues",
            )?;
            validate_block_values(&block.side_values, &side_ids, "blocks.sideValues")?;
            bounded(&block.supported_methods, 1, 4, "blocks.supportedMethods")?;
            sorted_by(
                &block.supported_methods,
                |method| method.as_str(),
                "blocks.supportedMethods",
            )?;
            validate_anchor(&block.occurrence_anchor, &placement_ids, &term_ids)?;
            if let Some(resolution) = &block.resolution {
                resolved_blocks += 1;
                if !block.supported_methods.contains(&resolution.method) {
                    return Err("block resolution method is not supported".into());
                }
                validate_resolution(resolution, &term_ids, "blocks.resolution")?;
            }
        }

        bounded(&self.segments, 0, 200_000, "segments")?;
        for segment in &self.segments {
            if let Segment::Block { block_id } = segment {
                if !block_ids.contains(block_id.as_str()) {
                    return Err("segment references an unknown blockId".into());
                }
            }
        }

        bounded(&self.origin_step_ids, 1, 128, "originStepIds")?;
        sorted_by(&self.origin_step_ids, StepId::as_str, "originStepIds")?;
        let origins = self
            .origin_step_ids
            .iter()
            .map(StepId::as_str)
            .collect::<BTreeSet<_>>();
        for side in &self.sides {
            if !origins.contains(side.provenance.step_id.as_str()) {
                return Err("side provenance stepId is absent from originStepIds".into());
            }
        }
        for base in &self.bases {
            match &base.provenance {
                BaseProvenance::Step { step_id } => {
                    if !origins.contains(step_id.as_str()) {
                        return Err("base provenance stepId is absent from originStepIds".into());
                    }
                }
                BaseProvenance::Virtual {
                    source_step_ids, ..
                } => {
                    if source_step_ids
                        .iter()
                        .any(|step_id| !origins.contains(step_id.as_str()))
                    {
                        return Err("virtual base sourceStepId is absent from originStepIds".into());
                    }
                }
            }
        }

        let has_resolution = !self.placement_resolutions.is_empty() || resolved_blocks > 0;
        match (self.resolution_state, has_resolution) {
            (ResolutionState::Unresolved, true) => {
                return Err("unresolved ConflictObject contains a resolution".into());
            }
            (ResolutionState::Partial, false) => {
                return Err("partial ConflictObject contains no resolution".into());
            }
            _ => {}
        }
        Ok(())
    }
}

pub(crate) fn verify_conflict_object(
    canonical: &[u8],
    descriptor: &ObjectDescriptor,
) -> V3StoreResult<()> {
    let value: ConflictObject = serde_json::from_slice(canonical)
        .map_err(|error| V3StoreError::CorruptObject(error.to_string()))?;
    let encoded = serde_jcs::to_vec(&value)
        .map_err(|error| V3StoreError::CorruptObject(error.to_string()))?;
    if encoded != canonical {
        return Err(V3StoreError::CorruptObject(
            "ConflictObject is not exact JCS".into(),
        ));
    }
    value.validate().map_err(V3StoreError::CorruptObject)?;
    if value.conflict_id.as_str() != descriptor.object_id.as_str() {
        return Err(V3StoreError::IdentityMismatch);
    }
    let payload = serde_jcs::to_vec(&value.identity())
        .map_err(|error| V3StoreError::CorruptObject(error.to_string()))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"layrs-conflict-v1\0");
    hasher.update(&payload);
    if format!("blake3:{}", hasher.finalize().to_hex()) != descriptor.object_id.as_str() {
        return Err(V3StoreError::IdentityMismatch);
    }
    Ok(())
}

fn validate_values(
    values: &[PlacedValue],
    placements: &BTreeSet<&str>,
    field: &str,
) -> Result<(), String> {
    bounded(values, 1, 4_096, field)?;
    sorted_by(values, |value| value.placement_id.as_str(), field)?;
    let actual = values
        .iter()
        .map(|value| value.placement_id.as_str())
        .collect::<BTreeSet<_>>();
    if actual != *placements {
        return Err(format!(
            "{field} must contain exactly one value per placement"
        ));
    }
    for value in values {
        validate_conflict_value(&value.value, field)?;
    }
    Ok(())
}

fn validate_block_values(
    values: &[BlockTermValue],
    allowed_terms: &BTreeSet<&str>,
    field: &str,
) -> Result<(), String> {
    for value in values {
        if !allowed_terms.contains(value.term_id.as_str()) {
            return Err(format!("{field} references a term of the wrong kind"));
        }
        validate_conflict_value(&value.value, field)?;
    }
    Ok(())
}

fn validate_conflict_value(value: &ConflictValue, field: &str) -> Result<(), String> {
    match value {
        ConflictValue::Absent => Ok(()),
        ConflictValue::File { executable, .. } if executable.0.is_some() => Ok(()),
        ConflictValue::Tree { executable, .. } | ConflictValue::Symlink { executable, .. }
            if executable.0.is_none() =>
        {
            Ok(())
        }
        ConflictValue::File { .. } => Err(format!("{field}: file executable must be boolean")),
        ConflictValue::Tree { .. } | ConflictValue::Symlink { .. } => {
            Err(format!("{field}: tree/symlink executable must be null"))
        }
    }
}

fn validate_resolution(
    resolution: &Resolution,
    terms: &BTreeSet<&str>,
    field: &str,
) -> Result<(), String> {
    bounded(&resolution.source_term_ids, 1, 64, field)?;
    sorted_by(
        &resolution.source_term_ids,
        ObjectId::as_str,
        "resolution.sourceTermIds",
    )?;
    if resolution
        .source_term_ids
        .iter()
        .any(|term| !terms.contains(term.as_str()))
    {
        return Err(format!("{field} references an unknown sourceTermId"));
    }
    if resolution.method == ResolutionMethod::SelectTerm && resolution.source_term_ids.len() != 1 {
        return Err(format!(
            "{field}: select_term requires exactly one sourceTermId"
        ));
    }
    validate_conflict_value(&resolution.result, field)
}

fn validate_anchor(
    anchor: &OccurrenceAnchor,
    placements: &BTreeSet<&str>,
    terms: &BTreeSet<&str>,
) -> Result<(), String> {
    let placement_id = match anchor {
        OccurrenceAnchor::Text {
            placement_id,
            spans,
        } => {
            bounded(spans, 2, 64, "blocks.occurrenceAnchor.spans")?;
            sorted_by(
                spans,
                |span| span.term_id.as_str(),
                "blocks.occurrenceAnchor.spans",
            )?;
            for span in spans {
                if !terms.contains(span.term_id.as_str()) {
                    return Err("text anchor references an unknown termId".into());
                }
                if span.start_byte > span.end_byte {
                    return Err("text anchor requires startByte <= endByte".into());
                }
            }
            placement_id
        }
        OccurrenceAnchor::Structural {
            placement_id,
            node_path,
            term_ids,
        } => {
            bounded(node_path, 0, 4_096, "blocks.occurrenceAnchor.nodePath")?;
            bounded(term_ids, 2, 64, "blocks.occurrenceAnchor.termIds")?;
            sorted_by(
                term_ids,
                ObjectId::as_str,
                "blocks.occurrenceAnchor.termIds",
            )?;
            if term_ids.iter().any(|term| !terms.contains(term.as_str())) {
                return Err("structural anchor references an unknown termId".into());
            }
            placement_id
        }
    };
    if !placements.contains(placement_id.as_str()) {
        return Err("occurrence anchor references an unknown placementId".into());
    }
    Ok(())
}

fn bounded<T>(items: &[T], min: usize, max: usize, field: &str) -> Result<(), String> {
    if (min..=max).contains(&items.len()) {
        Ok(())
    } else {
        Err(format!("{field} requires {min}..={max} items"))
    }
}

fn sorted_by<T>(items: &[T], key: impl Fn(&T) -> &str, field: &str) -> Result<(), String> {
    if items.windows(2).any(|pair| key(&pair[0]) >= key(&pair[1])) {
        Err(format!(
            "{field} must be strictly ASCII-sorted and duplicate-free"
        ))
    } else {
        Ok(())
    }
}

fn string_len(value: &str, min: usize, max: usize, field: &str) -> Result<(), String> {
    let len = value.chars().count();
    if (min..=max).contains(&len) {
        Ok(())
    } else {
        Err(format!("{field} length must be {min}..={max}"))
    }
}

fn identifier(value: &str, max: usize, field: &str) -> Result<(), String> {
    string_len(value, 1, max, field)?;
    let mut bytes = value.bytes();
    let first = bytes.next().ok_or_else(|| format!("{field} is empty"))?;
    if !(first.is_ascii_lowercase() || first.is_ascii_digit())
        || !bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        return Err(format!("{field} has an invalid identifier syntax"));
    }
    Ok(())
}

fn valid_semver(value: &str) -> bool {
    let (without_build, build) = value
        .split_once('+')
        .map_or((value, None), |(left, right)| (left, Some(right)));
    if build.is_some_and(|part| part.is_empty() || !semver_suffix(part))
        || without_build.matches('+').count() > 0
    {
        return false;
    }
    let (core, prerelease) = without_build
        .split_once('-')
        .map_or((without_build, None), |(left, right)| (left, Some(right)));
    if prerelease.is_some_and(|part| part.is_empty() || !semver_suffix(part)) {
        return false;
    }
    let parts = core.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (*part == "0" || !part.starts_with('0'))
        })
}

fn semver_suffix(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

#[cfg(test)]
mod tests;
