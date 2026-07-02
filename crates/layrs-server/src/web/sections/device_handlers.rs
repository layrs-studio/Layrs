async fn start_device_flow(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let device_flow_id = prefixed_id("device_flow");
    let device_code = token("device");
    let user_code = format!(
        "LAYRS-{}",
        &Uuid::new_v4().simple().to_string()[..6].to_uppercase()
    );

    sqlx::query(
        r#"
        INSERT INTO device_authorization_flows
            (device_flow_id, device_code_digest, user_code_digest, account_id, client_name, public_key_thumbprint, interval_seconds, expires_at)
        VALUES
            ($1, $2, $3, $4, 'Layrs Desktop', 'local-dev-thumbprint', $5, now() + ($6 || ' seconds')::interval)
        "#,
    )
    .bind(&device_flow_id)
    .bind(digest_secret(&device_code))
    .bind(digest_secret(&user_code))
    .bind(Option::<String>::None)
    .bind(DEVICE_FLOW_POLL_INTERVAL_SECONDS as i32)
    .bind(DEVICE_FLOW_EXPIRES_SECONDS)
    .execute(&state.pool)
    .await?;

    let verification_uri = format!("http://{}/v1/desktop/device", state.config.addr);
    let verification_uri_complete = format!("{verification_uri}?user_code={user_code}");

    Ok(Json(json!({
        "device_code": device_code,
        "user_code": user_code,
        "verification_uri": verification_uri,
        "verification_uri_complete": verification_uri_complete,
        "interval_seconds": DEVICE_FLOW_POLL_INTERVAL_SECONDS,
        "expires_in_seconds": DEVICE_FLOW_EXPIRES_SECONDS
    })))
}

#[derive(Deserialize)]
struct DevicePageQuery {
    #[serde(default)]
    user_code: Option<String>,
    #[serde(default, rename = "userCode")]
    user_code_camel: Option<String>,
}

#[derive(Deserialize)]
struct DeviceApproveForm {
    #[serde(default)]
    user_code: Option<String>,
    #[serde(default, rename = "userCode")]
    user_code_camel: Option<String>,
}

#[derive(Deserialize)]
struct DevicePollRequest {
    #[serde(default)]
    device_code: Option<String>,
    #[serde(default, rename = "deviceCode")]
    device_code_camel: Option<String>,
}

async fn device_verification_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DevicePageQuery>,
) -> Html<String> {
    let user_code = query
        .user_code
        .or(query.user_code_camel)
        .unwrap_or_default();
    let account = optional_session(&state.pool, &headers).await.ok().flatten();
    let message = account
        .as_ref()
        .map(|user| {
            format!(
                "Approve this device as {}. Only Spaces visible to this account will appear in Desktop.",
                user.email
            )
        })
        .unwrap_or_else(|| {
            "Sign in to Layrs Studio in this browser before approving this device.".to_string()
        });
    Html(device_verification_html(
        &user_code,
        &message,
        account.is_none(),
        account.as_ref(),
        account.is_some(),
    ))
}

async fn approve_device_flow(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(body): Form<DeviceApproveForm>,
) -> Result<Response, ApiError> {
    let user_code = normalize_user_code(
        &body
            .user_code
            .or(body.user_code_camel)
            .ok_or_else(|| ApiError::bad_request("user_code is required"))?,
    )?;
    let user = match require_session(&state.pool, &headers).await {
        Ok(user) => user,
        Err(_) => {
            return Ok((
                StatusCode::UNAUTHORIZED,
                Html(device_verification_html(
                    &user_code,
                    "Sign in to Layrs Studio in this browser before approving this device.",
                    true,
                    None,
                    false,
                )),
            )
                .into_response());
        }
    };
    let row = sqlx::query(
        r#"
        SELECT device_flow_id, status, expires_at < now() AS expired, approved_at IS NOT NULL AS has_approved_at
        FROM device_authorization_flows
        WHERE user_code_digest = $1
        "#,
    )
    .bind(digest_secret(&user_code))
    .fetch_optional(&state.pool)
    .await?;

    let Some(row) = row else {
        return Ok((
            StatusCode::NOT_FOUND,
            Html(device_verification_html(
                &user_code,
                "This device code was not found. Check the code and try again.",
                true,
                Some(&user),
                false,
            )),
        )
            .into_response());
    };

    let status: String = row.get("status");
    let expired: bool = row.get("expired");
    let has_approved_at: bool = row.get("has_approved_at");
    let rejection = if expired || status == "expired" {
        if has_approved_at {
            Some("This device code was already used. Start a new login from Layrs Desktop.")
        } else {
            Some("This device code has expired. Start a new login from Layrs Desktop.")
        }
    } else if status == "denied" {
        Some("This device code was denied. Start a new login from Layrs Desktop.")
    } else if status == "approved" {
        Some("This device code is already approved. Return to Layrs Desktop and click Check now.")
    } else {
        None
    };

    if let Some(message) = rejection {
        return Ok((
            StatusCode::BAD_REQUEST,
            Html(device_verification_html(
                &user_code,
                message,
                true,
                Some(&user),
                false,
            )),
        )
            .into_response());
    }

    let approved = sqlx::query(
        r#"
        UPDATE device_authorization_flows
        SET status = 'approved', approved_at = now(), account_id = $2
        WHERE device_flow_id = $1 AND status = 'pending' AND expires_at >= now()
        "#,
    )
    .bind(row.get::<String, _>("device_flow_id"))
    .bind(&user.id)
    .execute(&state.pool)
    .await?;
    if approved.rows_affected() != 1 {
        return Ok((
            StatusCode::BAD_REQUEST,
            Html(device_verification_html(
                &user_code,
                "This device code could not be approved. Start a new login from Layrs Desktop.",
                true,
                Some(&user),
                false,
            )),
        )
            .into_response());
    }

    Ok(Html(device_verification_html(
        &user_code,
        &format!(
            "Device approved for {}. Return to Layrs Desktop and click Check now.",
            user.email
        ),
        false,
        Some(&user),
        false,
    ))
    .into_response())
}

#[derive(Deserialize)]
struct LayerAccessPolicyBody {
    #[serde(default)]
    rules: Vec<LayerAccessRuleBody>,
    #[serde(default)]
    signature: Option<LayerAccessSignatureBody>,
}

#[derive(Deserialize)]
struct LayerAccessRuleBody {
    #[serde(default)]
    id: Option<String>,
    path: String,
    #[serde(default)]
    artifact_id: Option<String>,
    mode: String,
    #[serde(default = "default_stub_visibility")]
    visibility: String,
    permissions: LayerAccessPermissionsBody,
}

#[derive(Deserialize)]
struct LayerAccessPermissionsBody {
    #[serde(default)]
    read: LayerAccessPrincipalsBody,
    #[serde(default)]
    write: LayerAccessPrincipalsBody,
    #[serde(default)]
    admin: LayerAccessPrincipalsBody,
}

#[derive(Default, Deserialize)]
struct LayerAccessPrincipalsBody {
    #[serde(default)]
    accounts: Vec<String>,
    #[serde(default)]
    teams: Vec<String>,
}

#[derive(Deserialize)]
struct LayerAccessSignatureBody {
    key_id: String,
    value: String,
}

async fn poll_device_flow(
    State(state): State<AppState>,
    Json(body): Json<DevicePollRequest>,
) -> Result<Response, ApiError> {
    let device_code = body
        .device_code
        .or(body.device_code_camel)
        .ok_or_else(|| ApiError::bad_request("device_code is required"))?;
    let code_digest = digest_secret(&device_code);
    let row = sqlx::query(
        r#"
        UPDATE device_authorization_flows
        SET poll_count = poll_count + 1
        WHERE device_code_digest = $1
        RETURNING device_flow_id, account_id, status, poll_count, expires_at < now() AS expired, approved_at IS NOT NULL AS has_approved_at
        "#,
    )
    .bind(code_digest)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::unauthorized("device code is unknown"))?;

    let expired: bool = row.get("expired");
    let status: String = row.get("status");
    let has_approved_at: bool = row.get("has_approved_at");
    if expired || status == "expired" || status == "denied" {
        if status == "expired" && has_approved_at {
            return Err(ApiError::bad_request("device code was already consumed"));
        }
        return Err(ApiError::bad_request("device code is expired or denied"));
    }
    if status == "pending" {
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "status": "authorization_pending",
                "interval_seconds": DEVICE_FLOW_POLL_INTERVAL_SECONDS
            })),
        )
            .into_response());
    }

    if status != "approved" {
        return Err(ApiError::bad_request("device code is not approved"));
    }

    let account_id: Option<String> = row.get("account_id");
    let account_id = account_id
        .ok_or_else(|| ApiError::unauthorized("device approval is missing a Studio account"))?;
    let device_flow_id: String = row.get("device_flow_id");
    let (user, access_token) =
        issue_desktop_token_for_flow(&state, &device_flow_id, &account_id).await?;

    Ok(Json(json!({
        "status": "authorized",
        "user": desktop_user_json(&user),
        "desktop_token": {
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in_seconds": DESKTOP_TOKEN_EXPIRES_SECONDS
        }
    }))
    .into_response())
}

async fn desktop_bootstrap(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_bearer(&state.pool, &headers).await?;
    let workspaces = workspace_values(&state.pool, &user.id).await?;
    let workspace_ids = workspaces
        .iter()
        .filter_map(|workspace| {
            workspace
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    let spaces = space_summaries_for_workspaces(&state.pool, &workspace_ids).await?;
    let layers = layer_summaries_for_workspaces(&state.pool, &workspace_ids).await?;

    Ok(Json(json!({
        "user": desktop_user_json(&user),
        "account": desktop_account_json(&user),
        "workspaces": workspaces,
        "spaces": spaces,
        "layers": layers,
        "server": {
            "mode": "postgres",
            "database_url_configured": true,
            "routes_path": "/v1/routes"
        }
    })))
}

async fn issue_desktop_token_for_flow(
    state: &AppState,
    device_flow_id: &str,
    account_id: &str,
) -> Result<(UserPrincipal, String), ApiError> {
    let user = load_user_by_id(&state.pool, account_id)
        .await?
        .ok_or_else(|| ApiError::unauthorized("device account is unavailable"))?;
    let access_token = token("desktop");
    let refresh_token = token("desktop_refresh");
    let device_id = prefixed_id("device");
    let token_id = prefixed_id("desktop_token");

    let mut tx = state.pool.begin().await?;
    let consumed = sqlx::query(
        r#"
        UPDATE device_authorization_flows
        SET status = 'expired', approved_at = COALESCE(approved_at, now())
        WHERE device_flow_id = $1 AND status IN ('pending', 'approved') AND expires_at >= now()
        "#,
    )
    .bind(device_flow_id)
    .execute(&mut *tx)
    .await?;
    if consumed.rows_affected() != 1 {
        return Err(ApiError::bad_request(
            "device code is expired, denied or already consumed",
        ));
    }

    sqlx::query(
        r#"
        INSERT INTO desktop_devices (device_id, account_id, display_name, public_key_thumbprint, last_seen_at)
        VALUES ($1, $2, 'Layrs Desktop', 'local-dev-thumbprint', now())
        "#,
    )
    .bind(&device_id)
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO desktop_device_tokens
            (token_id, device_id, account_id, access_token_digest, refresh_token_digest, expires_at)
        VALUES
            ($1, $2, $3, $4, $5, now() + ($6 || ' seconds')::interval)
        "#,
    )
    .bind(&token_id)
    .bind(&device_id)
    .bind(&user.id)
    .bind(digest_secret(&access_token))
    .bind(digest_secret(&refresh_token))
    .bind(DESKTOP_TOKEN_EXPIRES_SECONDS)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok((user, access_token))
}
