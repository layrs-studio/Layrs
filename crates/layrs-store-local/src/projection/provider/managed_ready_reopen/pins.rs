//! Physical identity inputs only. Every file is existing and opened without
//! following reparses; no pathname is adopted as a protocol ownership proof.
use super::*;
use crate::{
    projection::{
        file::{self, DatabasePin},
        format,
        shared::provider_access::FamilyAccess,
    },
    secure_fs::{PinnedDirectory, PinnedFile},
};
use layrs_vfs::OrdinaryRootGuard;
use std::{ffi::OsStr, io::Read, path::PathBuf};

pub(super) struct ManagedReadyPins {
    ordinary: OrdinaryRootGuard,
    pub root: PinnedDirectory,
    pub control: PinnedDirectory,
    metadata: PinnedDirectory,
    format: PinnedFile,
    database_pin: DatabasePin,
    catalog_pin: DatabasePin,
    pub database: database_io::DatabaseFile,
    pub family: FamilyAccess,
    pub marker: format::Marker,
    checkout_path: PathBuf,
    metadata_path: PathBuf,
}

impl ManagedReadyPins {
    pub(super) fn open(requested: &Path, cancel: &Cancellation) -> Result<Self> {
        cancel.check()?;
        let ordinary = OrdinaryRootGuard::open(requested)?;
        let root = PinnedDirectory::open(ordinary.root())?;
        if root.identity_token() != ordinary.filesystem_identity() {
            return Err(invalid("managed Ready root observation differs"));
        }
        let control = root.open_private_child(OsStr::new(".layrs"))?;
        let format = control.open_immutable_file(OsStr::new("FORMAT"))?;
        file::require_single_link(format.file())?;
        if format.file().metadata()?.len() > 65536 {
            return Err(invalid("managed Ready FORMAT exceeds bound"));
        }
        let mut bytes = Vec::new();
        let mut reader = format.file();
        Read::by_ref(&mut reader)
            .take(65537)
            .read_to_end(&mut bytes)?;
        let marker = format::parse_marker_bytes(&bytes)?;
        let binding = marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("managed Ready requires shared FORMAT"))?;
        if marker.import_digest.is_none() || root.identity_token() != marker.checkout_identity() {
            return Err(invalid("managed Ready FORMAT root or import differs"));
        }
        let checkout_path = PathBuf::from(&marker.checkout_root);
        let original = PinnedDirectory::open(&checkout_path)?;
        if original.identity_token() != root.identity_token() {
            return Err(invalid("managed Ready bound checkout path changed"));
        }
        let family = FamilyAccess::open(&binding.root)?;
        let family_record = family.writer_record()?;
        if family.writer_identity(&marker.identity.worktree_id) != marker.identity
            || family_record.incarnation != binding.incarnation
            || family_record.root_identity != binding.root_identity
            || family.member(&marker.identity.worktree_id) != binding.metadata_path
        {
            return Err(invalid("managed Ready physical family differs from FORMAT"));
        }
        let metadata_path = binding.metadata_path.clone();
        let metadata = PinnedDirectory::open(&metadata_path)?;
        if metadata.identity_token() != marker.metadata_identity {
            return Err(invalid("managed Ready private metadata identity differs"));
        }
        let database_pin = DatabasePin::open(&metadata_path.join("projection.sqlite3"))?;
        file::require_single_link(&database_pin.file)?;
        let database = database_io::DatabaseFile::open(
            metadata_path.join("projection.sqlite3"),
            &binding.database_identity,
        )?;
        let catalog_pin = DatabasePin::open(&family.root().join("metadata/catalog.sqlite3"))?;
        file::require_single_link(&catalog_pin.file)?;
        if catalog_pin.identity() != family_record.catalog_file_identity {
            return Err(invalid("managed Ready family catalog identity differs"));
        }
        let pins = Self {
            ordinary,
            root,
            control,
            metadata,
            format,
            database_pin,
            catalog_pin,
            database,
            family,
            marker,
            checkout_path,
            metadata_path,
        };
        pins.verify(cancel)?;
        Ok(pins)
    }

    pub(super) fn checkout_path(&self) -> &Path {
        &self.checkout_path
    }
    pub(super) fn metadata_path(&self) -> &Path {
        &self.metadata_path
    }

    pub(super) fn verify(&self, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        self.ordinary.revalidate()?;
        self.root.verify_path_identity()?;
        if self.root.identity_token() != self.marker.checkout_identity()
            || self.root.identity_token() != self.ordinary.filesystem_identity()
            || PinnedDirectory::open(&self.checkout_path)?.identity_token()
                != self.root.identity_token()
        {
            return Err(invalid("managed Ready checkout identity changed"));
        }
        self.control.verify_path_identity()?;
        if self
            .root
            .open_private_child(OsStr::new(".layrs"))?
            .identity_token()
            != self.control.identity_token()
        {
            return Err(invalid("managed Ready control directory changed"));
        }
        self.format.verify_parent()?;
        file::require_single_link(self.format.file())?;
        if !self
            .format
            .same_file(&self.control.open_readonly_file(OsStr::new("FORMAT"))?)
            || self.format.file().metadata()?.len() != self.marker.bytes()?.len() as u64
        {
            return Err(invalid("managed Ready FORMAT inode or length changed"));
        }
        self.metadata.verify_path_identity()?;
        if self.metadata.identity_token() != self.marker.metadata_identity {
            return Err(invalid("managed Ready metadata identity changed"));
        }
        self.database.verify()?;
        self.database_pin
            .verify(&self.metadata_path.join("projection.sqlite3"))?;
        file::require_single_link(&self.database_pin.file)?;
        self.catalog_pin
            .verify(&self.family.root().join("metadata/catalog.sqlite3"))?;
        file::require_single_link(&self.catalog_pin.file)?;
        self.family.verify()?;
        self.ordinary.revalidate()?;
        cancel.check()?;
        Ok(())
    }

    pub(super) fn require_data(
        &self,
        data: &OwnedReadySnapshot,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.require_parts(
            &data.marker,
            &data.ready,
            &data.expected,
            &data.binding,
            cancel,
        )
    }

    /// Physical comparisons shared by genuinely interpreted Ready8 and
    /// admitted terminal10 DATA. This does not choose a SQL origin/layout.
    pub(super) fn require_parts(
        &self,
        marker: &format::Marker,
        ready: &ProviderTransitionIntentRecord,
        expected: &ExpectedCheckout,
        binding: &crate::projection::shared::exact_content::ExactBinding,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.verify(cancel)?;
        let actual = self.family.writer_record()?;
        let proof = ready
            .destination
            .as_ref()
            .ok_or_else(|| invalid("managed Ready destination DATA absent"))?;
        if *marker != self.marker
            || proof.format != self.marker
            || self.control.identity_token() != expected.header().control_identity
            || file::identity(self.format.file())? != proof.format_file_identity
            || self.database_pin.identity() != proof.private_database_identity
            || binding.family_incarnation != actual.incarnation
            || binding.family_root_identity != actual.root_identity
            || binding.family_marker_digest != self.family.marker_digest()
            || proof.family_marker_digest != self.family.marker_digest()
            || binding.family_catalog_identity != actual.catalog_file_identity
        {
            return Err(invalid("managed Ready SQL DATA differs from physical pins"));
        }
        cancel.check()?;
        Ok(())
    }
}
