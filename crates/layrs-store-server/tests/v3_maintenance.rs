use bytes::Bytes;
use futures_util::stream;
use layrs_core::scm::{Chunk, ObjectId};
use layrs_store_server::v3::{CasLimits, FsCasMaintenance, FsServerCas, ServerCas};
use layrs_sync::v3::{Compression, ObjectDescriptor, ObjectKind};
use std::path::PathBuf;

fn child(root: &std::path::Path, mode: &str) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "maintenance_process_child", "--nocapture"])
        .env("LAYRS_MAINTENANCE_TEST_ROOT", root)
        .env("LAYRS_MAINTENANCE_TEST_MODE", mode)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
fn maintenance_process_child() {
    let Some(root) = std::env::var_os("LAYRS_MAINTENANCE_TEST_ROOT") else {
        return;
    };
    let mode = std::env::var("LAYRS_MAINTENANCE_TEST_MODE").unwrap();
    let root = PathBuf::from(root);
    match mode.as_str() {
        "exclusive-denied" => assert!(FsCasMaintenance::try_acquire(&root).is_err()),
        "exclusive-allowed" => {
            let _guard = FsCasMaintenance::try_acquire(&root).unwrap();
        }
        "shared-denied" => {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            assert!(
                runtime
                    .block_on(FsServerCas::open(root, CasLimits::default()))
                    .is_err()
            );
        }
        _ => panic!("unexpected test mode"),
    }
}

#[tokio::test]
async fn maintenance_excludes_processes_clones_and_outstanding_read_streams() {
    let root = std::env::temp_dir().join(format!(
        "layrs-maintenance-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let cas = FsServerCas::open(&root, CasLimits::default())
        .await
        .unwrap();
    let clone = cas.clone();
    child(&root, "exclusive-denied");
    drop(clone);
    let bytes = b"immutable object retained during maintenance fence tests";
    let chunk = Chunk::new(bytes.to_vec()).unwrap();
    let object = ObjectDescriptor {
        kind: ObjectKind::Chunk,
        object_id: ObjectId::parse(chunk.id().as_str()).unwrap(),
        canonical_byte_len: bytes.len() as u64,
        encoded_byte_len: bytes.len() as u64,
        compression: Compression::Identity,
    };
    cas.put_if_absent(
        &object,
        Box::pin(stream::once(async { Ok(Bytes::from_static(bytes)) })),
    )
    .await
    .unwrap();
    let stream = cas.verified_stream(&object).await.unwrap();
    drop(cas);
    child(&root, "exclusive-denied");
    drop(stream);
    child(&root, "exclusive-allowed");
    let maintenance = FsCasMaintenance::try_acquire(&root).unwrap();
    child(&root, "shared-denied");
    child(&root, "exclusive-denied");
    drop(maintenance);
    let cas = FsServerCas::open(&root, CasLimits::default())
        .await
        .unwrap();
    assert!(cas.head(&object).await.unwrap().is_some());
    drop(cas);
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn maintenance_refuses_linked_root_lock_and_hardlinked_lock() {
    use std::os::unix::fs::symlink;
    let root = std::env::temp_dir().join(format!("layrs-maintenance-links-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let cas = root.join("cas");
    std::fs::create_dir(&cas).unwrap();
    let outside = root.join("outside");
    std::fs::write(&outside, b"").unwrap();
    let lock = cas.join(".layrs-cas-maintenance.lock");
    symlink(&outside, &lock).unwrap();
    assert!(FsCasMaintenance::try_acquire(&cas).is_err());
    std::fs::remove_file(&lock).unwrap();
    std::fs::hard_link(&outside, &lock).unwrap();
    assert!(FsCasMaintenance::try_acquire(&cas).is_err());
    std::fs::remove_file(&lock).unwrap();
    let link = root.join("alias");
    symlink(&cas, &link).unwrap();
    assert!(FsCasMaintenance::try_acquire(&link).is_err());
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(root).unwrap();
}
