use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::Path;

use crate::{
    LensReconcileInput, LensReconcileResult, LensReconcileSide, ResolutionMethod, ResolutionScope,
};

pub const LENS_CONTRACT_VERSION: &str = "layrs.lens.v1";
pub const LENS_BUNDLE_FORMAT: &str = "layrslens.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensPin {
    pub lens_id: String,
    pub version: String,
    #[serde(
        serialize_with = "crate::validation::serialize_sha256_digest",
        deserialize_with = "crate::validation::deserialize_sha256_digest"
    )]
    pub digest: String,
}

impl LensPin {
    pub fn new(
        lens_id: impl Into<String>,
        version: impl Into<String>,
        digest: impl Into<String>,
    ) -> Self {
        Self::try_new(lens_id, version, digest)
            .expect("Lens pin digest must be a valid sha256 digest")
    }

    pub fn try_new(
        lens_id: impl Into<String>,
        version: impl Into<String>,
        digest: impl Into<String>,
    ) -> Result<Self, crate::LensContractValidationError> {
        let digest = digest.into();
        crate::validate_sha256_digest(&digest)?;
        Ok(Self {
            lens_id: lens_id.into(),
            version: version.into(),
            digest,
        })
    }

    pub fn validate(&self) -> Result<(), crate::LensContractValidationError> {
        crate::validate_sha256_digest(&self.digest)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedResolutionMethod {
    pub method: ResolutionMethod,
    pub scope: ResolutionScope,
}

impl ScopedResolutionMethod {
    pub const fn new(method: ResolutionMethod, scope: ResolutionScope) -> Self {
        Self { method, scope }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensResourceQuotas {
    pub max_memory_bytes: u64,
    pub max_execution_ms: u64,
    pub max_input_bytes: u64,
    pub max_output_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fuel: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WasiEntrypointRuntime {
    Wasi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EsmEntrypointRuntime {
    Esm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WasiLensEntrypoint {
    pub runtime: WasiEntrypointRuntime,
    pub path: String,
    pub export: String,
    #[serde(
        serialize_with = "crate::validation::serialize_sha256_digest",
        deserialize_with = "crate::validation::deserialize_sha256_digest"
    )]
    pub digest: String,
    pub resources: LensResourceQuotas,
}

impl WasiLensEntrypoint {
    pub fn new(
        path: impl Into<String>,
        export: impl Into<String>,
        digest: impl Into<String>,
        resources: LensResourceQuotas,
    ) -> Self {
        let digest = digest.into();
        crate::validate_sha256_digest(&digest)
            .expect("WASI Lens entrypoint digest must be a valid sha256 digest");
        Self {
            runtime: WasiEntrypointRuntime::Wasi,
            path: path.into(),
            export: export.into(),
            digest,
            resources,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EsmLensEntrypoint {
    pub runtime: EsmEntrypointRuntime,
    pub path: String,
    pub export: String,
    #[serde(
        serialize_with = "crate::validation::serialize_sha256_digest",
        deserialize_with = "crate::validation::deserialize_sha256_digest"
    )]
    pub digest: String,
    pub resources: LensResourceQuotas,
}

impl EsmLensEntrypoint {
    pub fn new(
        path: impl Into<String>,
        export: impl Into<String>,
        digest: impl Into<String>,
        resources: LensResourceQuotas,
    ) -> Self {
        let digest = digest.into();
        crate::validate_sha256_digest(&digest)
            .expect("ESM Lens entrypoint digest must be a valid sha256 digest");
        Self {
            runtime: EsmEntrypointRuntime::Esm,
            path: path.into(),
            export: export.into(),
            digest,
            resources,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensBundleEntrypoints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analyzer: Option<WasiLensEntrypoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconcile: Option<WasiLensEntrypoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer: Option<EsmLensEntrypoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensSignatureMetadata {
    pub algorithm: String,
    pub key_id: String,
    #[serde(
        serialize_with = "crate::validation::serialize_sha256_digest",
        deserialize_with = "crate::validation::deserialize_sha256_digest"
    )]
    pub signed_digest: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensBundleContract {
    pub format: String,
    pub entrypoints: LensBundleEntrypoints,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<LensSignatureMetadata>,
}

impl LensBundleContract {
    pub fn new(entrypoints: LensBundleEntrypoints) -> Self {
        Self {
            format: LENS_BUNDLE_FORMAT.to_string(),
            entrypoints,
            signature: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensWindowRange {
    pub start: u64,
    pub end_exclusive: u64,
    pub total: u64,
}

impl LensWindowRange {
    pub const fn new(start: u64, end_exclusive: u64, total: u64) -> Self {
        Self {
            start,
            end_exclusive,
            total,
        }
    }

    pub const fn is_valid(self) -> bool {
        self.start <= self.end_exclusive && self.end_exclusive <= self.total
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LensPreviewWindowUnit {
    Bytes,
    Lines,
    Items,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensPreviewWindow {
    pub contract_version: String,
    pub lens: LensPin,
    pub artifact_id: String,
    pub unit: LensPreviewWindowUnit,
    pub window: LensWindowRange,
}

impl LensPreviewWindow {
    pub fn new(
        lens: LensPin,
        artifact_id: impl Into<String>,
        unit: LensPreviewWindowUnit,
        window: LensWindowRange,
    ) -> Self {
        Self {
            contract_version: LENS_CONTRACT_VERSION.to_string(),
            lens,
            artifact_id: artifact_id.into(),
            unit,
            window,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensDiffWindow {
    pub contract_version: String,
    pub lens: LensPin,
    pub path: String,
    pub lines: LensWindowRange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<LensWindowRange>,
}

impl LensDiffWindow {
    pub fn new(lens: LensPin, path: impl Into<String>, lines: LensWindowRange) -> Self {
        Self {
            contract_version: LENS_CONTRACT_VERSION.to_string(),
            lens,
            path: path.into(),
            lines,
            columns: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensReconcileSideContract {
    pub exists: bool,
    pub bytes: Vec<u8>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::validation::serialize_optional_sha256_digest",
        deserialize_with = "crate::validation::deserialize_optional_sha256_digest"
    )]
    pub content_hash: Option<String>,
    pub size: u64,
}

impl LensReconcileSideContract {
    pub fn present(bytes: impl Into<Vec<u8>>, content_hash: Option<String>) -> Self {
        let bytes = bytes.into();
        if let Some(digest) = content_hash.as_deref() {
            crate::validate_sha256_digest(digest)
                .expect("reconcile content hash must be a valid sha256 digest");
        }
        Self {
            exists: true,
            size: bytes.len() as u64,
            bytes,
            content_hash,
        }
    }

    pub fn absent() -> Self {
        Self {
            exists: false,
            bytes: Vec::new(),
            content_hash: None,
            size: 0,
        }
    }

    fn as_execution_side(&self) -> LensReconcileSide<'_> {
        LensReconcileSide {
            exists: self.exists,
            bytes: &self.bytes,
            content_hash: self.content_hash.as_deref(),
            size: self.size,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensReconcileLabelsContract {
    pub existing: String,
    pub incoming: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensReconcileRequest {
    pub contract_version: String,
    pub lens: LensPin,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    pub base: LensReconcileSideContract,
    pub existing: LensReconcileSideContract,
    pub incoming: LensReconcileSideContract,
    pub labels: LensReconcileLabelsContract,
}

impl LensReconcileRequest {
    pub fn new(
        lens: LensPin,
        path: impl Into<String>,
        media_type: Option<String>,
        base: LensReconcileSideContract,
        existing: LensReconcileSideContract,
        incoming: LensReconcileSideContract,
        labels: LensReconcileLabelsContract,
    ) -> Self {
        Self {
            contract_version: LENS_CONTRACT_VERSION.to_string(),
            lens,
            path: path.into(),
            media_type,
            base,
            existing,
            incoming,
            labels,
        }
    }

    pub fn as_execution_input(&self) -> LensReconcileInput<'_> {
        LensReconcileInput {
            path: Some(Path::new(&self.path)),
            media_type: self.media_type.as_deref(),
            base: self.base.as_execution_side(),
            ours: self.existing.as_execution_side(),
            theirs: self.incoming.as_execution_side(),
            ours_label: &self.labels.existing,
            theirs_label: &self.labels.incoming,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensReconcileResponse {
    pub contract_version: String,
    pub lens: LensPin,
    pub result: LensReconcileResult,
}

impl LensReconcileResponse {
    pub fn new(lens: LensPin, result: LensReconcileResult) -> Self {
        Self::try_new(lens, result)
            .expect("Lens reconcile response requires a valid pin and result")
    }

    pub fn try_new(
        lens: LensPin,
        result: LensReconcileResult,
    ) -> Result<Self, crate::LensContractValidationError> {
        lens.validate()?;
        result.validate()?;
        Ok(Self {
            contract_version: LENS_CONTRACT_VERSION.to_string(),
            lens,
            result,
        })
    }
}

pub(crate) fn serialize_scoped_methods<S>(
    methods: &[ResolutionMethod],
    scope: ResolutionScope,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    methods
        .iter()
        .copied()
        .map(|method| ScopedResolutionMethod::new(method, scope))
        .collect::<Vec<_>>()
        .serialize(serializer)
}

pub(crate) fn deserialize_scoped_methods<'de, D>(
    deserializer: D,
    expected_scope: ResolutionScope,
) -> Result<Vec<ResolutionMethod>, D::Error>
where
    D: Deserializer<'de>,
{
    let methods = Vec::<ScopedResolutionMethod>::deserialize(deserializer)?;
    methods
        .into_iter()
        .map(|method| {
            if method.scope == expected_scope {
                Ok(method.method)
            } else {
                Err(D::Error::custom(format!(
                    "expected {} resolution method, found {}",
                    expected_scope.as_str(),
                    method.scope.as_str()
                )))
            }
        })
        .collect()
}

pub(crate) fn serialize_block_methods<S>(
    methods: &[ResolutionMethod],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serialize_scoped_methods(methods, ResolutionScope::Block, serializer)
}

pub(crate) fn deserialize_block_methods<'de, D>(
    deserializer: D,
) -> Result<Vec<ResolutionMethod>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_scoped_methods(deserializer, ResolutionScope::Block)
}
