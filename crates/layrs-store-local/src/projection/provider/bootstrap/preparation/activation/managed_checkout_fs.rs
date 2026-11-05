//! Checkout filesystem effects only. Every journal argument is caller-validated
//! DATA read after COMMIT and positive close. No SQL, adoption or cleanup here.
use crate::{
    projection::{
        file,
        provider::{
            checkout_journal::{
                CheckoutEntry, CheckoutFile, CheckoutHeader, CheckoutJournal, CheckoutRow,
                DirectoryState, ExpectedCheckout, FileState, RowKey,
            },
            recovery::error::{Result, invalid},
        },
    },
    secure_fs::{PinnedDirectory, PinnedFile},
};
use layrs_vfs::Cancellation;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

pub(in crate::projection::provider) struct CheckoutStorage {
    root: PinnedDirectory,
    control: PinnedDirectory,
    header: CheckoutHeader,
}
pub(in crate::projection::provider) struct OwnedCheckoutFile {
    ordinal: u32,
    header: CheckoutHeader,
    inode: String,
    pin: PinnedFile,
}
pub(in crate::projection::provider) struct VerifiedCheckout {
    storage: CheckoutStorage,
    files: Vec<(u32, PinnedFile)>,
}

/// Inspection-only facade. All journal ownership is caller-validated DATA;
/// this opens existing objects, checks every byte/inode and retains immutable
/// final pins. It cannot create, publish, adopt, repair or clean any object.
pub(in crate::projection::provider) fn verify_managed_ready_checkout(
    root: PinnedDirectory,
    control: PinnedDirectory,
    expected: &ExpectedCheckout,
    journal: &CheckoutJournal,
    cancel: &Cancellation,
) -> Result<VerifiedCheckout> {
    CheckoutStorage::new(root, control, expected, cancel)?
        .verify_complete(expected, journal, cancel)
}

impl CheckoutStorage {
    pub(in crate::projection::provider) fn new(
        root: PinnedDirectory,
        control: PinnedDirectory,
        expected: &ExpectedCheckout,
        cancel: &Cancellation,
    ) -> Result<Self> {
        cancel.check()?;
        // Windows has no executable-bit implementation in install::set_mode.
        // Settle this before creating any checkout directory or temporary file.
        if expected.files().iter().any(|f| f.executable) {
            return Err(invalid(
                "Windows managed checkout executable mode unsupported",
            ));
        }
        let value = Self {
            root,
            control,
            header: expected.header().clone(),
        };
        value.verify_binding(expected, cancel)?;
        let pin = value.control.open_readonly_file(OsStr::new("FORMAT"))?;
        let wanted = expected
            .marker()
            .bytes()
            .map_err(|e| invalid(e.to_string()))?;
        if wanted.len() > 65536 {
            return Err(invalid("checkout FORMAT exceeds bound"));
        }
        let mut bytes = Vec::with_capacity(wanted.len());
        let mut reader = pin.file();
        Read::by_ref(&mut reader)
            .take(wanted.len() as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes != wanted {
            return Err(invalid("checkout FORMAT canonical bytes differ"));
        }
        value.verify_binding(expected, cancel)?;
        Ok(value)
    }

    /// Mechanical pre-declaration check. Absence of SQL progress grants no
    /// ownership over checkout names: only the pinned .layrs/FORMAT may exist.
    pub(in crate::projection::provider) fn require_unstarted_namespace(
        &self,
        expected: &ExpectedCheckout,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.verify_binding(expected, cancel)?;
        for (directory, allowed) in [(&self.root, ".layrs"), (&self.control, "FORMAT")] {
            directory.verify_path_identity()?;
            let path = if allowed == ".layrs" {
                PathBuf::from(&expected.marker().checkout_root)
            } else {
                PathBuf::from(&expected.marker().checkout_root).join(".layrs")
            };
            let mut count = 0usize;
            for entry in fs::read_dir(path)? {
                cancel.check()?;
                if count != 0 || entry?.file_name().as_os_str() != OsStr::new(allowed) {
                    return Err(invalid("unstarted checkout namespace has unowned entry"));
                }
                count += 1;
            }
            if count != 1 {
                return Err(invalid("unstarted checkout namespace binding absent"));
            }
            directory.verify_path_identity()?;
        }
        self.verify_binding(expected, cancel)
    }

    fn verify_binding(&self, expected: &ExpectedCheckout, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        if self.header != *expected.header()
            || self.root.identity_token() != self.header.checkout_identity
            || self.control.identity_token() != self.header.control_identity
        {
            return Err(invalid("checkout storage binding differs"));
        }
        self.root.verify_path_identity()?;
        self.control.verify_path_identity()?;
        let control = self.root.open_private_child(OsStr::new(".layrs"))?;
        if control.identity_token() != self.header.control_identity {
            return Err(invalid("checkout control path differs"));
        }
        let format = control.open_readonly_file(OsStr::new("FORMAT"))?;
        file::require_single_link(format.file())?;
        if file::identity(format.file())? != self.header.format_file_identity
            || format.file().metadata()?.len()
                != expected
                    .marker()
                    .bytes()
                    .map_err(|e| invalid(e.to_string()))?
                    .len() as u64
        {
            return Err(invalid("checkout FORMAT identity or length differs"));
        }
        cancel.check()?;
        Ok(())
    }

    fn require(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.verify_binding(expected, cancel)?;
        match &journal.row(RowKey::Header)?.entry {
            CheckoutEntry::Header { header } if header == expected.header() => Ok(()),
            _ => Err(invalid(
                "checkout journal header differs from retained storage",
            )),
        }
    }

    fn stage_name(expected: &ExpectedCheckout) -> String {
        format!("checkout-{}", expected.header().transaction_id)
    }

    fn stage(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
    ) -> Result<PinnedDirectory> {
        let CheckoutEntry::Stage {
            state: DirectoryState::Owned { identity },
        } = &journal.row(RowKey::Stage)?.entry
        else {
            return Err(invalid("checkout stage is not durably Owned"));
        };
        let pin = self
            .control
            .open_private_child_for_write(OsStr::new(&Self::stage_name(expected)))?;
        if pin.identity_token() != *identity {
            return Err(invalid("checkout stage identity differs"));
        }
        Ok(pin)
    }

    pub(in crate::projection::provider) fn prepare_stage(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        cancel: &Cancellation,
    ) -> Result<(PinnedDirectory, CheckoutRow)> {
        self.require_namespace(expected, journal, cancel)?;
        let row = journal.row(RowKey::Stage)?;
        match &row.entry {
            CheckoutEntry::Stage {
                state: DirectoryState::Declared,
            } => {
                let pin = self
                    .control
                    .create_new_private_child(OsStr::new(&Self::stage_name(expected)))?;
                let next = row.owned_directory(&pin.identity_token())?;
                Ok((pin, next))
            }
            CheckoutEntry::Stage {
                state: DirectoryState::Owned { .. },
            } => Ok((self.stage(expected, journal)?, row.clone())),
            _ => Err(invalid("checkout stage row has wrong kind")),
        }
    }

    fn parent(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        path: &str,
    ) -> Result<(PinnedDirectory, String)> {
        let (prefix, leaf) = path.rsplit_once('/').unwrap_or(("", path));
        let mut pin = PinnedDirectory::open(Path::new(&expected.marker().checkout_root))?;
        if pin.identity_token() != self.header.checkout_identity {
            return Err(invalid("checkout root changed"));
        }
        let mut relative = String::new();
        for component in prefix.split('/').filter(|s| !s.is_empty()) {
            if !relative.is_empty() {
                relative.push('/');
            }
            relative.push_str(component);
            let ordinal = expected
                .directories()
                .binary_search_by(|p| p.as_str().cmp(&relative))
                .map_err(|_| invalid("checkout parent is outside expected inventory"))?;
            let CheckoutEntry::Directory {
                path,
                state: DirectoryState::Owned { identity },
                ..
            } = &journal.row(RowKey::Directory(ordinal as u32))?.entry
            else {
                return Err(invalid("checkout parent directory is not durably Owned"));
            };
            if path.as_str() != relative {
                return Err(invalid("checkout parent journal path differs"));
            }
            pin = pin.open_child(OsStr::new(component))?;
            if pin.identity_token() != *identity {
                return Err(invalid("checkout parent inode differs"));
            }
        }
        pin.verify_path_identity()?;
        Ok((pin, leaf.to_owned()))
    }

    pub(in crate::projection::provider) fn prepare_directory(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        ordinal: u32,
        cancel: &Cancellation,
    ) -> Result<(PinnedDirectory, CheckoutRow)> {
        self.require_namespace(expected, journal, cancel)?;
        let path = expected
            .directories()
            .get(ordinal as usize)
            .ok_or_else(|| invalid("checkout directory ordinal absent"))?;
        let row = journal.row(RowKey::Directory(ordinal))?;
        let CheckoutEntry::Directory {
            path: recorded,
            state,
            ..
        } = &row.entry
        else {
            return Err(invalid("checkout directory row kind differs"));
        };
        if recorded != path {
            return Err(invalid("checkout directory path differs"));
        }
        let (parent, leaf) = self.parent(expected, journal, path.as_str())?;
        match state {
            DirectoryState::Declared => {
                let pin = parent.create_new_child(OsStr::new(&leaf))?;
                let next = row.owned_directory(&pin.identity_token())?;
                Ok((pin, next))
            }
            DirectoryState::Owned { identity } => {
                let pin = parent.open_child(OsStr::new(&leaf))?;
                if pin.identity_token() != *identity {
                    return Err(invalid("checkout directory identity differs"));
                }
                Ok((pin, row.clone()))
            }
        }
    }

    fn file_row<'a>(
        expected: &'a ExpectedCheckout,
        journal: &'a CheckoutJournal,
        ordinal: u32,
    ) -> Result<(&'a CheckoutRow, &'a CheckoutFile, &'a FileState)> {
        let wanted = expected
            .files()
            .get(ordinal as usize)
            .ok_or_else(|| invalid("checkout file ordinal absent"))?;
        let row = journal.row(RowKey::File(ordinal))?;
        let CheckoutEntry::File {
            ordinal: recorded,
            file,
            state,
        } = &row.entry
        else {
            return Err(invalid("checkout file row kind differs"));
        };
        if *recorded != ordinal || file != wanted || file.executable {
            return Err(invalid("checkout file inventory or Windows mode differs"));
        }
        Ok((row, file, state))
    }

    pub(in crate::projection::provider) fn create_temp(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        ordinal: u32,
        cancel: &Cancellation,
    ) -> Result<(OwnedCheckoutFile, CheckoutRow)> {
        self.require_namespace(expected, journal, cancel)?;
        let (row, _, state) = Self::file_row(expected, journal, ordinal)?;
        if *state != FileState::Declared {
            return Err(invalid("checkout create requires Declared file"));
        }
        let stage = self.stage(expected, journal)?;
        let pin = stage
            .create_new_private_file(OsStr::new(&expected.temporary_name(ordinal as usize)?))?;
        file::require_single_link(pin.file())?;
        let inode = file::identity(pin.file())?;
        let next = row.owned_file(&inode)?;
        Ok((
            OwnedCheckoutFile {
                ordinal,
                header: self.header.clone(),
                inode,
                pin,
            },
            next,
        ))
    }

    pub(in crate::projection::provider) fn open_owned_temp(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        ordinal: u32,
        cancel: &Cancellation,
    ) -> Result<OwnedCheckoutFile> {
        self.require_namespace(expected, journal, cancel)?;
        let (_, _, state) = Self::file_row(expected, journal, ordinal)?;
        let inode = state
            .owned_temp_inode()
            .ok_or_else(|| invalid("checkout rewrite requires durably Owned file"))?;
        let stage = self.stage(expected, journal)?;
        let pin =
            stage.open_file_for_sync(OsStr::new(&expected.temporary_name(ordinal as usize)?))?;
        require_inode(&pin, inode)?;
        Ok(OwnedCheckoutFile {
            ordinal,
            header: self.header.clone(),
            inode: inode.into(),
            pin,
        })
    }

    /// Only a fresh durably Owned row grants reset-to-zero of its exact TEMP.
    /// A newly created candidate cannot expose a writer before caller close.
    pub(in crate::projection::provider) fn begin_write<'p>(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        piece: &'p mut OwnedCheckoutFile,
        cancel: &Cancellation,
    ) -> Result<&'p mut fs::File> {
        self.require_namespace(expected, journal, cancel)?;
        let (_, _, state) = Self::file_row(expected, journal, piece.ordinal)?;
        if piece.header != self.header || state.owned_temp_inode() != Some(piece.inode.as_str()) {
            return Err(invalid("checkout writer is not the durable Owned inode"));
        }
        let stage = self.stage(expected, journal)?;
        let observed = stage.open_readonly_file(OsStr::new(
            &expected.temporary_name(piece.ordinal as usize)?,
        ))?;
        if !piece.pin.same_file(&observed) {
            return Err(invalid("checkout temporary path replaced"));
        }
        require_inode(&piece.pin, &piece.inode)?;
        cancel.check()?;
        piece.pin.file().set_len(0)?;
        piece.pin.file_mut().seek(SeekFrom::Start(0))?;
        Ok(piece.pin.file_mut())
    }

    pub(in crate::projection::provider) fn verify_temp(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        piece: &OwnedCheckoutFile,
        cancel: &Cancellation,
    ) -> Result<CheckoutRow> {
        self.require_namespace(expected, journal, cancel)?;
        let (row, file, state) = Self::file_row(expected, journal, piece.ordinal)?;
        if piece.header != self.header || state.owned_temp_inode() != Some(piece.inode.as_str()) {
            return Err(invalid("checkout verification requires Owned inode"));
        }
        let observed = self
            .stage(expected, journal)?
            .open_readonly_file(OsStr::new(
                &expected.temporary_name(piece.ordinal as usize)?,
            ))?;
        if !observed.same_file(&piece.pin) {
            return Err(invalid("checkout verify temporary path replaced"));
        }
        require_inode(&piece.pin, &piece.inode)?;
        piece.pin.file().sync_all()?;
        verify_bytes(&piece.pin, file, cancel)?;
        self.verify_binding(expected, cancel)?;
        row.verified_file()
    }

    pub(in crate::projection::provider) fn prepare_publish(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        ordinal: u32,
        cancel: &Cancellation,
    ) -> Result<CheckoutRow> {
        self.require_namespace(expected, journal, cancel)?;
        let (row, file, state) = Self::file_row(expected, journal, ordinal)?;
        let FileState::Verified { inode } = state else {
            return Err(invalid("checkout publishing requires Verified"));
        };
        let pin = self
            .stage(expected, journal)?
            .open_readonly_file(OsStr::new(&expected.temporary_name(ordinal as usize)?))?;
        require_inode(&pin, inode)?;
        verify_bytes(&pin, file, cancel)?;
        let (parent, _) = self.parent(expected, journal, file.path.as_str())?;
        row.publishing_file(&parent.identity_token())
    }

    /// Publishing is already durably acknowledged. A rename gap resumes only
    /// its exact inode at one name. Published replay performs no write or sync.
    pub(in crate::projection::provider) fn publish(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        ordinal: u32,
        cancel: &Cancellation,
    ) -> Result<CheckoutRow> {
        self.require_namespace(expected, journal, cancel)?;
        let (row, file, state) = Self::file_row(expected, journal, ordinal)?;
        let (inode, parent_id, done) = match state {
            FileState::Publishing {
                inode,
                parent_identity,
            } => (inode, parent_identity, false),
            FileState::Published {
                inode,
                parent_identity,
            } => (inode, parent_identity, true),
            _ => return Err(invalid("checkout rename requires durable Publishing")),
        };
        let (parent, leaf) = self.parent(expected, journal, file.path.as_str())?;
        if parent.identity_token() != *parent_id {
            return Err(invalid("checkout publishing parent differs"));
        }
        let stage = self.stage(expected, journal)?;
        let name = expected.temporary_name(ordinal as usize)?;
        let temp = optional(stage.open_readonly_file(OsStr::new(&name)))?;
        let final_pin = optional(parent.open_readonly_file(OsStr::new(&leaf)))?;
        if done {
            if temp.is_some() {
                return Err(invalid("Published checkout has a temporary sibling"));
            }
            let pin = final_pin.ok_or_else(|| invalid("Published checkout file missing"))?;
            require_inode(&pin, inode)?;
            verify_bytes(&pin, file, cancel)?;
            return Ok(row.clone());
        }
        match (temp, final_pin) {
            (Some(observed), None) => {
                require_inode(&observed, inode)?;
                drop(observed);
                let pin = stage.open_file_for_sync(OsStr::new(&name))?;
                require_inode(&pin, inode)?;
                verify_bytes(&pin, file, cancel)?;
                pin.file().sync_all()?;
                cancel.check()?;
                if pin.rename_to(&parent, OsStr::new(&leaf))? {
                    return Err(invalid(
                        "checkout final collision; no replacement permitted",
                    ));
                }
                stage.sync()?;
                parent.sync()?;
                drop(pin);
            }
            (None, Some(observed)) => {
                require_inode(&observed, inode)?;
                drop(observed);
                let pin = parent.open_file_for_sync(OsStr::new(&leaf))?;
                require_inode(&pin, inode)?;
                verify_bytes(&pin, file, cancel)?;
                pin.file().sync_all()?;
                stage.sync()?;
                parent.sync()?;
                drop(pin);
            }
            _ => {
                return Err(invalid(
                    "checkout Publishing requires exactly temporary or final inode",
                ));
            }
        }
        let final_pin = parent.open_readonly_file(OsStr::new(&leaf))?;
        require_inode(&final_pin, inode)?;
        verify_bytes(&final_pin, file, cancel)?;
        if optional(stage.open_readonly_file(OsStr::new(&name)))?.is_some() {
            return Err(invalid("checkout temporary name survived publication"));
        }
        self.verify_binding(expected, cancel)?;
        row.published_file()
    }

    pub(in crate::projection::provider) fn verify_complete(
        self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        cancel: &Cancellation,
    ) -> Result<VerifiedCheckout> {
        self.require_namespace(expected, journal, cancel)?;
        if !journal.is_complete() {
            return Err(invalid("checkout complete proof requires all Published"));
        }
        let mut files = Vec::with_capacity(expected.files().len());
        for (ordinal, file) in expected.files().iter().enumerate() {
            cancel.check()?;
            let (_, _, state) = Self::file_row(expected, journal, ordinal as u32)?;
            let FileState::Published {
                inode,
                parent_identity,
            } = state
            else {
                return Err(invalid("checkout final file is not Published"));
            };
            let (parent, leaf) = self.parent(expected, journal, file.path.as_str())?;
            if parent.identity_token() != *parent_identity {
                return Err(invalid("checkout complete parent differs"));
            }
            let pin = parent.open_immutable_file(OsStr::new(&leaf))?;
            require_inode(&pin, inode)?;
            verify_bytes(&pin, file, cancel)?;
            files.push((ordinal as u32, pin));
        }
        let proof = VerifiedCheckout {
            storage: self,
            files,
        };
        proof.require(expected, journal, cancel)?;
        Ok(proof)
    }

    /// Exhaustive bounded listing at the existing pins-before/after guarantee.
    /// Enumeration is observation only; children are reopened relative to pins
    /// and checked against durable ownership. No reparse child is traversed.
    pub(in crate::projection::provider) fn require_namespace(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.require(expected, journal, cancel)?;
        let (allowed, alternatives) = expected_nodes(expected, journal, cancel)?;
        let base = PathBuf::from(&expected.marker().checkout_root);
        let root = PinnedDirectory::open(&base)?;
        if root.identity_token() != self.header.checkout_identity {
            return Err(invalid("checkout namespace root differs"));
        }
        let mut pending = vec![(root, String::new())];
        let mut seen = BTreeSet::new();
        let mut bytes = 0usize;
        while let Some((pin, relative)) = pending.pop() {
            cancel.check()?;
            pin.verify_path_identity()?;
            for entry in fs::read_dir(base.join(&relative))? {
                cancel.check()?;
                let name = entry?.file_name();
                let leaf = name
                    .to_str()
                    .ok_or_else(|| invalid("checkout namespace name is not Unicode"))?;
                let path = if relative.is_empty() {
                    leaf.to_owned()
                } else {
                    format!("{relative}/{leaf}")
                };
                bytes = bytes
                    .checked_add(path.len())
                    .filter(|n| *n <= 64 * 1024 * 1024)
                    .ok_or_else(|| invalid("checkout namespace byte budget exceeded"))?;
                if seen.len() >= allowed.len() || !seen.insert(path.clone()) {
                    return Err(invalid(
                        "checkout namespace entry count or duplicate differs",
                    ));
                }
                let node = allowed
                    .get(&path)
                    .ok_or_else(|| invalid("checkout namespace has unowned entry"))?;
                match node {
                    Node::Directory { identity, private } => {
                        let child = if *private {
                            pin.open_private_child(&name)?
                        } else {
                            pin.open_child_for_traversal(&name)?
                        };
                        if child.identity_token() != *identity {
                            return Err(invalid("checkout namespace directory inode differs"));
                        }
                        pending.push((child, path));
                    }
                    Node::File {
                        inode,
                        bytes,
                        partial,
                        parent_identity,
                    } => {
                        if parent_identity
                            .as_ref()
                            .is_some_and(|id| *id != pin.identity_token())
                        {
                            return Err(invalid("checkout namespace file parent differs"));
                        }
                        let child = pin.open_readonly_file(&name)?;
                        require_inode(&child, inode)?;
                        let length = child.file().metadata()?.len();
                        if (*partial && length > *bytes) || (!*partial && length != *bytes) {
                            return Err(invalid("checkout namespace file length differs"));
                        }
                    }
                }
            }
            pin.verify_path_identity()?;
        }
        for (path, _) in &allowed {
            cancel.check()?;
            if !seen.contains(path)
                && !alternatives
                    .get(path)
                    .is_some_and(|other| seen.contains(other))
            {
                return Err(invalid("checkout namespace owned entry missing"));
            }
        }
        for (ordinal, file) in expected.files().iter().enumerate() {
            cancel.check()?;
            let (_, _, state) = Self::file_row(expected, journal, ordinal as u32)?;
            if matches!(state, FileState::Publishing { .. }) {
                let temp = format!(
                    "{}/{}",
                    expected.stage_relative_path(),
                    expected.temporary_name(ordinal)?
                );
                if seen.contains(file.path.as_str()) == seen.contains(&temp) {
                    return Err(invalid("checkout Publishing has both or neither name"));
                }
            }
        }
        self.verify_binding(expected, cancel)
    }
}

impl VerifiedCheckout {
    /// No byte hash is needed again: retained immutable handles exclude normal
    /// write/delete opens. Namespace pins are still checked before and after.
    pub(in crate::projection::provider) fn require(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        cancel: &Cancellation,
    ) -> Result<()> {
        if !journal.is_complete() || self.files.len() != expected.files().len() {
            return Err(invalid("checkout proof lacks complete journal"));
        }
        self.storage.require_namespace(expected, journal, cancel)?;
        self.require_identity(expected, journal, cancel)
    }

    /// Recheck only retained identities. Callers must separately run the full
    /// namespace check outside catalog gates, before and after positive closes.
    pub(in crate::projection::provider) fn require_identity(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        cancel: &Cancellation,
    ) -> Result<()> {
        if !journal.is_complete() || self.files.len() != expected.files().len() {
            return Err(invalid("checkout proof lacks complete journal"));
        }
        for (ordinal, pin) in &self.files {
            cancel.check()?;
            let (_, file, state) = CheckoutStorage::file_row(expected, journal, *ordinal)?;
            let FileState::Published {
                inode,
                parent_identity,
            } = state
            else {
                return Err(invalid("checkout proof file is not Published"));
            };
            require_inode(pin, inode)?;
            let (parent, leaf) = self.storage.parent(expected, journal, file.path.as_str())?;
            if parent.identity_token() != *parent_identity
                || !pin.same_file(&parent.open_readonly_file(OsStr::new(&leaf))?)
            {
                return Err(invalid("checkout proof final path replaced"));
            }
        }
        self.storage.verify_binding(expected, cancel)
    }
}

fn optional<T>(result: std::io::Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn require_inode(pin: &PinnedFile, inode: &str) -> Result<()> {
    pin.verify_parent()?;
    file::require_single_link(pin.file())?;
    if file::identity(pin.file())? != inode {
        return Err(invalid("checkout file inode differs from durable journal"));
    }
    Ok(())
}
fn verify_bytes(pin: &PinnedFile, expected: &CheckoutFile, cancel: &Cancellation) -> Result<()> {
    cancel.check()?;
    file::require_single_link(pin.file())?;
    if expected.executable || pin.file().metadata()?.len() != expected.bytes {
        return Err(invalid("checkout file length or Windows mode differs"));
    }
    let mut reader = pin.file();
    reader.seek(SeekFrom::Start(0))?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-file-v1\0");
    let mut scratch = [0u8; 64 * 1024];
    let mut remaining = expected.bytes;
    while remaining != 0 {
        cancel.check()?;
        let count = remaining.min(scratch.len() as u64) as usize;
        reader.read_exact(&mut scratch[..count])?;
        hash.update(&scratch[..count]);
        remaining -= count as u64;
    }
    if reader.read(&mut [0u8; 1])? != 0
        || format!("blake3:{}", hash.finalize()) != expected.file_id.as_str()
        || pin.file().metadata()?.len() != expected.bytes
    {
        return Err(invalid("checkout exact FileId bytes differ"));
    }
    pin.verify_parent()?;
    file::require_single_link(pin.file())?;
    cancel.check()?;
    Ok(())
}

enum Node {
    Directory {
        identity: String,
        private: bool,
    },
    File {
        inode: String,
        bytes: u64,
        partial: bool,
        parent_identity: Option<String>,
    },
}
fn expected_nodes(
    expected: &ExpectedCheckout,
    journal: &CheckoutJournal,
    cancel: &Cancellation,
) -> Result<(BTreeMap<String, Node>, BTreeMap<String, String>)> {
    let mut nodes = BTreeMap::new();
    let mut alternatives = BTreeMap::new();
    nodes.insert(
        ".layrs".into(),
        Node::Directory {
            identity: expected.header().control_identity.clone(),
            private: true,
        },
    );
    nodes.insert(
        ".layrs/FORMAT".into(),
        Node::File {
            inode: expected.header().format_file_identity.clone(),
            bytes: expected
                .marker()
                .bytes()
                .map_err(|e| invalid(e.to_string()))?
                .len() as u64,
            partial: false,
            parent_identity: Some(expected.header().control_identity.clone()),
        },
    );
    match &journal.row(RowKey::Stage)?.entry {
        CheckoutEntry::Stage {
            state: DirectoryState::Owned { identity },
        } => {
            nodes.insert(
                expected.stage_relative_path(),
                Node::Directory {
                    identity: identity.clone(),
                    private: true,
                },
            );
        }
        CheckoutEntry::Stage {
            state: DirectoryState::Declared,
        } => {}
        _ => return Err(invalid("checkout namespace stage row differs")),
    }
    for (ordinal, path) in expected.directories().iter().enumerate() {
        cancel.check()?;
        match &journal.row(RowKey::Directory(ordinal as u32))?.entry {
            CheckoutEntry::Directory {
                path: recorded,
                state,
                ..
            } if recorded == path => {
                if let DirectoryState::Owned { identity } = state {
                    nodes.insert(
                        path.as_str().into(),
                        Node::Directory {
                            identity: identity.clone(),
                            private: false,
                        },
                    );
                }
            }
            _ => return Err(invalid("checkout namespace directory row differs")),
        }
    }
    for (ordinal, file) in expected.files().iter().enumerate() {
        cancel.check()?;
        let (_, _, state) = CheckoutStorage::file_row(expected, journal, ordinal as u32)?;
        let temp = format!(
            "{}/{}",
            expected.stage_relative_path(),
            expected.temporary_name(ordinal)?
        );
        if let Some(inode) = state.inode() {
            if !matches!(state, FileState::Published { .. }) {
                nodes.insert(
                    temp.clone(),
                    Node::File {
                        inode: inode.into(),
                        bytes: file.bytes,
                        partial: matches!(state, FileState::Owned { .. }),
                        parent_identity: None,
                    },
                );
            }
            if matches!(
                state,
                FileState::Publishing { .. } | FileState::Published { .. }
            ) {
                nodes.insert(
                    file.path.as_str().into(),
                    Node::File {
                        inode: inode.into(),
                        bytes: file.bytes,
                        partial: false,
                        parent_identity: match state {
                            FileState::Publishing {
                                parent_identity, ..
                            }
                            | FileState::Published {
                                parent_identity, ..
                            } => Some(parent_identity.clone()),
                            _ => None,
                        },
                    },
                );
            }
            if matches!(state, FileState::Publishing { .. }) {
                alternatives.insert(temp.clone(), file.path.as_str().into());
                alternatives.insert(file.path.as_str().into(), temp);
            }
        }
    }
    Ok((nodes, alternatives))
}
