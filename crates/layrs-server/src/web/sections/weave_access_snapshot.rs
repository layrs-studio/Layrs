#[derive(Debug)]
struct WeaveLayerAuthorizationSnapshot {
    repository_role: RepositoryRole,
    policy_epoch: i64,
    rules: Vec<AccessRuleRow>,
}

#[derive(Debug)]
struct WeaveAuthorizationSnapshot {
    account_id: String,
    team_ids: HashSet<String>,
    layers: HashMap<String, WeaveLayerAuthorizationSnapshot>,
}

impl WeaveAuthorizationSnapshot {
    fn layer(&self, layer_id: &str) -> Result<&WeaveLayerAuthorizationSnapshot, ApiError> {
        self.layers
            .get(layer_id)
            .ok_or_else(|| ApiError::internal("Weave authorization snapshot is incomplete"))
    }

    fn policy_epoch(&self, layer_id: &str) -> Result<i64, ApiError> {
        Ok(self.layer(layer_id)?.policy_epoch)
    }

    fn decision(
        &self,
        layer_id: &str,
        path: &str,
        artifact_id: Option<&str>,
    ) -> Result<AccessDecision, ApiError> {
        let layer = self.layer(layer_id)?;
        Ok(access_decision_from_rules(
            layer.repository_role,
            &self.team_ids,
            &layer.rules,
            &self.account_id,
            path,
            artifact_id,
        ))
    }
}

#[derive(Debug, Default)]
struct WeaveArtifactIds {
    by_layer_and_path: HashMap<(String, String), String>,
}

impl WeaveArtifactIds {
    fn get(&self, layer_id: &str, path: &str) -> Option<&str> {
        self.by_layer_and_path
            .get(&(layer_id.to_string(), path.to_string()))
            .map(String::as_str)
    }
}

#[derive(Debug, PartialEq)]
struct WeaveLayerGeneration {
    root_tree_id: Option<String>,
    latest_step_id: Option<String>,
}

async fn load_weave_authorization_snapshot_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_ids: &[String],
    account_id: &str,
) -> Result<WeaveAuthorizationSnapshot, ApiError> {
    let layer_ids = sorted_unique_strings(layer_ids);
    if layer_ids.is_empty() {
        return Err(ApiError::internal(
            "Weave authorization snapshot requires at least one Layer",
        ));
    }

    // Every Weave authorization path uses this order: Layers, actor/teams/roles,
    // policies, then rules. Policy writers lock the policy before touching rules.
    let locked_layers = sqlx::query_scalar::<_, String>(
        r#"
        SELECT layer_id
        FROM layers
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = ANY($3)
        ORDER BY layer_id ASC
        FOR SHARE
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(&layer_ids)
    .fetch_all(&mut **tx)
    .await?;
    if locked_layers != layer_ids {
        return Err(ApiError::not_found("layer not found"));
    }

    lock_active_weave_actor_in_tx(tx, account_id).await?;
    let team_ids = account_team_ids_in_tx(tx, workspace_id, account_id).await?;
    let mut roles = HashMap::new();
    for layer_id in &layer_ids {
        let role =
            effective_layer_role_in_tx(tx, workspace_id, space_id, layer_id, account_id, &team_ids)
                .await?;
        roles.insert(layer_id.clone(), role);
    }

    let policy_rows = sqlx::query(
        r#"
        SELECT policy_id, layer_id, policy_epoch
        FROM layer_access_policies
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = ANY($3)
        ORDER BY policy_id ASC
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(&layer_ids)
    .fetch_all(&mut **tx)
    .await?;
    if policy_rows.len() != layer_ids.len() {
        return Err(ApiError::not_found("layer access policy not found"));
    }

    let policy_ids = policy_rows
        .iter()
        .map(|row| row.get::<String, _>("policy_id"))
        .collect::<Vec<_>>();
    let rule_rows = sqlx::query(
        r#"
        SELECT policy_id, path, artifact_id, mode,
               read_account_ids, read_team_ids, write_account_ids, write_team_ids,
               admin_account_ids, admin_team_ids
        FROM layer_access_policy_rules
        WHERE policy_id = ANY($1)
        ORDER BY policy_id ASC, length(path) DESC, rule_id ASC
        FOR SHARE
        "#,
    )
    .bind(&policy_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut rules_by_policy = HashMap::<String, Vec<AccessRuleRow>>::new();
    for row in rule_rows {
        rules_by_policy
            .entry(row.get("policy_id"))
            .or_default()
            .push(access_rule_row(&row));
    }

    let mut layers = HashMap::new();
    for row in policy_rows {
        let layer_id = row.get::<String, _>("layer_id");
        let policy_id = row.get::<String, _>("policy_id");
        let repository_role = roles
            .remove(&layer_id)
            .ok_or_else(|| ApiError::internal("Weave Layer role snapshot is incomplete"))?;
        layers.insert(
            layer_id,
            WeaveLayerAuthorizationSnapshot {
                repository_role,
                policy_epoch: row.get("policy_epoch"),
                rules: rules_by_policy.remove(&policy_id).unwrap_or_default(),
            },
        );
    }

    Ok(WeaveAuthorizationSnapshot {
        account_id: account_id.to_string(),
        team_ids,
        layers,
    })
}

fn access_rule_row(row: &sqlx::postgres::PgRow) -> AccessRuleRow {
    AccessRuleRow {
        path: row.get("path"),
        artifact_id: row.try_get("artifact_id").ok(),
        mode: row.get("mode"),
        read_accounts: hashset(
            row.try_get::<Vec<String>, _>("read_account_ids")
                .unwrap_or_default(),
        ),
        read_teams: hashset(
            row.try_get::<Vec<String>, _>("read_team_ids")
                .unwrap_or_default(),
        ),
        write_accounts: hashset(
            row.try_get::<Vec<String>, _>("write_account_ids")
                .unwrap_or_default(),
        ),
        write_teams: hashset(
            row.try_get::<Vec<String>, _>("write_team_ids")
                .unwrap_or_default(),
        ),
        admin_accounts: hashset(
            row.try_get::<Vec<String>, _>("admin_account_ids")
                .unwrap_or_default(),
        ),
        admin_teams: hashset(
            row.try_get::<Vec<String>, _>("admin_team_ids")
                .unwrap_or_default(),
        ),
    }
}

async fn weave_artifact_ids_for_paths(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_ids: &[String],
    paths: &[String],
) -> Result<WeaveArtifactIds, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT layer_id, logical_path, artifact_id
        FROM artifacts
        WHERE workspace_id = $1 AND space_id = $2
          AND layer_id = ANY($3) AND logical_path = ANY($4)
        ORDER BY layer_id ASC, logical_path ASC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(sorted_unique_strings(layer_ids))
    .bind(sorted_unique_strings(paths))
    .fetch_all(pool)
    .await?;
    Ok(weave_artifact_ids(rows))
}

async fn weave_artifact_ids_for_paths_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_ids: &[String],
    paths: &[String],
) -> Result<WeaveArtifactIds, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT layer_id, logical_path, artifact_id
        FROM artifacts
        WHERE workspace_id = $1 AND space_id = $2
          AND layer_id = ANY($3) AND logical_path = ANY($4)
        ORDER BY layer_id ASC, logical_path ASC
        FOR SHARE
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(sorted_unique_strings(layer_ids))
    .bind(sorted_unique_strings(paths))
    .fetch_all(&mut **tx)
    .await?;
    Ok(weave_artifact_ids(rows))
}

fn weave_artifact_ids(rows: Vec<sqlx::postgres::PgRow>) -> WeaveArtifactIds {
    WeaveArtifactIds {
        by_layer_and_path: rows
            .into_iter()
            .map(|row| {
                (
                    (
                        row.get::<String, _>("layer_id"),
                        row.get::<String, _>("logical_path"),
                    ),
                    row.get::<String, _>("artifact_id"),
                )
            })
            .collect(),
    }
}

fn weave_path_access_decisions(
    snapshot: &WeaveAuthorizationSnapshot,
    artifacts: &WeaveArtifactIds,
    source_layer_id: &str,
    target_layer_id: &str,
    path: &str,
) -> Result<(AccessDecision, AccessDecision), ApiError> {
    let source = snapshot.decision(source_layer_id, path, artifacts.get(source_layer_id, path))?;
    let target = snapshot.decision(target_layer_id, path, artifacts.get(target_layer_id, path))?;
    Ok((source, target))
}

async fn lock_weave_layer_generations_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_ids: &[String],
) -> Result<HashMap<String, WeaveLayerGeneration>, ApiError> {
    let layer_ids = sorted_unique_strings(layer_ids);
    let head_rows = sqlx::query(
        r#"
        SELECT layer_id, root_tree_id
        FROM layer_heads
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = ANY($3)
        ORDER BY layer_id ASC
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(&layer_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut root_trees = head_rows
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("layer_id"),
                row.try_get::<Option<String>, _>("root_tree_id")
                    .unwrap_or(None),
            )
        })
        .collect::<HashMap<_, _>>();

    let mut generations = HashMap::new();
    for layer_id in layer_ids {
        let latest_step_id = sqlx::query_scalar::<_, String>(
            r#"
            SELECT step_id
            FROM layer_steps
            WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
              AND cleared_at IS NULL
            ORDER BY COALESCE(timeline_position, 9223372036854775807) DESC,
                     captured_at DESC, created_at DESC, step_id DESC
            LIMIT 1
            FOR SHARE
            "#,
        )
        .bind(workspace_id)
        .bind(space_id)
        .bind(&layer_id)
        .fetch_optional(&mut **tx)
        .await?;
        generations.insert(
            layer_id.clone(),
            WeaveLayerGeneration {
                root_tree_id: root_trees.remove(&layer_id).flatten(),
                latest_step_id,
            },
        );
    }
    Ok(generations)
}

async fn ensure_weave_abort_actor_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    target_layer_id: &str,
    requested_by_account_id: Option<&str>,
    account_id: &str,
) -> Result<(), ApiError> {
    lock_active_weave_actor_in_tx(tx, account_id).await?;
    let team_ids = account_team_ids_in_tx(tx, workspace_id, account_id).await?;
    let target_role = effective_layer_role_in_tx(
        tx,
        workspace_id,
        space_id,
        target_layer_id,
        account_id,
        &team_ids,
    )
    .await?;

    // Abort is intentionally narrower than target write: the active requester
    // may withdraw their request, while workspace/Space/target-Layer admins may
    // stop it administratively. Ordinary writers cannot abort another request.
    if requested_by_account_id == Some(account_id) || target_role == RepositoryRole::Admin {
        return Ok(());
    }
    Err(ApiError::forbidden(
        "Only the Weave requester or a workspace, Space, or target Layer admin can abort it",
    ))
}

async fn lock_active_weave_actor_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account_id: &str,
) -> Result<(), ApiError> {
    let account = sqlx::query_scalar::<_, String>(
        "SELECT account_id FROM accounts WHERE account_id = $1 AND status = 'active' FOR SHARE",
    )
    .bind(account_id)
    .fetch_optional(&mut **tx)
    .await?;
    if account.is_some() {
        Ok(())
    } else {
        Err(ApiError::unauthorized("active account is required"))
    }
}

fn sorted_unique_strings(values: &[String]) -> Vec<String> {
    let mut values = values.to_vec();
    values.sort();
    values.dedup();
    values
}

fn weave_session_optional_string(
    payload: &Value,
    key: &'static str,
) -> Result<Option<String>, ApiError> {
    match payload.get(key) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(ApiError::conflict(format!(
            "Weave plan has an invalid {key} snapshot; prepare a new Weave plan"
        ))),
        None => Err(ApiError::conflict(format!(
            "Weave plan predates {key} capture; prepare a new Weave plan"
        ))),
    }
}
