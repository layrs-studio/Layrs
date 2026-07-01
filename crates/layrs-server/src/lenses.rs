use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const LENSES_DIR_ENV: &str = "LAYRS_LENSES_DIR";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LensRegistry {
    pub items: Vec<Value>,
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
        .filter_map(|manifest| manifest.get("id").and_then(Value::as_str))
        .map(str::to_string)
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
        let manifest = match serde_json::from_str::<Value>(&manifest_text) {
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
        let manifest = match normalize_manifest(manifest) {
            Ok(value) => value,
            Err(message) => {
                registry
                    .errors
                    .push(registry_error(&manifest_path, "manifest_invalid", message));
                continue;
            }
        };
        let id = manifest
            .get("id")
            .and_then(Value::as_str)
            .expect("normalized manifest has an id");
        if !ids.insert(id.to_string()) {
            registry.errors.push(registry_error(
                &manifest_path,
                "duplicate_lens_id",
                format!("lens id {id} is already registered"),
            ));
            continue;
        }

        registry.items.push(manifest);
    }

    registry
}

pub fn built_in_lens_manifests() -> Vec<Value> {
    vec![
        lens_manifest(
            "layrs.code",
            "Code",
            "CodeArtifactViewer",
            &["code"],
            &["textLines"],
            &[
                "text/rust",
                "text/typescript",
                "text/javascript",
                "text/css",
                "text/html",
                "application/json",
                "application/toml",
                "application/yaml",
                "text/x-go",
                "text/x-python",
            ],
            &[
                "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "json", "css", "html", "htm", "toml",
                "yaml", "yml", "py", "go", "java", "kt", "kts", "swift", "c", "h", "cc", "cpp",
                "cxx", "hpp", "cs", "php", "rb", "sh", "bash", "zsh", "ps1", "sql",
            ],
            &[
                "view",
                "diff",
                "reconcile",
                "metadata",
                "preview",
                "references",
            ],
        ),
        lens_manifest(
            "layrs.text",
            "Text",
            "TextArtifactViewer",
            &["text"],
            &["textLines"],
            &["text/plain", "text/markdown"],
            &["txt", "md", "markdown", "rst", "log"],
            &[
                "view",
                "diff",
                "reconcile",
                "metadata",
                "preview",
                "references",
            ],
        ),
        lens_manifest(
            "layrs.image",
            "Image",
            "ImageArtifactViewer",
            &["image"],
            &["imageMetadata"],
            &["image/png", "image/jpeg", "image/webp"],
            &["png", "jpg", "jpeg", "webp"],
            &[
                "view",
                "diff",
                "reconcile",
                "metadata",
                "preview",
                "proofRecipes",
            ],
        ),
        lens_manifest(
            "layrs.raw",
            "Raw",
            "RawArtifactViewer",
            &["raw"],
            &["binary"],
            &["application/octet-stream"],
            &[],
            &["view", "diff", "reconcile", "metadata", "preview"],
        ),
    ]
}

fn lens_manifest(
    id: &str,
    name: &str,
    component: &str,
    preview_kinds: &[&str],
    diff_kinds: &[&str],
    supported_media_types: &[&str],
    file_extensions: &[&str],
    capabilities: &[&str],
) -> Value {
    json!({
        "id": id,
        "name": name,
        "version": "0.0.0",
        "applies_to": {
            "artifact_kinds": preview_kinds,
            "media_types": supported_media_types,
            "file_extensions": file_extensions
        },
        "capabilities": capabilities,
        "permissions": {},
        "analyzer": {
            "supportedMediaTypes": supported_media_types,
            "fileExtensions": file_extensions,
            "capabilities": capabilities
        },
        "viewer": {
            "viewerId": format!("layrs.viewer.{}", id.trim_start_matches("layrs.")),
            "schemaVersion": "layrs.viewer.v1",
            "component": component,
            "previewKinds": preview_kinds,
            "diffKinds": diff_kinds,
            "reconcileStatuses": [
                "unsupported",
                "needs_manual_resolution",
                "auto_resolvable"
            ],
            "inspectorFields": []
        }
    })
}

fn normalize_manifest(manifest: Value) -> Result<Value, String> {
    let mut object = match manifest {
        Value::Object(object) => object,
        _ => return Err("manifest must be a JSON object".to_string()),
    };
    move_alias(&mut object, "appliesTo", "applies_to");

    let id = required_string(&mut object, "id")?;
    if !valid_lens_id(&id) {
        return Err(
            "id must use only ASCII letters, digits, dots, underscores or dashes".to_string(),
        );
    }
    required_string(&mut object, "name")?;
    required_string(&mut object, "version")?;

    let mut analyzer = required_object(&mut object, "analyzer")?;
    move_alias(
        &mut analyzer,
        "supported_media_types",
        "supportedMediaTypes",
    );
    move_alias(&mut analyzer, "file_extensions", "fileExtensions");

    let supported_media_types = required_string_array(&mut analyzer, "supportedMediaTypes", true)?;
    let file_extensions = required_string_array(&mut analyzer, "fileExtensions", true)?;
    if supported_media_types.is_empty() && file_extensions.is_empty() {
        return Err(
            "analyzer.supportedMediaTypes or analyzer.fileExtensions must include at least one value"
                .to_string(),
        );
    }

    let analyzer_capabilities = optional_string_array(&analyzer, "capabilities")?;
    let manifest_capabilities = optional_string_array(&object, "capabilities")?;
    let capabilities = manifest_capabilities
        .clone()
        .or_else(|| analyzer_capabilities.clone())
        .ok_or_else(|| "capabilities or analyzer.capabilities is required".to_string())?;
    if capabilities.is_empty() {
        return Err("capabilities must include at least one value".to_string());
    }
    let analyzer_capabilities = analyzer_capabilities.unwrap_or_else(|| capabilities.clone());
    if analyzer_capabilities.is_empty() {
        return Err("analyzer.capabilities must include at least one value".to_string());
    }
    analyzer.insert(
        "capabilities".to_string(),
        string_array_value(&analyzer_capabilities),
    );
    object.insert(
        "capabilities".to_string(),
        string_array_value(&capabilities),
    );

    let applies_to = match object.remove("applies_to") {
        Some(Value::Object(mut applies_to)) => {
            normalize_optional_string_array(&mut applies_to, "artifact_kinds")?;
            normalize_optional_string_array(&mut applies_to, "media_types")?;
            normalize_optional_string_array(&mut applies_to, "file_extensions")?;
            Value::Object(applies_to)
        }
        Some(_) => return Err("applies_to must be a JSON object".to_string()),
        None => json!({
            "media_types": supported_media_types,
            "file_extensions": file_extensions
        }),
    };
    object.insert("applies_to".to_string(), applies_to);

    let permissions = match object.remove("permissions") {
        Some(Value::Object(permissions)) => Value::Object(permissions),
        Some(_) => return Err("permissions must be a JSON object".to_string()),
        None => json!({}),
    };
    object.insert("permissions".to_string(), permissions);

    let mut viewer = required_object(&mut object, "viewer")?;
    move_alias(&mut viewer, "viewer_id", "viewerId");
    move_alias(&mut viewer, "schema_version", "schemaVersion");
    move_alias(&mut viewer, "preview_kinds", "previewKinds");
    move_alias(&mut viewer, "diff_kinds", "diffKinds");
    move_alias(&mut viewer, "reconcile_statuses", "reconcileStatuses");
    move_alias(&mut viewer, "inspector_fields", "inspectorFields");

    required_string(&mut viewer, "viewerId")?;
    let schema_version = required_string(&mut viewer, "schemaVersion")?;
    if schema_version != "layrs.viewer.v1" {
        return Err("viewer.schemaVersion must be layrs.viewer.v1".to_string());
    }
    required_string(&mut viewer, "component")?;
    let preview_kinds = required_string_array(&mut viewer, "previewKinds", false)?;
    let diff_kinds = required_string_array(&mut viewer, "diffKinds", false)?;
    if preview_kinds.is_empty() {
        return Err("viewer.previewKinds must include at least one value".to_string());
    }
    if diff_kinds.is_empty() {
        return Err("viewer.diffKinds must include at least one value".to_string());
    }
    if !viewer.contains_key("reconcileStatuses") {
        viewer.insert(
            "reconcileStatuses".to_string(),
            string_array_value(&["unsupported".to_string()]),
        );
    } else {
        required_string_array(&mut viewer, "reconcileStatuses", true)?;
    }
    match viewer.get("inspectorFields") {
        Some(Value::Array(_)) => {}
        Some(_) => return Err("viewer.inspectorFields must be an array".to_string()),
        None => {
            viewer.insert("inspectorFields".to_string(), Value::Array(Vec::new()));
        }
    }

    object.insert("analyzer".to_string(), Value::Object(analyzer));
    object.insert("viewer".to_string(), Value::Object(viewer));

    Ok(Value::Object(object))
}

fn registry_error(
    path: &Path,
    code: &'static str,
    message: impl Into<String>,
) -> LensManifestError {
    LensManifestError {
        path: path.display().to_string(),
        code,
        message: message.into(),
    }
}

fn move_alias(object: &mut Map<String, Value>, alias: &str, canonical: &str) {
    if !object.contains_key(canonical) {
        if let Some(value) = object.remove(alias) {
            object.insert(canonical.to_string(), value);
        }
    }
}

fn required_object(
    object: &mut Map<String, Value>,
    field: &'static str,
) -> Result<Map<String, Value>, String> {
    match object.remove(field) {
        Some(Value::Object(value)) => Ok(value),
        Some(_) => Err(format!("{field} must be a JSON object")),
        None => Err(format!("{field} is required")),
    }
}

fn required_string(object: &mut Map<String, Value>, field: &'static str) -> Result<String, String> {
    let value = object
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{field} is required"))?
        .to_string();
    object.insert(field.to_string(), Value::String(value.clone()));
    Ok(value)
}

fn required_string_array(
    object: &mut Map<String, Value>,
    field: &'static str,
    allow_empty: bool,
) -> Result<Vec<String>, String> {
    let values = object
        .get(field)
        .ok_or_else(|| format!("{field} is required"))
        .and_then(|value| string_array(value, field))?;
    if !allow_empty && values.is_empty() {
        return Err(format!("{field} must include at least one value"));
    }
    object.insert(field.to_string(), string_array_value(&values));
    Ok(values)
}

fn optional_string_array(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<Vec<String>>, String> {
    object
        .get(field)
        .map(|value| string_array(value, field))
        .transpose()
}

fn normalize_optional_string_array(
    object: &mut Map<String, Value>,
    field: &'static str,
) -> Result<(), String> {
    if let Some(values) = optional_string_array(object, field)? {
        object.insert(field.to_string(), string_array_value(&values));
    }
    Ok(())
}

fn string_array(value: &Value, field: &'static str) -> Result<Vec<String>, String> {
    let array = value
        .as_array()
        .ok_or_else(|| format!("{field} must be an array"))?;
    let mut values = Vec::with_capacity(array.len());
    for (index, item) in array.iter().enumerate() {
        let value = item
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{field}[{index}] must be a non-empty string"))?;
        values.push(value.to_string());
    }
    Ok(values)
}

fn string_array_value(values: &[String]) -> Value {
    Value::Array(
        values
            .iter()
            .map(|value| Value::String(value.clone()))
            .collect(),
    )
}

fn valid_lens_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn missing_lenses_dir_keeps_built_ins_without_errors() {
        let registry = load_lens_registry(unique_temp_dir("missing"));

        assert_eq!(lens_ids(&registry.items), built_in_ids());
        assert!(registry.errors.is_empty());
    }

    #[test]
    fn external_lenses_are_merged_after_built_ins() {
        let root = unique_temp_dir("merge");
        fs::create_dir_all(root.join("markdown")).expect("lens dir is created");
        fs::write(
            root.join("markdown").join("manifest.json"),
            r#"{
                "id": "com.example.markdown",
                "name": "Markdown Plus",
                "version": "1.2.3",
                "applies_to": {
                    "media_types": ["text/markdown"],
                    "file_extensions": ["mdx"]
                },
                "capabilities": ["view", "preview"],
                "permissions": {},
                "analyzer": {
                    "supportedMediaTypes": ["text/markdown"],
                    "fileExtensions": ["mdx"]
                },
                "viewer": {
                    "viewerId": "com.example.viewer.markdown",
                    "schemaVersion": "layrs.viewer.v1",
                    "component": "MarkdownPlusViewer",
                    "previewKinds": ["text"],
                    "diffKinds": ["textLines"]
                }
            }"#,
        )
        .expect("manifest is written");

        let registry = load_lens_registry(&root);
        let _ = fs::remove_dir_all(&root);

        let mut expected_ids = built_in_ids();
        expected_ids.push("com.example.markdown".to_string());
        assert_eq!(lens_ids(&registry.items), expected_ids);
        assert!(registry.errors.is_empty());

        let external = registry.items.last().expect("external manifest is present");
        assert_eq!(
            external
                .get("analyzer")
                .and_then(|value| value.get("capabilities"))
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        assert!(
            external
                .get("permissions")
                .and_then(Value::as_object)
                .is_some()
        );
    }

    #[test]
    fn invalid_external_manifests_are_non_fatal() {
        let root = unique_temp_dir("invalid");
        fs::create_dir_all(root.join("broken")).expect("lens dir is created");
        fs::write(
            root.join("broken").join("manifest.json"),
            r#"{"id": "bad lens"}"#,
        )
        .expect("manifest is written");

        let registry = load_lens_registry(&root);
        let _ = fs::remove_dir_all(&root);

        assert_eq!(lens_ids(&registry.items), built_in_ids());
        assert_eq!(registry.errors.len(), 1);
        assert_eq!(registry.errors[0].code, "manifest_invalid");
    }

    #[test]
    fn duplicate_external_ids_are_rejected_without_replacing_built_ins() {
        let root = unique_temp_dir("duplicate");
        fs::create_dir_all(root.join("code")).expect("lens dir is created");
        fs::write(
            root.join("code").join("manifest.json"),
            r#"{
                "id": "layrs.code",
                "name": "Shadow Code",
                "version": "1.0.0",
                "capabilities": ["view"],
                "permissions": {},
                "analyzer": {
                    "supportedMediaTypes": ["text/plain"],
                    "fileExtensions": ["txt"],
                    "capabilities": ["view"]
                },
                "viewer": {
                    "viewerId": "example.viewer.code",
                    "schemaVersion": "layrs.viewer.v1",
                    "component": "ShadowCodeViewer",
                    "previewKinds": ["text"],
                    "diffKinds": ["textLines"]
                }
            }"#,
        )
        .expect("manifest is written");

        let registry = load_lens_registry(&root);
        let _ = fs::remove_dir_all(&root);

        assert_eq!(lens_ids(&registry.items), built_in_ids());
        assert_eq!(registry.errors.len(), 1);
        assert_eq!(registry.errors[0].code, "duplicate_lens_id");
    }

    fn lens_ids(items: &[Value]) -> Vec<String> {
        items
            .iter()
            .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string))
            .collect()
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
}
