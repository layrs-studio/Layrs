use super::*;
use layrs_lens_sdk::{
    sha256_digest, LensReconcileLabelsContract, LensReconcileRequest, LensReconcileResultStatus,
    LensReconcileSideContract as Side,
};
use layrs_weave::reconciliation::{
    native_manifest, reconcile_file, FailureAction, FileMergeLimits, FileMergeOutcome,
    FileMergePolicy, NativeLens,
};

pub(super) fn reconcile(
    path: &RepoPath,
    base: Option<CapturedFile>,
    local: Option<CapturedFile>,
    remote: Option<CapturedFile>,
    graph: &GraphObjects,
    spools: &mut BTreeMap<FileId, File>,
) -> RemoteResult<ProjectionMergeFile> {
    let mut row = ProjectionMergeFile {
        path: path.clone(),
        base,
        local,
        remote,
        conflict: false,
        automatic: None,
    };
    if row.local == row.base {
        row.automatic = row.remote.clone();
        return Ok(row);
    }
    if row.remote == row.base || row.local == row.remote {
        row.automatic = row.local.clone();
        return Ok(row);
    }
    row.conflict = true;
    let mode = |f: &Option<CapturedFile>| f.as_ref().map(|f| f.executable);
    let (b, l, r) = (mode(&row.base), mode(&row.local), mode(&row.remote));
    let mode = if l == b {
        r
    } else if r == b || l == r {
        l
    } else {
        return Ok(row);
    };
    let size: u64 = [&row.base, &row.local, &row.remote]
        .iter()
        .map(|f| f.as_ref().map_or(0, |f| f.bytes))
        .sum();
    // Leave room for path, labels and pin in the Lens's deterministic quota.
    if size > 1024 * 1024 - 8192 {
        return Ok(row);
    }
    let mut side = |file: &Option<CapturedFile>| -> RemoteResult<Side> {
        let Some(file) = file else {
            return Ok(Side::absent());
        };
        let bytes = if let Some(spool) = spools.get_mut(&file.file_id) {
            spool.seek(SeekFrom::Start(0))?;
            let mut bytes = Vec::new();
            spool.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 != file.bytes || FileId::from_bytes(&bytes) != file.file_id {
                return Err(NativeRemoteError::Invalid("partial merge spool changed"));
            }
            bytes
        } else {
            graph.file_bytes(&file.file_id)?
        };
        Ok(Side::present(bytes.as_slice(), Some(sha256_digest(&bytes))))
    };
    let request = LensReconcileRequest::new(
        native_manifest(NativeLens::Text)
            .pin()
            .ok_or(NativeRemoteError::Invalid("native text Lens pin missing"))?,
        path.as_str(),
        Some("text/plain".into()),
        side(&row.base)?,
        side(&row.local)?,
        side(&row.remote)?,
        LensReconcileLabelsContract {
            existing: "local".into(),
            incoming: "remote".into(),
        },
    );
    let result = reconcile_file(
        &request,
        FileMergePolicy {
            unsupported: FailureAction::StructuralFallback,
            quota: FailureAction::StructuralFallback,
            limits: FileMergeLimits::default(),
        },
    )
    .map_err(|e| local_error(format!("partial reconciliation failed: {e:?}")))?;
    if let FileMergeOutcome::Semantic(result) = result.outcome {
        if result.status == LensReconcileResultStatus::AutoResolved {
            let resolved = result
                .resolved
                .ok_or(NativeRemoteError::Invalid("clean merge value missing"))?;
            if resolved.exists {
                let Some(executable) = mode else {
                    return Ok(row);
                };
                let file_id = FileId::from_bytes(&resolved.bytes);
                let mut spool = layrs_store_local::private_spool_file()?;
                spool.write_all(&resolved.bytes)?;
                row.automatic = Some(CapturedFile {
                    file_id: file_id.clone(),
                    executable,
                    bytes: resolved.bytes.len() as u64,
                });
                spools.insert(file_id, spool);
            }
            row.conflict = false;
        }
    }
    Ok(row)
}
fn local_error(message: String) -> NativeRemoteError {
    NativeRemoteError::Io(std::io::Error::other(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan(
        base: Option<&[u8]>,
        local: Option<&[u8]>,
        remote: Option<&[u8]>,
    ) -> (ProjectionMergeFile, BTreeMap<FileId, File>) {
        let mut spools = BTreeMap::new();
        let mut file = |bytes: Option<&[u8]>| {
            bytes.map(|bytes| {
                let file_id = FileId::from_bytes(bytes);
                let mut spool = layrs_store_local::private_spool_file().unwrap();
                spool.write_all(bytes).unwrap();
                spools.insert(file_id.clone(), spool);
                CapturedFile {
                    file_id,
                    bytes: bytes.len() as u64,
                    executable: false,
                }
            })
        };
        let (base, local, remote) = (file(base), file(local), file(remote));
        let result = reconcile(
            &RepoPath::parse("file.txt").unwrap(),
            base,
            local,
            remote,
            &GraphObjects::default(),
            &mut spools,
        )
        .unwrap();
        (result, spools)
    }
    #[test]
    fn disjoint_edits_merge_but_overlaps_remain_choices() {
        let (merged, mut bytes) = plan(Some(b"a\nb\nc\n"), Some(b"A\nb\nc\n"), Some(b"a\nb\nC\n"));
        assert!(!merged.conflict);
        let id = &merged.automatic.unwrap().file_id;
        let spool = bytes.get_mut(id).unwrap();
        spool.seek(SeekFrom::Start(0)).unwrap();
        let mut actual = Vec::new();
        spool.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, b"A\nb\nC\n");
        let (conflict, _) = plan(Some(b"base\n"), Some(b"local\n"), Some(b"remote\n"));
        assert!(conflict.conflict);
        assert!(conflict.automatic.is_none());
    }
    #[test]
    fn absence_empty_files_and_binary_values_are_not_conflated() {
        let (add, _) = plan(None, None, Some(b""));
        assert!(!add.conflict);
        assert!(add.automatic.is_some());
        let (delete, _) = plan(Some(b"base"), None, Some(b"base"));
        assert!(!delete.conflict);
        assert!(delete.automatic.is_none());
        let (modify_delete, _) = plan(Some(b"base"), Some(b"changed"), None);
        assert!(modify_delete.conflict);
        let (binary, _) = plan(Some(b"\0b"), Some(b"\0l"), Some(b"\0r"));
        assert!(binary.conflict);
    }
    #[test]
    fn oversized_divergent_values_require_whole_file_choices() {
        let base = vec![b'a'; 400_000];
        let local = vec![b'b'; 400_000];
        let remote = vec![b'c'; 400_000];
        let (conflict, _) = plan(Some(&base), Some(&local), Some(&remote));
        assert!(conflict.conflict);
    }
}
