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
    admission.require_usable()?;
    require_filename(c, admission.inner.database.path())?;
    if marker != admission.data().marker() {
        return Err(invalid(
            "admitted private connection FORMAT differs from preflight",
        ));
    }
    if !c.is_autocommit() {
        return validate_active(admission, c, marker);
    }
    c.execute_batch("BEGIN DEFERRED")?;
    let result = validate_active(admission, c, marker);
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
