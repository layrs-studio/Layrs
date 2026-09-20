use crate::{
    args::{Cli, CliCommand},
    engine::CliError,
    render::Rendered,
};
use layrs_client_core::native_scm::{CapturePlan, NativeWorkspace};
use layrs_core::scm::{MarkerId, OperationId, OperationKind, StepId};
use serde_json::json;
#[path = "native_blocks.rs"]
mod blocks;
#[path = "native_maintenance.rs"]
mod maintenance;
#[path = "native_preview.rs"]
mod preview_output;

// Progress goes to stderr, preserving the machine-readable stdout envelope.
fn bridge_progress() -> impl FnMut(&layrs_git_bridge::BridgeProgress) -> bool {
    let mut last = None;
    let mut phase = None;
    move |progress| {
        let now = std::time::Instant::now();
        if phase != Some(progress.phase)
            || last.is_none_or(|at: std::time::Instant| now.duration_since(at).as_millis() >= 500)
        {
            eprintln!(
                "Git {:?}: {}/{}; {} objects; {} bytes",
                progress.phase,
                progress.completed,
                progress.total.map_or_else(|| "?".into(), |v| v.to_string()),
                progress.objects,
                progress.bytes
            );
            last = Some(now);
            phase = Some(progress.phase);
        }
        true
    }
}
fn bridge_limits(seconds: u32) -> layrs_git_bridge::GitBridgeLimits {
    layrs_git_bridge::GitBridgeLimits {
        max_duration_ms: u64::from(seconds) * 1000,
        ..Default::default()
    }
}
fn bridge_selection(references: &[String]) -> layrs_git_bridge::ImportSelection {
    if references.is_empty() {
        layrs_git_bridge::ImportSelection::AllLocalReferences
    } else {
        layrs_git_bridge::ImportSelection::References(references.to_vec())
    }
}

pub fn try_run(cli: &Cli) -> Option<Result<Rendered, CliError>> {
    if let CliCommand::Remote { args } = &cli.command {
        return Some(crate::native_remote::run(cli, args));
    }
    if let CliCommand::GitScanCache {
        source,
        output,
        timeout_seconds,
        references,
    } = &cli.command
    {
        return Some((|| {
            let checkpoint = layrs_git_bridge::prepare_git_scan_checkpoint(
                source,
                output,
                &bridge_selection(references),
                bridge_limits(*timeout_seconds),
                &mut bridge_progress(),
            )
            .map_err(error)?;
            Ok(Rendered {
                human: format!(
                    "Cached {} commits at {}\nPin: {}\nThe cache contains the selected history; protect it like the source repository.",
                    checkpoint.commits,
                    checkpoint.path.display(),
                    checkpoint.digest
                ),
                data: json!({"checkpoint":checkpoint}),
            })
        })());
    }
    if let CliCommand::GitImportCache {
        checkpoint,
        digest,
        dry_run: true,
        timeout_seconds,
    } = &cli.command
    {
        return Some((|| {
            let plan = layrs_git_bridge::resume_git_scan_checkpoint(
                checkpoint,
                digest,
                bridge_limits(*timeout_seconds),
                &mut bridge_progress(),
            )
            .map_err(error)?;
            Ok(Rendered {
                human: format!(
                    "{} cached commits, {} references\n{}",
                    plan.report().commits.len(),
                    plan.report().references.len(),
                    plan.report().warnings.join("\n")
                ),
                data: json!({"dryRun":true,"report":plan.report()}),
            })
        })());
    }
    if let CliCommand::GitImport {
        source,
        dry_run: true,
        timeout_seconds,
        references,
    } = &cli.command
    {
        return Some((|| {
            let plan = layrs_git_bridge::scan_git_selected_with_progress(
                source,
                &bridge_selection(references),
                bridge_limits(*timeout_seconds),
                &mut bridge_progress(),
            )
            .map_err(error)?;
            Ok(Rendered {
                human: format!(
                    "{} commits, {} references\n{}",
                    plan.report().commits.len(),
                    plan.report().references.len(),
                    plan.report().warnings.join("\n")
                ),
                data: json!({"dryRun":true,"report":plan.report()}),
            })
        })());
    }
    if matches!(
        cli.command,
        CliCommand::Login { .. }
            | CliCommand::Logout
            | CliCommand::Whoami
            | CliCommand::Spaces
            | CliCommand::Help
    ) {
        return None;
    }
    let root = match cli
        .globals
        .space
        .clone()
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
    {
        Ok(root) => root,
        Err(e) => return Some(Err(error(e))),
    };
    if let CliCommand::Compact { resume } = &cli.command {
        return Some(maintenance::compact(&root, *resume));
    }
    if let CliCommand::Init { name, path } = &cli.command {
        return Some((|| {
            let workspace =
                NativeWorkspace::initialize(path.as_ref().unwrap_or(&root), name).map_err(error)?;
            let state = workspace.state().map_err(error)?;
            Ok(Rendered {
                human: format!(
                    "Initialized {} at {}",
                    state.space.name(),
                    state.worktree.checkout_root()
                ),
                data: json!({"spaceId":state.space.id(),"worktreeId":state.worktree.id(),"path":state.worktree.checkout_root(),"state":state}),
            })
        })());
    }
    if matches!(cli.command, CliCommand::Doctor) {
        return Some((|| {
            let data = layrs_client_core::native_scm::diagnose_checkout(&root).map_err(error)?;
            let human = format!(
                "Integrity: {}. {} objects, {} rows checked.\n{}",
                if data["clean"] == true && data["complete"] == true {
                    "clean"
                } else {
                    "needs attention"
                },
                data["checkedObjects"],
                data["checkedRows"],
                serde_json::to_string_pretty(&data["findings"]).map_err(error)?
            );
            Ok(Rendered { human, data })
        })());
    }
    // Existing legacy metadata still uses the legacy engine; no format conversion.
    let native = root
        .ancestors()
        .find(|p| p.join(".layrs").exists())
        .is_some_and(|p| p.join(".layrs/FORMAT").exists());
    if !native {
        return None;
    }
    Some((|| {
        let workspace = NativeWorkspace::discover(&root).map_err(error)?;
        if let CliCommand::Diff {
            step_id,
            stat,
            name_only,
            window,
            ..
        } = &cli.command
        {
            let mut changes = match step_id {
                Some(id) => workspace.step_diff(
                    &workspace.resolve_step_name(id).map_err(error)?.step_id,
                    None,
                ),
                None => workspace.working_diff(),
            }
            .map_err(error)?;
            let mut human = String::new();
            for change in &mut changes {
                if *name_only {
                    human.push_str(&format!("{}\n", change.path));
                    continue;
                }
                human.push_str(&format!(
                    "{} {} ({} → {} bytes)\n",
                    change.status,
                    change.path,
                    change
                        .before_bytes
                        .map_or("absent".into(), |n| n.to_string()),
                    change
                        .after_bytes
                        .map_or("absent".into(), |n| n.to_string())
                ));
                if !stat {
                    if let Some(lines) = &change.text_diff {
                        for line in lines
                            .iter()
                            .skip(window.as_ref().map_or(0, |w| w.start as usize))
                            .take(window.as_ref().map_or(usize::MAX, |w| w.limit as usize))
                        {
                            human.push_str(match line.op {
                                "insert" => "+",
                                "delete" => "-",
                                _ => " ",
                            });
                            human.push_str(&line.text);
                            if !line.text.ends_with('\n') {
                                human.push_str("\n\\ No newline at end of file\n");
                            }
                        }
                    } else if change.binary {
                        human.push_str("Binary content changed\n");
                    } else if change.text_omitted {
                        human.push_str("Text preview exceeds display limits\n");
                    }
                }
            }
            if human.is_empty() {
                human = "No changes.".into();
            }
            return Ok(Rendered {
                human,
                data: json!({"changes":changes}),
            });
        }
        let data = match &cli.command {
            CliCommand::ResolveBlock { .. } => blocks::execute(&workspace, &cli.command)?,
            CliCommand::ResolveTerm {
                layer,
                conflict,
                term,
                preview,
            } => {
                let choices = std::collections::BTreeMap::from([(
                    layrs_core::scm::ConflictId::parse(conflict).map_err(error)?,
                    layrs_core::scm::ObjectId::parse(term).map_err(error)?,
                )]);
                let plan = workspace
                    .preview_conflict_resolution(
                        &workspace.layer_named(layer).map_err(error)?,
                        &choices,
                        Default::default(),
                    )
                    .map_err(error)?;
                if *preview {
                    json!({"resultStep":plan.bundle().step,"conflicts":plan.bundle().conflicts})
                } else {
                    json!({"resolution":workspace.accept_conflict_resolution(&plan).map_err(error)?})
                }
            }
            CliCommand::Weave {
                source,
                target,
                preview,
            } => {
                let plan = if *preview {
                    workspace.preview_named_weave(source, target)
                } else {
                    workspace.prepare_named_weave(source, target)
                }
                .map_err(error)?;
                if *preview {
                    json!({"resultStep":plan.plan().step(),"sourceHeads":plan.plan().source_heads(),"targetHeads":plan.plan().target_heads(),"conflicts":plan.plan().conflicts().values().collect::<Vec<_>>()})
                } else {
                    json!({"weave":workspace.accept_merge(&plan).map_err(error)?})
                }
            }
            CliCommand::ConflictVariant {
                layer,
                conflict,
                term,
                against,
            } => {
                let layer = workspace.layer_named(layer).map_err(error)?;
                let conflict = layrs_core::scm::ConflictId::parse(conflict).map_err(error)?;
                let term = layrs_core::scm::ObjectId::parse(term).map_err(error)?;
                match against {
                    Some(before) => workspace
                        .compare_conflict_variants(
                            &layer,
                            &conflict,
                            &layrs_core::scm::ObjectId::parse(before).map_err(error)?,
                            &term,
                        )
                        .map_err(error)?,
                    None => workspace
                        .conflict_variant(&layer, &conflict, &term)
                        .map_err(error)?,
                }
            }
            CliCommand::WeaveContinue | CliCommand::ConflictContinue => {
                return Err(CliError::usage(
                    "Native conflicts are immutable Steps, not an active session. Use weave conflicts, then resolve-term or resolve-file. The last resolution installs the result automatically; no continue is required.",
                ));
            }
            CliCommand::WeaveAbort | CliCommand::ConflictAbort => {
                return Err(CliError::usage(
                    "Native conflicts have no session to abort. Use operation-history then restore-view OPERATION --preview to inspect a historical View, or undo to return to the preceding Operation. These append history; they do not erase the conflict Step.",
                ));
            }
            CliCommand::WeaveConflicts
            | CliCommand::ConflictList
            | CliCommand::ConflictStatus
            | CliCommand::WeaveStatus => {
                let state = workspace.state().map_err(error)?;
                let layer = state
                    .view
                    .worktrees()
                    .iter()
                    .find(|w| w.worktree_id() == workspace.worktree_id())
                    .and_then(|w| w.active_layer_id())
                    .ok_or_else(|| CliError::runtime("No active Layer"))?;
                json!({"layerId":layer,"conflicts":workspace.layer_conflicts(layer).map_err(error)?})
            }
            CliCommand::ResolveFile {
                layer,
                conflict,
                file,
                executable,
                preview,
            } => {
                use std::io::Read;
                let mut bytes = Vec::new();
                std::fs::File::open(file)
                    .map_err(error)?
                    .take(64 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(error)?;
                if bytes.len() > 64 * 1024 * 1024 {
                    return Err(CliError::usage("replacement exceeds 64 MiB"));
                }
                let choices = std::collections::BTreeMap::from([(
                    layrs_core::scm::ConflictId::parse(conflict).map_err(error)?,
                    layrs_client_core::native_scm::ConflictReplacement {
                        bytes,
                        executable: *executable,
                    },
                )]);
                let plan = workspace
                    .preview_conflict_replacements(
                        &workspace.layer_named(layer).map_err(error)?,
                        &choices,
                        Default::default(),
                    )
                    .map_err(error)?;
                if *preview {
                    json!({"resultStep":plan.bundle().step,"conflicts":plan.bundle().conflicts})
                } else {
                    json!({"resolution":workspace.accept_conflict_resolution(&plan).map_err(error)?})
                }
            }
            CliCommand::OperationHistory {
                search,
                actor,
                since,
                until,
                cursor,
                limit,
            } => {
                let filter = layrs_client_core::native_scm::OperationHistoryFilter {
                    actor: actor
                        .as_ref()
                        .map(layrs_core::scm::ActorId::parse)
                        .transpose()
                        .map_err(error)?,
                    since: since
                        .as_ref()
                        .map(layrs_core::scm::Timestamp::parse)
                        .transpose()
                        .map_err(error)?,
                    until: until
                        .as_ref()
                        .map(layrs_core::scm::Timestamp::parse)
                        .transpose()
                        .map_err(error)?,
                };
                let cursor = cursor
                    .as_ref()
                    .map(|v| serde_json::from_str(v))
                    .transpose()
                    .map_err(error)?;
                json!(
                    workspace
                        .operation_history_filtered(
                            cursor.as_ref(),
                            *limit as usize,
                            search,
                            &filter
                        )
                        .map_err(error)?
                )
            }
            CliCommand::LocalIdentity {
                actor,
                device,
                name,
            } => {
                let identity = match actor {
                    Some(actor) => Some(
                        workspace
                            .configure_identity(layrs_client_core::native_scm::LocalIdentity {
                                actor_id: layrs_core::scm::ActorId::parse(actor).map_err(error)?,
                                device_id: device
                                    .as_ref()
                                    .map(layrs_core::scm::DeviceId::parse)
                                    .transpose()
                                    .map_err(error)?,
                                display_name: name.clone(),
                            })
                            .map_err(error)?,
                    ),
                    None => workspace.local_identity().map_err(error)?,
                };
                json!({"identity": identity})
            }
            CliCommand::History {
                selector,
                path,
                actor,
                since,
                until,
                cursor,
                limit,
            } => {
                let cursor = cursor
                    .as_ref()
                    .map(|value| {
                        serde_json::from_str::<layrs_client_core::native_scm::StepHistoryCursor>(
                            value,
                        )
                    })
                    .transpose()
                    .map_err(error)?;
                json!(
                    workspace
                        .step_history_filtered(
                            selector,
                            cursor.as_ref(),
                            *limit as usize,
                            path.as_deref(),
                            &layrs_client_core::native_scm::StepHistoryFilter {
                                actor: actor
                                    .as_ref()
                                    .map(layrs_core::scm::ActorId::parse)
                                    .transpose()
                                    .map_err(error)?,
                                since: since
                                    .as_ref()
                                    .map(layrs_core::scm::Timestamp::parse)
                                    .transpose()
                                    .map_err(error)?,
                                until: until
                                    .as_ref()
                                    .map(layrs_core::scm::Timestamp::parse)
                                    .transpose()
                                    .map_err(error)?,
                            }
                        )
                        .map_err(error)?
                )
            }
            CliCommand::Rewrite {
                kind,
                source,
                mainline,
                preview,
            } => {
                let source = workspace.resolve_step_name(source).map_err(error)?.step_id;
                let mainline = mainline
                    .as_ref()
                    .map(|id| workspace.resolve_step_name(id).map(|r| r.step_id))
                    .transpose()
                    .map_err(error)?;
                let plan = match kind.as_str() {
                    "replay" if *preview => workspace.preview_replay(&source, mainline.as_ref()),
                    "revert" if *preview => workspace.preview_revert(&source, mainline.as_ref()),
                    "squash" if *preview => workspace.preview_squash(&source),
                    "replay" => workspace.prepare_replay(&source, mainline.as_ref()),
                    "revert" => workspace.prepare_revert(&source, mainline.as_ref()),
                    "squash" => workspace.prepare_squash(&source),
                    _ => unreachable!(),
                }
                .map_err(error)?;
                if *preview {
                    json!({"preview":plan.summary(),"conflicts":plan.conflicts()})
                } else {
                    json!({"stepId":workspace.apply_rewrite(&plan).map_err(error)?,"rewrite":plan.summary()})
                }
            }
            CliCommand::GitImport {
                source,
                timeout_seconds,
                references,
                ..
            } => {
                let plan = layrs_git_bridge::scan_git_selected_with_progress(
                    source,
                    &bridge_selection(references),
                    bridge_limits(*timeout_seconds),
                    &mut bridge_progress(),
                )
                .map_err(error)?;
                let outcome = plan.install(&workspace).map_err(error)?;
                json!({"import":outcome,"report":plan.report()})
            }
            CliCommand::GitImportCache {
                checkpoint,
                digest,
                timeout_seconds,
                ..
            } => {
                let plan = layrs_git_bridge::resume_git_scan_checkpoint(
                    checkpoint,
                    digest,
                    bridge_limits(*timeout_seconds),
                    &mut bridge_progress(),
                )
                .map_err(error)?;
                let outcome = plan.install(&workspace).map_err(error)?;
                json!({"import":outcome,"report":plan.report()})
            }
            CliCommand::GitExport {
                destination,
                snapshot,
                resume,
                timeout_seconds,
                references,
            } => {
                let mode = match snapshot {
                    Some(id) => layrs_git_bridge::ExportMode::Snapshot {
                        step_id: StepId::parse(id).map_err(error)?,
                        reference: "refs/heads/main".into(),
                    },
                    None if references.is_empty() => layrs_git_bridge::ExportMode::History,
                    None => layrs_git_bridge::ExportMode::SelectedHistory {
                        references: references.clone(),
                    },
                };
                let report = if *resume {
                    layrs_git_bridge::resume_export_workspace(
                        &workspace,
                        destination,
                        mode,
                        bridge_limits(*timeout_seconds),
                        &mut bridge_progress(),
                    )
                } else {
                    layrs_git_bridge::export_workspace_with_progress(
                        &workspace,
                        destination,
                        mode,
                        bridge_limits(*timeout_seconds),
                        &mut bridge_progress(),
                    )
                }
                .map_err(error)?;
                json!({"export":report})
            }
            CliCommand::Status => {
                let repair = workspace.needs_repair().map_err(error)?;
                let dirty = if repair {
                    None
                } else {
                    Some(workspace.is_dirty().map_err(error)?)
                };
                json!({"dirty":dirty,"needsRepair":repair,"state":workspace.state().map_err(error)?})
            }
            CliCommand::Step => match workspace.capture().map_err(error)? {
                CapturePlan::Unchanged { step_id } => {
                    json!({"status":"unchanged","stepId":step_id})
                }
                CapturePlan::Change(change) => {
                    json!({"status":"captured","stepId":change.step_id()})
                }
            },
            CliCommand::Timeline { limit } => {
                json!({"operations":workspace.operations(limit.unwrap_or(100) as usize).map_err(error)?})
            }
            CliCommand::Layers => json!({"layers":workspace.state().map_err(error)?.layers}),
            CliCommand::LayerCreate { name } => {
                json!({"layer":workspace.create_layer(name).map_err(error)?})
            }
            CliCommand::LayerUse { name_or_id } => {
                json!({"stepId":workspace.switch_layer(name_or_id).map_err(error)?})
            }
            CliCommand::Restore {
                step,
                path,
                preview,
                expected,
            } => {
                let target = workspace.resolve_step_name(step).map_err(error)?.step_id;
                if *preview {
                    json!({"preview":workspace.preview_restore_step(&target,path.as_deref()).map_err(error)?})
                } else if let Some(expected) = expected {
                    if expected.len() > 4096 {
                        return Err(CliError::usage("restore expectation exceeds 4096 bytes"));
                    }
                    let guard = serde_json::from_str(expected).map_err(error)?;
                    json!({"stepId":workspace.restore_step_guarded(&target,path.as_deref(),&guard).map_err(error)?})
                } else {
                    json!({"stepId":workspace.restore_step(&target,path.as_deref()).map_err(error)?})
                }
            }
            CliCommand::RestoreView {
                operation,
                preview,
                expected,
            } => {
                let target = OperationId::parse(operation).map_err(error)?;
                if *preview {
                    let checkout = workspace
                        .state()
                        .map_err(error)?
                        .worktree
                        .checkout_root()
                        .to_owned();
                    json!({"preview":layrs_client_core::native_scm::execute_workspace(checkout, layrs_client_core::native_scm::WorkspaceAction::PreviewView { operation: target }).map_err(error)?})
                } else if let Some(expected) = expected {
                    if expected.len() > 4096 {
                        return Err(CliError::usage("restore expectation exceeds 4096 bytes"));
                    }
                    let guard = serde_json::from_str(expected).map_err(error)?;
                    json!({"operationId":workspace.restore_operation_guarded(&target,&guard).map_err(error)?})
                } else {
                    json!({"operationId":workspace.restore_operation(&target,OperationKind::Restore).map_err(error)?})
                }
            }
            CliCommand::Undo => json!({"operationId":workspace.undo().map_err(error)?}),
            CliCommand::Redo { operation } => {
                let id = operation
                    .as_ref()
                    .map(OperationId::parse)
                    .transpose()
                    .map_err(error)?;
                json!({"operationId":workspace.redo(id.as_ref()).map_err(error)?})
            }
            CliCommand::Recover => json!({"recovered":workspace.recover().map_err(error)?}),
            CliCommand::Markers => json!({"markers":workspace.markers().map_err(error)?}),
            CliCommand::MarkerShow { id } => {
                json!({"marker":workspace.show_marker(&MarkerId::parse(id).map_err(error)?).map_err(error)?})
            }
            CliCommand::MarkerCreate { name } => {
                json!({"marker":workspace.create_marker(name,"").map_err(error)?})
            }
            CliCommand::MarkerHide { id } => {
                workspace
                    .hide_marker(&MarkerId::parse(id).map_err(error)?)
                    .map_err(error)?;
                json!({"hidden":id})
            }
            CliCommand::Releases => json!({"releases":workspace.releases().map_err(error)?}),
            CliCommand::ReleaseCreate { version } => {
                json!({"release":workspace.create_release(version,"").map_err(error)?})
            }
            _ => {
                return Err(CliError::runtime(
                    "This command is not yet connected to the native SCM engine.",
                ));
            }
        };
        let human = human_output(&cli.command, &data);
        Ok(Rendered { human, data })
    })())
}
fn error(e: impl std::fmt::Display) -> CliError {
    CliError::runtime(e.to_string())
}

fn human_output(command: &CliCommand, data: &serde_json::Value) -> String {
    if let Some(output) = preview_output::render(command, data) {
        return output;
    }
    match command {
        CliCommand::OperationHistory { .. } => {
            let mut output = rows(data, "operations", "summary", "operationId");
            if !data["next"].is_null() {
                output.push_str(&format!("\nNext cursor: {}", data["next"]));
            }
            output
        }
        CliCommand::History { .. } => {
            let mut output = data["steps"]
                .as_array()
                .map(|steps| {
                    steps
                        .iter()
                        .map(|step| {
                            format!(
                                "{}  parents: {}",
                                step["stepId"].as_str().unwrap_or(""),
                                step["parentStepIds"]
                                    .as_array()
                                    .map(|parents| parents
                                        .iter()
                                        .filter_map(|p| p.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", "))
                                    .unwrap_or_default()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            if !data["next"].is_null() {
                output.push_str(&format!("\nNext cursor: {}", data["next"]));
            }
            output
        }
        CliCommand::Status => format!(
            "{} — {}\n{}",
            data["state"]["space"]["name"].as_str().unwrap_or("Project"),
            if data["needsRepair"] == true {
                "Interrupted write: run layrs recover"
            } else if data["dirty"] == true {
                "Changes to capture"
            } else {
                "Working tree clean"
            },
            data["state"]["worktree"]["checkoutRoot"]
                .as_str()
                .unwrap_or("")
        ),
        CliCommand::Step => format!(
            "{} {}",
            data["status"].as_str().unwrap_or("Captured"),
            data["stepId"].as_str().unwrap_or("")
        ),
        CliCommand::Layers => rows(data, "layers", "name", "layerId"),
        CliCommand::ResolveBlock { .. } => blocks::render(data),
        CliCommand::Markers => rows(data, "markers", "name", "targetStepId"),
        CliCommand::Releases => rows(data, "releases", "version", "targetStepId"),
        CliCommand::Timeline { .. } => rows(data, "operations", "summary", "operationId"),
        CliCommand::Recover => format!("Recovered {} interrupted operation(s).", data["recovered"]),
        CliCommand::ConflictVariant { against, .. } => {
            if against.is_none() {
                format!(
                    "{}\n{}\n{}",
                    data["termId"].as_str().unwrap_or(""),
                    data["path"].as_str().unwrap_or(""),
                    data["text"]
                        .as_str()
                        .unwrap_or("Text unavailable (binary, structural value or size limit).")
                )
            } else if data["available"] != true {
                "Text comparison unavailable (binary, structural value or size limit). Use --json to inspect exact variant identities.".into()
            } else {
                let mut output = format!(
                    "{} -> {}\n",
                    data["beforeTerm"].as_str().unwrap_or(""),
                    data["afterTerm"].as_str().unwrap_or("")
                );
                for change in data["changes"].as_array().into_iter().flatten() {
                    output.push_str(change["path"].as_str().unwrap_or(""));
                    output.push('\n');
                    if change["textOmitted"] == true {
                        output.push_str("Text diff exceeds preview limits.\n");
                    }
                    if change["executableChanged"] == true {
                        output.push_str("Executable mode changed.\n");
                    }
                    for line in change["textDiff"].as_array().into_iter().flatten() {
                        output.push(match line["op"].as_str() {
                            Some("insert") => '+',
                            Some("delete") => '-',
                            _ => ' ',
                        });
                        let text = line["text"].as_str().unwrap_or("");
                        output.push_str(text);
                        if !text.ends_with('\n') {
                            output.push_str("\n[No final newline]\n");
                        }
                    }
                }
                output
            }
        }
        _ => format!(
            "Done. {}",
            data.get("operationId")
                .or_else(|| data.get("stepId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
        ),
    }
}
fn rows(data: &serde_json::Value, key: &str, label: &str, id: &str) -> String {
    data[key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|v| {
                    format!(
                        "{}  {}",
                        v[label].as_str().unwrap_or(""),
                        v[id].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or("None.".into())
}
