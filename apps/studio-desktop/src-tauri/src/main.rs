mod folder_dialog;
mod git_bridge;
mod native;

use tauri::{Manager, PhysicalSize, Size};

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
        .manage(git_bridge::GitBridgeState::default())
        .setup(|app| {
            apply_e2e_window_settings(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            native::native_workspace,
            native::native_remote,
            native::native_checkout_kind,
            native::native_projection,
            git_bridge::native_git_bridge,
            select_folder
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Layrs Studio Desktop");
}
