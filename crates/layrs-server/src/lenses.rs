use layrs_lens_sdk::{LensManifest, LoadedLensReconcileImplementation};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const LENSES_DIR_ENV: &str = "LAYRS_LENSES_DIR";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LensRegistry {
    pub items: Vec<LensManifest>,
    pub errors: Vec<LensManifestError>,
}

impl LensRegistry {
    pub fn response_value(&self) -> Value {
        json!({
            "items": &self.items,
            "errors": &self.errors
        })
    }

    pub fn response_json(&self) -> String {
        serde_json::to_string(&self.response_value())
            .unwrap_or_else(|_| r#"{"items":[],"errors":[]}"#.to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LensManifestError {
    pub path: String,
    pub code: &'static str,
    pub message: String,
}

pub fn load_lens_registry_from_env() -> LensRegistry {
    let lenses_dir = env::var_os(LENSES_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("lenses"));

    load_lens_registry(lenses_dir)
}

pub fn registry_response_value_from_env() -> Value {
    load_lens_registry_from_env().response_value()
}

pub fn registry_response_json_from_env() -> String {
    load_lens_registry_from_env().response_json()
}

pub fn load_lens_registry(lenses_dir: impl AsRef<Path>) -> LensRegistry {
    let lenses_dir = lenses_dir.as_ref();
    let mut registry = LensRegistry {
        items: built_in_lens_manifests(),
        errors: Vec::new(),
    };
    let mut ids = registry
        .items
        .iter()
        .map(|manifest| manifest.id.clone())
        .collect::<BTreeSet<_>>();

    let entries = match fs::read_dir(lenses_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return registry,
        Err(error) => {
            registry.errors.push(registry_error(
                lenses_dir,
                "lenses_dir_unreadable",
                format!("could not read lenses directory: {error}"),
            ));
            return registry;
        }
    };

    let mut lens_dirs = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => lens_dirs.push(entry),
            Err(error) => registry.errors.push(registry_error(
                lenses_dir,
                "lenses_dir_entry_unreadable",
                format!("could not read lenses directory entry: {error}"),
            )),
        }
    }
    lens_dirs.sort_by_key(|entry| entry.file_name());

    for entry in lens_dirs {
        let lens_dir = entry.path();
        match entry.file_type() {
            Ok(file_type) if file_type.is_dir() => {}
            Ok(_) => continue,
            Err(error) => {
                registry.errors.push(registry_error(
                    &lens_dir,
                    "lens_dir_unreadable",
                    format!("could not inspect lens directory entry: {error}"),
                ));
                continue;
            }
        }

        let manifest_path = lens_dir.join("manifest.json");
        if !manifest_path.is_file() {
            registry.errors.push(registry_error(
                &manifest_path,
                "manifest_missing",
                "manifest.json is required for an external lens",
            ));
            continue;
        }

        let manifest_text = match fs::read_to_string(&manifest_path) {
            Ok(value) => value,
            Err(error) => {
                registry.errors.push(registry_error(
                    &manifest_path,
                    "manifest_unreadable",
                    format!("could not read lens manifest: {error}"),
                ));
                continue;
            }
        };
        let manifest_value = match serde_json::from_str::<Value>(&manifest_text) {
            Ok(value) => value,
            Err(error) => {
                registry.errors.push(registry_error(
                    &manifest_path,
                    "manifest_parse_failed",
                    format!("manifest.json is not valid JSON: {error}"),
                ));
                continue;
            }
        };
        let manifest = match serde_json::from_value::<LensManifest>(manifest_value) {
            Ok(manifest) => manifest,
            Err(error) => {
                registry.errors.push(registry_error(
                    &manifest_path,
                    "manifest_invalid",
                    format!("manifest.json is not a canonical LensManifest: {error}"),
                ));
                continue;
            }
        };
        if let Err(message) = validate_external_manifest(&lens_dir, &manifest) {
            registry
                .errors
                .push(registry_error(&manifest_path, "manifest_invalid", message));
            continue;
        }
        if !ids.insert(manifest.id.clone()) {
            registry.errors.push(registry_error(
                &manifest_path,
                "duplicate_lens_id",
                format!("lens id {} is already registered", manifest.id),
            ));
            continue;
        }

        registry.items.push(manifest);
    }

    registry
}

pub fn built_in_lens_manifests() -> Vec<LensManifest> {
    canonical_built_in_lens_manifests().to_vec()
}

#[cfg(test)]
fn built_in_lens_manifest(lens_id: &str) -> Option<&'static LensManifest> {
    canonical_built_in_lens_manifests()
        .iter()
        .find(|manifest| manifest.id == lens_id)
}

fn canonical_built_in_lens_manifests() -> &'static [LensManifest; 4] {
    static MANIFESTS: OnceLock<[LensManifest; 4]> = OnceLock::new();
    MANIFESTS.get_or_init(|| {
        [
            pin_built_in_manifest(layrs_lens_code::manifest()),
            pin_built_in_manifest(layrs_lens_text::manifest()),
            pin_built_in_manifest(layrs_lens_image::manifest()),
            pin_built_in_manifest(layrs_lens_raw::manifest()),
        ]
    })
}

fn pin_built_in_manifest(mut manifest: LensManifest) -> LensManifest {
    manifest.digest = Some(
        built_in_implementation_digest(&manifest.id)
            .expect("every built-in Lens has implementation sources"),
    );
    manifest
        .validate()
        .expect("built-in Lens manifest satisfies the canonical contract");
    manifest
}

fn built_in_implementation_digest(lens_id: &str) -> Option<String> {
    let mut sources: Vec<(&str, &[u8])> = match lens_id {
        layrs_lens_code::CODE_LENS_ID => vec![
            (
                "crates/layrs-lens-code/src/lib.rs",
                include_bytes!("../../layrs-lens-code/src/lib.rs"),
            ),
            (
                "crates/layrs-lens-text/src/lib.rs",
                include_bytes!("../../layrs-lens-text/src/lib.rs"),
            ),
        ],
        layrs_lens_text::TEXT_LENS_ID => vec![(
            "crates/layrs-lens-text/src/lib.rs",
            include_bytes!("../../layrs-lens-text/src/lib.rs"),
        )],
        layrs_lens_image::IMAGE_LENS_ID => vec![(
            "crates/layrs-lens-image/src/lib.rs",
            include_bytes!("../../layrs-lens-image/src/lib.rs"),
        )],
        layrs_lens_raw::RAW_LENS_ID => vec![(
            "crates/layrs-lens-raw/src/lib.rs",
            include_bytes!("../../layrs-lens-raw/src/lib.rs"),
        )],
        _ => return None,
    };
    sources.extend_from_slice(SDK_IMPLEMENTATION_SOURCES);
    Some(implementation_digest(&sources))
}

const SDK_IMPLEMENTATION_SOURCES: &[(&str, &[u8])] = &[
    (
        "crates/layrs-lens-sdk/src/lib.rs",
        include_bytes!("../../layrs-lens-sdk/src/lib.rs"),
    ),
    (
        "crates/layrs-lens-sdk/src/contract.rs",
        include_bytes!("../../layrs-lens-sdk/src/contract.rs"),
    ),
    (
        "crates/layrs-lens-sdk/src/manifest_validation.rs",
        include_bytes!("../../layrs-lens-sdk/src/manifest_validation.rs"),
    ),
    (
        "crates/layrs-lens-sdk/src/selection.rs",
        include_bytes!("../../layrs-lens-sdk/src/selection.rs"),
    ),
    (
        "crates/layrs-lens-sdk/src/validation.rs",
        include_bytes!("../../layrs-lens-sdk/src/validation.rs"),
    ),
    (
        "crates/layrs-lens-sdk/src/wire.rs",
        include_bytes!("../../layrs-lens-sdk/src/wire.rs"),
    ),
];

fn validate_external_manifest(lens_dir: &Path, manifest: &LensManifest) -> Result<(), String> {
    manifest.validate().map_err(|error| error.to_string())?;
    let Some(bundle) = manifest.bundle.as_ref() else {
        if manifest.digest.is_some() {
            return Err(
                "an external Lens without an executable bundle cannot declare a verifiable digest"
                    .to_string(),
            );
        }
        return Ok(());
    };

    if let Some(entrypoint) = bundle.entrypoints.analyzer.as_ref() {
        validate_entrypoint_file(lens_dir, &entrypoint.path, &entrypoint.digest)?;
    }
    if let Some(entrypoint) = bundle.entrypoints.reconcile.as_ref() {
        validate_entrypoint_file(lens_dir, &entrypoint.path, &entrypoint.digest)?;
    }
    if let Some(entrypoint) = bundle.entrypoints.viewer.as_ref() {
        validate_entrypoint_file(lens_dir, &entrypoint.path, &entrypoint.digest)?;
    }

    let expected_identity = manifest_identity_digest(manifest)?;
    if manifest.digest.as_deref() != Some(expected_identity.as_str()) {
        return Err(format!(
            "manifest identity digest does not match canonical contract and implementation digests; expected {expected_identity}"
        ));
    }
    Ok(())
}

pub fn load_external_reconcile_implementation(
    lens_dir: &Path,
    manifest: &LensManifest,
) -> Result<LoadedLensReconcileImplementation, String> {
    validate_external_manifest(lens_dir, manifest)?;
    let entrypoint = manifest
        .bundle
        .as_ref()
        .and_then(|bundle| bundle.entrypoints.reconcile.as_ref())
        .ok_or_else(|| "Lens manifest does not declare a reconcile entrypoint".to_string())?;
    let bytes = read_entrypoint_file(lens_dir, &entrypoint.path, &entrypoint.digest)?;
    LoadedLensReconcileImplementation::try_load(manifest, bytes).map_err(|error| error.to_string())
}

fn validate_entrypoint_file(lens_dir: &Path, path: &str, expected: &str) -> Result<(), String> {
    read_entrypoint_file(lens_dir, path, expected).map(|_| ())
}

fn read_entrypoint_file(lens_dir: &Path, path: &str, expected: &str) -> Result<Box<[u8]>, String> {
    let root = fs::canonicalize(lens_dir)
        .map_err(|error| format!("could not resolve external Lens directory: {error}"))?;
    let entrypoint = fs::canonicalize(lens_dir.join(path))
        .map_err(|error| format!("could not resolve bundle entrypoint {path}: {error}"))?;
    if !entrypoint.starts_with(&root) || !entrypoint.is_file() {
        return Err(format!(
            "bundle entrypoint {path} must resolve to a file inside the Lens directory"
        ));
    }
    let bytes = fs::read(&entrypoint)
        .map_err(|error| format!("could not read bundle entrypoint {path}: {error}"))?;
    let actual = sha256_digest(&bytes);
    if actual != expected {
        return Err(format!(
            "bundle entrypoint {path} digest mismatch: expected {expected}, found {actual}"
        ));
    }
    Ok(bytes.into_boxed_slice())
}

fn manifest_identity_digest(manifest: &LensManifest) -> Result<String, String> {
    let mut identity = manifest.clone();
    identity.digest = None;
    if let Some(bundle) = identity.bundle.as_mut() {
        bundle.signature = None;
    }
    let payload = serde_json::to_vec(&identity)
        .map_err(|error| format!("could not serialize canonical manifest identity: {error}"))?;
    Ok(sha256_digest(&payload))
}

fn implementation_digest(parts: &[(&str, &[u8])]) -> String {
    let mut hasher = Sha256::new();
    for (path, bytes) in parts {
        hasher.update((path.len() as u64).to_be_bytes());
        hasher.update(path.as_bytes());
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn registry_error(
    path: impl AsRef<Path>,
    code: &'static str,
    message: impl Into<String>,
) -> LensManifestError {
    LensManifestError {
        path: path.as_ref().display().to_string(),
        code,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_lens_sdk::{
        AnalyzerContract, DiffKind, LensBundleContract, LensBundleEntrypoints, LensCapability,
        LensResourceQuotas, LensSelectionRequest, PreviewKind, ResolutionMethod, ResolutionMethods,
        ViewerContract, WasiLensEntrypoint, select_lens_manifest,
    };
    use serde::Deserialize;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn missing_lenses_dir_keeps_built_ins_without_errors() {
        let registry = load_lens_registry(unique_temp_dir("missing"));

        assert_eq!(lens_ids(&registry.items), built_in_ids());
        assert!(registry.errors.is_empty());
    }

    #[test]
    fn built_ins_are_canonical_and_pinned_to_implementation_sources() {
        let manifests = built_in_lens_manifests();
        assert_eq!(lens_ids(&manifests), built_in_ids());
        for manifest in manifests {
            manifest.validate().expect("built-in manifest is canonical");
            assert_eq!(
                manifest.digest.as_deref(),
                built_in_implementation_digest(&manifest.id).as_deref()
            );
        }

        assert_ne!(
            implementation_digest(&[("lens.rs", b"before")]),
            implementation_digest(&[("lens.rs", b"after")])
        );
    }

    #[test]
    fn canonical_capabilities_exclude_code_and_image_reconcile() {
        for lens_id in [
            layrs_lens_code::CODE_LENS_ID,
            layrs_lens_image::IMAGE_LENS_ID,
        ] {
            let manifest = built_in_lens_manifest(lens_id).expect("built-in manifest");
            assert!(!manifest.supports_capability(&LensCapability::Reconcile));
            assert!(manifest.resolution_methods.file.is_empty());
            assert!(manifest.resolution_methods.block.is_empty());
        }
    }

    #[test]
    fn external_lenses_are_typed_canonical_manifests() {
        let root = unique_temp_dir("canonical");
        let lens_dir = root.join("markdown");
        fs::create_dir_all(&lens_dir).expect("lens dir is created");
        write_manifest(&lens_dir, &external_manifest("com.example.markdown"));

        let registry = load_lens_registry(&root);
        let _ = fs::remove_dir_all(&root);

        let mut expected_ids = built_in_ids();
        expected_ids.push("com.example.markdown".to_string());
        assert_eq!(lens_ids(&registry.items), expected_ids);
        assert!(registry.errors.is_empty());
        let external = registry.items.last().expect("external manifest");
        external.validate().expect("external manifest is canonical");
        let value = serde_json::to_value(external).expect("manifest serializes");
        let object = value.as_object().expect("manifest object");
        assert!(!object.contains_key("applies_to"));
        assert!(!object.contains_key("capabilities"));
        assert!(!object.contains_key("permissions"));
    }

    #[test]
    fn legacy_external_manifest_fields_are_rejected() {
        let root = unique_temp_dir("legacy");
        let lens_dir = root.join("markdown");
        fs::create_dir_all(&lens_dir).expect("lens dir is created");
        let mut value = serde_json::to_value(external_manifest("com.example.markdown"))
            .expect("manifest serializes");
        value.as_object_mut().expect("manifest object").insert(
            "applies_to".to_string(),
            json!({"media_types": ["text/markdown"]}),
        );
        fs::write(
            lens_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&value).expect("manifest JSON"),
        )
        .expect("manifest is written");

        let registry = load_lens_registry(&root);
        let _ = fs::remove_dir_all(&root);

        assert_eq!(lens_ids(&registry.items), built_in_ids());
        assert_eq!(registry.errors.len(), 1);
        assert_eq!(registry.errors[0].code, "manifest_invalid");
    }

    #[test]
    fn external_bundle_identity_tracks_executed_bytes() {
        let root = unique_temp_dir("bundle");
        let lens_dir = root.join("reconcile");
        fs::create_dir_all(&lens_dir).expect("lens dir is created");
        let entrypoint = lens_dir.join("reconcile.wasm");
        fs::write(&entrypoint, b"implementation-v1").expect("entrypoint is written");

        let mut manifest = bundled_external_manifest(b"implementation-v1");
        let first_identity = manifest.digest.clone().expect("identity digest");
        write_manifest(&lens_dir, &manifest);
        assert!(load_lens_registry(&root).errors.is_empty());

        fs::write(&entrypoint, b"implementation-v2").expect("entrypoint is updated");
        let stale_implementation = load_lens_registry(&root);
        assert_eq!(stale_implementation.errors.len(), 1);
        assert!(
            stale_implementation.errors[0]
                .message
                .contains("digest mismatch")
        );

        manifest
            .bundle
            .as_mut()
            .and_then(|bundle| bundle.entrypoints.reconcile.as_mut())
            .expect("reconcile entrypoint")
            .digest = sha256_digest(b"implementation-v2");
        write_manifest(&lens_dir, &manifest);
        let stale_identity = load_lens_registry(&root);
        assert_eq!(stale_identity.errors.len(), 1);
        assert!(stale_identity.errors[0].message.contains("identity digest"));

        manifest.digest = Some(manifest_identity_digest(&manifest).expect("identity digest"));
        assert_ne!(manifest.digest.as_deref(), Some(first_identity.as_str()));
        write_manifest(&lens_dir, &manifest);
        assert!(load_lens_registry(&root).errors.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn real_reconcile_loader_rejects_mutated_executable_bytes() {
        let root = unique_temp_dir("loaded-bundle");
        let lens_dir = root.join("reconcile");
        fs::create_dir_all(&lens_dir).expect("lens dir is created");
        let entrypoint = lens_dir.join("reconcile.wasm");
        fs::write(&entrypoint, b"implementation-v1").expect("entrypoint is written");
        let manifest = bundled_external_manifest(b"implementation-v1");
        write_manifest(&lens_dir, &manifest);

        let loaded = load_external_reconcile_implementation(&lens_dir, &manifest)
            .expect("real loader accepts matching executable bytes");
        fs::write(&entrypoint, b"implementation-v2").expect("entrypoint is mutated");

        assert_eq!(loaded.implementation_bytes(), b"implementation-v1");
        let error = load_external_reconcile_implementation(&lens_dir, &manifest)
            .expect_err("old manifest must not prove mutated executable bytes");
        assert!(error.contains("digest mismatch"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn server_loader_matches_shared_bundle_validation_vectors() {
        let vectors: BundleValidationVectors = serde_json::from_str(include_str!(
            "../../../packages/lens-sdk/test/bundle-validation.vectors.json"
        ))
        .expect("bundle validation vectors parse");

        for (index, vector) in vectors.paths.into_iter().enumerate() {
            let root = unique_temp_dir(&format!("path-{index}"));
            let lens_dir = root.join("reconcile");
            fs::create_dir_all(&lens_dir).expect("lens dir is created");
            let mut manifest = bundled_external_manifest(b"implementation");
            manifest
                .bundle
                .as_mut()
                .and_then(|bundle| bundle.entrypoints.reconcile.as_mut())
                .expect("reconcile entrypoint")
                .path = vector.value.clone();
            manifest.digest = Some(manifest_identity_digest(&manifest).expect("identity digest"));
            if vector.valid {
                let entrypoint = lens_dir.join(&vector.value);
                fs::create_dir_all(entrypoint.parent().expect("entrypoint parent"))
                    .expect("entrypoint parent is created");
                fs::write(entrypoint, b"implementation").expect("entrypoint is written");
            }
            write_manifest(&lens_dir, &manifest);

            let registry = load_lens_registry(&root);
            assert_eq!(registry.errors.is_empty(), vector.valid, "{}", vector.name);
            let _ = fs::remove_dir_all(&root);
        }

        for (index, vector) in vectors.quotas.into_iter().enumerate() {
            let root = unique_temp_dir(&format!("quota-{index}"));
            let lens_dir = root.join("reconcile");
            fs::create_dir_all(&lens_dir).expect("lens dir is created");
            fs::write(lens_dir.join("reconcile.wasm"), b"implementation")
                .expect("entrypoint is written");
            let mut manifest = bundled_external_manifest(b"implementation");
            manifest
                .bundle
                .as_mut()
                .and_then(|bundle| bundle.entrypoints.reconcile.as_mut())
                .expect("reconcile entrypoint")
                .resources
                .max_memory_bytes = vector.value;
            manifest.digest = Some(manifest_identity_digest(&manifest).expect("identity digest"));
            write_manifest(&lens_dir, &manifest);

            let registry = load_lens_registry(&root);
            assert_eq!(registry.errors.is_empty(), vector.valid, "{}", vector.name);
            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn external_loader_rejects_symlink_escape() {
        let root = unique_temp_dir("symlink");
        let lens_dir = root.join("reconcile");
        let outside = root.with_extension("outside.wasm");
        fs::create_dir_all(&lens_dir).expect("lens dir is created");
        fs::write(&outside, b"outside implementation").expect("outside file is written");
        if !create_file_symlink(&outside, &lens_dir.join("reconcile.wasm")) {
            let _ = fs::remove_dir_all(&root);
            let _ = fs::remove_file(&outside);
            return;
        }
        let manifest = bundled_external_manifest(b"outside implementation");
        write_manifest(&lens_dir, &manifest);

        let registry = load_lens_registry(&root);

        assert_eq!(registry.errors.len(), 1);
        assert!(
            registry.errors[0]
                .message
                .contains("inside the Lens directory")
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&outside);
    }

    #[test]
    fn duplicate_external_ids_do_not_replace_built_ins() {
        let root = unique_temp_dir("duplicate");
        let lens_dir = root.join("code");
        fs::create_dir_all(&lens_dir).expect("lens dir is created");
        write_manifest(&lens_dir, &external_manifest("layrs.code"));

        let registry = load_lens_registry(&root);
        let _ = fs::remove_dir_all(&root);

        assert_eq!(lens_ids(&registry.items), built_in_ids());
        assert_eq!(registry.errors.len(), 1);
        assert_eq!(registry.errors[0].code, "duplicate_lens_id");
    }

    #[test]
    fn rust_selector_matches_shared_fail_closed_vectors() {
        let vectors: Vec<SelectionVector> = serde_json::from_str(include_str!(
            "../../../packages/lens-sdk/test/selection.vectors.json"
        ))
        .expect("selection vectors parse");

        for vector in vectors {
            let mut manifests = built_in_lens_manifests();
            if let Some(source_id) = vector.duplicate_from_lens_id.as_deref() {
                let mut duplicate = manifests
                    .iter()
                    .find(|manifest| manifest.id == source_id)
                    .expect("duplicate source manifest")
                    .clone();
                duplicate.id = vector
                    .duplicate_lens_id
                    .clone()
                    .expect("duplicate id is present");
                manifests.push(duplicate);
            }

            let result = select_lens_manifest(
                &manifests,
                LensSelectionRequest {
                    lens_id: vector.lens_id.as_deref(),
                    path: vector.path.as_deref(),
                    media_type: vector.media_type.as_deref(),
                    fallback_lens_id: vector.fallback_lens_id.as_deref(),
                },
            );
            if let Some(expected_error) = vector.expected_error.as_deref() {
                let error = result.expect_err(&vector.name);
                assert_eq!(error.code(), expected_error, "{}", vector.name);
                continue;
            }
            let (manifest, reason) = result
                .expect(&vector.name)
                .expect("selection vector expects a match");
            assert_eq!(
                Some(manifest.id.as_str()),
                vector.expected_lens_id.as_deref(),
                "{}",
                vector.name
            );
            assert_eq!(
                Some(reason.as_str()),
                vector.expected_reason.as_deref(),
                "{}",
                vector.name
            );
        }
    }

    fn external_manifest(id: &str) -> LensManifest {
        LensManifest::new(
            id,
            "External Lens",
            "1.2.3",
            AnalyzerContract {
                supported_media_types: vec!["text/markdown".to_string()],
                file_extensions: vec!["mdx".to_string()],
                capabilities: vec![LensCapability::View, LensCapability::Preview],
            },
            ViewerContract::new(
                "com.example.viewer",
                "ExternalViewer",
                vec![PreviewKind::Text],
                vec![DiffKind::TextLines],
            ),
        )
    }

    fn bundled_external_manifest(bytes: &[u8]) -> LensManifest {
        let mut manifest = external_manifest("com.example.reconcile");
        manifest
            .analyzer
            .capabilities
            .push(LensCapability::Reconcile);
        manifest.resolution_methods =
            ResolutionMethods::file(vec![ResolutionMethod::Existing, ResolutionMethod::Incoming]);
        manifest.digest = Some(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        );
        manifest.bundle = Some(LensBundleContract::new(LensBundleEntrypoints {
            analyzer: None,
            reconcile: Some(WasiLensEntrypoint::new(
                "reconcile.wasm",
                "reconcile",
                sha256_digest(bytes),
                LensResourceQuotas {
                    max_memory_bytes: 64 * 1024 * 1024,
                    max_execution_ms: 2_000,
                    max_input_bytes: 16 * 1024 * 1024,
                    max_output_bytes: 16 * 1024 * 1024,
                    max_fuel: Some(10_000_000),
                },
            )),
            viewer: None,
        }));
        manifest.digest = Some(manifest_identity_digest(&manifest).expect("identity digest"));
        manifest
    }

    fn write_manifest(lens_dir: &Path, manifest: &LensManifest) {
        fs::write(
            lens_dir.join("manifest.json"),
            serde_json::to_vec_pretty(manifest).expect("manifest JSON"),
        )
        .expect("manifest is written");
    }

    fn lens_ids(items: &[LensManifest]) -> Vec<String> {
        items.iter().map(|item| item.id.clone()).collect()
    }

    fn built_in_ids() -> Vec<String> {
        vec![
            "layrs.code".to_string(),
            "layrs.text".to_string(),
            "layrs.image".to_string(),
            "layrs.raw".to_string(),
        ]
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time is after epoch")
            .as_nanos();
        env::temp_dir().join(format!(
            "layrs-lenses-{label}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SelectionVector {
        name: String,
        lens_id: Option<String>,
        path: Option<String>,
        media_type: Option<String>,
        fallback_lens_id: Option<String>,
        duplicate_from_lens_id: Option<String>,
        duplicate_lens_id: Option<String>,
        expected_lens_id: Option<String>,
        expected_reason: Option<String>,
        expected_error: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct BundleValidationVectors {
        paths: Vec<BundlePathVector>,
        quotas: Vec<BundleQuotaVector>,
    }

    #[derive(Debug, Deserialize)]
    struct BundlePathVector {
        name: String,
        value: String,
        valid: bool,
    }

    #[derive(Debug, Deserialize)]
    struct BundleQuotaVector {
        name: String,
        value: u64,
        valid: bool,
    }

    #[cfg(unix)]
    fn create_file_symlink(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).expect("file symlink is created");
        true
    }

    #[cfg(windows)]
    fn create_file_symlink(target: &Path, link: &Path) -> bool {
        match std::os::windows::fs::symlink_file(target, link) {
            Ok(()) => true,
            Err(error)
                if error.kind() == io::ErrorKind::PermissionDenied
                    || error.raw_os_error() == Some(1314) =>
            {
                false
            }
            Err(error) => panic!("file symlink is created: {error}"),
        }
    }
}
