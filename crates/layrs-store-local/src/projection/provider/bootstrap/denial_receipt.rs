//! Compact fixed-slot denial-start receipt. Process-interruption protocol only:
//! parent.sync follows the existing Windows suppression policy and does not
//! establish a successful native directory flush or power-loss durability.
//! No SQL, family reservation, A/P/T receipt kind or receipt adoption is supplied.
use crate::{
    projection::{
        file,
        provider::{
            cancellation,
            records::{
                codec, denial_start::PendingDeniedCoverageReleaseRecord, AllocationRecord,
                PreparedRecord, Receipt, Record,
            },
            recovery::error::{invalid, Result},
        },
        shared::{provider_access::canonical_path, source_denial::SourceDeniedCoverageReservation},
    },
    secure_fs::{PinnedDirectory, PinnedFile},
};
use layrs_vfs::Cancellation;
use std::{
    ffi::OsStr,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

pub(in crate::projection::provider) const FILE_NAME: &str = "pending-denial-start.json";

/// Retains the authenticated final inode and its parent. Cached record DATA
/// alone never authorizes verification or deletion of a pathname replacement.
pub(in crate::projection::provider) struct PublishedDeniedStartReceipt {
    parent: PinnedDirectory,
    file: PinnedFile,
    record: PendingDeniedCoverageReleaseRecord,
}

fn control(
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    write: bool,
    cancel: Option<&Cancellation>,
) -> Result<PinnedDirectory> {
    cancellation::check(cancel)?;
    allocation.validate_fields()?;
    prepared.require_allocation(allocation)?;
    let path = Path::new(&allocation.private.metadata_path);
    let metadata = PinnedDirectory::open(path)?;
    if metadata.identity_token() != allocation.private.metadata_identity
        || canonical_path(path)? != allocation.private.canonical_metadata_path
    {
        return Err(invalid("denial receipt metadata directory binding differs"));
    }
    let parent = if write {
        metadata.open_private_child_for_write(OsStr::new("provider-control"))?
    } else {
        metadata.open_private_child(OsStr::new("provider-control"))?
    };
    if parent.identity_token() != allocation.private.provider_control_identity {
        return Err(invalid("denial receipt control directory binding differs"));
    }
    metadata.verify_path_identity()?;
    parent.verify_path_identity()?;
    cancellation::check(cancel)?;
    Ok(parent)
}

fn temporary_name(transaction: &str, nonce: &str) -> Result<String> {
    codec::transaction(transaction)?;
    let parts: Vec<_> = nonce.split('-').collect();
    if parts.len() != 3
        || parts.iter().zip([39, 10, 20]).any(|(part, max)| {
            part.is_empty() || part.len() > max || !part.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(invalid("denial receipt temporary nonce shape differs"));
    }
    let name = format!("pd-start-{transaction}-{nonce}.tmp");
    if name.len() > 122 {
        return Err(invalid("denial receipt temporary name exceeds bound"));
    }
    Ok(name)
}

fn read_bounded(file: &File) -> Result<Vec<u8>> {
    let bytes = file.metadata()?.len();
    if bytes == 0 || bytes > PendingDeniedCoverageReleaseRecord::MAX_BYTES as u64 {
        return Err(invalid("denial receipt length exceeds bound"));
    }
    let mut reader = file.try_clone()?;
    reader.seek(SeekFrom::Start(0))?;
    let mut actual = Vec::new();
    reader
        .take(PendingDeniedCoverageReleaseRecord::MAX_BYTES as u64 + 1)
        .read_to_end(&mut actual)?;
    if actual.len() as u64 != bytes || file.metadata()?.len() != bytes {
        return Err(invalid("denial receipt length changed during read"));
    }
    Ok(actual)
}

fn read_pinned(pin: &PinnedFile) -> Result<PendingDeniedCoverageReleaseRecord> {
    pin.verify_parent()?;
    let identity = file::identity(pin.file())?;
    let actual = read_bounded(pin.file())?;
    let record = PendingDeniedCoverageReleaseRecord::decode(&actual)?;
    if record.file_identity() != identity || file::identity(pin.file())? != identity {
        return Err(invalid("denial receipt native self-identity differs"));
    }
    Ok(record)
}

fn read_slot(
    parent: &PinnedDirectory,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    cancel: Option<&Cancellation>,
) -> Result<Option<(PinnedFile, PendingDeniedCoverageReleaseRecord)>> {
    cancellation::check(cancel)?;
    parent.verify_path_identity()?;
    let file = match parent.open_file(OsStr::new(FILE_NAME)) {
        Ok(file) => file,
        Err(error) if error.raw_os_error() == Some(2) => {
            parent.verify_path_identity()?;
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    let record = read_pinned(&file)?;
    record.require_references(allocation, prepared)?;
    parent.verify_path_identity()?;
    cancellation::check(cancel)?;
    Ok(Some((file, record)))
}

/// Reopen only the fixed slot, from actual bounded canonical bytes and native
/// inode identity. The caller authenticates actual A/P, owner and recovery gates.
pub(in crate::projection::provider) fn reopen(
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    cancel: Option<&Cancellation>,
) -> Result<Option<PublishedDeniedStartReceipt>> {
    let parent = control(allocation, prepared, false, cancel)?;
    let Some((file, record)) = read_slot(&parent, allocation, prepared, cancel)? else {
        return Ok(None);
    };
    let published = PublishedDeniedStartReceipt {
        parent,
        file,
        record,
    };
    published.verify(cancel)?;
    Ok(Some(published))
}

/// Publish before the first family writer. Existing final receipts are accepted
/// only after complete native/byte/A/P authentication and exact funding equality.
/// Failed create/write/rename/finalize retains physical objects without cleanup;
/// no error code permits repair, replacement or temporary-file adoption.
pub(in crate::projection::provider) fn publish(
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    funding: &SourceDeniedCoverageReservation,
    cancel: Option<&Cancellation>,
) -> Result<PublishedDeniedStartReceipt> {
    let parent = control(allocation, prepared, true, cancel)?;
    if let Some((file, record)) = read_slot(&parent, allocation, prepared, cancel)? {
        record.require_reservation(funding)?;
        let published = PublishedDeniedStartReceipt {
            parent,
            file,
            record,
        };
        published.verify(cancel)?;
        return Ok(published);
    }
    let name = temporary_name(
        &funding.binding().transaction_id,
        &crate::projection::shared::unique_id()?,
    )?;
    cancellation::check(cancel)?;
    // Exclusive protected creation can fail after producing a physical file.
    // Never infer absence or clean up an unreturned/uncertain create result.
    let mut temporary = parent.create_new_private_file(OsStr::new(&name))?;
    let record = PendingDeniedCoverageReleaseRecord::new(
        allocation,
        prepared,
        funding,
        file::identity(temporary.file())?,
    )?;
    let bytes = record.encode()?;
    cancellation::check(cancel)?;
    temporary.file_mut().write_all(&bytes)?;
    temporary.file().sync_all()?;
    if read_pinned(&temporary)? != record {
        return Err(invalid("denial receipt temporary full readback differs"));
    }
    parent.verify_path_identity()?;
    cancellation::check(cancel)?;
    if temporary.rename_to(&parent, OsStr::new(FILE_NAME))? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "denial receipt final collision; temporary retained without replacement",
        )
        .into());
    }
    // The existing Windows sync policy can suppress unsupported-directory flush
    // errors. This publication promises process-interruption behavior only.
    parent.sync()?;
    cancellation::check(cancel)?;
    let Some((file, actual)) = read_slot(&parent, allocation, prepared, cancel)? else {
        return Err(invalid("denial receipt final slot disappeared"));
    };
    if !temporary.same_file(&file) || actual != record {
        return Err(invalid(
            "denial receipt final inode or complete bytes differ",
        ));
    }
    let published = PublishedDeniedStartReceipt {
        parent,
        file,
        record,
    };
    published.verify(cancel)?;
    Ok(published)
}

impl PublishedDeniedStartReceipt {
    pub(in crate::projection::provider) fn record(&self) -> &PendingDeniedCoverageReleaseRecord {
        &self.record
    }

    pub(in crate::projection::provider) fn verify(
        &self,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        cancellation::check(cancel)?;
        self.parent.verify_path_identity()?;
        if read_pinned(&self.file)? != self.record {
            return Err(invalid("retained denial receipt bytes differ"));
        }
        let current = self.parent.open_readonly_file(OsStr::new(FILE_NAME))?;
        if !self.file.same_file(&current) || read_pinned(&current)? != self.record {
            return Err(invalid("denial receipt fixed slot was replaced or changed"));
        }
        self.parent.verify_path_identity()?;
        cancellation::check(cancel)
    }

    /// Caller proves completed Planned or exact funded-only release first.
    /// Delete this verified pinned inode, never an entry selected by name alone.
    /// No automatic cleanup runs on any publication or verification failure.
    pub(in crate::projection::provider) fn cleanup(
        self,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        self.verify(cancel)?;
        cancellation::check(cancel)?;
        let Self { parent, file, .. } = self;
        file.unlink()?;
        drop(file);
        parent.sync()?;
        parent.verify_path_identity()?;
        match parent.open_readonly_file(OsStr::new(FILE_NAME)) {
            Err(error) if error.raw_os_error() == Some(2) => {
                parent.verify_path_identity()?;
                Ok(())
            }
            Err(error) => Err(error.into()),
            Ok(_) => Err(invalid(
                "denial receipt fixed slot remains after pinned cleanup",
            )),
        }
    }
}

#[cfg(test)]
#[path = "denial_receipt/tests.rs"]
mod tests;
