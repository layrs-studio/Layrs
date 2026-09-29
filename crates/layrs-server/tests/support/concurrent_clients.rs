use layrs_client_core::native_scm::NativeRemote;
use layrs_core::scm::{SpaceId, StepId};
use std::{path::Path, sync::Barrier, time::Instant};

/// Actual independent native Stores and verified TLS downloads. This is read
/// contention, not a claim that ten simultaneous writers were benchmarked.
pub fn run(remote: &NativeRemote, space: &SpaceId, step: &StepId, root: &Path) {
    let mut cases = vec![];
    for count in [2, 10] {
        let barrier = Barrier::new(count);
        let start = Instant::now();
        let durations = std::thread::scope(|scope| {
            let tasks = (0..count)
                .map(|index| {
                    let directory = root.join(format!("concurrent-{count}-{index}"));
                    std::fs::create_dir(&directory).unwrap();
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        let began = Instant::now();
                        let workspace = remote
                            .clone_into(&directory, space, "Concurrent clone")
                            .unwrap();
                        assert_eq!(
                            std::fs::read(directory.join("docs/readme.txt")).unwrap(),
                            b"initial contents"
                        );
                        assert!(!workspace.read_step_files(step).unwrap().is_empty());
                        assert!(workspace.working_diff().unwrap().is_empty());
                        let received = remote.receive(&workspace).unwrap();
                        assert!(!received.materialized && !received.divergent);
                        began.elapsed().as_millis() as u64
                    })
                })
                .collect::<Vec<_>>();
            tasks
                .into_iter()
                .map(|task| task.join().unwrap())
                .collect::<Vec<_>>()
        });
        let elapsed = start.elapsed().as_millis() as u64;
        eprintln!(
            "Concurrent native clones: {count} clients, {elapsed} ms, verified bytes and no-change receive"
        );
        cases
            .push(serde_json::json!({"clients":count,"elapsedMs":elapsed,"perClientMs":durations}));
    }
    if let Some(path) = std::env::var_os("LAYRS_TEST_CLIENT_MEASUREMENT_FILE") {
        let report = serde_json::json!({"schemaVersion":"layrs.concurrent-clones.v1","platform":std::env::consts::OS,"cases":cases,"limitations":["Tiny shared fixture, warm server caches; build/test debug binaries.","Parallel download/clone and no-change receive; ten simultaneous publications are not measured.","Tokens, endpoints and private paths omitted."]});
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
