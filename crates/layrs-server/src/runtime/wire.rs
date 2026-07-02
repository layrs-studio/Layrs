use crate::auth::{
    AuthError, AuthSession, DesktopBootstrap, DeviceStart, UserPrincipal, session_cookie_name,
};
use crate::{AuthRequirement, HttpMethod};

use super::RuntimeConfig;

pub(super) fn session_cookie_header(token: &str) -> String {
    format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800",
        session_cookie_name(),
        token
    )
}

pub(super) fn expired_session_cookie() -> String {
    format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
        session_cookie_name()
    )
}

pub(super) fn required_json_field(body: &str, field: &str) -> Result<String, AuthError> {
    json_string_field(body, field)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AuthError::invalid(format!("{field} is required")))
}

pub(super) fn json_string_field(body: &str, field: &str) -> Option<String> {
    let key = format!(r#""{}""#, field);
    let key_start = body.find(&key)?;
    let after_key = &body[key_start + key.len()..];
    let colon_offset = after_key.find(':')?;
    let after_colon = after_key[colon_offset + 1..].trim_start();
    let value_start = after_colon.strip_prefix('"')?;
    let mut escaped = false;
    let mut value = String::new();

    for character in value_start.chars() {
        if escaped {
            match character {
                '"' => value.push('"'),
                '\\' => value.push('\\'),
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                other => value.push(other),
            }
            escaped = false;
            continue;
        }

        match character {
            '\\' => escaped = true,
            '"' => return Some(value),
            other => value.push(other),
        }
    }

    None
}

pub(super) fn health_json(config: &RuntimeConfig) -> String {
    format!(
        r#"{{"service":"layrs-server","status":"ok","runtime":"std-http","auth_store":"dev-memory","database_url_configured":{},"deployment_id":"{}"}}"#,
        config.database_url_configured(),
        escape_json(&config.deployment_id)
    )
}

pub(super) fn session_json(session: &AuthSession) -> String {
    auth_session_json(&session.user)
}

pub(super) fn me_json(user: &UserPrincipal) -> String {
    format!(r#"{{"user":{}}}"#, user_json(user))
}

pub(super) fn auth_session_json(user: &UserPrincipal) -> String {
    format!(
        r#"{{"state":"authenticated","account":{},"session":{},"workspaces":[{}],"activeWorkspaceId":"workspace-dev"}}"#,
        studio_account_json(user),
        studio_session_json(user),
        workspace_json()
    )
}

pub(super) fn studio_snapshot_json(user: &UserPrincipal) -> String {
    let workspace = workspace_json();
    format!(
        r#"{{"account":{},"session":{},"workspace":{},"workspaces":[{}],"teams":[{}],"spaces":[{}],"layers":[{},{}],"artifacts":[{},{}],"steps":[],"weaves":[],"proofs":[],"gates":[],"policies":[],"timeline":[],"accessRegistries":[{}],"devices":[{}],"auditEvents":[{}]}}"#,
        studio_account_json(user),
        studio_session_json(user),
        workspace,
        workspace_json(),
        team_json(),
        space_json(),
        main_layer_json(),
        restricted_layer_json(),
        public_artifact_json(),
        redacted_artifact_json(),
        access_registry_json(),
        device_json(user),
        audit_event_json(user)
    )
}

pub(super) fn user_json(user: &UserPrincipal) -> String {
    format!(
        r#"{{"id":"{}","email":"{}","display_name":"{}"}}"#,
        escape_json(&user.id),
        escape_json(&user.email),
        escape_json(&user.display_name)
    )
}

pub(super) fn desktop_account_json(user: &UserPrincipal) -> String {
    format!(
        r#"{{"id":"{}","email":"{}","displayName":"{}"}}"#,
        escape_json(&user.id),
        escape_json(&user.email),
        escape_json(&user.display_name)
    )
}

pub(super) fn studio_account_json(user: &UserPrincipal) -> String {
    format!(
        r#"{{"id":"{}","email":"{}","name":"{}","role":"owner","avatarInitials":"{}","createdAt":"2026-06-29T00:00:00Z"}}"#,
        escape_json(&user.id),
        escape_json(&user.email),
        escape_json(&user.display_name),
        escape_json(&avatar_initials(&user.display_name))
    )
}

pub(super) fn studio_session_json(user: &UserPrincipal) -> String {
    format!(
        r#"{{"id":"session-dev","accountId":"{}","activeWorkspaceId":"workspace-dev","expiresAt":"2026-07-06T00:00:00Z","createdAt":"2026-06-29T00:00:00Z"}}"#,
        escape_json(&user.id)
    )
}

pub(super) fn workspace_json() -> String {
    workspace_json_with_id(
        "workspace-dev",
        "Layrs Studio",
        "layrs-studio",
        "Development workspace for validating Studio server workflows.",
    )
}

pub(super) fn workspace_json_with(name: &str, slug: &str, description: &str) -> String {
    let id = format!("workspace-{}", slugify(slug));
    workspace_json_with_id(&id, name, slug, description)
}

pub(super) fn workspace_json_with_id(
    id: &str,
    name: &str,
    slug: &str,
    description: &str,
) -> String {
    format!(
        r#"{{"id":"{}","name":"{}","slug":"{}","description":"{}","health":"pending","updatedAt":"2026-06-29T20:30:00Z"}}"#,
        escape_json(id),
        escape_json(name),
        escape_json(slug),
        escape_json(description)
    )
}

pub(super) fn team_json() -> String {
    r#"{"id":"team-art","workspaceId":"workspace-dev","name":"Art Team","purpose":"Owns restricted visual assets and texture reviews.","members":1,"gateResponsibility":"asset access"}"#.to_string()
}

pub(super) fn space_json() -> String {
    r#"{"id":"space-game","workspaceId":"workspace-dev","teamId":"team-art","name":"Game Prototype","description":"Generalist Layrs Space with code and image artifacts.","status":"pending","currentLayerId":"layer-main","updatedAt":"2026-06-29T20:30:00Z"}"#.to_string()
}

pub(super) fn main_layer_json() -> String {
    r#"{"id":"layer-main","spaceId":"space-game","name":"Main","kind":"base","status":"active","summary":"Primary working layer with inherited access rules.","artifactIds":["artifact-readme","artifact-hero-texture"],"stepIds":[],"gateIds":[]}"#.to_string()
}

pub(super) fn restricted_layer_json() -> String {
    r#"{"id":"layer-art-private","spaceId":"space-game","parentId":"layer-main","name":"Art Private","kind":"proposal","status":"review","summary":"Child layer carrying restricted image work.","artifactIds":["artifact-hero-texture"],"stepIds":[],"gateIds":[]}"#.to_string()
}

pub(super) fn public_artifact_json() -> String {
    r#"{"id":"artifact-readme","spaceId":"space-game","layerId":"layer-main","name":"README.md","type":"file","summary":"Public project notes.","location":"README.md","updatedAt":"2026-06-29T20:30:00Z","sizeLabel":"2 KB","proofIds":[],"access":{"mode":"read","canOpen":true,"isRedacted":false}}"#.to_string()
}

pub(super) fn redacted_artifact_json() -> String {
    r#"{"id":"artifact-hero-texture","spaceId":"space-game","layerId":"layer-main","name":"hero.texture.png","type":"image","summary":"Restricted by Layer access policy","location":"Assets/Private/hero.texture.png","updatedAt":"2026-06-29T20:30:00Z","sizeLabel":"redacted","proofIds":[],"access":{"mode":"none","canOpen":false,"isRedacted":true,"reason":"Restricted by Layer access policy"}}"#.to_string()
}

pub(super) fn access_registry_json() -> String {
    r#"{"id":"registry-layer-main","workspaceId":"workspace-dev","layerId":"layer-main","rules":[{"id":"access-rule-art-team","subjectKind":"team","subjectId":"team-art","subjectName":"Art Team","mode":"read"}],"updatedAt":"2026-06-29T20:30:00Z"}"#.to_string()
}

pub(super) fn device_json(user: &UserPrincipal) -> String {
    format!(
        r#"{{"id":"device-browser-dev","accountId":"{}","name":"Studio Web dev session","kind":"browser","status":"trusted","lastSeenAt":"2026-06-29T20:30:00Z"}}"#,
        escape_json(&user.id)
    )
}

pub(super) fn audit_event_json(user: &UserPrincipal) -> String {
    format!(
        r#"{{"id":"audit-dev-login","workspaceId":"workspace-dev","actorAccountId":"{}","action":"auth.login","target":"studio","summary":"Signed in to Layrs Studio.","at":"2026-06-29T20:30:00Z"}}"#,
        escape_json(&user.id)
    )
}

pub(super) fn device_start_json(start: &DeviceStart) -> String {
    format!(
        r#"{{"device_code":"{}","user_code":"{}","verification_uri":"{}","interval_seconds":{},"expires_in_seconds":{}}}"#,
        escape_json(&start.device_code),
        escape_json(&start.user_code),
        escape_json(&start.verification_uri),
        start.interval_seconds,
        start.expires_in_seconds
    )
}

pub(super) fn desktop_bootstrap_json(bootstrap: &DesktopBootstrap) -> String {
    format!(
        r#"{{"user":{},"account":{},"workspaces":[{{"id":"workspace-dev","name":"Layrs Studio","slug":"layrs-studio"}}],"spaces":[{{"id":"space-game","workspaceId":"workspace-dev","name":"Game Prototype","currentLayerId":"layer-main"}}],"layers":[{{"id":"layer-main","workspaceId":"workspace-dev","spaceId":"space-game","name":"Main","kind":"base","access":"open"}},{{"id":"layer-art-private","workspaceId":"workspace-dev","spaceId":"space-game","name":"Art Private","kind":"proposal","parentLayerId":"layer-main","access":"redacted"}}],"server":{{"mode":"{}","database_url_configured":{},"routes_path":"{}"}}}}"#,
        user_json(&bootstrap.user),
        desktop_account_json(&bootstrap.user),
        escape_json(&bootstrap.server_mode),
        bootstrap.database_url_configured,
        escape_json(&bootstrap.routes_path)
    )
}

pub(super) fn error_json(code: &str, message: &str) -> String {
    format!(
        r#"{{"error":{{"code":"{}","message":"{}"}}}}"#,
        escape_json(code),
        escape_json(message)
    )
}

pub(super) fn method_label(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
    }
}

pub(super) fn auth_label(auth: AuthRequirement) -> &'static str {
    match auth {
        AuthRequirement::Public => "public",
        AuthRequirement::Session => "session",
        AuthRequirement::Device => "device",
        AuthRequirement::Principal => "principal",
        AuthRequirement::WorkspaceRead => "workspace_read",
        AuthRequirement::WorkspaceWrite => "workspace_write",
        AuthRequirement::WorkspaceAdmin => "workspace_admin",
    }
}

pub(super) fn avatar_initials(value: &str) -> String {
    let mut initials = value
        .split_whitespace()
        .filter_map(|part| part.chars().next())
        .filter(|character| character.is_ascii_alphabetic())
        .take(2)
        .collect::<String>()
        .to_ascii_uppercase();

    if initials.is_empty() {
        initials = "LA".to_string();
    }

    initials
}

pub(super) fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;

    for character in value.trim().to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    if slug.is_empty() {
        "workspace".to_string()
    } else {
        slug
    }
}

pub(super) fn escape_json(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}
