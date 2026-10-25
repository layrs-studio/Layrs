//! Own only the process we spawn; readiness is bounded and authenticated by caller.
use crate::error::CliError;
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

const MAX_READY: usize = 64 * 1024;
const MAX_ERROR: usize = 4096;

pub(super) fn start(
    command: &mut Command,
    timeout: Duration,
    verify: impl FnOnce(&Value, Duration) -> Result<(), String> + Send + 'static,
) -> Result<Value, CliError> {
    if timeout.is_zero() || timeout > Duration::from_secs(300) {
        return Err(CliError::usage(
            "VFS readiness timeout must be positive and at most 300 seconds",
        ));
    }
    let deadline = Instant::now() + timeout;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| CliError::runtime(format!("could not start VFS host: {error}")))?;
    let pid = child.id();
    let stderr_bytes = Arc::new(Mutex::new(Vec::new()));
    let Some(stdout) = child.stdout.take() else {
        return Err(fail(
            &mut child,
            "host stdout pipe unavailable",
            &stderr_bytes,
        ));
    };
    let Some(mut stderr) = child.stderr.take() else {
        return Err(fail(
            &mut child,
            "host stderr pipe unavailable",
            &stderr_bytes,
        ));
    };
    let captured = stderr_bytes.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("vfs-host-stderr".into())
        .spawn(move || {
            let mut buffer = [0; 1024];
            while let Ok(length) = stderr.read(&mut buffer) {
                if length == 0 {
                    break;
                }
                if let Ok(mut bytes) = captured.lock() {
                    let keep = length.min(MAX_ERROR - bytes.len());
                    bytes.extend_from_slice(&buffer[..keep]);
                }
            }
        })
    {
        return Err(fail(
            &mut child,
            &format!("could not read host errors: {error}"),
            &stderr_bytes,
        ));
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    if let Err(error) = std::thread::Builder::new()
        .name("vfs-host-ready".into())
        .spawn(move || {
            let result: Result<Value, String> = (|| {
                let data = read_ready(stdout, pid)?;
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .ok_or("VFS readiness timed out")?;
                verify(&data, remaining)?;
                if Instant::now() >= deadline {
                    return Err("VFS readiness timed out".into());
                }
                Ok(data)
            })();
            let _ = sender.send(result);
        })
    {
        return Err(fail(
            &mut child,
            &format!("could not observe host readiness: {error}"),
            &stderr_bytes,
        ));
    }
    wait_ready(&mut child, receiver, deadline, &stderr_bytes)
}

fn wait_ready(
    child: &mut Child,
    receiver: mpsc::Receiver<Result<Value, String>>,
    deadline: Instant,
    stderr_bytes: &Arc<Mutex<Vec<u8>>>,
) -> Result<Value, CliError> {
    loop {
        let remaining = match deadline.checked_duration_since(Instant::now()) {
            Some(remaining) => remaining,
            None => {
                return Err(fail(child, "VFS host readiness timed out", stderr_bytes));
            }
        };
        match receiver.recv_timeout(remaining.min(Duration::from_millis(25))) {
            Ok(Ok(data)) => match child.try_wait() {
                Ok(None) => return Ok(data),
                Ok(Some(status)) => {
                    return Err(fail(
                        child,
                        &format!("VFS host exited during readiness ({status})"),
                        stderr_bytes,
                    ));
                }
                Err(error) => {
                    return Err(fail(
                        child,
                        &format!("could not observe VFS host: {error}"),
                        stderr_bytes,
                    ));
                }
            },
            Ok(Err(error)) => return Err(fail(child, &error, stderr_bytes)),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(fail(child, "VFS readiness observer stopped", stderr_bytes));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(fail(
                        child,
                        &format!("VFS host exited before readiness ({status})"),
                        stderr_bytes,
                    ));
                }
                Ok(None) => {}
                Err(error) => {
                    return Err(fail(
                        child,
                        &format!("could not observe VFS host: {error}"),
                        stderr_bytes,
                    ));
                }
            },
        }
    }
}

fn read_ready(reader: impl Read, expected_pid: u32) -> Result<Value, String> {
    let mut line = Vec::new();
    BufReader::new(reader.take((MAX_READY + 1) as u64))
        .read_until(b'\n', &mut line)
        .map_err(|e| e.to_string())?;
    if line.len() > MAX_READY {
        return Err("VFS readiness response exceeds 64 KiB".into());
    }
    if line.last() != Some(&b'\n') {
        return Err("VFS host exited without a complete readiness response".into());
    }
    let envelope: Value =
        serde_json::from_slice(&line).map_err(|_| "invalid VFS readiness JSON")?;
    if envelope["ok"] == false {
        return Err(envelope["error"]["message"]
            .as_str()
            .unwrap_or("VFS host startup failed")
            .chars()
            .take(MAX_ERROR)
            .collect());
    }
    if envelope["ok"] != true
        || envelope["data"]["status"] != "ready"
        || envelope["data"]["pid"].as_u64() != Some(u64::from(expected_pid))
        || !envelope["data"]["ticket"].is_object()
        || !envelope["data"]["path"].is_string()
    {
        return Err("VFS host did not return a valid readiness envelope".into());
    }
    Ok(envelope["data"].clone())
}

fn fail(child: &mut Child, message: &str, stderr: &Arc<Mutex<Vec<u8>>>) -> CliError {
    let pid = child.id();
    let state = match child.try_wait() {
        Ok(Some(status)) => format!("child PID {pid} exited ({status})"),
        _ => match child.kill() {
            Err(error) => {
                format!("could not terminate owned child PID {pid}: {error}; check this PID")
            }
            Ok(()) => {
                let until = Instant::now() + Duration::from_secs(5);
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            break format!("owned child PID {pid} terminated ({status})");
                        }
                        Err(error) => {
                            break format!(
                                "termination requested for PID {pid}; exit check failed: {error}"
                            );
                        }
                        Ok(None) if Instant::now() >= until => {
                            break format!(
                                "termination requested for PID {pid}; exit not yet observed"
                            );
                        }
                        Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
            }
        },
    };
    let detail = stderr
        .lock()
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .unwrap_or_default();
    CliError::runtime(format!(
        "{message}; {state}. The root and any durable creation/recovery state are retained; no files were removed.{}",
        if detail.is_empty() {
            String::new()
        } else {
            format!(" Host error: {detail}")
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sleeping_child() {
        if std::env::var_os("LAYRS_VFS_LAUNCH_TEST_CHILD").is_some() {
            std::thread::sleep(Duration::from_secs(30));
        }
    }

    struct TestChild(Child);
    impl Drop for TestChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn child() -> TestChild {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "native_vfs::process::tests::sleeping_child",
                "--nocapture",
            ])
            .env("LAYRS_VFS_LAUNCH_TEST_CHILD", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        TestChild(command.spawn().unwrap())
    }

    #[test]
    fn readiness_timeout_and_auth_failure_terminate_only_the_owned_child() {
        let mut unrelated = child();
        for timed_out in [true, false] {
            let mut owned = child();
            let (sender, receiver) = mpsc::sync_channel(1);
            if !timed_out {
                sender.send(Err("authentication rejected".into())).unwrap();
            }
            let before = Instant::now();
            let error = wait_ready(
                &mut owned.0,
                receiver,
                before + Duration::from_millis(150),
                &Arc::new(Mutex::new(Vec::new())),
            )
            .unwrap_err();
            assert!(
                error.message.contains(if timed_out {
                    "timed out"
                } else {
                    "authentication rejected"
                }),
                "{error}"
            );
            assert!(before.elapsed() < Duration::from_secs(6));
            assert!(owned.0.try_wait().unwrap().is_some());
            assert!(unrelated.0.try_wait().unwrap().is_none());
        }
    }

    #[test]
    fn malformed_readiness_cannot_report_success_from_an_exited_process() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.arg("--list").stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let result = start(&mut command, Duration::from_secs(5), |_, _| {
            panic!("invalid output must not reach authentication")
        });
        assert!(result.is_err());
    }

    #[test]
    fn readiness_requires_one_complete_bounded_success_from_the_owned_child() {
        let valid = json!({"ok":true,"data":{"status":"ready","pid":42,"ticket":{},"path":"root"}})
            .to_string()
            + "\n";
        assert_eq!(read_ready(valid.as_bytes(), 42).unwrap()["pid"], 42);
        assert!(read_ready(valid.as_bytes(), 43).is_err());
        assert!(read_ready(valid.trim_end().as_bytes(), 42).is_err());
        assert!(read_ready(vec![b'x'; MAX_READY + 1].as_slice(), 42).is_err());
        assert!(read_ready(b"not JSON\n".as_slice(), 42).is_err());
        assert_eq!(
            read_ready(
                b"{\"ok\":false,\"error\":{\"message\":\"provider unavailable\"}}\n".as_slice(),
                42
            )
            .unwrap_err(),
            "provider unavailable"
        );
        for value in [
            json!({"ok":true,"data":{"status":"ready","pid":42,"path":"root"}}),
            json!({"ok":true,"data":{"status":"starting","pid":42,"ticket":{},"path":"root"}}),
        ] {
            assert!(read_ready((value.to_string() + "\n").as_bytes(), 42).is_err());
        }
    }
}
