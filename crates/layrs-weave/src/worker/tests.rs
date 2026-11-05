use super::*;

#[cfg(any(windows, all(target_os = "linux", target_arch = "x86_64")))]
#[test]
#[ignore = "subprocess fixture, explicitly selected by resource-bound tests"]
fn probe_process() {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    if linux_probes::special_probe(&input) {
        return;
    }
    match input.as_str() {
        "sleep" => std::thread::sleep(std::time::Duration::from_secs(30)),
        "memory" => {
            let mut blocks = Vec::new();
            for _ in 0..256 {
                blocks.push(vec![42u8; 1024 * 1024]);
            }
            std::hint::black_box(blocks);
        }
        "output" => {
            let bytes = [b'x'; 8192];
            for _ in 0..4096 {
                if std::io::stdout().write_all(&bytes).is_err() {
                    return;
                }
            }
        }
        "environment" => {
            assert!(std::env::var_os("PATH").is_none());
            assert!(std::env::var_os("USERPROFILE").is_none());
            println!("empty-env");
        }
        "child" => {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("--list")
                .status();
            assert!(status.is_err() || !status.unwrap().success());
            println!("child-refused");
        }
        _ => panic!("unknown probe"),
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "linux_tests.rs"]
mod linux_probes;
#[cfg(any(windows, all(target_os = "linux", target_arch = "x86_64")))]
fn probe(mode: &str, limits: WorkerLimits) -> Result<Vec<u8>, WorkerError> {
    run_process(
        &std::env::current_exe().unwrap(),
        &[
            "--ignored",
            "--exact",
            "worker::tests::probe_process",
            "--nocapture",
        ],
        mode.as_bytes(),
        limits,
    )
}

#[cfg(any(windows, all(target_os = "linux", target_arch = "x86_64")))]
#[test]
fn real_process_deadline_and_output_limit_are_enforced() {
    let limit = WorkerLimits {
        deadline_ms: 150,
        ..WorkerLimits::default()
    };
    let start = std::time::Instant::now();
    assert!(matches!(probe("sleep", limit), Err(WorkerError::Deadline)));
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert!(matches!(
        probe(
            "output",
            WorkerLimits {
                output_bytes: 1024,
                ..WorkerLimits::default()
            }
        ),
        Err(WorkerError::OutputLimit)
    ));
}
#[cfg(any(windows, all(target_os = "linux", target_arch = "x86_64")))]
#[test]
fn real_process_memory_cap_and_child_limit_are_enforced() {
    let baseline = probe(
        "environment",
        WorkerLimits {
            memory_bytes: 64 * 1024 * 1024,
            ..WorkerLimits::default()
        },
    )
    .unwrap();
    assert!(String::from_utf8_lossy(&baseline).contains("empty-env"));
    let result = probe(
        "memory",
        WorkerLimits {
            memory_bytes: 64 * 1024 * 1024,
            ..WorkerLimits::default()
        },
    );
    assert!(
        matches!(result, Err(WorkerError::ProcessExit(_))),
        "{result:?}"
    );
    let bytes = probe("child", WorkerLimits::default()).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("child-refused"));
}
#[cfg(any(windows, all(target_os = "linux", target_arch = "x86_64")))]
#[test]
fn worker_environment_is_cleared_and_input_rejects_before_start() {
    let bytes = probe("environment", WorkerLimits::default()).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("empty-env"));
    assert!(matches!(
        run_process(
            Path::new("missing-worker"),
            &[],
            b"12",
            WorkerLimits {
                input_bytes: 1,
                ..WorkerLimits::default()
            }
        ),
        Err(WorkerError::InputLimit)
    ));
}

#[test]
fn invalid_protocol_is_rejected_without_unbounded_output() {
    let mut output = vec![];
    assert!(serve(&b"{}"[..], &mut output).is_err());
    assert!(output.is_empty());
    assert!(serve(&vec![b' '; MAX_WIRE + 1][..], &mut output).is_err());
    assert!(output.is_empty());
}
