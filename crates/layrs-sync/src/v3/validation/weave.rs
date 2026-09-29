//! Validation of reproducible Weave execution records.
use super::*;

fn valid_algorithm_component(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn validate_algorithm(value: &AlgorithmVersion, field: &'static str) -> V3Result<()> {
    if !valid_algorithm_component(&value.id, 128)
        || value.version.is_empty()
        || value.version.len() > 64
    {
        Err(V3ValidationError::new(
            "invalid_weave_record",
            field,
            "invalid algorithm id/version",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn validate_weave(weave: &WeaveReceiptRecord) -> V3Result<()> {
    if weave.source_heads.is_empty()
        || weave.source_heads.len() > 128
        || weave.target_heads_before.is_empty()
        || weave.target_heads_before.len() > 128
    {
        return Err(V3ValidationError::new(
            "invalid_weave_record",
            "record.heads",
            "Weave head sets require 1..=128 entries",
        ));
    }
    sorted_unique(&weave.source_heads, "record.sourceHeads")?;
    sorted_unique(&weave.target_heads_before, "record.targetHeadsBefore")?;
    if weave.merge_bases.len() > 128 {
        return Err(V3ValidationError::new(
            "limit_exceeded",
            "record.mergeBases",
            "maximum is 128",
        ));
    }
    let merge_ids = weave
        .merge_bases
        .iter()
        .map(MergeBase::base_id)
        .collect::<Vec<_>>();
    sorted_unique(&merge_ids, "record.mergeBases")?;
    for base in &weave.merge_bases {
        if let MergeBase::Virtual {
            source_step_ids,
            algorithm,
            ..
        } = base
        {
            if source_step_ids.len() < 2 || source_step_ids.len() > 128 {
                return Err(V3ValidationError::new(
                    "invalid_weave_record",
                    "record.mergeBases.sourceStepIds",
                    "virtual base requires 2..=128 Steps",
                ));
            }
            sorted_unique(source_step_ids, "record.mergeBases.sourceStepIds")?;
            validate_algorithm(algorithm, "record.mergeBases.algorithm")?;
        }
    }
    sorted_unique(&weave.conflict_ids, "record.conflictIds")?;
    match weave.outcome {
        WeaveOutcome::Clean if !weave.conflict_ids.is_empty() => {
            return Err(V3ValidationError::new(
                "invalid_weave_record",
                "record.conflictIds",
                "clean Weave cannot retain conflicts",
            ));
        }
        WeaveOutcome::Conflicted if weave.conflict_ids.is_empty() => {
            return Err(V3ValidationError::new(
                "invalid_weave_record",
                "record.conflictIds",
                "conflicted Weave requires conflicts",
            ));
        }
        _ => {}
    }
    if weave.lens_executions.len() > 100_000 {
        return Err(V3ValidationError::new(
            "limit_exceeded",
            "record.lensExecutions",
            "maximum is 100000",
        ));
    }
    let execution_ids = weave
        .lens_executions
        .iter()
        .map(|e| &e.execution_id)
        .collect::<Vec<_>>();
    sorted_unique(&execution_ids, "record.lensExecutions")?;
    for execution in &weave.lens_executions {
        if execution.term_roles.len() < 2 || execution.term_roles.len() > 192 {
            return Err(V3ValidationError::new(
                "invalid_weave_record",
                "record.lensExecutions.termRoles",
                "requires 2..=192 roles",
            ));
        }
        let roles = execution
            .term_roles
            .iter()
            .map(|r| (&r.term_id, r.role))
            .collect::<Vec<_>>();
        sorted_unique(&roles, "record.lensExecutions.termRoles")?;
        match execution.result {
            LensExecutionResult::AutoResolved if execution.conflict_id.is_some() => {
                return Err(V3ValidationError::new(
                    "invalid_weave_record",
                    "record.lensExecutions.conflictId",
                    "auto-resolved execution must use null",
                ));
            }
            LensExecutionResult::Conflicted | LensExecutionResult::StructuralFallback
                if execution.conflict_id.is_none() =>
            {
                return Err(V3ValidationError::new(
                    "invalid_weave_record",
                    "record.lensExecutions.conflictId",
                    "non-auto execution requires Conflict ID",
                ));
            }
            _ => {}
        }
    }
    let profile = &weave.determinism_profile;
    validate_algorithm(
        &profile.merge_algorithm,
        "record.determinismProfile.mergeAlgorithm",
    )?;
    validate_algorithm(
        &profile.base_algorithm,
        "record.determinismProfile.baseAlgorithm",
    )?;
    let ambient = &profile.host_policy.ambient_capabilities;
    if ambient.network
        || ambient.wall_clock
        || ambient.randomness
        || ambient.locale
        || ambient.host_filesystem
        || ambient.environment
    {
        return Err(V3ValidationError::new(
            "invalid_weave_record",
            "record.determinismProfile.hostPolicy",
            "ambient capabilities must all be false",
        ));
    }
    let policy = &profile.resolution_policy;
    if policy.automatic_side_selection || policy.allowed_methods.is_empty() {
        return Err(V3ValidationError::new(
            "invalid_weave_record",
            "record.determinismProfile.resolutionPolicy",
            "automatic side selection is false and methods are non-empty",
        ));
    }
    sorted_unique(
        &policy.allowed_methods,
        "record.determinismProfile.resolutionPolicy.allowedMethods",
    )?;
    let limits = &profile.limits;
    if [
        limits.max_memory_bytes,
        limits.max_execution_ms,
        limits.max_input_bytes,
        limits.max_output_bytes,
        limits.max_blocks,
        limits.max_segments,
        limits.max_paths,
        limits.max_recursion_depth,
    ]
    .contains(&0)
        || limits.max_fuel.as_ref().is_some_and(|fuel| *fuel == 0)
    {
        return Err(V3ValidationError::new(
            "invalid_weave_record",
            "record.determinismProfile.limits",
            "all configured limits must be positive",
        ));
    }
    Ok(())
}
