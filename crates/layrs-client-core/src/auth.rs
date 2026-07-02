use crate::{
    access_registry::AccessRegistryResult,
    desktop_state::{
        load_cached_bootstrap, save_cached_bootstrap, validate_desktop_bootstrap, Account,
        BootstrapData, DesktopConfig,
    },
    http_client::{get_json, post_json},
    secret_store::{OsSecretStore, SecretStore, SecretStoreStatus},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopStatus {
    pub server_endpoint: String,
    pub device_id: String,
    pub secret_store: SecretStoreStatusView,
    pub connected: bool,
    pub cached_bootstrap: Option<BootstrapData>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretStoreStatusView {
    pub available: bool,
    pub provider: String,
    pub message: String,
}

impl From<SecretStoreStatus> for SecretStoreStatusView {
    fn from(value: SecretStoreStatus) -> Self {
        Self {
            available: value.available,
            provider: value.provider.to_string(),
            message: value.message,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigureEndpointRequest {
    pub server_endpoint: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLoginStartRequest {
    pub client: String,
    pub device_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLoginStartResponse {
    #[serde(alias = "device_code")]
    pub device_code: String,
    #[serde(alias = "user_code")]
    pub user_code: String,
    #[serde(alias = "verification_uri")]
    pub verification_uri: String,
    #[serde(default)]
    #[serde(alias = "verification_uri_complete")]
    pub verification_uri_complete: Option<String>,
    #[serde(default = "default_expires_in")]
    #[serde(alias = "expires_in_seconds")]
    pub expires_in: u64,
    #[serde(default = "default_poll_interval")]
    #[serde(alias = "interval_seconds")]
    pub interval: u64,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLoginPollRequest {
    pub device_code: String,
    pub device_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLoginPollServerResponse {
    pub status: String,
    #[serde(default)]
    #[serde(alias = "access_token")]
    pub access_token: Option<String>,
    #[serde(default)]
    #[serde(alias = "desktop_token")]
    pub desktop_token: Option<DesktopTokenPayload>,
    #[serde(default)]
    pub account: Option<Account>,
    #[serde(default)]
    pub user: Option<Account>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopTokenPayload {
    #[serde(alias = "access_token")]
    pub access_token: String,
    #[serde(default)]
    #[serde(alias = "token_type")]
    pub token_type: Option<String>,
    #[serde(default)]
    #[serde(alias = "expires_in_seconds")]
    pub expires_in_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLoginPollResponse {
    pub status: String,
    pub message: Option<String>,
    pub account: Option<Account>,
    pub bootstrap: Option<BootstrapData>,
    pub access_registry: Option<AccessRegistryResult>,
}

pub fn desktop_status() -> Result<DesktopStatus, String> {
    let config = DesktopConfig::load_or_create()?;
    let store = OsSecretStore::new();
    let secret_store = store.status();
    let token = if secret_store.available {
        store
            .get_token(&config.device_id)
            .map_err(|error| format!("Layrs Desktop could not inspect OS secret store: {error}"))?
    } else {
        None
    };
    let connected = token.is_some();
    let mut cached_bootstrap = load_cached_bootstrap()?;

    if connected && cached_bootstrap.is_none() {
        if let Some(token) = token.as_deref() {
            if let Ok(bootstrap) = load_bootstrap_with_token(&config.server_endpoint, token) {
                save_cached_bootstrap(&bootstrap)?;
                cached_bootstrap = Some(bootstrap);
            }
        }
    }

    Ok(DesktopStatus {
        server_endpoint: config.server_endpoint,
        device_id: config.device_id,
        secret_store: secret_store.into(),
        connected,
        cached_bootstrap,
    })
}

pub fn configure_endpoint(server_endpoint: String) -> Result<DesktopStatus, String> {
    let mut config = DesktopConfig::load_or_create()?;
    let endpoint = server_endpoint.trim().trim_end_matches('/').to_string();
    if !endpoint.starts_with("http://") {
        return Err(
            "Layrs Desktop currently accepts only http:// server endpoints for local development."
                .to_string(),
        );
    }
    config.server_endpoint = endpoint;
    config.save()?;
    desktop_status()
}

pub fn start_device_login() -> Result<DeviceLoginStartResponse, String> {
    let config = DesktopConfig::load_or_create()?;
    require_secret_store()?;

    post_json(
        &config.server_endpoint,
        "/v1/desktop/device/start",
        None,
        &DeviceLoginStartRequest {
            client: "layrs-studio-desktop".to_string(),
            device_id: config.device_id,
        },
    )
}

pub fn poll_device_login(
    device_code: String,
    _workspace_root: Option<String>,
) -> Result<DeviceLoginPollResponse, String> {
    let config = DesktopConfig::load_or_create()?;
    let store = require_secret_store()?;
    let response: DeviceLoginPollServerResponse = post_json(
        &config.server_endpoint,
        "/v1/desktop/device/poll",
        None,
        &DeviceLoginPollRequest {
            device_code,
            device_id: config.device_id.clone(),
        },
    )?;

    match response.status.as_str() {
        "approved" | "connected" | "authorized" => {
            let token = response
                .access_token
                .as_deref()
                .or_else(|| {
                    response
                        .desktop_token
                        .as_ref()
                        .map(|token| token.access_token.as_str())
                })
                .ok_or_else(|| {
                    "Layrs server approved the device login without returning an access token."
                        .to_string()
                })?;
            let account = response.account.clone().or(response.user.clone());
            store
                .set_token(&config.device_id, token)
                .map_err(|error| format!("Layrs Desktop refused the login: {error}"))?;
            let bootstrap = load_bootstrap_with_token(&config.server_endpoint, token)
                .unwrap_or_else(|_| BootstrapData {
                    account: account.clone(),
                    ..BootstrapData::default()
                });
            save_cached_bootstrap(&bootstrap)?;
            Ok(DeviceLoginPollResponse {
                status: "connected".to_string(),
                message: response.message,
                account: bootstrap.account.clone().or(account),
                bootstrap: Some(bootstrap),
                access_registry: None,
            })
        }
        "authorization_pending" | "pending" | "slow_down" => Ok(DeviceLoginPollResponse {
            status: response.status,
            message: response.message,
            account: response.account.or(response.user),
            bootstrap: None,
            access_registry: None,
        }),
        "denied" | "expired" => Ok(DeviceLoginPollResponse {
            status: response.status,
            message: response.message,
            account: response.account.or(response.user),
            bootstrap: None,
            access_registry: None,
        }),
        other => Err(format!(
            "Layrs server returned an unknown device login status: {other}"
        )),
    }
}

pub fn refresh_bootstrap(
    _workspace_root: Option<String>,
) -> Result<DeviceLoginPollResponse, String> {
    let config = DesktopConfig::load_or_create()?;
    let store = require_secret_store()?;
    let token = store
        .get_token(&config.device_id)
        .map_err(|error| format!("Layrs Desktop could not read OS secret store: {error}"))?
        .ok_or_else(|| {
            "Layrs Desktop is not connected. Connect before loading workspaces.".to_string()
        })?;
    let bootstrap = load_bootstrap_with_token(&config.server_endpoint, &token)?;
    save_cached_bootstrap(&bootstrap)?;
    Ok(DeviceLoginPollResponse {
        status: "connected".to_string(),
        message: Some("Distant Spaces refreshed from Layrs server.".to_string()),
        account: bootstrap.account.clone(),
        bootstrap: Some(bootstrap),
        access_registry: None,
    })
}

fn load_bootstrap_with_token(endpoint: &str, token: &str) -> Result<BootstrapData, String> {
    let bootstrap = get_json(endpoint, "/v1/desktop/bootstrap", Some(token))?;
    validate_desktop_bootstrap(bootstrap, "/v1/desktop/bootstrap")
}

fn require_secret_store() -> Result<OsSecretStore, String> {
    let store = OsSecretStore::new();
    let status = store.status();
    if !status.available {
        return Err(status.message);
    }
    Ok(store)
}

fn default_expires_in() -> u64 {
    900
}

fn default_poll_interval() -> u64 {
    5
}
