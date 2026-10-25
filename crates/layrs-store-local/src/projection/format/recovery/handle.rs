use super::*;

/// Reviewed filesystem/metadata identity. Digests use `blake3:` followed by 64
/// lowercase hexadecimal digits; native identity strings are opaque.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryBinding {
    pub identity: ProjectionIdentity,
    pub checkout_identity: String,
    pub marker_digest: String,
    pub database_identity: String,
    pub family_marker_digest: Option<String>,
}
impl RecoveryBinding {
    pub(super) fn validate(&self) -> Result<()> {
        let digest = |s: &str| {
            s.strip_prefix("blake3:").is_some_and(|hex| {
                hex.len() == 64
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        };
        if !digest(&self.marker_digest)
            || self
                .family_marker_digest
                .as_deref()
                .is_some_and(|s| !digest(s))
            || [&self.checkout_identity, &self.database_identity]
                .iter()
                .any(|s| s.is_empty() || s.len() > 512)
        {
            return Err(invalid("invalid recovery binding encoding"));
        }
        validate_identity(&self.identity)
    }
    pub(super) fn from_parts(
        marker: &Marker,
        pin: &super::super::super::file::DatabasePin,
        family: Option<&shared::Lease>,
    ) -> Result<Self> {
        let mut digest = blake3::Hasher::new();
        digest.update(b"layrs-projection-recovery-marker-v1\0");
        digest.update(&marker.bytes()?);
        Ok(Self {
            identity: marker.identity.clone(),
            checkout_identity: marker.checkout_identity.clone(),
            marker_digest: format!("blake3:{}", digest.finalize()),
            database_identity: pin.identity().to_owned(),
            family_marker_digest: family
                .map(shared::Lease::recovery_marker_digest)
                .transpose()?,
        })
    }
}

/// Retains the reviewed marker and root pin. It grants only explicit recovery;
/// every method reacquires and validates the same private/family bindings.
pub struct ProjectionRecovery {
    store: ProjectionStore,
    binding: RecoveryBinding,
    caller: PinnedDirectory,
}
impl ProjectionRecovery {
    pub(super) fn open(root: &Path, expected: &RecoveryBinding) -> Result<Self> {
        inspect(root, false, Some(expected))?;
        let caller = PinnedDirectory::open(root)?;
        let marker = read_marker(root)?;
        let store = ProjectionStore {
            root: PathBuf::from(&marker.checkout_root),
            marker,
        };
        // No SQL/effects in construction after the readonly inspection. The
        // complete binding is checked again under the operation's owner lock.
        if caller.identity_token() != expected.checkout_identity
            || store.identity() != &expected.identity
        {
            return Err(invalid("recovery binding changed"));
        }
        Ok(Self {
            store,
            binding: expected.clone(),
            caller,
        })
    }
    fn access(&self, initializing: bool) -> Result<Access> {
        self.caller.verify_path_identity()?;
        access(&self.store, Some(&self.binding), initializing)
    }
    pub fn recover_databases(&self) -> Result<ProjectionInspection> {
        self.caller.verify_path_identity()?;
        inspect(&self.store.root, true, Some(&self.binding))
    }
    pub fn resume_initialization(&self) -> Result<ProjectionInspection> {
        let access = self.access(true)?;
        super::super::family::finish_initialization(&self.store, &access)?;
        drop(access);
        inspect(&self.store.root, false, Some(&self.binding))
    }
    pub fn resume_installation(&self) -> Result<VersionedProjection> {
        install::resume_with_access(&self.store, self.access(false)?)
    }
    pub fn resume_refresh(&self) -> Result<VersionedProjection> {
        refresh::resume_with_access(&self.store, self.access(false)?)
    }
    pub fn resume_namespace(&self) -> Result<CapturedPatch> {
        namespace::resume_with_access(&self.store, self.access(false)?)
    }
    pub fn resume_restore(&self) -> Result<RestoreReceipt> {
        restore::resume_with_access(&self.store, self.access(false)?)
    }
    pub fn resume_rebase(&self) -> Result<CapturedPatch> {
        rebase::resume_with_access(&self.store, self.access(false)?)
    }
    pub fn cancel_shared_reservation(&self) -> Result<()> {
        retention::cancel_with_access(&self.store, self.access(false)?)
    }
}

pub(in crate::projection::format) fn verify_binding(
    root: &Path,
    marker: &Marker,
    pin: &super::super::super::file::DatabasePin,
    family: Option<&shared::Lease>,
    expected: &RecoveryBinding,
) -> Result<()> {
    if read_marker(root)? != *marker
        || RecoveryBinding::from_parts(marker, pin, family)? != *expected
    {
        return Err(invalid("recovery binding changed"));
    }
    Ok(())
}

/// No migration, implicit SQLite rollback, or claim settlement. Validate SQL
/// readonly under the SAME exclusive owner lock before obtaining a writer.
pub(in crate::projection) fn access(
    store: &ProjectionStore,
    expected: Option<&RecoveryBinding>,
    initializing: bool,
) -> Result<Access> {
    if let Some(expected) = expected {
        expected.validate()?;
    }
    let family = shared::for_store_discovery(store)?;
    let path = store.marker.metadata_path(&store.root);
    let lock = StoreLock::acquire_existing(&path.join("layrs.lock"), SessionMode::Exclusive)
        .map_err(invalid)?;
    let metadata = PinnedDirectory::open(&path)?;
    let pin = super::super::super::file::DatabasePin::open(&path.join(DATABASE))?;
    let boundaries = preliminary_pins(&store.root, &store.marker, &metadata, &pin)?;
    if store
        .marker
        .shared
        .as_ref()
        .is_some_and(|b| b.database_identity != pin.identity())
    {
        return Err(invalid("shared Worktree database replaced"));
    }
    let actual = RecoveryBinding::from_parts(&store.marker, &pin, family.as_ref())?;
    verify_binding(
        &store.root,
        &store.marker,
        &pin,
        family.as_ref(),
        expected.unwrap_or(&actual),
    )?;
    if let Some(lease) = &family {
        let catalog = lease.inspection_catalog(false)?;
        let (bytes, state, registered_path, identity): (Vec<u8>, String, String, String) = catalog.connection.query_row(
            "SELECT marker,state,metadata_path,database_identity FROM worktrees WHERE id=?1 AND length(marker)<=65536",
            [store.identity().worktree_id.as_str()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        if bytes != store.marker.bytes()?
            || Path::new(&registered_path) != path
            || identity != pin.identity()
            || !(state == "ready" || (initializing && state == "initializing"))
        {
            return Err(invalid(
                "Worktree recovery registration differs or is not ready",
            ));
        }
    }
    let validate = |c: &rusqlite::Connection| -> Result<()> {
        database::validate(c, &store.marker)?;
        if store.marker.import_digest.is_some() {
            if initializing {
                return Err(invalid("conversion requires source-preserving recovery"));
            }
            if !c.query_row(
                "SELECT ready FROM projection_conversion WHERE singleton=1",
                [],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(invalid("conversion pending; source is retained"));
            }
            conversion::provenance(c, store)?;
        }
        Ok(())
    };
    let read = database::connect(&path.join(DATABASE), false)?;
    validate(&read)?;
    drop(read);
    verify_boundaries(&store.root, &store.marker, &metadata, &pin)?;
    verify_binding(&store.root, &store.marker, &pin, family.as_ref(), &actual)?;
    let connection = database::connect(&path.join(DATABASE), true)?;
    validate(&connection)?;
    let access = Access {
        connection,
        metadata,
        database: pin,
        root: store.root.clone(),
        marker: store.marker.clone(),
        recovery_binding: Some(actual),
        _boundaries: boundaries,
        _lock: lock,
        family,
    };
    access.verify()?;
    Ok(access)
}
