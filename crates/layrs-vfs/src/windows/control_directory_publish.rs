//! One-attempt publication of an independently allocated immutable FORMAT.
use super::{
    control_directory::{
        ControlDirectoryGuard, ControlRootPreflight, check_directory, check_format, match_root,
        validate_parent,
    },
    control_directory_io as io, fs,
};
use crate::{
    ControlDirectoryFailure as Failure, ControlDirectoryIdentity, ControlDirectoryPublishFaults,
    ControlDirectoryPublishPoint as Point, ControlDirectoryStageIdentity, Error, Result,
    control_directory::MAX_FORMAT_BYTES,
};
use layrs_core::scm::FileId;
use std::{fs::File, path::Path};

pub(super) fn failure(
    root: &Path,
    stage: &Path,
    kind: Failure,
    point: Point,
    operation: &'static str,
    cause: Error,
) -> Error {
    Error::ControlDirectory {
        kind,
        point,
        operation,
        root: root.to_owned(),
        stage: stage.to_owned(),
        cause: Box::new(cause),
    }
}
struct Context<'a> {
    root: &'a Path,
    stage: &'a Path,
    faults: &'a dyn ControlDirectoryPublishFaults,
}
impl Context<'_> {
    fn checked<T>(&self, point: Point, operation: &'static str, result: Result<T>) -> Result<T> {
        result.map_err(|cause| {
            failure(
                self.root,
                self.stage,
                Failure::NeedsRepair,
                point,
                operation,
                cause,
            )
        })
    }
    fn fault(&self, point: Point) -> Result<()> {
        self.faults.at(point).map_err(|cause| {
            failure(
                self.root,
                self.stage,
                Failure::Interrupted,
                point,
                "control publication fault point",
                cause.into(),
            )
        })
    }
}

/// Publish the caller's exact allocated stage. The caller authenticates its
/// allocation/prepared receipts, finishes and syncs FORMAT, and releases ALL its
/// FORMAT handles before calling. This function writes neither bytes nor receipts.
/// Pass `allocated.stage()` as the canonical exact `staged_control` argument.
///
/// `allocated` is mandatory even before a prepared receipt exists. Path+matching
/// bytes never replace allocation identity. `prepared` suppresses `persist` in
/// every branch. A persist error has UNKNOWN durability; reread the Store receipt.
///
/// The sole DELETE directory pin remains owned across the close window. There is
/// at most one no-replace rename attempt per call; no internal retry or cleanup.
/// Raw access denied is RenameRefused, not evidence of a reader or a generic Busy.
/// A later explicit attempt must bind the retained stage again. After process
/// exit with a final directory, use fresh ControlDirectoryGuard::open followed by
/// revalidate_bootstrap_root; a fresh empty-root preflight is then inappropriate.
///
/// Absolute destination resolution uses retained ancestors and post-rename IDs.
/// This is not atomic namespace, a registration lock or a power-loss guarantee.
/// Any NeedsRepair result retains files and grants no cleanup/repair authority;
/// even a transient sharing error retains its exact original native cause.
pub fn publish_control_directory_recoverable(
    preflight: &ControlRootPreflight,
    staged_control: &Path,
    allocated: &ControlDirectoryStageIdentity,
    format_bytes: &[u8],
    prepared: Option<&ControlDirectoryIdentity>,
    persist: impl FnOnce(&ControlDirectoryIdentity) -> Result<()>,
    faults: &dyn ControlDirectoryPublishFaults,
) -> Result<ControlDirectoryGuard> {
    let context = Context {
        root: preflight.root(),
        stage: staged_control,
        faults,
    };
    context.checked(
        Point::StageValidated,
        "validate allocation data",
        allocated.validate(),
    )?;
    let input_path = context.checked(
        Point::StageValidated,
        "validate explicit stage path",
        io::normalized(staged_control),
    )?;
    let valid = input_path == allocated.stage
        && !format_bytes.is_empty()
        && format_bytes.len() <= MAX_FORMAT_BYTES;
    context.checked(
        Point::StageValidated,
        "bind allocation path/bytes",
        if valid {
            Ok(())
        } else {
            Err(Error::IdentityMismatch)
        },
    )?;
    context.checked(
        Point::StageValidated,
        "bind preflight root",
        match_root(&preflight.root, allocated),
    )?;
    context.checked(
        Point::StageValidated,
        "preflight registration boundary",
        preflight.revalidate(),
    )?;
    let expected = ControlDirectoryIdentity {
        stage: allocated.clone(),
        size: format_bytes.len() as u32,
        content: FileId::from_bytes(format_bytes),
    };
    if prepared.is_some_and(|prepared| prepared != &expected) {
        return context.checked(
            Point::StageValidated,
            "bind prepared identity to allocation",
            Err(Error::IdentityMismatch),
        );
    }
    let final_path = preflight.root().join(".layrs");
    let exists = match std::fs::symlink_metadata(&final_path) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return context.checked(
                Point::StageValidated,
                "observe final path",
                Err(error.into()),
            );
        }
    };
    if exists {
        if prepared.is_none() {
            return context.checked(
                Point::BeforeProof,
                "final requires durable prepared identity",
                Err(Error::ForeignRoot),
            );
        }
        // Fresh acquisition; no live stage handle is needed and no historical
        // stage/parent path is opened. The final must still be bootstrap-only.
        let guard = context.checked(
            Point::BeforeProof,
            "reopen final control",
            ControlDirectoryGuard::open(preflight.root(), &expected),
        )?;
        context.checked(
            Point::BeforeProof,
            "validate bootstrap replay",
            guard.revalidate_bootstrap_root(),
        )?;
        context.fault(Point::BeforeProof)?;
        context.checked(
            Point::BeforeProof,
            "last bootstrap replay boundary",
            guard.revalidate_bootstrap_root(),
        )?;
        return Ok(guard);
    }
    context.checked(
        Point::StageValidated,
        "require empty bootstrap root",
        io::empty(preflight.root()),
    )?;
    let parent = context.checked(
        Point::StageValidated,
        "pin exact stage parent",
        io::RootPins::open(allocated.stage_parent(), false),
    )?;
    context.checked(
        Point::StageValidated,
        "validate private stage ancestry",
        validate_parent(preflight, &parent),
    )?;
    if parent.root != allocated.stage_parent()
        || parent.identity() != (allocated.volume, allocated.parent_index)
    {
        return context.checked(
            Point::StageValidated,
            "bind stage parent",
            Err(Error::IdentityMismatch),
        );
    }
    // Compare allocation directory identity before requesting the mutation pin.
    {
        let observed = context.checked(
            Point::StageValidated,
            "observe allocated stage",
            io::directory_observer(staged_control),
        )?;
        context.checked(
            Point::StageValidated,
            "verify allocated stage type",
            io::not_under(&preflight.api, &observed)
                .and_then(|_| io::control_directory_attributes(&observed)),
        )?;
        context.checked(
            Point::StageValidated,
            "verify allocated stage privacy",
            preflight.privacy.verify(&observed),
        )?;
        if context.checked(
            Point::StageValidated,
            "read stage native identity",
            io::id(&observed),
        )? != (allocated.volume, allocated.directory_index)
            || context.checked(
                Point::StageValidated,
                "read stage canonical path",
                io::canonical(&observed),
            )? != allocated.stage
        {
            return context.checked(
                Point::StageValidated,
                "bind allocated stage",
                Err(Error::IdentityMismatch),
            );
        }
        context.checked(
            Point::StageValidated,
            "validate allocated stage layout",
            io::only_child(staged_control, "FORMAT"),
        )?;
        // Observe registration/type/identity without data access first. This
        // transient metadata handle never survives the close window.
        let format = context.checked(
            Point::StageValidated,
            "observe allocated FORMAT",
            fs::open(
                &staged_control.join("FORMAT"),
                io::ALL_SHARING,
                windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES
                    | super::control_directory_privacy::READ_CONTROL,
            ),
        )?;
        if context.checked(
            Point::StageValidated,
            "read allocated FORMAT identity",
            io::not_under(&preflight.api, &format).and_then(|_| {
                io::format_attributes(&format)?;
                io::id(&format)
            }),
        )? != (allocated.volume, allocated.format_index)
        {
            return context.checked(
                Point::StageValidated,
                "bind allocated FORMAT",
                Err(Error::IdentityMismatch),
            );
        }
    }
    let directory = context.checked(
        Point::StageValidated,
        "acquire sole DELETE stage pin",
        io::directory_owner(staged_control),
    )?;
    context.checked(
        Point::StageValidated,
        "validate pinned stage",
        io::not_under(&preflight.api, &directory).and_then(|_| {
            check_directory(
                &preflight.root,
                &directory,
                staged_control,
                allocated,
                &preflight.privacy,
            )
        }),
    )?;
    let format = context.checked(
        Point::StageValidated,
        "acquire strong FORMAT",
        io::format_pin(&staged_control.join("FORMAT")),
    )?;
    let bytes = context.checked(
        Point::StageValidated,
        "verify allocated FORMAT bytes",
        io::not_under(&preflight.api, &format)
            .and_then(|_| io::read_format(&format, &preflight.privacy)),
    )?;
    context.checked(
        Point::StageValidated,
        "bind FORMAT bytes/identity",
        check_format(&format, &bytes, &expected),
    )?;
    if bytes != format_bytes {
        return context.checked(
            Point::StageValidated,
            "compare FORMAT bytes",
            Err(Error::IdentityMismatch),
        );
    }
    context.checked(
        Point::StageValidated,
        "last allocation boundary",
        check_stage(preflight, &parent, &directory, &format, &expected, &bytes),
    )?;
    context.fault(Point::StageValidated)?;
    context.checked(
        Point::StageValidated,
        "last stage boundary before persistence",
        check_stage(preflight, &parent, &directory, &format, &expected, &bytes),
    )?;
    if prepared.is_none() {
        // Strong FORMAT and all directory/ancestor pins survive this callback.
        persist(&expected).map_err(|cause| {
            failure(
                preflight.root(),
                staged_control,
                Failure::ReceiptDurabilityUnknown,
                Point::AfterPersist,
                "persist control directory identity",
                cause,
            )
        })?;
    }
    context.fault(Point::AfterPersist)?;
    context.checked(
        Point::AfterPersist,
        "registration after persist",
        preflight.revalidate(),
    )?;
    context.checked(
        Point::AfterPersist,
        "stage/FORMAT registration after persist",
        check_registration(preflight, &directory, &expected),
    )?;
    context.fault(Point::PreClose)?;
    context.checked(
        Point::PreClose,
        "validate stage before close",
        check_stage(preflight, &parent, &directory, &format, &expected, &bytes),
    )?;
    context.checked(
        Point::PreClose,
        "empty root before close",
        io::empty(preflight.root()),
    )?;
    context.checked(
        Point::PreClose,
        "last registration before close",
        preflight.revalidate(),
    )?;
    drop(format); // Only FORMAT closes; directory/root/ancestor pins remain.
    context.fault(Point::PostClose)?;
    context.fault(Point::PreRename)?;
    context.checked(
        Point::PreRename,
        "registration before rename",
        preflight.revalidate(),
    )?;
    context.checked(
        Point::PreRename,
        "stage parent before rename",
        validate_parent(preflight, &parent),
    )?;
    context.checked(
        Point::PreRename,
        "stage/FORMAT registration before rename",
        check_registration(preflight, &directory, &expected),
    )?;
    if let Err(cause) = io::rename_no_replace(&directory, &final_path) {
        let collision =
            matches!(&cause, Error::Io(error) if matches!(error.raw_os_error(), Some(80 | 183)));
        return Err(failure(
            preflight.root(),
            staged_control,
            if collision {
                Failure::NeedsRepair
            } else {
                Failure::RenameRefused
            },
            Point::PreRename,
            "SetFileInformationByHandle(FileRenameInfo, no replacement)",
            cause,
        ));
    }
    context.fault(Point::PostRename)?;
    context.fault(Point::PreStrongPin)?;
    context.checked(
        Point::PreStrongPin,
        "registration after rename",
        preflight.revalidate(),
    )?;
    context.checked(
        Point::PreStrongPin,
        "verify final directory before child open",
        io::not_under(&preflight.api, &directory).and_then(|_| {
            check_directory(
                &preflight.root,
                &directory,
                &final_path,
                allocated,
                &preflight.privacy,
            )
        }),
    )?;
    let format = context.checked(
        Point::PreStrongPin,
        "acquire final strong FORMAT",
        io::format_pin(&final_path.join("FORMAT")),
    )?;
    let final_bytes = context.checked(
        Point::PreStrongPin,
        "verify final FORMAT",
        io::not_under(&preflight.api, &format)
            .and_then(|_| io::read_format(&format, &preflight.privacy)),
    )?;
    context.checked(
        Point::PreStrongPin,
        "bind final FORMAT",
        check_format(&format, &final_bytes, &expected),
    )?;
    if final_bytes != bytes {
        return context.checked(
            Point::BeforeProof,
            "compare final FORMAT bytes",
            Err(Error::IdentityMismatch),
        );
    }
    let guard = ControlDirectoryGuard {
        root: context.checked(
            Point::BeforeProof,
            "retain final root pins",
            preflight.root.duplicate(&preflight.privacy),
        )?,
        directory,
        format,
        expected,
        bytes: final_bytes,
        privacy: preflight.privacy.clone(),
    };
    context.checked(
        Point::BeforeProof,
        "validate final full layout",
        guard.revalidate_inner(),
    )?;
    context.fault(Point::BeforeProof)?;
    // Includes a final held+reopened registration boundary AFTER every final
    // open/full check/fault. No inspection or source capability is manufactured.
    context.checked(
        Point::BeforeProof,
        "last bootstrap boundary",
        guard.revalidate_bootstrap_root(),
    )?;
    Ok(guard)
}

fn check_stage(
    preflight: &ControlRootPreflight,
    parent: &io::RootPins,
    directory: &File,
    format: &File,
    expected: &ControlDirectoryIdentity,
    bytes: &[u8],
) -> Result<()> {
    preflight.revalidate()?;
    validate_parent(preflight, parent)?;
    if parent.identity() != (expected.stage.volume, expected.stage.parent_index) {
        return Err(Error::IdentityMismatch);
    }
    super::control_directory::control_registration_boundary(
        &preflight.api,
        directory,
        format,
        expected.stage.stage(),
    )?;
    check_directory(
        &preflight.root,
        directory,
        expected.stage.stage(),
        &expected.stage,
        &preflight.privacy,
    )?;
    io::format_attributes(format)?;
    preflight.privacy.verify(format)?;
    io::stream_inventory(format, false)?;
    if io::id(format)? != (expected.stage.volume, expected.stage.format_index) {
        return Err(Error::IdentityMismatch);
    }
    let current = io::format_pin(&expected.stage.stage().join("FORMAT"))?;
    let current_bytes = io::read_format(&current, &preflight.privacy)?;
    check_format(&current, &current_bytes, expected)?;
    if current_bytes != bytes || fs::size(&fs::identity(format)?) != expected.size as u64 {
        return Err(Error::IdentityMismatch);
    }
    super::control_directory::control_registration_boundary(
        &preflight.api,
        directory,
        format,
        expected.stage.stage(),
    )?;
    preflight.revalidate()
}

fn check_registration(
    preflight: &ControlRootPreflight,
    directory: &File,
    expected: &ControlDirectoryIdentity,
) -> Result<()> {
    let format = fs::open(
        &expected.stage.stage().join("FORMAT"),
        io::ALL_SHARING,
        windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES
            | super::control_directory_privacy::READ_CONTROL,
    )?;
    if io::id(&format)? != (expected.stage.volume, expected.stage.format_index) {
        return Err(Error::IdentityMismatch);
    }
    super::control_directory::control_registration_boundary(
        &preflight.api,
        directory,
        &format,
        expected.stage.stage(),
    )
}
