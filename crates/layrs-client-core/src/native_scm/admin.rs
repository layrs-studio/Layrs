//! Explicit administration routes. Secrets have no Debug implementation and
//! flow only to the requesting interface, never the OS session credential slot.
use super::{remote::RemoteResult, NativeRemote, NativeRemoteError};
use layrs_core::scm::{LayerId, SpaceId};
use layrs_sync::v3::PrincipalId;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeTeamRole {
    Member,
    Auditor,
    ReleaseManager,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeTeamRequest {
    pub idempotency_key: String,
    pub expected_version: u32,
    pub name: String,
    pub role: NativeTeamRole,
    pub member_principal_ids: Vec<PrincipalId>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeArchiveTeamRequest {
    pub idempotency_key: String,
    pub expected_version: u32,
    pub reason: String,
}
#[derive(Deserialize, Serialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AdministrationCommand {
    ListTeams {},
    PolicyManagement {},
    EligibleTeamMembers {},
    TeamHistory {
        team: String,
        after: u32,
        limit: u16,
        anchor: Option<u32>,
    },
    CreateTeam {
        request: NativeTeamRequest,
    },
    UpdateTeam {
        team: String,
        request: NativeTeamRequest,
    },
    ArchiveTeam {
        team: String,
        request: NativeArchiveTeamRequest,
    },
    Metrics {},
    InvitationInbox {
        after: u64,
    },
    ListInvitations {
        after: u64,
    },
    InviteMember {
        request: Value,
    },
    DecideInvitation {
        invitation: String,
        decision: String,
    },
    StorageBudget {
        space: Option<SpaceId>,
    },
    ListAccounts,
    CreateAccount {
        request: Value,
    },
    RevokeAccount {
        principal_id: PrincipalId,
    },
    CreateService {
        request: Value,
    },
    RotateService {
        request: Value,
    },
    ListTokens,
    RevokeToken {
        token_id: String,
    },
    GrantMember {
        request: Value,
    },
    Protection {
        layer: LayerId,
    },
    ConfigureProtection {
        layer: LayerId,
        request: Value,
    },
    ListWebhooks,
    CreateWebhook {
        request: Value,
    },
    WebhookDeliveries {
        webhook: String,
    },
    RotateWebhook {
        webhook: String,
    },
    DisableWebhook {
        webhook: String,
    },
    ReplayWebhook {
        webhook: String,
        delivery_id: String,
    },
    Notifications {
        after: u64,
    },
    MarkNotificationsRead {
        last_sequence: u64,
    },
}
impl AdministrationCommand {
    pub(super) fn needs_space(&self) -> bool {
        !matches!(
            self,
            Self::ListAccounts
                | Self::Metrics { .. }
                | Self::InvitationInbox { .. }
                | Self::DecideInvitation { .. }
                | Self::StorageBudget { .. }
                | Self::CreateAccount { .. }
                | Self::RevokeAccount { .. }
                | Self::CreateService { .. }
                | Self::RotateService { .. }
                | Self::ListTokens
                | Self::RevokeToken { .. }
        )
    }
}
pub(super) fn execute(
    remote: &NativeRemote,
    space: Option<&SpaceId>,
    command: AdministrationCommand,
) -> RemoteResult<Value> {
    let base = match space {
        Some(space) => format!("v3/spaces/{space}"),
        None if command.needs_space() => {
            return Err(NativeRemoteError::Invalid(
                "space required for administration",
            ));
        }
        None => String::new(),
    };
    let (method, path, body) = match command {
        AdministrationCommand::ListTeams {} => (Method::GET, format!("{base}/teams"), None),
        AdministrationCommand::PolicyManagement {} => {
            (Method::GET, format!("{base}/policy-management"), None)
        }
        AdministrationCommand::EligibleTeamMembers {} => {
            (Method::GET, format!("{base}/teams/eligible-members"), None)
        }
        AdministrationCommand::TeamHistory {
            team,
            after,
            limit,
            anchor,
        } => {
            team_identifier(&team)?;
            if !(1..=100).contains(&limit)
                || after > 1000
                || anchor.is_some_and(|a| a > 1000 || a < after)
            {
                return Err(NativeRemoteError::Invalid("invalid team history page"));
            }
            (
                Method::GET,
                format!(
                    "{base}/teams/{team}/history?after={after}&limit={limit}{}",
                    anchor.map(|v| format!("&anchor={v}")).unwrap_or_default()
                ),
                None,
            )
        }
        AdministrationCommand::CreateTeam { request } => {
            (Method::POST, format!("{base}/teams"), Some(json!(request)))
        }
        AdministrationCommand::UpdateTeam { team, request } => {
            team_identifier(&team)?;
            (
                Method::PUT,
                format!("{base}/teams/{team}"),
                Some(json!(request)),
            )
        }
        AdministrationCommand::ArchiveTeam { team, request } => {
            team_identifier(&team)?;
            (
                Method::POST,
                format!("{base}/teams/{team}/archive"),
                Some(json!(request)),
            )
        }
        AdministrationCommand::Metrics {} => (Method::GET, "v3/admin/metrics".into(), None),
        AdministrationCommand::InvitationInbox { after } => {
            (Method::GET, format!("v3/invitations?after={after}"), None)
        }
        AdministrationCommand::ListInvitations { after } => (
            Method::GET,
            format!("{base}/invitations?after={after}"),
            None,
        ),
        AdministrationCommand::InviteMember { request } => {
            (Method::POST, format!("{base}/invitations"), Some(request))
        }
        AdministrationCommand::DecideInvitation {
            invitation,
            decision,
        } => {
            if invitation.len() != 75
                || !invitation.starts_with("invitation:")
                || !invitation[11..].bytes().all(|b| b.is_ascii_hexdigit())
                || !matches!(decision.as_str(), "accept" | "reject" | "revoke")
            {
                return Err(NativeRemoteError::Invalid("invalid invitation decision"));
            }
            (
                Method::POST,
                format!("v3/invitations/{invitation}/actions"),
                Some(json!({"action":decision})),
            )
        }
        AdministrationCommand::StorageBudget { space } => {
            let query = space
                .map(|space| {
                    format!(
                        "?{}",
                        url::form_urlencoded::Serializer::new(String::new())
                            .append_pair("spaceId", space.as_str())
                            .finish()
                    )
                })
                .unwrap_or_default();
            (Method::GET, format!("v3/admin/storage-budget{query}"), None)
        }
        AdministrationCommand::ListAccounts => (Method::GET, "v3/auth/accounts".into(), None),
        AdministrationCommand::CreateAccount { request } => {
            (Method::POST, "v3/auth/accounts".into(), Some(request))
        }
        AdministrationCommand::RevokeAccount { principal_id } => (
            Method::POST,
            "v3/auth/accounts/revoke".into(),
            Some(json!({"principalId":principal_id})),
        ),
        AdministrationCommand::CreateService { request } => (
            Method::POST,
            "v3/auth/service-accounts".into(),
            Some(request),
        ),
        AdministrationCommand::RotateService { request } => (
            Method::POST,
            "v3/auth/service-accounts/rotate".into(),
            Some(request),
        ),
        AdministrationCommand::ListTokens => (Method::GET, "v3/auth/tokens".into(), None),
        AdministrationCommand::RevokeToken { token_id } => (
            Method::POST,
            "v3/auth/tokens/revoke".into(),
            Some(json!({"tokenId":token_id})),
        ),
        AdministrationCommand::GrantMember { request } => {
            (Method::POST, format!("{base}/members"), Some(request))
        }
        AdministrationCommand::Protection { layer } => (
            Method::GET,
            format!("{base}/layers/{layer}/protection"),
            None,
        ),
        AdministrationCommand::ConfigureProtection { layer, request } => (
            Method::PUT,
            format!("{base}/layers/{layer}/protection"),
            Some(request),
        ),
        AdministrationCommand::ListWebhooks => (Method::GET, format!("{base}/webhooks"), None),
        AdministrationCommand::CreateWebhook { request } => {
            (Method::POST, format!("{base}/webhooks"), Some(request))
        }
        AdministrationCommand::WebhookDeliveries { webhook } => (
            Method::GET,
            format!("{base}/webhooks/{}/deliveries", segment(&webhook)?),
            None,
        ),
        AdministrationCommand::RotateWebhook { webhook } => (
            Method::POST,
            format!("{base}/webhooks/{}/rotate", segment(&webhook)?),
            Some(json!({})),
        ),
        AdministrationCommand::DisableWebhook { webhook } => (
            Method::POST,
            format!("{base}/webhooks/{}/disable", segment(&webhook)?),
            Some(json!({})),
        ),
        AdministrationCommand::ReplayWebhook {
            webhook,
            delivery_id,
        } => (
            Method::POST,
            format!("{base}/webhooks/{}/replay", segment(&webhook)?),
            Some(json!({"deliveryId":delivery_id})),
        ),
        AdministrationCommand::Notifications { after } => (
            Method::GET,
            format!("{base}/notifications?after={after}"),
            None,
        ),
        AdministrationCommand::MarkNotificationsRead { last_sequence } => (
            Method::POST,
            format!("{base}/notifications/read"),
            Some(json!({"lastSequence":last_sequence})),
        ),
    };
    if body.as_ref().is_some_and(|body| {
        serde_json::to_vec(body).map_or(true, |bytes| bytes.len() > 1024 * 1024)
    }) {
        return Err(NativeRemoteError::Invalid(
            "administration request too large",
        ));
    }
    remote.admin_request(method, &path, body.as_ref())
}
fn segment(value: &str) -> RemoteResult<&str> {
    if !value.starts_with("webhook:")
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b))
    {
        return Err(NativeRemoteError::Invalid("invalid webhook identifier"));
    }
    Ok(value)
}

fn team_identifier(value: &str) -> RemoteResult<()> {
    if value.len() != 69
        || !value.starts_with("team:")
        || !value[5..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(NativeRemoteError::Invalid("invalid team identifier"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn team_commands_have_closed_scoped_roles_and_explicit_versions() {
        let request = json!({"idempotencyKey":"teams-client-test-001","expectedVersion":0,"name":"Engineering","role":"member","memberPrincipalIds":[]});
        let command: AdministrationCommand =
            serde_json::from_value(json!({"action":"createTeam","request":request})).unwrap();
        assert!(command.needs_space());
        assert_eq!(
            serde_json::to_value(command).unwrap()["request"]["expectedVersion"],
            0
        );
        let mut invalid = request.clone();
        invalid["role"] = json!("administrator");
        assert!(serde_json::from_value::<NativeTeamRequest>(invalid).is_err());
        let mut invalid = request;
        invalid["token"] = json!("not-a-secret-storage-field");
        assert!(serde_json::from_value::<NativeTeamRequest>(invalid).is_err());
        assert!(team_identifier(&format!("team:{}", "a".repeat(64))).is_ok());
        assert!(team_identifier(&format!("team:{}", "A".repeat(64))).is_err());
        assert!(team_identifier("team:../other").is_err());
    }
    #[test]
    fn metrics_is_an_explicit_global_read_without_project_or_secret_fields() {
        let command: AdministrationCommand =
            serde_json::from_value(json!({"action":"metrics"})).unwrap();
        assert!(!command.needs_space());
        assert!(matches!(command, AdministrationCommand::Metrics {}));
        assert!(serde_json::from_value::<AdministrationCommand>(
            json!({"action":"metrics","token":"forbidden"})
        )
        .is_err());
    }
}
