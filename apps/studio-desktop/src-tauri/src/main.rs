mod access_registry;
mod auth;
mod desktop_state;
mod folder_dialog;
mod http_client;
mod secret_store;

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
fn publish_local_space(
    local_space: String,
) -> Result<access_registry::SyncOperationResult, String> {
    access_registry::publish_local_space(local_space)
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

fn main() {
    tauri::Builder::default()
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
            send_draft_local_space,
            open_local_space,
            forget_local_space,
            switch_layer,
            create_layer_from_current,
            delete_layer,
            scan_working_tree,
            load_diff_window,
            receive_local_space,
            publish_local_space,
            load_desktop_settings,
            save_desktop_settings,
            select_folder
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Layrs Studio Desktop");
}
