//! Real TLS contention: independent Stores sharing one observed Layer head.
use layrs_client_core::native_scm::{NativeRemote, NativeRemoteError, NativeWorkspace};
use layrs_store_local::scm_v1::checkout::CheckoutContent;
use layrs_sync::v3::{LayerMutationMode, ReceiptStatus};
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{
        Barrier,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

pub fn run(remote: &NativeRemote, root: &Path, lose_response: &AtomicBool) {
    let mut measurements = vec![];
    for count in [2, 10] {
        let seed_root = root.join(format!("writers-{count}-seed"));
        std::fs::create_dir(&seed_root).unwrap();
        std::fs::write(seed_root.join("baseline.txt"), b"shared baseline").unwrap();
        let seed = NativeWorkspace::initialize(&seed_root, "Concurrent publication").unwrap();
        let provision = remote
            .provision(&seed, "writer-tests", &format!("writers-{count}"))
            .unwrap();
        assert_eq!(
            remote
                .publish(&seed, LayerMutationMode::Advance)
                .unwrap()
                .status,
            ReceiptStatus::Applied
        );
        let mut clients = vec![];
        let mut expected_steps = BTreeSet::new();
        for index in 0..count {
            let directory = root.join(format!("writers-{count}-{index}"));
            std::fs::create_dir(&directory).unwrap();
            let workspace = remote
                .clone_into(&directory, &provision.space_id, "Concurrent writer")
                .unwrap();
            let path = format!("client-{index}.txt");
            let content = format!("unique captured bytes from writer {index}").into_bytes();
            std::fs::write(directory.join(&path), &content).unwrap();
            workspace.capture().unwrap();
            let step = workspace
                .state()
                .unwrap()
                .view
                .layer_heads()
                .iter()
                .find(|head| head.layer_id() == &provision.layer_id)
                .unwrap()
                .step_ids()[0]
                .clone();
            assert!(expected_steps.insert(step.clone()));
            clients.push((workspace, directory, path, content, step));
        }
        let barrier = Barrier::new(count);
        lose_response.store(true, Ordering::SeqCst);
        let started = Instant::now();
        let results = std::thread::scope(|scope| {
            let handles = clients
                .iter()
                .map(|(workspace, ..)| {
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        let began = Instant::now();
                        let (receipt, retried) = match remote
                            .publish(workspace, LayerMutationMode::Advance)
                        {
                            Ok(receipt) => (receipt, false),
                            Err(NativeRemoteError::Http(502)) => {
                                let journal: serde_json::Value = serde_json::from_slice(
                                    &std::fs::read(
                                        workspace.store_root().join("native-remote.json"),
                                    )
                                    .unwrap(),
                                )
                                .unwrap();
                                let pending = &journal["pending"];
                                let receipt = remote
                                    .publish(workspace, LayerMutationMode::Advance)
                                    .unwrap();
                                assert_eq!(
                                    pending["idempotencyKey"].as_str(),
                                    Some(receipt.idempotency_key.as_str())
                                );
                                assert_eq!(
                                    pending["requestDigest"].as_str(),
                                    Some(receipt.request_digest.as_str())
                                );
                                (receipt, true)
                            }
                            Err(error) => panic!("concurrent publication failed: {}", error.code()),
                        };
                        (receipt, retried, began.elapsed().as_millis() as u64)
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        let elapsed = started.elapsed().as_millis() as u64;
        assert_eq!(results.iter().filter(|(_, retried, _)| *retried).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|(receipt, _, _)| receipt.status == ReceiptStatus::Applied)
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|(receipt, _, _)| receipt.status == ReceiptStatus::Rejected)
                .count(),
            count - 1
        );
        assert_eq!(
            results
                .iter()
                .map(|(receipt, _, _)| receipt.idempotency_key.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            count
        );
        // Explicit integration retains each losing branch; it never silently
        // overwrites the winner. Keep this phase separate from advance timing.
        for ((workspace, directory, path, content, _), (receipt, _, _)) in
            clients.iter().zip(&results)
        {
            assert_eq!(&std::fs::read(directory.join(path)).unwrap(), content);
            assert!(workspace.working_diff().unwrap().is_empty());
            if receipt.status == ReceiptStatus::Rejected {
                assert_eq!(
                    remote
                        .publish(workspace, LayerMutationMode::Integrate)
                        .unwrap()
                        .status,
                    ReceiptStatus::Applied
                );
            }
        }
        let state = remote.bootstrap(&provision.space_id).unwrap();
        let observed: BTreeSet<_> = state
            .current_repository_view
            .record
            .layer_heads()
            .iter()
            .find(|head| head.layer_id() == &provision.layer_id)
            .unwrap()
            .step_ids()
            .iter()
            .cloned()
            .collect();
        assert_eq!(observed, expected_steps);
        for (workspace, directory, path, content, step) in &clients {
            let files = remote.fetch_step_files(&provision.space_id, step).unwrap();
            assert!(files.iter().any(|entry| entry.path == *path
                && matches!(
                    &entry.content, CheckoutContent::File { bytes, .. } if bytes == content
                )));
            let received = remote.receive(workspace).unwrap();
            assert!(received.divergent && !received.materialized);
            assert_eq!(&std::fs::read(directory.join(path)).unwrap(), content);
        }
        eprintln!(
            "Concurrent native publication: {count} writers, {elapsed} ms, 1 advance + {} explicit retained divergences",
            count - 1
        );
        measurements.push(serde_json::json!({"clients":count,"advanceElapsedMs":elapsed,
            "perClientMs":results.iter().map(|(_,_,ms)| *ms).collect::<Vec<_>>(),
            "appliedAdvances":1,"rejectedAdvances":count-1,"lostResponseReplays":1,"retainedHeads":count}));
    }
    if let Some(path) = std::env::var_os("LAYRS_TEST_PUBLICATION_MEASUREMENT_FILE") {
        let report = serde_json::json!({"schemaVersion":"layrs.concurrent-publications.v1",
            "platform":std::env::consts::OS,"cases":measurements,
            "limitations":["Tiny fixtures, debug binaries, one node, one authenticated account with independent client IDs.",
                "Advance timing includes immutable uploads and one simulated lost-response retry; initial clones and explicit subsequent integrations excluded.",
                "No production throughput, large-file, full-disk or cross-machine latency guarantee."]});
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
