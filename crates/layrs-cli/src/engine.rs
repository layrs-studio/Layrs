use crate::args::Window;
use layrs_client_core::{
    access_registry as core_space, auth as core_auth,
    desktop_state::DesktopConfig,
    secret_store::{OsSecretStore, SecretStore},
};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

mod weaves;

pub use weaves::{
    ConflictInteractiveOutput, WeaveConflictBlockOutput, WeaveConflictOutput, WeaveOutput,
    WeaveSessionOutput,
};

#[derive(Debug, Clone)]
pub struct EngineContext {
    pub space: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ClientCoreEngine {
    context: EngineContext,
}

impl ClientCoreEngine {
    pub fn new(context: EngineContext) -> Self {
        Self { context }
    }

    pub fn init_local_space(
        &self,
        name: &str,
        path: Option<&Path>,
    ) -> Result<InitLocalSpace, CliError> {
        let root = match path
            .map(Path::to_path_buf)
            .or_else(|| self.context.space.clone())
        {
            Some(path) => path_string(path),
            None => current_dir_string()?,
        };
        let result = map_core(core_space::init_local_space(name.to_string(), root))?;
        Ok(InitLocalSpace {
            space_id: result.local_space.space_id,
            local_space_id: result.local_space.local_space_id,
            name: result.local_space.name,
            path: result.local_space.root_path,
            active_layer_id: result
                .local_space
                .active_layer_id
                .unwrap_or_else(|| "unknown".to_string()),
            initial_step_id: result.initial_step_id,
            scanned_files: result.scanned_files as u32,
            pending_publish_count: result.pending_publish_count as u32,
        })
    }

    pub fn save_local_step(&self) -> Result<StepSaved, CliError> {
        let result = map_core(core_space::save_local_step(self.space_selector()?))?;
        Ok(StepSaved {
            status: result.status,
            message: result.message,
            step_id: result.step_id,
            layer_id: result
                .local_space
                .active_layer_id
                .unwrap_or_else(|| "unknown".to_string()),
            changed_files: result.changed_files as u32,
            additions: result.diff_stats.additions as u32,
            deletions: result.diff_stats.deletions as u32,
            pending_publish_count: result.pending_publish_count as u32,
        })
    }

    pub fn diff(&self, request: DiffRequest<'_>) -> Result<DiffOutput, CliError> {
        let scan = map_core(core_space::scan_working_tree(self.space_selector()?))?;
        let (source, message, step_id, entries) = if let Some(step_id) = request.step_id {
            let step = scan
                .steps
                .iter()
                .find(|step| step.step_id == step_id)
                .ok_or_else(|| {
                    CliError::runtime(format!("Layrs could not find local Step {step_id}."))
                })?;
            (
                DiffSource::Step,
                format!("Displaying Step {step_id}."),
                Some(step_id.to_string()),
                step.diffs.as_slice(),
            )
        } else if scan.diffs.is_empty() && scan.pending_publish_count > 0 {
            if let Some(step) = scan.steps.last() {
                (
                    DiffSource::LatestPendingStep,
                    format!(
                        "No working tree changes; displaying latest pending Step {}.",
                        step.step_id
                    ),
                    Some(step.step_id.clone()),
                    step.diffs.as_slice(),
                )
            } else {
                (
                    DiffSource::WorkingTree,
                    "No working tree changes.".to_string(),
                    None,
                    [].as_slice(),
                )
            }
        } else {
            let message = if scan.diffs.is_empty() {
                "No working tree changes.".to_string()
            } else {
                "Displaying working tree changes.".to_string()
            };
            (
                DiffSource::WorkingTree,
                message,
                None,
                scan.diffs.as_slice(),
            )
        };

        let files = entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let stats = entries
            .iter()
            .map(|entry| DiffStat {
                path: entry.path.clone(),
                additions: number_field(&entry.diff.fields, "additions"),
                deletions: number_field(&entry.diff.fields, "deletions"),
            })
            .collect::<Vec<_>>();
        let truncated = entries.iter().any(|entry| {
            entry
                .diff
                .fields
                .get("hasMore")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || entry
                    .diff
                    .fields
                    .get("truncated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        });
        let text = render_core_diff(entries, &request, &stats);

        Ok(DiffOutput {
            source,
            message,
            step_id,
            text,
            files,
            stats,
            truncated,
        })
    }

    pub fn timeline(&self, limit: Option<u32>) -> Result<TimelineOutput, CliError> {
        let scan = map_core(core_space::scan_working_tree(self.space_selector()?))?;
        let max = limit.unwrap_or(20) as usize;
        let steps = scan
            .steps
            .into_iter()
            .rev()
            .take(max)
            .map(|step| TimelineStep {
                step_id: step.step_id,
                layer_id: step.layer_id,
                captured_at_unix: step.captured_at,
                timeline_position: step.timeline_position,
                origin_layer_id: step.origin_layer_id,
                origin_layer_name: step.origin_layer_name,
                origin_step_id: step.origin_step_id,
                step_kind: step.step_kind,
                summary: format!(
                    "{} file(s), +{} -{}",
                    step.diff_stats.files, step.diff_stats.additions, step.diff_stats.deletions
                ),
            })
            .collect();
        Ok(TimelineOutput { steps })
    }

    pub fn publish(&self, workspace: Option<&str>) -> Result<PublishOutput, CliError> {
        let selector = self.space_selector()?;
        let space = map_core(core_space::open_local_space(selector.clone()))?;
        if space.state == "draft" {
            let workspace_id = match workspace {
                Some(workspace_id) => workspace_id.to_string(),
                None => self.default_workspace_for_draft_publish()?,
            };
            let result = map_core(core_space::send_draft_local_space(selector, workspace_id))?;
            return Ok(PublishOutput {
                workspace_id: result.workspace_id,
                status: "published".to_string(),
                message: format!(
                    "Sent Draft Space {} to Studio.",
                    result.local_space.local_space_id
                ),
                pushed_objects: 0,
                pushed_steps: result.published_layers as u32,
                sync_state_path: None,
            });
        }

        let result = map_core(core_space::publish_local_space(selector))?;
        Ok(PublishOutput {
            workspace_id: result.local_space.workspace_id,
            status: result.status,
            message: result.message,
            pushed_objects: 0,
            pushed_steps: 0,
            sync_state_path: Some(result.sync_state_path),
        })
    }

    pub fn receive(&self) -> Result<ReceiveOutput, CliError> {
        let result = map_core(core_space::receive_local_space(self.space_selector()?))?;
        Ok(ReceiveOutput {
            status: result.status,
            message: result.message,
            pulled_objects: 0,
            pulled_steps: 0,
            sync_state_path: result.sync_state_path,
        })
    }

    pub fn sync(&self, workspace: Option<&str>) -> Result<SyncOutput, CliError> {
        let selector = self.space_selector()?;
        let space = map_core(core_space::open_local_space(selector.clone()))?;
        if space.state == "draft" {
            let workspace_id = match workspace {
                Some(workspace_id) => workspace_id.to_string(),
                None => self.default_workspace_for_draft_publish()?,
            };
            let result = map_core(core_space::send_draft_local_space(selector, workspace_id))?;
            return Ok(SyncOutput {
                workspace_id: result.workspace_id,
                status: "synced".to_string(),
                message: format!(
                    "Sent Draft Space {} to Studio.",
                    result.local_space.local_space_id
                ),
                sync_state_path: None,
            });
        }

        let result = map_core(core_space::sync_local_space(selector))?;
        Ok(SyncOutput {
            workspace_id: result.local_space.workspace_id,
            status: result.status,
            message: result.message,
            sync_state_path: Some(result.sync_state_path),
        })
    }

    pub fn compact(&self) -> Result<CompactOutput, CliError> {
        let result = map_core(core_space::compact_local_space(self.space_selector()?))?;
        Ok(CompactOutput {
            local_space_id: result.local_space.local_space_id,
            path: result.local_space.root_path,
            packed_chunks: result.packed_chunks as u32,
            loose_chunks_removed: result.loose_chunks_removed as u32,
            raw_bytes: result.raw_bytes,
            stored_bytes: result.stored_bytes,
            pack_path: result.pack_path,
        })
    }

    pub fn status(&self) -> Result<StatusOutput, CliError> {
        let scan = map_core(core_space::scan_working_tree(self.space_selector()?))?;
        Ok(StatusOutput {
            path: scan.root_path,
            active_layer_id: scan.active_layer_id,
            changed: scan.changed,
            added_files: scan.added.len() as u32,
            modified_files: scan.modified.len() as u32,
            deleted_files: scan.deleted.len() as u32,
            pending_steps: scan.pending_publish_count as u32,
        })
    }

    pub fn login(&self, endpoint: Option<&str>) -> Result<LoginOutput, CliError> {
        if let Some(endpoint) = endpoint {
            map_core(core_auth::configure_endpoint(endpoint.to_string()))?;
        }
        let status = map_core(core_auth::desktop_status())?;
        let start = map_core(core_auth::start_device_login())?;
        eprintln!(
            "Open this URL to authorize Layrs CLI:\n{}\n\ncode: {}",
            start
                .verification_uri_complete
                .as_deref()
                .unwrap_or(start.verification_uri.as_str()),
            start.user_code
        );
        let deadline = Instant::now() + Duration::from_secs(start.expires_in);
        loop {
            let poll = map_core(core_auth::poll_device_login(
                start.device_code.clone(),
                self.context
                    .space
                    .as_ref()
                    .map(|path| path.display().to_string()),
            ))?;
            match poll.status.as_str() {
                "connected" => {
                    return Ok(LoginOutput {
                        endpoint: status.server_endpoint,
                        status: poll.status,
                        account_id: poll.account.as_ref().map(|account| account.id.clone()),
                        email: poll.account.as_ref().map(|account| account.email.clone()),
                        device_code: start.device_code,
                        user_code: start.user_code,
                        verification_uri: start.verification_uri,
                        verification_uri_complete: start.verification_uri_complete,
                        expires_in: start.expires_in,
                        interval: start.interval,
                        message: poll.message.or(start.message),
                    });
                }
                "pending" | "authorization_pending" | "slow_down" => {}
                "denied" | "expired" => {
                    return Err(CliError::auth_required(format!(
                        "Layrs device login {}.",
                        poll.status
                    )));
                }
                other => {
                    return Err(CliError::runtime(format!(
                        "Layrs server returned unexpected device login status `{other}`."
                    )));
                }
            }

            if Instant::now() >= deadline {
                return Err(CliError::auth_required(
                    "Layrs device login expired before approval.",
                ));
            }
            std::thread::sleep(Duration::from_secs(start.interval.max(1)));
        }
    }

    pub fn whoami(&self) -> Result<WhoamiOutput, CliError> {
        let status = map_core(core_auth::desktop_status())?;
        if !status.connected {
            return Err(CliError::auth_required(
                "Layrs is not logged in. Run `layrs login` first.",
            ));
        }
        let account = status
            .cached_bootstrap
            .and_then(|bootstrap| bootstrap.account)
            .ok_or_else(|| {
                CliError::runtime("Layrs is connected, but no cached account profile is available.")
            })?;
        Ok(WhoamiOutput {
            endpoint: status.server_endpoint,
            account_id: account.id,
            email: account.email,
            display_name: account.display_name,
        })
    }

    pub fn logout(&self) -> Result<LogoutOutput, CliError> {
        let config = map_core(DesktopConfig::load_or_create())?;
        let store = OsSecretStore::new();
        map_core(
            store
                .delete_token(&config.device_id)
                .map_err(|error| error.to_string()),
        )?;
        Ok(LogoutOutput {
            endpoint: Some(config.server_endpoint),
            logged_out: true,
        })
    }

    pub fn spaces(&self) -> Result<SpacesOutput, CliError> {
        let active = self.context.space.as_ref().map(path_key);
        let spaces = map_core(core_space::list_available_spaces())?
            .into_iter()
            .map(|space| {
                let local_path = space
                    .local_spaces
                    .first()
                    .map(|local_space| local_space.root_path.clone());
                let is_active = active.as_deref().is_some_and(|active| {
                    local_path
                        .as_deref()
                        .is_some_and(|path| path_key_string(path) == active)
                });
                SpaceOutput {
                    space_id: space.space_id,
                    local_space_id: space
                        .local_spaces
                        .first()
                        .map(|local_space| local_space.local_space_id.clone()),
                    name: space.name,
                    path: local_path,
                    active: is_active,
                }
            })
            .collect();
        Ok(SpacesOutput { spaces })
    }

    pub fn layers(&self) -> Result<LayersOutput, CliError> {
        let space = map_core(core_space::open_local_space(self.space_selector()?))?;
        let active = space.active_layer_id.as_deref();
        Ok(LayersOutput {
            layers: space
                .layers
                .into_iter()
                .map(|layer| layer_output(layer, active))
                .collect(),
        })
    }

    pub fn layer_use(&self, name_or_id: &str) -> Result<LayerOutput, CliError> {
        let space = map_core(core_space::open_local_space(self.space_selector()?))?;
        let target = resolve_layer_id(&space.layers, name_or_id)?;
        let result = map_core(core_space::switch_layer(space.local_space_id, target))?;
        let active = Some(result.active_layer_id.as_str());
        result
            .local_space
            .layers
            .into_iter()
            .find(|layer| layer.layer_id == result.active_layer_id)
            .map(|layer| layer_output(layer, active))
            .ok_or_else(|| CliError::runtime("Layrs switched Layer but did not return it."))
    }

    pub fn layer_create(&self, name: &str) -> Result<LayerOutput, CliError> {
        let result = map_core(core_space::create_layer_from_current(
            self.space_selector()?,
            name.to_string(),
        ))?;
        let active = Some(result.active_layer_id.as_str());
        result
            .local_space
            .layers
            .into_iter()
            .find(|layer| layer.layer_id == result.active_layer_id)
            .map(|layer| layer_output(layer, active))
            .ok_or_else(|| CliError::runtime("Layrs created Layer but did not return it."))
    }

    pub fn layer_delete(&self, name_or_id: &str, yes: bool) -> Result<LayerDeleted, CliError> {
        if !yes {
            return Err(CliError::runtime(
                "Refusing to delete a Layer without --yes.",
            ));
        }
        let space = map_core(core_space::open_local_space(self.space_selector()?))?;
        let target = resolve_layer_id(&space.layers, name_or_id)?;
        let result = map_core(core_space::delete_layer(space.local_space_id, target))?;
        Ok(LayerDeleted {
            layer_id: result.deleted_layer_id,
            name: name_or_id.to_string(),
            deleted: true,
            message: result.message,
        })
    }

    pub fn layer_disconnect(
        &self,
        name_or_id: &str,
        yes: bool,
    ) -> Result<LayerActionOutput, CliError> {
        if !yes {
            return Err(CliError::runtime(
                "Refusing to disconnect a Layer from its parent without --yes.",
            ));
        }
        let space = map_core(core_space::open_local_space(self.space_selector()?))?;
        let target = resolve_layer_id(&space.layers, name_or_id)?;
        let result = map_core(core_space::disconnect_layer_from_parent(
            space.local_space_id,
            target,
        ))?;
        Ok(LayerActionOutput {
            layer_id: result.layer_id,
            name: name_or_id.to_string(),
            message: result.message,
            archived_steps_path: result.archived_steps_path,
        })
    }

    pub fn layer_clear_steps(
        &self,
        name_or_id: &str,
        yes: bool,
    ) -> Result<LayerActionOutput, CliError> {
        if !yes {
            return Err(CliError::runtime(
                "Refusing to clear Layer Steps without --yes.",
            ));
        }
        let space = map_core(core_space::open_local_space(self.space_selector()?))?;
        let target = resolve_layer_id(&space.layers, name_or_id)?;
        let result = map_core(core_space::clear_layer_steps(
            space.local_space_id,
            target,
            true,
        ))?;
        Ok(LayerActionOutput {
            layer_id: result.layer_id,
            name: name_or_id.to_string(),
            message: result.message,
            archived_steps_path: result.archived_steps_path,
        })
    }

    fn space_selector(&self) -> Result<String, CliError> {
        match self.context.space.clone() {
            Some(path) => Ok(path_string(path)),
            None => discover_current_space(),
        }
    }

    fn default_workspace_for_draft_publish(&self) -> Result<String, CliError> {
        let bootstrap = map_core(core_auth::refresh_bootstrap(None))?
            .bootstrap
            .ok_or_else(|| {
                CliError::auth_required("Layrs could not load Workspaces. Run `layrs login` first.")
            })?;
        match bootstrap.workspaces.as_slice() {
            [workspace] => Ok(workspace.id.clone()),
            [] => Err(CliError::runtime(
                "No Workspace is available for this account.",
            )),
            workspaces => Err(CliError::runtime(format!(
                "Draft publish needs --workspace because {} Workspaces are available: {}",
                workspaces.len(),
                workspaces
                    .iter()
                    .map(|workspace| format!("{} ({})", workspace.name, workspace.id))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success = 0,
    Runtime = 1,
    Usage = 2,
    AuthRequired = 3,
}

impl ExitCode {
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub message: String,
    pub exit_code: ExitCode,
}

impl CliError {
    pub fn runtime(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit_code: ExitCode::Runtime,
        }
    }

    pub fn auth_required(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit_code: ExitCode::AuthRequired,
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for CliError {}

impl From<io::Error> for CliError {
    fn from(error: io::Error) -> Self {
        CliError::runtime(format!("Layrs CLI I/O failed: {error}"))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InitLocalSpace {
    pub space_id: String,
    pub local_space_id: String,
    pub name: String,
    pub path: String,
    pub active_layer_id: String,
    pub initial_step_id: Option<String>,
    pub scanned_files: u32,
    pub pending_publish_count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepSaved {
    pub status: String,
    pub message: String,
    pub step_id: Option<String>,
    pub layer_id: String,
    pub changed_files: u32,
    pub additions: u32,
    pub deletions: u32,
    pub pending_publish_count: u32,
}

#[derive(Debug, Clone)]
pub struct DiffRequest<'a> {
    pub step_id: Option<&'a str>,
    pub stat: bool,
    pub name_only: bool,
    pub window: Option<&'a Window>,
    pub wrap: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffOutput {
    pub source: DiffSource,
    pub message: String,
    pub step_id: Option<String>,
    pub text: String,
    pub files: Vec<String>,
    pub stats: Vec<DiffStat>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DiffSource {
    #[serde(rename = "workingTree")]
    WorkingTree,
    #[serde(rename = "latestPendingStep")]
    LatestPendingStep,
    #[serde(rename = "step")]
    Step,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffStat {
    pub path: String,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimelineOutput {
    pub steps: Vec<TimelineStep>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimelineStep {
    pub step_id: String,
    pub layer_id: String,
    pub captured_at_unix: u64,
    pub timeline_position: Option<u64>,
    pub origin_layer_id: Option<String>,
    pub origin_layer_name: Option<String>,
    pub origin_step_id: Option<String>,
    pub step_kind: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublishOutput {
    pub workspace_id: String,
    pub status: String,
    pub message: String,
    pub pushed_objects: u32,
    pub pushed_steps: u32,
    pub sync_state_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReceiveOutput {
    pub status: String,
    pub message: String,
    pub pulled_objects: u32,
    pub pulled_steps: u32,
    pub sync_state_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncOutput {
    pub workspace_id: String,
    pub status: String,
    pub message: String,
    pub sync_state_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompactOutput {
    pub local_space_id: String,
    pub path: String,
    pub packed_chunks: u32,
    pub loose_chunks_removed: u32,
    pub raw_bytes: u64,
    pub stored_bytes: u64,
    pub pack_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusOutput {
    pub path: String,
    pub active_layer_id: String,
    pub changed: bool,
    pub added_files: u32,
    pub modified_files: u32,
    pub deleted_files: u32,
    pub pending_steps: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoginOutput {
    pub endpoint: String,
    pub status: String,
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WhoamiOutput {
    pub endpoint: String,
    pub account_id: String,
    pub email: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogoutOutput {
    pub endpoint: Option<String>,
    pub logged_out: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpacesOutput {
    pub spaces: Vec<SpaceOutput>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpaceOutput {
    pub space_id: String,
    pub local_space_id: Option<String>,
    pub name: String,
    pub path: Option<String>,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LayersOutput {
    pub layers: Vec<LayerOutput>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LayerOutput {
    pub layer_id: String,
    pub name: String,
    pub active: bool,
    pub parent_layer_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LayerDeleted {
    pub layer_id: String,
    pub name: String,
    pub deleted: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LayerActionOutput {
    pub layer_id: String,
    pub name: String,
    pub message: String,
    pub archived_steps_path: Option<String>,
}

fn map_core<T>(result: Result<T, String>) -> Result<T, CliError> {
    result.map_err(|message| {
        if is_auth_required(&message) {
            CliError::auth_required(message)
        } else {
            CliError::runtime(message)
        }
    })
}

fn is_auth_required(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("not connected")
        || lower.contains("connect a device")
        || lower.contains("not logged in")
        || lower.contains("secret store")
}

fn render_core_diff(
    entries: &[core_space::LensDiffEntry],
    request: &DiffRequest<'_>,
    stats: &[DiffStat],
) -> String {
    if request.name_only {
        return entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>()
            .join("\n");
    }

    if request.stat {
        return stats
            .iter()
            .map(|stat| format!("{} | +{} -{}", stat.path, stat.additions, stat.deletions))
            .collect::<Vec<_>>()
            .join("\n");
    }

    let mut rendered = String::new();
    for entry in entries {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str(&format!("diff --layrs {}\n", entry.path));
        rendered.push_str(&format!("--- {}\n", entry.path));
        rendered.push_str(&format!("+++ {}\n", entry.path));
        for hunk in &entry.diff.hunks {
            rendered.push_str(&format!(
                "@@ -{},{} +{},{} @@\n",
                hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
            ));
            let mut lines = hunk.lines.iter().collect::<Vec<_>>();
            if let Some(window) = request.window {
                lines = lines
                    .into_iter()
                    .skip(window.start as usize)
                    .take(window.limit as usize)
                    .collect();
            }
            for line in lines {
                let prefix = match line.op.as_str() {
                    "insert" => '+',
                    "delete" => '-',
                    _ => ' ',
                };
                rendered.push(prefix);
                rendered.push_str(&line.text);
                rendered.push('\n');
            }
        }
    }

    rendered
}

fn number_field(fields: &BTreeMap<String, Value>, key: &str) -> u32 {
    fields
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0)
}

fn resolve_layer_id(
    layers: &[core_space::LocalLayerSummary],
    name_or_id: &str,
) -> Result<String, CliError> {
    if let Some(layer) = layers.iter().find(|layer| layer.layer_id == name_or_id) {
        return Ok(layer.layer_id.clone());
    }

    let matches = layers
        .iter()
        .filter(|layer| layer.display_name == name_or_id)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [layer] => Ok(layer.layer_id.clone()),
        [] => Err(CliError::runtime(format!(
            "Layrs could not find Layer `{name_or_id}`."
        ))),
        _ => Err(CliError::runtime(format!(
            "Layer name `{name_or_id}` is ambiguous; use a Layer id."
        ))),
    }
}

fn layer_output(layer: core_space::LocalLayerSummary, active: Option<&str>) -> LayerOutput {
    LayerOutput {
        active: active == Some(layer.layer_id.as_str()),
        layer_id: layer.layer_id,
        name: layer.display_name,
        parent_layer_id: layer.parent_layer_id,
    }
}

fn current_dir_string() -> Result<String, CliError> {
    std::env::current_dir()
        .map(path_string)
        .map_err(|error| CliError::runtime(format!("Layrs could not resolve cwd: {error}")))
}

fn discover_current_space() -> Result<String, CliError> {
    let mut path = std::env::current_dir()
        .map_err(|error| CliError::runtime(format!("Layrs could not resolve cwd: {error}")))?;
    loop {
        if path.join(".layrs").join("local-space.json").exists() {
            return Ok(path_string(path));
        }
        if !path.pop() {
            return current_dir_string();
        }
    }
}

fn path_string(path: PathBuf) -> String {
    path.display().to_string()
}

fn path_key(path: &PathBuf) -> String {
    path_key_string(&path.display().to_string())
}

fn path_key_string(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}
