//! Immutable, pinned cache of Git acquisition, independent of the native Store. Replay
//! revalidates raw Git objects and runs the regular native converter; no saved
//! native object or claimed import report is trusted as an installation plan.
use super::*;
use crate::Result;
use import::{AcquiredScan, FrozenMetadata};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
};

#[path = "scan_checkpoint_validation.rs"]
mod validation;

const MAGIC: &[u8] = b"LAYRS-GIT-SCAN\0\x01";
const MAX_HEADER: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitScanCheckpoint {
    pub path: PathBuf,
    /// Pin returned to the caller, required separately when reopening the file.
    pub digest: String,
    pub commits: usize,
    pub objects: usize,
    pub references: Vec<String>,
}

/// Acquires one complete immutable source snapshot. The new destination must
/// be outside the source repository. Once this returns, source Git is no longer
/// needed for conversion or retries; a later source change is a new snapshot.
pub fn prepare_git_scan_checkpoint(
    source: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    selection: &ImportSelection,
    limits: GitBridgeLimits,
    callback: &mut ProgressCallback<'_>,
) -> Result<GitScanCheckpoint> {
    let source = fs::canonicalize(source)?;
    let destination = std::path::absolute(destination)?;
    let parent = fs::canonicalize(
        destination
            .parent()
            .ok_or_else(|| invalid("checkpoint parent missing"))?,
    )?;
    let destination = parent.join(
        destination
            .file_name()
            .ok_or_else(|| invalid("checkpoint filename missing"))?,
    );
    let source_git = process::Git::new(&source, limits)?;
    let common = source_git.run(
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        None,
        16_384,
    )?;
    let common = fs::canonicalize(utf8(&common)?.trim())?;
    if within(&destination, &source) || within(&destination, &common) {
        return Err(invalid("scan checkpoint must be outside the Git source"));
    }
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(invalid("scan checkpoint destination must not exist"));
    }
    let acquired = import::acquire(&source, selection, limits, callback)?;
    validation::validate(&acquired, limits)?;
    let header = canonical_json(&acquired.metadata).map_err(invalid)?;
    if header.len() > MAX_HEADER.min(limits.max_total_bytes) {
        return Err(BridgeError::Limit("scan checkpoint header"));
    }
    // A temporary sibling is exclusively created; hard-link publication cannot
    // replace an existing destination on either Unix or Windows. Unsupported
    // filesystems fail explicitly. A crashed partial sibling is never resumed.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(invalid)?
        .as_nanos();
    let pending = parent.join(format!(
        ".layrs-git-scan-{}-{nonce}.incomplete",
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&pending)?;
    let mut hash = Sha256::new();
    let mut emit = |bytes: &[u8]| -> Result<()> {
        file.write_all(bytes)?;
        hash.update(bytes);
        Ok(())
    };
    emit(MAGIC)?;
    emit(&(header.len() as u64).to_be_bytes())?;
    emit(&header)?;
    emit(&(acquired.objects.len() as u64).to_be_bytes())?;
    for (id, (kind, bytes)) in &acquired.objects {
        let kind = match kind.as_str() {
            "commit" => 1,
            "tree" => 2,
            "blob" => 3,
            "tag" => 4,
            _ => return Err(invalid("checkpoint object kind")),
        };
        emit(&[kind, id.len() as u8])?;
        emit(id.as_bytes())?;
        emit(&(bytes.len() as u64).to_be_bytes())?;
        emit(bytes)?;
    }
    let digest = hash.finalize();
    file.write_all(&digest)?;
    file.sync_all()?;
    drop(file);
    fs::hard_link(&pending, &destination)?;
    // Only the exact sibling created above is removed, never a caller path.
    fs::remove_file(&pending)?;
    Ok(GitScanCheckpoint {
        path: destination,
        digest: format!("sha256:{digest:x}"),
        commits: acquired.metadata.commits.len(),
        objects: acquired.objects.len(),
        references: acquired
            .metadata
            .references
            .iter()
            .map(|r| r.0.clone())
            .collect(),
    })
}

/// Reconstructs the native plan from the pinned cache with zero Git processes
/// and no source-directory reads. Conversion restarts from its first commit;
/// this is a persisted acquisition checkpoint, not a serialized allocator.
pub fn resume_git_scan_checkpoint(
    checkpoint: impl AsRef<Path>,
    expected_digest: &str,
    limits: GitBridgeLimits,
    callback: &mut ProgressCallback<'_>,
) -> Result<ImportPlan> {
    let acquired = read(checkpoint.as_ref(), expected_digest, limits)?;
    validation::validate(&acquired, limits)?;
    import::convert(acquired, limits, callback)
}

fn read(path: &Path, expected: &str, limits: GitBridgeLimits) -> Result<AcquiredScan> {
    let started = std::time::Instant::now();
    if !expected.strip_prefix("sha256:").is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        return Err(invalid(
            "expected full lowercase scan checkpoint SHA256 pin",
        ));
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || is_reparse(&metadata) {
        return Err(invalid("scan checkpoint must be a regular non-link file"));
    }
    let max = limits
        .max_total_bytes
        .checked_add(MAX_HEADER.min(limits.max_total_bytes))
        .and_then(|n| n.checked_add(limits.max_objects.checked_mul(74)?))
        .and_then(|n| n.checked_add(128))
        .ok_or(BridgeError::Limit("scan checkpoint size"))?;
    if metadata.len() > max as u64 {
        return Err(BridgeError::Limit("scan checkpoint size"));
    }
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    if take(&mut file, &mut hash, MAGIC.len())? != MAGIC {
        return Err(invalid("scan checkpoint magic/version"));
    }
    let size = number(&mut file, &mut hash)?;
    if size > MAX_HEADER.min(limits.max_total_bytes) {
        return Err(BridgeError::Limit("scan checkpoint header"));
    }
    let header = take(&mut file, &mut hash, size)?;
    let metadata: FrozenMetadata = serde_json::from_slice(&header).map_err(invalid)?;
    if canonical_json(&metadata).map_err(invalid)? != header {
        return Err(invalid("noncanonical scan checkpoint header"));
    }
    let count = number(&mut file, &mut hash)?;
    if count > limits.max_objects {
        return Err(BridgeError::Limit("scan checkpoint objects"));
    }
    let mut objects = BTreeMap::new();
    let mut total = 0usize;
    for _ in 0..count {
        let kind = match take(&mut file, &mut hash, 1)?[0] {
            1 => "commit",
            2 => "tree",
            3 => "blob",
            4 => "tag",
            _ => return Err(invalid("checkpoint object kind")),
        };
        let len = take(&mut file, &mut hash, 1)?[0] as usize;
        if ![40, 64].contains(&len) {
            return Err(invalid("checkpoint Git ID length"));
        }
        let id = take(&mut file, &mut hash, len)?;
        let id = hex_id(utf8(&id)?)?.to_owned();
        if objects
            .last_key_value()
            .is_some_and(|(prior, _)| prior >= &id)
        {
            return Err(invalid("checkpoint object order/duplicate"));
        }
        let size = number(&mut file, &mut hash)?;
        total = total
            .checked_add(size)
            .ok_or(BridgeError::Limit("scan checkpoint bytes"))?;
        if size > limits.max_object_bytes || total > limits.max_total_bytes {
            return Err(BridgeError::Limit("scan checkpoint bytes"));
        }
        let bytes = take(&mut file, &mut hash, size)?;
        objects.insert(id, (kind.into(), bytes));
    }
    let mut trailer = [0u8; 32];
    file.read_exact(&mut trailer)?;
    let mut suffix = [0u8; 1];
    if file.read(&mut suffix)? != 0 {
        return Err(invalid("scan checkpoint trailing data"));
    }
    let actual = hash.finalize();
    if actual.as_slice() != trailer || format!("sha256:{actual:x}") != expected {
        return Err(invalid("scan checkpoint digest mismatch"));
    }
    Ok(AcquiredScan {
        metadata,
        objects,
        processes: 0,
        started,
    })
}

fn take(file: &mut File, hash: &mut Sha256, length: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0; length];
    file.read_exact(&mut bytes)?;
    hash.update(&bytes);
    Ok(bytes)
}
fn number(file: &mut File, hash: &mut Sha256) -> Result<usize> {
    let bytes: [u8; 8] = take(file, hash, 8)?
        .try_into()
        .map_err(|_| invalid("checkpoint integer framing"))?;
    usize::try_from(u64::from_be_bytes(bytes)).map_err(invalid)
}
fn within(path: &Path, directory: &Path) -> bool {
    if cfg!(windows) {
        PathBuf::from(path.to_string_lossy().to_lowercase())
            .starts_with(PathBuf::from(directory.to_string_lossy().to_lowercase()))
    } else {
        path.starts_with(directory)
    }
}
fn is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
