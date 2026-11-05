//! Explicit Store-local declarations, never remote authentication credentials.
use super::{NativeStoreExclusive, NativeStoreOpen};
use crate::layout::{
    create_new_file_no_follow, open_read_no_follow, reject_reparse_components, sync_directory,
};
use layrs_core::scm::{ActorId, DeviceId, StoreId, canonical_json};
use layrs_store::scm::{Result, StoreError};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::Path,
};

const SCHEMA: &str = "layrs.local-identity-registry.v1";
const FILE: &str = "local-identities.json";
const MAX_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalIdentity {
    pub actor_id: ActorId,
    pub device_id: Option<DeviceId>,
    pub display_name: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct IdentityRegistry {
    schema_version: String,
    store_id: StoreId,
    actors: BTreeMap<ActorId, Option<String>>,
    devices: BTreeSet<DeviceId>,
    active: Option<LocalIdentity>,
}
impl IdentityRegistry {
    fn empty(store_id: &StoreId) -> Self {
        Self {
            schema_version: SCHEMA.into(),
            store_id: store_id.clone(),
            actors: BTreeMap::new(),
            devices: BTreeSet::new(),
            active: None,
        }
    }
    pub(crate) fn actor_exists(&self, actor: &ActorId) -> bool {
        self.actors.contains_key(actor)
    }
    pub(crate) fn device_exists(&self, device: &DeviceId) -> bool {
        self.devices.contains(device)
    }
    fn validate(&self, store: &StoreId) -> Result<()> {
        if self.schema_version != SCHEMA
            || self.store_id != *store
            || self.actors.len() > 1024
            || self.devices.len() > 1024
        {
            return Err(invalid(
                "invalid local identity registry scope/version/limit",
            ));
        }
        for name in self.actors.values().flatten() {
            validate_name(name)?;
        }
        if let Some(active) = &self.active {
            if self.actors.get(&active.actor_id) != Some(&active.display_name)
                || active
                    .device_id
                    .as_ref()
                    .is_some_and(|id| !self.devices.contains(id))
            {
                return Err(invalid("active local identity is not registered"));
            }
        }
        Ok(())
    }
}

pub(crate) fn read_registry(root: &Path, store: &StoreId) -> Result<IdentityRegistry> {
    let path = root.join("metadata").join(FILE);
    reject_reparse_components(path.parent().expect("identity parent")).map_err(io_error)?;
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(IdentityRegistry::empty(store));
        }
        Err(e) => return Err(io_error(e)),
        Ok(meta) if !meta.is_file() || meta.len() > MAX_BYTES => {
            return Err(invalid("local identity registry file invalid or too large"));
        }
        Ok(_) => {}
    }
    reject_reparse_components(&path).map_err(io_error)?;
    let mut bytes = vec![];
    open_read_no_follow(&path)
        .map_err(io_error)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid("local identity registry exceeds byte limit"));
    }
    let registry: IdentityRegistry = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("invalid local identity registry JSON"))?;
    registry.validate(store)?;
    if canonical_json(&registry).map_err(|_| invalid("identity canonical encoding"))? != bytes {
        return Err(invalid("local identity registry must be canonical"));
    }
    Ok(registry)
}

pub fn local_identity(store: &NativeStoreOpen) -> Result<Option<LocalIdentity>> {
    Ok(read_registry(store.layout().root(), store.store_id())?.active)
}

/// The exclusive Store capability prevents configuration racing admission.
/// Previously registered IDs remain valid for already prepared mutations.
pub fn configure_local_identity(
    store: &NativeStoreExclusive,
    identity: LocalIdentity,
) -> Result<LocalIdentity> {
    if let Some(name) = &identity.display_name {
        validate_name(name)?;
    }
    let mut registry = read_registry(store.layout().root(), store.store_id())?;
    registry
        .actors
        .insert(identity.actor_id.clone(), identity.display_name.clone());
    if let Some(device) = &identity.device_id {
        registry.devices.insert(device.clone());
    }
    registry.active = Some(identity.clone());
    registry.validate(store.store_id())?;
    let bytes = canonical_json(&registry).map_err(|_| invalid("identity canonical encoding"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid("local identity registry exceeds byte limit"));
    }
    let parent = store.layout().metadata_dir();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| invalid("identity clock unavailable"))?
        .as_nanos();
    let temporary = parent.join(format!(
        ".local-identities-{}-{nonce}.tmp",
        std::process::id()
    ));
    let path = parent.join(FILE);
    let result = (|| -> std::io::Result<()> {
        let mut file = create_new_file_no_follow(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        sync_directory(parent)?;
        super::format::atomic_replace(&temporary, &path)?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(io_error)?;
    Ok(identity)
}
fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err(invalid(
            "local identity display name must contain 1..256 non-control bytes",
        ));
    }
    Ok(())
}
fn invalid(message: &str) -> StoreError {
    StoreError::Corruption {
        message: message.into(),
    }
}
fn io_error(error: std::io::Error) -> StoreError {
    StoreError::Backend {
        message: format!("local identity: {error}"),
    }
}
