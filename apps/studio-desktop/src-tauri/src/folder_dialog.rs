pub fn select_folder(
    window: &tauri::WebviewWindow,
    initial_directory: Option<String>,
) -> Result<Option<String>, String> {
    if let Ok(folder) = std::env::var("LAYRS_E2E_SELECTED_FOLDER") {
        let folder = folder.trim().to_string();
        if !folder.is_empty() {
            return Ok(Some(folder));
        }
    }
    select_folder_impl(window, initial_directory)
}

#[cfg(target_os = "windows")]
fn select_folder_impl(
    window: &tauri::WebviewWindow,
    initial_directory: Option<String>,
) -> Result<Option<String>, String> {
    let owner = window.hwnd().map_err(|error| error.to_string())?;
    windows_picker::select(owner, initial_directory)
        .map_err(|error| format!("Layrs Desktop could not open the folder picker: {error}"))
}

#[cfg(target_os = "windows")]
mod windows_picker {
    use std::path::{Path, PathBuf};
    use windows::{
        core::{w, HSTRING},
        Win32::{
            Foundation::{ERROR_CANCELLED, HWND},
            System::Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
            },
            UI::Shell::{
                FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName,
                FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS,
                SIGDN_FILESYSPATH,
            },
        },
    };

    struct ComApartment;

    impl Drop for ComApartment {
        fn drop(&mut self) {
            // Balanced even when COM was already initialized (S_FALSE).
            unsafe { CoUninitialize() };
        }
    }

    fn existing_directory(initial: &str) -> Option<PathBuf> {
        if initial.trim().is_empty() {
            return None;
        }
        Path::new(initial)
            .ancestors()
            .find(|path| path.is_dir())
            .map(Path::to_path_buf)
    }

    pub fn select(
        owner: HWND,
        initial_directory: Option<String>,
    ) -> windows::core::Result<Option<String>> {
        // This synchronous Tauri command runs on the UI thread. The native dialog
        // pumps its own messages and remains modal to the invoking Layrs window.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).ok()?;
            let _apartment = ComApartment;
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            dialog.SetOptions(
                dialog.GetOptions()?
                    | FOS_PICKFOLDERS
                    | FOS_FORCEFILESYSTEM
                    | FOS_PATHMUSTEXIST
                    | FOS_NOCHANGEDIR,
            )?;
            dialog.SetTitle(w!("Choisir un dossier pour Layrs"))?;

            if let Some(directory) = initial_directory.as_deref().and_then(existing_directory) {
                // A stale or unavailable starting location must not prevent browsing.
                let path = HSTRING::from(directory.as_os_str());
                if let Ok(folder) = SHCreateItemFromParsingName::<_, _, IShellItem>(&path, None) {
                    let _ = dialog.SetFolder(&folder);
                }
            }

            if let Err(error) = dialog.Show(owner) {
                if error.code() == ERROR_CANCELLED.to_hresult() {
                    return Ok(None);
                }
                return Err(error);
            }

            let item = dialog.GetResult()?;
            let path = item.GetDisplayName(SIGDN_FILESYSPATH)?;
            let selected = path.to_string();
            CoTaskMemFree(Some(path.0.cast()));
            Ok(Some(selected?))
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn select_folder_impl(
    _window: &tauri::WebviewWindow,
    _initial_directory: Option<String>,
) -> Result<Option<String>, String> {
    Err(
        "Layrs Desktop native folder picker is only implemented on Windows in this build."
            .to_string(),
    )
}
