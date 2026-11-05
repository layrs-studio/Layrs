use crate::args::CliCommand;
use serde_json::Value;
use std::fmt::Write;

pub(super) fn render(command: &CliCommand, data: &Value) -> Option<String> {
    let preview = &data["preview"];
    match command {
        CliCommand::Restore { preview: true, .. }
        | CliCommand::RestoreView { preview: true, .. } => {
            let mut output =
                String::from("Read-only restore preview. No files or history changed.\n");
            let _ = writeln!(output, "Step: {}", preview["stepId"].as_str().unwrap_or(""));
            let _ = writeln!(
                output,
                "Scope: {}",
                preview["path"]
                    .as_str()
                    .map(|path| format!("{path:?}"))
                    .unwrap_or_else(|| "entire checkout".into())
            );
            if let Some(operation) = preview["operationId"].as_str() {
                let _ = writeln!(output, "Historical View from {operation}");
                for heads in preview["layerHeads"].as_array().into_iter().flatten() {
                    let _ = writeln!(
                        output,
                        "Layer {} -> {}",
                        heads["layerId"].as_str().unwrap_or(""),
                        heads["stepIds"]
                    );
                }
            }
            for change in preview["changes"].as_array().into_iter().flatten() {
                let _ = writeln!(
                    output,
                    "{} {} ({} -> {} bytes)",
                    change["status"].as_str().unwrap_or(""),
                    change["path"],
                    change["beforeBytes"],
                    change["afterBytes"]
                );
                if change["binary"] == true {
                    output.push_str("Binary content changed.\n");
                }
                if change["executableChanged"] == true {
                    output.push_str("Executable mode changed.\n");
                }
                if change["textOmitted"] == true {
                    output.push_str("Text diff exceeds preview limits.\n");
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
            let collisions: Vec<_> = preview["collisions"]
                .as_array()
                .into_iter()
                .flatten()
                .collect();
            if collisions.is_empty() {
                output.push_str("Apply with the same target/path and --expected followed by this JSON (quote it as one argument):\n");
                let _ = writeln!(output, "{}", preview["expected"]);
            } else {
                output.push_str("BLOCKED: preserve and move these obstructions before requesting a new preview:\n");
                for collision in collisions {
                    let _ = writeln!(
                        output,
                        "{}: {}",
                        collision["path"],
                        collision["reason"].as_str().unwrap_or("collision")
                    );
                }
            }
            Some(output)
        }
        CliCommand::Rewrite { preview: true, .. } => {
            let mut output =
                String::from("Read-only rewrite preview. No files or history changed.\n");
            let _ = writeln!(
                output,
                "Layer {}: {} -> {}",
                preview["targetLayerId"].as_str().unwrap_or(""),
                preview["targetHeadsBefore"],
                preview["proposedStep"]["stepId"].as_str().unwrap_or("")
            );
            for item in preview["rewriteMap"].as_array().into_iter().flatten() {
                let _ = writeln!(
                    output,
                    "History {} -> {}",
                    item["oldStepId"].as_str().unwrap_or(""),
                    item["newStepIds"]
                );
            }
            for key in ["markersUnchanged", "releasesUnchanged"] {
                for id in preview[key].as_array().into_iter().flatten() {
                    let _ = writeln!(
                        output,
                        "Retained at original Step: {}",
                        id.as_str().unwrap_or("")
                    );
                }
            }
            let _ = writeln!(output, "{} conflict(s).", preview["conflictCount"]);
            Some(output)
        }
        _ => None,
    }
}
