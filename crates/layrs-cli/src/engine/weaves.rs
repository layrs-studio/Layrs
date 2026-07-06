use crate::args::ConflictResolveMethod;
use layrs_client_core::access_registry as core_space;
use serde::Serialize;
use serde_json::Value;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};

use super::{CliError, ClientCoreEngine, map_core, resolve_layer_id};

impl ClientCoreEngine {
    pub fn weave(
        &self,
        source: &str,
        target: &str,
        preview: bool,
    ) -> Result<WeaveOutput, CliError> {
        let space = map_core(core_space::open_local_space(self.space_selector()?))?;
        let source_id = resolve_layer_id(&space.layers, source)?;
        let target_id = resolve_layer_id(&space.layers, target)?;
        let result = map_core(core_space::weave_layers(
            space.local_space_id,
            source_id,
            target_id,
            preview,
        ))?;
        Ok(WeaveOutput::from_result(result))
    }

    pub fn weave_parent(&self, preview: bool) -> Result<WeaveOutput, CliError> {
        let result = map_core(core_space::weave_active_layer_to_parent(
            self.space_selector()?,
            preview,
        ))?;
        Ok(WeaveOutput::from_result(result))
    }

    pub fn weave_status(&self) -> Result<Option<WeaveSessionOutput>, CliError> {
        Ok(map_core(core_space::weave_status(self.space_selector()?))?
            .map(WeaveSessionOutput::from_summary))
    }

    pub fn weave_conflicts(&self) -> Result<Vec<WeaveConflictOutput>, CliError> {
        Ok(
            map_core(core_space::weave_conflicts(self.space_selector()?))?
                .into_iter()
                .map(WeaveConflictOutput::from_summary)
                .collect(),
        )
    }

    pub fn weave_resolve(
        &self,
        path: &str,
        resolution: &str,
        file: Option<&Path>,
    ) -> Result<WeaveOutput, CliError> {
        if let Some(output) = self.weave_resolve_legacy_compat(path, resolution, file)? {
            return Ok(output);
        }
        let manual_text = if resolution.ends_with(":manual") {
            let file = file.ok_or_else(|| {
                CliError::runtime("Manual text block resolution requires --manual-text FILE.")
            })?;
            Some(read_manual_text_resolution(file)?)
        } else {
            None
        };
        let replacement_file = if manual_text.is_some() {
            None
        } else {
            file.map(|path| path.display().to_string())
        };
        let result = map_core(core_space::resolve_weave_conflict(
            self.space_selector()?,
            path.to_string(),
            resolution.to_string(),
            replacement_file,
            manual_text,
        ))?;
        Ok(WeaveOutput::from_result(result))
    }

    pub fn weave_continue(&self) -> Result<WeaveOutput, CliError> {
        let result = map_core(core_space::continue_weave(self.space_selector()?))?;
        Ok(WeaveOutput::from_result(result))
    }

    pub fn weave_abort(&self) -> Result<WeaveOutput, CliError> {
        let result = map_core(core_space::abort_weave(self.space_selector()?))?;
        Ok(WeaveOutput::from_result(result))
    }

    fn weave_resolve_legacy_compat(
        &self,
        path: &str,
        resolution: &str,
        file: Option<&Path>,
    ) -> Result<Option<WeaveOutput>, CliError> {
        if resolution.starts_with("block:") {
            return Ok(None);
        }

        let Some(session) = self.conflict_status()? else {
            return Ok(None);
        };
        let Some(conflict) = session
            .conflicts
            .into_iter()
            .find(|conflict| conflict.path == path || conflict.conflict_id == path)
        else {
            return Ok(None);
        };

        if resolution == "file" {
            let file =
                file.ok_or_else(|| CliError::runtime("Resolution --file requires a file path."))?;
            let bytes = std::fs::read(file).map_err(|error| {
                CliError::runtime(format!(
                    "Layrs could not read resolution file {}: {error}",
                    file.display()
                ))
            })?;
            return self
                .resolve_conflict_bytes_compat(path, &bytes, "file")
                .map(Some);
        }

        if resolution == "base" {
            if is_text_conflict(&conflict) {
                return self
                    .resolve_text_blocks_compat(&conflict, "manual", true)
                    .map(Some);
            }
            let bytes = self.read_conflict_side_bytes(path, "base")?;
            return self
                .resolve_conflict_bytes_compat(path, &bytes, "base")
                .map(Some);
        }

        if is_text_conflict(&conflict) {
            let choice = match resolution {
                "ours" => Some("existing"),
                "theirs" => Some("incoming"),
                _ => None,
            };
            if let Some(choice) = choice {
                return self
                    .resolve_text_blocks_compat(&conflict, choice, false)
                    .map(Some);
            }
        }

        Ok(None)
    }

    pub fn conflict_status(&self) -> Result<Option<WeaveSessionOutput>, CliError> {
        self.weave_status()
    }

    pub fn conflict_list(&self) -> Result<Vec<WeaveConflictOutput>, CliError> {
        self.weave_conflicts()
    }

    pub fn conflict_resolve(
        &self,
        method: ConflictResolveMethod,
        path: &str,
        block: Option<&str>,
    ) -> Result<WeaveOutput, CliError> {
        let conflict = self.active_conflict(path)?;
        let is_text = is_text_conflict(&conflict);
        let (resolution, manual_text) = if is_text {
            let block = block.ok_or_else(|| {
                CliError::runtime(format!(
                    "layrs conflict resolve {} requires --block for text conflicts.",
                    method.as_str()
                ))
            })?;
            let choice = conflict_text_choice(method);
            let manual_text = if method == ConflictResolveMethod::Manual {
                Some(read_manual_text_from_stdin()?)
            } else {
                None
            };
            (format!("block:{block}:{choice}"), manual_text)
        } else {
            if block.is_some() {
                return Err(CliError::runtime(format!(
                    "layrs conflict resolve {} --block cannot be used for raw conflicts.",
                    method.as_str()
                )));
            }
            (conflict_raw_choice(method)?, None)
        };

        let result = map_core(core_space::resolve_weave_conflict(
            self.space_selector()?,
            path.to_string(),
            resolution,
            None,
            manual_text,
        ))?;
        Ok(WeaveOutput::from_result(result))
    }

    pub fn conflict_continue(&self) -> Result<WeaveOutput, CliError> {
        self.weave_continue()
    }

    pub fn conflict_abort(&self) -> Result<WeaveOutput, CliError> {
        self.weave_abort()
    }

    pub fn conflict_resolve_interactive(&self) -> Result<ConflictInteractiveOutput, CliError> {
        let stdin = io::stdin();
        let mut input = stdin.lock();
        let mut output = io::stdout();
        self.conflict_resolve_interactive_with_io(&mut input, &mut output)
    }

    fn conflict_resolve_interactive_with_io<R, W>(
        &self,
        input: &mut R,
        output: &mut W,
    ) -> Result<ConflictInteractiveOutput, CliError>
    where
        R: BufRead,
        W: Write,
    {
        let mut actions = Vec::new();
        loop {
            let conflicts = self.conflict_list()?;
            let Some(next) = next_unresolved_conflict(&conflicts) else {
                write_prompt(
                    output,
                    "All conflicts are resolved. Choose conflict action: c continue, a abort, q quit > ",
                )?;
                let Some(choice) = read_prompt_line(input)? else {
                    return Ok(ConflictInteractiveOutput::new(
                        "input_ended",
                        "Interactive conflict resolution ended before continue.",
                        actions,
                        self.conflict_status()?,
                    ));
                };
                match choice.trim() {
                    "c" | "continue" => {
                        let result = self.conflict_continue()?;
                        actions.push(ConflictInteractiveAction::simple("continue"));
                        return Ok(ConflictInteractiveOutput::new(
                            "continued",
                            "Conflict session continued.",
                            actions,
                            Some(result.session),
                        ));
                    }
                    "a" | "abort" => {
                        if self.confirm_abort(input, output)? {
                            let result = self.conflict_abort()?;
                            actions.push(ConflictInteractiveAction::simple("abort"));
                            return Ok(ConflictInteractiveOutput::new(
                                "aborted",
                                "Conflict session aborted.",
                                actions,
                                Some(result.session),
                            ));
                        }
                    }
                    "q" | "quit" => {
                        actions.push(ConflictInteractiveAction::simple("quit"));
                        return Ok(ConflictInteractiveOutput::new(
                            "quit",
                            "Interactive conflict resolution quit.",
                            actions,
                            self.conflict_status()?,
                        ));
                    }
                    _ => {
                        writeln!(output, "Unknown choice.")?;
                    }
                }
                continue;
            };

            render_interactive_conflict(output, &next)?;
            let Some(choice) = read_prompt_line(input)? else {
                return Ok(ConflictInteractiveOutput::new(
                    "input_ended",
                    "Interactive conflict resolution ended.",
                    actions,
                    self.conflict_status()?,
                ));
            };
            let choice = choice.trim();
            match choice {
                "e" | "existing" => {
                    self.resolve_interactive_choice(
                        ConflictResolveMethod::Existing,
                        &next,
                        None,
                        &mut actions,
                    )?;
                }
                "i" | "incoming" => {
                    self.resolve_interactive_choice(
                        ConflictResolveMethod::Incoming,
                        &next,
                        None,
                        &mut actions,
                    )?;
                }
                "b" | "both" => {
                    if !next.is_text {
                        writeln!(output, "Both is not available for raw conflicts.")?;
                        continue;
                    }
                    self.resolve_interactive_choice(
                        ConflictResolveMethod::Both,
                        &next,
                        None,
                        &mut actions,
                    )?;
                }
                "m" | "manual" => {
                    if !next.is_text {
                        writeln!(
                            output,
                            "Manual resolution is not available for raw conflicts."
                        )?;
                        continue;
                    }
                    let manual_text = read_interactive_manual_text(input, output)?;
                    self.resolve_interactive_choice(
                        ConflictResolveMethod::Manual,
                        &next,
                        Some(manual_text),
                        &mut actions,
                    )?;
                }
                "c" | "continue" => {
                    writeln!(output, "Resolve all conflicts before continuing.")?;
                }
                "a" | "abort" => {
                    if self.confirm_abort(input, output)? {
                        let result = self.conflict_abort()?;
                        actions.push(ConflictInteractiveAction::simple("abort"));
                        return Ok(ConflictInteractiveOutput::new(
                            "aborted",
                            "Conflict session aborted.",
                            actions,
                            Some(result.session),
                        ));
                    }
                }
                "q" | "quit" => {
                    actions.push(ConflictInteractiveAction::simple("quit"));
                    return Ok(ConflictInteractiveOutput::new(
                        "quit",
                        "Interactive conflict resolution quit.",
                        actions,
                        self.conflict_status()?,
                    ));
                }
                _ => {
                    writeln!(output, "Unknown choice.")?;
                }
            }
        }
    }

    fn resolve_interactive_choice(
        &self,
        method: ConflictResolveMethod,
        conflict: &InteractiveConflict,
        manual_text: Option<String>,
        actions: &mut Vec<ConflictInteractiveAction>,
    ) -> Result<(), CliError> {
        if !conflict.is_text
            && matches!(
                method,
                ConflictResolveMethod::Both | ConflictResolveMethod::Manual
            )
        {
            return Err(CliError::runtime(format!(
                "layrs conflict resolve {} cannot be used for raw conflicts; use existing or incoming.",
                method.as_str()
            )));
        }
        let resolution = if conflict.is_text {
            let block = conflict.block_id.as_deref().ok_or_else(|| {
                CliError::runtime("Interactive text conflict is missing a block id.")
            })?;
            let choice = conflict_text_choice(method);
            format!("block:{block}:{choice}")
        } else {
            conflict_raw_choice(method)?
        };
        let result = map_core(core_space::resolve_weave_conflict(
            self.space_selector()?,
            conflict.path.clone(),
            resolution,
            None,
            manual_text,
        ))?;
        actions.push(ConflictInteractiveAction {
            action: "resolve".to_string(),
            path: Some(conflict.path.clone()),
            block: conflict.block_id.clone(),
            method: Some(method.as_str().to_string()),
        });
        if result.session.status == "resolved" {
            actions.push(ConflictInteractiveAction::simple("all_resolved"));
        }
        Ok(())
    }

    fn confirm_abort<R, W>(&self, input: &mut R, output: &mut W) -> Result<bool, CliError>
    where
        R: BufRead,
        W: Write,
    {
        write_prompt(output, "Abort active conflict session? y/N > ")?;
        let Some(answer) = read_prompt_line(input)? else {
            return Ok(false);
        };
        Ok(matches!(answer.trim(), "y" | "yes" | "Y" | "YES"))
    }

    fn active_conflict(&self, path: &str) -> Result<WeaveConflictOutput, CliError> {
        let session = self.conflict_status()?.ok_or_else(|| {
            CliError::runtime(
                "No active conflict session. Start a Weave or Sync that produces conflicts first.",
            )
        })?;
        session
            .conflicts
            .into_iter()
            .find(|conflict| conflict.path == path || conflict.conflict_id == path)
            .ok_or_else(|| CliError::runtime(format!("No active conflict matches `{path}`.")))
    }

    fn resolve_text_blocks_compat(
        &self,
        conflict: &WeaveConflictOutput,
        choice: &str,
        use_base_as_manual_text: bool,
    ) -> Result<WeaveOutput, CliError> {
        let mut output = None;
        for block in conflict
            .blocks
            .iter()
            .filter(|block| block.status != "resolved")
        {
            let manual_text = if use_base_as_manual_text {
                Some(block.base.clone())
            } else {
                None
            };
            let result = map_core(core_space::resolve_weave_conflict(
                self.space_selector()?,
                conflict.path.clone(),
                format!("block:{}:{choice}", block.block_id),
                None,
                manual_text,
            ))?;
            output = Some(WeaveOutput::from_result(result));
        }
        output.ok_or_else(|| CliError::runtime("No unresolved text conflict blocks remain."))
    }

    fn read_conflict_side_bytes(&self, path: &str, side: &str) -> Result<Vec<u8>, CliError> {
        let (weave_dir, conflict_dir, _) = self.active_conflict_paths(path)?;
        let side_path = conflict_dir.join(side);
        std::fs::read(&side_path).map_err(|error| {
            CliError::runtime(format!(
                "Layrs could not read {side} conflict bytes in {}: {error}",
                weave_dir.display()
            ))
        })
    }

    fn resolve_conflict_bytes_compat(
        &self,
        path: &str,
        bytes: &[u8],
        resolution: &str,
    ) -> Result<WeaveOutput, CliError> {
        let selector = self.space_selector()?;
        let (weave_dir, conflict_dir, session_path) = self.active_conflict_paths(path)?;
        std::fs::write(conflict_dir.join("resolved"), bytes).map_err(|error| {
            CliError::runtime(format!(
                "Layrs could not write resolved conflict bytes in {}: {error}",
                conflict_dir.display()
            ))
        })?;

        let mut session: Value =
            serde_json::from_str(&std::fs::read_to_string(&session_path).map_err(|error| {
                CliError::runtime(format!(
                    "Layrs could not read active Weave session {}: {error}",
                    session_path.display()
                ))
            })?)
            .map_err(|error| {
                CliError::runtime(format!(
                    "Layrs could not parse active Weave session {}: {error}",
                    session_path.display()
                ))
            })?;
        let conflicts = session
            .get_mut("conflicts")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                CliError::runtime(format!(
                    "Layrs active Weave session {} is missing conflicts.",
                    session_path.display()
                ))
            })?;
        let conflict = conflicts
            .iter_mut()
            .find(|conflict| {
                conflict.get("path").and_then(Value::as_str) == Some(path)
                    || conflict.get("conflictId").and_then(Value::as_str) == Some(path)
            })
            .ok_or_else(|| CliError::runtime(format!("No active conflict matches `{path}`.")))?;
        if let Some(object) = conflict.as_object_mut() {
            object.insert("status".to_string(), Value::String("resolved".to_string()));
            object.insert(
                "resolution".to_string(),
                Value::String(resolution.to_string()),
            );
            if let Some(blocks) = object.get_mut("blocks").and_then(Value::as_array_mut) {
                for block in blocks {
                    if let Some(block) = block.as_object_mut() {
                        block.insert("status".to_string(), Value::String("resolved".to_string()));
                        block.insert(
                            "resolution".to_string(),
                            Value::String(resolution.to_string()),
                        );
                    }
                }
            }
        }
        let all_resolved = conflicts
            .iter()
            .all(|conflict| conflict.get("status").and_then(Value::as_str) == Some("resolved"));
        if all_resolved {
            session["status"] = Value::String("resolved".to_string());
        }
        session["updatedAtUnix"] = Value::from(unix_now_secs());
        std::fs::write(
            &session_path,
            serde_json::to_string_pretty(&session).map_err(|error| {
                CliError::runtime(format!(
                    "Layrs could not encode active Weave session: {error}"
                ))
            })?,
        )
        .map_err(|error| {
            CliError::runtime(format!(
                "Layrs could not write active Weave session {}: {error}",
                session_path.display()
            ))
        })?;

        let space = map_core(core_space::open_local_space(selector))?;
        let session = self.conflict_status()?.ok_or_else(|| {
            CliError::runtime(format!(
                "Layrs resolved conflict bytes in {}, but no active session remains.",
                weave_dir.display()
            ))
        })?;
        Ok(WeaveOutput {
            message: "Conflict resolved.".to_string(),
            local_space_id: space.local_space_id,
            session,
        })
    }

    fn active_conflict_paths(&self, path: &str) -> Result<(PathBuf, PathBuf, PathBuf), CliError> {
        let selector = self.space_selector()?;
        let layrs_dir = PathBuf::from(selector).join(".layrs");
        let active_path = layrs_dir.join("weaves").join("active.json");
        let active: Value =
            serde_json::from_str(&std::fs::read_to_string(&active_path).map_err(|error| {
                CliError::runtime(format!(
                    "Layrs could not read active Weave marker {}: {error}",
                    active_path.display()
                ))
            })?)
            .map_err(|error| {
                CliError::runtime(format!(
                    "Layrs could not parse active Weave marker {}: {error}",
                    active_path.display()
                ))
            })?;
        let weave_id = active
            .get("weaveId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CliError::runtime(format!(
                    "Layrs active Weave marker {} is missing weaveId.",
                    active_path.display()
                ))
            })?;
        let weave_dir = layrs_dir
            .join("weaves")
            .join(safe_id_fragment_cli(weave_id));
        let session_path = weave_dir.join("session.json");
        let session: Value =
            serde_json::from_str(&std::fs::read_to_string(&session_path).map_err(|error| {
                CliError::runtime(format!(
                    "Layrs could not read active Weave session {}: {error}",
                    session_path.display()
                ))
            })?)
            .map_err(|error| {
                CliError::runtime(format!(
                    "Layrs could not parse active Weave session {}: {error}",
                    session_path.display()
                ))
            })?;
        let conflict_id = session
            .get("conflicts")
            .and_then(Value::as_array)
            .and_then(|conflicts| {
                conflicts.iter().find_map(|conflict| {
                    let matches = conflict.get("path").and_then(Value::as_str) == Some(path)
                        || conflict.get("conflictId").and_then(Value::as_str) == Some(path);
                    matches
                        .then(|| conflict.get("conflictId").and_then(Value::as_str))
                        .flatten()
                })
            })
            .ok_or_else(|| CliError::runtime(format!("No active conflict matches `{path}`.")))?;
        let conflict_dir = weave_dir
            .join("conflicts")
            .join(safe_id_fragment_cli(conflict_id));
        Ok((weave_dir, conflict_dir, session_path))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WeaveOutput {
    pub message: String,
    pub local_space_id: String,
    pub session: WeaveSessionOutput,
}

impl WeaveOutput {
    fn from_result(result: core_space::WeaveOperationResult) -> Self {
        Self {
            message: result.message,
            local_space_id: result.local_space.local_space_id,
            session: WeaveSessionOutput::from_summary(result.session),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WeaveSessionOutput {
    pub weave_id: String,
    pub source_layer_id: String,
    pub target_layer_id: String,
    pub status: String,
    pub pre_weave_target_tree_id: Option<String>,
    pub pre_weave_target_step_id: Option<String>,
    pub planned_steps: Vec<String>,
    pub applied_steps: Vec<String>,
    pub conflicts: Vec<WeaveConflictOutput>,
}

impl WeaveSessionOutput {
    fn from_summary(session: core_space::WeaveSessionSummary) -> Self {
        Self {
            weave_id: session.weave_id,
            source_layer_id: session.source_layer_id,
            target_layer_id: session.target_layer_id,
            status: session.status,
            pre_weave_target_tree_id: session.pre_weave_target_tree_id,
            pre_weave_target_step_id: session.pre_weave_target_step_id,
            planned_steps: session.planned_steps,
            applied_steps: session.applied_steps,
            conflicts: session
                .conflicts
                .into_iter()
                .map(WeaveConflictOutput::from_summary)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WeaveConflictOutput {
    pub conflict_id: String,
    pub path: String,
    pub lens_id: String,
    pub status: String,
    pub message: String,
    pub methods: Vec<String>,
    pub resolution: Option<String>,
    pub blocks: Vec<WeaveConflictBlockOutput>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WeaveConflictBlockOutput {
    pub block_id: String,
    pub status: String,
    pub base: String,
    pub existing: String,
    pub incoming: String,
    pub ours: String,
    pub theirs: String,
    pub methods: Vec<String>,
    pub resolution: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConflictInteractiveOutput {
    pub status: String,
    pub message: String,
    pub actions: Vec<ConflictInteractiveAction>,
    pub session: Option<WeaveSessionOutput>,
}

impl ConflictInteractiveOutput {
    fn new(
        status: impl Into<String>,
        message: impl Into<String>,
        actions: Vec<ConflictInteractiveAction>,
        session: Option<WeaveSessionOutput>,
    ) -> Self {
        Self {
            status: status.into(),
            message: message.into(),
            actions,
            session,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConflictInteractiveAction {
    pub action: String,
    pub path: Option<String>,
    pub block: Option<String>,
    pub method: Option<String>,
}

impl ConflictInteractiveAction {
    fn simple(action: &str) -> Self {
        Self {
            action: action.to_string(),
            path: None,
            block: None,
            method: None,
        }
    }
}

#[derive(Debug, Clone)]
struct InteractiveConflict {
    path: String,
    lens_id: String,
    is_text: bool,
    block_id: Option<String>,
    existing_preview: Option<String>,
    incoming_preview: Option<String>,
}

impl WeaveConflictOutput {
    fn from_summary(conflict: core_space::WeaveConflictSummary) -> Self {
        Self {
            conflict_id: conflict.conflict_id,
            path: conflict.path,
            lens_id: conflict.lens_id,
            status: conflict.status,
            message: conflict.message,
            methods: conflict.methods,
            resolution: conflict.resolution,
            blocks: conflict
                .blocks
                .into_iter()
                .map(|block| WeaveConflictBlockOutput {
                    block_id: block.block_id,
                    status: block.status,
                    base: block.base,
                    existing: block.existing,
                    incoming: block.incoming,
                    ours: block.ours,
                    theirs: block.theirs,
                    methods: block.methods,
                    resolution: block.resolution,
                })
                .collect(),
        }
    }
}

fn is_text_conflict(conflict: &WeaveConflictOutput) -> bool {
    conflict.lens_id == "layrs.text" && !conflict.blocks.is_empty()
}

fn conflict_text_choice(method: ConflictResolveMethod) -> &'static str {
    match method {
        ConflictResolveMethod::Existing => "existing",
        ConflictResolveMethod::Incoming => "incoming",
        ConflictResolveMethod::Both => "both",
        ConflictResolveMethod::Manual => "manual",
    }
}

fn conflict_raw_choice(method: ConflictResolveMethod) -> Result<String, CliError> {
    match method {
        ConflictResolveMethod::Existing => Ok("existing".to_string()),
        ConflictResolveMethod::Incoming => Ok("incoming".to_string()),
        ConflictResolveMethod::Both | ConflictResolveMethod::Manual => {
            Err(CliError::runtime(format!(
                "layrs conflict resolve {} cannot be used for raw conflicts; use existing or incoming.",
                method.as_str()
            )))
        }
    }
}

fn next_unresolved_conflict(conflicts: &[WeaveConflictOutput]) -> Option<InteractiveConflict> {
    for conflict in conflicts {
        if is_text_conflict(conflict) {
            if let Some(block) = conflict
                .blocks
                .iter()
                .find(|block| block.status != "resolved")
            {
                return Some(InteractiveConflict {
                    path: conflict.path.clone(),
                    lens_id: conflict.lens_id.clone(),
                    is_text: true,
                    block_id: Some(block.block_id.clone()),
                    existing_preview: Some(preview_text(&block.ours)),
                    incoming_preview: Some(preview_text(&block.theirs)),
                });
            }
        } else if conflict.status != "resolved" {
            return Some(InteractiveConflict {
                path: conflict.path.clone(),
                lens_id: conflict.lens_id.clone(),
                is_text: false,
                block_id: None,
                existing_preview: None,
                incoming_preview: None,
            });
        }
    }
    None
}

fn preview_text(text: &str) -> String {
    const LIMIT: usize = 120;
    let mut preview = text.replace('\n', "\\n");
    if preview.chars().count() > LIMIT {
        preview = preview.chars().take(LIMIT).collect::<String>();
        preview.push_str("...");
    }
    preview
}

fn render_interactive_conflict<W: Write>(
    output: &mut W,
    conflict: &InteractiveConflict,
) -> Result<(), CliError> {
    writeln!(output, "Conflict: {} ({})", conflict.path, conflict.lens_id)?;
    if conflict.is_text {
        writeln!(
            output,
            "Block: {}",
            conflict.block_id.as_deref().unwrap_or("unknown")
        )?;
        writeln!(
            output,
            "existing: {}",
            conflict.existing_preview.as_deref().unwrap_or("")
        )?;
        writeln!(
            output,
            "incoming: {}",
            conflict.incoming_preview.as_deref().unwrap_or("")
        )?;
        write_prompt(
            output,
            "Choose conflict action: e existing, i incoming, b both, m manual, c continue, a abort, q quit > ",
        )
    } else {
        write_prompt(
            output,
            "Choose conflict action: e existing, i incoming, c continue, a abort, q quit > ",
        )
    }
}

fn write_prompt<W: Write>(output: &mut W, prompt: &str) -> Result<(), CliError> {
    output.write_all(prompt.as_bytes())?;
    output.flush()?;
    Ok(())
}

fn read_prompt_line<R: BufRead>(input: &mut R) -> Result<Option<String>, CliError> {
    let mut line = String::new();
    let bytes = input.read_line(&mut line)?;
    if bytes == 0 { Ok(None) } else { Ok(Some(line)) }
}

fn read_interactive_manual_text<R, W>(input: &mut R, output: &mut W) -> Result<String, CliError>
where
    R: BufRead,
    W: Write,
{
    writeln!(
        output,
        "Enter manual block text. End with a single '.' line."
    )?;
    let mut manual = String::new();
    loop {
        let Some(line) = read_prompt_line(input)? else {
            break;
        };
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "." {
            break;
        }
        manual.push_str(&line);
    }
    Ok(manual)
}

fn read_manual_text_resolution(path: &Path) -> Result<String, CliError> {
    if path == Path::new("-") {
        return read_manual_text_from_stdin();
    }
    std::fs::read_to_string(path).map_err(|error| {
        CliError::runtime(format!(
            "Layrs could not read manual text resolution {}: {error}",
            path.display()
        ))
    })
}

fn read_manual_text_from_stdin() -> Result<String, CliError> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).map_err(|error| {
        CliError::runtime(format!(
            "Layrs could not read manual text from stdin: {error}"
        ))
    })?;
    Ok(input)
}

fn safe_id_fragment_cli(value: &str) -> String {
    let mut safe = String::with_capacity(value.len().max(4));
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            safe.push(ch.to_ascii_lowercase());
        } else if ch.is_whitespace() || matches!(ch, '/' | '\\' | ':') {
            safe.push('-');
        } else {
            safe.push('_');
        }
    }

    let safe = safe.trim_matches('-').to_string();
    if safe.is_empty() {
        "layer".to_string()
    } else {
        safe
    }
}

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
