//! Native managed-image adapter recipes. Each case owns a fresh provider root
//! and retains the real export checkpoint through the image verification.
use super::*;
use crate::projection::provider::{
    bootstrap::tests::capture_family::active_holder_with_writable_file_private,
    process_profile::tests::child, recovery::error::ProviderBootstrapCause,
};
use layrs_core::scm::{Chunk, ChunkingDescriptor, FileId, FileManifest};
use std::fs;

macro_rules! owned {
    ($name:ident, $body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned native managed-image recipe"]
        fn $name() {
            child::run(
                concat!(module_path!(), "::", stringify!($name)),
                240,
                || {
                    child::declare();
                    $body
                },
            );
        }
    };
}

fn remove_placeholder(path: &Path) {
    if fs::remove_file(path).is_err() {
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).unwrap();
        fs::remove_file(path).unwrap();
    }
}

fn read_active(session: &ProviderActiveSession) -> validation::PrivateActiveCaptureValidation {
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
    .unwrap_or_else(|error| panic!("active read refused: {}", error.cause))
}

owned!(
    writable_replacement_has_exact_manifest_and_refuses_later_unknown_path,
    {
        let (fixture, holder, _, _, _) = active_holder_with_writable_file_private();
        let mut session = holder
            .into_disconnected_session()
            .unwrap_or_else(|error| panic!("active session withheld: {}", error.cause()));
        session.connect().unwrap();

        let path = fixture.checkout.join("README.md");
        assert!(path.exists(), "writable source placeholder is installed");
        remove_placeholder(&path); // no cold base hydration
        let replacement = vec![b'm'; 80 * 1024];
        fs::write(&path, &replacement).unwrap();

        // The complete private/family read and its scoped catalog locks finish
        // before any native file hashing under the retained export checkpoint.
        let active = read_active(&session);
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
        assert_eq!(image.files().len(), 1);
        assert!(image.deletions().is_empty());
        let file = &image.files()[0];
        assert_eq!(file.path.as_str(), "README.md");
        assert_eq!(
            file.manifest.manifest().file_id(),
            &FileId::from_bytes(&replacement)
        );
        assert_eq!(
            file.manifest.manifest().byte_length(),
            replacement.len() as u64
        );
        let expected_manifest = FileManifest::from_chunks(
            ChunkingDescriptor::deterministic_fixed(1024 * 1024).unwrap(),
            &[Chunk::new(replacement.clone()).unwrap()],
        )
        .unwrap();
        assert_eq!(file.manifest.manifest(), &expected_manifest);
        let base = active
            .active
            .source
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.path.as_str() == "README.md")
            .unwrap();
        assert_eq!(
            file.expected.as_ref(),
            Some(&(base.file_id.clone(), base.executable))
        );
        let digest = image.digest().to_owned();
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        image.revalidate_and_match_digest(&digest, &cancel).unwrap();

        let unknown = fixture.checkout.join("unknown.txt");
        fs::write(&unknown, b"not in fixed source").unwrap();
        assert!(image.revalidate_and_match_digest(&digest, &cancel).is_err());
        assert_eq!(fs::read(&path).unwrap(), replacement);
        assert_eq!(fs::read(&unknown).unwrap(), b"not in fixed source");
        drop(image);
        drop(checkpoint);
        session.disconnect().unwrap();
    }
);

owned!(cold_immutable_source_refuses_missing_bytes, {
    let (fixture, holder, _, _, _) = active_holder_with_writable_file_private();
    let mut session = holder
        .into_disconnected_session()
        .unwrap_or_else(|error| panic!("active session withheld: {}", error.cause()));
    session.connect().unwrap();
    let path = fixture.checkout.join("README.md");
    assert!(
        path.exists(),
        "cold immutable source placeholder is installed"
    );
    let active = read_active(&session);
    assert!(
        active.active.source.descriptors()[0].bytes > 0,
        "cold bytes must actually be required"
    );
    let mut checkpoint = session.session.export_checkpoint().unwrap();
    let error = match super::super::managed_image::plan(
        &mut checkpoint,
        session.source.as_ref(),
        &active.active.source,
        &active.active.activated.snapshot,
        &Cancellation::default(),
    ) {
        Ok(_) => panic!("cold source cannot produce a verified managed image"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        ProviderBootstrapCause::Vfs(layrs_vfs::Error::MissingBytes { .. })
    ));
    assert!(path.exists(), "refusal preserves the original placeholder");
    drop(checkpoint);
    session.disconnect().unwrap();
});
