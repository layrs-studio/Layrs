async fn list_workspaces(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    Ok(Json(json!({
        "items": workspace_values(&state.pool, &user.id).await?
    })))
}

async fn create_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateWorkspaceBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    let name = required_body_text("name", &body.name)?;
    let slug = body.slug.unwrap_or_else(|| slugify(&name));
    let workspace_id = prefixed_id("workspace");

    create_workspace_owner_only(&state.pool, &user.id, &workspace_id, &name, &slug).await?;

    write_audit(
        &state.pool,
        Some(&workspace_id),
        Some(&user.id),
        "workspace.create",
        "workspace",
        Some(&workspace_id),
        json!({ "name": name, "slug": slug }),
    )
    .await?;

    Ok(Json(workspace_value(
        &workspace_id,
        &name,
        &slug,
        body.description.as_deref().unwrap_or_default(),
        "2026-06-29T00:00:00Z",
    )))
}

async fn create_team(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateTeamBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    let team_id = prefixed_id("team");
    let name = required_body_text("name", &body.name)?;
    let purpose = body.purpose.unwrap_or_default();

    sqlx::query("INSERT INTO teams (team_id, workspace_id, name, purpose) VALUES ($1, $2, $3, $4)")
        .bind(&team_id)
        .bind(&workspace_id)
        .bind(&name)
        .bind(&purpose)
        .execute(&state.pool)
        .await?;
    sqlx::query(
        "INSERT INTO team_memberships (team_id, account_id, role) VALUES ($1, $2, 'maintainer')",
    )
    .bind(&team_id)
    .bind(&user.id)
    .execute(&state.pool)
    .await?;

    Ok(Json(json!({
        "id": team_id,
        "workspaceId": workspace_id,
        "name": name,
        "purpose": purpose,
        "members": 1,
        "gateResponsibility": "workspace"
    })))
}

async fn list_teams(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    Ok(Json(
        json!({ "items": team_values(&state.pool, &workspace_id).await? }),
    ))
}

async fn get_team(
    State(state): State<AppState>,
    Path((workspace_id, team_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    Ok(Json(
        team_value(&state.pool, &workspace_id, &team_id).await?,
    ))
}

async fn list_team_members(
    State(state): State<AppState>,
    Path((workspace_id, team_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_team_in_workspace(&state.pool, &workspace_id, &team_id).await?;
    Ok(Json(json!({
        "items": team_member_values(&state.pool, &workspace_id, &team_id).await?
    })))
}

async fn add_team_member(
    State(state): State<AppState>,
    Path((workspace_id, team_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<AddTeamMemberBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    ensure_team_in_workspace(&state.pool, &workspace_id, &team_id).await?;
    let role = team_member_role(body.role.as_deref())?;
    let account_id = body.account_id.or(body.account_id_camel);

    if let Some(account_id) = account_id {
        ensure_account_in_workspace(&state.pool, &workspace_id, &account_id).await?;
        upsert_team_member(&state.pool, &workspace_id, &team_id, &account_id, role).await?;
        return Ok(Json(json!({
            "member": team_member_value(&state.pool, &workspace_id, &team_id, &account_id).await?
        })));
    }

    let email = normalize_email(
        body.email
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("account_id or email is required"))?,
    )?;
    if let Some(existing) = load_user_by_email(&state.pool, &email).await? {
        ensure_workspace_membership(&state.pool, &workspace_id, &existing.id, "member").await?;
        upsert_team_member(&state.pool, &workspace_id, &team_id, &existing.id, role).await?;
        return Ok(Json(json!({
            "member": team_member_value(&state.pool, &workspace_id, &team_id, &existing.id).await?
        })));
    }

    let invitation_id = create_pending_invitation(
        &state.pool,
        &workspace_id,
        &email,
        "member",
        &user.id,
        &[team_id],
        role,
    )
    .await?;
    Ok(Json(json!({
        "invitation": invitation_value(&state.pool, &invitation_id).await?
    })))
}

async fn remove_team_member(
    State(state): State<AppState>,
    Path((workspace_id, team_id, account_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    ensure_team_in_workspace(&state.pool, &workspace_id, &team_id).await?;
    let result = sqlx::query(
        r#"
        DELETE FROM team_memberships tm
        USING teams t
        WHERE tm.team_id = t.team_id
          AND t.workspace_id = $1
          AND tm.team_id = $2
          AND tm.account_id = $3
        "#,
    )
    .bind(&workspace_id)
    .bind(&team_id)
    .bind(&account_id)
    .execute(&state.pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("team member not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn list_workspace_members(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    Ok(Json(json!({
        "items": workspace_member_values(&state.pool, &workspace_id).await?
    })))
}

async fn list_workspace_invitations(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    Ok(Json(json!({
        "items": invitation_values_for_workspace(&state.pool, &workspace_id).await?
    })))
}

async fn create_invitation(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateInvitationBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    let email = normalize_email(&body.email)?;
    let role = workspace_role(body.role.as_deref())?;
    let team_ids = invitation_team_ids(body.team_ids, body.team_ids_camel);
    ensure_teams_in_workspace(&state.pool, &workspace_id, &team_ids).await?;
    let invitation_id = create_pending_invitation(
        &state.pool,
        &workspace_id,
        &email,
        role,
        &user.id,
        &team_ids,
        "member",
    )
    .await?;
    Ok(Json(invitation_value(&state.pool, &invitation_id).await?))
}

async fn list_my_invitations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    Ok(Json(json!({
        "items": invitation_values_for_email(&state.pool, &user.email).await?
    })))
}

async fn accept_invitation(
    State(state): State<AppState>,
    Path(invitation_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    accept_or_decline_invitation(&state.pool, &invitation_id, &user, true).await?;
    Ok(Json(invitation_value(&state.pool, &invitation_id).await?))
}

async fn decline_invitation(
    State(state): State<AppState>,
    Path(invitation_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    accept_or_decline_invitation(&state.pool, &invitation_id, &user, false).await?;
    Ok(Json(invitation_value(&state.pool, &invitation_id).await?))
}

