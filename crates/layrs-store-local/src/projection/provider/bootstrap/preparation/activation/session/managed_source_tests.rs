//! One fresh owned native recipe for retained managed-image byte copying.
use super::*;
use crate::projection::provider::{
    bootstrap::tests::capture_family::active_holder_with_writable_file_private,
    process_profile::tests::child,
};
use layrs_core::scm::FileId;
use std::fs;

#[test]
#[ignore = "UNQUALIFIED until ROOT releases this exact owned native managed-source recipe"]
fn retained_native_reader_copies_exact_manifest_then_rejects_namespace_change() {
    child::run(
        concat!(
            module_path!(),
            "::retained_native_reader_copies_exact_manifest_then_rejects_namespace_change"
        ),
        240,
        || {
            child::declare();
            let (fixture, holder, _, _, _) = active_holder_with_writable_file_private();
            let mut session = holder
                .into_disconnected_session()
                .unwrap_or_else(|error| panic!("active session withheld: {}", error.cause()));
            session.connect().unwrap();

            let path = fixture.checkout.join("README.md");
            assert!(
                path.exists(),
                "fixture installed the exact base placeholder"
            );
            if fs::remove_file(&path).is_err() {
                let mut permissions = fs::metadata(&path).unwrap().permissions();
                permissions.set_readonly(false);
                fs::set_permissions(&path, permissions).unwrap();
                fs::remove_file(&path).unwrap();
            }
            let replacement = vec![b'v'; 80 * 1024];
            fs::write(&path, &replacement).unwrap();

            // Native owner/source validation finishes under scoped locks before
            // the retained export checkpoint performs any content read.
            let active = {
                let held = &session.source.held;
                let _locks = held
                    .family
                    .locks(
                        &held.artifacts.allocation.format.identity.worktree_id,
                        false,
                    )
                    .unwrap();
                capture_state::read(
                    held,
                    Some(&session.source.root),
                    Some(&Cancellation::default()),
                )
                .unwrap_or_else(|error| panic!("active source read: {}", error.cause))
            };
            let mut checkpoint = session.session.export_checkpoint().unwrap();
            let cancel = Cancellation::default();
            let mut image = super::super::managed_image::plan(
                &mut checkpoint,
                session.source.as_ref(),
                &active.active.source,
                &active.active.activated.snapshot,
                &cancel,
            )
            .unwrap();
            image
                .require_owner(
                    &session.source.root,
                    &active.active.source,
                    &active.active.activated.snapshot,
                )
                .unwrap();
            assert_eq!(image.files().len(), 1);
            let file = &image.files()[0];
            let file_path = file.path.clone();
            let manifest = file.manifest.manifest().clone();
            assert_eq!(file_path.as_str(), "README.md");
            assert_eq!(manifest.file_id(), &FileId::from_bytes(&replacement));
            let base = &active.active.source.descriptors()[0];
            assert_eq!(
                file.expected.as_ref(),
                Some(&(base.file_id.clone(), base.executable))
            );

            let mut copied = Vec::new();
            image
                .copy_exact_file(&file_path, &manifest, &mut copied, &cancel)
                .unwrap();
            assert_eq!(copied, replacement);
            let digest = image.digest().to_owned();
            image.revalidate_and_match_digest(&digest, &cancel).unwrap();

            let unknown = fixture.checkout.join("unknown.txt");
            fs::write(&unknown, b"outside the fixed image").unwrap();
            let mut refused = Vec::new();
            assert!(image
                .copy_exact_file(&file_path, &manifest, &mut refused, &cancel)
                .is_err());
            assert!(refused.is_empty(), "namespace refusal precedes output");
            assert_eq!(fs::read(&path).unwrap(), replacement);
            assert_eq!(fs::read(&unknown).unwrap(), b"outside the fixed image");
            drop(image);
            drop(checkpoint);
            session.disconnect().unwrap();
        },
    );
}
