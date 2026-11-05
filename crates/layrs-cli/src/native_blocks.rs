use super::*;
use crate::args::BlockChoice;
use layrs_client_core::native_scm::BlockResolutionChoice;
use layrs_core::scm::{ConflictId, ObjectId};

pub(super) fn execute(
    workspace: &NativeWorkspace,
    command: &CliCommand,
) -> Result<serde_json::Value, CliError> {
    let CliCommand::ResolveBlock {
        layer,
        conflict,
        block,
        choice,
        preview,
        expected_operation,
        expected_step,
    } = command
    else {
        unreachable!()
    };
    let choice = match choice {
        BlockChoice::Term(term) => BlockResolutionChoice::SelectTerm {
            term: ObjectId::parse(term).map_err(error)?,
        },
        BlockChoice::File { file, sources } => {
            use std::io::Read;
            let mut bytes = vec![];
            std::fs::File::open(file)
                .map_err(error)?
                .take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(error)?;
            if bytes.len() > 1024 * 1024 {
                return Err(CliError::usage("manual block exceeds 1 MiB"));
            }
            let text = String::from_utf8(bytes).map_err(|_| {
                CliError::usage(
                    "manual block requires exact UTF-8 text; use resolve-file for binary data",
                )
            })?;
            let mut source_term_ids = sources
                .iter()
                .map(ObjectId::parse)
                .collect::<Result<Vec<_>, _>>()
                .map_err(error)?;
            source_term_ids.sort();
            BlockResolutionChoice::Manual {
                text,
                source_term_ids,
            }
        }
    };
    let expected_operation = expected_operation
        .as_ref()
        .map(OperationId::parse)
        .transpose()
        .map_err(error)?;
    let expected_step = expected_step
        .as_ref()
        .map(StepId::parse)
        .transpose()
        .map_err(error)?;
    if let Some(id) = &expected_operation {
        if workspace.state().map_err(error)?.operation.id() != id {
            return Err(CliError::runtime("block preview is stale"));
        }
    }
    let plan = workspace
        .preview_block_resolution(
            layer,
            ConflictId::parse(conflict).map_err(error)?,
            ObjectId::parse(block).map_err(error)?,
            choice,
        )
        .map_err(error)?;
    if expected_operation
        .as_ref()
        .is_some_and(|id| plan.expected_state().operation.id() != id)
        || expected_step
            .as_ref()
            .is_some_and(|id| plan.bundle().step.id() != id)
    {
        return Err(CliError::runtime("block preview is stale"));
    }
    if *preview {
        Ok(
            json!({"expectedOperation":plan.expected_state().operation.id(),"expectedStep":plan.bundle().step.id(),"conflicts":plan.bundle().conflicts}),
        )
    } else {
        Ok(json!({"resolution":workspace.accept_conflict_resolution(&plan).map_err(error)?}))
    }
}

pub(super) fn render(data: &serde_json::Value) -> String {
    if let Some(step) = data["expectedStep"].as_str() {
        format!(
            "Proposed Step: {step}\nRemaining conflicts: {}\nApply guards: --expected-operation {} --expected-step {step}",
            data["conflicts"].as_array().map_or(0, Vec::len),
            data["expectedOperation"].as_str().unwrap_or("")
        )
    } else {
        format!(
            "Resolution saved: {}\nRemaining conflicts: {}",
            data["resolution"]["stepId"].as_str().unwrap_or(""),
            data["resolution"]["conflictIds"]
                .as_array()
                .map_or(0, Vec::len)
        )
    }
}
