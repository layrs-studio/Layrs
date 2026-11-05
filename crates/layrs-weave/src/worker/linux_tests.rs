use super::*;
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(super) fn special_probe(input: &str) -> bool {
    if let Some(path) = input.strip_prefix("pid:") {
        std::fs::write(path, std::process::id().to_string()).unwrap();
        std::thread::sleep(Duration::from_secs(30));
    } else if let Some(path) = input.strip_prefix("supervise:") {
        let _ = probe(&format!("pid:{path}"), WorkerLimits::default());
    } else if input == "limits" {
        let mut bound = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut bound) }, 0);
        assert_eq!(bound.rlim_cur, bound.rlim_max);
        println!("address-space-bound={}", bound.rlim_max);
        let larger = libc::rlimit {
            rlim_cur: bound.rlim_max * 2,
            rlim_max: bound.rlim_max * 2,
        };
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_AS, &larger) }, -1);
        assert_eq!(
            unsafe { libc::prlimit(0, libc::RLIMIT_AS, &larger, std::ptr::null_mut()) },
            -1
        );
        assert_eq!(unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, 0) }, -1);
        // Includes privileged execution: RLIMIT_NPROC would not enforce this
        // for root; seccomp must reject the actual kernel process operations.
        for syscall in [libc::SYS_fork, libc::SYS_vfork] {
            assert_eq!(unsafe { libc::syscall(syscall) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
        }
        assert_eq!(
            unsafe { libc::syscall(libc::SYS_clone, libc::SIGCHLD, 0, 0, 0, 0) },
            -1
        );
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
        assert_eq!(
            unsafe { libc::syscall(libc::SYS_clone3, std::ptr::null::<u8>(), 0) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ENOSYS)
        );
    } else {
        return false;
    }
    true
}

#[test]
fn linux_limits_are_exact_and_cannot_be_raised_or_escaped_by_process_creation() {
    let output = probe(
        "limits",
        WorkerLimits {
            memory_bytes: 64 * 1024 * 1024,
            ..WorkerLimits::default()
        },
    )
    .unwrap();
    assert!(String::from_utf8_lossy(&output).contains("address-space-bound=67108864"));
}

#[test]
fn linux_parent_death_kills_the_supervised_process() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("layrs-lens-parent-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let pid_file = directory.join("child.pid");
    let mut parent = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "worker::tests::probe_process",
            "--nocapture",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    parent
        .stdin
        .take()
        .unwrap()
        .write_all(format!("supervise:{}", pid_file.display()).as_bytes())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut pid = None;
    while pid.is_none() && Instant::now() < deadline {
        pid = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|text| text.parse::<u32>().ok());
        std::thread::sleep(Duration::from_millis(10));
    }
    parent.kill().unwrap();
    parent.wait().unwrap();
    let pid = pid.expect("supervised probe did not start");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let stopped = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => true,
            Ok(stat) => stat
                .rsplit_once(") ")
                .is_some_and(|(_, tail)| tail.starts_with('Z')),
            _ => false,
        };
        if stopped {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "worker survived loss of its supervising parent"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::remove_file(pid_file).unwrap();
    std::fs::remove_dir(directory).unwrap();
}
