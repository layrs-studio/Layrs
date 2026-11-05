//! Explicit Git interoperability for complete local projects. Preview tokens
//! bind the user confirmation to one checkout state and one exact scan/target.
use layrs_client_core::native_scm::NativeWorkspace;
use layrs_core::scm::StepId;
use layrs_git_bridge::{
    export_workspace_reviewed, review_export_destination, scan_git_selected, ExpectedBridgeState,
    ExportDestination, ExportMode, GitBridgeLimits, ImportSelection,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[derive(Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum GitExportRequest {
    History,
    SelectedHistory { references: Vec<String> },
    Snapshot { step_id: String },
}

#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GitAction {
    PreviewImport {
        source: String,
        references: Vec<String>,
    },
    ApplyImport {
        preview_id: String,
    },
    PreviewExport {
        destination: String,
        mode: GitExportRequest,
    },
    ApplyExport {
        preview_id: String,
    },
    DiscardPreview {
        preview_id: String,
    },
}

enum PendingKind {
    Import {
        source: PathBuf,
        references: Vec<String>,
        report_digest: String,
    },
    Export {
        destination: ExportDestination,
        mode: GitExportRequest,
    },
}

struct Pending {
    root: PathBuf,
    expected: ExpectedBridgeState,
    created: Instant,
    kind: PendingKind,
}

static PENDING: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
fn pending() -> &'static Mutex<HashMap<String, Pending>> {
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

fn limits() -> GitBridgeLimits {
    GitBridgeLimits::default()
}
fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn project(root: &str) -> Result<(PathBuf, NativeWorkspace), String> {
    let requested = NativeWorkspace::open(root).map_err(message)?;
    // Keep the registered path spelling for the bridge's exact Store binding.
    // Windows canonicalize() adds a verbatim prefix, while validation reopens
    // the persisted checkout path. Canonical paths are only used for overlap checks.
    let root = requested.checkout_root().map_err(message)?;
    let workspace = NativeWorkspace::open(&root).map_err(message)?;
    if workspace.needs_repair().map_err(message)? {
        return Err("Recover the native Worktree before Git operations.".into());
    }
    Ok((root, workspace))
}

fn register(
    root: PathBuf,
    expected: ExpectedBridgeState,
    kind: PendingKind,
) -> Result<String, String> {
    let token = uuid::Uuid::new_v4().to_string();
    let mut entries = pending().lock().map_err(message)?;
    entries.retain(|_, item| item.created.elapsed() < Duration::from_secs(600));
    if entries.len() >= 32 {
        if let Some(oldest) = entries
            .iter()
            .min_by_key(|(_, item)| item.created)
            .map(|(key, _)| key.clone())
        {
            entries.remove(&oldest);
        }
    }
    entries.insert(
        token.clone(),
        Pending {
            root,
            expected,
            created: Instant::now(),
            kind,
        },
    );
    Ok(token)
}

fn take(
    root: &Path,
    token: &str,
) -> Result<(NativeWorkspace, PendingKind, ExpectedBridgeState), String> {
    let entry = pending()
        .lock()
        .map_err(message)?
        .remove(token)
        .ok_or("Git preview expired; preview again before confirming.")?;
    if entry.created.elapsed() >= Duration::from_secs(600) || entry.root != root {
        return Err("Git preview expired or belongs to another project; preview again.".into());
    }
    let workspace = NativeWorkspace::open(root).map_err(message)?;
    if workspace.needs_repair().map_err(message)? {
        return Err("Recover the native Worktree before Git operations.".into());
    }
    if entry.expected.validate(&workspace).is_err() {
        return Err("Project changed since the Git preview; preview again.".into());
    }
    Ok((workspace, entry.kind, entry.expected))
}

fn selection(references: Vec<String>) -> ImportSelection {
    if references.is_empty() {
        ImportSelection::AllLocalReferences
    } else {
        ImportSelection::References(references)
    }
}

fn destination(root: &Path, input: &str) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(message)?;
    let requested = std::path::absolute(input).map_err(message)?;
    let name = requested
        .file_name()
        .ok_or("Choose a new Git folder name.")?;
    let label = name.to_string_lossy();
    if label.eq_ignore_ascii_case(".git") || label.eq_ignore_ascii_case(".layrs") {
        return Err("A reserved metadata folder cannot be a Git export destination.".into());
    }
    let parent = requested
        .parent()
        .ok_or("Choose a parent folder for the Git export.")?
        .canonicalize()
        .map_err(message)?;
    if !parent.is_dir() {
        return Err("The export parent is not a directory.".into());
    }
    let target = parent.join(name);
    if parent.starts_with(&root) || root.starts_with(&target) {
        return Err("Choose a new Git folder outside the Layrs project.".into());
    }
    match std::fs::symlink_metadata(&target) {
        Ok(_) => Err("Git export destination already exists; choose a new folder.".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(target),
        Err(e) => Err(message(e)),
    }
}

fn export_mode(mode: GitExportRequest) -> Result<ExportMode, String> {
    Ok(match mode {
        GitExportRequest::History => ExportMode::History,
        GitExportRequest::SelectedHistory { references } => {
            if references.is_empty() {
                return Err("Choose at least one exact Git reference.".into());
            }
            if references
                .iter()
                .any(|r| !r.starts_with("refs/heads/") && !r.starts_with("refs/tags/"))
            {
                return Err("Use full refs/heads/ or refs/tags/ names.".into());
            }
            ExportMode::SelectedHistory { references }
        }
        GitExportRequest::Snapshot { step_id } => ExportMode::Snapshot {
            step_id: StepId::parse(step_id).map_err(message)?,
            reference: "refs/heads/main".into(),
        },
    })
}

fn execute(root: String, request: GitAction) -> Result<Value, String> {
    let (root, workspace) = project(&root)?;
    match request {
        GitAction::PreviewImport { source, references } => {
            let expected = ExpectedBridgeState::capture(&workspace).map_err(message)?;
            let source = std::fs::canonicalize(source).map_err(message)?;
            let canonical_root = root.canonicalize().map_err(message)?;
            if source.starts_with(&canonical_root) || canonical_root.starts_with(&source) {
                return Err("Choose a separate Git repository outside the Layrs project.".into());
            }
            let plan = scan_git_selected(&source, &selection(references.clone()), limits())
                .map_err(message)?;
            let report = plan.report();
            let digest = blake3::hash(&serde_json::to_vec(report).map_err(message)?)
                .to_hex()
                .to_string();
            let summary = json!({
                "source": source, "commitCount": report.commits.len(), "references": report.references.iter().map(|r| &r.git_ref).collect::<Vec<_>>(),
                "warnings": report.warnings, "sourceBytes": report.source_bytes,
            });
            expected.validate(&workspace).map_err(message)?;
            let preview_id = register(
                root,
                expected,
                PendingKind::Import {
                    source,
                    references,
                    report_digest: digest,
                },
            )?;
            Ok(json!({"previewId": preview_id, "summary": summary}))
        }
        GitAction::ApplyImport { preview_id } => {
            let (workspace, kind, expected) = take(&root, &preview_id)?;
            let PendingKind::Import {
                source,
                references,
                report_digest,
            } = kind
            else {
                return Err("This preview belongs to a Git export.".into());
            };
            let plan =
                scan_git_selected(&source, &selection(references), limits()).map_err(message)?;
            let actual = blake3::hash(&serde_json::to_vec(plan.report()).map_err(message)?)
                .to_hex()
                .to_string();
            if actual != report_digest {
                return Err("Git source changed since the preview; preview again.".into());
            }
            let outcome = plan
                .install_expected(&workspace, &expected)
                .map_err(message)?;
            let report = plan.report();
            Ok(json!({"import": outcome, "report": {
                "commitCount": report.commits.len(), "references": report.references.iter().map(|r| json!({"gitRef":r.git_ref,"gitId":r.git_id,"stepId":r.step_id})).collect::<Vec<_>>(),
                "warnings": report.warnings, "sourceBytes": report.source_bytes,
            }}))
        }
        GitAction::PreviewExport {
            destination: input,
            mode,
        } => {
            let target = destination(&root, &input)?;
            let _ = export_mode(mode.clone())?
                .validated(limits())
                .map_err(message)?;
            let reviewed = review_export_destination(&target, limits()).map_err(message)?;
            let expected = ExpectedBridgeState::capture(&workspace).map_err(message)?;
            let state = expected.state();
            let summary = json!({
                "destination": target, "mode": match &mode { GitExportRequest::History => "history", GitExportRequest::SelectedHistory { .. } => "selected_history", GitExportRequest::Snapshot { .. } => "snapshot" },
                "layerCount": state.layers.len(), "markerCount": state.view.marker_ids().len(),
                "references": match &mode { GitExportRequest::SelectedHistory { references } => references.clone(), _ => vec![] },
                "stepId": match &mode { GitExportRequest::Snapshot { step_id } => Some(step_id.clone()), _ => None },
            });
            let preview_id = register(
                root,
                expected,
                PendingKind::Export {
                    destination: reviewed,
                    mode,
                },
            )?;
            Ok(json!({"previewId": preview_id, "summary": summary}))
        }
        GitAction::ApplyExport { preview_id } => {
            let (workspace, kind, expected) = take(&root, &preview_id)?;
            let PendingKind::Export {
                destination: reviewed,
                mode,
            } = kind
            else {
                return Err("This preview belongs to a Git import.".into());
            };
            let target = reviewed.path();
            if destination(&root, target.to_str().ok_or("Destination is not UTF-8.")?)? != target {
                return Err("Git export parent changed since the preview; preview again.".into());
            }
            let report = export_workspace_reviewed(
                &workspace,
                &expected,
                &reviewed,
                export_mode(mode)?,
                limits(),
                &mut |_| true,
            )
            .map_err(message)?;
            Ok(json!({"export": report, "destination": target}))
        }
        GitAction::DiscardPreview { preview_id } => {
            let mut entries = pending().lock().map_err(message)?;
            if entries
                .get(&preview_id)
                .is_some_and(|item| item.root == root)
            {
                entries.remove(&preview_id);
            }
            Ok(json!({"discarded": true}))
        }
    }
}

#[tauri::command]
pub async fn native_git(root: String, request: GitAction) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || execute(root, request))
        .await
        .map_err(message)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    struct Sandbox(PathBuf);
    impl Sandbox {
        fn new() -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!("layrs-desktop-git-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn commit(root: &Path, name: &str) {
        std::fs::write(root.join("content.txt"), name).unwrap();
        git(root, &["add", "content.txt"]);
        git(
            root,
            &[
                "-c",
                "user.name=Layrs Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                name,
            ],
        );
    }

    #[test]
    fn request_shapes_are_explicit() {
        let request: GitAction = serde_json::from_value(json!({"action":"previewExport","destination":"unused","mode":{"kind":"snapshot","stepId":"step:abc"}})).unwrap();
        assert!(matches!(
            request,
            GitAction::PreviewExport {
                mode: GitExportRequest::Snapshot { .. },
                ..
            }
        ));
    }

    #[test]
    fn import_rechecks_source_and_export_requires_a_new_safe_destination() {
        let sandbox = Sandbox::new();
        let source = sandbox.0.join("source");
        let project_root = sandbox.0.join("project");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&project_root).unwrap();
        git(&source, &["init", "-q"]);
        commit(&source, "first");
        NativeWorkspace::initialize(&project_root, "Git adapter test").unwrap();
        let root = project_root.to_string_lossy().into_owned();
        let source_text = source.to_string_lossy().into_owned();
        let first = execute(
            root.clone(),
            GitAction::PreviewImport {
                source: source_text.clone(),
                references: vec![],
            },
        )
        .unwrap();
        commit(&source, "second");
        let error = execute(
            root.clone(),
            GitAction::ApplyImport {
                preview_id: first["previewId"].as_str().unwrap().into(),
            },
        )
        .unwrap_err();
        assert!(error.contains("changed since the preview"), "{error}");
        assert!(
            NativeWorkspace::open(&project_root)
                .unwrap()
                .state()
                .unwrap()
                .layers
                .len()
                == 1
        );

        let current = execute(
            root.clone(),
            GitAction::PreviewImport {
                source: source_text,
                references: vec![],
            },
        )
        .unwrap();
        let import_plan =
            scan_git_selected(&source, &ImportSelection::AllLocalReferences, limits()).unwrap();
        let before_import =
            ExpectedBridgeState::capture(&NativeWorkspace::open(&project_root).unwrap()).unwrap();
        let imported = execute(
            root.clone(),
            GitAction::ApplyImport {
                preview_id: current["previewId"].as_str().unwrap().into(),
            },
        )
        .unwrap();
        assert_eq!(imported["import"]["alreadyImported"], false);
        assert_eq!(imported["import"]["layerIds"].as_array().unwrap().len(), 1);
        assert!(import_plan
            .install_expected(
                &NativeWorkspace::open(&project_root).unwrap(),
                &before_import
            )
            .is_err());

        let reserved = project_root.join(".layrs").to_string_lossy().into_owned();
        assert!(execute(
            root.clone(),
            GitAction::PreviewExport {
                destination: reserved,
                mode: GitExportRequest::History
            }
        )
        .is_err());
        let target = sandbox.0.join("export");
        let preview = execute(
            root.clone(),
            GitAction::PreviewExport {
                destination: target.to_string_lossy().into_owned(),
                mode: GitExportRequest::History,
            },
        )
        .unwrap();
        assert!(!target.exists());
        let before_export =
            ExpectedBridgeState::capture(&NativeWorkspace::open(&project_root).unwrap()).unwrap();
        std::fs::write(project_root.join("changed.txt"), "new project work").unwrap();
        NativeWorkspace::open(&project_root)
            .unwrap()
            .capture()
            .unwrap();
        let stale = execute(
            root.clone(),
            GitAction::ApplyExport {
                preview_id: preview["previewId"].as_str().unwrap().into(),
            },
        )
        .unwrap_err();
        assert!(stale.contains("changed since the Git preview"), "{stale}");
        assert!(!target.exists());
        assert!(layrs_git_bridge::export_workspace_expected(
            &NativeWorkspace::open(&project_root).unwrap(),
            &before_export,
            &target,
            ExportMode::History,
            limits(),
            &mut |_| true,
        )
        .is_err());
        assert!(!target.exists());
        let preview = execute(
            root.clone(),
            GitAction::PreviewExport {
                destination: target.to_string_lossy().into_owned(),
                mode: GitExportRequest::History,
            },
        )
        .unwrap();
        let exported = execute(
            root,
            GitAction::ApplyExport {
                preview_id: preview["previewId"].as_str().unwrap().into(),
            },
        )
        .unwrap();
        assert!(!exported["export"]["references"]
            .as_object()
            .unwrap()
            .is_empty());
        assert!(target.join("HEAD").exists());
        assert!(target.join("objects").exists());
    }
}
