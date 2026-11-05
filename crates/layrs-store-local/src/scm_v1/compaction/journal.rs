use super::{ACTIVE, CompactionResult, MetadataCompactionReport};
use crate::{
    layout::{
        create_dir_all_durable, create_new_file_no_follow, open_read_no_follow,
        reject_reparse_components, sync_directory,
    },
    scm_v1::NativeStoreLayout,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
const MAX_DATABASE: u64 = 1024 * 1024 * 1024;
const MAX_JOURNAL: u64 = 16 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Journal {
    pub report: MetadataCompactionReport,
    pub before_hash: String,
    pub after_hash: String,
}

pub fn create_directory(layout: &NativeStoreLayout) -> CompactionResult<PathBuf> {
    create_dir_all_durable(layout.repair_backups_dir())?;
    reject_reparse_components(layout.repair_backups_dir())?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let path = layout
        .repair_backups_dir()
        .join(format!("compaction-{now:032x}-{:08x}", std::process::id()));
    fs::create_dir(&path)?;
    sync_directory(layout.repair_backups_dir())?;
    Ok(path)
}
pub fn file_hash(path: &Path) -> CompactionResult<String> {
    reject_reparse_components(path)?;
    let mut file = open_read_no_follow(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_DATABASE {
        return Err("compaction file invalid or exceeds 1 GiB".into());
    }
    let mut digest = blake3::Hasher::new();
    let mut buffer = [0u8; 65536];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > MAX_DATABASE {
            return Err("compaction file grew beyond bound".into());
        }
        digest.update(&buffer[..n]);
    }
    if size != metadata.len() {
        return Err("compaction file size changed".into());
    }
    Ok(digest.finalize().to_hex().to_string())
}
pub fn copy_new(source: &Path, destination: &Path) -> CompactionResult<()> {
    reject_reparse_components(source)?;
    reject_reparse_components(destination.parent().ok_or("copy destination parent")?)?;
    let mut input = open_read_no_follow(source)?;
    if input.metadata()?.len() > MAX_DATABASE {
        return Err("compaction source exceeds 1 GiB".into());
    }
    let mut output = create_new_file_no_follow(destination)?;
    let length = std::io::copy(
        &mut Read::by_ref(&mut input).take(MAX_DATABASE + 1),
        &mut output,
    )?;
    if length > MAX_DATABASE {
        return Err("compaction source grew beyond bound".into());
    }
    output.sync_all()?;
    sync_directory(destination.parent().ok_or("copy destination parent")?)?;
    Ok(())
}
fn write_new(path: &Path, bytes: &[u8]) -> CompactionResult<()> {
    if bytes.len() as u64 > MAX_JOURNAL {
        return Err("compaction journal exceeds bound".into());
    }
    match fs::symlink_metadata(path) {
        Ok(_) => {
            if read_bytes(path)? == bytes {
                return Ok(());
            }
            return Err("compaction immutable record differs".into());
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    let parent = path.parent().ok_or("journal parent")?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let staged = parent.join(format!(".record-{nonce:032x}.tmp"));
    let mut file = create_new_file_no_follow(&staged)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    super::super::format::atomic_replace(&staged, path)?;
    sync_directory(parent)?;
    Ok(())
}
fn read_bytes(path: &Path) -> CompactionResult<Vec<u8>> {
    reject_reparse_components(path)?;
    let mut bytes = Vec::new();
    open_read_no_follow(path)?
        .take(MAX_JOURNAL + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JOURNAL {
        return Err("compaction journal exceeds bound".into());
    }
    Ok(bytes)
}
pub fn validate(layout: &NativeStoreLayout, record: &Journal) -> CompactionResult<()> {
    let report = &record.report;
    let name = report
        .backup_directory
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("compaction backup name")?;
    if report.schema_version != "layrs.metadata-compaction.v1"
        || report.history_deleted
        || !report.restart_history_traversal
        || report.backup_directory.parent() != Some(layout.repair_backups_dir())
        || !name.starts_with("compaction-")
        || name.len() > 80
        || !name[11..]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b'-')
        || report.before_bytes > MAX_DATABASE
        || report.after_bytes > MAX_DATABASE
        || report.retained_rows > 1_000_000
    {
        return Err("invalid compaction journal scope or limits".into());
    }
    reject_reparse_components(&report.backup_directory)?;
    for hash in [
        &record.before_hash,
        &record.after_hash,
        &report.logical_digest,
    ] {
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid compaction digest".into());
        }
    }
    let backup = report.backup_directory.join("before.sqlite3");
    if fs::metadata(&backup)?.len() != report.before_bytes
        || file_hash(&backup)? != record.before_hash
    {
        return Err("compaction backup differs".into());
    }
    Ok(())
}
pub fn publish(layout: &NativeStoreLayout, record: &Journal) -> CompactionResult<()> {
    validate(layout, record)?;
    let bytes = serde_jcs::to_vec(record)?;
    write_new(&record.report.backup_directory.join("journal.json"), &bytes)?;
    let staged = record.report.backup_directory.join("active.json");
    write_new(&staged, &bytes)?;
    super::super::format::atomic_replace(&staged, &layout.metadata_dir().join(ACTIVE))?;
    sync_directory(&record.report.backup_directory)?;
    sync_directory(layout.metadata_dir())?;
    Ok(())
}
pub fn read(layout: &NativeStoreLayout) -> CompactionResult<Journal> {
    let bytes = read_bytes(&layout.metadata_dir().join(ACTIVE))?;
    let record: Journal = serde_json::from_slice(&bytes)?;
    if serde_jcs::to_vec(&record)? != bytes {
        return Err("compaction journal is not canonical".into());
    }
    validate(layout, &record)?;
    Ok(record)
}
pub fn complete(_layout: &NativeStoreLayout, record: &Journal) -> CompactionResult<()> {
    write_new(
        &record.report.backup_directory.join("completed.json"),
        &serde_jcs::to_vec(record)?,
    )
}
