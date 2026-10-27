use super::*;

/// SQLite's win32 VFS can omit the verbatim prefix or use slash separators.
/// Do not case-fold: NTFS may have case-sensitive directories. Do not canonicalize
/// a nominated filename by following aliases or additional filesystem paths.
pub(super) fn same_native_filename(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        path.to_str().map(|text| {
            text.strip_prefix("\\\\?\\")
                .unwrap_or(text)
                .replace('/', "\\")
        })
    };
    match (normalize(left), normalize(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

pub(super) fn require_filename(c: &Connection, expected: &Path) -> Result<()> {
    if c.path()
        .is_none_or(|filename| !same_native_filename(Path::new(filename), expected))
    {
        return Err(invalid(
            "admitted private connection main filename differs from retained database",
        ));
    }
    Ok(())
}

pub(super) fn validate(
    admission: &DurableAdmission,
    c: &Connection,
    marker: &format::Marker,
) -> Result<()> {
    validate_with(admission, c, marker, |_| Ok(()))
}

/// Reinterpret immutable admission before running a read-only DATA callback
/// in the same existing or owned snapshot. Only an owned snapshot ends here;
/// failed explicit rollback poisons every clone of the durable authority.
pub(super) fn validate_with<T>(
    admission: &DurableAdmission,
    c: &Connection,
    marker: &format::Marker,
    callback: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    admission.require_usable()?;
    require_filename(c, admission.inner.database.path())?;
    if marker != admission.data().marker() {
        return Err(invalid(
            "admitted private connection FORMAT differs from preflight",
        ));
    }
    let active = |c: &Connection| {
        validate_active(admission, c, marker)?;
        let value = callback(c)?;
        if c.is_autocommit() {
            return Err(invalid(
                "admitted validation callback ended its read transaction",
            ));
        }
        admission.inner.database.verify()?;
        Ok(value)
    };
    if !c.is_autocommit() {
        return active(c);
    }
    c.execute_batch("BEGIN DEFERRED")?;
    let result = active(c);
    let rollback = c.execute_batch("ROLLBACK").map_err(Cause::from);
    if rollback.is_err() {
        admission
            .inner
            .rollback_failed
            .store(true, Ordering::Release);
    }
    conclude(result, rollback)
}

fn validate_active(
    admission: &DurableAdmission,
    c: &Connection,
    marker: &format::Marker,
) -> Result<()> {
    let fresh = admitted_ordinary::inspect(c, marker, None)?;
    admission.data().require_same_import(&fresh)?;
    admission.inner.database.verify()
}
