//! Exact ownership of checkout-side files before a filesystem journal accepts them.
use super::*;
use crate::secure_fs::PinnedDirectory;
use std::{fs, path::Path};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OwnedStage {
    operation: String,
    path: String,
    identity: String,
    files: Option<Vec<String>>,
}
pub(in crate::projection) fn require_no_staging(
    c: &Connection,
    store: &ProjectionStore,
) -> Result<()> {
    if store.is_shared()
        && c.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_prejournal)",
            [],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Err(invalid(
            "private staging pending; cancel_shared_reservation before a new attempt",
        ));
    }
    Ok(())
}

pub(in crate::projection) fn track_directory(
    c: &Connection,
    store: &ProjectionStore,
    operation: &str,
    path: &str,
    directory: &PinnedDirectory,
    files: Vec<String>,
) -> Result<()> {
    track(
        c,
        store,
        OwnedStage {
            operation: operation.into(),
            path: path.into(),
            identity: directory.identity_token(),
            files: Some(files),
        },
    )
}
pub(in crate::projection) fn track_file(
    c: &Connection,
    store: &ProjectionStore,
    operation: &str,
    path: &str,
    file: &PinnedFile,
) -> Result<()> {
    track(
        c,
        store,
        OwnedStage {
            operation: operation.into(),
            path: path.into(),
            identity: super::super::file::identity(file.file())?,
            files: None,
        },
    )
}
fn validate(value: &OwnedStage) -> Result<()> {
    let parts = value.path.split('/').collect::<Vec<_>>();
    if parts.len() != 3
        || parts[0] != ".layrs"
        || !matches!(
            parts[1],
            "namespace-staging" | "projection-staging" | "projection-quarantine"
        )
        || parts[2].is_empty()
        || parts[2].len() > 200
        || !parts[2]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || value.identity.is_empty()
    {
        return Err(invalid("prejournal staging path or identity invalid"));
    }
    if let Some(files) = &value.files {
        if files.len() > 10000 {
            return Err(invalid("prejournal staging file bound exceeded"));
        }
        let mut seen = BTreeSet::new();
        for name in files {
            if name.is_empty()
                || name.len() > 128
                || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || !seen.insert(name)
            {
                return Err(invalid("prejournal exact filename set invalid"));
            }
        }
    }
    Ok(())
}
fn track(c: &Connection, store: &ProjectionStore, value: OwnedStage) -> Result<()> {
    if !store.is_shared() {
        return Ok(());
    }
    validate(&value)?;
    let payload = layrs_core::scm::canonical_json(&value).map_err(invalid)?;
    c.execute(
        "INSERT INTO projection_prejournal VALUES(?1,?2,?3,?4,'owned')",
        params![
            value.path,
            value.operation,
            shared::digest(&value)?,
            payload
        ],
    )?;
    Ok(())
}
pub(in crate::projection) fn accept_staging(
    c: &Connection,
    store: &ProjectionStore,
    operation: &str,
) -> Result<()> {
    if store.is_shared() {
        c.execute(
            "DELETE FROM projection_prejournal WHERE operation=?1",
            [operation],
        )?;
    }
    Ok(())
}
pub(super) fn cancel(c: &Connection, store: &ProjectionStore) -> Result<()> {
    let mut q=c.prepare("SELECT path,operation,digest,payload,state FROM projection_prejournal ORDER BY path LIMIT 1025")?;
    let rows = q
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Vec<u8>>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(q);
    if rows.len() > 1024 {
        return Err(invalid("too many prejournal staging records; retained"));
    }
    let root = PinnedDirectory::open(&store.root)?;
    for (path, operation, digest, payload, state) in rows {
        let plan: OwnedStage = serde_json::from_slice(&payload).map_err(invalid)?;
        validate(&plan)?;
        if plan.path != path
            || plan.operation != operation
            || shared::digest(&plan)? != digest
            || layrs_core::scm::canonical_json(&plan).map_err(invalid)? != payload
        {
            return Err(invalid("prejournal staging proof mismatch"));
        }
        let (parent, name) = root.existing_parent(Path::new(&path))?;
        let missing = if plan.files.is_some() {
            match parent.open_child(&name) {
                Ok(pin) => {
                    if pin.identity_token() != plan.identity {
                        return Err(invalid("prejournal directory replaced; retained"));
                    }
                    false
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => true,
                Err(e) => return Err(e.into()),
            }
        } else {
            match parent.open_file(&name) {
                Ok(pin) => {
                    if super::super::file::identity(pin.file())? != plan.identity {
                        return Err(invalid("prejournal file replaced; retained"));
                    }
                    false
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => true,
                Err(e) => return Err(e.into()),
            }
        };
        if missing && state != "cleaning" {
            return Err(invalid("owned prejournal staging disappeared; retained"));
        }
        if !missing {
            if let Some(files) = &plan.files {
                let directory = parent.open_child(&name)?;
                if directory.identity_token() != plan.identity {
                    return Err(invalid(
                        "prejournal directory replaced during cleanup; retained",
                    ));
                }
                let mut actual = Vec::new();
                for item in fs::read_dir(store.root.join(&path))?.take(files.len() + 1) {
                    let file = item?.file_name();
                    if !files.iter().any(|n| file == std::ffi::OsStr::new(n)) {
                        return Err(invalid("unexpected entry in owned staging; retained"));
                    }
                    directory.open_file(&file)?;
                    actual.push(file);
                }
                directory.verify_path_identity()?;
                c.execute(
                    "UPDATE projection_prejournal SET state='cleaning' WHERE path=?1",
                    [&path],
                )?;
                for file in actual {
                    directory.open_file(&file)?.unlink()?;
                }
                directory.sync()?;
                drop(directory);
                parent.remove_empty_child(&name)?;
            } else {
                let file = parent.open_file(&name)?;
                if super::super::file::identity(file.file())? != plan.identity {
                    return Err(invalid("prejournal file replaced during cleanup; retained"));
                }
                c.execute(
                    "UPDATE projection_prejournal SET state='cleaning' WHERE path=?1",
                    [&path],
                )?;
                file.verify_parent()?;
                file.unlink()?;
                parent.sync()?;
            }
        }
        c.execute("DELETE FROM projection_prejournal WHERE path=?1", [path])?;
    }
    root.verify_path_identity()?;
    Ok(())
}
