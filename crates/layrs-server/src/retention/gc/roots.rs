use super::{files::Directory, types::*};
use crate::{backup, config::ServerConfig};
use layrs_core::scm::ObjectId;
use layrs_store_server::v3::{CasLimits, FsServerCas};
use layrs_sync::v3::ObjectDescriptor;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub struct Roots {
    pub _backup_guard: FsServerCas,
    pub transaction: Transaction<'static, Postgres>,
    pub catalog: BTreeMap<String, ObjectDescriptor>,
    pub retained: BTreeSet<String>,
    pub reservations: usize,
    pub digest: String,
    pub backup_digest: String,
}
pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub async fn read(
    pool: &PgPool,
    config: &ServerConfig,
    backup_path: &Path,
    root: &Directory,
) -> GcResult<Roots> {
    let backup_path = std::fs::canonicalize(backup_path)?;
    let manifest_bytes = Directory::root(&backup_path)?
        .read("manifest.json", MAX_RECORD_BYTES)?
        .ok_or("GC backup manifest missing")?;
    let bounded: backup::BackupManifest = serde_json::from_slice(&manifest_bytes)?;
    if bounded.objects.len() > 100000 {
        return Err("GC backup object bound exceeded".into());
    }
    let mut backup_budget = 0u64;
    for object in &bounded.objects {
        backup_budget = backup_budget
            .checked_add(object.encoded_byte_len.max(object.canonical_byte_len))
            .ok_or("GC backup length overflow")?;
        if backup_budget > MAX_TOTAL_BYTES
            || object.encoded_byte_len > MAX_FILE_BYTES
            || object.canonical_byte_len > MAX_FILE_BYTES
        {
            return Err("GC backup verification budget exceeded".into());
        }
    }
    if backup_path.starts_with(std::fs::canonicalize(&config.cas_root)?) {
        return Err("GC backup must be outside the managed CAS".into());
    }
    let backup_guard = FsServerCas::open(backup_path.join("cas"), CasLimits::default()).await?;
    // Backup validation independently verifies every canonical closure, including
    // historical Views, Markers, Releases, conflicts, evidence and policies.
    let backup = backup::verify_backup(&backup_path).await?;
    if backup.deployment_id != config.deployment_id || backup.store_id != config.store_id {
        return Err("GC backup deployment/Store differs".into());
    }
    let backup_root = Directory::root(&backup_path.join("cas"))?;
    if backup_root.identity()? == root.identity()? {
        return Err("GC backup must be an independent CAS copy".into());
    }
    let backup_bytes = serde_jcs::to_vec(&backup)?;
    let backup_digest = digest(&backup_bytes);
    let backup_objects: BTreeMap<_, _> = backup
        .objects
        .iter()
        .map(|o| (o.object_id.as_str(), o))
        .collect();
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='15s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("LOCK TABLE canonical_objects,object_plan_items,object_leases IN SHARE MODE")
        .execute(&mut *tx)
        .await?;
    let configured =
        serde_jcs::to_string(&serde_json::json!({"path":config.cas_root.to_string_lossy()}))?;
    let valid:bool=sqlx::query_scalar("SELECT count(*)=1 AND bool_and(s.store_id=$1 AND s.backend_kind='filesystem' AND s.backend_config_jcs=$2) FROM stores s").bind(config.store_id.as_str()).bind(configured).fetch_one(&mut *tx).await?;
    if !valid {
        return Err("GC requires the single configured filesystem Store".into());
    }
    let rows=sqlx::query("SELECT object_id,object_kind,canonical_byte_length,encoded_byte_length,compression,storage_key,verification_state FROM canonical_objects ORDER BY object_id LIMIT 100001").fetch_all(&mut *tx).await?;
    if rows.len() > 100000 {
        return Err("GC catalogue bound exceeded".into());
    }
    let mut catalog = BTreeMap::new();
    let mut retained = BTreeSet::new();
    let mut total = 0u64;
    for row in rows {
        if row.get::<String, _>("verification_state") != "verified" {
            return Err("GC retains inconsistent/quarantined catalogue: repair required".into());
        }
        let object = ObjectDescriptor {
            object_id: ObjectId::parse(row.get::<String, _>("object_id"))?,
            kind: serde_json::from_value(serde_json::Value::String(row.get("object_kind")))?,
            canonical_byte_len: u64::try_from(row.get::<i64, _>("canonical_byte_length"))?,
            encoded_byte_len: u64::try_from(row.get::<i64, _>("encoded_byte_length"))?,
            compression: serde_json::from_value(serde_json::Value::String(row.get("compression")))?,
        };
        if backup_objects.get(object.object_id.as_str()).copied() != Some(&object) {
            return Err("GC backup does not cover the exact current catalogue".into());
        }
        total = total
            .checked_add(object.encoded_byte_len)
            .ok_or("GC length overflow")?;
        if total > MAX_TOTAL_BYTES {
            return Err("GC verification byte budget exceeded".into());
        }
        let key: String = row.get("storage_key");
        let kind: String = row.get("object_kind");
        let hex = &object.object_id.as_str()[7..];
        if key != format!("objects/{kind}/{}/{hex}", &hex[..2]) {
            return Err("GC catalogue storage key is not canonical".into());
        }
        let current = root
            .fingerprint(&key)?
            .ok_or("GC catalogued file missing")?;
        let saved = backup_root
            .fingerprint(&key)?
            .ok_or("GC backup file missing")?;
        if current.0.bytes != object.encoded_byte_len || current.1 != saved.1 {
            return Err("GC current CAS differs from verified backup".into());
        }
        retained.insert(object.object_id.as_str().to_owned());
        catalog.insert(key, object);
    }
    let ids:Vec<String>=sqlx::query_scalar("SELECT object_id FROM object_plan_items UNION SELECT object_id FROM object_leases ORDER BY object_id LIMIT 100001").fetch_all(&mut *tx).await?;
    if ids.len() > 100000 {
        return Err("GC reservation bound exceeded".into());
    }
    for id in &ids {
        ObjectId::parse(id.clone())?;
        retained.insert(id.clone());
    }
    // Also preserve every object in the verified backup, including an older root.
    for id in backup_objects.keys() {
        retained.insert((*id).to_owned());
    }
    let digest = digest(&serde_jcs::to_vec(&(&catalog, &ids))?);
    Ok(Roots {
        _backup_guard: backup_guard,
        transaction: tx,
        catalog,
        retained,
        reservations: ids.len(),
        digest,
        backup_digest,
    })
}
