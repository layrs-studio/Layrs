//! A real Accepted capture remains readable while its published CAS files are
//! held against write/delete sharing. This does not finalize a managed import.
use super::*;
use crate::{
    projection::provider::{
        bootstrap::tests::capture_family::active_holder_with_writable_file_private, database_io,
        process_profile::tests::child, records::capture::ProviderCaptureState,
    },
    secure_fs::PinnedDirectory,
};
use layrs_core::scm::{canonical_json, FileId, FileManifest};
use std::{collections::BTreeMap, ffi::OsStr, fs, io::Read, path::PathBuf};

#[test]
#[ignore = "UNQUALIFIED until ROOT releases this exact owned Accepted-capture immutable-reader recipe"]
fn accepted_capture_catalog_reads_under_live_immutable_cas_pins() {
    child::run(
        concat!(
            module_path!(),
            "::accepted_capture_catalog_reads_under_live_immutable_cas_pins"
        ),
        240,
        || {
            child::declare();
            let (fixture, holder, _, _, _) = active_holder_with_writable_file_private();
            let mut session = holder
                .into_disconnected_session()
                .unwrap_or_else(|e| panic!("active session withheld: {}", e.cause()));
            session.connect().unwrap();
            let native = fixture.checkout.join("README.md");
            assert!(native.exists());
            if fs::remove_file(&native).is_err() {
                let mut permissions = fs::metadata(&native).unwrap().permissions();
                permissions.set_readonly(false);
                fs::set_permissions(&native, permissions).unwrap();
                fs::remove_file(&native).unwrap();
            }
            let replacement = b"accepted capture retained by immutable CAS pins";
            fs::write(&native, replacement).unwrap();
            let cancel = Cancellation::default();
            let accepted = session
                .capture("accepted-immutable-readers", &cancel)
                .unwrap();
            assert_eq!(accepted.patch.changes.len(), 1);
            assert_eq!(accepted.patch.sequence, 1);
            let target = accepted.patch.changes[0].replacement.as_ref().unwrap();
            assert_eq!(target.file_id, FileId::from_bytes(replacement));
            assert_eq!(target.bytes, replacement.len() as u64);
            let held = &session.source.held;
            let proof = {
                let _locks = held
                    .family
                    .locks(
                        &held.artifacts.allocation.format.identity.worktree_id,
                        false,
                    )
                    .unwrap();
                capture_state::read(held, Some(&session.source.root), Some(&cancel)).unwrap_or_else(
                    |e| panic!("Accepted private/family proof refused: {}", e.cause),
                )
            };
            assert_eq!(proof.capture.transactions.len(), 1);
            assert_eq!(
                proof.capture.content.len(),
                1,
                "a descriptor-only fixture does not exercise captured content"
            );
            assert_eq!(proof.capture.claims.len(), 1);
            assert_eq!(proof.capture.receipts.len(), 1);
            assert_eq!(proof.capture.patches.len(), 1);
            let ProviderCaptureState::Accepted { patch, .. } = &proof.capture.transactions[0].state
            else {
                panic!("native capture must have committed real Accepted history")
            };
            assert_eq!(*patch, accepted.patch);
            assert_eq!(proof.capture.content[0].file_id, target.file_id.as_str());

            let objects = PinnedDirectory::open(fixture.family.root())
                .unwrap()
                .open_child_for_traversal(OsStr::new("objects"))
                .unwrap();
            let manifests = objects
                .open_child_for_traversal(OsStr::new("manifests"))
                .unwrap();
            let chunks = objects
                .open_child_for_traversal(OsStr::new("chunks"))
                .unwrap();
            let mapping = &proof.capture.content[0];
            let manifest_name = mapping.manifest_id.strip_prefix("blake3:").unwrap();
            let manifest = {
                let pin = manifests
                    .open_readonly_file(OsStr::new(manifest_name))
                    .unwrap();
                assert!(pin.file().metadata().unwrap().len() <= 262144);
                let mut payload = Vec::new();
                let mut reader = pin.file();
                reader.read_to_end(&mut payload).unwrap();
                let manifest: FileManifest = serde_json::from_slice(&payload).unwrap();
                assert_eq!(canonical_json(&manifest).unwrap(), payload);
                assert_eq!(manifest.id().as_str(), mapping.manifest_id);
                assert_eq!(manifest.file_id().as_str(), mapping.file_id);
                assert_eq!(manifest.byte_length(), mapping.bytes);
                manifest
            };
            assert_eq!(manifest.chunks().len(), 1);

            let mut paths = vec![
                fixture.metadata.join("projection.sqlite3"),
                fixture.family.root().join("metadata/catalog.sqlite3"),
                fixture
                    .family
                    .root()
                    .join("objects/manifests")
                    .join(manifest_name),
            ];
            let mut pinned = Vec::new();
            // The legacy open asks for DELETE. It succeeds before the immutable
            // pin and must conflict after it: this isolates the original P2.
            drop(manifests.open_file(OsStr::new(manifest_name)).unwrap());
            let pin = manifests
                .open_immutable_file(OsStr::new(manifest_name))
                .unwrap();
            let conflict = manifests
                .open_file(OsStr::new(manifest_name))
                .err()
                .expect("DELETE access must conflict with live immutable pin");
            assert_eq!(
                conflict.raw_os_error(),
                Some(32),
                "actual Windows sharing violation required"
            );
            pinned.push((true, manifest_name.to_owned(), pin));
            for part in manifest.chunks() {
                let name = part.chunk_id().as_str().strip_prefix("blake3:").unwrap();
                drop(chunks.open_file(OsStr::new(name)).unwrap());
                let pin = chunks.open_immutable_file(OsStr::new(name)).unwrap();
                let conflict = chunks
                    .open_file(OsStr::new(name))
                    .err()
                    .expect("DELETE chunk access must conflict with live immutable pin");
                assert_eq!(conflict.raw_os_error(), Some(32));
                pinned.push((false, name.to_owned(), pin));
                paths.push(fixture.family.root().join("objects/chunks").join(name));
            }
            let witness = || -> BTreeMap<PathBuf, (String, u64, String)> {
                paths
                    .iter()
                    .map(|p| (p.clone(), database_io::tests::support::witness(p)))
                    .collect()
            };
            let before = witness();
            let validated = {
                let _locks = held
                    .family
                    .locks(
                        &held.artifacts.allocation.format.identity.worktree_id,
                        false,
                    )
                    .unwrap();
                let state = database_io::read_cancellable(&held.family.catalog, &cancel, |c| {
                    assert!(
                        !c.is_autocommit(),
                        "caller retains the family read transaction"
                    );
                    assert!(c.is_readonly(rusqlite::DatabaseName::Main).unwrap());
                    held.family.validate_capture_catalog(
                        c,
                        &held.artifacts.allocation,
                        &proof.capture,
                        Some(&cancel),
                    )
                })
                .unwrap();
                // This DATA is returned only after the observable RO close has
                // succeeded. No writer, cleanup or catalogue repair is used.
                super::super::managed_destination::valid(state).unwrap()
            };
            assert_eq!(validated, ActivationRegistration::Ready);
            for (is_manifest, name, pin) in &pinned {
                let parent = if *is_manifest { &manifests } else { &chunks };
                pin.verify_parent().unwrap();
                assert!(pin.same_file(&parent.open_readonly_file(OsStr::new(name)).unwrap()));
            }
            assert_eq!(
                witness(),
                before,
                "Accepted history validation is read-only under retained immutable pins"
            );
            // Release the deliberately restrictive pins only after validation,
            // close acknowledgement and all physical witnesses were checked.
            drop(pinned);
            drop(chunks);
            drop(manifests);
            drop(objects);
            session.disconnect().unwrap();
        },
    );
}
