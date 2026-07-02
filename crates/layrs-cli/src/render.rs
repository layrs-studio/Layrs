use crate::engine::{
    CompactOutput, DiffOutput, InitLocalSpace, LayerDeleted, LayerOutput, LayersOutput,
    LoginOutput, LogoutOutput, PublishOutput, ReceiveOutput, SpacesOutput, StatusOutput, StepSaved,
    TimelineOutput, WhoamiOutput,
};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct Rendered {
    pub human: String,
    pub data: Value,
}

impl Rendered {
    pub fn from_serializable<T>(human: String, data: &T) -> Result<Self, String>
    where
        T: Serialize,
    {
        Ok(Self {
            human,
            data: serde_json::to_value(data)
                .map_err(|error| format!("failed to encode command output: {error}"))?,
        })
    }
}

pub fn ok_json(data: Value) -> String {
    json!({ "ok": true, "data": data }).to_string()
}

pub fn error_json(message: &str) -> String {
    json!({ "ok": false, "error": { "message": message } }).to_string()
}

pub fn init(data: InitLocalSpace) -> Result<Rendered, String> {
    let step = data
        .initial_step_id
        .as_deref()
        .map(|step_id| format!("\ninitial step: {step_id}"))
        .unwrap_or_else(|| "\ninitial step: none".to_string());
    Rendered::from_serializable(
        format!(
            "Initialized Layrs Space `{}`\nlocal space: {}\npath: {}\nactive layer: {}\nscanned files: {}\npending publish: {}{}",
            data.name,
            data.local_space_id,
            data.path,
            data.active_layer_id,
            data.scanned_files,
            data.pending_publish_count,
            step
        ),
        &data,
    )
}

pub fn step(data: StepSaved) -> Result<Rendered, String> {
    let step = data.step_id.as_deref().unwrap_or("none");
    Rendered::from_serializable(
        format!(
            "{}\nstep: {}\nlayer: {}\nchanged files: {}\n+{} -{}\npending publish: {}",
            data.message,
            step,
            data.layer_id,
            data.changed_files,
            data.additions,
            data.deletions,
            data.pending_publish_count
        ),
        &data,
    )
}

pub fn diff(data: DiffOutput, color: bool) -> Result<Rendered, String> {
    let human = if data.text.trim().is_empty() {
        "No diff.".to_string()
    } else {
        render_diff_text(&data.text, color)
    };
    Rendered::from_serializable(human, &data)
}

pub fn timeline(data: TimelineOutput) -> Result<Rendered, String> {
    let human = if data.steps.is_empty() {
        "No steps yet.".to_string()
    } else {
        data.steps
            .iter()
            .map(|step| {
                format!(
                    "{}  {}  {}  {}",
                    step.step_id, step.layer_id, step.captured_at_unix, step.summary
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Rendered::from_serializable(human, &data)
}

pub fn publish(data: PublishOutput) -> Result<Rendered, String> {
    Rendered::from_serializable(
        format!(
            "{}\nworkspace: {}\nstatus: {}\npushed objects: {}\npushed steps: {}{}",
            data.message,
            data.workspace_id,
            data.status,
            data.pushed_objects,
            data.pushed_steps,
            data.sync_state_path
                .as_deref()
                .map(|path| format!("\nsync state: {path}"))
                .unwrap_or_default()
        ),
        &data,
    )
}

pub fn receive(data: ReceiveOutput) -> Result<Rendered, String> {
    Rendered::from_serializable(
        format!(
            "{}\nstatus: {}\npulled objects: {}\npulled steps: {}\nsync state: {}",
            data.message, data.status, data.pulled_objects, data.pulled_steps, data.sync_state_path
        ),
        &data,
    )
}

pub fn compact(data: CompactOutput) -> Result<Rendered, String> {
    Rendered::from_serializable(
        format!(
            "Compacted Layrs store\nlocal space: {}\npath: {}\npacked chunks: {}\nloose chunks removed: {}\nraw bytes: {}\nstored bytes: {}{}",
            data.local_space_id,
            data.path,
            data.packed_chunks,
            data.loose_chunks_removed,
            data.raw_bytes,
            data.stored_bytes,
            data.pack_path
                .as_deref()
                .map(|path| format!("\npack: {path}"))
                .unwrap_or_default()
        ),
        &data,
    )
}

pub fn status(data: StatusOutput) -> Result<Rendered, String> {
    Rendered::from_serializable(
        format!(
            "path: {}\nactive layer: {}\nchanged: {}\nadded: {}\nmodified: {}\ndeleted: {}\npending steps: {}",
            data.path,
            data.active_layer_id,
            data.changed,
            data.added_files,
            data.modified_files,
            data.deleted_files,
            data.pending_steps
        ),
        &data,
    )
}

pub fn login(data: LoginOutput) -> Result<Rendered, String> {
    Rendered::from_serializable(
        format!(
            "Logged in to {}\nstatus: {}\naccount: {}\nemail: {}",
            data.endpoint,
            data.status,
            data.account_id.as_deref().unwrap_or("unknown"),
            data.email.as_deref().unwrap_or("unknown")
        ),
        &data,
    )
}

pub fn whoami(data: WhoamiOutput) -> Result<Rendered, String> {
    Rendered::from_serializable(
        format!(
            "endpoint: {}\naccount: {}\nemail: {}\nname: {}",
            data.endpoint, data.account_id, data.email, data.display_name
        ),
        &data,
    )
}

pub fn logout(data: LogoutOutput) -> Result<Rendered, String> {
    let endpoint = data.endpoint.as_deref().unwrap_or("configured endpoint");
    Rendered::from_serializable(format!("Logged out of {endpoint}"), &data)
}

pub fn spaces(data: SpacesOutput) -> Result<Rendered, String> {
    let human = if data.spaces.is_empty() {
        "No Local Spaces found.".to_string()
    } else {
        data.spaces
            .iter()
            .map(|space| {
                let marker = if space.active { "*" } else { " " };
                let path = space.path.as_deref().unwrap_or("-");
                format!(
                    "{marker} {}  {}  local: {}  {}",
                    space.space_id,
                    space.name,
                    space.local_space_id.as_deref().unwrap_or("-"),
                    path
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Rendered::from_serializable(human, &data)
}

pub fn layers(data: LayersOutput) -> Result<Rendered, String> {
    let human = if data.layers.is_empty() {
        "No Layers found.".to_string()
    } else {
        data.layers
            .iter()
            .map(format_layer_line)
            .collect::<Vec<_>>()
            .join("\n")
    };
    Rendered::from_serializable(human, &data)
}

pub fn layer(data: LayerOutput, action: &str) -> Result<Rendered, String> {
    Rendered::from_serializable(
        format!("{action} Layer\n{}", format_layer_line(&data)),
        &data,
    )
}

pub fn layer_deleted(data: LayerDeleted) -> Result<Rendered, String> {
    Rendered::from_serializable(
        format!(
            "{}\nLayer: {}\nname: {}",
            data.message, data.layer_id, data.name
        ),
        &data,
    )
}

pub fn render_diff_text(text: &str, color: bool) -> String {
    if !color {
        return text.to_string();
    }

    let mut rendered = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches('\n').trim_end_matches('\r');
        if bare.starts_with('+') && !bare.starts_with("+++") {
            rendered.push_str("\x1b[32m");
            rendered.push_str(line);
            rendered.push_str("\x1b[0m");
        } else if bare.starts_with('-') && !bare.starts_with("---") {
            rendered.push_str("\x1b[31m");
            rendered.push_str(line);
            rendered.push_str("\x1b[0m");
        } else {
            rendered.push_str(line);
        }
    }

    rendered
}

fn format_layer_line(layer: &LayerOutput) -> String {
    let marker = if layer.active { "*" } else { " " };
    let parent = layer.parent_layer_id.as_deref().unwrap_or("-");
    format!(
        "{marker} {}  {}  parent: {}",
        layer.layer_id, layer.name, parent
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_diff_additions_and_deletions_without_headers() {
        let text = "--- a/file\n+++ b/file\n-old line\n+new line\n context\n";

        assert_eq!(
            render_diff_text(text, true),
            "--- a/file\n+++ b/file\n\x1b[31m-old line\n\x1b[0m\x1b[32m+new line\n\x1b[0m context\n"
        );
    }

    #[test]
    fn leaves_long_lines_untruncated() {
        let long_line = format!("+{}", "x".repeat(500));
        let rendered = render_diff_text(&long_line, false);

        assert_eq!(rendered, long_line);
    }
}
