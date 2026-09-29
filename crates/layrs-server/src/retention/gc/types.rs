use serde::{Deserialize, Serialize};

pub type GcResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub const MIN_RETENTION_SECONDS: u64 = 24 * 60 * 60;
pub const MAX_CANDIDATES: usize = 1_000;
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_RECORD_BYTES: usize = 2 * 1024 * 1024;
pub const ACTIVE: &str = ".layrs-gc-active";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Identity {
    pub device: u64,
    pub inode: u64,
    pub bytes: u64,
    pub modified_seconds: i64,
    pub modified_nanos: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Candidate {
    pub path: String,
    pub object_id: String,
    pub identity: Identity,
    pub raw_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GcPlan {
    pub schema_version: String,
    pub deployment_id: String,
    pub store_id: String,
    pub root_device: u64,
    pub root_inode: u64,
    pub created_at: u64,
    pub retain_seconds: u64,
    pub backup_manifest_sha256: String,
    pub roots_sha256: String,
    pub catalogued_retained: usize,
    pub reserved_retained: usize,
    pub candidates: Vec<Candidate>,
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg(target_os = "linux")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Active {
    pub plan_id: String,
    pub action: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[cfg(target_os = "linux")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Completion {
    pub plan_id: String,
    pub action: String,
    pub completed_at: u64,
    pub affected: usize,
    pub retained: usize,
}

/// Clock is injectable for deterministic retention tests. CLI always uses OS time.
pub trait GcClock: Send + Sync {
    fn now(&self) -> GcResult<u64>;
}
pub struct SystemGcClock;
impl GcClock for SystemGcClock {
    fn now(&self) -> GcResult<u64> {
        Ok(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs())
    }
}
