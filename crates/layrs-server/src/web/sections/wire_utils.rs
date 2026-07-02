fn studio_account_json(user: &UserPrincipal) -> Value {
    json!({
        "id": user.id,
        "email": user.email,
        "name": user.display_name,
        "role": "owner",
        "avatarInitials": avatar_initials(&user.display_name),
        "createdAt": "2026-06-29T00:00:00Z"
    })
}

fn desktop_account_json(user: &UserPrincipal) -> Value {
    json!({
        "id": user.id,
        "email": user.email,
        "displayName": user.display_name
    })
}

fn desktop_user_json(user: &UserPrincipal) -> Value {
    json!({
        "id": user.id,
        "email": user.email,
        "displayName": user.display_name
    })
}

fn user_wire_json(user: &UserPrincipal) -> Value {
    json!({
        "id": user.id,
        "email": user.email,
        "display_name": user.display_name
    })
}

fn session_json(user: &UserPrincipal, active_workspace_id: Option<&str>) -> Value {
    json!({
        "id": "session-current",
        "accountId": user.id,
        "activeWorkspaceId": active_workspace_id,
        "expiresAt": "2026-07-06T00:00:00Z",
        "createdAt": "2026-06-29T00:00:00Z"
    })
}

fn route_json(route: &RouteDescriptor) -> Value {
    json!({
        "method": method_label(route.method),
        "path": route.path,
        "name": route.name,
        "auth": auth_label(route.auth)
    })
}

fn artifact_metadata_value(
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    artifact_id: &str,
    path: &str,
    artifact_kind: &str,
    updated_at: &str,
    file_object_id: Option<String>,
    tree_id: Option<String>,
    redacted: bool,
    reason: &str,
) -> Value {
    json!({
        "id": artifact_id,
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "layerId": layer_id,
        "name": path.rsplit('/').next().unwrap_or(path),
        "type": artifact_type(artifact_kind),
        "summary": if redacted { "Restricted by Layer access policy" } else { "Persisted artifact" },
        "location": path,
        "updatedAt": updated_at,
        "fileObjectId": file_object_id,
        "treeId": tree_id,
        "sizeLabel": if redacted { "redacted" } else { "stored" },
        "proofIds": [],
        "access": {
            "mode": if redacted { "none" } else { "read" },
            "canOpen": !redacted,
            "isRedacted": redacted,
            "reason": if redacted { reason } else { "" }
        }
    })
}

fn latest_timeline_cursor(events: &[Value]) -> Option<String> {
    events
        .last()
        .and_then(|event| event.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn redact_timeline_body(mut body: Value) -> Value {
    if let Some(object) = body.as_object_mut() {
        if object.remove("content").is_some() {
            object.insert("contentIncluded".to_string(), Value::Bool(false));
            object.insert(
                "contentEndpoint".to_string(),
                Value::String("layer_artifact_content.get".to_string()),
            );
        }
    }
    body
}

#[derive(Debug)]
struct PreparedChunk {
    chunk_id: String,
    digest: Option<String>,
    size_bytes: Option<i64>,
    media_type: Option<String>,
}

fn prepared_chunk_from_item(item: PrepareChunkItem) -> Result<PreparedChunk, ApiError> {
    match item {
        PrepareChunkItem::Id(value) => {
            let chunk_id = validate_chunk_id(&value)?;
            Ok(PreparedChunk {
                digest: Some(chunk_id.clone()),
                chunk_id,
                size_bytes: None,
                media_type: None,
            })
        }
        PrepareChunkItem::Object {
            id,
            chunk_id,
            chunk_id_camel,
            sha256,
            size_bytes,
            size_bytes_camel,
            media_type,
            media_type_camel,
        } => {
            let raw_chunk_id = id
                .or(chunk_id)
                .or(chunk_id_camel)
                .or_else(|| sha256.as_ref().map(|value| value.to_string()))
                .ok_or_else(|| ApiError::bad_request("chunk id is required"))?;
            let chunk_id = validate_chunk_id(&raw_chunk_id)?;
            let digest = Some(validate_object_digest(
                sha256.as_deref().unwrap_or(&chunk_id),
            )?);
            Ok(PreparedChunk {
                chunk_id,
                digest,
                size_bytes: size_bytes.or(size_bytes_camel),
                media_type: media_type.or(media_type_camel),
            })
        }
    }
}

async fn object_chunk_exists(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    chunk_id: &str,
) -> Result<bool, ApiError> {
    sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM object_chunks oc
            JOIN space_object_chunks soc ON soc.chunk_id = oc.chunk_id
            WHERE soc.workspace_id = $1
              AND soc.space_id = $2
              AND oc.chunk_id = $3
              AND oc.state = 'available'
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(chunk_id)
    .fetch_one(pool)
    .await
    .map_err(ApiError::from)
}

async fn mark_chunk_available_for_space(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    chunk_id: &str,
    account_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO space_object_chunks
            (workspace_id, space_id, chunk_id, created_by_account_id)
        VALUES
            ($1, $2, $3, $4)
        ON CONFLICT (workspace_id, space_id, chunk_id) DO NOTHING
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(chunk_id)
    .bind(account_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_chunk_available_for_space_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    chunk_id: &str,
    account_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO space_object_chunks
            (workspace_id, space_id, chunk_id, created_by_account_id)
        VALUES
            ($1, $2, $3, $4)
        ON CONFLICT (workspace_id, space_id, chunk_id) DO NOTHING
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(chunk_id)
    .bind(account_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn mark_file_available_for_space_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    file_object_id: &str,
    account_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO space_file_objects
            (workspace_id, space_id, file_object_id, created_by_account_id)
        VALUES
            ($1, $2, $3, $4)
        ON CONFLICT (workspace_id, space_id, file_object_id) DO NOTHING
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(file_object_id)
    .bind(account_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn mark_tree_available_for_space_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    tree_id: &str,
    account_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO space_tree_objects
            (workspace_id, space_id, tree_id, created_by_account_id)
        VALUES
            ($1, $2, $3, $4)
        ON CONFLICT (workspace_id, space_id, tree_id) DO NOTHING
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(tree_id)
    .bind(account_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn validate_chunk_id(value: &str) -> Result<String, ApiError> {
    validate_object_digest(value)
}

fn validate_object_digest(value: &str) -> Result<String, ApiError> {
    let value = value.trim().to_ascii_lowercase();
    let Some(hex) = value.strip_prefix("blake3:") else {
        return Err(ApiError::bad_request(
            "digest must use blake3:<64 hex> format",
        ));
    };
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request(
            "digest must use blake3:<64 hex> format",
        ));
    }
    Ok(value)
}

fn blake3_digest_for_bytes(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

fn chunk_compression_from_headers(headers: &HeaderMap) -> Result<String, ApiError> {
    let compression = headers
        .get("x-layrs-chunk-compression")
        .and_then(|value| value.to_str().ok())
        .unwrap_or(CHUNK_COMPRESSION_IDENTITY)
        .trim()
        .to_ascii_lowercase();
    match compression.as_str() {
        CHUNK_COMPRESSION_IDENTITY | CHUNK_COMPRESSION_ZSTD => Ok(compression),
        _ => Err(ApiError::bad_request("unsupported chunk compression")),
    }
}

fn header_i64(headers: &HeaderMap, name: &str) -> Result<Option<i64>, ApiError> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| ApiError::bad_request("invalid chunk size header"))?
                .parse::<i64>()
                .map_err(|_| ApiError::bad_request("invalid chunk size header"))
        })
        .transpose()
}

fn decode_chunk_bytes(bytes: &[u8], compression: &str) -> Result<Vec<u8>, ApiError> {
    match compression {
        CHUNK_COMPRESSION_IDENTITY => Ok(bytes.to_vec()),
        CHUNK_COMPRESSION_ZSTD => zstd::stream::decode_all(std::io::Cursor::new(bytes))
            .map_err(|_| ApiError::bad_request("chunk zstd payload could not be decoded")),
        _ => Err(ApiError::bad_request("unsupported chunk compression")),
    }
}

fn require_layer_id(
    layer_id: Option<String>,
    layer_id_camel: Option<String>,
) -> Result<String, ApiError> {
    layer_id
        .or(layer_id_camel)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("layer_id is required"))
}

fn normalize_deleted_paths(paths: &[String]) -> Result<Vec<String>, ApiError> {
    let mut unique = Vec::new();
    for path in paths {
        let path = validate_publish_path(path.trim())?;
        if !unique.iter().any(|existing| existing == &path) {
            unique.push(path);
        }
    }
    Ok(unique)
}

fn artifact_requests_deletion(body: &PublishArtifactBody) -> bool {
    body.deleted.unwrap_or(false)
        || body
            .state
            .as_deref()
            .map(|value| matches!(value.trim(), "deleted" | "delete" | "tombstone"))
            .unwrap_or(false)
        || body
            .operation
            .as_deref()
            .or(body.action.as_deref())
            .map(|value| matches!(value.trim(), "delete" | "deleted" | "remove" | "tombstone"))
            .unwrap_or(false)
}

fn required_artifact_path(body: &PublishArtifactBody) -> Result<String, ApiError> {
    let path = body
        .path
        .as_deref()
        .or(body.logical_path.as_deref())
        .or(body.logical_path_camel.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("artifact path is required"))?;
    validate_publish_path(path)
}

fn validate_publish_path(path: &str) -> Result<String, ApiError> {
    validate_relative_path(path)?;
    if path == ".layrs" || path.starts_with(".layrs/") {
        return Err(ApiError::forbidden(
            "reserved .layrs paths cannot be published",
        ));
    }
    Ok(path.to_string())
}

fn normalize_artifact_kind(value: Option<&str>) -> Result<&'static str, ApiError> {
    match value.unwrap_or("file").trim() {
        "" | "file" | "code" | "layrs.code" => Ok("file"),
        "text" | "note" | "layrs.text" => Ok("note"),
        "image" | "layrs.image" => Ok("image"),
        "texture" => Ok("texture"),
        "raw" | "binary" | "layrs.raw" => Ok("binary"),
        "proof" => Ok("proof"),
        _ => Err(ApiError::bad_request("unsupported artifact kind")),
    }
}

fn hashset(values: Vec<String>) -> HashSet<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn intersects(left: &HashSet<String>, right: &HashSet<String>) -> bool {
    left.iter().any(|value| right.contains(value))
}

fn path_matches_rule(path: &str, rule_path: &str) -> bool {
    let rule_path = rule_path.trim();
    if rule_path == "*" || rule_path == "**" {
        return true;
    }
    if let Some(prefix) = rule_path.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }
    path == rule_path || path.starts_with(&format!("{rule_path}/"))
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

fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| ApiError::bad_request(format!("password could not be hashed: {error}")))
}

fn verify_password(password: &str, hash: &str) -> Result<(), ApiError> {
    let parsed_hash = PasswordHash::new(hash)
        .map_err(|_| ApiError::unauthorized("email or password is incorrect"))?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .map_err(|_| ApiError::unauthorized("email or password is incorrect"))
}

fn digest_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get(COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|cookie| {
        let (name, value) = cookie.trim().split_once('=')?;
        (name == SESSION_COOKIE_NAME).then(|| value.to_string())
    })
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .or_else(|| {
            headers
                .get(AUTHORIZATION)?
                .to_str()
                .ok()?
                .strip_prefix("bearer ")
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn build_session_cookie(token: &str, config: &WebServerConfig) -> String {
    let secure = if config.cookie_secure { "; Secure" } else { "" };
    format!(
        "{SESSION_COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_MAX_AGE_SECONDS}{secure}"
    )
}

fn expired_session_cookie(config: &WebServerConfig) -> String {
    let secure = if config.cookie_secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}")
}

fn normalize_email(email: &str) -> Result<String, ApiError> {
    let email = email.trim().to_ascii_lowercase();
    if !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
        return Err(ApiError::bad_request("email must be a valid address"));
    }
    Ok(email)
}

fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.len() < 8 {
        return Err(ApiError::bad_request(
            "password must be at least 8 characters",
        ));
    }
    Ok(())
}

fn required_body_text(field: &'static str, value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ApiError::bad_request(format!("{field} is required")));
    }
    Ok(value.to_string())
}

fn local_layer_import_id(layer: &LocalLayerImportBody, fallback: &str) -> String {
    layer
        .local_layer_id
        .as_deref()
        .or(layer.local_layer_id_camel.as_deref())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .trim()
        .to_string()
}

fn normalize_user_code(value: &str) -> Result<String, ApiError> {
    let value = value.trim().to_ascii_uppercase();
    if value.is_empty() {
        return Err(ApiError::bad_request("user_code is required"));
    }
    Ok(value)
}

fn workspace_role(value: Option<&str>) -> Result<&'static str, ApiError> {
    match value.unwrap_or("member").trim() {
        "owner" => Err(ApiError::bad_request(
            "owner cannot be assigned through invitations",
        )),
        "admin" => Ok("admin"),
        "member" | "" => Ok("member"),
        "viewer" => Ok("viewer"),
        _ => Err(ApiError::bad_request("unsupported workspace role")),
    }
}

fn team_member_role(value: Option<&str>) -> Result<&'static str, ApiError> {
    match value.unwrap_or("member").trim() {
        "maintainer" => Ok("maintainer"),
        "member" | "" => Ok("member"),
        _ => Err(ApiError::bad_request("unsupported team member role")),
    }
}

fn invitation_team_ids(mut snake: Vec<String>, camel: Vec<String>) -> Vec<String> {
    snake.extend(camel);
    unique_strings(&snake)
}

fn unique_strings(values: &[String]) -> Vec<String> {
    let mut unique = Vec::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() && !unique.iter().any(|existing| existing == value) {
            unique.push(value.to_string());
        }
    }
    unique
}

async fn validate_layer_access_rule(
    pool: &PgPool,
    workspace_id: &str,
    rule: &LayerAccessRuleBody,
) -> Result<(), ApiError> {
    validate_relative_path(&rule.path)?;
    match rule.mode.as_str() {
        "restricted" => {
            if rule.permissions.read.accounts.is_empty()
                && rule.permissions.read.teams.is_empty()
                && rule.permissions.write.accounts.is_empty()
                && rule.permissions.write.teams.is_empty()
                && rule.permissions.admin.accounts.is_empty()
                && rule.permissions.admin.teams.is_empty()
            {
                return Err(ApiError::bad_request(
                    "restricted rule must define read, write or admin principals",
                ));
            }
        }
        "reserved_redacted" => {}
        _ => return Err(ApiError::bad_request("unsupported access rule mode")),
    }
    if !matches!(rule.visibility.as_str(), "full" | "stub") {
        return Err(ApiError::bad_request("unsupported access rule visibility"));
    }
    let mut account_ids = Vec::new();
    account_ids.extend(rule.permissions.read.accounts.iter().cloned());
    account_ids.extend(rule.permissions.write.accounts.iter().cloned());
    account_ids.extend(rule.permissions.admin.accounts.iter().cloned());
    ensure_accounts_in_workspace(pool, workspace_id, &account_ids).await?;
    let mut team_ids = Vec::new();
    team_ids.extend(rule.permissions.read.teams.iter().cloned());
    team_ids.extend(rule.permissions.write.teams.iter().cloned());
    team_ids.extend(rule.permissions.admin.teams.iter().cloned());
    ensure_teams_in_workspace(pool, workspace_id, &team_ids).await?;
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<(), ApiError> {
    if path.trim().is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains("//")
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(ApiError::bad_request(
            "path must be a normalized relative path",
        ));
    }
    Ok(())
}

fn default_stub_visibility() -> String {
    "stub".to_string()
}

fn token(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

fn prefixed_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
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

fn artifact_type(value: &str) -> &'static str {
    match value {
        "image" | "texture" => "image",
        "note" => "note",
        "proof" => "proof",
        _ => "file",
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn device_verification_html(
    user_code: &str,
    message: &str,
    is_error: bool,
    account: Option<&UserPrincipal>,
    can_approve: bool,
) -> String {
    let tone = if is_error { "error" } else { "notice" };
    let account_html = account
        .map(|user| {
            format!(
                r#"<p class="account">Studio account: <strong>{}</strong></p>"#,
                escape_html(&user.email)
            )
        })
        .unwrap_or_else(|| {
            r#"<p class="account">No Studio session is active in this browser.</p>"#.to_string()
        });
    let form_html = if can_approve {
        format!(
            r#"<form method="post" action="/v1/desktop/device/approve">
        <label for="user_code">Device code</label>
        <input id="user_code" name="user_code" value="{user_code}" autocomplete="one-time-code" required autofocus />
        <button type="submit">Approve device</button>
      </form>"#,
            user_code = escape_html(user_code)
        )
    } else {
        format!(
            r#"<label for="user_code">Device code</label>
      <input id="user_code" name="user_code" value="{user_code}" autocomplete="one-time-code" readonly />"#,
            user_code = escape_html(user_code)
        )
    };
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Layrs Desktop Device Verification</title>
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
        width: min(460px, calc(100vw - 40px));
      }}
      h1 {{
        margin: 0 0 10px;
        font-size: 28px;
      }}
      p {{
        margin: 0 0 18px;
      }}
      .notice, .error {{
        padding: 12px 14px;
        border: 1px solid color-mix(in srgb, CanvasText 18%, Canvas 82%);
        border-radius: 8px;
        margin-bottom: 18px;
      }}
      .error {{
        border-color: #b42318;
        color: #b42318;
      }}
      .account {{
        padding: 10px 0;
        font-size: 14px;
      }}
      label {{
        display: block;
        font-weight: 700;
        margin-bottom: 8px;
      }}
      input {{
        box-sizing: border-box;
        width: 100%;
        min-height: 44px;
        padding: 10px 12px;
        border: 1px solid color-mix(in srgb, CanvasText 28%, Canvas 72%);
        border-radius: 6px;
        font: inherit;
        text-transform: uppercase;
      }}
      button {{
        margin-top: 14px;
        min-height: 44px;
        padding: 0 16px;
        border: 0;
        border-radius: 6px;
        background: CanvasText;
        color: Canvas;
        font: inherit;
        font-weight: 700;
        cursor: pointer;
      }}
    </style>
  </head>
  <body>
    <main>
      <h1>Layrs Desktop</h1>
      <p class="{tone}">{message}</p>
      {account_html}
      {form_html}
    </main>
  </body>
</html>"#,
        tone = tone,
        message = escape_html(message),
        account_html = account_html,
        form_html = form_html
    )
}
