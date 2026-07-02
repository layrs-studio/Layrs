use std::process::{Command, Stdio};

pub fn select_folder(initial_directory: Option<String>) -> Result<Option<String>, String> {
    if let Ok(folder) = std::env::var("LAYRS_E2E_SELECTED_FOLDER") {
        let folder = folder.trim().to_string();
        if !folder.is_empty() {
            return Ok(Some(folder));
        }
    }

    select_folder_impl(initial_directory)
}

#[cfg(target_os = "windows")]
fn select_folder_impl(initial_directory: Option<String>) -> Result<Option<String>, String> {
    let script = r#"
Add-Type -AssemblyName System.Windows.Forms
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$dialog = New-Object System.Windows.Forms.FolderBrowserDialog
$dialog.Description = "Choose a Layrs folder"
$dialog.ShowNewFolderButton = $true
$initial = [Environment]::GetEnvironmentVariable("LAYRS_INITIAL_FOLDER")
if ($initial) {
    $candidate = $initial
    while ($candidate -and -not (Test-Path -LiteralPath $candidate)) {
        $candidate = Split-Path -Parent $candidate
    }
    if ($candidate) {
        $dialog.SelectedPath = $candidate
    }
}
if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    Write-Output $dialog.SelectedPath
}
"#;

    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(initial_directory) = initial_directory.filter(|value| !value.trim().is_empty()) {
        command.env("LAYRS_INITIAL_FOLDER", initial_directory);
    }

    let output = command
        .output()
        .map_err(|error| format!("Layrs Desktop could not open the folder picker: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "Layrs Desktop folder picker exited without a selected folder.".to_string()
        } else {
            format!("Layrs Desktop folder picker failed: {stderr}")
        });
    }

    let selected = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if selected.is_empty() {
        Ok(None)
    } else {
        Ok(Some(selected))
    }
}

#[cfg(not(target_os = "windows"))]
fn select_folder_impl(_initial_directory: Option<String>) -> Result<Option<String>, String> {
    Err(
        "Layrs Desktop native folder picker is only implemented on Windows in this build."
            .to_string(),
    )
}
