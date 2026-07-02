async fn session_response(
    state: &AppState,
    user: &UserPrincipal,
    status: StatusCode,
) -> Result<Response, ApiError> {
    let session_token = token("session");
    sqlx::query(
        r#"
        INSERT INTO web_sessions (session_id, account_id, session_token_digest, expires_at)
        VALUES ($1, $2, $3, now() + ($4 || ' seconds')::interval)
        "#,
    )
    .bind(prefixed_id("session"))
    .bind(&user.id)
    .bind(digest_secret(&session_token))
    .bind(SESSION_MAX_AGE_SECONDS)
    .execute(&state.pool)
    .await?;

    let mut response = (status, Json(auth_session_value(&state.pool, user).await?)).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&build_session_cookie(&session_token, &state.config))
            .map_err(|error| ApiError::bad_request(error.to_string()))?,
    );
    Ok(response)
}

async fn auth_session_value(pool: &PgPool, user: &UserPrincipal) -> Result<Value, ApiError> {
    let workspaces = workspace_values(pool, &user.id).await?;
    let active_workspace_id = workspaces
        .first()
        .and_then(|workspace| workspace.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(json!({
        "state": "authenticated",
        "account": studio_account_json(user),
        "session": session_json(user, active_workspace_id.as_deref()),
        "workspaces": workspaces,
        "activeWorkspaceId": active_workspace_id
    }))
}

async fn require_session(pool: &PgPool, headers: &HeaderMap) -> Result<UserPrincipal, ApiError> {
    let token = session_cookie(headers)
        .ok_or_else(|| ApiError::unauthorized("authentication is required"))?;
    let digest = digest_secret(&token);
    let row = sqlx::query(
        r#"
        SELECT a.account_id, a.email, a.display_name
        FROM web_sessions s
        JOIN accounts a ON a.account_id = s.account_id
        WHERE s.session_token_digest = $1
          AND s.revoked_at IS NULL
          AND s.expires_at > now()
          AND a.status = 'active'
        "#,
    )
    .bind(digest)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::unauthorized("authentication is required"))?;

    Ok(row_user(&row))
}

async fn optional_session(
    pool: &PgPool,
    headers: &HeaderMap,
) -> Result<Option<UserPrincipal>, ApiError> {
    let Some(token) = session_cookie(headers) else {
        return Ok(None);
    };
    let digest = digest_secret(&token);
    let row = sqlx::query(
        r#"
        SELECT a.account_id, a.email, a.display_name
        FROM web_sessions s
        JOIN accounts a ON a.account_id = s.account_id
        WHERE s.session_token_digest = $1
          AND s.revoked_at IS NULL
          AND s.expires_at > now()
          AND a.status = 'active'
        "#,
    )
    .bind(digest)
    .fetch_optional(pool)
    .await?;

    Ok(row.as_ref().map(row_user))
}

async fn require_principal(pool: &PgPool, headers: &HeaderMap) -> Result<UserPrincipal, ApiError> {
    if let Ok(user) = require_session(pool, headers).await {
        return Ok(user);
    }
    require_bearer(pool, headers).await
}

async fn require_bearer(pool: &PgPool, headers: &HeaderMap) -> Result<UserPrincipal, ApiError> {
    let token = bearer_token(headers)
        .ok_or_else(|| ApiError::unauthorized("missing desktop bearer token"))?;
    let digest = digest_secret(&token);
    let row = sqlx::query(
        r#"
        SELECT a.account_id, a.email, a.display_name
        FROM desktop_device_tokens t
        JOIN accounts a ON a.account_id = t.account_id
        WHERE t.access_token_digest = $1
          AND t.revoked_at IS NULL
          AND t.expires_at > now()
          AND a.status = 'active'
        "#,
    )
    .bind(digest)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::unauthorized("desktop bearer token is invalid"))?;

    Ok(row_user(&row))
}

async fn load_user_by_id(
    pool: &PgPool,
    account_id: &str,
) -> Result<Option<UserPrincipal>, ApiError> {
    let row = sqlx::query(
        "SELECT account_id, email, display_name FROM accounts WHERE account_id = $1 AND status = 'active'",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| row_user(&row)))
}

fn row_user(row: &sqlx::postgres::PgRow) -> UserPrincipal {
    UserPrincipal {
        id: row.get("account_id"),
        email: row.get("email"),
        display_name: row.get("display_name"),
    }
}

async fn ensure_workspace_member(
    pool: &PgPool,
    workspace_id: &str,
    account_id: &str,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workspace_memberships WHERE workspace_id = $1 AND account_id = $2 AND state = 'active')",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::forbidden("workspace membership is required"))
    }
}

async fn ensure_workspace_admin(
    pool: &PgPool,
    workspace_id: &str,
    account_id: &str,
) -> Result<(), ApiError> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM workspace_memberships WHERE workspace_id = $1 AND account_id = $2 AND state = 'active'",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    match role.as_deref() {
        Some("owner" | "admin") => Ok(()),
        Some(_) => Err(ApiError::forbidden(
            "workspace admin permission is required",
        )),
        None => Err(ApiError::forbidden("workspace membership is required")),
    }
}

async fn ensure_workspace_membership(
    pool: &PgPool,
    workspace_id: &str,
    account_id: &str,
    role: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO workspace_memberships (membership_id, workspace_id, account_id, role, state)
        VALUES ($1, $2, $3, $4, 'active')
        ON CONFLICT (workspace_id, account_id) DO UPDATE
        SET role = CASE
                WHEN workspace_memberships.role IN ('owner', 'admin') THEN workspace_memberships.role
                ELSE excluded.role
            END,
            state = 'active',
            updated_at = now()
        "#,
    )
    .bind(prefixed_id("membership"))
    .bind(workspace_id)
    .bind(account_id)
    .bind(role)
    .execute(pool)
    .await?;
    Ok(())
}

async fn ensure_account_in_workspace(
    pool: &PgPool,
    workspace_id: &str,
    account_id: &str,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workspace_memberships WHERE workspace_id = $1 AND account_id = $2 AND state = 'active')",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::bad_request("account is not a workspace member"))
    }
}

async fn ensure_accounts_in_workspace(
    pool: &PgPool,
    workspace_id: &str,
    account_ids: &[String],
) -> Result<(), ApiError> {
    let account_ids = unique_strings(account_ids);
    if account_ids.is_empty() {
        return Ok(());
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT account_id)::bigint FROM workspace_memberships WHERE workspace_id = $1 AND state = 'active' AND account_id = ANY($2)",
    )
    .bind(workspace_id)
    .bind(&account_ids)
    .fetch_one(pool)
    .await?;
    if count == account_ids.len() as i64 {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "access rule references an account outside this workspace",
        ))
    }
}

async fn ensure_team_in_workspace(
    pool: &PgPool,
    workspace_id: &str,
    team_id: &str,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM teams WHERE workspace_id = $1 AND team_id = $2)",
    )
    .bind(workspace_id)
    .bind(team_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::not_found("team not found"))
    }
}

async fn ensure_teams_in_workspace(
    pool: &PgPool,
    workspace_id: &str,
    team_ids: &[String],
) -> Result<(), ApiError> {
    let team_ids = unique_strings(team_ids);
    if team_ids.is_empty() {
        return Ok(());
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT team_id)::bigint FROM teams WHERE workspace_id = $1 AND team_id = ANY($2)",
    )
    .bind(workspace_id)
    .bind(&team_ids)
    .fetch_one(pool)
    .await?;
    if count == team_ids.len() as i64 {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "request references a team outside this workspace",
        ))
    }
}

async fn ensure_layer_in_space(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM layers WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3)",
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::not_found("layer not found"))
    }
}

async fn ensure_space_in_workspace(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM spaces WHERE workspace_id = $1 AND space_id = $2)",
    )
    .bind(workspace_id)
    .bind(space_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::not_found("space not found"))
    }
}

async fn ensure_workspace_write(
    pool: &PgPool,
    workspace_id: &str,
    account_id: &str,
) -> Result<(), ApiError> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM workspace_memberships WHERE workspace_id = $1 AND account_id = $2 AND state = 'active'",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    match role.as_deref() {
        Some("owner" | "admin" | "member") => Ok(()),
        Some(_) => Err(ApiError::forbidden(
            "workspace write permission is required",
        )),
        None => Err(ApiError::forbidden("workspace membership is required")),
    }
}

async fn workspace_value_for_account(
    pool: &PgPool,
    workspace_id: &str,
    account_id: &str,
) -> Result<Value, ApiError> {
    workspace_values(pool, account_id)
        .await?
        .into_iter()
        .find(|workspace| workspace.get("id").and_then(Value::as_str) == Some(workspace_id))
        .ok_or_else(|| ApiError::not_found("workspace not found"))
}
