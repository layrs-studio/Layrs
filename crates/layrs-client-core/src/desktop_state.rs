use serde::{Deserialize, Serialize};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:8787";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopConfig {
    pub server_endpoint: String,
    pub device_id: String,
    #[serde(default)]
    pub auto_receive: bool,
    #[serde(default)]
    pub auto_publish: bool,
    #[serde(default = "default_auto_local_steps")]
    pub auto_local_steps: bool,
    #[serde(default = "default_sync_interval_seconds")]
    pub sync_interval_seconds: u64,
    #[serde(default = "default_local_spaces_folder")]
    pub default_local_spaces_folder: String,
    #[serde(default)]
    pub shortcuts: DesktopShortcutSettings,
    #[serde(default)]
    pub local_spaces: Vec<LocalSpaceConfigEntry>,
}

impl DesktopConfig {
    pub fn load_or_create() -> Result<Self, String> {
        let path = config_path()?;

        if path.exists() {
            let body = fs::read_to_string(&path).map_err(|error| {
                format!(
                    "Layrs Desktop could not read local non-secret config at {}: {error}",
                    path.display()
                )
            })?;

            let mut config: Self = serde_json::from_str(&body).map_err(|error| {
                format!(
                    "Layrs Desktop local config at {} is invalid: {error}",
                    path.display()
                )
            })?;

            if config.server_endpoint.trim().is_empty() {
                config.server_endpoint = DEFAULT_ENDPOINT.to_string();
            }
            if config.device_id.trim().is_empty() {
                config.device_id = generate_device_id();
            }
            config.normalize();

            config.save()?;
            return Ok(config);
        }

        let config = Self {
            server_endpoint: DEFAULT_ENDPOINT.to_string(),
            device_id: generate_device_id(),
            auto_receive: false,
            auto_publish: false,
            auto_local_steps: true,
            sync_interval_seconds: default_sync_interval_seconds(),
            default_local_spaces_folder: default_local_spaces_folder(),
            shortcuts: DesktopShortcutSettings::default(),
            local_spaces: Vec::new(),
        };
        config.save()?;
        Ok(config)
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Layrs Desktop could not create config directory {}: {error}",
                    parent.display()
                )
            })?;
        }

        let body = serde_json::to_string_pretty(self)
            .map_err(|error| format!("Layrs Desktop could not encode local config: {error}"))?;
        fs::write(&path, body).map_err(|error| {
            format!(
                "Layrs Desktop could not write local non-secret config at {}: {error}",
                path.display()
            )
        })
    }

    pub fn settings(&self) -> DesktopSettings {
        DesktopSettings {
            server_endpoint: self.server_endpoint.clone(),
            auto_receive: self.auto_receive,
            auto_publish: self.auto_publish,
            auto_local_steps: self.auto_local_steps,
            sync_interval_seconds: self.sync_interval_seconds,
            default_local_spaces_folder: self.default_local_spaces_folder.clone(),
            shortcuts: self.shortcuts.clone(),
        }
    }

    pub fn apply_settings(&mut self, settings: DesktopSettings) {
        self.server_endpoint = settings
            .server_endpoint
            .trim()
            .trim_end_matches('/')
            .to_string();
        self.auto_receive = settings.auto_receive;
        self.auto_publish = settings.auto_publish;
        self.auto_local_steps = settings.auto_local_steps;
        self.sync_interval_seconds = settings.sync_interval_seconds.max(30);
        self.default_local_spaces_folder = settings.default_local_spaces_folder;
        self.shortcuts = settings.shortcuts;
        self.normalize();
    }

    pub fn remember_local_space(&mut self, local_space: LocalSpaceConfigEntry) {
        self.local_spaces
            .retain(|entry| entry.local_space_id != local_space.local_space_id);
        self.local_spaces.push(local_space);
    }

    fn normalize(&mut self) {
        if self.server_endpoint.trim().is_empty() {
            self.server_endpoint = DEFAULT_ENDPOINT.to_string();
        } else {
            self.server_endpoint = self
                .server_endpoint
                .trim()
                .trim_end_matches('/')
                .to_string();
        }

        if self.device_id.trim().is_empty() {
            self.device_id = generate_device_id();
        }

        if self.sync_interval_seconds == 0 {
            self.sync_interval_seconds = default_sync_interval_seconds();
        }

        if self.default_local_spaces_folder.trim().is_empty() {
            self.default_local_spaces_folder = default_local_spaces_folder();
        }

        self.shortcuts.normalize();
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSettings {
    pub server_endpoint: String,
    pub auto_receive: bool,
    pub auto_publish: bool,
    pub auto_local_steps: bool,
    pub sync_interval_seconds: u64,
    pub default_local_spaces_folder: String,
    #[serde(default)]
    pub shortcuts: DesktopShortcutSettings,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopShortcutSettings {
    #[serde(default = "default_shortcuts_enabled")]
    pub enabled: bool,
    #[serde(default = "default_save_step_shortcut")]
    pub save_step: String,
    #[serde(default = "default_publish_shortcut")]
    pub publish: String,
    #[serde(default = "default_smart_save_publishes_pending_step")]
    pub smart_save_publishes_pending_step: bool,
}

impl Default for DesktopShortcutSettings {
    fn default() -> Self {
        Self {
            enabled: default_shortcuts_enabled(),
            save_step: default_save_step_shortcut(),
            publish: default_publish_shortcut(),
            smart_save_publishes_pending_step: default_smart_save_publishes_pending_step(),
        }
    }
}

impl DesktopShortcutSettings {
    fn normalize(&mut self) {
        if self.save_step.trim().is_empty() {
            self.save_step = default_save_step_shortcut();
        }
        if self.publish.trim().is_empty() {
            self.publish = default_publish_shortcut();
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSpaceConfigEntry {
    pub local_space_id: String,
    pub space_id: String,
    pub root_path: String,
    #[serde(default)]
    pub active_layer_id: Option<String>,
    pub updated_at_unix: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub email: String,
    #[serde(alias = "display_name")]
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceSummary {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    #[serde(default)]
    pub current_layer_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerSummary {
    pub id: String,
    pub workspace_id: String,
    pub space_id: String,
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub parent_layer_id: Option<String>,
    #[serde(default)]
    pub access: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapData {
    pub account: Option<Account>,
    pub workspaces: Vec<WorkspaceSummary>,
    pub spaces: Vec<SpaceSummary>,
    pub layers: Vec<LayerSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapDataWire {
    #[serde(default)]
    account: Option<Account>,
    #[serde(default)]
    user: Option<Account>,
    #[serde(default)]
    workspaces: Vec<WorkspaceSummary>,
    #[serde(default)]
    spaces: Vec<SpaceSummary>,
    #[serde(default)]
    layers: Vec<LayerSummary>,
}

impl<'de> Deserialize<'de> for BootstrapData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = BootstrapDataWire::deserialize(deserializer)?;
        Ok(Self {
            account: wire.account.or(wire.user),
            workspaces: wire.workspaces,
            spaces: wire.spaces,
            layers: wire.layers,
        })
    }
}

impl BootstrapData {
    pub fn has_account(&self) -> bool {
        self.account.is_some()
    }
}

pub fn validate_desktop_bootstrap(
    bootstrap: BootstrapData,
    source_path: &str,
) -> Result<BootstrapData, String> {
    if bootstrap.account.is_none() {
        return Err(format!(
            "Layrs Desktop rejected the response from {source_path}: desktop bootstrap is missing account identity."
        ));
    }
    Ok(bootstrap)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedBootstrap {
    pub cached_at_unix: u64,
    pub bootstrap: BootstrapData,
}

pub fn load_cached_bootstrap() -> Result<Option<BootstrapData>, String> {
    let path = cache_path()?;
    if !path.exists() {
        return Ok(None);
    }

    let body = fs::read_to_string(&path).map_err(|error| {
        format!(
            "Layrs Desktop could not read local non-secret bootstrap cache at {}: {error}",
            path.display()
        )
    })?;
    let cache: CachedBootstrap = serde_json::from_str(&body).map_err(|error| {
        format!(
            "Layrs Desktop bootstrap cache at {} is invalid: {error}",
            path.display()
        )
    })?;

    if !cache.bootstrap.has_account() {
        return Ok(None);
    }

    Ok(Some(cache.bootstrap))
}

pub fn save_cached_bootstrap(bootstrap: &BootstrapData) -> Result<(), String> {
    let path = cache_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Layrs Desktop could not create cache directory {}: {error}",
                parent.display()
            )
        })?;
    }

    let cache = CachedBootstrap {
        cached_at_unix: unix_now(),
        bootstrap: bootstrap.clone(),
    };
    let body = serde_json::to_string_pretty(&cache)
        .map_err(|error| format!("Layrs Desktop could not encode bootstrap cache: {error}"))?;
    fs::write(&path, body).map_err(|error| {
        format!(
            "Layrs Desktop could not write local non-secret bootstrap cache at {}: {error}",
            path.display()
        )
    })
}

pub fn workspace_root(input: Option<String>) -> Result<PathBuf, String> {
    match input {
        Some(value) if !value.trim().is_empty() => {
            let root = PathBuf::from(value.trim());
            absolute_path(&root)
        }
        _ => env::current_dir()
            .map_err(|error| {
                format!("Layrs Desktop could not resolve its current directory: {error}")
            })
            .and_then(|path| absolute_path(&path)),
    }
}

fn config_path() -> Result<PathBuf, String> {
    migrated_client_file("client.json", "desktop.json")
}

fn cache_path() -> Result<PathBuf, String> {
    migrated_client_file("bootstrap-cache.json", "bootstrap-cache.json")
}

fn migrated_client_file(file_name: &str, legacy_file_name: &str) -> Result<PathBuf, String> {
    let path = config_dir()?.join(file_name);
    if !path.exists() {
        if let Ok(legacy_path) = legacy_config_dir().map(|dir| dir.join(legacy_file_name)) {
            if legacy_path.exists() {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        format!(
                            "Layrs could not create client config directory {}: {error}",
                            parent.display()
                        )
                    })?;
                }
                fs::copy(&legacy_path, &path).map_err(|error| {
                    format!(
                        "Layrs could not migrate local config from {} to {}: {error}",
                        legacy_path.display(),
                        path.display()
                    )
                })?;
            }
        }
    }
    Ok(path)
}

fn config_dir() -> Result<PathBuf, String> {
    if let Ok(appdata) = env::var("APPDATA") {
        return Ok(PathBuf::from(appdata).join("Layrs").join("Client"));
    }

    if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("layrs").join("client"));
    }

    if let Ok(home) = env::var("HOME") {
        return Ok(PathBuf::from(home)
            .join(".config")
            .join("layrs")
            .join("client"));
    }

    Err("Layrs could not find a safe user config directory.".to_string())
}

fn legacy_config_dir() -> Result<PathBuf, String> {
    if let Ok(appdata) = env::var("APPDATA") {
        return Ok(PathBuf::from(appdata).join("Layrs").join("Studio Desktop"));
    }

    if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("layrs").join("studio-desktop"));
    }

    if let Ok(home) = env::var("HOME") {
        return Ok(PathBuf::from(home)
            .join(".config")
            .join("layrs")
            .join("studio-desktop"));
    }

    Err("Layrs could not find a safe legacy Desktop config directory.".to_string())
}

fn default_auto_local_steps() -> bool {
    true
}

fn default_shortcuts_enabled() -> bool {
    true
}

fn default_save_step_shortcut() -> String {
    "Ctrl+S".to_string()
}

fn default_publish_shortcut() -> String {
    "Ctrl+P".to_string()
}

fn default_smart_save_publishes_pending_step() -> bool {
    true
}

fn default_sync_interval_seconds() -> u64 {
    300
}

fn default_local_spaces_folder() -> String {
    if let Ok(profile) = env::var("USERPROFILE") {
        return PathBuf::from(profile)
            .join("Documents")
            .join("Layrs Spaces")
            .display()
            .to_string();
    }

    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home)
            .join("Layrs Spaces")
            .display()
            .to_string();
    }

    "Layrs Spaces".to_string()
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if path.is_absolute() {
                Ok(path.to_path_buf())
            } else {
                env::current_dir()
                    .map(|cwd| cwd.join(path))
                    .map_err(|cwd_error| {
                        format!(
                            "Layrs Desktop could not resolve local workspace root {}: {cwd_error}",
                            path.display()
                        )
                    })
            }
        }
        Err(error) => Err(format!(
            "Layrs Desktop could not resolve local workspace root {}: {error}",
            path.display()
        )),
    }
}

fn generate_device_id() -> String {
    let now = unix_now();
    let pid = std::process::id();
    format!("layrs-desktop-{now:x}-{pid:x}")
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_bootstrap_requires_account_identity() {
        let error = validate_desktop_bootstrap(BootstrapData::default(), "/v1/desktop/bootstrap")
            .unwrap_err();

        assert!(error.contains("missing account identity"));
    }

    #[test]
    fn desktop_bootstrap_accepts_empty_workspace_list_with_account() {
        let bootstrap = BootstrapData {
            account: Some(Account {
                id: "account_1".to_string(),
                email: "alex@example.test".to_string(),
                display_name: "Alex".to_string(),
            }),
            workspaces: Vec::new(),
            spaces: Vec::new(),
            layers: Vec::new(),
        };

        let validated = validate_desktop_bootstrap(bootstrap, "/v1/desktop/bootstrap").unwrap();

        assert_eq!(
            validated
                .account
                .as_ref()
                .map(|account| account.email.as_str()),
            Some("alex@example.test")
        );
    }

    #[test]
    fn desktop_bootstrap_decodes_account_and_legacy_user_together() {
        let bootstrap: BootstrapData = serde_json::from_str(
            r#"{
                "account": {
                    "id": "account_primary",
                    "email": "primary@example.test",
                    "displayName": "Primary"
                },
                "user": {
                    "id": "account_legacy",
                    "email": "legacy@example.test",
                    "displayName": "Legacy"
                },
                "workspaces": [
                    { "id": "workspace_1", "name": "Demo", "slug": "demo" }
                ],
                "spaces": [],
                "layers": []
            }"#,
        )
        .unwrap();

        assert_eq!(
            bootstrap
                .account
                .as_ref()
                .map(|account| account.email.as_str()),
            Some("primary@example.test")
        );
        assert_eq!(bootstrap.workspaces.len(), 1);
    }

    #[test]
    fn desktop_bootstrap_decodes_legacy_user_without_account() {
        let bootstrap: BootstrapData = serde_json::from_str(
            r#"{
                "user": {
                    "id": "account_legacy",
                    "email": "legacy@example.test",
                    "displayName": "Legacy"
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            bootstrap
                .account
                .as_ref()
                .map(|account| account.email.as_str()),
            Some("legacy@example.test")
        );
    }
}
