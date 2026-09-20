use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use crate::fault;
use crate::{LocalStore, LocalStoreConfig};
use layrs_sync::{DeterministicChunker, LocalStepRef, TreeEntry};

#[test]
fn cas_path_shards_by_algorithm_and_first_four_hex_chars() {
    let hash = ContentHash::blake3("abcdef0123456789").expect("valid hash");
    let path = cas_path(Path::new("store").join("objects"), &hash);

    assert_eq!(
        path,
        Path::new("store")
            .join("objects")
            .join("blake3")
            .join("ab")
            .join("cd")
            .join("abcdef0123456789")
    );
}

#[test]
fn placeholder_object_write_preserves_public_cas_behavior() {
    let root = unique_test_dir("placeholder");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
    let bytes = b"hello layrs";
    let hash = ContentHash::placeholder_for(bytes);

    let outcome = store
        .write_object_prehashed(bytes, &hash)
        .expect("write object");
    assert!(!outcome.existed);
    assert_eq!(outcome.verification, HashVerification::Verified);
    assert_eq!(fs::read(outcome.path).expect("read object"), bytes);

    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn build_file_tree_and_step_v2_persists_merkle_objects() {
    let root = unique_test_dir("merkle-v2");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
    let chunker = DeterministicChunker::default();

    let stored_file = store
        .build_file_v2(b"hello merkle store", &chunker)
        .expect("build file");
    let stored_tree = store
        .write_tree_v2(vec![TreeEntry::file("src/hello.txt", &stored_file.file)])
        .expect("write tree");
    let step = LocalStepRef::new(
        "layer-main",
        stored_tree.tree.tree_id.clone(),
        None,
        1,
        "2026-06-30T12:00:00Z",
        None,
    )
    .expect("step ref");
    let stored_step = store.write_local_step_ref_v2(step).expect("write step ref");

    assert_eq!(stored_file.chunks.len(), 1);
    assert!(stored_file.file_write.path.is_file());
    assert!(stored_tree.write.path.is_file());
    assert!(stored_step.write.path.is_file());

    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn detects_existing_corrupt_object_instead_of_accepting_it() {
    let root = unique_test_dir("corrupt");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
    let bytes = b"durable bytes";
    let hash = ContentHash::blake3_for(bytes);
    let first = store
        .write_object_prehashed(bytes, &hash)
        .expect("write object");
    fs::write(&first.path, b"corrupt").expect("corrupt object");

    let error = store
        .write_object_prehashed(bytes, &hash)
        .expect_err("corruption must fail");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);

    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn fault_before_rename_never_publishes_target() {
    let root = unique_test_dir("fault");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
    let bytes = b"not published";
    let hash = ContentHash::blake3_for(bytes);
    fault::fail_once(FaultPoint::BeforeRename);

    store
        .write_object_prehashed(bytes, &hash)
        .expect_err("fault must abort write");
    assert!(!store.cas_path(&hash).exists());

    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

#[cfg(unix)]
#[test]
fn unix_publication_recovers_from_every_link_unlink_durability_boundary() {
    let points = [
        FaultPoint::BeforeRename,
        FaultPoint::AfterCasLink,
        FaultPoint::BeforeCasDirectorySync,
        FaultPoint::AfterCasDirectorySync,
        FaultPoint::BeforePendingUnlink,
        FaultPoint::AfterPendingUnlink,
        FaultPoint::BeforePendingDirectorySync,
        FaultPoint::AfterPendingDirectorySync,
    ];
    for (index, point) in points.into_iter().enumerate() {
        let root = unique_test_dir(&format!("unix-boundary-{index}"));
        let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
        let bytes = format!("durable boundary {index}").into_bytes();
        let digest = ObjectDigest::blake3_for(&bytes);
        let pending = write_temp_object(store.layout(), &bytes).expect("write pending object");
        let target = store.cas_path_v2(&digest);
        fault::fail_once(point);

        install_pending(store.layout(), &pending, &target, |stored| {
            require_digest(stored, &digest)
        })
        .expect_err("injected durability fault");

        if pending.path.exists() {
            install_pending(store.layout(), &pending, &target, |stored| {
                require_digest(stored, &digest)
            })
            .expect("retry pending publication");
        } else {
            store
                .write_object_v2(&bytes, &digest)
                .expect("retry completed publication");
        }
        assert_eq!(fs::read(&target).expect("read recovered target"), bytes);
        assert!(!pending.path.exists());

        drop(store);
        fs::remove_dir_all(root).expect("remove root");
    }
}

#[test]
fn concurrent_publishers_never_clobber_the_atomic_winner() {
    let root = unique_test_dir("publish-race");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
    let first_bytes = b"first synchronized writer".to_vec();
    let second_bytes = b"second synchronized writer".to_vec();
    let first_pending =
        write_temp_object(store.layout(), &first_bytes).expect("first pending object");
    let second_pending =
        write_temp_object(store.layout(), &second_bytes).expect("second pending object");
    let target = store.layout().cas_dir.join("synchronized-race-winner");
    let barrier = Arc::new(Barrier::new(2));

    let first = spawn_racing_publish(
        store.layout().clone(),
        first_pending,
        target.clone(),
        first_bytes.clone(),
        Arc::clone(&barrier),
    );
    let second = spawn_racing_publish(
        store.layout().clone(),
        second_pending,
        target.clone(),
        second_bytes.clone(),
        barrier,
    );
    let outcomes = [
        first.join().expect("first writer thread"),
        second.join().expect("second writer thread"),
    ];

    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Ok(false)))
            .count(),
        1
    );
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_err()).count(),
        1
    );
    let winner = fs::read(&target).expect("read atomic winner");
    assert!(winner == first_bytes || winner == second_bytes);

    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn swapped_cas_ancestor_never_redirects_publication_outside_store() {
    let base = unique_test_dir("swapped-ancestor");
    let root = base.join("store");
    let outside = base.join("outside");
    fs::create_dir_all(&outside).expect("create outside");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
    let bytes = b"ancestor pinned publication".to_vec();
    let digest = ObjectDigest::blake3_for(&bytes);
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let writer_store = store.clone();
    let writer_entered = Arc::clone(&entered);
    let writer_release = Arc::clone(&release);
    let writer_digest = digest.clone();
    let writer = thread::spawn(move || {
        fault::pause_once(FaultPoint::BeforeRename, writer_entered, writer_release);
        writer_store.write_object_v2(&bytes, &writer_digest)
    });

    entered.wait();
    let algorithm = store.layout().cas_dir.join("blake3");
    let parked = store.layout().cas_dir.join("blake3-pinned");
    let swapped = match fs::rename(&algorithm, &parked) {
        Ok(()) => {
            create_directory_link(&outside, &algorithm);
            true
        }
        #[cfg(windows)]
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => false,
        Err(error) => panic!("park pinned algorithm directory: {error}"),
    };
    release.wait();

    let result = writer.join().expect("writer thread");
    if swapped {
        let error = result.expect_err("swapped ancestor must abort publication");
        assert!(matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
        ));
        assert!(!cas_path_for_digest(&parked, &digest).exists());
    } else {
        result.expect("pinned Windows handles must complete publication");
        assert_eq!(
            fs::read(store.cas_path_v2(&digest)).expect("read pinned publication"),
            b"ancestor pinned publication"
        );
    }
    assert_eq!(fs::read_dir(&outside).expect("read outside").count(), 0);

    if swapped {
        remove_directory_link(&algorithm);
        fs::rename(&parked, &algorithm).expect("restore algorithm directory");
    }
    drop(store);
    fs::remove_dir_all(base).expect("remove fixture");
}

#[test]
fn swapped_pending_source_never_poisons_cas_target() {
    let root = unique_test_dir("swapped-pending");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
    let bytes = b"verified pending source".to_vec();
    let digest = ObjectDigest::blake3_for(&bytes);
    let target = store.cas_path_v2(&digest);
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let writer_store = store.clone();
    let writer_entered = Arc::clone(&entered);
    let writer_release = Arc::clone(&release);
    let writer_digest = digest.clone();
    let writer = thread::spawn(move || {
        fault::pause_once(FaultPoint::BeforeRename, writer_entered, writer_release);
        writer_store.write_object_v2(&bytes, &writer_digest)
    });

    entered.wait();
    let pending = fs::read_dir(&store.layout().tmp_dir)
        .expect("read temporary directory")
        .next()
        .expect("pending entry")
        .expect("read pending entry")
        .path();
    let parked = pending.with_extension("parked");
    fs::rename(&pending, &parked).expect("swap pending source");
    fs::write(&pending, b"poison").expect("place poisoned replacement");
    release.wait();

    let result = writer.join().expect("writer thread");
    #[cfg(windows)]
    {
        // Windows publishes the pinned handle, independent of its replaced name.
        result.expect("publish pinned original");
        assert_eq!(
            fs::read(&target).expect("read CAS target"),
            b"verified pending source"
        );
    }
    #[cfg(unix)]
    {
        // linkat publishes by name; the inode check must reject its replacement.
        let error = result.expect_err("replaced pending inode must reject publication");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(
            error.to_string(),
            "pending source changed during CAS publication"
        );
        assert!(!target.exists(), "replacement was admitted into the CAS");
        assert_eq!(
            fs::read(&parked).expect("read pinned original"),
            b"verified pending source"
        );
    }
    assert!(!pending.exists(), "poisoned replacement was not removed");

    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

#[cfg(unix)]
#[test]
fn swapped_pending_symlink_is_removed_from_cas() {
    let root = unique_test_dir("swapped-pending-symlink");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
    let bytes = b"verified pending source".to_vec();
    let digest = ObjectDigest::blake3_for(&bytes);
    let target = store.cas_path_v2(&digest);
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let writer_store = store.clone();
    let writer_entered = Arc::clone(&entered);
    let writer_release = Arc::clone(&release);
    let writer_digest = digest.clone();
    let writer = thread::spawn(move || {
        fault::pause_once(FaultPoint::BeforeRename, writer_entered, writer_release);
        writer_store.write_object_v2(&bytes, &writer_digest)
    });

    entered.wait();
    let pending = fs::read_dir(&store.layout().tmp_dir)
        .expect("read temporary directory")
        .next()
        .expect("pending entry")
        .expect("read pending entry")
        .path();
    let parked = pending.with_extension("parked");
    fs::rename(&pending, &parked).expect("park pending source");
    std::os::unix::fs::symlink(&parked, &pending).expect("replace pending source with symlink");
    release.wait();

    writer
        .join()
        .expect("writer thread")
        .expect_err("symlink source must fail publication");
    assert!(!target.exists(), "symlink remained in the CAS");

    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

#[test]
fn mutated_pinned_pending_source_leaves_no_cas_target() {
    let root = unique_test_dir("mutated-pending");
    let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
    let bytes = b"verified before mutation".to_vec();
    let digest = ObjectDigest::blake3_for(&bytes);
    let target = store.cas_path_v2(&digest);
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let writer_store = store.clone();
    let writer_entered = Arc::clone(&entered);
    let writer_release = Arc::clone(&release);
    let writer_digest = digest.clone();
    let writer = thread::spawn(move || {
        fault::pause_once(FaultPoint::BeforeRename, writer_entered, writer_release);
        writer_store.write_object_v2(&bytes, &writer_digest)
    });

    entered.wait();
    let pending = fs::read_dir(&store.layout().tmp_dir)
        .expect("read temporary directory")
        .next()
        .expect("pending entry")
        .expect("read pending entry")
        .path();
    let parked = pending.with_extension("parked");
    fs::rename(&pending, &parked).expect("park verified pending source");
    fs::write(&parked, b"poisoned after verification").expect("mutate pinned source");
    fs::write(&pending, b"replacement").expect("replace pending name");
    release.wait();

    let error = writer
        .join()
        .expect("writer thread")
        .expect_err("mutated source must fail publication");
    #[cfg(windows)]
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    #[cfg(unix)]
    {
        // Name replacement is detected before verification of the modified bytes.
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(
            error.to_string(),
            "pending source changed during CAS publication"
        );
    }
    assert!(!target.exists(), "mutated bytes remained in the CAS");
    assert!(!pending.exists(), "replacement pending source remained");

    drop(store);
    fs::remove_dir_all(root).expect("remove root");
}

fn spawn_racing_publish(
    layout: StoreLayout,
    pending: PendingObject,
    target: PathBuf,
    expected: Vec<u8>,
    barrier: Arc<Barrier>,
) -> thread::JoinHandle<io::Result<bool>> {
    thread::spawn(move || {
        let first_verification = AtomicBool::new(true);
        install_pending(&layout, &pending, &target, |stored| {
            if first_verification.swap(false, Ordering::SeqCst) {
                barrier.wait();
            }
            if stored == expected {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "another synchronized writer won publication",
                ))
            }
        })
    })
}

fn unique_test_dir(label: &str) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "layrs-store-local-cas-{label}-{}-{timestamp}",
        std::process::id()
    ))
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("create directory symlink");
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) {
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .expect("run mklink");
    assert!(status.success(), "create directory junction");
}

#[cfg(unix)]
fn remove_directory_link(link: &Path) {
    fs::remove_file(link).expect("remove directory symlink");
}

#[cfg(windows)]
fn remove_directory_link(link: &Path) {
    fs::remove_dir(link).expect("remove directory junction");
}
