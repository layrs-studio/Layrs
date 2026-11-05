use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    LENS_CONTRACT_VERSION, LensConflictBlock, LensConflictSegment, LensConflictSegmentKind,
    LensReconcileContent, LensReconcileInput, LensReconcileRequest, LensReconcileResult,
    LensReconcileResultStatus, LensReconcileSide, LensReconcileSideContract, MetadataMap,
    ResolutionMethod, ResolutionScope, ScopedResolutionMethod, resolution_method_labels,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensContractValidationError {
    message: String,
}

impl LensContractValidationError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for LensContractValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for LensContractValidationError {}

pub fn is_sha256_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn validate_sha256_digest(value: &str) -> Result<(), LensContractValidationError> {
    if is_sha256_digest(value) {
        Ok(())
    } else {
        Err(LensContractValidationError::new(
            "digest must match sha256: followed by 64 lowercase hexadecimal characters",
        ))
    }
}

pub(crate) fn serialize_sha256_digest<S>(value: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    validate_sha256_digest(value).map_err(S::Error::custom)?;
    serializer.serialize_str(value)
}

pub(crate) fn deserialize_sha256_digest<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    validate_sha256_digest(&value).map_err(D::Error::custom)?;
    Ok(value)
}

pub(crate) fn serialize_optional_sha256_digest<S>(
    value: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if let Some(digest) = value {
        validate_sha256_digest(digest).map_err(S::Error::custom)?;
    }
    value.serialize(serializer)
}

pub(crate) fn deserialize_optional_sha256_digest<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    if let Some(digest) = value.as_deref() {
        validate_sha256_digest(digest).map_err(D::Error::custom)?;
    }
    Ok(value)
}

impl LensReconcileSideContract {
    pub fn validate(&self) -> Result<(), LensContractValidationError> {
        validate_reconcile_side(
            self.exists,
            &self.bytes,
            self.content_hash.as_deref(),
            self.size,
            "Lens reconcile side",
        )
    }
}

impl LensReconcileSide<'_> {
    pub fn validate(&self) -> Result<(), LensContractValidationError> {
        validate_reconcile_side(
            self.exists,
            self.bytes,
            self.content_hash,
            self.size,
            "Lens reconcile execution side",
        )
    }
}

impl LensReconcileInput<'_> {
    pub fn validate(&self) -> Result<(), LensContractValidationError> {
        self.base.validate()?;
        self.ours.validate()?;
        self.theirs.validate()?;
        if self.ours_label.is_empty() || self.theirs_label.is_empty() {
            return Err(LensContractValidationError::new(
                "Lens reconcile labels must not be empty",
            ));
        }
        Ok(())
    }
}

impl LensReconcileRequest {
    pub fn validate(&self) -> Result<(), LensContractValidationError> {
        if self.contract_version != LENS_CONTRACT_VERSION {
            return Err(LensContractValidationError::new(format!(
                "contractVersion must be {LENS_CONTRACT_VERSION}"
            )));
        }
        self.lens.validate()?;
        if self.media_type.as_ref().is_some_and(String::is_empty) {
            return Err(LensContractValidationError::new(
                "reconcile mediaType must not be empty",
            ));
        }
        self.base.validate()?;
        self.existing.validate()?;
        self.incoming.validate()?;
        if self.labels.existing.is_empty() || self.labels.incoming.is_empty() {
            return Err(LensContractValidationError::new(
                "reconcile labels must not be empty",
            ));
        }
        Ok(())
    }
}

impl LensReconcileContent {
    pub fn validate(&self) -> Result<(), LensContractValidationError> {
        if !self.exists && !self.bytes.is_empty() {
            return Err(LensContractValidationError::new(
                "absent reconcile content forbids non-empty bytes",
            ));
        }
        Ok(())
    }
}

fn validate_reconcile_side(
    exists: bool,
    bytes: &[u8],
    content_hash: Option<&str>,
    size: u64,
    label: &str,
) -> Result<(), LensContractValidationError> {
    if exists {
        if size != bytes.len() as u64 {
            return Err(LensContractValidationError::new(format!(
                "{label} size must equal its byte length"
            )));
        }
        if let Some(content_hash) = content_hash {
            validate_sha256_digest(content_hash)?;
        }
        return Ok(());
    }
    if !bytes.is_empty() {
        return Err(LensContractValidationError::new(format!(
            "{label} absent content forbids non-empty bytes"
        )));
    }
    if size != 0 {
        return Err(LensContractValidationError::new(format!(
            "{label} absent content requires size 0"
        )));
    }
    if content_hash.is_some() {
        return Err(LensContractValidationError::new(format!(
            "{label} absent content forbids contentHash"
        )));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UncheckedLensReconcileSideContract {
    exists: bool,
    bytes: Vec<u8>,
    #[serde(default)]
    content_hash: Option<String>,
    size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LensReconcileSideContractRef<'a> {
    exists: bool,
    bytes: &'a [u8],
    #[serde(skip_serializing_if = "Option::is_none")]
    content_hash: Option<&'a str>,
    size: u64,
}

impl<'de> Deserialize<'de> for LensReconcileSideContract {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let unchecked = UncheckedLensReconcileSideContract::deserialize(deserializer)?;
        let side = Self {
            exists: unchecked.exists,
            bytes: unchecked.bytes,
            content_hash: unchecked.content_hash,
            size: unchecked.size,
        };
        side.validate().map_err(D::Error::custom)?;
        Ok(side)
    }
}

impl Serialize for LensReconcileSideContract {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate().map_err(S::Error::custom)?;
        LensReconcileSideContractRef {
            exists: self.exists,
            bytes: &self.bytes,
            content_hash: self.content_hash.as_deref(),
            size: self.size,
        }
        .serialize(serializer)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UncheckedLensReconcileContent {
    exists: bool,
    bytes: Vec<u8>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LensReconcileContentRef<'a> {
    exists: bool,
    bytes: &'a [u8],
}

impl<'de> Deserialize<'de> for LensReconcileContent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let unchecked = UncheckedLensReconcileContent::deserialize(deserializer)?;
        let content = Self {
            exists: unchecked.exists,
            bytes: unchecked.bytes,
        };
        content.validate().map_err(D::Error::custom)?;
        Ok(content)
    }
}

impl Serialize for LensReconcileContent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate().map_err(S::Error::custom)?;
        LensReconcileContentRef {
            exists: self.exists,
            bytes: &self.bytes,
        }
        .serialize(serializer)
    }
}

impl LensReconcileResult {
    pub fn auto_resolved(summary: impl Into<String>, content: LensReconcileContent) -> Self {
        Self {
            status: LensReconcileResultStatus::AutoResolved,
            summary: summary.into(),
            resolved: Some(content),
            conflict: None,
            file_methods: Vec::new(),
            blocks: Vec::new(),
            segments: Vec::new(),
            fields: MetadataMap::new(),
        }
    }

    pub fn conflicted(
        summary: impl Into<String>,
        content: LensReconcileContent,
        blocks: Vec<LensConflictBlock>,
        segments: Vec<LensConflictSegment>,
    ) -> Self {
        Self::try_conflicted(summary, content, blocks, segments)
            .expect("conflicted Lens result must contain a coherent conflict graph")
    }

    pub fn try_conflicted(
        summary: impl Into<String>,
        content: LensReconcileContent,
        blocks: Vec<LensConflictBlock>,
        segments: Vec<LensConflictSegment>,
    ) -> Result<Self, LensContractValidationError> {
        let result = Self {
            status: LensReconcileResultStatus::Conflicted,
            summary: summary.into(),
            resolved: None,
            conflict: Some(content),
            file_methods: Vec::new(),
            blocks,
            segments,
            fields: MetadataMap::new(),
        };
        result.validate()?;
        Ok(result)
    }

    pub fn unsupported(summary: impl Into<String>) -> Self {
        Self {
            status: LensReconcileResultStatus::Unsupported,
            summary: summary.into(),
            resolved: None,
            conflict: None,
            file_methods: Vec::new(),
            blocks: Vec::new(),
            segments: Vec::new(),
            fields: MetadataMap::new(),
        }
    }

    pub fn with_file_methods(self, methods: Vec<ResolutionMethod>) -> Self {
        self.try_with_file_methods(methods)
            .expect("file resolution methods require a valid conflicted Lens result")
    }

    pub fn try_with_file_methods(
        mut self,
        methods: Vec<ResolutionMethod>,
    ) -> Result<Self, LensContractValidationError> {
        self.file_methods = methods;
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), LensContractValidationError> {
        if let Some(resolved) = &self.resolved {
            resolved.validate()?;
        }
        if let Some(conflict) = &self.conflict {
            conflict.validate()?;
        }
        validate_unique_methods(&self.file_methods, "file resolution methods")?;
        for block in &self.blocks {
            if block.block_id.is_empty() {
                return Err(LensContractValidationError::new(
                    "conflict block id must not be empty",
                ));
            }
            validate_unique_methods(&block.methods, &format!("block {} methods", block.block_id))?;
        }

        match self.status {
            LensReconcileResultStatus::AutoResolved => {
                if self.resolved.is_none() {
                    return Err(LensContractValidationError::new(
                        "auto_resolved result requires resolved content",
                    ));
                }
                if self.conflict.is_some() {
                    return Err(LensContractValidationError::new(
                        "auto_resolved result forbids conflict content",
                    ));
                }
                self.validate_no_conflict_artifacts("auto_resolved")
            }
            LensReconcileResultStatus::Conflicted => {
                if self.conflict.is_none() {
                    return Err(LensContractValidationError::new(
                        "conflicted result requires conflict content",
                    ));
                }
                if self.resolved.is_some() {
                    return Err(LensContractValidationError::new(
                        "conflicted result forbids resolved content",
                    ));
                }
                self.validate_conflict_graph()
            }
            LensReconcileResultStatus::Unsupported => {
                if self.resolved.is_some() || self.conflict.is_some() {
                    return Err(LensContractValidationError::new(
                        "unsupported result forbids resolved and conflict content",
                    ));
                }
                self.validate_no_conflict_artifacts("unsupported")
            }
        }
    }

    fn validate_no_conflict_artifacts(
        &self,
        status: &str,
    ) -> Result<(), LensContractValidationError> {
        if self.file_methods.is_empty() && self.blocks.is_empty() && self.segments.is_empty() {
            Ok(())
        } else {
            Err(LensContractValidationError::new(format!(
                "{status} result forbids resolution methods, conflict blocks, and segments"
            )))
        }
    }

    fn validate_conflict_graph(&self) -> Result<(), LensContractValidationError> {
        let mut block_ids = BTreeSet::new();
        for block in &self.blocks {
            if !block_ids.insert(block.block_id.as_str()) {
                return Err(LensContractValidationError::new(format!(
                    "duplicate conflict block {}",
                    block.block_id
                )));
            }
        }
        if self.blocks.is_empty() && !self.segments.is_empty() {
            return Err(LensContractValidationError::new(
                "conflict segments require conflict blocks",
            ));
        }

        let mut references = BTreeMap::<&str, usize>::new();
        for (index, segment) in self.segments.iter().enumerate() {
            match segment.kind {
                LensConflictSegmentKind::Text => {
                    if segment.text.is_none() || segment.block_id.is_some() {
                        return Err(LensContractValidationError::new(format!(
                            "text conflict segment {index} requires text and forbids blockId"
                        )));
                    }
                }
                LensConflictSegmentKind::Block => {
                    let block_id = segment
                        .block_id
                        .as_deref()
                        .filter(|value| !value.is_empty());
                    if block_id.is_none() || segment.text.is_some() {
                        return Err(LensContractValidationError::new(format!(
                            "block conflict segment {index} requires blockId and forbids text"
                        )));
                    }
                    let block_id = block_id.expect("block id checked above");
                    if !block_ids.contains(block_id) {
                        return Err(LensContractValidationError::new(format!(
                            "conflict segment references unknown block {block_id}"
                        )));
                    }
                    *references.entry(block_id).or_default() += 1;
                }
            }
        }
        for block_id in block_ids {
            if references.get(block_id).copied() != Some(1) {
                return Err(LensContractValidationError::new(format!(
                    "conflict block {block_id} must be referenced by exactly one segment"
                )));
            }
        }
        Ok(())
    }
}

fn validate_unique_methods(
    methods: &[ResolutionMethod],
    label: &str,
) -> Result<(), LensContractValidationError> {
    let mut seen = BTreeSet::new();
    for method in methods {
        if !seen.insert(*method) {
            return Err(LensContractValidationError::new(format!(
                "{label} contains duplicate method {method}"
            )));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UncheckedLensReconcileResult {
    status: LensReconcileResultStatus,
    summary: String,
    #[serde(default)]
    resolved: Option<LensReconcileContent>,
    #[serde(default)]
    conflict: Option<LensReconcileContent>,
    resolution_methods: Vec<ScopedResolutionMethod>,
    blocks: Vec<LensConflictBlock>,
    segments: Vec<LensConflictSegment>,
    fields: MetadataMap,
}

impl<'de> Deserialize<'de> for LensReconcileResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let unchecked = UncheckedLensReconcileResult::deserialize(deserializer)?;
        if let Some(method) = unchecked
            .resolution_methods
            .iter()
            .find(|method| method.scope != ResolutionScope::File)
        {
            return Err(D::Error::custom(format!(
                "expected file resolution method, found {}",
                method.scope.as_str()
            )));
        }
        let mut blocks = unchecked.blocks;
        for block in &mut blocks {
            block.supported_resolutions = resolution_method_labels(&block.methods);
        }
        let result = Self {
            status: unchecked.status,
            summary: unchecked.summary,
            resolved: unchecked.resolved,
            conflict: unchecked.conflict,
            file_methods: unchecked
                .resolution_methods
                .into_iter()
                .map(|method| method.method)
                .collect(),
            blocks,
            segments: unchecked.segments,
            fields: unchecked.fields,
        };
        result.validate().map_err(D::Error::custom)?;
        Ok(result)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LensReconcileResultRef<'a> {
    status: LensReconcileResultStatus,
    summary: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved: Option<&'a LensReconcileContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conflict: Option<&'a LensReconcileContent>,
    resolution_methods: Vec<ScopedResolutionMethod>,
    blocks: &'a [LensConflictBlock],
    segments: &'a [LensConflictSegment],
    fields: &'a MetadataMap,
}

impl Serialize for LensReconcileResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate().map_err(S::Error::custom)?;
        LensReconcileResultRef {
            status: self.status,
            summary: &self.summary,
            resolved: self.resolved.as_ref(),
            conflict: self.conflict.as_ref(),
            resolution_methods: self
                .file_methods
                .iter()
                .copied()
                .map(|method| ScopedResolutionMethod::new(method, ResolutionScope::File))
                .collect(),
            blocks: &self.blocks,
            segments: &self.segments,
            fields: &self.fields,
        }
        .serialize(serializer)
    }
}
