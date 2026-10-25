//! Shared launch/control adapters; the caller owns the long-lived host process.
use super::{
    NativeWorkspace, Result, VfsHost, VfsHostClient, VfsHostCommand, VfsHostResponse,
    VfsHostTicket, VfsWorktree,
};
use layrs_core::scm::{LayerId, WorktreeId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::Path, time::Duration};

#[derive(Debug, Deserialize, Serialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum VfsLaunchAction {
    Create {
        path: String,
        layer: String,
        step: String,
    },
    Serve {
        path: String,
    },
    Resume {
        worktree_id: WorktreeId,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VfsControlRequest {
    pub ticket: VfsHostTicket,
    pub command: VfsHostCommand,
}

/// No detachment: dropping this host disconnects the provider. Retain it and
/// call serve() only after delivering its non-secret ticket to the owner.
pub fn prepare_vfs_host(source_root: impl AsRef<Path>, action: VfsLaunchAction) -> Result<VfsHost> {
    let worktree = match action {
        VfsLaunchAction::Create { path, layer, step } => {
            let source = NativeWorkspace::discover(source_root)?;
            let layer = match LayerId::parse(&layer) {
                Ok(id) => id,
                Err(_) => source.layer_named(&layer)?,
            };
            let step = source.resolve_step_name(&step)?.step_id;
            // The engine validates the exact Layer in the bound Store/Space.
            VfsWorktree::create(&source, &std::path::absolute(path)?, &layer, &step)?
        }
        VfsLaunchAction::Serve { path } => VfsWorktree::open(&std::path::absolute(path)?)?,
        VfsLaunchAction::Resume { worktree_id } => {
            let source = NativeWorkspace::discover(source_root)?;
            VfsWorktree::resume_registered(&source, &worktree_id)?
        }
    };
    VfsHost::bind(worktree)
}

pub fn execute_vfs_control(request: VfsControlRequest) -> std::result::Result<Value, String> {
    if matches!(
        request.command,
        VfsHostCommand::Capture {
            writers_quiesced: false
        } | VfsHostCommand::Export {
            writers_quiesced: false,
            ..
        }
    ) {
        return Err(
            "stop Worktree writers before checkpointing; keep them stopped until the reply".into(),
        );
    }
    let timeout = match &request.command {
        VfsHostCommand::Status {} => Duration::from_secs(3),
        VfsHostCommand::Stop {} => Duration::from_secs(10),
        VfsHostCommand::Capture { .. } | VfsHostCommand::Export { .. } => Duration::from_secs(300),
    };
    let response = VfsHostClient::open(&request.ticket)
        .map_err(|error| format!("{}: {error}", error.code()))?
        .request_with_timeout(request.command, timeout)
        .map_err(|error| format!("{}: {error}", error.code()))?;
    match response {
        VfsHostResponse::Ok { value } => Ok(value),
        VfsHostResponse::Error { code, message } => Err(format!("{code}: {message}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn launch_contract_requires_exact_selection_and_rejects_unrequested_mutations() {
        let launch: VfsLaunchAction = serde_json::from_value(
            json!({"action":"create","path":"root","layer":"layer:task","step":"@layer:task"}),
        )
        .unwrap();
        assert!(matches!(launch, VfsLaunchAction::Create { .. }));
        let resume: VfsLaunchAction =
            serde_json::from_value(json!({"action":"resume","worktreeId":"worktree:retained"}))
                .unwrap();
        assert!(matches!(resume, VfsLaunchAction::Resume { worktree_id }
            if worktree_id.as_str() == "worktree:retained"));
        for value in [
            json!({"action":"create","path":"root","layer":"main"}),
            json!({"action":"serve","path":"root","restore":true}),
            json!({"action":"resume"}),
            json!({"action":"resume","worktreeId":"step:wrong"}),
            json!({"action":"resume","worktreeId":"worktree:retained","path":"guessed"}),
            json!({"action":"resume","worktreeId":"worktree:retained","step":"new-base"}),
        ] {
            assert!(serde_json::from_value::<VfsLaunchAction>(value).is_err());
        }
    }

    #[test]
    fn quiescence_is_checked_before_opening_any_control_endpoint() {
        let ticket = json!({
            "storeRoot":"missing-store",
            "storeIdentity":"unopened",
            "storeMetadataIdentity":"unopened-metadata",
            "storeId":"store:fixture",
            "worktreeId":"worktree:fixture",
            "endpointId":"missing-endpoint",
        });
        for command in [
            json!({"action":"capture","writers_quiesced":false}),
            json!({"action":"export","parent":"parent","writers_quiesced":false}),
        ] {
            let request =
                serde_json::from_value(json!({"ticket":ticket,"command":command})).unwrap();
            assert!(
                execute_vfs_control(request)
                    .unwrap_err()
                    .contains("stop Worktree writers")
            );
        }
        assert!(
            serde_json::from_value::<VfsControlRequest>(
                json!({"ticket":ticket,"command":{"action":"capture"}})
            )
            .is_err()
        );
        assert!(
            !serde_json::to_string(&serde_json::from_value::<VfsHostTicket>(ticket).unwrap())
                .unwrap()
                .contains("token")
        );
    }
}
