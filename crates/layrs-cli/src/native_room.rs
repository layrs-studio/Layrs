//! `layrs room` (ADR 0025): lightweight working folders, virtual by default.
use crate::{
    args::{Cli, RoomCommand, RoomLocationAction},
    error::CliError,
    render::Rendered,
};
use layrs_client_core::native_scm::{
    NativeWorkspace, RoomKind, RoomRecord, RoomsLocationSource, room_layer_name,
    set_user_rooms_location, user_rooms_location, user_rooms_location_file, validate_room_name,
    virtual_room_origin,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Where a Room command runs: the project that owns the Store (and the Room
/// registry), and the Room the current folder belongs to, if any.
struct Context {
    project: NativeWorkspace,
    current_room: Option<String>,
    /// The project's state as discovery read it (Layer names and ids, the
    /// project folder): read once per command.
    project_state: layrs_client_core::native_scm::WorkspaceState,
}

fn context(cli: &Cli) -> Result<Context, CliError> {
    let start = cli
        .globals
        .space
        .clone()
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .map_err(error)?;
    let start = std::path::absolute(&start).map_err(error)?;
    // Each discovery's opening session is released at once (a Room
    // command may write); only the state it read is kept.
    let discover = |root: &Path| {
        NativeWorkspace::discover_session(root)
            .map(|(workspace, _, state)| (workspace, state))
            .map_err(error)
    };
    if let Some((room_root, project_root)) = virtual_room_origin(&start).map_err(error)? {
        let (project, project_state) = discover(&project_root)?;
        let current_room = room_at(&project, &room_root)?;
        return Ok(Context {
            project,
            current_room,
            project_state,
        });
    }
    let (here, here_state) = discover(&start)?;
    // A physical Room is a Worktree of the same Store: always act from the
    // project folder that owns the Store, so merges reach its files.
    let project_root = here
        .store_root()
        .parent()
        .ok_or_else(|| CliError::runtime("Store has no project folder"))?
        .to_path_buf();
    let here_root = PathBuf::from(here_state.worktree.checkout_root());
    let current_room = room_at(&here, &here_root)?;
    let (project, project_state) = if same_path(&project_root, &here_root) {
        (here, here_state)
    } else {
        discover(&project_root)?
    };
    Ok(Context {
        project,
        current_room,
        project_state,
    })
}

/// `layrs room location`: applies a change to this project's or the user's
/// choice, then shows where new Rooms go and every level's setting.
fn location(
    project: Option<&NativeWorkspace>,
    action: &RoomLocationAction,
    global: bool,
) -> Result<Rendered, CliError> {
    let chosen = match action {
        RoomLocationAction::Show => None,
        RoomLocationAction::Set(dir) => Some(Some(dir.as_path())),
        RoomLocationAction::Reset => Some(None),
    };
    if let Some(chosen) = chosen {
        if global {
            set_user_rooms_location(chosen).map_err(error)?;
        } else {
            project
                .ok_or_else(|| CliError::usage("run this from a project, or add --global"))?
                .set_project_rooms_location(chosen)
                .map_err(error)?;
        }
    }
    let user = user_rooms_location().map_err(error)?;
    let user_file = user_rooms_location_file().map_err(error)?;
    let text = |path: &Option<std::path::PathBuf>| {
        path.as_ref()
            .map_or_else(|| "not set".to_owned(), |p| p.display().to_string())
    };
    let Some(project) = project else {
        return Ok(Rendered {
            human: format!(
                "Your Rooms location (every project): {}\n  setting: {}",
                text(&user),
                user_file.display()
            ),
            data: json!({ "user": user, "userSetting": user_file }),
        });
    };
    let state = project.state().map_err(error)?;
    let effective = project
        .rooms_location(state.space.name(), state.space.id().as_str())
        .map_err(error)?;
    let own = project.project_rooms_location().map_err(error)?;
    let why = match effective.source {
        RoomsLocationSource::Project => "this project's choice",
        RoomsLocationSource::User => "your choice for every project",
        RoomsLocationSource::Default => "default",
    };
    Ok(Rendered {
        human: format!(
            "New Rooms go to: {} ({why})\n  this project: {}\n  every project: {} ({})",
            effective.parent.display(),
            text(&own),
            text(&user),
            user_file.display()
        ),
        data: json!({
            "location": effective,
            "project": own,
            "user": user,
            "userSetting": user_file,
        }),
    })
}

fn room_at(workspace: &NativeWorkspace, root: &Path) -> Result<Option<String>, CliError> {
    Ok(workspace
        .rooms()
        .map_err(error)?
        .into_iter()
        .find(|room| same_path(Path::new(&room.path), root))
        .map(|room| room.name))
}

fn same_path(a: &Path, b: &Path) -> bool {
    let normalize = |p: &Path| {
        let text = p.to_string_lossy().trim_end_matches(['\\', '/']).to_owned();
        if cfg!(windows) {
            text.to_lowercase()
        } else {
            text
        }
    };
    normalize(a) == normalize(b)
}

fn selected(context: &Context, name: Option<&String>) -> Result<RoomRecord, CliError> {
    let name = name
        .cloned()
        .or_else(|| context.current_room.clone())
        .ok_or_else(|| CliError::usage("name the Room, or run this command from inside it"))?;
    context.project.room(&name).map_err(error)
}

pub fn run(cli: &Cli, command: &RoomCommand) -> Result<Rendered, CliError> {
    // The user's choice applies to every project: no project is needed.
    if let RoomCommand::Location {
        action,
        global: true,
    } = command
    {
        if !matches!(action, RoomLocationAction::Show) {
            return location(None, action, true);
        }
    }
    let span = layrs_store_local::perf::span("room.context");
    let context = context(cli)?;
    drop(span);
    match command {
        RoomCommand::Location { action, global } => {
            location(Some(&context.project), action, *global)
        }
        RoomCommand::List => list(&context.project),
        RoomCommand::Path { name } => {
            let room = context.project.room(name).map_err(error)?;
            Ok(Rendered {
                human: room.path.clone(),
                data: json!({ "room": room }),
            })
        }
        RoomCommand::New {
            name,
            physical,
            dir,
        } => new(cli, &context.project, name, *physical, dir.as_deref()),
        RoomCommand::Status { name } => {
            let mut room = selected(&context, name.as_ref())?;
            let status = status(cli, &context.project, &mut room)?;
            Ok(Rendered {
                human: format!(
                    "Room {} ({}) on Layer {}\n{}\nStep {}",
                    room.name,
                    kind_text(room.kind),
                    room.layer_name,
                    room.path,
                    status["stepId"].as_str().unwrap_or("?")
                ),
                data: json!({ "room": room, "status": status }),
            })
        }
        RoomCommand::Step { name } => {
            let mut room = selected(&context, name.as_ref())?;
            let step = step(cli, &context.project, &mut room)?;
            Ok(Rendered {
                human: if step["changed"] == true {
                    format!(
                        "Room {}: new Step {}",
                        room.name,
                        step["stepId"].as_str().unwrap_or("?")
                    )
                } else {
                    format!("Room {}: no change", room.name)
                },
                data: json!({ "room": room, "step": step }),
            })
        }
        RoomCommand::Merge {
            name,
            into,
            preview,
        } => {
            let span = layrs_store_local::perf::span("room.selected");
            let mut room = selected(&context, name.as_ref())?;
            drop(span);
            merge(
                cli,
                &context.project,
                &context.project_state,
                &mut room,
                into.as_deref(),
                *preview,
            )
        }
        RoomCommand::Drop { name, force } => {
            let room = selected(&context, name.as_ref())?;
            drop_room(cli, &context.project, room, *force)
        }
    }
}

fn list(workspace: &NativeWorkspace) -> Result<Rendered, CliError> {
    let rooms = workspace.rooms().map_err(error)?;
    let human = if rooms.is_empty() {
        "No Room yet. Start one with `layrs room new NAME`.".to_owned()
    } else {
        rooms
            .iter()
            .map(|room| {
                format!(
                    "{:<24} {:<8} {:<28} {}",
                    room.name,
                    kind_text(room.kind),
                    room.layer_name,
                    room.path
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(Rendered {
        human,
        data: json!({ "rooms": rooms }),
    })
}

fn new(
    cli: &Cli,
    workspace: &NativeWorkspace,
    name: &str,
    physical: bool,
    dir: Option<&Path>,
) -> Result<Rendered, CliError> {
    validate_room_name(name).map_err(error)?;
    if workspace.room(name).is_ok() {
        return Err(CliError::usage(format!("Room {name} already exists")));
    }
    let state = workspace.state().map_err(error)?;
    let source_layer_id = state
        .view
        .worktrees()
        .iter()
        .find(|worktree| worktree.worktree_id() == workspace.worktree_id())
        .and_then(|worktree| worktree.active_layer_id())
        .map(|layer| layer.as_str().to_owned());
    let path = match dir {
        Some(dir) => std::path::absolute(dir).map_err(error)?,
        None => free_room_folder(
            &workspace
                .rooms_location(state.space.name(), state.space.id().as_str())
                .map_err(error)?
                .parent,
            name,
        ),
    };
    prepare_empty_folder(&path)?;
    let path_text = path
        .to_str()
        .ok_or_else(|| CliError::usage("Room path must be UTF-8"))?
        .to_owned();
    // The Room's Layer starts at this folder's current Step (unsaved work is
    // captured first, as for any Layer creation).
    // A dropped Room keeps its Layer (and history): a new Room of the same
    // name gets the next free Layer name.
    let taken: std::collections::BTreeSet<String> = state
        .layers
        .iter()
        .map(|layer| layer.name().to_owned())
        .collect();
    let layer_name = std::iter::once(room_layer_name(name))
        .chain((2..1000).map(|n| format!("{}-{n}", room_layer_name(name))))
        .find(|candidate| !taken.contains(candidate))
        .ok_or_else(|| CliError::usage("no free Layer name for this Room"))?;
    let layer = workspace.create_layer(&layer_name).map_err(error)?;
    let base_step_id = own_base_step(workspace)?;
    let virtual_room = !physical && cfg!(windows);
    let (kind, worktree_id, ticket) = if virtual_room {
        let data =
            crate::native_vfs::create_room_host(cli, &path, layer.id().as_str(), &base_step_id)?;
        (
            RoomKind::Virtual,
            data["ticket"]["worktreeId"].as_str().map(str::to_owned),
            Some(data["ticket"].clone()),
        )
    } else {
        // Physical Rooms keep a change index (ADR 0025): the files written
        // here are recognised by later captures without being reopened.
        let step = layrs_core::scm::StepId::parse(base_step_id.clone())
            .map_err(|e| CliError::runtime(e.to_string()))?;
        let created = workspace
            .create_room_worktree(&path, layer.id(), &step)
            .map_err(error)?;
        (
            RoomKind::Physical,
            Some(created.worktree_id().as_str().to_owned()),
            None,
        )
    };
    let record = RoomRecord {
        name: name.to_owned(),
        kind,
        path: path_text.clone(),
        layer_id: layer.id().as_str().to_owned(),
        layer_name: layer_name.clone(),
        source_layer_id,
        base_step_id,
        worktree_id,
        ticket,
        created_at: unix_millis(),
        merged_step_id: None,
    };
    workspace.record_room(&record).map_err(error)?;
    let note = if !physical && !virtual_room {
        "\n(virtual Rooms need Windows today: this one is a physical folder)"
    } else {
        ""
    };
    Ok(Rendered {
        human: format!(
            "Room {name} ready ({}) on Layer {layer_name}\n{path_text}{note}",
            kind_text(kind)
        ),
        data: json!({ "room": record }),
    })
}

fn own_base_step(workspace: &NativeWorkspace) -> Result<String, CliError> {
    workspace
        .state()
        .map_err(error)?
        .view
        .worktrees()
        .iter()
        .find(|worktree| worktree.worktree_id() == workspace.worktree_id())
        .and_then(|worktree| worktree.base_step_id())
        .map(|step| step.as_str().to_owned())
        .ok_or_else(|| CliError::runtime("folder has no base Step"))
}

/// Sends a command to a virtual Room's VFS host, starting the host first
/// (and recording its new ticket) if it is not running.
#[cfg(windows)]
fn host_request(
    cli: &Cli,
    project: &NativeWorkspace,
    room: &mut RoomRecord,
    command: layrs_client_core::native_scm::VfsHostCommand,
) -> Result<Value, CliError> {
    use layrs_client_core::native_scm::{
        VfsControlRequest, VfsHostClient, VfsHostCommand, VfsHostResponse, VfsHostTicket,
        execute_vfs_control,
    };
    let connected = |ticket: &VfsHostTicket| {
        VfsHostClient::open(ticket)
            .and_then(|client| {
                client.request_with_timeout(
                    VfsHostCommand::Status {},
                    std::time::Duration::from_secs(3),
                )
            })
            .is_ok_and(|response| {
                matches!(response, VfsHostResponse::Ok { ref value } if value["connected"] == true)
            })
    };
    let mut ticket: Option<VfsHostTicket> = room
        .ticket
        .clone()
        .and_then(|ticket| serde_json::from_value(ticket).ok());
    // The recorded host answers directly. Only when that fails is it probed:
    // a live, connected host's error is the answer; otherwise it is
    // replaced below (a request it may have executed is safe to repeat:
    // captures are idempotent, a stop of a gone host is moot).
    if let Some(recorded) = &ticket {
        match execute_vfs_control(VfsControlRequest {
            ticket: recorded.clone(),
            command: command.clone(),
        }) {
            Ok(value) => return Ok(value),
            Err(message) if message.starts_with("invalid_vfs_control_request") => {}
            Err(message) if connected(recorded) => return Err(error(message)),
            Err(_) => {}
        }
    }
    if !ticket.as_ref().is_some_and(connected) {
        let data = crate::native_vfs::start_room_host(cli, Path::new(&room.path))?;
        room.ticket = Some(data["ticket"].clone());
        project.update_room(room).map_err(error)?;
        ticket = Some(
            serde_json::from_value(data["ticket"].clone())
                .map_err(|_| CliError::runtime("VFS host returned an invalid ticket"))?,
        );
    }
    let ticket = ticket.expect("ticket set above");
    match execute_vfs_control(VfsControlRequest {
        ticket: ticket.clone(),
        command: command.clone(),
    }) {
        // A host started by an older layrs does not know this request: stop
        // it (its work stays in the folder), start one from this executable
        // and retry once.
        Err(message) if message.starts_with("invalid_vfs_control_request") => {
            let _ = execute_vfs_control(VfsControlRequest {
                ticket,
                command: VfsHostCommand::Stop {},
            });
            let data = crate::native_vfs::start_room_host(cli, Path::new(&room.path))?;
            room.ticket = Some(data["ticket"].clone());
            project.update_room(room).map_err(error)?;
            let ticket: VfsHostTicket = serde_json::from_value(data["ticket"].clone())
                .map_err(|_| CliError::runtime("VFS host returned an invalid ticket"))?;
            execute_vfs_control(VfsControlRequest { ticket, command }).map_err(error)
        }
        other => other.map_err(error),
    }
}

fn status(cli: &Cli, project: &NativeWorkspace, room: &mut RoomRecord) -> Result<Value, CliError> {
    match room.kind {
        #[cfg(windows)]
        RoomKind::Virtual => {
            let value = host_request(
                cli,
                project,
                room,
                layrs_client_core::native_scm::VfsHostCommand::Status {},
            )?;
            Ok(json!({ "stepId": value["acceptedStepId"], "host": value }))
        }
        #[cfg(not(windows))]
        RoomKind::Virtual => {
            let _ = (cli, project);
            Err(CliError::runtime("virtual Rooms need Windows today"))
        }
        RoomKind::Physical => {
            let _ = cli;
            let room_workspace = NativeWorkspace::discover(&room.path).map_err(error)?;
            let dirty = room_workspace.is_dirty().map_err(error)?;
            Ok(json!({ "stepId": own_base_step(&room_workspace)?, "dirty": dirty }))
        }
    }
}

/// Captures the Room's current files as a Step of its Layer.
fn step(cli: &Cli, project: &NativeWorkspace, room: &mut RoomRecord) -> Result<Value, CliError> {
    match room.kind {
        #[cfg(windows)]
        RoomKind::Virtual => {
            // A Room command is issued by its user/agent between edits: the
            // writers are its own and are quiescent while it runs.
            let value = host_request(
                cli,
                project,
                room,
                layrs_client_core::native_scm::VfsHostCommand::Capture {
                    writers_quiesced: true,
                },
            )?;
            let step_id = match value["stepId"].as_str() {
                Some(step) => Value::String(step.to_owned()),
                None => status(cli, project, room)?["stepId"].clone(),
            };
            Ok(
                json!({ "changed": value["changed"] == true, "stepId": step_id, "counts": value["counts"] }),
            )
        }
        #[cfg(not(windows))]
        RoomKind::Virtual => {
            let _ = (cli, project);
            Err(CliError::runtime("virtual Rooms need Windows today"))
        }
        RoomKind::Physical => {
            let _ = cli;
            let room_workspace = NativeWorkspace::discover(&room.path).map_err(error)?;
            let plan = room_workspace.capture().map_err(error)?;
            let (changed, step_id) = match &plan {
                layrs_client_core::native_scm::CapturePlan::Unchanged { step_id } => {
                    (false, step_id.as_str().to_owned())
                }
                layrs_client_core::native_scm::CapturePlan::Change(change) => {
                    (true, change.step_id().as_str().to_owned())
                }
            };
            Ok(json!({ "changed": changed, "stepId": step_id }))
        }
    }
}

fn layer_name_of(
    state: &layrs_client_core::native_scm::WorkspaceState,
    id: &str,
) -> Result<String, CliError> {
    state
        .layers
        .iter()
        .find(|layer| layer.id().as_str() == id)
        .map(|layer| layer.name().to_owned())
        .ok_or_else(|| CliError::runtime("the Room's source Layer is no longer visible"))
}

fn merge(
    cli: &Cli,
    project: &NativeWorkspace,
    project_state: &layrs_client_core::native_scm::WorkspaceState,
    room: &mut RoomRecord,
    into: Option<&str>,
    preview: bool,
) -> Result<Rendered, CliError> {
    let span = layrs_store_local::perf::span("room.merge.target");
    let target = match into {
        Some(layer) => layer.to_owned(),
        None => layer_name_of(
            project_state,
            room.source_layer_id.as_deref().ok_or_else(|| {
                CliError::usage("this Room has no source Layer: pass --into LAYER")
            })?,
        )?,
    };
    drop(span);
    let span = layrs_store_local::perf::span("room.merge.capture_room");
    let captured = step(cli, project, room)?;
    drop(span);
    let head = captured["stepId"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| CliError::runtime("the Room has no captured Step"))?;
    if head == room.base_step_id || room.merged_step_id.as_deref() == Some(&head) {
        return Ok(Rendered {
            human: format!("Room {}: nothing new to merge into {target}", room.name),
            data: json!({ "room": room, "target": target, "merged": false, "conflicts": [] }),
        });
    }
    // Fast path: the target has not moved since the Room forked (or since its
    // last merge), so the target simply advances to the Room's Step and only
    // the changed files are written. Otherwise a full Weave reconciles both.
    // Layer names and ids are those discovery read: the Room's capture
    // changes only its own Layer's head.
    let target_id = NativeWorkspace::layer_named_in(project_state, &target).map_err(error)?;
    let expected = room
        .merged_step_id
        .clone()
        .unwrap_or_else(|| room.base_step_id.clone());
    let parse = |step: &str| {
        layrs_core::scm::StepId::parse(step.to_owned())
            .map_err(|e| CliError::runtime(e.to_string()))
    };
    if !preview {
        let span = layrs_store_local::perf::span("room.merge.fast_forward_attempt");
        let attempt = project.fast_forward_layer(&target_id, &parse(&expected)?, &parse(&head)?);
        drop(span);
        match attempt {
            Ok(materialized) => {
                room.merged_step_id = Some(head.clone());
                let _span = layrs_store_local::perf::span("room.merge.update_room");
                project.update_room(room).map_err(error)?;
                return Ok(Rendered {
                    human: format!(
                        "Room {} merged into {target} (fast-forward to {head}){}",
                        room.name,
                        if materialized {
                            "; project files updated"
                        } else {
                            ""
                        }
                    ),
                    data: json!({ "room": room, "target": target, "merged": true, "fastForward": true, "materialized": materialized, "stepId": head }),
                });
            }
            Err(layrs_client_core::native_scm::NativeClientError::NeedsIntegration) => {}
            Err(e) => return Err(error(e)),
        }
    }
    // Each folder sees Layer heads through its own last Operation: name the
    // Room's exact captured Step as the source, not its Layer.
    // The fast-forward attempt above already captured the project folder;
    // acceptance still rechecks that folder before writing anything.
    let span = layrs_store_local::perf::span("room.merge.weave_prepare");
    let plan = if preview {
        project.preview_named_weave(&head, &target)
    } else {
        project.prepare_named_weave_after_capture(&head, &target)
    }
    .map_err(error)?;
    drop(span);
    let conflicts: Vec<Value> = plan
        .plan()
        .conflicts()
        .values()
        .map(|conflict| serde_json::to_value(conflict).unwrap_or(Value::Null))
        .collect();
    if preview || !conflicts.is_empty() {
        let human = if conflicts.is_empty() {
            format!("Room {} merges cleanly into {target}", room.name)
        } else {
            format!(
                "Room {} conflicts with {target} on {} path(s); nothing was merged. Resolve with `layrs weave {} --target {target}`.",
                room.name,
                conflicts.len(),
                room.layer_name
            )
        };
        return Ok(Rendered {
            human,
            data: json!({ "room": room, "target": target, "conflicts": conflicts, "merged": false }),
        });
    }
    let span = layrs_store_local::perf::span("room.merge.weave_accept");
    let accepted = project.accept_merge(&plan).map_err(error)?;
    drop(span);
    room.merged_step_id = Some(head);
    project.update_room(room).map_err(error)?;
    Ok(Rendered {
        human: format!(
            "Room {} merged into {target} (Step {}){}",
            room.name,
            accepted.step_id,
            if accepted.materialized {
                "; project files updated"
            } else {
                ""
            }
        ),
        data: json!({ "room": room, "target": target, "merged": true, "weave": accepted }),
    })
}

fn drop_room(
    cli: &Cli,
    project: &NativeWorkspace,
    mut room: RoomRecord,
    force: bool,
) -> Result<Rendered, CliError> {
    // Whatever the folder holds is captured first: dropping never loses work,
    // it stays on the Room's Layer. Captures trust the change index (ADR
    // 0025); before deleting anything, every local byte is verified instead.
    // One capture, the verifying one: it is strictly stronger than an
    // index-trusting step, and whatever it finds is kept on the Layer.
    let span = layrs_store_local::perf::span("room.drop.verify_all");
    let head = verify_all(cli, project, &mut room)?;
    drop(span);
    let settled = head == room.base_step_id || room.merged_step_id.as_deref() == Some(&head);
    if !settled && !force {
        return Err(CliError::usage(format!(
            "Room {} has work not merged yet (kept on Layer {}). Merge it with `layrs room merge {}`, or drop it anyway with --force (the work stays on that Layer).",
            room.name, room.layer_name, room.name
        )));
    }
    let path = PathBuf::from(&room.path);
    match room.kind {
        #[cfg(windows)]
        RoomKind::Virtual => {
            let span = layrs_store_local::perf::span("room.drop.host_stop");
            host_request(
                cli,
                project,
                &mut room,
                layrs_client_core::native_scm::VfsHostCommand::Stop {},
            )?;
            drop(span);
        }
        #[cfg(not(windows))]
        RoomKind::Virtual => {}
        RoomKind::Physical => {}
    }
    let span = layrs_store_local::perf::span("room.drop.discard_folder");
    let purging = match room.kind {
        // Unregistering a sync root walks its every placeholder (seconds at
        // 10k files): a detached process unregisters, then deletes it.
        #[cfg(windows)]
        RoomKind::Virtual => discard_virtual_folder(&path)?,
        _ => discard_folder(&path)?,
    };
    drop(span);
    let _span = layrs_store_local::perf::span("room.drop.retire");
    if let Some(worktree) = &room.worktree_id {
        project.retire_worktree(worktree).map_err(error)?;
        project.forget_change_index(worktree).map_err(error)?;
    }
    project.forget_room(&room.name).map_err(error)?;
    Ok(Rendered {
        human: format!(
            "Room {} dropped; its Steps stay on Layer {}",
            room.name, room.layer_name
        ),
        data: json!({ "room": room, "dropped": true, "unmergedKept": !settled, "purging": purging }),
    })
}

/// Hidden first argument of the detached folder deletion.
pub const PURGE_COMMAND: &str = "__layrs-purge-dropped-room";
/// Hidden first argument of the detached unregistration and deletion of a
/// dropped virtual Room's folder.
pub const PURGE_VIRTUAL_COMMAND: &str = "__layrs-purge-dropped-virtual-room";

/// Hands a stopped virtual Room's folder to a detached process that
/// unregisters it, renames it aside and deletes it. Without that process,
/// does both here.
#[cfg(windows)]
fn discard_virtual_folder(path: &Path) -> Result<Option<String>, CliError> {
    // The worker acts only on this very folder (its native identity), never
    // on whatever may occupy a path later.
    let identity = layrs_store_local::scm_v1::checkout::CheckoutReader::open(path, None)
        .map_err(error)?
        .identity();
    // Out of the way at once when the platform allows renaming the stopped
    // sync root; the worker then unregisters it under either name.
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CliError::runtime("Room folder has no UTF-8 name"))?;
    let aside = path.with_file_name(format!(".{name}{DROPPED_MARK}{}", unix_millis()));
    let renamed = std::fs::rename(path, &aside).is_ok();
    let target = if renamed {
        aside.clone()
    } else {
        path.to_path_buf()
    };
    let mut command = std::process::Command::new(std::env::current_exe().map_err(error)?);
    command
        .arg(PURGE_VIRTUAL_COMMAND)
        .arg(&target)
        .arg(&identity)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP: outlives this console.
        command.creation_flags(0x0800_0000 | 0x0000_0200);
    }
    crate::native_vfs::keep_standard_handles_private();
    if command.spawn().is_err() {
        if layrs_client_core::native_scm::unregister_virtual_room(path).is_err() {
            layrs_client_core::native_scm::unregister_virtual_room(&target).map_err(error)?;
        }
        return if renamed {
            Ok(
                (purge_dropped(&[target.to_string_lossy().into_owned()]) != 0)
                    .then(|| target.to_string_lossy().into_owned()),
            )
        } else {
            discard_folder(path)
        };
    }
    Ok(Some(target.to_string_lossy().into_owned()))
}

/// The detached half of `discard_virtual_folder`: `target` is the stopped
/// Room folder (renamed aside, or still at its `original` path). It acts
/// only on that very folder (same native identity, checked before each
/// step) still holding a virtual Room's control file: unregisters its sync
/// root, under either name, then deletes it like `purge_dropped`.
#[cfg(windows)]
pub fn purge_dropped_virtual(args: &[String]) -> i32 {
    let [target, identity, original] = args else {
        return 2;
    };
    let target = PathBuf::from(target);
    let original = PathBuf::from(original);
    let same_folder = || {
        layrs_store_local::scm_v1::checkout::CheckoutReader::open(&target, Some(identity)).is_ok()
    };
    if !target.is_absolute()
        || !same_folder()
        || !layrs_client_core::native_scm::is_virtual_room_root(&target)
    {
        return 2;
    }
    if layrs_client_core::native_scm::unregister_virtual_room(&original).is_err()
        && layrs_client_core::native_scm::unregister_virtual_room(&target).is_err()
    {
        return 1;
    }
    if !same_folder() {
        return 2;
    }
    if target != original {
        return purge_dropped(&[target.to_string_lossy().into_owned()]);
    }
    match discard_folder(&target) {
        Ok(Some(aside)) => purge_dropped(&[aside]),
        Ok(None) => 0,
        Err(_) => 1,
    }
}
const DROPPED_MARK: &str = ".dropped-";

/// Takes a dropped Room's folder out of the way at once: renamed aside on
/// its volume, then deleted by a detached process, so dropping never waits
/// for every file. A refused rename means a handle is still open inside
/// (council r5): the folder is never deleted in place, the drop fails and
/// the Room is kept. Returns the folder being deleted, if any.
fn discard_folder(path: &Path) -> Result<Option<String>, CliError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CliError::runtime("Room folder has no UTF-8 name"))?;
    let aside = path.with_file_name(format!(".{name}{DROPPED_MARK}{}", unix_millis()));
    // A VFS host that just acknowledged Stop may still be closing its handles
    // on the folder: retry the rename briefly before deleting in place.
    let mut renamed = std::fs::rename(path, &aside).is_ok();
    for attempt in 0..30 {
        if renamed || !path.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20 + 10 * attempt));
        renamed = std::fs::rename(path, &aside).is_ok();
    }
    if !renamed {
        if !path.exists() {
            return Ok(None);
        }
        return Err(CliError::runtime(format!(
            "Room folder {} is still in use (a program holds a file or folder open in it); close it and drop again. Nothing was deleted.",
            path.display()
        )));
    }
    let mut command = std::process::Command::new(std::env::current_exe().map_err(error)?);
    command
        .arg(PURGE_COMMAND)
        .arg(&aside)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP: outlives this console.
        command.creation_flags(0x0800_0000 | 0x0000_0200);
    }
    crate::native_vfs::keep_standard_handles_private();
    if command.spawn().is_err() {
        std::fs::remove_dir_all(&aside).map_err(error)?;
        return Ok(None);
    }
    Ok(Some(aside.to_string_lossy().into_owned()))
}

/// The detached deletion. It only ever deletes a folder renamed aside by
/// `discard_folder` (its name says so), retrying briefly while handles close.
pub fn purge_dropped(args: &[String]) -> i32 {
    let [path] = args else { return 2 };
    let path = PathBuf::from(path);
    let marked = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.') && name.contains(DROPPED_MARK));
    if !marked || !path.is_absolute() {
        return 2;
    }
    for attempt in 0..20 {
        match std::fs::remove_dir_all(&path) {
            Ok(()) => return 0,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return 0,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(250 * (attempt + 1))),
        }
    }
    1
}

/// Captures the Room again reading every local byte (no change-index trust)
/// and returns its resulting Step: the verification required before deleting.
fn verify_all(
    cli: &Cli,
    project: &NativeWorkspace,
    room: &mut RoomRecord,
) -> Result<String, CliError> {
    match room.kind {
        #[cfg(windows)]
        RoomKind::Virtual => {
            let value = host_request(
                cli,
                project,
                room,
                layrs_client_core::native_scm::VfsHostCommand::CaptureVerifying {},
            )?;
            match value["stepId"].as_str() {
                Some(step) => Ok(step.to_owned()),
                None => Ok(status(cli, project, room)?["stepId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()),
            }
        }
        #[cfg(not(windows))]
        RoomKind::Virtual => {
            let _ = (cli, project);
            Err(CliError::runtime("virtual Rooms need Windows today"))
        }
        RoomKind::Physical => {
            let _ = cli;
            let workspace = NativeWorkspace::discover(&room.path).map_err(error)?;
            workspace
                .capture_verifying_all(layrs_core::scm::CaptureReason::BeforeOperation)
                .map_err(error)?;
            own_base_step(&NativeWorkspace::discover(&room.path).map_err(error)?)
        }
    }
}

/// `parent/name`, or `parent/name-2`, `-3`... while that folder is taken
/// (a dropped Room's folder may still be being removed in the background).
fn free_room_folder(parent: &Path, name: &str) -> PathBuf {
    let taken =
        |path: &Path| std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some());
    let first = parent.join(name);
    if !taken(&first) {
        return first;
    }
    (2..1000)
        .map(|n| parent.join(format!("{name}-{n}")))
        .find(|path| !taken(path))
        .unwrap_or(first)
}

/// A Room never adopts an existing non-empty folder.
fn prepare_empty_folder(path: &PathBuf) -> Result<(), CliError> {
    match std::fs::read_dir(path) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(CliError::usage(format!(
                    "Room folder {} already exists and is not empty",
                    path.display()
                )));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path).map_err(error)
        }
        Err(e) => Err(error(e)),
    }
}

fn kind_text(kind: RoomKind) -> &'static str {
    match kind {
        RoomKind::Virtual => "virtual",
        RoomKind::Physical => "physical",
    }
}

fn unix_millis() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis().to_string())
        .unwrap_or_default()
}

fn error(error: impl std::fmt::Display) -> CliError {
    CliError::runtime(error.to_string())
}
