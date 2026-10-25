//! One durable installation per reservation/FileId. A retry never adds a second
//! physical attempt while the first attempt's bytes remain charged ambiguously.
use super::*;
use layrs_core::scm::FileId;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Intent {
    incarnation: String,
    reservation: String,
    owner: WorktreeId,
    database_identity: String,
    operation: String,
    file_id: FileId,
    bytes: u64,
    directory: String,
    directory_identity: String,
}
pub(super) struct Stage {
    intent: Intent,
    payload: Vec<u8>,
    pub manifest: Option<FileManifest>,
    pub published: bool,
}
impl Stage {
    pub fn reservation(&self) -> &str {
        &self.intent.reservation
    }
    pub fn begin(lease: &Lease, reservation: &str, expected: &CapturedFile) -> Result<Self> {
        let catalog = lease.catalog(true)?;
        let (owner, operation, database_identity): (String, String, String) = catalog.connection.query_row(
            "SELECT r.owner,r.operation,w.database_identity FROM reservations r JOIN worktrees w ON w.id=r.owner WHERE r.id=?1",
            [reservation], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let existing = load(&catalog.connection, reservation, &expected.file_id)?;
        let remaining: u64 = catalog.connection.query_row(
            "SELECT remaining FROM reservations WHERE id=?1",
            [reservation],
            |r| r.get(0),
        )?;
        let required = retention::budget_for(std::slice::from_ref(expected), 0)?;
        if remaining < required && existing.is_none() {
            lease.grow_reservation(&catalog.connection, reservation, remaining, required)?;
        }
        let stage = if let Some(stage) = existing {
            stage
        } else {
            let parent = PinnedDirectory::open(&lease.family.root.join("staging"))?;
            let name = unique_id()?;
            let directory = parent.ensure_private_child(name.as_ref())?;
            let intent = Intent {
                incarnation: lease.family.marker.incarnation.clone(),
                reservation: reservation.into(),
                owner: WorktreeId::parse(owner.clone()).map_err(invalid)?,
                database_identity: database_identity.clone(),
                operation: operation.clone(),
                file_id: expected.file_id.clone(),
                bytes: expected.bytes,
                directory: name,
                directory_identity: directory.identity_token(),
            };
            let payload = canonical_json(&intent).map_err(invalid)?;
            // No content is written before this row commits. A crash between the
            // mkdir and this insert can leave only an empty directory.
            catalog.connection.execute(
                "INSERT INTO installations VALUES(?1,?2,?3,NULL,0)",
                params![reservation, expected.file_id.as_str(), payload],
            )?;
            Stage {
                intent,
                payload,
                manifest: None,
                published: false,
            }
        };
        if stage.intent.incarnation != lease.family.marker.incarnation
            || stage.intent.reservation != reservation
            || stage.intent.file_id != expected.file_id
            || stage.intent.bytes != expected.bytes
            || stage.intent.owner.as_str() != owner
            || stage.intent.operation != operation
            || stage.intent.database_identity != database_identity
        {
            return Err(invalid("installation ownership or source identity differs"));
        }
        if let Some(manifest) = &stage.manifest {
            if manifest.file_id() != &expected.file_id || manifest.byte_length() != expected.bytes {
                return Err(invalid("installation manifest differs from its intent"));
            }
        }
        Ok(stage)
    }
    pub fn directory(&self, lease: &Lease) -> Result<PinnedDirectory> {
        if self.intent.directory.is_empty()
            || self.intent.directory.len() > 128
            || !self
                .intent
                .directory
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'-')
        {
            return Err(invalid("installation directory is not bounded"));
        }
        let directory = PinnedDirectory::open(&lease.family.root.join("staging"))?
            .open_child(self.intent.directory.as_ref())?;
        if directory.identity_token() != self.intent.directory_identity {
            return Err(invalid("installation directory replaced"));
        }
        Ok(directory)
    }
    pub fn reset_unverified(&self, lease: &Lease) -> Result<()> {
        if self.manifest.is_some() || self.published {
            return Err(invalid("verified installation cannot be restaged"));
        }
        self.clean_files(lease)
    }
    fn clean_files(&self, lease: &Lease) -> Result<()> {
        let directory = self.directory(lease)?;
        let path = lease
            .family
            .root
            .join("staging")
            .join(&self.intent.directory);
        let mut names = Vec::new();
        // Enumerate a bounded whitelist first. An unexpected entry or reparse
        // point blocks cleanup and keeps the reservation; never recursively delete.
        for item in fs::read_dir(path)?.take(1026) {
            let name = item?.file_name();
            let valid = name.to_str().is_some_and(|s| {
                s == "manifest"
                    || s.strip_prefix("chunk-")
                        .and_then(|s| s.parse::<usize>().ok())
                        .is_some_and(|n| n < 1024 && s == format!("chunk-{n}"))
            });
            if !valid || names.len() >= 1025 {
                return Err(invalid(
                    "unrecognized installation staging; reservation retained",
                ));
            }
            let pin = directory.open_file(&name)?;
            if pin.file().metadata()?.len() > CAPTURE_CHUNK_BYTES as u64 {
                return Err(invalid("installation staged file exceeds bound"));
            }
            names.push(name);
        }
        directory.verify_path_identity()?;
        for name in names {
            directory.open_file(&name)?.unlink()?;
        }
        directory.sync()?;
        directory.verify_path_identity()?;
        Ok(())
    }
    pub fn verified(&mut self, lease: &Lease, manifest: &FileManifest) -> Result<()> {
        let payload = canonical_json(manifest).map_err(invalid)?;
        if manifest.file_id() != &self.intent.file_id || manifest.byte_length() != self.intent.bytes
        {
            return Err(invalid("verified installation identity mismatch"));
        }
        let changed = lease.catalog(true)?.connection.execute("UPDATE installations SET manifest=?1 WHERE reservation=?2 AND file_id=?3 AND payload=?4 AND published=0", params![payload, self.intent.reservation, self.intent.file_id.as_str(), self.payload])?;
        if changed != 1 {
            return Err(invalid("installation intent changed"));
        }
        self.manifest = Some(manifest.clone());
        Ok(())
    }
    pub fn accept(&self, c: &Connection) -> Result<()> {
        if c.execute("UPDATE installations SET published=1 WHERE reservation=?1 AND file_id=?2 AND payload=?3 AND manifest IS NOT NULL", params![self.intent.reservation, self.intent.file_id.as_str(), self.payload])? != 1 {
            return Err(invalid("installation publication lost its intent"));
        }
        Ok(())
    }
    pub fn finish(&self, lease: &Lease) -> Result<()> {
        // Fully admitted content is verified before caller permits this cleanup.
        match self.directory(lease) {
            Ok(directory) => {
                self.clean_files(lease)?;
                drop(directory);
                PinnedDirectory::open(&lease.family.root.join("staging"))?
                    .remove_empty_child(self.intent.directory.as_ref())?;
            }
            Err(ProjectionStoreError::Io(e))
                if e.kind() == io::ErrorKind::NotFound
                    && (self.published || self.manifest.is_none()) =>
            {
                ()
            }
            Err(e) => return Err(e),
        }
        if lease.catalog(true)?.connection.execute(
            "DELETE FROM installations WHERE reservation=?1 AND file_id=?2 AND payload=?3",
            params![
                self.intent.reservation,
                self.intent.file_id.as_str(),
                self.payload
            ],
        )? != 1
        {
            return Err(invalid("installation cleanup lost its intent"));
        }
        Ok(())
    }
    pub fn finish_existing(
        lease: &Lease,
        reservation: &str,
        expected: &CapturedFile,
    ) -> Result<()> {
        let catalog = lease.catalog(false)?;
        let exists = load(&catalog.connection, reservation, &expected.file_id)?.is_some();
        drop(catalog);
        if exists {
            let stage = Self::begin(lease, reservation, expected)?;
            stage.finish(lease)?;
        }
        Ok(())
    }
}

impl Lease {
    pub(super) fn installation_dependencies(
        &self,
    ) -> Result<(
        std::collections::BTreeSet<String>,
        std::collections::BTreeSet<String>,
    )> {
        let catalog = self.catalog(false)?;
        let mut q = catalog.connection.prepare("SELECT reservation,file_id FROM installations WHERE manifest IS NOT NULL ORDER BY reservation,file_id LIMIT 16385")?;
        let keys = q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if keys.len() > 16384 {
            return Err(invalid(
                "installation dependency bound exceeded; reclamation refused",
            ));
        }
        let mut manifests = std::collections::BTreeSet::new();
        let mut chunks = std::collections::BTreeSet::new();
        let mut budget = 0usize;
        for (reservation, file) in keys {
            let stage = load(
                &catalog.connection,
                &reservation,
                &FileId::parse(file).map_err(invalid)?,
            )?
            .ok_or_else(|| invalid("installation disappeared"))?;
            let manifest = stage
                .manifest
                .ok_or_else(|| invalid("installation dependency missing"))?;
            budget = budget
                .checked_add(canonical_json(&manifest).map_err(invalid)?.len())
                .filter(|n| *n <= 64 * 1024 * 1024)
                .ok_or_else(|| invalid("installation dependency metadata exceeds bound"))?;
            if stage.intent.incarnation != self.family.marker.incarnation
                || manifest.file_id() != &stage.intent.file_id
                || manifest.byte_length() != stage.intent.bytes
            {
                return Err(invalid("installation dependency identity mismatch"));
            }
            manifests.insert(manifest.id().to_string());
            chunks.extend(manifest.chunks().iter().map(|p| p.chunk_id().to_string()));
        }
        Ok((manifests, chunks))
    }
    /// Caller holds either this owner's exclusive private lock or the exclusive
    /// family lifecycle. Verified pieces are admitted before releasing their
    /// charge; unverified owned staging is removed without touching checkout data.
    pub(in crate::projection) fn cancel_installations(
        &self,
        reservation: &str,
        owner: &WorktreeId,
    ) -> Result<()> {
        let catalog = self.catalog(false)?;
        let actual: Option<String> = catalog
            .connection
            .query_row(
                "SELECT owner FROM reservations WHERE id=?1",
                [reservation],
                |r| r.get(0),
            )
            .optional()?;
        let Some(actual) = actual else {
            return Ok(());
        };
        if actual != owner.as_str() {
            return Err(invalid("reservation owner mismatch"));
        }
        let mut q = catalog.connection.prepare(
            "SELECT file_id FROM installations WHERE reservation=?1 ORDER BY file_id LIMIT 1025",
        )?;
        let files = q
            .query_map([reservation], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if files.len() > 1024 {
            return Err(invalid("installation recovery exceeds bound"));
        }
        let mut stages = Vec::new();
        for file in files {
            stages.push(
                load(
                    &catalog.connection,
                    reservation,
                    &FileId::parse(file).map_err(invalid)?,
                )?
                .ok_or_else(|| invalid("installation disappeared"))?,
            );
        }
        drop(q);
        drop(catalog);
        for stage in stages {
            let expected = CapturedFile {
                file_id: stage.intent.file_id.clone(),
                bytes: stage.intent.bytes,
                executable: false,
            };
            let stage = Stage::begin(self, reservation, &expected)?;
            if let Some(manifest) = self.manifest_for(&expected.file_id)? {
                self.verify_manifest(&manifest)?;
                stage.finish(self)?;
            } else if stage.manifest.is_some() {
                self.publish_installation(stage, &mut |_| Ok(()))?;
            } else {
                stage.finish(self)?;
            }
        }
        Ok(())
    }
}
fn load(c: &Connection, reservation: &str, file: &FileId) -> Result<Option<Stage>> {
    let row: Option<(Vec<u8>, Option<Vec<u8>>, bool)> = c.query_row("SELECT payload,manifest,published FROM installations WHERE reservation=?1 AND file_id=?2 AND length(payload)<=8192", params![reservation, file.as_str()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
    row.map(|(payload, manifest, published)| {
        let intent: Intent = serde_json::from_slice(&payload).map_err(invalid)?;
        if canonical_json(&intent).map_err(invalid)? != payload {
            return Err(invalid("installation intent is not canonical"));
        }
        let manifest = manifest
            .map(|p| {
                let value: FileManifest = serde_json::from_slice(&p).map_err(invalid)?;
                if canonical_json(&value).map_err(invalid)? != p {
                    return Err(invalid("installation manifest is not canonical"));
                }
                Ok(value)
            })
            .transpose()?;
        if published && manifest.is_none() {
            return Err(invalid("published installation lacks manifest"));
        }
        Ok(Stage {
            intent,
            payload,
            manifest,
            published,
        })
    })
    .transpose()
}
