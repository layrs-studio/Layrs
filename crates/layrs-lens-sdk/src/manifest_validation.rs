use std::collections::BTreeSet;

use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serializer};

use crate::{
    EsmLensEntrypoint, LENS_BUNDLE_FORMAT, LENS_CONTRACT_VERSION, LensBundleContract,
    LensContractValidationError, LensManifest, LensResourceQuotas, WasiLensEntrypoint,
    validate_sha256_digest,
};

pub const MAX_PORTABLE_INTEGER: u64 = 9_007_199_254_740_991;

pub fn is_lens_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub fn validate_lens_id(value: &str) -> Result<(), LensContractValidationError> {
    if is_lens_id(value) {
        Ok(())
    } else {
        Err(LensContractValidationError::new(
            "Lens id must start with an ASCII letter or digit and use only letters, digits, dots, underscores, or dashes",
        ))
    }
}

pub fn is_semantic_version(value: &str) -> bool {
    let mut build_split = value.split('+');
    let Some(version_and_pre) = build_split.next() else {
        return false;
    };
    if let Some(build) = build_split.next() {
        if !valid_identifier_list(build, false) {
            return false;
        }
    }
    if build_split.next().is_some() {
        return false;
    }

    let (core, pre_release) = version_and_pre
        .split_once('-')
        .map_or((version_and_pre, None), |(core, pre)| (core, Some(pre)));
    let core_parts = core.split('.').collect::<Vec<_>>();
    if core_parts.len() != 3 || !core_parts.iter().all(|part| valid_numeric_identifier(part)) {
        return false;
    }
    pre_release.is_none_or(|pre| valid_identifier_list(pre, true))
}

pub fn validate_semantic_version(value: &str) -> Result<(), LensContractValidationError> {
    if is_semantic_version(value) {
        Ok(())
    } else {
        Err(LensContractValidationError::new(
            "version must be a valid semantic version",
        ))
    }
}

pub(crate) fn serialize_contract_version<S>(value: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if value != LENS_CONTRACT_VERSION {
        return Err(S::Error::custom(format!(
            "contractVersion must be {LENS_CONTRACT_VERSION}"
        )));
    }
    serializer.serialize_str(value)
}

pub(crate) fn deserialize_contract_version<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value != LENS_CONTRACT_VERSION {
        return Err(D::Error::custom(format!(
            "contractVersion must be {LENS_CONTRACT_VERSION}"
        )));
    }
    Ok(value)
}

impl LensManifest {
    pub fn validate(&self) -> Result<(), LensContractValidationError> {
        if self.contract_version != LENS_CONTRACT_VERSION {
            return Err(LensContractValidationError::new(format!(
                "contractVersion must be {LENS_CONTRACT_VERSION}"
            )));
        }
        validate_lens_id(&self.id)?;
        validate_non_empty(&self.name, "manifest name")?;
        validate_semantic_version(&self.version)?;
        if let Some(digest) = self.digest.as_deref() {
            validate_sha256_digest(digest)?;
        }

        validate_unique_strings(
            &self.analyzer.supported_media_types,
            "analyzer supportedMediaTypes",
        )?;
        validate_unique_strings(&self.analyzer.file_extensions, "analyzer fileExtensions")?;
        for extension in &self.analyzer.file_extensions {
            if !valid_extension(extension) {
                return Err(LensContractValidationError::new(format!(
                    "invalid analyzer file extension {extension}"
                )));
            }
        }
        if self.analyzer.capabilities.is_empty() {
            return Err(LensContractValidationError::new(
                "analyzer capabilities must not be empty",
            ));
        }
        let mut capabilities = BTreeSet::new();
        for capability in &self.analyzer.capabilities {
            validate_non_empty(capability.as_str(), "analyzer capability")?;
            if !capabilities.insert(capability.as_str()) {
                return Err(LensContractValidationError::new(format!(
                    "duplicate analyzer capability {}",
                    capability.as_str()
                )));
            }
        }

        validate_non_empty(&self.viewer.viewer_id, "viewerId")?;
        if self.viewer.schema_version != "layrs.viewer.v1" {
            return Err(LensContractValidationError::new(
                "viewer schemaVersion must be layrs.viewer.v1",
            ));
        }
        validate_non_empty(&self.viewer.component, "viewer component")?;
        validate_unique_wire_values(
            self.viewer.preview_kinds.iter().map(|value| value.as_str()),
            "viewer previewKinds",
        )?;
        validate_unique_wire_values(
            self.viewer.diff_kinds.iter().map(|value| value.as_str()),
            "viewer diffKinds",
        )?;
        validate_unique_wire_values(
            self.viewer
                .reconcile_statuses
                .iter()
                .map(|value| value.as_str()),
            "viewer reconcileStatuses",
        )?;
        for field in &self.viewer.inspector_fields {
            validate_non_empty(&field.key, "inspector field key")?;
            validate_non_empty(&field.label, "inspector field label")?;
        }

        validate_unique_methods(&self.resolution_methods.file, "file resolution methods")?;
        validate_unique_methods(&self.resolution_methods.block, "block resolution methods")?;
        if (!self.resolution_methods.file.is_empty() || !self.resolution_methods.block.is_empty())
            && !self
                .analyzer
                .capabilities
                .iter()
                .any(|capability| capability.as_str() == "reconcile")
        {
            return Err(LensContractValidationError::new(
                "resolutionMethods require the reconcile capability",
            ));
        }

        if let Some(bundle) = &self.bundle {
            let digest = self.digest.as_deref().ok_or_else(|| {
                LensContractValidationError::new(
                    "a bundled Lens manifest requires an identity digest",
                )
            })?;
            validate_bundle(bundle, digest)?;
        }
        Ok(())
    }
}

pub(crate) fn serialize_lens_id<S>(value: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    validate_lens_id(value).map_err(S::Error::custom)?;
    serializer.serialize_str(value)
}

pub(crate) fn deserialize_lens_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    validate_lens_id(&value).map_err(D::Error::custom)?;
    Ok(value)
}

pub(crate) fn serialize_semantic_version<S>(value: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    validate_semantic_version(value).map_err(S::Error::custom)?;
    serializer.serialize_str(value)
}

pub(crate) fn deserialize_semantic_version<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    validate_semantic_version(&value).map_err(D::Error::custom)?;
    Ok(value)
}

fn validate_bundle(
    bundle: &LensBundleContract,
    identity_digest: &str,
) -> Result<(), LensContractValidationError> {
    if bundle.format != LENS_BUNDLE_FORMAT {
        return Err(LensContractValidationError::new(format!(
            "bundle format must be {LENS_BUNDLE_FORMAT}"
        )));
    }
    let entrypoints = &bundle.entrypoints;
    if entrypoints.analyzer.is_none()
        && entrypoints.reconcile.is_none()
        && entrypoints.viewer.is_none()
    {
        return Err(LensContractValidationError::new(
            "bundle must declare at least one entrypoint",
        ));
    }
    if let Some(entrypoint) = &entrypoints.analyzer {
        validate_wasi_entrypoint(entrypoint, "analyzer")?;
    }
    if let Some(entrypoint) = &entrypoints.reconcile {
        validate_wasi_entrypoint(entrypoint, "reconcile")?;
    }
    if let Some(entrypoint) = &entrypoints.viewer {
        validate_esm_entrypoint(entrypoint, "viewer")?;
    }
    if let Some(signature) = &bundle.signature {
        validate_non_empty(&signature.algorithm, "bundle signature algorithm")?;
        validate_non_empty(&signature.key_id, "bundle signature keyId")?;
        validate_sha256_digest(&signature.signed_digest)?;
        if signature.signed_digest != identity_digest {
            return Err(LensContractValidationError::new(
                "bundle signedDigest must equal the manifest identity digest",
            ));
        }
        if !valid_base64_signature(&signature.signature) {
            return Err(LensContractValidationError::new(
                "bundle signature must be base64-prefixed data",
            ));
        }
    }
    Ok(())
}

fn validate_wasi_entrypoint(
    entrypoint: &WasiLensEntrypoint,
    label: &str,
) -> Result<(), LensContractValidationError> {
    validate_entrypoint(
        &entrypoint.path,
        &entrypoint.export,
        &entrypoint.digest,
        &entrypoint.resources,
        label,
    )
}

fn validate_esm_entrypoint(
    entrypoint: &EsmLensEntrypoint,
    label: &str,
) -> Result<(), LensContractValidationError> {
    validate_entrypoint(
        &entrypoint.path,
        &entrypoint.export,
        &entrypoint.digest,
        &entrypoint.resources,
        label,
    )
}

fn validate_entrypoint(
    path: &str,
    export: &str,
    digest: &str,
    resources: &LensResourceQuotas,
    label: &str,
) -> Result<(), LensContractValidationError> {
    if !valid_bundle_path(path) {
        return Err(LensContractValidationError::new(format!(
            "{label} entrypoint path must be a safe relative bundle path"
        )));
    }
    validate_non_empty(export, &format!("{label} entrypoint export"))?;
    validate_sha256_digest(digest)?;
    let required_quotas = [
        resources.max_memory_bytes,
        resources.max_execution_ms,
        resources.max_input_bytes,
        resources.max_output_bytes,
    ];
    if required_quotas
        .into_iter()
        .any(|value| value == 0 || value > MAX_PORTABLE_INTEGER)
        || resources
            .max_fuel
            .is_some_and(|value| value == 0 || value > MAX_PORTABLE_INTEGER)
    {
        return Err(LensContractValidationError::new(format!(
            "{label} entrypoint resource quotas must be between 1 and {MAX_PORTABLE_INTEGER}"
        )));
    }
    Ok(())
}

fn valid_identifier_list(value: &str, reject_numeric_leading_zero: bool) -> bool {
    !value.is_empty()
        && value.split('.').all(|identifier| {
            !identifier.is_empty()
                && identifier
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && (!reject_numeric_leading_zero
                    || !identifier.bytes().all(|byte| byte.is_ascii_digit())
                    || valid_numeric_identifier(identifier))
        })
}

fn valid_numeric_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
}

fn valid_extension(value: &str) -> bool {
    value
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_bundle_path(value: &str) -> bool {
    !value.is_empty()
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'-'))
}

fn valid_base64_signature(value: &str) -> bool {
    let Some(encoded) = value.strip_prefix("base64:") else {
        return false;
    };
    let padding = encoded.chars().rev().take_while(|ch| *ch == '=').count();
    let body = &encoded[..encoded.len().saturating_sub(padding)];
    !body.is_empty()
        && padding <= 2
        && !body.contains('=')
        && encoded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

fn validate_non_empty(value: &str, label: &str) -> Result<(), LensContractValidationError> {
    if value.is_empty() {
        Err(LensContractValidationError::new(format!(
            "{label} must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn validate_unique_strings(
    values: &[String],
    label: &str,
) -> Result<(), LensContractValidationError> {
    validate_unique_wire_values(values.iter().map(String::as_str), label)
}

fn validate_unique_wire_values<'a>(
    values: impl Iterator<Item = &'a str>,
    label: &str,
) -> Result<(), LensContractValidationError> {
    let mut seen = BTreeSet::new();
    for value in values {
        validate_non_empty(value, label)?;
        if !seen.insert(value) {
            return Err(LensContractValidationError::new(format!(
                "{label} contains duplicate value {value}"
            )));
        }
    }
    Ok(())
}

fn validate_unique_methods(
    methods: &[crate::ResolutionMethod],
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
