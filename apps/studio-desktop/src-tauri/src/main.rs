mod folder_dialog;

use layrs_client_core::{access_registry, auth, desktop_state};
use tauri::{Manager, PhysicalSize, Size};

#[tauri::command]
fn desktop_status() -> Result<auth::DesktopStatus, String> {
    auth::desktop_status()
}

#[tauri::command]
fn configure_server_endpoint(server_endpoint: String) -> Result<auth::DesktopStatus, String> {
    auth::configure_endpoint(server_endpoint)
}

#[tauri::command]
fn start_device_login() -> Result<auth::DeviceLoginStartResponse, String> {
    auth::start_device_login()
}

#[tauri::command]
fn poll_device_login(
    device_code: String,
    workspace_root: Option<String>,
) -> Result<auth::DeviceLoginPollResponse, String> {
    auth::poll_device_login(device_code, workspace_root)
}

#[tauri::command]
fn refresh_bootstrap(
    workspace_root: Option<String>,
) -> Result<auth::DeviceLoginPollResponse, String> {
    auth::refresh_bootstrap(workspace_root)
}

#[tauri::command]
fn list_available_spaces() -> Result<Vec<access_registry::AvailableSpaceView>, String> {
    access_registry::list_available_spaces()
}

#[tauri::command]
fn list_local_spaces() -> Result<Vec<access_registry::LocalSpaceSummary>, String> {
    access_registry::list_local_spaces()
}

#[tauri::command]
fn create_local_space(
    space_id: String,
    target_folder: String,
    initial_layer_id: Option<String>,
) -> Result<access_registry::CreateLocalSpaceResult, String> {
    access_registry::create_local_space(space_id, target_folder, initial_layer_id)
}

#[tauri::command]
fn create_draft_local_space(
    name: String,
    target_folder: String,
) -> Result<access_registry::CreateLocalSpaceResult, String> {
    access_registry::create_draft_local_space(name, target_folder)
}

#[tauri::command]
fn init_local_space(
    name: String,
    target_folder: String,
) -> Result<access_registry::CreateLocalSpaceResult, String> {
    let result = access_registry::init_local_space(name, target_folder)?;
    Ok(access_registry::CreateLocalSpaceResult {
        local_space: result.local_space,
        created: result.created,
    })
}

#[tauri::command]
fn send_draft_local_space(
    local_space: String,
    workspace_id: String,
) -> Result<access_registry::SendDraftLocalSpaceResult, String> {
    access_registry::send_draft_local_space(local_space, workspace_id)
}

#[tauri::command]
fn open_local_space(
    local_space_id_or_path: String,
) -> Result<access_registry::LocalSpaceSummary, String> {
    access_registry::open_local_space(local_space_id_or_path)
}

#[tauri::command]
fn forget_local_space(
    local_space: String,
) -> Result<access_registry::ForgetLocalSpaceResult, String> {
    access_registry::forget_local_space(local_space)
}

#[tauri::command]
fn switch_layer(
    local_space: String,
    target_layer_id: String,
) -> Result<access_registry::LayerSwitchResult, String> {
    access_registry::switch_layer(local_space, target_layer_id)
}

#[tauri::command]
fn create_layer_from_current(
    local_space: String,
    name: String,
) -> Result<access_registry::LayerSwitchResult, String> {
    access_registry::create_layer_from_current(local_space, name)
}

#[tauri::command]
fn delete_layer(
    local_space: String,
    layer_id: String,
) -> Result<access_registry::DeleteLayerResult, String> {
    access_registry::delete_layer(local_space, layer_id)
}

#[tauri::command]
fn disconnect_layer_from_parent(
    local_space: String,
    layer_id: String,
) -> Result<access_registry::LayerSettingsResult, String> {
    access_registry::disconnect_layer_from_parent(local_space, layer_id)
}

#[tauri::command]
fn clear_layer_steps(
    local_space: String,
    layer_id: String,
) -> Result<access_registry::LayerSettingsResult, String> {
    access_registry::clear_layer_steps(local_space, layer_id, true)
}

#[tauri::command]
fn scan_working_tree(local_space: String) -> Result<access_registry::WorkingTreeScan, String> {
    access_registry::scan_working_tree(local_space)
}

#[tauri::command]
fn load_diff_window(
    local_space: String,
    path: String,
    source: Option<String>,
    start: usize,
    limit: usize,
) -> Result<access_registry::LensDiffEntry, String> {
    access_registry::load_diff_window(local_space, path, source, start, limit)
}

#[tauri::command]
fn receive_local_space(
    local_space: String,
) -> Result<access_registry::SyncOperationResult, String> {
    access_registry::receive_local_space(local_space)
}

#[tauri::command]
fn save_local_step(
    local_space: String,
) -> Result<access_registry::SaveLocalStepResult, String> {
    access_registry::save_local_step(local_space)
}

#[tauri::command]
fn publish_local_space(
    local_space: String,
) -> Result<access_registry::SyncOperationResult, String> {
    access_registry::publish_local_space(local_space)
}

#[tauri::command]
fn sync_local_space(
    local_space: String,
) -> Result<access_registry::SyncOperationResult, String> {
    access_registry::sync_local_space(local_space)
}

#[tauri::command]
fn weave_layers(
    local_space: String,
    source_layer_id: String,
    target_layer_id: String,
    preview: bool,
) -> Result<access_registry::WeaveOperationResult, String> {
    access_registry::weave_layers(local_space, source_layer_id, target_layer_id, preview)
}

#[tauri::command]
fn weave_active_layer_to_parent(
    local_space: String,
    preview: bool,
) -> Result<access_registry::WeaveOperationResult, String> {
    access_registry::weave_active_layer_to_parent(local_space, preview)
}

#[tauri::command]
fn weave_status(local_space: String) -> Result<Option<access_registry::WeaveSessionSummary>, String> {
    access_registry::weave_status(local_space)
}

#[tauri::command]
fn weave_conflicts(local_space: String) -> Result<Vec<access_registry::WeaveConflictSummary>, String> {
    access_registry::weave_conflicts(local_space)
}

#[tauri::command]
fn resolve_weave_conflict(
    local_space: String,
    path: String,
    resolution: String,
    replacement_file: Option<String>,
    manual_text: Option<String>,
) -> Result<access_registry::WeaveOperationResult, String> {
    access_registry::resolve_weave_conflict(local_space, path, resolution, replacement_file, manual_text)
}

#[tauri::command]
fn continue_weave(local_space: String) -> Result<access_registry::WeaveOperationResult, String> {
    access_registry::continue_weave(local_space)
}

#[tauri::command]
fn abort_weave(local_space: String) -> Result<access_registry::WeaveOperationResult, String> {
    access_registry::abort_weave(local_space)
}

#[tauri::command]
fn load_desktop_settings() -> Result<desktop_state::DesktopSettings, String> {
    access_registry::load_desktop_settings()
}

#[tauri::command]
fn save_desktop_settings(
    settings: desktop_state::DesktopSettings,
) -> Result<desktop_state::DesktopSettings, String> {
    access_registry::save_desktop_settings(settings)
}

#[tauri::command]
fn select_folder(initial_directory: Option<String>) -> Result<Option<String>, String> {
    folder_dialog::select_folder(initial_directory)
}

fn apply_e2e_window_settings(app: &tauri::App) -> Result<(), tauri::Error> {
    if let Some(window) = app.get_webview_window("main") {
        if let Ok(instance_id) = std::env::var("LAYRS_E2E_INSTANCE_ID") {
            window.set_title(&format!("Layrs Studio E2E {instance_id}"))?;
        }

        if std::env::var("LAYRS_E2E_WINDOW_SIZE").ok().as_deref() == Some("1920x1080") {
            window.set_size(Size::Physical(PhysicalSize::new(1920, 1080)))?;
            let _ = window.center();
        }
    }

    Ok(())
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            apply_e2e_window_settings(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            desktop_status,
            configure_server_endpoint,
            start_device_login,
            poll_device_login,
            refresh_bootstrap,
            list_available_spaces,
            list_local_spaces,
            create_local_space,
            create_draft_local_space,
            init_local_space,
            send_draft_local_space,
            open_local_space,
            forget_local_space,
            switch_layer,
            create_layer_from_current,
            delete_layer,
            disconnect_layer_from_parent,
            clear_layer_steps,
            scan_working_tree,
            load_diff_window,
            receive_local_space,
            save_local_step,
            publish_local_space,
            sync_local_space,
            weave_layers,
            weave_active_layer_to_parent,
            weave_status,
            weave_conflicts,
            resolve_weave_conflict,
            continue_weave,
            abort_weave,
            load_desktop_settings,
            save_desktop_settings,
            select_folder
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Layrs Studio Desktop");
}
