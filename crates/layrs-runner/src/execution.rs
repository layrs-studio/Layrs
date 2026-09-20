use super::{Recipe, Result};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
pub struct Execution {
    pub passed: bool,
    pub log: String,
}
fn docker() -> Command {
    let mut command = Command::new("docker");
    command.env_clear();
    for name in [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "HOME",
        "USERPROFILE",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    if let Some(config) = std::env::var_os("LAYRS_RUNNER_DOCKER_CONFIG") {
        command.args(["--config"]).arg(config);
    }
    if let Some(host) = std::env::var_os("LAYRS_RUNNER_DOCKER_HOST") {
        command.args(["-H"]).arg(host);
    }
    command.stdin(Stdio::null());
    command
}
fn terminate(name: &str) -> Result<()> {
    let mut child = docker()
        .args(["rm", "--force", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(15) {
            child.kill()?;
            let _ = child.wait();
            return Err("Docker cleanup timed out; inspect the recorded container name".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}
pub fn execute_recipe(recipe: &Recipe, checkout: &Path) -> Result<Execution> {
    recipe.validate()?;
    let checkout = checkout.canonicalize()?;
    let checkout = checkout.to_str().ok_or("checkout path must be UTF-8")?;
    #[cfg(windows)]
    let checkout = checkout.strip_prefix(r"\\?\").unwrap_or(checkout);
    if checkout.contains(',') {
        return Err("checkout path contains unsupported mount separator".into());
    }
    let name = format!("layrs-runner-{}", uuid::Uuid::new_v4());
    let mut command = docker();
    command.args([
        "run",
        "--rm",
        "--pull",
        "never",
        "--name",
        &name,
        "--network",
        "none",
        "--read-only",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--pids-limit",
        "128",
        "--memory",
        "512m",
        "--cpus",
        "1",
        "--tmpfs",
        "/tmp:rw,noexec,nosuid,size=64m",
        "--mount",
        &format!("type=bind,source={checkout},target=/work"),
        "--workdir",
        "/work",
    ]);
    #[cfg(unix)]
    {
        let uid = Command::new("id").arg("-u").output()?;
        let gid = Command::new("id").arg("-g").output()?;
        if !uid.status.success() || !gid.status.success() {
            return Err("cannot resolve runner UID/GID".into());
        }
        command.args([
            "--user",
            &format!(
                "{}:{}",
                String::from_utf8(uid.stdout)?.trim(),
                String::from_utf8(gid.stdout)?.trim()
            ),
        ]);
    }
    #[cfg(windows)]
    command.args(["--user", "10001:10001"]);
    command
        .arg("--entrypoint")
        .arg(&recipe.command[0])
        .arg(&recipe.image)
        .args(&recipe.command[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let log = Arc::new(Mutex::new(Vec::<u8>::new()));
    let truncated = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut readers = vec![];
    for mut pipe in [
        Box::new(child.stdout.take().ok_or("missing stdout")?) as Box<dyn Read + Send>,
        Box::new(child.stderr.take().ok_or("missing stderr")?) as Box<dyn Read + Send>,
    ] {
        let log = log.clone();
        let truncated = truncated.clone();
        let limit = recipe.maximum_log_bytes;
        readers.push(thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            while let Ok(count) = pipe.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                let mut output = log.lock().expect("log lock");
                let available = limit.saturating_sub(output.len());
                output.extend_from_slice(&buffer[..count.min(available)]);
                if count > available {
                    truncated.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }));
    }
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > Duration::from_secs(recipe.timeout_seconds) {
            timed_out = true;
            let cleanup = terminate(&name);
            let _ = child.kill();
            let status = child.wait()?;
            cleanup?;
            break status;
        }
        thread::sleep(Duration::from_millis(25));
    };
    terminate(&name)?;
    for reader in readers {
        reader.join().map_err(|_| "log collector failed")?;
    }
    let bytes = log.lock().map_err(|_| "log lock failed")?;
    // Lossy UTF-8 can expand; bound on the final UTF-8 bytes as well.
    let mut output = String::from_utf8_lossy(&bytes).into_owned();
    while output.len() > recipe.maximum_log_bytes {
        output.pop();
    }
    if truncated.load(std::sync::atomic::Ordering::Relaxed) {
        output.push_str("\n[log truncated]\n")
    }
    if timed_out {
        output.push_str("\n[execution timed out; container terminated]\n")
    }
    output.push_str(&format!("\n[container {name}; exit {:?}]\n", status.code()));
    Ok(Execution {
        passed: status.success() && !timed_out,
        log: output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recipes_reject_mutable_images_and_unbounded_jobs() {
        let mut r = Recipe {
            image: "alpine:latest".into(),
            command: vec!["/bin/true".into()],
            timeout_seconds: 1,
            maximum_log_bytes: 1024,
        };
        assert!(r.validate().is_err());
        r.image = format!("sha256:{}", "a".repeat(64));
        assert!(r.validate().is_ok());
        r.timeout_seconds = 1801;
        assert!(r.validate().is_err());
    }
    #[test]
    #[ignore = "requires preloaded Docker image LAYRS_TEST_RUNNER_IMAGE"]
    fn real_container_success_timeout_and_log_limit() {
        let image = std::env::var("LAYRS_TEST_RUNNER_IMAGE").unwrap();
        let directory =
            std::env::temp_dir().join(format!("layrs-runner-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("sample.txt"), b"exact step").unwrap();
        let recipe = |script: &str, timeout| Recipe {
            image: image.clone(),
            command: vec!["/bin/sh".into(), "-c".into(), script.into()],
            timeout_seconds: timeout,
            maximum_log_bytes: 1024,
        };
        let result=execute_recipe(&recipe("test \"$(cat sample.txt)\" = 'exact step' && test -z \"$LAYRS_RUNNER_TOKEN_FILE\" && printf passed",10),&directory).unwrap();
        assert!(result.passed, "{}", result.log);
        assert!(result.log.contains("passed"));
        let result = execute_recipe(&recipe("sleep 30", 1), &directory).unwrap();
        assert!(!result.passed);
        assert!(result.log.contains("timed out"));
        let result = execute_recipe(
            &recipe(
                "i=0; while test $i -lt 4000; do printf x; i=$((i+1)); done",
                10,
            ),
            &directory,
        )
        .unwrap();
        assert!(result.passed);
        assert!(result.log.contains("truncated"));
        assert!(result.log.len() < 1300);
        std::fs::remove_file(directory.join("sample.txt")).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
