async fn revalidate_weave_target_paths_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    weave_id: &str,
    target_layer_id: &str,
    account_id: &str,
) -> Result<i64, ApiError> {
    // Policy writers update rules before bumping the epoch. These locks make the
    // authorization snapshot stable until the Apply transaction commits.
    sqlx::query("LOCK TABLE layer_access_policy_rules, layer_access_policies IN SHARE MODE")
        .execute(&mut **tx)
        .await?;

    let policy = sqlx::query(
        r#"
        SELECT policy_id, policy_epoch
        FROM layer_access_policies
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
        FOR SHARE
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(target_layer_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("layer access policy not found"))?;
    let policy_id = policy.get::<String, _>("policy_id");
    let policy_epoch = policy.get::<i64, _>("policy_epoch");

    let team_ids = account_team_ids_in_tx(tx, workspace_id, account_id).await?;
    let repository_role = effective_layer_role_in_tx(
        tx,
        workspace_id,
        space_id,
        target_layer_id,
        account_id,
        &team_ids,
    )
    .await?;
    if repository_role == RepositoryRole::Reader {
        return Err(ApiError::forbidden(
            "Layer write permission is required to apply this Weave",
        ));
    }

    let rule_rows = sqlx::query(
        r#"
        SELECT path, artifact_id, mode,
               read_account_ids, read_team_ids, write_account_ids, write_team_ids,
               admin_account_ids, admin_team_ids
        FROM layer_access_policy_rules
        WHERE policy_id = $1
        ORDER BY length(path) DESC
        "#,
    )
    .bind(&policy_id)
    .fetch_all(&mut **tx)
    .await?;
    let rules = rule_rows
        .iter()
        .map(|row| AccessRuleRow {
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
        })
        .collect::<Vec<_>>();
    let paths = weave_replay_paths_for_update_in_tx(tx, weave_id).await?;
    let artifact_rows = sqlx::query(
        r#"
        SELECT logical_path, artifact_id
        FROM artifacts
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
          AND logical_path = ANY($4)
        FOR SHARE
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(target_layer_id)
    .bind(&paths)
    .fetch_all(&mut **tx)
    .await?;
    let artifact_ids = artifact_rows
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("logical_path"),
                row.get::<String, _>("artifact_id"),
            )
        })
        .collect::<HashMap<_, _>>();

    for path in paths {
        if !weave_path_is_writable(
            &rules,
            &team_ids,
            account_id,
            &path,
            artifact_ids.get(&path).map(String::as_str),
        ) {
            return Err(ApiError::forbidden(
                "Weave target path is no longer writable under Layer access policy",
            ));
        }
    }
    Ok(policy_epoch)
}

async fn weave_replay_paths_for_update_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    weave_id: &str,
) -> Result<Vec<String>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT changed_paths, path_replays
        FROM weave_step_replays
        WHERE weave_id = $1
        ORDER BY order_index ASC
        FOR UPDATE
        "#,
    )
    .bind(weave_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut paths = BTreeMap::new();
    for row in rows {
        for path in row.get::<Vec<String>, _>("changed_paths") {
            paths.insert(validate_publish_path(&path)?, ());
        }
        let path_replays = row.get::<Value, _>("path_replays");
        let path_replays = path_replays
            .as_array()
            .ok_or_else(|| ApiError::internal("Weave replay path plan is invalid"))?;
        for path_replay in path_replays {
            let path = path_replay_path(path_replay)?;
            paths.insert(validate_publish_path(&path)?, ());
        }
    }
    Ok(paths.into_keys().collect())
}

fn weave_path_is_writable(
    rules: &[AccessRuleRow],
    team_ids: &HashSet<String>,
    account_id: &str,
    path: &str,
    artifact_id: Option<&str>,
) -> bool {
    let mut best: Option<&AccessRuleRow> = None;
    for rule in rules {
        let artifact_matches = match (artifact_id, rule.artifact_id.as_deref()) {
            (Some(artifact_id), Some(rule_artifact_id)) => artifact_id == rule_artifact_id,
            _ => false,
        };
        if artifact_matches || path_matches_rule(path, &rule.path) {
            let replace = best
                .map(|current| rule.path.len() > current.path.len())
                .unwrap_or(true);
            if replace {
                best = Some(rule);
            }
        }
    }

    let Some(rule) = best else {
        return true;
    };
    if rule.mode == "reserved_redacted" {
        return false;
    }
    rule.write_accounts.contains(account_id)
        || rule.admin_accounts.contains(account_id)
        || intersects(team_ids, &rule.write_teams)
        || intersects(team_ids, &rule.admin_teams)
}
