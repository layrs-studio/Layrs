use super::*;

/// Verify immutable import provenance against its historical View. Current
/// heads may have advanced since the import and must never be reset on retry.
pub(super) fn validate(
    reader: &impl ScmReader,
    operation: &Operation,
    space: &SpaceId,
    scope: &str,
    report: &ImportReport,
) -> Result<()> {
    if operation.space_id() != space
        || operation.kind() != OperationKind::ReceiveRemoteState
        || canonical_json(&read_report(operation.metadata())?).map_err(invalid)?
            != canonical_json(report).map_err(invalid)?
    {
        return Err(invalid("import identity already used by another operation"));
    }
    let view = reader
        .get_repository_view(operation.resulting_view_id())
        .map_err(invalid)?
        .ok_or_else(|| invalid("import historical View missing"))?;
    for reference in &report.references {
        let suffix = reference.git_ref.strip_prefix("refs/heads/");
        let id = named_id(scope, &reference.git_ref);
        let metadata = if let Some(suffix) = suffix {
            let layer_id = LayerId::parse(format!("layer:git-{id}")).map_err(invalid)?;
            let layer = reader
                .get_layer(&layer_id)
                .map_err(invalid)?
                .ok_or_else(|| invalid("import Layer missing"))?;
            if layer.space_id() != space
                || layer.name() != format!("git/{suffix}")
                || !view.layer_heads().iter().any(|head| {
                    head.layer_id() == &layer_id && head.step_ids() == [reference.step_id.clone()]
                })
            {
                return Err(invalid("import historical Layer does not match report"));
            }
            layer.metadata().clone()
        } else {
            let suffix = reference
                .git_ref
                .strip_prefix("refs/tags/")
                .ok_or_else(|| invalid("unsupported import reference"))?;
            let marker_id = MarkerId::parse(format!("marker:git-{id}")).map_err(invalid)?;
            let marker = reader
                .get_marker(&marker_id)
                .map_err(invalid)?
                .ok_or_else(|| invalid("import Marker missing"))?;
            if marker.space_id() != space
                || marker.name() != format!("git/{suffix}")
                || marker.target_step_id() != &reference.step_id
                || !view.marker_ids().contains(&marker_id)
            {
                return Err(invalid("import historical Marker does not match report"));
            }
            marker.metadata().clone()
        };
        if metadata_value(&metadata, "git.importId") != Some(report.import_id.as_str())
            || metadata_value(&metadata, "git.ref") != Some(reference.git_ref.as_str())
            || metadata_value(&metadata, "git.commitId") != Some(reference.git_id.as_str())
        {
            return Err(invalid("import reference provenance does not match report"));
        }
    }
    for commit in &report.commits {
        let id = ObservationId::parse(format!(
            "observation:git-{}",
            named_id(scope, &commit.git_id)
        ))
        .map_err(invalid)?;
        let observation = reader
            .get_step_observation(&id)
            .map_err(invalid)?
            .ok_or_else(|| invalid("import observation missing"))?;
        if observation.step_id() != &commit.step_id
            || observation.operation_id() != Some(operation.id())
            || metadata_value(observation.metadata(), "git.commitId")
                != Some(commit.git_id.as_str())
            || metadata_value(observation.metadata(), "git.importId")
                != Some(report.import_id.as_str())
        {
            return Err(invalid("import observation does not match report"));
        }
    }
    Ok(())
}
