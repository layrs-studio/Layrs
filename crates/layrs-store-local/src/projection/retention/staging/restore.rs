//! Restore-only bounded DATA and private, non-creating checkout cleanup.
use super::super::cancellation::{canonical_number, restore_id, RestoreCancellationPoint};
use super::*;

#[derive(Debug, PartialEq, Eq)]
pub(in crate::projection::retention) struct RestoreStage {
    plan: OwnedStage,
    state: String,
}

pub(in crate::projection::retention) fn read_restore(
    c: &Connection,
    operation: Option<&str>,
) -> Result<Option<RestoreStage>> {
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_prejournal LIMIT 2)",
        [],
        |r| r.get(0),
    )?;
    if count == 0 {
        return Ok(None);
    }
    let operation = operation.ok_or_else(|| invalid("checkout staging has no restore budget"))?;
    if count != 1 || c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_prejournal WHERE length(CAST(path AS BLOB))=0 OR length(CAST(path AS BLOB))>256 OR length(CAST(operation AS BLOB))=0 OR length(CAST(operation AS BLOB))>128 OR length(CAST(digest AS BLOB))!=64 OR length(payload)=0 OR length(payload)>32768 OR state NOT IN ('owned','cleaning'))",
        [], |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("restore checkout staging is ambiguous or exceeds bounds"));
    }
    let (path, actual_operation, digest, payload, state): (
        String,
        String,
        String,
        Vec<u8>,
        String,
    ) = c.query_row(
        "SELECT path,operation,digest,payload,state FROM projection_prejournal",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    let plan: OwnedStage = serde_json::from_slice(&payload).map_err(invalid)?;
    validate(&plan)?;
    let id = restore_id(operation)?;
    let prefix = format!(".layrs/projection-quarantine/restore-{id}-");
    let timestamp = path
        .strip_prefix(&prefix)
        .ok_or_else(|| invalid("restore checkout staging path differs from budget"))?;
    let files = plan
        .files
        .as_ref()
        .ok_or_else(|| invalid("restore checkout staging must be a directory"))?;
    if !canonical_number::<u128>(timestamp)
        || actual_operation != operation
        || plan.operation != operation
        || plan.path != path
        || shared::digest(&plan)? != digest
        || layrs_core::scm::canonical_json(&plan).map_err(invalid)? != payload
        || files.len() > 1024
        || !valid_identity(&plan.identity)
    {
        return Err(invalid("restore checkout staging proof differs"));
    }
    let mut previous = None;
    for name in files {
        let index = name
            .strip_prefix("new-")
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|n| *n < 1024 && format!("new-{n:04}") == *name)
            .ok_or_else(|| invalid("restore checkout staging filename invalid"))?;
        if previous.is_some_and(|n| n >= index) {
            return Err(invalid("restore checkout staging filename order invalid"));
        }
        previous = Some(index);
    }
    Ok(Some(RestoreStage { plan, state }))
}

fn valid_identity(value: &str) -> bool {
    let Some(raw) = value.strip_prefix("fs:v1:") else {
        return false;
    };
    let Some((volume, file)) = raw.split_once(':') else {
        return false;
    };
    [volume, file].iter().all(|v| {
        v.len() == 16
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

impl RestoreStage {
    fn parent(&self, store: &ProjectionStore, writable: bool) -> Result<PinnedDirectory> {
        let root = PinnedDirectory::open(&store.root)?;
        let metadata = root.open_child_for_traversal(".layrs".as_ref())?;
        let parent = if writable {
            metadata.open_private_child_for_write("projection-quarantine".as_ref())?
        } else {
            metadata.open_private_child("projection-quarantine".as_ref())?
        };
        root.verify_path_identity()?;
        Ok(parent)
    }

    fn directory(
        &self,
        parent: &PinnedDirectory,
        writable: bool,
    ) -> Result<Option<PinnedDirectory>> {
        let name = self.plan.path.rsplit('/').next().unwrap();
        let result = if writable {
            parent.open_private_child_for_write(name.as_ref())
        } else {
            parent.open_private_child(name.as_ref())
        };
        match result {
            Ok(directory) => {
                if directory.identity_token() != self.plan.identity {
                    return Err(invalid("restore checkout staging directory replaced"));
                }
                Ok(Some(directory))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound && self.state == "cleaning" => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn inventory(
        &self,
        store: &ProjectionStore,
        directory: &PinnedDirectory,
    ) -> Result<Vec<std::ffi::OsString>> {
        let files = self.plan.files.as_ref().unwrap();
        let mut actual = Vec::new();
        let mut bytes = 0u64;
        for entry in fs::read_dir(store.root.join(&self.plan.path))?.take(files.len() + 1) {
            let name = entry?.file_name();
            if !files
                .iter()
                .any(|allowed| name == std::ffi::OsStr::new(allowed))
            {
                return Err(invalid("unexpected entry in restore checkout staging"));
            }
            let file = directory.open_readonly_file(&name)?;
            super::super::super::file::require_single_link(file.file())?;
            let size = file.file().metadata()?.len();
            bytes = bytes
                .checked_add(size)
                .filter(|total| size <= MAX_CAPTURE_FILE_BYTES && *total <= MAX_CAPTURE_BYTES)
                .ok_or_else(|| invalid("restore checkout staging size exceeds bound"))?;
            file.verify_parent()?;
            actual.push(name);
        }
        directory.verify_path_identity()?;
        actual.sort();
        Ok(actual)
    }

    pub(in crate::projection::retention) fn validate_physical(
        &self,
        store: &ProjectionStore,
    ) -> Result<()> {
        let parent = self.parent(store, false)?;
        if let Some(directory) = self.directory(&parent, false)? {
            self.inventory(store, &directory)?;
            self.directory(&parent, false)?;
        }
        parent.verify_path_identity()?;
        Ok(())
    }

    pub(in crate::projection::retention) fn cancel(
        &self,
        c: &Connection,
        store: &ProjectionStore,
        faults: &dyn Fn(RestoreCancellationPoint) -> Result<()>,
    ) -> Result<()> {
        self.validate_physical(store)?;
        let parent = self.parent(store, true)?;
        if let Some(directory) = self.directory(&parent, true)? {
            let actual = self.inventory(store, &directory)?;
            // Open and validate the complete deletion set before the first
            // unlink. The durable record owns names inside this private inode.
            let mut files = Vec::new();
            for name in actual {
                let file = directory.open_file(&name)?;
                super::super::super::file::require_single_link(file.file())?;
                files.push(file);
            }
            if c.execute(
                "UPDATE projection_prejournal SET state='cleaning' WHERE path=?1 AND operation=?2",
                params![self.plan.path, self.plan.operation],
            )? != 1
            {
                return Err(invalid(
                    "restore checkout staging ownership changed before cleanup",
                ));
            }
            faults(RestoreCancellationPoint::StagingMarkedCleaning)?;
            for file in files {
                let fresh_parent = self.parent(store, false)?;
                if fresh_parent.identity_token() != parent.identity_token() {
                    return Err(invalid("restore checkout staging parent replaced"));
                }
                self.directory(&parent, false)?;
                super::super::super::file::require_single_link(file.file())?;
                file.verify_parent()?;
                file.unlink()?;
                faults(RestoreCancellationPoint::StagingFileRemoved)?;
            }
            directory.sync()?;
            self.directory(&parent, false)?;
            drop(directory);
            parent.remove_empty_child(self.plan.path.rsplit('/').next().unwrap().as_ref())?;
            faults(RestoreCancellationPoint::StagingDirectoryRemoved)?;
        }
        parent.verify_path_identity()?;
        if c.execute(
            "DELETE FROM projection_prejournal WHERE path=?1 AND operation=?2",
            params![self.plan.path, self.plan.operation],
        )? != 1
        {
            return Err(invalid(
                "restore checkout staging ownership changed before retirement",
            ));
        }
        Ok(())
    }
}
