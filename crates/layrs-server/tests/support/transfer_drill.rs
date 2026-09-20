use layrs_client_core::native_scm::NativeRemote;
use layrs_core::scm::{SpaceId, StepId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{io::Write, path::Path};

pub fn run(
    remote: &NativeRemote,
    space: &SpaceId,
    step: &StepId,
    interrupted: &AtomicUsize,
    resumed: &AtomicUsize,
) {
    let expected = remote.fetch_step_files(space, step).unwrap();
    interrupted.store(1, Ordering::SeqCst);
    assert_eq!(
        remote
            .fetch_step_files(space, step)
            .expect("interrupted download must resume verified bytes"),
        expected
    );
    assert_eq!(interrupted.load(Ordering::SeqCst), 0);
    assert!(
        resumed.load(Ordering::SeqCst) > 0,
        "a retry must request only the missing suffix"
    );
    interrupted.store(4, Ordering::SeqCst);
    assert!(
        remote.fetch_step_files(space, step).is_err(),
        "repeated disconnects must exhaust a finite retry budget"
    );
    assert_eq!(interrupted.load(Ordering::SeqCst), 0);
    assert_eq!(remote.fetch_step_files(space, step).unwrap(), expected);
}

pub fn large_file(
    remote: &NativeRemote,
    root: &Path,
    interrupted: &AtomicUsize,
    minimum: &AtomicUsize,
) {
    use layrs_client_core::native_scm::NativeWorkspace;
    use layrs_store_local::scm_v1::checkout::CheckoutContent;
    use layrs_sync::v3::{LayerMutationMode, ReceiptStatus};
    let directory = root.join("transfer-large-file");
    std::fs::create_dir(&directory).unwrap();
    let mut file = std::fs::File::create(directory.join("asset.bin")).unwrap();
    let mut digest = blake3::Hasher::new();
    for index in 0..256 {
        let block = [(index % 251) as u8; 64 * 1024];
        file.write_all(&block).unwrap();
        digest.update(&block);
    }
    drop(file);
    let workspace = NativeWorkspace::initialize(&directory, "Large transfer").unwrap();
    let provision = remote
        .provision(&workspace, "transfer-tests", "asset")
        .unwrap();
    let receipt = remote
        .publish(&workspace, LayerMutationMode::Advance)
        .unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Applied);
    let step = workspace.state().unwrap().view.layer_heads()[0].step_ids()[0].clone();
    // Native capture currently chunks files at 64 KiB. Select a chunk rather
    // than the small Step/Tree/manifest that precedes it in this fetch.
    minimum.store(32 * 1024, Ordering::SeqCst);
    interrupted.store(1, Ordering::SeqCst);
    let download_started = std::time::Instant::now();
    let files = remote.fetch_step_files(&provision.space_id, &step).unwrap();
    let download_ms = download_started.elapsed().as_millis();
    assert_eq!(
        interrupted.load(Ordering::SeqCst),
        0,
        "large chunk interruption was exercised"
    );
    minimum.store(0, Ordering::SeqCst);
    let asset = files
        .iter()
        .find(|entry| entry.path == "asset.bin")
        .unwrap();
    let CheckoutContent::File { bytes, .. } = &asset.content else {
        panic!("file expected")
    };
    assert_eq!(bytes.len(), 16 * 1024 * 1024);
    assert_eq!(blake3::hash(bytes), digest.finalize());
    assert!(workspace.working_diff().unwrap().is_empty());
    eprintln!(
        "Native TLS interrupted file transfer: 16 MiB, {download_ms} ms, exact final digest, bounded ranged object downloads"
    );
}
