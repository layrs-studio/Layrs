use super::*;
use crate::{lock::SessionMode, projection::provider::owned_test as child};
use layrs_core::scm::{Chunk, ChunkingDescriptor};
use std::{collections::BTreeMap, fs, path::Path};

fn open(root: &Path, binding: &ExactBinding) -> ExactStorage {
    ExactStorage::open(
        root,
        &binding.family_root_identity,
        StoreLock::acquire_existing(&root.join("lifecycle.lock"), SessionMode::Shared).unwrap(),
    )
    .unwrap()
}

#[test]
#[ignore = "owned audited filesystem child; read-only CAS proof exact representations and corrupt byte refusal"]
fn published_cas_verifies_exact_union_without_staging_and_refuses_corruption() {
    child::run(
        concat!(
            module_path!(),
            "::published_cas_verifies_exact_union_without_staging_and_refuses_corruption"
        ),
        120,
        || {
            child::declare();
            let root = child::owned_root("published-cas");
            PinnedDirectory::open(&root)
                .unwrap()
                .ensure_private_child(OsStr::new("staging"))
                .unwrap();
            for relative in ["objects", "objects/chunks", "objects/manifests"] {
                fs::create_dir(root.join(relative)).unwrap();
            }
            child::create(&root.join("lifecycle.lock"), b"");
            let digest = format!("blake3:{}", "1".repeat(64));
            let operation = format!("provider-transition:{}", "2".repeat(64));
            let binding = ExactBinding {
                family_incarnation: "family".into(),
                family_root_identity: PinnedDirectory::open(&root).unwrap().identity_token(),
                family_marker_digest: digest.clone(),
                family_catalog_identity: "1:0:3".into(),
                plan_digest: digest.clone(),
                prefix_id: digest.clone(),
                destination_proof_digest: digest,
                transaction_id: "0".repeat(32),
                owner: WorktreeId::parse("worktree:cas-proof").unwrap(),
                database_identity: "1:0:4".into(),
                reservation: operation.clone(),
                operation,
                quota_bytes: 1024 * 1024,
            };
            let chunks = [
                Chunk::new(b"abc".to_vec()).unwrap(),
                Chunk::new(b"abcabc".to_vec()).unwrap(),
            ];
            let required = [
                FileManifest::from_chunks(
                    ChunkingDescriptor::deterministic_fixed(3).unwrap(),
                    &[chunks[0].clone(), chunks[0].clone()],
                )
                .unwrap(),
                FileManifest::from_chunks(
                    ChunkingDescriptor::deterministic_fixed(6).unwrap(),
                    &[chunks[1].clone()],
                )
                .unwrap(),
            ];
            assert_eq!(required[0].file_id(), required[1].file_id());
            assert_ne!(required[0].id(), required[1].id());
            let mut before = BTreeMap::new();
            for (manifest, chunk) in required.iter().zip(&chunks) {
                let path = root
                    .join("objects/manifests")
                    .join(object_name(manifest.id().as_str()).unwrap());
                child::create(&path, &journal::manifest_bytes(manifest).unwrap());
                before.insert(path.clone(), child::witness(&path));
                let path = root
                    .join("objects/chunks")
                    .join(object_name(chunk.id().as_str()).unwrap());
                child::create(&path, chunk.bytes());
                before.insert(path.clone(), child::witness(&path));
            }
            let cancel = Cancellation::default();
            let proof =
                verify_published_cas(open(&root, &binding), &binding, &required, &cancel).unwrap();
            proof.require(&binding, &required, &cancel).unwrap();
            proof
                .require(
                    &binding,
                    &[required[1].clone(), required[0].clone()],
                    &cancel,
                )
                .unwrap();
            assert!(proof.require(&binding, &required[..1], &cancel).is_err());
            let mut wrong = binding.clone();
            wrong.database_identity = "1:0:99".into();
            assert!(proof.require(&wrong, &required, &cancel).is_err());
            assert_eq!(fs::read_dir(root.join("staging")).unwrap().count(), 0);
            for (path, witness) in before {
                assert_eq!(child::witness(&path), witness);
            }
            let chunk_path = root
                .join("objects/chunks")
                .join(object_name(chunks[0].id().as_str()).unwrap());
            // A live proof must exclude ordinary write/delete opens, not just
            // notice pre-existing corruption during its initial hash pass.
            assert_eq!(
                proof.objects.len(),
                4,
                "duplicate ordinals share one retained handle"
            );
            assert_eq!(
                fs::write(&chunk_path, b"xyz").unwrap_err().raw_os_error(),
                Some(32)
            );
            assert_eq!(
                fs::remove_file(&chunk_path).unwrap_err().raw_os_error(),
                Some(32)
            );
            let replacement = root.join("replacement");
            child::create(&replacement, b"xyz");
            assert!(fs::rename(&replacement, &chunk_path).is_err());
            assert_eq!(fs::read(&replacement).unwrap(), b"xyz");
            proof.require(&binding, &required, &cancel).unwrap();
            assert_eq!(fs::read(&chunk_path).unwrap(), b"abc");
            drop(proof);
            // Existing ordinary writers also prevent a new proof from forming.
            let writer = fs::OpenOptions::new()
                .write(true)
                .open(&chunk_path)
                .unwrap();
            assert!(
                verify_published_cas(open(&root, &binding), &binding, &required, &cancel).is_err()
            );
            drop(writer);
            fs::write(&chunk_path, b"xyz").unwrap();
            let corrupted = child::witness(&chunk_path);
            let result = verify_published_cas(open(&root, &binding), &binding, &required, &cancel);
            assert!(result
                .err()
                .unwrap()
                .to_string()
                .contains("Published CAS chunk bytes differ"));
            assert_eq!(
                child::witness(&chunk_path),
                corrupted,
                "failed verification does not repair bytes"
            );
            assert_eq!(fs::read_dir(root.join("staging")).unwrap().count(), 0);
            // Retain this owned fixture, including deliberately corrupt bytes.
        },
    );
}
