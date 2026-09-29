//! Ctrl+C requests cooperative transfer cancellation; it never aborts a worker
//! whose durable transaction may already have reached the server.
use layrs_client_core::native_scm::{
    NativeRemoteAction as Action, NativeRemoteRequest, execute_remote,
};
use serde_json::Value;
use std::{
    path::Path,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

static SIGNALS: AtomicU64 = AtomicU64::new(0);
static NEXT_TRANSFER: AtomicU64 = AtomicU64::new(0);
static HANDLER: OnceLock<Result<(), String>> = OnceLock::new();

fn install() -> Result<(), String> {
    HANDLER
        .get_or_init(|| {
            ctrlc::set_handler(|| {
                SIGNALS.fetch_add(1, Ordering::Relaxed);
            })
            .map_err(|_| "cannot install the Ctrl+C handler; transfer not started".to_owned())
        })
        .clone()
}

pub fn run_remote(
    root: impl AsRef<Path>,
    mut request: NativeRemoteRequest,
) -> Result<Value, String> {
    let transfer_id = format!(
        "cli-transfer-{}-{}",
        std::process::id(),
        NEXT_TRANSFER.fetch_add(1, Ordering::Relaxed)
    );
    request.action = match request.action {
        Action::Publish { integrate } => Action::PublishCancellable {
            integrate,
            transfer_id: transfer_id.clone(),
        },
        Action::PublishCandidates => Action::PublishCandidatesCancellable {
            transfer_id: transfer_id.clone(),
        },
        Action::Receive => Action::ReceiveCancellable {
            transfer_id: transfer_id.clone(),
        },
        Action::Clone { space, name } => Action::CloneCancellable {
            space,
            name,
            transfer_id: transfer_id.clone(),
        },
        _ => return execute_remote(root, request),
    };
    let initial = SIGNALS.load(Ordering::Relaxed);
    install()?;
    let root = root.as_ref().to_path_buf();
    let worker_root = root.clone();
    let origin = request.origin.clone();
    let ca_pem = request.ca_pem.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = sender.send(execute_remote(worker_root, request));
    });
    wait_for_result(receiver, &SIGNALS, initial, || {
        execute_remote(
            &root,
            NativeRemoteRequest {
                origin: origin.clone(),
                ca_pem: ca_pem.clone(),
                action: Action::CancelTransfer {
                    transfer_id: transfer_id.clone(),
                },
            },
        )
    })
}

fn wait_for_result(
    receiver: mpsc::Receiver<Result<Value, String>>,
    signals: &AtomicU64,
    initial: u64,
    mut cancel: impl FnMut() -> Result<Value, String>,
) -> Result<Value, String> {
    let mut sent = false;
    loop {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(result)=>return result,
            Err(mpsc::RecvTimeoutError::Disconnected)=>return Err("remote transfer worker stopped without a result; inspect or retry its pending transaction".into()),
            Err(mpsc::RecvTimeoutError::Timeout)=>{},
        }
        if !sent && signals.load(Ordering::Relaxed) != initial {
            match cancel() {
                // Registration may follow a signal received during credential
                // loading. Keep retrying until registration or final outcome.
                Ok(value) if value["status"] == "notFound" => {}
                Ok(value) if value["status"] == "tooLate" => {
                    eprintln!("Commit may already be in progress; waiting for the actual result.");
                    sent = true;
                }
                Ok(value)
                    if value["status"] == "cancelRequested"
                        || value["status"] == "alreadyRequested" =>
                {
                    eprintln!("Cancellation requested; waiting for confirmation.");
                    sent = true;
                }
                _ => {
                    eprintln!(
                        "Cancellation could not be confirmed; waiting for the transfer result."
                    );
                    sent = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn cancellation_request_is_not_a_success_or_a_forced_exit() {
        for final_result in [
            Err("publication_pending: retry exact transaction".into()),
            Ok(json!({"receipt":{"status":"applied"}})),
        ] {
            let signals = AtomicU64::new(1);
            let (sender, receiver) = mpsc::sync_channel(1);
            let mut attempts = 0;
            let result = wait_for_result(receiver, &signals, 0, || {
                attempts += 1;
                if attempts == 1 {
                    return Ok(json!({"status":"notFound"}));
                }
                sender.send(final_result.clone()).unwrap();
                Ok(json!({"status":"cancelRequested"}))
            });
            assert_eq!(result, final_result);
            assert_eq!(attempts, 2);
        }
    }
    #[test]
    fn too_late_waits_for_the_matching_worker_result() {
        let signals = AtomicU64::new(1);
        let (sender, receiver) = mpsc::sync_channel(1);
        let result = wait_for_result(receiver, &signals, 0, || {
            sender
                .send(Err("HTTP response lost; pending retained".into()))
                .unwrap();
            Ok(json!({"status":"tooLate"}))
        });
        assert_eq!(result, Err("HTTP response lost; pending retained".into()));
    }
    #[cfg(unix)]
    #[test]
    fn real_sigint_reaches_the_portable_handler() {
        const CHILD: &str = "LAYRS_TEST_SIGINT_CHILD";
        if std::env::var_os(CHILD).is_some() {
            install().unwrap();
            let initial = SIGNALS.load(Ordering::Relaxed);
            println!("signal-handler-ready");
            let (sender, receiver) = mpsc::sync_channel(1);
            assert_eq!(
                wait_for_result(receiver, &SIGNALS, initial, || {
                    sender.send(Err("cancelled".into())).unwrap();
                    Ok(json!({"status":"cancelRequested"}))
                }),
                Err("cancelled".into())
            );
            return;
        }
        use std::io::{BufRead, BufReader};
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native_interrupt::tests::real_sigint_reaches_the_portable_handler",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let output = child.stdout.take().unwrap();
        let mut output = BufReader::new(output);
        loop {
            let mut line = String::new();
            assert!(
                output.read_line(&mut line).unwrap() > 0,
                "child did not install its signal handler"
            );
            if line.contains("signal-handler-ready") {
                break;
            }
        }
        assert!(
            std::process::Command::new("kill")
                .args(["-INT", &child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                panic!("SIGINT handler timed out");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
