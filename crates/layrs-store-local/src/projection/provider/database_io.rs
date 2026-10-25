//! Fixed-sidecar admission and one-attempt rollback under caller-owned gates.
mod bare;
mod sidecars;
use super::{
    journal,
    recovery::error::{ProviderBootstrapCause as Cause, Result, invalid},
};
pub(super) use bare::{BareConnection, WriterCloseObserver};
#[cfg(test)]
pub(super) use bare::{WriterOpenPoint, writer_close_result};
use rusqlite::{Connection, ffi};
pub(in crate::projection) use sidecars::DatabaseFile;
use std::sync::Arc;

pub(super) enum ReadState<T> {
    Uninitialized,
    Valid(T),
    Hot(HotJournal),
    /// A known semantic contradiction. Whether the first pager read succeeded
    /// remains separate evidence for physical rollback completion.
    Rejected {
        cause: Cause,
        pager_read: bool,
    },
    /// Pager read and close succeeded, but validation could not finish (for
    /// example BUSY/IO/profile drift). This is not contradictory SQL evidence.
    ReadButUnavailable(Cause),
    /// An actual RO rollback error was observed, but its concluding filesystem
    /// checks could not establish admission. Keep both causes; no Hot capability.
    UnboundRollback {
        trigger: Arc<rusqlite::Error>,
        cause: Cause,
    },
    Unavailable(Cause),
}
pub(super) struct HotJournal {
    pub(super) trigger: Arc<rusqlite::Error>,
    snapshot: sidecars::Snapshot,
    geometry: journal::ReplayGeometry,
}

/// Both DBs are probed by the orchestrator even when one is unavailable. An
/// observable close failure is the exception: stop all subsequent probing.
pub(super) fn read<T>(
    database: &DatabaseFile,
    validate: impl FnOnce(&Connection) -> Result<T>,
) -> Result<ReadState<T>> {
    let before = match database.snapshot() {
        Ok(v) => v,
        Err(e) => return Ok(ReadState::Unavailable(e)),
    };
    if before.empty() {
        return Ok(ReadState::Uninitialized);
    }
    let bare = match BareConnection::open(database.path(), false) {
        Ok(v) => v,
        Err(e @ Cause::Cleanup { .. }) => return Err(e),
        Err(e) => return Ok(ReadState::Unavailable(e)),
    };
    let geometry = bare
        .geometry
        .ok_or_else(|| invalid("database replay geometry absent"));
    let mut pager_read = false;
    let result = geometry.and_then(|geometry| {
        bare.connection().execute_batch("BEGIN DEFERRED")?;
        // First pager-forcing SQL; no setup PRAGMA can run before geometry.
        let _: u64 =
            bare.connection()
                .query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))?;
        pager_read = true;
        Ok((validate(bare.connection())?, geometry))
    });
    let mut observed_trigger = None;
    let result = match result {
        Err(Cause::Sqlite(trigger)) if is_rollback(&trigger) => {
            let trigger = Arc::new(trigger);
            observed_trigger = Some(Arc::clone(&trigger));
            Err(Cause::ReadonlyRollback(trigger))
        }
        other => other,
    };
    let geometry = bare.geometry;
    let result = bare.finish(result);
    let result = match result {
        Err(e @ Cause::Cleanup { .. }) => return Err(e),
        result => result,
    };
    let after = match database.snapshot() {
        Ok(v) => v,
        Err(e) => return Ok(unavailable_after_probe(e, observed_trigger)),
    };
    if before != after {
        return Ok(unavailable_after_probe(
            invalid("database or sidecars changed during read-only probe"),
            observed_trigger,
        ));
    }
    match result {
        Ok((value, _)) => Ok(ReadState::Valid(value)),
        Err(Cause::ReadonlyRollback(trigger)) => match geometry {
            Some(geometry) if !pager_read && after.journal.is_some() => {
                Ok(ReadState::Hot(HotJournal {
                    trigger,
                    snapshot: after,
                    geometry,
                }))
            }
            _ => Ok(ReadState::UnboundRollback {
                trigger,
                cause: invalid("read-only rollback trigger lacks initial pager/journal binding"),
            }),
        },
        Err(e) => Ok(classify_sql_failure(e, pager_read)),
    }
}

fn unavailable_after_probe<T>(cause: Cause, trigger: Option<Arc<rusqlite::Error>>) -> ReadState<T> {
    match trigger {
        Some(trigger) => ReadState::UnboundRollback { trigger, cause },
        None => ReadState::Unavailable(cause),
    }
}

fn classify_sql_failure<T>(cause: Cause, pager_read: bool) -> ReadState<T> {
    if semantic_contradiction(&cause) {
        ReadState::Rejected { cause, pager_read }
    } else if pager_read {
        ReadState::ReadButUnavailable(cause)
    } else {
        ReadState::Unavailable(cause)
    }
}
#[cfg(test)]
fn validation_failure<T>(cause: Cause) -> ReadState<T> {
    classify_sql_failure(cause, true)
}

fn semantic_contradiction(cause: &Cause) -> bool {
    match cause {
        Cause::Validation(_) => true,
        Cause::Sqlite(error) => {
            matches!(
                error,
                rusqlite::Error::QueryReturnedNoRows
                    | rusqlite::Error::InvalidColumnType(..)
                    | rusqlite::Error::FromSqlConversionFailure(..)
                    | rusqlite::Error::IntegralValueOutOfRange(..)
            ) || matches!(error,
            rusqlite::Error::SqliteFailure(code, _) if matches!(code.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase))
        }
        _ => false,
    }
}

fn is_rollback(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(code, _) if code.extended_code == ffi::SQLITE_READONLY_ROLLBACK)
}

pub(super) struct AdmittedRollback {
    hot: HotJournal,
    _admission: journal::JournalAdmission,
}
pub(super) struct AdmissionFailure {
    pub(super) trigger: Arc<rusqlite::Error>,
    pub(super) cause: Cause,
}

pub(super) fn admit(
    database: &DatabaseFile,
    hot: HotJournal,
) -> std::result::Result<AdmittedRollback, AdmissionFailure> {
    let checked = (|| -> Result<_> {
        if database.snapshot()? != hot.snapshot {
            return Err(invalid("hot-journal binding changed before admission"));
        }
        let expected = hot
            .snapshot
            .journal
            .as_ref()
            .ok_or_else(|| invalid("hot journal absent"))?;
        let mut reader = crate::layout::open_read_no_follow(&database.sidecar_path("-journal"))?;
        if crate::projection::file::identity(&reader)? != expected.identity
            || reader.metadata()?.len() != expected.bytes
        {
            return Err(invalid("journal replaced before admission"));
        }
        let header = hot.snapshot.database_evidence()?;
        let admission = journal::inspect_journal(
            &mut reader,
            expected.bytes,
            header,
            hot.geometry,
            journal::JournalLimits::default(),
        )?;
        if crate::projection::file::identity(&reader)? != expected.identity
            || reader.metadata()?.len() != expected.bytes
        {
            return Err(invalid("journal changed during admission"));
        }
        drop(reader); // NONDELETE, but still close every reader before SQLite.
        if database.snapshot()? != hot.snapshot {
            return Err(invalid("hot-journal binding changed after admission"));
        }
        Ok(admission)
    })();
    match checked {
        Ok(admission) => Ok(AdmittedRollback {
            hot,
            _admission: admission,
        }),
        Err(cause) => Err(AdmissionFailure {
            trigger: hot.trigger,
            cause,
        }),
    }
}

/// Caller has rechecked terminal/expected bindings and recorded Uncertain BEFORE
/// entering. A bare writable open already has possible effects even when later
/// geometry validation refuses before any SQL.
pub(super) fn rollback(
    database: &DatabaseFile,
    admitted: AdmittedRollback,
) -> std::result::Result<Arc<rusqlite::Error>, AdmissionFailure> {
    let result = (|| -> Result<()> {
        if database.snapshot()? != admitted.hot.snapshot {
            return Err(invalid("database changed before writable attempt"));
        }
        let bare = BareConnection::open(database.path(), true)?;
        let result = if bare.geometry != Some(admitted.hot.geometry) {
            Err(invalid(
                "read-only/writable SQLite replay geometry differs before SQL",
            ))
        } else {
            // Only this read is permitted: no migration, SQL write or mode PRAGMA.
            bare.connection()
                .query_row("SELECT count(*) FROM sqlite_schema", [], |r| {
                    r.get::<_, u64>(0)
                })
                .map(|_| ())
                .map_err(Cause::from)
        };
        bare.finish(result)?;
        let after = database.snapshot()?;
        if after.empty() || after.journal.is_some() {
            return Err(invalid(
                "rollback completion requires nonzero database and absent journal",
            ));
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(admitted.hot.trigger),
        Err(cause) => Err(AdmissionFailure {
            trigger: admitted.hot.trigger,
            cause,
        }),
    }
}

/// Literal, ASCII-case-insensitive internal prefix: SQLite3.46.0
/// sqlite3CheckObjectName uses sqlite3StrNICmp(name,"sqlite_",7), not LIKE.
/// Bound metadata before allocating strings and use this same inventory for
/// private, family and fresh reference schemas. Ordinary schema helpers stay unchanged.
pub(in crate::projection) fn schema_inventory(
    c: &Connection,
) -> Result<Vec<(String, String, String)>> {
    const FILTER: &str = "substr(name,1,7) COLLATE NOCASE <> 'sqlite_'";
    let (count,bytes,names):(u64,u64,u64)=c.query_row(
        &format!("SELECT count(*),coalesce(sum(length(CAST(sql AS BLOB))),0),coalesce(sum(length(CAST(type AS BLOB))+length(CAST(name AS BLOB))),0) FROM sqlite_schema WHERE {FILTER}"),
        [],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if count > 64 || bytes > 64 * 1024 || names > 64 * 1024 {
        return Err(invalid("provider schema exceeds validation budget"));
    }
    let mut statement=c.prepare(&format!("SELECT type,name,sql FROM sqlite_schema WHERE {FILTER} ORDER BY type COLLATE BINARY,name COLLATE BINARY"))?;
    Ok(statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Profile-gated schema reference; statements affect only this fresh memory DB.
/// Ordinary database::validate_schema_identity is intentionally not used: its
/// implicit reference opener has a different process/admission contract.
pub(in crate::projection) fn with_reference<T>(
    ddl: &str,
    action: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    let bare = BareConnection::reference()?;
    let result = (|| {
        bare.connection().execute_batch(ddl)?;
        action(bare.connection())
    })();
    bare.finish(result)
}

#[cfg(test)]
pub(super) mod test_hooks;
#[cfg(test)]
pub(super) mod tests;

pub(super) fn validate_writer_filename(path: &std::path::Path) -> Result<()> {
    bare::native_filename(path).map(|_| ())
}

/// A clean writer admission is operation-local and only constructed under the
/// caller's continuously held gates. Fresh zero-DB admission is initializer-only.
pub(super) struct CleanWriteAdmission {
    snapshot: sidecars::Snapshot,
    geometry: Option<journal::ReplayGeometry>,
}
pub(super) fn clean_admission(
    database: &DatabaseFile,
    fresh: bool,
    validate: impl FnOnce(&Connection) -> Result<()>,
) -> Result<CleanWriteAdmission> {
    let before = database.snapshot()?;
    if before.empty() {
        if !fresh {
            return Err(invalid(
                "clean provider writer refuses uninitialized database",
            ));
        }
        return Ok(CleanWriteAdmission {
            snapshot: before,
            geometry: None,
        });
    }
    if fresh {
        return Err(invalid(
            "fresh provider initializer requires exact zero-byte database",
        ));
    }
    let bare = BareConnection::open(database.path(), false)?;
    let geometry = bare.geometry;
    let result = (|| {
        if geometry.is_none() {
            return Err(invalid("clean writer read-only geometry absent"));
        }
        bare.connection().execute_batch("BEGIN DEFERRED")?;
        let _: u64 =
            bare.connection()
                .query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))?;
        #[cfg(test)]
        tests::writer_support::admission(
            database.path(),
            tests::writer_support::AdmissionPoint::ReadonlyActive,
        )?;
        validate(bare.connection())
    })();
    let result = match result {
        Err(Cause::Sqlite(e)) if is_rollback(&e) => Err(Cause::ReadonlyRollback(Arc::new(e))),
        other => other,
    };
    bare.finish(result)?; // A close failure prevents the following snapshot.
    #[cfg(test)]
    tests::writer_support::admission(
        database.path(),
        tests::writer_support::AdmissionPoint::ReadonlyClosed,
    )?;
    if database.snapshot()? != before {
        return Err(invalid("clean writer read-only database/sidecars changed"));
    }
    // A supported journal first receives the real locked RO probe. Hot errors
    // above retain their original SQLite cause; even a nonhot journal cannot
    // cross the clean-writer boundary. No recovery or sidecar deletion occurs.
    if before.journal.is_some() {
        return Err(invalid(
            "clean provider writer refuses any rollback journal",
        ));
    }
    Ok(CleanWriteAdmission {
        snapshot: before,
        geometry,
    })
}
impl CleanWriteAdmission {
    pub(super) fn open(
        &self,
        database: &DatabaseFile,
        observed: &mut impl WriterCloseObserver,
    ) -> Result<BareConnection> {
        if database.snapshot()? != self.snapshot {
            return Err(invalid(
                "clean writer admission changed before writable open",
            ));
        }
        #[cfg(test)]
        tests::writer_support::admission(
            database.path(),
            tests::writer_support::AdmissionPoint::BeforeWritableOpen,
        )?;
        BareConnection::open_writer(database.path(), observed)
    }
    pub(super) fn require_geometry(&self, bare: &BareConnection) -> Result<()> {
        if bare.geometry.is_none() || self.geometry.is_some_and(|g| bare.geometry != Some(g)) {
            return Err(invalid(
                "clean writer read-only/writable geometry differs before SQL",
            ));
        }
        Ok(())
    }
}
