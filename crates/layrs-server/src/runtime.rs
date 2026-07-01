use crate::auth::{
    AuthError, AuthErrorCode, AuthSession, AuthStore, DesktopBootstrap, DevicePoll, DeviceStart,
    UserPrincipal, session_cookie_name,
};
use crate::{AuthRequirement, HttpMethod, ROUTES, RouteDescriptor};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeConfig {
    pub addr: String,
    pub studio_url: String,
    pub database_url: Option<String>,
    pub deployment_id: String,
}

impl RuntimeConfig {
    pub fn database_url_configured(&self) -> bool {
        self.database_url
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    }

    pub fn verification_uri(&self) -> String {
        format!("http://{}/v1/desktop/device", self.addr)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub reason: &'static str,
    pub content_type: &'static str,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpResponse {
    pub fn json(status: u16, reason: &'static str, body: impl Into<String>) -> Self {
        Self {
            status,
            reason,
            content_type: "application/json; charset=utf-8",
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn html(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            reason: "OK",
            content_type: "text/html; charset=utf-8",
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

pub fn handle_connection(
    stream: &mut TcpStream,
    config: &RuntimeConfig,
    auth_store: &mut impl AuthStore,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;

    let request = read_request(stream)?;
    let response = match request {
        Some(request) => route_request(request, config, auth_store),
        None => HttpResponse::json(
            400,
            "Bad Request",
            error_json("bad_request", "empty request"),
        ),
    };

    write_response(stream, response)
}

pub fn route_request(
    request: HttpRequest,
    config: &RuntimeConfig,
    auth_store: &mut impl AuthStore,
) -> HttpResponse {
    let method = request.method.as_str();
    let path = request.path.as_str();
    let cors_origin = request
        .headers
        .get("origin")
        .cloned()
        .unwrap_or_else(|| config.studio_url.clone());

    let mut response = match (method, path) {
        ("OPTIONS", _) => HttpResponse {
            status: 204,
            reason: "No Content",
            content_type: "text/plain; charset=utf-8",
            headers: vec![("Access-Control-Max-Age".to_string(), "600".to_string())],
            body: String::new(),
        },
        ("GET", "/") | ("GET", "/server") => HttpResponse::html(server_page(config)),
        ("GET", "/healthz") => HttpResponse::json(200, "OK", health_json(config)),
        ("GET", "/v1/routes") => HttpResponse::json(200, "OK", routes_json()),
        ("POST", "/v1/auth/signup") => signup(request, auth_store),
        ("POST", "/v1/auth/login") => login(request, auth_store),
        ("POST", "/v1/auth/logout") => logout(request, auth_store),
        ("GET", "/v1/auth/session") => auth_session(request, auth_store),
        ("GET", "/v1/me") => me(request, auth_store),
        ("GET", "/v1/lenses") => {
            HttpResponse::json(200, "OK", crate::lenses::registry_response_json_from_env())
        }
        ("GET", "/v1/studio/snapshot") => studio_snapshot(request, auth_store),
        ("GET", "/v1/workspaces") => list_workspaces(request, auth_store),
        ("POST", "/v1/workspaces") => create_workspace(request, auth_store),
        ("GET", "/v1/devices") => list_devices(request, auth_store),
        ("POST", "/v1/desktop/device/start") => {
            match auth_store.start_device_flow(&config.verification_uri()) {
                Ok(start) => HttpResponse::json(200, "OK", device_start_json(&start)),
                Err(error) => auth_error_response(error),
            }
        }
        ("POST", "/v1/desktop/device/poll") => device_poll(request, auth_store),
        ("GET", "/v1/desktop/bootstrap") => desktop_bootstrap(request, config, auth_store),
        _ if method == "GET" && is_workspace_audit_events_path(path) => {
            audit_events(request, auth_store)
        }
        _ if route_exists(method, path) => HttpResponse::json(
            501,
            "Not Implemented",
            error_json(
                "registered_route_not_implemented",
                "route is registered but no handler is wired yet",
            ),
        ),
        _ => HttpResponse::json(404, "Not Found", error_json("not_found", "route not found")),
    };

    apply_cors(&mut response, &cors_origin);
    response
}

fn signup(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    let email = required_json_field(&request.body, "email");
    let password = required_json_field(&request.body, "password");
    let display_name = json_string_field(&request.body, "display_name")
        .or_else(|| json_string_field(&request.body, "name"));

    match (email, password) {
        (Ok(email), Ok(password)) => {
            match auth_store.signup(&email, &password, display_name.as_deref()) {
                Ok(session) => session_response(201, "Created", &session),
                Err(error) => auth_error_response(error),
            }
        }
        (Err(error), _) | (_, Err(error)) => auth_error_response(error),
    }
}

fn login(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    let email = required_json_field(&request.body, "email");
    let password = required_json_field(&request.body, "password");

    match (email, password) {
        (Ok(email), Ok(password)) => match auth_store.login(&email, &password) {
            Ok(session) => session_response(200, "OK", &session),
            Err(error) => auth_error_response(error),
        },
        (Err(error), _) | (_, Err(error)) => auth_error_response(error),
    }
}

fn logout(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    let Some(token) = session_cookie(&request) else {
        return unauthorized();
    };

    match auth_store.logout(&token) {
        Ok(()) => HttpResponse::json(200, "OK", r#"{"ok":true}"#)
            .with_header("Set-Cookie", expired_session_cookie()),
        Err(error) => auth_error_response(error),
    }
}

fn me(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    match authenticated_session_user(&request, auth_store) {
        Ok(Some(user)) => HttpResponse::json(200, "OK", me_json(&user)),
        Ok(None) => unauthorized(),
        Err(error) => auth_error_response(error),
    }
}

fn auth_session(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    match authenticated_session_user(&request, auth_store) {
        Ok(Some(user)) => HttpResponse::json(200, "OK", auth_session_json(&user)),
        Ok(None) => unauthorized(),
        Err(error) => auth_error_response(error),
    }
}

fn studio_snapshot(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    match authenticated_session_user(&request, auth_store) {
        Ok(Some(user)) => HttpResponse::json(200, "OK", studio_snapshot_json(&user)),
        Ok(None) => unauthorized(),
        Err(error) => auth_error_response(error),
    }
}

fn list_workspaces(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    match authenticated_user_from_session_or_bearer(&request, auth_store) {
        Ok(Some(_)) => {
            HttpResponse::json(200, "OK", format!(r#"{{"items":[{}]}}"#, workspace_json()))
        }
        Ok(None) => unauthorized(),
        Err(error) => auth_error_response(error),
    }
}

fn create_workspace(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    match authenticated_session_user(&request, auth_store) {
        Ok(Some(_)) => {
            let name = json_string_field(&request.body, "name")
                .unwrap_or_else(|| "Layrs Workspace".to_string());
            let slug = json_string_field(&request.body, "slug").unwrap_or_else(|| slugify(&name));
            let description = json_string_field(&request.body, "description")
                .unwrap_or_else(|| "Server-backed Layrs workspace.".to_string());
            HttpResponse::json(
                201,
                "Created",
                workspace_json_with(&name, &slug, &description),
            )
        }
        Ok(None) => unauthorized(),
        Err(error) => auth_error_response(error),
    }
}

fn list_devices(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    match authenticated_session_user(&request, auth_store) {
        Ok(Some(user)) => HttpResponse::json(
            200,
            "OK",
            format!(r#"{{"items":[{}]}}"#, device_json(&user)),
        ),
        Ok(None) => unauthorized(),
        Err(error) => auth_error_response(error),
    }
}

fn audit_events(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    match authenticated_session_user(&request, auth_store) {
        Ok(Some(user)) => HttpResponse::json(
            200,
            "OK",
            format!(r#"{{"items":[{}]}}"#, audit_event_json(&user)),
        ),
        Ok(None) => unauthorized(),
        Err(error) => auth_error_response(error),
    }
}

fn device_poll(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    let device_code = json_string_field(&request.body, "device_code")
        .or_else(|| json_string_field(&request.body, "deviceCode"))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AuthError::invalid("device_code is required"));

    match device_code {
        Ok(device_code) => match auth_store.poll_device_flow(&device_code) {
            Ok(DevicePoll::AuthorizationPending { interval_seconds }) => HttpResponse::json(
                202,
                "Accepted",
                format!(
                    r#"{{"status":"authorization_pending","interval_seconds":{interval_seconds}}}"#
                ),
            ),
            Ok(DevicePoll::Authorized {
                user,
                desktop_token,
            }) => HttpResponse::json(
                200,
                "OK",
                format!(
                    r#"{{"status":"authorized","user":{},"desktop_token":{{"access_token":"{}","token_type":"{}","expires_in_seconds":{}}}}}"#,
                    user_json(&user),
                    escape_json(&desktop_token.access_token),
                    escape_json(&desktop_token.token_type),
                    desktop_token.expires_in_seconds
                ),
            ),
            Ok(DevicePoll::Expired) => HttpResponse::json(
                400,
                "Bad Request",
                error_json(
                    "expired_device_code",
                    "device code is expired or already consumed",
                ),
            ),
            Err(error) => auth_error_response(error),
        },
        Err(error) => auth_error_response(error),
    }
}

fn desktop_bootstrap(
    request: HttpRequest,
    config: &RuntimeConfig,
    auth_store: &mut impl AuthStore,
) -> HttpResponse {
    let Some(token) = bearer_token(&request) else {
        return HttpResponse::json(
            401,
            "Unauthorized",
            error_json("unauthorized", "missing desktop bearer token"),
        );
    };

    match auth_store.desktop_bootstrap(&token, config.database_url_configured()) {
        Ok(Some(bootstrap)) => HttpResponse::json(200, "OK", desktop_bootstrap_json(&bootstrap)),
        Ok(None) => HttpResponse::json(
            401,
            "Unauthorized",
            error_json("unauthorized", "desktop bearer token is invalid"),
        ),
        Err(error) => auth_error_response(error),
    }
}

fn session_response(status: u16, reason: &'static str, session: &AuthSession) -> HttpResponse {
    HttpResponse::json(status, reason, session_json(session))
        .with_header("Set-Cookie", session_cookie_header(&session.session_token))
}

fn unauthorized() -> HttpResponse {
    HttpResponse::json(
        401,
        "Unauthorized",
        error_json("unauthorized", "authentication is required"),
    )
}

fn auth_error_response(error: AuthError) -> HttpResponse {
    match error.code {
        AuthErrorCode::InvalidRequest => HttpResponse::json(
            400,
            "Bad Request",
            error_json("invalid_request", &error.message),
        ),
        AuthErrorCode::Unauthorized => HttpResponse::json(
            401,
            "Unauthorized",
            error_json("unauthorized", &error.message),
        ),
        AuthErrorCode::Conflict => {
            HttpResponse::json(409, "Conflict", error_json("conflict", &error.message))
        }
        AuthErrorCode::StoreUnavailable => HttpResponse::json(
            503,
            "Service Unavailable",
            error_json("store_unavailable", &error.message),
        ),
    }
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<HttpRequest>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut headers_end = None;

    while headers_end.is_none() && buffer.len() < 64 * 1024 {
        let bytes_read = stream.read(&mut chunk)?;
        if bytes_read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..bytes_read]);
        headers_end = find_header_end(&buffer);
    }

    let Some(headers_end) = headers_end else {
        return Ok(None);
    };
    let head = String::from_utf8_lossy(&buffer[..headers_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut request_parts = request_line.split_whitespace();
    let Some(method) = request_parts.next() else {
        return Ok(None);
    };
    let Some(target) = request_parts.next() else {
        return Ok(None);
    };
    let path = target.split('?').next().unwrap_or(target).to_string();

    let mut headers = BTreeMap::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }

    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = headers_end + 4;

    while buffer.len().saturating_sub(body_start) < content_length {
        let bytes_read = stream.read(&mut chunk)?;
        if bytes_read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..bytes_read]);
    }

    let available_body = buffer.len().saturating_sub(body_start).min(content_length);
    let body =
        String::from_utf8_lossy(&buffer[body_start..body_start + available_body]).to_string();

    Ok(Some(HttpRequest {
        method: method.to_string(),
        path,
        headers,
        body,
    }))
}

fn write_response(stream: &mut TcpStream, response: HttpResponse) -> std::io::Result<()> {
    let mut headers = response.headers;
    headers.push((
        "Content-Type".to_string(),
        response.content_type.to_string(),
    ));
    headers.push((
        "Content-Length".to_string(),
        response.body.len().to_string(),
    ));
    headers.push(("Connection".to_string(), "close".to_string()));

    write!(
        stream,
        "HTTP/1.1 {} {}\r\n",
        response.status, response.reason
    )?;
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    write!(stream, "\r\n{}", response.body)
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn session_cookie(request: &HttpRequest) -> Option<String> {
    let cookies = request.headers.get("cookie")?;
    cookies.split(';').find_map(|cookie| {
        let (name, value) = cookie.trim().split_once('=')?;
        (name == session_cookie_name()).then(|| value.to_string())
    })
}

fn bearer_token(request: &HttpRequest) -> Option<String> {
    let value = request.headers.get("authorization")?;
    value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

fn authenticated_session_user(
    request: &HttpRequest,
    auth_store: &mut impl AuthStore,
) -> Result<Option<UserPrincipal>, AuthError> {
    let Some(token) = session_cookie(request) else {
        return Ok(None);
    };

    auth_store.session_user(&token)
}

fn authenticated_user_from_session_or_bearer(
    request: &HttpRequest,
    auth_store: &mut impl AuthStore,
) -> Result<Option<UserPrincipal>, AuthError> {
    if let Some(user) = authenticated_session_user(request, auth_store)? {
        return Ok(Some(user));
    }

    let Some(token) = bearer_token(request) else {
        return Ok(None);
    };

    auth_store
        .desktop_bootstrap(&token, false)
        .map(|bootstrap| bootstrap.map(|value| value.user))
}

fn apply_cors(response: &mut HttpResponse, origin: &str) {
    response.headers.push((
        "Access-Control-Allow-Origin".to_string(),
        origin.to_string(),
    ));
    response.headers.push((
        "Access-Control-Allow-Headers".to_string(),
        "content-type, authorization".to_string(),
    ));
    response.headers.push((
        "Access-Control-Allow-Methods".to_string(),
        "GET, POST, PUT, DELETE, OPTIONS".to_string(),
    ));
    response.headers.push((
        "Access-Control-Allow-Credentials".to_string(),
        "true".to_string(),
    ));
}

fn session_cookie_header(token: &str) -> String {
    format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800",
        session_cookie_name(),
        token
    )
}

fn expired_session_cookie() -> String {
    format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
        session_cookie_name()
    )
}

fn required_json_field(body: &str, field: &str) -> Result<String, AuthError> {
    json_string_field(body, field)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AuthError::invalid(format!("{field} is required")))
}

fn json_string_field(body: &str, field: &str) -> Option<String> {
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

fn is_workspace_audit_events_path(path: &str) -> bool {
    let parts: Vec<_> = path.split('/').collect();
    parts.len() == 5 && parts[1] == "v1" && parts[2] == "workspaces" && parts[4] == "audit-events"
}

fn route_exists(method: &str, path: &str) -> bool {
    let method = match method {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        "PATCH" => HttpMethod::Patch,
        "DELETE" => HttpMethod::Delete,
        _ => return false,
    };

    ROUTES
        .iter()
        .any(|route| route.method == method && route.path == path)
}

pub fn routes_json() -> String {
    let mut json = String::from("[");

    for (index, route) in ROUTES.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str(&route_json(route));
    }

    json.push(']');
    json
}

fn route_json(route: &RouteDescriptor) -> String {
    format!(
        r#"{{"method":"{}","path":"{}","name":"{}","auth":"{}"}}"#,
        method_label(route.method),
        escape_json(route.path),
        escape_json(route.name),
        auth_label(route.auth)
    )
}

fn health_json(config: &RuntimeConfig) -> String {
    format!(
        r#"{{"service":"layrs-server","status":"ok","runtime":"std-http","auth_store":"dev-memory","database_url_configured":{},"deployment_id":"{}"}}"#,
        config.database_url_configured(),
        escape_json(&config.deployment_id)
    )
}

fn session_json(session: &AuthSession) -> String {
    auth_session_json(&session.user)
}

fn me_json(user: &UserPrincipal) -> String {
    format!(r#"{{"user":{}}}"#, user_json(user))
}

fn auth_session_json(user: &UserPrincipal) -> String {
    format!(
        r#"{{"state":"authenticated","account":{},"session":{},"workspaces":[{}],"activeWorkspaceId":"workspace-dev"}}"#,
        studio_account_json(user),
        studio_session_json(user),
        workspace_json()
    )
}

fn studio_snapshot_json(user: &UserPrincipal) -> String {
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

fn user_json(user: &UserPrincipal) -> String {
    format!(
        r#"{{"id":"{}","email":"{}","display_name":"{}"}}"#,
        escape_json(&user.id),
        escape_json(&user.email),
        escape_json(&user.display_name)
    )
}

fn desktop_account_json(user: &UserPrincipal) -> String {
    format!(
        r#"{{"id":"{}","email":"{}","displayName":"{}"}}"#,
        escape_json(&user.id),
        escape_json(&user.email),
        escape_json(&user.display_name)
    )
}

fn studio_account_json(user: &UserPrincipal) -> String {
    format!(
        r#"{{"id":"{}","email":"{}","name":"{}","role":"owner","avatarInitials":"{}","createdAt":"2026-06-29T00:00:00Z"}}"#,
        escape_json(&user.id),
        escape_json(&user.email),
        escape_json(&user.display_name),
        escape_json(&avatar_initials(&user.display_name))
    )
}

fn studio_session_json(user: &UserPrincipal) -> String {
    format!(
        r#"{{"id":"session-dev","accountId":"{}","activeWorkspaceId":"workspace-dev","expiresAt":"2026-07-06T00:00:00Z","createdAt":"2026-06-29T00:00:00Z"}}"#,
        escape_json(&user.id)
    )
}

fn workspace_json() -> String {
    workspace_json_with_id(
        "workspace-dev",
        "Layrs Studio",
        "layrs-studio",
        "Development workspace for validating Studio server workflows.",
    )
}

fn workspace_json_with(name: &str, slug: &str, description: &str) -> String {
    let id = format!("workspace-{}", slugify(slug));
    workspace_json_with_id(&id, name, slug, description)
}

fn workspace_json_with_id(id: &str, name: &str, slug: &str, description: &str) -> String {
    format!(
        r#"{{"id":"{}","name":"{}","slug":"{}","description":"{}","health":"pending","updatedAt":"2026-06-29T20:30:00Z"}}"#,
        escape_json(id),
        escape_json(name),
        escape_json(slug),
        escape_json(description)
    )
}

fn team_json() -> String {
    r#"{"id":"team-art","workspaceId":"workspace-dev","name":"Art Team","purpose":"Owns restricted visual assets and texture reviews.","members":1,"gateResponsibility":"asset access"}"#.to_string()
}

fn space_json() -> String {
    r#"{"id":"space-game","workspaceId":"workspace-dev","teamId":"team-art","name":"Game Prototype","description":"Generalist Layrs Space with code and image artifacts.","status":"pending","currentLayerId":"layer-main","updatedAt":"2026-06-29T20:30:00Z"}"#.to_string()
}

fn main_layer_json() -> String {
    r#"{"id":"layer-main","spaceId":"space-game","name":"Main","kind":"base","status":"active","summary":"Primary working layer with inherited access rules.","artifactIds":["artifact-readme","artifact-hero-texture"],"stepIds":[],"gateIds":[]}"#.to_string()
}

fn restricted_layer_json() -> String {
    r#"{"id":"layer-art-private","spaceId":"space-game","parentId":"layer-main","name":"Art Private","kind":"proposal","status":"review","summary":"Child layer carrying restricted image work.","artifactIds":["artifact-hero-texture"],"stepIds":[],"gateIds":[]}"#.to_string()
}

fn public_artifact_json() -> String {
    r#"{"id":"artifact-readme","spaceId":"space-game","layerId":"layer-main","name":"README.md","type":"file","summary":"Public project notes.","location":"README.md","updatedAt":"2026-06-29T20:30:00Z","sizeLabel":"2 KB","proofIds":[],"access":{"mode":"read","canOpen":true,"isRedacted":false}}"#.to_string()
}

fn redacted_artifact_json() -> String {
    r#"{"id":"artifact-hero-texture","spaceId":"space-game","layerId":"layer-main","name":"hero.texture.png","type":"image","summary":"Restricted by Layer access policy","location":"Assets/Private/hero.texture.png","updatedAt":"2026-06-29T20:30:00Z","sizeLabel":"redacted","proofIds":[],"access":{"mode":"none","canOpen":false,"isRedacted":true,"reason":"Restricted by Layer access policy"}}"#.to_string()
}

fn access_registry_json() -> String {
    r#"{"id":"registry-layer-main","workspaceId":"workspace-dev","layerId":"layer-main","rules":[{"id":"access-rule-art-team","subjectKind":"team","subjectId":"team-art","subjectName":"Art Team","mode":"read"}],"updatedAt":"2026-06-29T20:30:00Z"}"#.to_string()
}

fn device_json(user: &UserPrincipal) -> String {
    format!(
        r#"{{"id":"device-browser-dev","accountId":"{}","name":"Studio Web dev session","kind":"browser","status":"trusted","lastSeenAt":"2026-06-29T20:30:00Z"}}"#,
        escape_json(&user.id)
    )
}

fn audit_event_json(user: &UserPrincipal) -> String {
    format!(
        r#"{{"id":"audit-dev-login","workspaceId":"workspace-dev","actorAccountId":"{}","action":"auth.login","target":"studio","summary":"Signed in to Layrs Studio.","at":"2026-06-29T20:30:00Z"}}"#,
        escape_json(&user.id)
    )
}

fn device_start_json(start: &DeviceStart) -> String {
    format!(
        r#"{{"device_code":"{}","user_code":"{}","verification_uri":"{}","interval_seconds":{},"expires_in_seconds":{}}}"#,
        escape_json(&start.device_code),
        escape_json(&start.user_code),
        escape_json(&start.verification_uri),
        start.interval_seconds,
        start.expires_in_seconds
    )
}

fn desktop_bootstrap_json(bootstrap: &DesktopBootstrap) -> String {
    format!(
        r#"{{"user":{},"account":{},"workspaces":[{{"id":"workspace-dev","name":"Layrs Studio","slug":"layrs-studio"}}],"spaces":[{{"id":"space-game","workspaceId":"workspace-dev","name":"Game Prototype","currentLayerId":"layer-main"}}],"layers":[{{"id":"layer-main","workspaceId":"workspace-dev","spaceId":"space-game","name":"Main","kind":"base","access":"open"}},{{"id":"layer-art-private","workspaceId":"workspace-dev","spaceId":"space-game","name":"Art Private","kind":"proposal","parentLayerId":"layer-main","access":"redacted"}}],"server":{{"mode":"{}","database_url_configured":{},"routes_path":"{}"}}}}"#,
        user_json(&bootstrap.user),
        desktop_account_json(&bootstrap.user),
        escape_json(&bootstrap.server_mode),
        bootstrap.database_url_configured,
        escape_json(&bootstrap.routes_path)
    )
}

fn error_json(code: &str, message: &str) -> String {
    format!(
        r#"{{"error":{{"code":"{}","message":"{}"}}}}"#,
        escape_json(code),
        escape_json(message)
    )
}

fn method_label(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
    }
}

fn auth_label(auth: AuthRequirement) -> &'static str {
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

fn avatar_initials(value: &str) -> String {
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

fn slugify(value: &str) -> String {
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

fn escape_json(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn server_page(config: &RuntimeConfig) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Layrs Server</title>
    <style>
      :root {{
        color-scheme: light dark;
        font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
        line-height: 1.5;
      }}
      body {{
        margin: 0;
        min-height: 100vh;
        display: grid;
        place-items: center;
        background: Canvas;
        color: CanvasText;
      }}
      main {{
        width: min(760px, calc(100vw - 40px));
      }}
      h1 {{
        font-size: 28px;
        margin: 0 0 8px;
      }}
      p {{
        margin: 0 0 20px;
        color: color-mix(in srgb, CanvasText 72%, Canvas 28%);
      }}
      dl {{
        display: grid;
        grid-template-columns: max-content 1fr;
        gap: 10px 16px;
        padding: 18px 0;
        border-top: 1px solid color-mix(in srgb, CanvasText 18%, Canvas 82%);
        border-bottom: 1px solid color-mix(in srgb, CanvasText 18%, Canvas 82%);
      }}
      dt {{
        font-weight: 700;
      }}
      a {{
        color: LinkText;
      }}
      code {{
        font: inherit;
        font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
      }}
    </style>
  </head>
  <body>
    <main>
      <h1>Layrs Server</h1>
      <p>Local development server for API contracts, health checks, auth and Studio handoff.</p>
      <dl>
        <dt>Status</dt>
        <dd><a href="/healthz">/healthz</a></dd>
        <dt>Routes</dt>
        <dd><a href="/v1/routes">/v1/routes</a></dd>
        <dt>Studio Web</dt>
        <dd><a href="{studio_url}">{studio_url}</a></dd>
        <dt>Runtime</dt>
        <dd><code>std-http</code>, auth store <code>dev-memory</code>, database configured: <code>{database_url_configured}</code></dd>
      </dl>
      <p>This page is intentionally a server status surface. The full product interface runs in Studio Web.</p>
    </main>
  </body>
</html>"#,
        studio_url = escape_html(&config.studio_url),
        database_url_configured = config.database_url_configured()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::DevAuthStore;

    fn config() -> RuntimeConfig {
        RuntimeConfig {
            addr: "127.0.0.1:8787".to_string(),
            studio_url: "http://127.0.0.1:5173".to_string(),
            database_url: None,
            deployment_id: "test".to_string(),
        }
    }

    fn request(method: &str, path: &str, body: &str) -> HttpRequest {
        HttpRequest {
            method: method.to_string(),
            path: path.to_string(),
            headers: BTreeMap::new(),
            body: body.to_string(),
        }
    }

    #[test]
    fn route_registry_includes_runtime_auth_routes() {
        let json = routes_json();

        assert!(json.contains(r#""path":"/healthz""#));
        assert!(json.contains(r#""path":"/v1/auth/signup""#));
        assert!(json.contains(r#""path":"/v1/desktop/bootstrap""#));
    }

    #[test]
    fn lenses_route_returns_registry_payload() {
        let mut store = DevAuthStore::new();
        let response = route_request(request("GET", "/v1/lenses", ""), &config(), &mut store);

        assert_eq!(response.status, 200);
        assert!(response.body.contains(r#""items""#));
        assert!(response.body.contains(r#""layrs.code""#));
        assert!(response.body.contains(r#""errors""#));
    }

    #[test]
    fn signup_sets_http_only_cookie_and_me_reads_it() {
        let mut store = DevAuthStore::new();
        let signup = route_request(
            request(
                "POST",
                "/v1/auth/signup",
                r#"{"email":"alice@example.com","password":"correct horse","display_name":"Alice"}"#,
            ),
            &config(),
            &mut store,
        );

        assert_eq!(signup.status, 201);
        let cookie = signup
            .headers
            .iter()
            .find(|(name, _)| name == "Set-Cookie")
            .map(|(_, value)| value.clone())
            .unwrap();
        assert!(cookie.contains("HttpOnly"));

        let mut me = request("GET", "/v1/me", "");
        me.headers.insert(
            "cookie".to_string(),
            cookie.split(';').next().unwrap().to_string(),
        );
        let response = route_request(me, &config(), &mut store);

        assert_eq!(response.status, 200);
        assert!(response.body.contains(r#""email":"alice@example.com""#));
    }

    #[test]
    fn auth_session_supports_studio_cookie_cors() {
        let mut store = DevAuthStore::new();
        let signup = route_request(
            request(
                "POST",
                "/v1/auth/signup",
                r#"{"email":"alice@example.com","password":"correct horse","name":"Alice"}"#,
            ),
            &config(),
            &mut store,
        );
        let cookie = signup
            .headers
            .iter()
            .find(|(name, _)| name == "Set-Cookie")
            .map(|(_, value)| value.clone())
            .unwrap();

        let mut session = request("GET", "/v1/auth/session", "");
        session
            .headers
            .insert("origin".to_string(), "http://127.0.0.1:5173".to_string());
        session.headers.insert(
            "cookie".to_string(),
            cookie.split(';').next().unwrap().to_string(),
        );
        let response = route_request(session, &config(), &mut store);

        assert_eq!(response.status, 200);
        assert!(response.body.contains(r#""state":"authenticated""#));
        assert!(response.headers.iter().any(|(name, value)| {
            name == "Access-Control-Allow-Credentials" && value == "true"
        }));
        assert!(response.headers.iter().any(|(name, value)| {
            name == "Access-Control-Allow-Origin" && value == "http://127.0.0.1:5173"
        }));
    }

    #[test]
    fn device_flow_bootstraps_with_bearer_token() {
        let mut store = DevAuthStore::new();
        let start = route_request(
            request("POST", "/v1/desktop/device/start", ""),
            &config(),
            &mut store,
        );
        let device_code = json_string_field(&start.body, "device_code").unwrap();

        let pending = route_request(
            request(
                "POST",
                "/v1/desktop/device/poll",
                &format!(r#"{{"device_code":"{}"}}"#, device_code),
            ),
            &config(),
            &mut store,
        );
        assert_eq!(pending.status, 202);

        let authorized = route_request(
            request(
                "POST",
                "/v1/desktop/device/poll",
                &format!(r#"{{"device_code":"{}"}}"#, device_code),
            ),
            &config(),
            &mut store,
        );
        let token = json_string_field(&authorized.body, "access_token").unwrap();
        let mut bootstrap = request("GET", "/v1/desktop/bootstrap", "");
        bootstrap
            .headers
            .insert("authorization".to_string(), format!("Bearer {token}"));

        let response = route_request(bootstrap, &config(), &mut store);

        assert_eq!(response.status, 200);
        assert!(response.body.contains(r#""mode":"dev-memory""#));
    }

    #[test]
    fn device_poll_accepts_desktop_camel_case_payload() {
        let mut store = DevAuthStore::new();
        let start = route_request(
            request("POST", "/v1/desktop/device/start", ""),
            &config(),
            &mut store,
        );
        let device_code = json_string_field(&start.body, "device_code").unwrap();

        let pending = route_request(
            request(
                "POST",
                "/v1/desktop/device/poll",
                &format!(r#"{{"deviceCode":"{}"}}"#, device_code),
            ),
            &config(),
            &mut store,
        );

        assert_eq!(pending.status, 202);
        assert!(pending.body.contains("authorization_pending"));
    }
}
