use crate::{args::Cli, engine::CliError, render::Rendered};
use layrs_client_core::native_scm::{
    NativeRemoteAction as Action, NativeRemoteRequest, execute_remote,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

pub fn run(cli: &Cli, arguments: &[String]) -> Result<Rendered, CliError> {
    let mut args = arguments.to_vec();
    let origin = option(&mut args, "--endpoint")?
        .or_else(|| std::env::var("LAYRS_SERVER_ENDPOINT").ok())
        .ok_or_else(|| {
            CliError::usage("remote requires --endpoint HTTPS_URL or LAYRS_SERVER_ENDPOINT")
        })?;
    let ca_pem = option(&mut args, "--ca")?
        .map(|path| {
            use std::io::Read;
            let mut pem = String::new();
            std::fs::File::open(path)
                .and_then(|f| f.take(1024 * 1024 + 1).read_to_string(&mut pem))
                .map_err(error)?;
            if pem.len() > 1024 * 1024 {
                return Err(CliError::usage("CA exceeds 1 MiB"));
            }
            Ok(pem)
        })
        .transpose()?;
    let mut root = cli
        .globals
        .space
        .clone()
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .map_err(error)?;
    let request_id = option(&mut args, "--request-id")?;
    let choose_output = option(&mut args, "--output")?;
    let history_path = option(&mut args, "--path")?;
    let history_actor = option(&mut args, "--actor")?;
    let history_since = option(&mut args, "--since")?;
    let history_until = option(&mut args, "--until")?;
    if [
        &history_path,
        &history_actor,
        &history_since,
        &history_until,
    ]
    .iter()
    .any(|value| value.is_some())
        && args.first().is_none_or(|arg| arg != "history")
    {
        return Err(CliError::usage(
            "--path/--actor/--since/--until are only supported by remote history",
        ));
    }
    let history_filter =
        serde_json::json!({"actorId":history_actor,"since":history_since,"until":history_until});
    if choose_output.is_some()
        && args.first().is_none_or(|arg| {
            !matches!(
                arg.as_str(),
                "choose-preview"
                    | "policy-preview"
                    | "policy-preview-rules"
                    | "policy-replace"
                    | "partial-restore-preview"
                    | "partial-directory-preview"
                    | "file"
            )
        })
    {
        return Err(CliError::usage(
            "--output requires a policy/choose preview or file download",
        ));
    }
    let expected_last = option(&mut args, "--expected-last")?;
    let reason = option(&mut args, "--reason")?;
    let mutation = args
        .first()
        .is_some_and(|a| matches!(a.as_str(), "release-publish" | "release-revoke"));
    if !mutation && (request_id.is_some() || expected_last.is_some() || reason.is_some()) {
        return Err(CliError::usage(
            "release mutation options require release-publish or release-revoke",
        ));
    }
    let publication_request = || -> Result<serde_json::Value, CliError> {
        let request = request_id.as_ref().ok_or_else(|| {
            CliError::usage("provide --request-id UNIQUE_ID; reuse it for the same retry")
        })?;
        let last = expected_last.as_ref().ok_or_else(|| {
            CliError::usage("provide --expected-last EVENT_OBJECT_ID or none from release-events")
        })?;
        Ok(
            serde_json::json!({"requestId":request,"expectedLastEventId":if last == "none" { None } else { Some(last) }}),
        )
    };
    let action = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["login"] => Action::StartLogin,
        ["whoami"] => Action::Whoami,
        ["file", step, path] => Action::ReadFile {
            step: layrs_core::scm::StepId::parse(*step).map_err(error)?,
            path: layrs_core::scm::RepoPath::parse(*path).map_err(error)?,
            output: PathBuf::from(
                choose_output
                    .as_ref()
                    .ok_or_else(|| CliError::usage("remote file requires --output NEW_FILE"))?,
            ),
        },
        ["observe", file] => {
            let mut request = json_file(file)?;
            let fields = request
                .as_object_mut()
                .ok_or_else(|| CliError::usage("observation request must be a JSON object"))?;
            if fields.contains_key("action") {
                return Err(CliError::usage(
                    "observation request must not supply an action",
                ));
            }
            fields.insert("action".into(), serde_json::json!("observeStep"));
            serde_json::from_value(request).map_err(error)?
        }
        ["logout"] => Action::Logout,
        ["choose-preview", file] => {
            let mut request = json_file(file)?;
            let fields = request
                .as_object_mut()
                .ok_or_else(|| CliError::usage("choose request must be a JSON object"))?;
            if fields.contains_key("action") {
                return Err(CliError::usage("choose request must not supply an action"));
            }
            fields.insert("action".into(), serde_json::json!("previewChoose"));
            serde_json::from_value(request).map_err(error)?
        }
        ["policy-preview", id] => Action::PreviewPolicy {
            policy_set_id: layrs_core::scm::ObjectId::parse(*id).map_err(error)?,
        },
        ["policy-preview-rules", file] => Action::PreviewPolicyRules {
            rules: json_file(file)?,
        },
        ["policy-read"] => Action::ReadPolicyRules {},
        ["policy-replace", policy, file] => Action::PreviewReplacePolicyRules {
            policy_id: serde_json::from_value(serde_json::json!(policy)).map_err(error)?,
            rules: json_file(file)?,
        },
        ["publish-partial", sequence, layer] => Action::PublishProjection {
            patch_id: sequence
                .parse()
                .map_err(|_| CliError::usage("invalid capture sequence"))?,
            layer_id: layrs_core::scm::LayerId::parse(*layer).map_err(error)?,
        },
        ["candidate-partial", sequence, layer] => Action::PublishProjectionCandidate {
            patch_id: sequence
                .parse()
                .map_err(|_| CliError::usage("invalid capture sequence"))?,
            layer_id: layrs_core::scm::LayerId::parse(*layer).map_err(error)?,
        },
        ["scoped-review", file] => serde_json::from_value(
            serde_json::json!({"action":"scopedReview", "command":json_file(file)?}),
        )
        .map_err(error)?,
        ["partial-directory-preview", generation, source, destination] => {
            Action::PreviewProjectionDirectoryMove {
                expected_generation: generation
                    .parse()
                    .map_err(|_| CliError::usage("invalid projection generation"))?,
                source: layrs_core::scm::RepoPath::parse(*source).map_err(error)?,
                destination: layrs_core::scm::RepoPath::parse(*destination).map_err(error)?,
            }
        }
        ["partial-directory-apply", file] => {
            let document = json_file(file)?;
            let preview = document.get("data").unwrap_or(&document);
            serde_json::from_value(
                serde_json::json!({"action":"applyProjectionDirectoryMove","preview":preview}),
            )
            .map_err(error)?
        }
        ["receive-partial", layer] => Action::ReceiveProjection {
            layer_id: layrs_core::scm::LayerId::parse(*layer).map_err(error)?,
        },
        ["migrate-partial", layer, destination] => Action::MigrateProjection {
            layer_id: layrs_core::scm::LayerId::parse(*layer).map_err(error)?,
            destination: (*destination).to_owned(),
        },
        ["partial-restore-preview", file] => {
            let mut request = json_file(file)?;
            let fields = request
                .as_object_mut()
                .ok_or_else(|| CliError::usage("restore request must be a JSON object"))?;
            if fields.contains_key("action") {
                return Err(CliError::usage("restore request must not supply an action"));
            }
            fields.insert(
                "action".into(),
                serde_json::json!("previewProjectionRestore"),
            );
            serde_json::from_value(request).map_err(error)?
        }
        ["partial-restore-apply", file] => {
            let document = json_file(file)?;
            let preview = document.get("data").unwrap_or(&document);
            serde_json::from_value(
                serde_json::json!({"action":"applyProjectionRestore","preview":preview}),
            )
            .map_err(error)?
        }
        [action @ ("choose-apply" | "policy-apply"), file] => {
            let document = json_file(file)?;
            let preview = document
                .get("data")
                .and_then(|v| v.get("preview"))
                .or_else(|| document.get("preview"))
                .unwrap_or(&document);
            serde_json::from_value(serde_json::json!({"action":if *action == "policy-apply" {"applyPolicy"} else {"applyChoose"},"preview":preview}))
                .map_err(error)?
        }
        ["teams"] => administration(serde_json::json!({"action":"listTeams"}))?,
        ["policy-management"] => administration(serde_json::json!({"action":"policyManagement"}))?,
        ["team-members"] => administration(serde_json::json!({"action":"eligibleTeamMembers"}))?,
        ["team-create", file] => {
            administration(serde_json::json!({"action":"createTeam","request":json_file(file)?}))?
        }
        ["team-update", team, file] => administration(
            serde_json::json!({"action":"updateTeam","team":team,"request":json_file(file)?}),
        )?,
        ["team-archive", team, file] => administration(
            serde_json::json!({"action":"archiveTeam","team":team,"request":json_file(file)?}),
        )?,
        ["team-history", team] => administration(
            serde_json::json!({"action":"teamHistory","team":team,"after":0,"limit":20,"anchor":null}),
        )?,
        ["team-history", team, after, anchor] => administration(
            serde_json::json!({"action":"teamHistory","team":team,"after":after.parse::<u32>().map_err(|_| CliError::usage("invalid history position"))?,"limit":20,"anchor":anchor.parse::<u32>().map_err(|_| CliError::usage("invalid history anchor"))?}),
        )?,
        ["metrics"] => Action::Administration {
            command: layrs_client_core::native_scm::AdministrationCommand::Metrics {},
        },
        ["storage-budget"] => Action::Administration {
            command: layrs_client_core::native_scm::AdministrationCommand::StorageBudget {
                space: None,
            },
        },
        ["invitations-inbox"] => Action::Administration {
            command: layrs_client_core::native_scm::AdministrationCommand::InvitationInbox {
                after: 0,
            },
        },
        ["invitations-inbox", after] => Action::Administration {
            command: layrs_client_core::native_scm::AdministrationCommand::InvitationInbox {
                after: after.parse().map_err(|_| {
                    CliError::usage("invitation cursor must be a non-negative integer")
                })?,
            },
        },
        ["invitations"] => Action::Administration {
            command: layrs_client_core::native_scm::AdministrationCommand::ListInvitations {
                after: 0,
            },
        },
        ["invitations", after] => Action::Administration {
            command: layrs_client_core::native_scm::AdministrationCommand::ListInvitations {
                after: after.parse().map_err(|_| {
                    CliError::usage("invitation cursor must be a non-negative integer")
                })?,
            },
        },
        ["invite-member", file] => Action::Administration {
            command: layrs_client_core::native_scm::AdministrationCommand::InviteMember {
                request: json_file(file)?,
            },
        },
        ["invitation-decision", invitation, decision] => Action::Administration {
            command: layrs_client_core::native_scm::AdministrationCommand::DecideInvitation {
                invitation: (*invitation).into(),
                decision: (*decision).into(),
            },
        },
        ["storage-budget", space] => Action::Administration {
            command: layrs_client_core::native_scm::AdministrationCommand::StorageBudget {
                space: Some(layrs_core::scm::SpaceId::parse(*space).map_err(error)?),
            },
        },
        ["reviews"] => {
            collaboration(serde_json::json!({"action":"listReviewsPage","cursor":null,"limit":50}))?
        }
        ["reviews", cursor] => collaboration(
            serde_json::json!({"action":"listReviewsPage","cursor":cursor,"limit":50}),
        )?,
        ["review", review] => {
            collaboration(serde_json::json!({"action":"review","review":review}))?
        }
        ["review-propose", file] => collaboration(
            serde_json::json!({"action":"propose","review":null,"request":json_file(file)?}),
        )?,
        ["review-update", review, file] => collaboration(
            serde_json::json!({"action":"propose","review":review,"request":json_file(file)?}),
        )?,
        ["review-action", review, file] => collaboration(
            serde_json::json!({"action":"decide","review":review,"request":json_file(file)?}),
        )?,
        ["review-correct", review, comment, file] => collaboration(
            serde_json::json!({"action":"correctComment","review":review,"comment":comment,"request":json_file(file)?}),
        )?,
        ["runs"] => {
            collaboration(serde_json::json!({"action":"listRunsPage","cursor":null,"limit":50}))?
        }
        ["runs", cursor] => {
            collaboration(serde_json::json!({"action":"listRunsPage","cursor":cursor,"limit":50}))?
        }
        ["run", run] => collaboration(serde_json::json!({"action":"runDetail","run":run}))?,
        ["run-cancel", run, file] => collaboration(
            serde_json::json!({"action":"cancelRun","run":run,"request":json_file(file)?}),
        )?,
        ["history", anchor] => collaboration(
            serde_json::json!({"action":"stepHistory","anchor":anchor,"cursor":null,"limit":30,"path":history_path,"filter":history_filter}),
        )?,
        ["history", anchor, cursor] => collaboration(
            serde_json::json!({"action":"stepHistory","anchor":anchor,"cursor":cursor,"limit":30,"path":history_path,"filter":history_filter}),
        )?,
        ["compare", before, after] => {
            collaboration(serde_json::json!({"action":"compare","before":before,"after":after}))?
        }
        ["releases"] => collaboration(serde_json::json!({"action":"listReleases"}))?,
        ["release-events", release] => {
            collaboration(serde_json::json!({"action":"releasePublications","release":release}))?
        }
        ["release-publish", release] if reason.is_none() => collaboration(
            serde_json::json!({"action":"publishRelease","release":release,"request":publication_request()?}),
        )?,
        ["release-revoke", release, publication] => {
            let mut request = publication_request()?;
            request["reason"] = serde_json::Value::String(
                reason
                    .clone()
                    .ok_or_else(|| CliError::usage("release-revoke requires --reason TEXT"))?,
            );
            collaboration(
                serde_json::json!({"action":"revokeRelease","release":release,"publication":publication,"request":request}),
            )?
        }
        ["provision", workspace, slug] => Action::Provision {
            workspace: (*workspace).into(),
            slug: (*slug).into(),
        },
        ["publish"] => Action::Publish { integrate: false },
        ["publish-candidates"] => Action::PublishCandidates,
        ["publish", "--integrate"] => Action::Publish { integrate: true },
        ["receive"] => Action::Receive,
        ["clone-partial", space, step, path] => {
            root = PathBuf::from(path);
            Action::CloneProjection {
                space: layrs_core::scm::SpaceId::parse(*space).map_err(error)?,
                step: layrs_core::scm::StepId::parse(*step).map_err(error)?,
            }
        }
        ["clone", space, path] => {
            root = PathBuf::from(path);
            Action::Clone {
                space: layrs_core::scm::SpaceId::parse(*space).map_err(error)?,
                name: (*space).into(),
            }
        }
        _ => {
            return Err(CliError::usage(
                "usage: layrs remote login | whoami | logout | metrics | storage-budget [SPACE_ID] | provision WORKSPACE_SLUG SPACE_SLUG | publish [--integrate] | receive | clone SPACE_ID EMPTY_DIRECTORY --endpoint HTTPS_URL [--ca FILE]",
            ));
        }
    };
    let login = matches!(action, Action::StartLogin);
    let choose_apply = matches!(&action, Action::ApplyChoose { .. });
    let policy_apply = matches!(&action, Action::ApplyPolicy { .. });
    let file_read = matches!(&action, Action::ReadFile { .. });
    let observation = matches!(&action, Action::ObserveStep { .. });
    let publication = matches!(&action, Action::Publish { .. } | Action::PublishCandidates);
    let partial_publication = matches!(
        &action,
        Action::PublishProjection { .. } | Action::PublishProjectionCandidate { .. }
    );
    let restore_preview = matches!(
        &action,
        Action::PreviewProjectionRestore { .. } | Action::PreviewProjectionDirectoryMove { .. }
    );
    let mut data = crate::native_interrupt::run_remote(
        &root,
        NativeRemoteRequest {
            origin: origin.clone(),
            ca_pem: ca_pem.clone(),
            action,
        },
    )
    .map_err(error)?;
    if partial_publication && data["receipt"]["receipt"]["status"] != "applied" {
        return Err(CliError::runtime(
            "Partial publication refused; the durable receipt is retained. Receive current state and verify permissions before preparing another capture.",
        ));
    }
    if partial_publication && data["pendingLocalAcceptance"] == true {
        return Err(CliError::runtime(
            "Partial publication committed remotely; local acceptance is pending. Retry the same capture sequence to finish it.",
        ));
    }
    if publication && data["receipt"]["status"] != "applied" {
        return Err(CliError::runtime(
            "Remote publication was refused. Receive the current View and verify permissions, gates and heads before retrying.",
        ));
    }
    if choose_apply && data["receipt"]["status"] != "applied" {
        return Err(CliError::runtime(
            "Remote choose was refused. Check permissions and gates; recreate the preview if the remote View changed.",
        ));
    }
    if policy_apply && data["receipt"]["status"] != "applied" {
        return Err(CliError::runtime(
            "Policy change refused. Check current authority and recreate the preview if its expectations changed.",
        ));
    }
    if observation && data["receipt"]["status"] != "applied" {
        return Err(CliError::runtime(
            "Observation refused. Check the admitted Step, permissions and idempotency key.",
        ));
    }
    if let Some(path) = choose_output.filter(|_| !file_read) {
        use std::io::Write;
        let bytes = serde_json::to_vec_pretty(if restore_preview {
            &data
        } else {
            &data["preview"]
        })
        .map_err(error)?;
        if bytes.len() > 256 * 1024 {
            return Err(CliError::usage(
                "preview exceeds the 256 KiB request file limit",
            ));
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(error)?;
        file.write_all(&bytes).map_err(error)?;
        file.sync_all().map_err(error)?;
    }
    if login {
        eprintln!(
            "Open {} and approve code {}",
            data["verificationUri"].as_str().unwrap_or(""),
            data["userCode"].as_str().unwrap_or("")
        );
        let until = Instant::now()
            + Duration::from_secs(data["expiresIn"].as_u64().unwrap_or(600).min(900));
        while data["status"] == "pending" {
            if Instant::now() >= until {
                return Err(CliError::runtime(
                    "Device login expired; run remote login again.",
                ));
            }
            std::thread::sleep(Duration::from_secs(
                data["interval"].as_u64().unwrap_or(5).clamp(1, 30),
            ));
            data = execute_remote(
                &root,
                NativeRemoteRequest {
                    origin: origin.clone(),
                    ca_pem: ca_pem.clone(),
                    action: Action::PollLogin,
                },
            )
            .map_err(error)?;
        }
    }
    let human = if data.get("received").is_some_and(|v| v["divergent"] == true) {
        "Received. Divergent heads preserved; integrate before editing this Layer.".into()
    } else if let Some(status) = data["status"].as_str() {
        status.into()
    } else {
        serde_json::to_string_pretty(&data).map_err(error)?
    };
    Ok(Rendered { human, data })
}
fn collaboration(value: serde_json::Value) -> Result<Action, CliError> {
    Ok(Action::Collaboration {
        command: serde_json::from_value(value).map_err(error)?,
    })
}
fn administration(value: serde_json::Value) -> Result<Action, CliError> {
    Ok(Action::Administration {
        command: serde_json::from_value(value).map_err(error)?,
    })
}
fn json_file(path: &str) -> Result<serde_json::Value, CliError> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(error)?
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() > 256 * 1024 {
        return Err(CliError::usage("review request exceeds 256 KiB"));
    }
    serde_json::from_slice(&bytes).map_err(error)
}
fn option(args: &mut Vec<String>, name: &str) -> Result<Option<String>, CliError> {
    let Some(at) = args.iter().position(|a| a == name) else {
        return Ok(None);
    };
    if at + 1 >= args.len() || args[at + 1].starts_with("--") {
        return Err(CliError::usage(format!("{name} needs a value")));
    }
    args.remove(at);
    let value = args.remove(at);
    if args.iter().any(|a| a == name) {
        return Err(CliError::usage(format!("duplicate {name}")));
    }
    Ok(Some(value))
}
fn error(e: impl std::fmt::Display) -> CliError {
    CliError::runtime(e.to_string())
}
