//! Owner-scoped admitted family access. Construction happens only after the
//! physical caller retains owner/lifecycle/private pins and real schema9 DATA.
//! No owner lock or private database is opened beneath this context.
use super::{admitted_catalog::AdmittedFamilyContext, provider_access::FamilyAccess, *};
use crate::projection::provider::{
    admitted_catalog_io,
    admitted_ordinary::AdmittedOrdinaryData,
    recovery::error::{
        ProviderBootstrapCause as Cause, Result as CauseResult, invalid as rejected,
    },
};

pub(super) struct AdmittedContext {
    family: FamilyAccess,
    provenance: AdmittedFamilyContext,
    admission_record: Vec<u8>,
}

fn store(cause: Cause) -> ProjectionStoreError {
    ProjectionStoreError::ProviderContent(Box::new(cause))
}
fn cause(error: ProjectionStoreError) -> Cause {
    match error {
        ProjectionStoreError::Io(error) => Cause::Io(error),
        ProjectionStoreError::Database(error) => Cause::Sqlite(error),
        ProjectionStoreError::ProviderContent(error) => *error,
        other => Cause::Validation(Box::new(other)),
    }
}
fn conclude<T>(result: CauseResult<T>, verify: CauseResult<()>) -> CauseResult<T> {
    match (result, verify) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Ok(())) => Err(error),
        (Err(primary), Err(close)) => Err(Cause::Cleanup {
            primary: Box::new(primary),
            close: Box::new(close),
        }),
    }
}
fn reserved(operation: &str) -> bool {
    let operation = operation.to_ascii_lowercase();
    operation.starts_with("provider-transition") || operation.starts_with("provider-import")
}

impl AdmittedContext {
    pub(super) fn new(family: &PartialStoreFamily, data: &AdmittedOrdinaryData) -> Result<Self> {
        family.verify()?;
        let shared = data
            .marker()
            .shared
            .as_ref()
            .ok_or_else(|| store(rejected("admitted content requires shared FORMAT")))?;
        let binding = data.binding();
        let identity = &data.marker().identity;
        if shared.root != family.root
            || shared.incarnation != family.marker.incarnation
            || shared.root_identity != family.marker.root_identity
            || shared.metadata_path != family.member_metadata(&identity.worktree_id)
            || shared.database_identity != binding.database_identity
            || identity.worktree_id != binding.owner
            || identity.store_id != family.marker.identity.store_id
            || identity.space_id != family.marker.identity.space_id
            || identity.origin != family.marker.identity.origin
            || identity.principal_id != family.marker.identity.principal_id
            || binding.family_incarnation != family.marker.incarnation
            || binding.family_root_identity != family.marker.root_identity
            || binding.family_catalog_identity != family.marker.database_identity
        {
            return Err(store(rejected(
                "admitted content differs from retained family binding",
            )));
        }
        let pins = FamilyAccess::open(&family.root).map_err(store)?;
        if pins.marker_bytes() != canonical_json(&family.marker).map_err(invalid)?
            || pins.marker_digest() != binding.family_marker_digest
        {
            return Err(store(rejected(
                "admitted content differs from pinned family marker",
            )));
        }
        let context = Self {
            family: pins,
            provenance: AdmittedFamilyContext::new(data).map_err(store)?,
            admission_record: data.admission_record_bytes().to_vec(),
        };
        context.require_same_admission(data)?;
        context.with_catalog(false, |_| Ok(()))?;
        Ok(context)
    }

    pub(super) fn verify(&self) -> Result<()> {
        self.family.verify().map_err(store)
    }

    pub(super) fn require_same_admission(&self, data: &AdmittedOrdinaryData) -> Result<()> {
        self.verify()?;
        if self.provenance.marker() != data.marker()
            || self.provenance.binding() != data.binding()
            || self.provenance.admission_digest() != data.admission_digest()
            || self.admission_record != data.admission_record_bytes()
        {
            return Err(store(rejected(
                "admitted content immutable admission changed",
            )));
        }
        Ok(())
    }

    fn validate(&self, c: &Connection) -> CauseResult<()> {
        self.family.verify()?;
        self.provenance
            .validate_catalog(c, self.family.marker_bytes(), None)?;
        self.family.verify()
    }

    pub(super) fn with_catalog<T>(
        &self,
        write: bool,
        operation: impl FnOnce(&mut Connection) -> Result<T>,
    ) -> Result<T> {
        self.verify()?;
        let gate = StoreLock::acquire_existing(
            &self.family.root().join("metadata/catalog.lock"),
            if write {
                SessionMode::Exclusive
            } else {
                SessionMode::Shared
            },
        )
        .map_err(|error| store(Cause::Store(error)))?;
        self.family.verify().map_err(store)?;
        let result = admitted_catalog_io::with_catalog(
            &self.family.catalog,
            write,
            |c| self.validate(c),
            |c| operation(c).map_err(cause),
        );
        let result = conclude(result, self.family.verify());
        drop(gate);
        result.map_err(store)
    }

    pub(super) fn authorize_owner(&self, owner: &WorktreeId) -> Result<()> {
        if owner != &self.provenance.binding().owner {
            return Err(store(rejected("admitted ordinary content owner differs")));
        }
        Ok(())
    }

    pub(super) fn authorize_reservation_request(
        &self,
        id: &str,
        owner: &WorktreeId,
        operation: &str,
    ) -> Result<()> {
        self.authorize_owner(owner)?;
        if id.is_empty()
            || id.len() > 128
            || id.contains('\0')
            || operation.is_empty()
            || operation.len() > 128
            || operation.contains('\0')
            || reserved(id)
            || reserved(operation)
        {
            return Err(store(rejected(
                "admitted ordinary reservation targets retained import",
            )));
        }
        Ok(())
    }

    /// Check before any catalog or filesystem effect, including Stage::begin.
    /// Absence is allowed for idempotent release; a staging caller still checks
    /// its own required row after this owner/retained-import fence.
    pub(super) fn authorize_reservation(&self, c: &Connection, id: &str) -> Result<()> {
        if id.is_empty()
            || id.len() > 128
            || id.contains('\0')
            || reserved(id)
            || c.query_row(
                "SELECT EXISTS(SELECT 1 FROM exact_installations WHERE reservation=?1)",
                [id],
                |r| r.get::<_, bool>(0),
            )?
        {
            return Err(store(rejected(
                "admitted ordinary content targets retained exact reservation",
            )));
        }
        let row: Option<(String,String)> = c.query_row("SELECT owner,operation FROM reservations WHERE id=?1 AND length(CAST(owner AS BLOB))<=512 AND length(CAST(operation AS BLOB))<=128",
            [id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((owner, operation)) = row {
            if owner != self.provenance.binding().owner.as_str()
                || operation.is_empty()
                || operation.contains('\0')
                || reserved(&operation)
            {
                return Err(store(rejected(
                    "admitted ordinary reservation owner or operation differs",
                )));
            }
        } else if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM reservations WHERE id=?1)",
            [id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(store(rejected(
                "admitted ordinary reservation exceeds metadata bounds",
            )));
        }
        Ok(())
    }

    pub(super) fn authorize_protection(
        &self,
        marker: &format::Marker,
        operation: &str,
    ) -> Result<()> {
        if marker != self.provenance.marker()
            || operation.is_empty()
            || operation.len() > 128
            || operation.contains('\0')
            || reserved(operation)
        {
            return Err(store(rejected(
                "admitted ordinary protection targets imported provenance",
            )));
        }
        Ok(())
    }

    /// Ordinary staging must never reuse a directory retained by an exact
    /// installation, even when its reservation and owner are otherwise valid.
    pub(super) fn authorize_directory(
        &self,
        c: &Connection,
        directory: &str,
        identity: &str,
    ) -> Result<()> {
        require_unretained_directory(c, directory, identity)
    }
}

pub(super) fn require_unretained_directory(
    c: &Connection,
    directory: &str,
    identity: &str,
) -> Result<()> {
    let mut query = c.prepare("SELECT payload FROM exact_installations LIMIT 100001")?;
    let mut rows = query.query([])?;
    let mut count = 0usize;
    while let Some(row) = rows.next()? {
        count += 1;
        let payload = row.get_ref(0)?.as_blob().map_err(invalid)?;
        if count > 100_000 || payload.len() > 8192 {
            return Err(invalid("retained directory inventory exceeds bound"));
        }
        let record: super::exact_content::ExactRecord =
            serde_json::from_slice(payload).map_err(invalid)?;
        if record.directory == directory || record.directory_identity.as_deref() == Some(identity) {
            return Err(invalid("ordinary staging aliases retained exact directory"));
        }
    }
    Ok(())
}
