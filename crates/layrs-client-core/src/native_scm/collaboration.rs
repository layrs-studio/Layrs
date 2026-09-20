//! Typed collaboration requests shared by interactive clients.
use super::{remote::RemoteResult, NativeRemote, NativeRemoteError};
use layrs_core::scm::{
    LayerId, ObjectId, ReleaseId, ReleasePublicationId, RepoPath, SpaceId, StepId,
};
use layrs_sync::v3::PrincipalId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishedHistoryFilter {
    pub actor_id: Option<layrs_core::scm::ActorId>,
    pub since: Option<layrs_core::scm::Timestamp>,
    pub until: Option<layrs_core::scm::Timestamp>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewProposal {
    pub idempotency_key: String,
    pub layer_id: LayerId,
    pub proposed_step_id: StepId,
    pub title: String,
    pub participants: Vec<PrincipalId>,
    pub expected_version: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewAnchor {
    pub path: RepoPath,
    pub start_line: u32,
    pub end_line: u32,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Comment,
    Approve,
    RequestChanges,
    Close,
    Reopen,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewAction {
    pub idempotency_key: String,
    pub version: u32,
    pub action: ReviewDecision,
    pub body: String,
    pub anchor: Option<ReviewAnchor>,
    pub reply_to: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelRunRequest {
    pub idempotency_key: String,
    pub reason: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorrectCommentRequest {
    pub idempotency_key: String,
    pub version: u32,
    pub expected_revision: u32,
    pub body: String,
    pub reason: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CollaborationCommand {
    StepHistory {
        anchor: StepId,
        cursor: Option<String>,
        limit: u16,
        #[serde(default)]
        path: Option<RepoPath>,
        #[serde(default)]
        filter: PublishedHistoryFilter,
    },
    ListReleases,
    ReleasePublications {
        release: ReleaseId,
    },
    PublishRelease {
        release: ReleaseId,
        request: PublishReleaseRequest,
    },
    RevokeRelease {
        release: ReleaseId,
        publication: ReleasePublicationId,
        request: RevokeReleaseRequest,
    },
    Compare {
        before: StepId,
        after: StepId,
    },
    RunDetail {
        run: String,
    },
    CancelRun {
        run: String,
        request: CancelRunRequest,
    },
    CorrectComment {
        review: String,
        comment: String,
        request: CorrectCommentRequest,
    },
    ListReviews,
    ListReviewsPage {
        cursor: Option<String>,
        limit: u16,
    },
    Review {
        review: String,
    },
    Propose {
        review: Option<String>,
        request: ReviewProposal,
    },
    Decide {
        review: String,
        request: ReviewAction,
    },
    ListRuns,
    ListRunsPage {
        cursor: Option<String>,
        limit: u16,
    },
    Protection {
        layer: LayerId,
    },
}
impl NativeRemote {
    pub fn collaborate(
        &self,
        space: &SpaceId,
        command: CollaborationCommand,
    ) -> RemoteResult<Value> {
        execute(self, space, command)
    }
}
pub(super) fn execute(
    remote: &NativeRemote,
    space: &SpaceId,
    command: CollaborationCommand,
) -> RemoteResult<Value> {
    let base = format!("v3/spaces/{space}");
    match command {
        CollaborationCommand::ListReleases => remote.get(&format!("{base}/releases")),
        CollaborationCommand::ReleasePublications { release } => {
            remote.get(&format!("{base}/releases/{release}/publications"))
        }
        CollaborationCommand::PublishRelease { release, request } => {
            key(&request.request_id)?;
            remote.post(&format!("{base}/releases/{release}/publications"), &request)
        }
        CollaborationCommand::RevokeRelease {
            release,
            publication,
            request,
        } => {
            key(&request.request_id)?;
            if request.reason.trim().is_empty()
                || request.reason.len() > 4096
                || request.reason.contains('\0')
            {
                return Err(invalid("invalid revocation reason"));
            }
            remote.post(
                &format!("{base}/releases/{release}/publications/{publication}/revoke"),
                &request,
            )
        }
        CollaborationCommand::StepHistory {
            anchor,
            cursor,
            limit,
            path,
            filter,
        } => {
            if !(1..=100).contains(&limit) || cursor.as_ref().is_some_and(|c| c.len() > 4096) {
                return Err(invalid("invalid history page"));
            }
            remote.post(
                &format!("{base}/history"),
                &serde_json::json!({"anchor":anchor,"cursor":cursor,"limit":limit,"path":path,"filter":filter}),
            )
        }
        CollaborationCommand::Compare { before, after } => remote.post(
            &format!("{base}/compare"),
            &serde_json::json!({"before":before,"after":after}),
        ),
        CollaborationCommand::RunDetail { run } => {
            remote.get(&format!("{base}/runs/{}", segment(&run)?))
        }
        CollaborationCommand::CorrectComment {
            review,
            comment,
            request,
        } => {
            key(&request.idempotency_key)?;
            if request.version == 0
                || request.version > 10001
                || request.expected_revision >= 100
                || request.body.trim().is_empty()
                || request.body.len() > 64 * 1024
                || request.body.contains('\0')
                || request.reason.trim().is_empty()
                || request.reason.len() > 4096
                || request.reason.contains('\0')
            {
                return Err(invalid("invalid comment correction"));
            }
            remote.post(
                &format!(
                    "{base}/reviews/{}/actions/{}/corrections",
                    segment(&review)?,
                    segment(&comment)?
                ),
                &request,
            )
        }
        CollaborationCommand::CancelRun { run, request } => {
            key(&request.idempotency_key)?;
            if request.reason.trim().is_empty()
                || request.reason.len() > 4096
                || request.reason.contains('\0')
            {
                return Err(invalid("invalid cancellation reason"));
            }
            remote.post(&format!("{base}/runs/{}/cancel", segment(&run)?), &request)
        }
        CollaborationCommand::ListReviews => remote.get(&format!("{base}/reviews")),
        CollaborationCommand::ListReviewsPage { cursor, limit } => {
            if !(1..=100).contains(&limit)
                || cursor.as_ref().is_some_and(|value| {
                    value.is_empty()
                        || value.len() > 1024
                        || !value
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                })
            {
                return Err(invalid("invalid review page"));
            }
            let query = cursor.map_or_else(String::new, |value| format!("&cursor={value}"));
            remote.get(&format!("{base}/reviews?limit={limit}{query}"))
        }
        CollaborationCommand::Review { review } => {
            remote.get(&format!("{base}/reviews/{}", segment(&review)?))
        }
        CollaborationCommand::Propose { review, request } => {
            key(&request.idempotency_key)?;
            if request.title.trim().is_empty()
                || request.title.len() > 1024
                || request.participants.len() > 100
                || request.expected_version > 10000
            {
                return Err(invalid("invalid review proposal"));
            }
            let path = match review {
                Some(review) => format!("{base}/reviews/{}/versions", segment(&review)?),
                None => format!("{base}/reviews"),
            };
            remote.post(&path, &request)
        }
        CollaborationCommand::Decide { review, request } => {
            key(&request.idempotency_key)?;
            if request.version == 0 || request.version > 10000 || request.body.len() > 64 * 1024 {
                return Err(invalid("invalid review action"));
            }
            if let Some(anchor) = &request.anchor {
                if anchor.start_line == 0
                    || anchor.end_line < anchor.start_line
                    || anchor.end_line > 2_000_000
                    || anchor.end_line - anchor.start_line > 10000
                {
                    return Err(invalid("invalid review anchor"));
                }
            }
            remote.post(
                &format!("{base}/reviews/{}/actions", segment(&review)?),
                &request,
            )
        }
        CollaborationCommand::ListRuns => remote.get(&format!("{base}/runs")),
        CollaborationCommand::ListRunsPage { cursor, limit } => {
            if !(1..=100).contains(&limit)
                || cursor.as_ref().is_some_and(|value| {
                    value.is_empty()
                        || value.len() > 1024
                        || !value
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                })
            {
                return Err(invalid("invalid run page"));
            }
            let query = cursor.map_or_else(String::new, |value| format!("&cursor={value}"));
            remote.get(&format!("{base}/runs?limit={limit}{query}"))
        }
        CollaborationCommand::Protection { layer } => {
            remote.get(&format!("{base}/layers/{layer}/protection"))
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishReleaseRequest {
    pub request_id: String,
    pub expected_last_event_id: Option<ObjectId>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevokeReleaseRequest {
    pub request_id: String,
    pub expected_last_event_id: Option<ObjectId>,
    pub reason: String,
}
fn segment(value: &str) -> RemoteResult<&str> {
    if value.is_empty()
        || value.len() > 160
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_:-".contains(&b))
    {
        return Err(invalid("invalid collaboration identifier"));
    }
    Ok(value)
}
fn key(value: &str) -> RemoteResult<()> {
    if !(16..=128).contains(&value.len())
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
    {
        return Err(invalid("invalid idempotency key"));
    }
    Ok(())
}
fn invalid(message: &'static str) -> NativeRemoteError {
    NativeRemoteError::Invalid(message)
}
