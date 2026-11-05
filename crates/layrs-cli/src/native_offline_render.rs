use crate::args::OfflineCommand;
use serde_json::Value;

pub(super) fn render(command: &OfflineCommand, data: &Value) -> String {
    match command {
        OfflineCommand::Status => format!(
            "{}\nShared content family: {}",
            version(&data["version"]),
            if data["shared"] == true {
                "yes"
            } else {
                "no; use explicit family conversion for offline preparation"
            }
        ),
        OfflineCommand::Check(_) => {
            let mut lines = vec![format!(
                "Offline operation: {}. Preparation: {}.",
                if data["available"] == true {
                    "available"
                } else {
                    "blocked"
                },
                if data["prepared"] == true {
                    "ready"
                } else {
                    "not prepared"
                }
            )];
            for row in data["paths"].as_array().into_iter().flatten() {
                let blockers = row["blockers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!(
                    "{} (operand {}): {}",
                    text(row, "path"),
                    row["operand"],
                    if blockers.is_empty() {
                        "available"
                    } else {
                        &blockers
                    }
                ));
            }
            let blockers = data["blockers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            if !blockers.is_empty() {
                lines.push(format!("Blockers: {blockers}"));
            }
            lines.join("\n")
        }
        OfflineCommand::Compare(_) => {
            let rows = data["differences"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|row| {
                    format!(
                        "{}: {} -> {}{}",
                        text(row, "path"),
                        row["before"].as_str().unwrap_or("absent"),
                        row["after"].as_str().unwrap_or("absent"),
                        if row["beforeExecutable"] != row["afterExecutable"] {
                            " (executable mode changed)"
                        } else {
                            ""
                        }
                    )
                })
                .collect::<Vec<_>>();
            if rows.is_empty() {
                "No differences at the selected exact versions.".into()
            } else {
                rows.join("\n")
            }
        }
        OfflineCommand::Preparations => {
            let rows = data["preparations"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| {
                    format!(
                        "{}  {}  {}  {}",
                        text(p, "id"),
                        text(p, "status"),
                        text(&p["request"], "operation"),
                        version(&p["request"]["version"])
                    )
                })
                .collect::<Vec<_>>();
            if rows.is_empty() {
                "No offline preparations.".into()
            } else {
                rows.join("\n")
            }
        }
        OfflineCommand::Release { .. } => {
            format!("Released preparation {}.", text(data, "released"))
        }
        OfflineCommand::CancelReservation => "Cancelled the interrupted capture reservation. Checkout files and accepted history retained.".into(),
        OfflineCommand::ReleaseGeneration { .. } => format!("Released {} retention claims for generation {}. Other protected content remains retained.", data["releasedClaims"], data["generation"]),
        OfflineCommand::Quota { .. } => usage(data),
        OfflineCommand::Evict => format!(
            "Evicted {} unretained objects.\n{}",
            data["evictedObjects"],
            usage(data)
        ),
        OfflineCommand::CreateFamily { .. } => format!(
            "Created shared content family at {}.\n{}",
            text(data, "path"),
            usage(data)
        ),
        OfflineCommand::Convert { .. } | OfflineCommand::ResumeConversion { .. } => format!(
            "Shared checkout ready at {}. Source checkout preserved.",
            text(data, "path")
        ),
        OfflineCommand::AbandonConversion { .. } => format!("Abandoned interrupted conversion at {}. Source, destination files and private metadata retained.", text(data, "destination")),
        OfflineCommand::RestoreApply { .. } => format!("Restored selected paths. Operation: {}. Result capture: {}. Overwritten preimages are retained.", text(&data["restoration"], "operationId"), data["restoration"]["capture"]["sequence"]),
        OfflineCommand::Read { .. } | OfflineCommand::RestorePreview { .. } => unreachable!("file output is rendered after writing"),
    }
}
fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("unknown")
}
fn usage(data: &Value) -> String {
    let u = &data["usage"];
    format!(
        "Content: {} bytes; reserved: {} bytes; quota: {} bytes; protection roots: {}.",
        u["contentBytes"], u["reservedBytes"], u["limitBytes"], u["protectionRoots"]
    )
}
fn version(value: &Value) -> String {
    match value["kind"].as_str() {
        Some("generation") => format!(
            "Generation {} ({})",
            value["generation"],
            text(value, "worktreeId")
        ),
        Some("capture") => format!(
            "Capture {} ({})",
            value["sequence"],
            text(value, "worktreeId")
        ),
        Some("remote") => format!("Observed Step {}", text(value, "stepId")),
        _ => "Unknown version".into(),
    }
}
