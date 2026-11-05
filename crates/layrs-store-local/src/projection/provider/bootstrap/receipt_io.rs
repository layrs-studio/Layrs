//! Atomic process-interruption publication of the three fixed receipts only.
use super::*;
use crate::secure_fs::{PinnedDirectory, PinnedFile};
use std::{
    ffi::OsStr,
    io::{Read, Seek, SeekFrom, Write},
};

pub(super) struct PendingReceipt {
    file: PinnedFile,
    temporary: PathBuf,
    final_path: PathBuf,
    kind: ProviderBootstrapReceipt,
}
pub(super) fn temporary_name(
    kind: ProviderBootstrapReceipt,
    transaction: &str,
    nonce: &str,
) -> CauseResult<String> {
    records::codec::transaction(transaction)?;
    let parts: Vec<_> = nonce.split('-').collect();
    if parts.len() != 3
        || parts
            .iter()
            .zip([39, 10, 20])
            .any(|(v, max)| v.is_empty() || v.len() > max || !v.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(invalid("provider receipt nonce shape differs"));
    }
    let (kind_name, _) = kind.names();
    let name = format!("pb-{kind_name}-{transaction}-{nonce}.tmp");
    if name.len() > 122 {
        return Err(invalid("provider receipt temporary name exceeds bound"));
    }
    Ok(name)
}

impl PendingReceipt {
    pub(super) fn create(
        parent: &PinnedDirectory,
        directory: &Path,
        transaction: &str,
        kind: ProviderBootstrapReceipt,
        ctx: &mut WriteContext,
    ) -> CauseResult<Self> {
        records::codec::transaction(transaction)?;
        let nonce = crate::projection::shared::unique_id()?;
        let name = temporary_name(kind, transaction, &nonce)?;
        let (_, final_name) = kind.names();
        let temporary = directory.join(&name);
        let index = ctx.arm(
            ProviderBootstrapWriteStep::ExclusiveCreate(ProviderBootstrapCreate::ReceiptTemp(kind)),
            &temporary,
        )?;
        // Composite helpers can fail after creation. No error-code-based adoption
        // or no-effect inference; only a returned validated handle confirms this step.
        let file = parent.create_new_private_file(OsStr::new(&name))?;
        ctx.confirm(index)?;
        Ok(Self {
            file,
            temporary,
            final_path: directory.join(final_name),
            kind,
        })
    }
    pub(super) fn identity(&self) -> CauseResult<String> {
        Ok(crate::projection::file::identity(self.file.file())?)
    }
    pub(super) fn publish<R: Receipt>(
        mut self,
        parent: &PinnedDirectory,
        record: &R,
        ctx: &mut WriteContext,
    ) -> CauseResult<()> {
        let bytes = record.encode()?;
        if record.file_identity() != self.identity()? {
            return Err(invalid(
                "provider receipt does not bind its exclusive temporary inode",
            ));
        }
        let write = ctx.arm(
            ProviderBootstrapWriteStep::ReceiptWrite(self.kind),
            &self.temporary,
        )?;
        #[cfg(test)]
        tests::hooks::receipt_prefix(&mut self.file, self.kind, &bytes)?;
        self.file.file_mut().write_all(&bytes)?;
        self.file.file().sync_all()?;
        self.file.file_mut().seek(SeekFrom::Start(0))?;
        let mut actual = Vec::new();
        self.file
            .file_mut()
            .take(bytes.len() as u64 + 1)
            .read_to_end(&mut actual)?;
        if actual != bytes || self.file.file().metadata()?.len() != bytes.len() as u64 {
            return Err(invalid("provider receipt temporary readback differs"));
        }
        self.file.verify_parent()?;
        parent.verify_path_identity()?;
        ctx.confirm(write)?;
        let rename = ctx.arm(
            ProviderBootstrapWriteStep::ReceiptRename(self.kind),
            &self.final_path,
        )?;
        if self
            .file
            .rename_to(parent, OsStr::new(self.kind.names().1))?
        {
            ctx.no_effect(rename)?;
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "provider authoritative receipt collision; temporary retained",
            )
            .into());
        }
        ctx.confirm(rename)?;
        let finalize = ctx.arm(
            ProviderBootstrapWriteStep::ReceiptFinalize(self.kind),
            &self.final_path,
        )?;
        // Existing Windows policy may suppress PermissionDenied/InvalidInput.
        // This is not proof of a native parent flush or power-loss durability.
        parent.sync()?;
        let mut final_file = crate::layout::open_read_no_follow(&self.final_path)?;
        if crate::projection::file::identity(&final_file)? != record.file_identity()
            || final_file.metadata()?.len() != bytes.len() as u64
        {
            return Err(invalid("provider receipt final identity/length differs"));
        }
        actual.clear();
        Read::by_ref(&mut final_file)
            .take(bytes.len() as u64 + 1)
            .read_to_end(&mut actual)?;
        if actual != bytes {
            return Err(invalid("provider receipt final bytes differ"));
        }
        parent.verify_path_identity()?;
        ctx.confirm(finalize)
    }
}
