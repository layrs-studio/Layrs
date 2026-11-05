use super::{files::Directory, roots, types::*};
use crate::{
    backup,
    config::ServerConfig,
    retention::{self, AuditLimits},
};
use layrs_store_server::v3::FsCasMaintenance;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::Path;

pub async fn command(pool: &PgPool, config: &ServerConfig, args: &[String]) -> GcResult<Value> {
    match args.first().map(String::as_str) {
        Some("gc-plan") if args.len() == 2 => {
            create_plan(pool, config, Path::new(&args[1]), &SystemGcClock).await
        }
        Some("gc-apply" | "gc-resume" | "gc-restore" | "gc-purge") if args.len() == 3 => {
            let recovered = if args[0] == "gc-resume" {
                let root = Directory::root(&config.cas_root)?;
                active(&root)?
                    .filter(|value| value.plan_id == args[1])
                    .map(|value| value.action)
            } else {
                None
            };
            let action = match args[0].as_str() {
                "gc-restore" => "restore",
                "gc-purge" => "purge",
                "gc-resume" => recovered.as_deref().unwrap_or("quarantine"),
                _ => "quarantine",
            };
            execute(
                pool,
                config,
                &args[1],
                Path::new(&args[2]),
                action,
                &SystemGcClock,
                &NoFaults,
            )
            .await
        }
        _ => {
            Err("GC usage: gc-plan BACKUP or gc-{apply,resume,restore,purge} PLAN_ID BACKUP".into())
        }
    }
}
pub trait GcFaults: Send + Sync {
    /// MAX marks durable active journal; MAX-1 marks durable completion.
    fn after_mutation(&self, index: usize) -> GcResult<()>;
}
pub struct NoFaults;
impl GcFaults for NoFaults {
    fn after_mutation(&self, _: usize) -> GcResult<()> {
        Ok(())
    }
}
fn validate_completion(
    value: &Completion,
    id: &str,
    action: &str,
    plan: &GcPlan,
    now: u64,
) -> GcResult<()> {
    if value.plan_id != id
        || value.action != action
        || value.completed_at < plan.created_at
        || value.completed_at > now
        || value.affected.checked_add(value.retained) != Some(plan.candidates.len())
        || (action != "purge" && value.retained != 0)
    {
        return Err("GC completion journal is inconsistent".into());
    }
    Ok(())
}
fn record(plan: &str, name: &str) -> String {
    format!(".layrs-retention/{plan}/{name}.json")
}
fn quarantine(plan: &str, index: usize) -> String {
    format!(".layrs-retention/{plan}/quarantine/{index:06}")
}
fn parse<T: DeserializeOwned + serde::Serialize>(bytes: &[u8]) -> GcResult<T> {
    let value: T = serde_json::from_slice(bytes)?;
    if serde_jcs::to_vec(&value)? != bytes {
        return Err("GC journal is not canonical".into());
    }
    Ok(value)
}
fn id_valid(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn active(root: &Directory) -> GcResult<Option<Active>> {
    root.read(ACTIVE, MAX_RECORD_BYTES)?
        .map(|v| parse(&v))
        .transpose()
}
fn validate_config(root: &Directory, config: &ServerConfig, plan: &GcPlan) -> GcResult<()> {
    if plan.schema_version != "layrs.gc-plan.v1"
        || plan.deployment_id != config.deployment_id
        || plan.store_id != config.store_id.as_str()
        || root.identity()? != (plan.root_device, plan.root_inode)
        || plan.retain_seconds < MIN_RETENTION_SECONDS
        || plan.candidates.len() > MAX_CANDIDATES
    {
        return Err("GC plan does not match this installation".into());
    }
    Ok(())
}
fn check_restore_marker(root: &Directory) -> GcResult<()> {
    if root
        .read(backup::RESTORE_MARKER, MAX_RECORD_BYTES)?
        .is_some()
    {
        return Err("GC refuses an unfinished restore".into());
    }
    Ok(())
}
fn object_id(path: &str) -> Option<String> {
    let pieces: Vec<_> = path.split('/').collect();
    if pieces.len() != 4
        || pieces[0] != "objects"
        || !matches!(
            pieces[1],
            "chunk"
                | "file_manifest"
                | "step"
                | "tree"
                | "conflict"
                | "repository_view"
                | "product"
                | "policy_set"
                | "evidence"
                | "authorization_snapshot"
                | "symlink"
                | "tombstone"
        )
        || !id_valid(pieces[3])
        || pieces[2] != &pieces[3][..2]
    {
        return None;
    }
    Some(format!("blake3:{}", pieces[3]))
}
pub async fn create_plan(
    pool: &PgPool,
    config: &ServerConfig,
    backup: &Path,
    clock: &dyn GcClock,
) -> GcResult<Value> {
    let _fence = FsCasMaintenance::try_acquire(&config.cas_root)?;
    let root = Directory::root(&config.cas_root)?;
    check_restore_marker(&root)?;
    if active(&root)?.is_some() {
        return Err("resume the active GC before planning".into());
    }
    let report = retention::audit(pool, config, AuditLimits::default()).await?;
    if !report.missing_files.is_empty() || !report.size_mismatches.is_empty() {
        return Err("GC refuses inconsistent catalogue/files".into());
    }
    let roots = roots::read(pool, config, backup, &root).await?;
    let now = clock.now()?;
    let mut candidates = Vec::new();
    let mut bytes = 0u64;
    for finding in report.files_to_examine {
        let Some(id) = object_id(&finding.relative_path) else {
            continue;
        };
        if roots.retained.contains(&id) {
            continue;
        }
        let Some((identity, raw_sha256)) = root.fingerprint(&finding.relative_path)? else {
            return Err("GC file disappeared".into());
        };
        let modified =
            u64::try_from(identity.modified_seconds).map_err(|_| "GC timestamp before epoch")?;
        if now.saturating_sub(modified) < MIN_RETENTION_SECONDS || modified > now {
            continue;
        }
        bytes = bytes
            .checked_add(identity.bytes)
            .ok_or("GC byte overflow")?;
        if bytes > MAX_TOTAL_BYTES || candidates.len() >= MAX_CANDIDATES {
            return Err("GC candidate bound exceeded".into());
        }
        candidates.push(Candidate {
            path: finding.relative_path,
            object_id: id,
            identity,
            raw_sha256,
        });
    }
    candidates.sort_by(|a, b| a.path.cmp(&b.path));
    let (root_device, root_inode) = root.identity()?;
    let plan = GcPlan {
        schema_version: "layrs.gc-plan.v1".into(),
        deployment_id: config.deployment_id.clone(),
        store_id: config.store_id.as_str().into(),
        root_device,
        root_inode,
        created_at: now,
        retain_seconds: MIN_RETENTION_SECONDS,
        backup_manifest_sha256: roots.backup_digest,
        roots_sha256: roots.digest,
        catalogued_retained: roots.catalog.len(),
        reserved_retained: roots.reservations,
        candidates,
    };
    let document = serde_jcs::to_vec(&plan)?;
    let id = roots::digest(&document);
    root.write_new(&record(&id, "plan"), &document)?;
    roots.transaction.commit().await?;
    Ok(
        json!({"schemaVersion":"layrs.gc-plan-result.v1","planId":id,"candidateFiles":plan.candidates.len(),"candidateBytes":bytes,"cataloguedRetained":plan.catalogued_retained,"reservedRetained":plan.reserved_retained,"mutation":"plan-only","retainSeconds":MIN_RETENTION_SECONDS}),
    )
}

pub async fn execute(
    pool: &PgPool,
    config: &ServerConfig,
    id: &str,
    backup: &Path,
    action: &str,
    clock: &dyn GcClock,
    faults: &dyn GcFaults,
) -> GcResult<Value> {
    if !id_valid(id) || !matches!(action, "quarantine" | "restore" | "purge") {
        return Err("invalid GC action/plan ID".into());
    }
    let _fence = FsCasMaintenance::try_acquire(&config.cas_root)?;
    let root = Directory::root(&config.cas_root)?;
    check_restore_marker(&root)?;
    let bytes = root
        .read(&record(id, "plan"), MAX_RECORD_BYTES)?
        .ok_or("GC plan missing")?;
    if roots::digest(&bytes) != id {
        return Err("GC plan digest mismatch".into());
    }
    let plan: GcPlan = parse(&bytes)?;
    validate_config(&root, config, &plan)?;
    let journal = Active {
        plan_id: id.into(),
        action: action.into(),
    };
    let previous = active(&root)?;
    if previous.as_ref().is_some_and(|value| value != &journal) {
        return Err("another GC action requires recovery".into());
    }
    let completion_path = record(id, action);
    if (action != "restore"
        && root
            .read(&record(id, "restore"), MAX_RECORD_BYTES)?
            .is_some())
        || (action != "purge" && root.read(&record(id, "purge"), MAX_RECORD_BYTES)?.is_some())
    {
        return Err("GC plan is already terminal".into());
    }
    if let Some(bytes) = root.read(&completion_path, MAX_RECORD_BYTES)? {
        let complete: Completion = parse(&bytes)?;
        validate_completion(&complete, id, action, &plan, clock.now()?)?;
        if previous.is_some() {
            root.remove_record(ACTIVE, &serde_jcs::to_vec(&journal)?)?
        }
        return Ok(
            json!({"schemaVersion":"layrs.gc-result.v1","planId":id,"action":action,"replayed":true,"affected":complete.affected,"retained":complete.retained}),
        );
    }
    if root
        .read(&record(id, "restore"), MAX_RECORD_BYTES)?
        .is_some()
        || root.read(&record(id, "purge"), MAX_RECORD_BYTES)?.is_some()
    {
        return Err("GC plan is already terminal".into());
    }
    let roots = roots::read(pool, config, backup, &root).await?;
    if action != "purge"
        && (roots.digest != plan.roots_sha256 || roots.backup_digest != plan.backup_manifest_sha256)
    {
        return Err("GC roots or backup changed; plan refused".into());
    }
    let now = clock.now()?;
    if now < plan.created_at {
        return Err("GC clock moved backwards".into());
    }
    if action == "purge" {
        let completed: Completion = parse(
            &root
                .read(&record(id, "quarantine"), MAX_RECORD_BYTES)?
                .ok_or("GC quarantine not completed")?,
        )?;
        validate_completion(&completed, id, "quarantine", &plan, now)?;
        if completed.completed_at > now || now - completed.completed_at < plan.retain_seconds {
            return Err("GC quarantine retention delay not elapsed".into());
        }
    }
    // Prevalidate every candidate before starting any mutation. Restoration also
    // supports an interrupted quarantine, but requires finishing its journal first.
    let mut retained = 0;
    let mut total = 0u64;
    if plan.candidates.windows(2).any(|p| p[0].path >= p[1].path) {
        return Err("GC candidates are not strictly ordered".into());
    }
    for (index, candidate) in plan.candidates.iter().enumerate() {
        if object_id(&candidate.path).as_deref() != Some(candidate.object_id.as_str()) {
            return Err("GC candidate path differs from ID".into());
        }
        total = total
            .checked_add(candidate.identity.bytes)
            .ok_or("GC byte overflow")?;
        if total > MAX_TOTAL_BYTES {
            return Err("GC byte bound exceeded".into());
        }
        let modified = u64::try_from(candidate.identity.modified_seconds)
            .map_err(|_| "GC candidate timestamp invalid")?;
        if modified > plan.created_at || plan.created_at - modified < plan.retain_seconds {
            return Err("GC candidate retention age invalid".into());
        }
        if roots.retained.contains(&candidate.object_id) {
            if action == "purge" {
                let stored = root.fingerprint(&quarantine(id, index))?;
                let source = root.fingerprint(&candidate.path)?;
                if let Some(value) = &stored {
                    if value != &(candidate.identity.clone(), candidate.raw_sha256.clone()) {
                        return Err("GC protected quarantine changed".into());
                    }
                }
                if let Some(value) = &source {
                    if value.0.bytes != candidate.identity.bytes || value.1 != candidate.raw_sha256
                    {
                        return Err("GC protected source collision; both copies retained".into());
                    }
                }
                if source.is_none() && stored.is_none() {
                    return Err("GC protected candidate missing".into());
                }
                retained += 1;
                continue;
            }
            return Err("GC candidate acquired an authority/reservation".into());
        }
        let destination = quarantine(id, index);
        // Ensure destination parent exists before optional reads; empty directories
        // are not data changes, and are retained for audit/recovery.
        root.child(".layrs-retention", true)?
            .child(id, true)?
            .child("quarantine", true)?;
        let source = root.fingerprint(&candidate.path)?;
        let stored = root.fingerprint(&destination)?;
        match (source, stored) {
            (Some(value), None) if action != "purge" => {
                if value != (candidate.identity.clone(), candidate.raw_sha256.clone()) {
                    return Err("GC source changed".into());
                }
            }
            (None, Some(value)) => {
                if value != (candidate.identity.clone(), candidate.raw_sha256.clone()) {
                    return Err("GC quarantine changed".into());
                }
            }
            (None, None) if action == "purge" && previous.is_some() => (),
            _ => return Err("GC candidate missing, duplicated or inconsistent".into()),
        }
    }
    let active_bytes = serde_jcs::to_vec(&journal)?;
    if previous.is_none() {
        root.write_new(ACTIVE, &active_bytes)?
    }
    faults.after_mutation(usize::MAX)?;
    let mut affected = 0;
    for (index, candidate) in plan.candidates.iter().enumerate() {
        if action == "purge" && roots.retained.contains(&candidate.object_id) {
            let destination = quarantine(id, index);
            // A reservation acquired since quarantine must not strand its only
            // bytes outside the active CAS. Never overwrite an existing object.
            if root.fingerprint(&candidate.path)?.is_none()
                && root.fingerprint(&destination)?.is_some()
            {
                root.move_checked(&destination, &candidate.path, candidate)?;
            }
            faults.after_mutation(index)?;
            continue;
        }
        let destination = quarantine(id, index);
        match action {
            "quarantine" => {
                if root.fingerprint(&candidate.path)?.is_some() {
                    root.move_checked(&candidate.path, &destination, candidate)?
                }
            }
            "restore" => {
                if root.fingerprint(&destination)?.is_some() {
                    root.move_checked(&destination, &candidate.path, candidate)?
                }
            }
            "purge" => {
                if root.fingerprint(&destination)?.is_some() {
                    root.unlink_checked(&destination, candidate)?
                }
            }
            _ => unreachable!(),
        }
        affected += 1;
        faults.after_mutation(index)?;
    }
    let completion = Completion {
        plan_id: id.into(),
        action: action.into(),
        completed_at: now,
        affected,
        retained,
    };
    root.write_new(&completion_path, &serde_jcs::to_vec(&completion)?)?;
    faults.after_mutation(usize::MAX - 1)?;
    root.remove_record(ACTIVE, &active_bytes)?;
    roots.transaction.commit().await?;
    Ok(
        json!({"schemaVersion":"layrs.gc-result.v1","planId":id,"action":action,"replayed":false,"affected":affected,"retained":retained,"cataloguedRetained":roots.catalog.len()}),
    )
}
