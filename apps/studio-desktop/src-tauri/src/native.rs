use layrs_client_core::native_scm::{
    NativeRemoteRequest, WorkspaceAction, execute_remote, execute_workspace,
};

#[tauri::command]
pub async fn native_workspace(
    root: String,
    request: WorkspaceAction,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        execute_workspace(root, request).map_err(|e| format!("{}: {e}", e.code()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn native_remote(
    root: String,
    request: NativeRemoteRequest,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || execute_remote(root, request))
        .await
        .map_err(|e| e.to_string())?
}
