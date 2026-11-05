//! Linux x86-64 supervision: address-space limit and process-creation filter
//! installed before exec, parent-death signal, bounded pipes and wall deadline.
//! Guest capability isolation remains the no-import WASM runtime, not this Job.
use super::*;
use std::{
    os::unix::process::{CommandExt, ExitStatusExt},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[path = "linux_image.rs"]
mod image;
pub(crate) use image::pin_image;

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn statement(code: u32, k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt: 0,
        jf: 0,
        k,
    }
}
fn jump(code: u32, k: u32, jt: u8, jf: u8) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt,
        jf,
        k,
    }
}

// Prepared in the parent: pre_exec must not allocate or acquire Rust locks.
fn process_filter() -> Vec<libc::sock_filter> {
    let load = libc::BPF_LD | libc::BPF_W | libc::BPF_ABS;
    let equal = libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K;
    let ret = libc::BPF_RET | libc::BPF_K;
    let denied = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;
    let mut filter = vec![
        statement(load, 4),            // seccomp_data.arch
        jump(equal, 0xc000003e, 1, 0), // AUDIT_ARCH_X86_64
        statement(ret, libc::SECCOMP_RET_KILL_PROCESS),
        statement(load, 0), // seccomp_data.nr
        jump(
            libc::BPF_JMP | libc::BPF_JGE | libc::BPF_K,
            0x40000000,
            0,
            1,
        ),
        statement(ret, libc::SECCOMP_RET_KILL_PROCESS), // reject x32 ABI
    ];
    for syscall in [
        libc::SYS_fork,
        libc::SYS_vfork,
        libc::SYS_setrlimit,
        libc::SYS_prctl,
    ] {
        filter.push(jump(equal, syscall as u32, 0, 1));
        filter.push(statement(ret, denied));
    }
    // libc pthreads may try clone3 before falling back to clone. Its pointer
    // argument cannot be safely inspected by classic BPF: never allow it.
    filter.extend([
        jump(equal, libc::SYS_clone3 as u32, 0, 1),
        statement(ret, libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32),
        jump(equal, libc::SYS_clone as u32, 0, 4),
        statement(load, 16), // clone flags, low 32 bits of argument 0
        jump(
            libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K,
            libc::CLONE_THREAD as u32,
            1,
            0,
        ),
        statement(ret, denied),
        statement(ret, libc::SECCOMP_RET_ALLOW),
        // Allow querying resource limits, never changing their hard ceilings.
        jump(equal, libc::SYS_prlimit64 as u32, 0, 7),
        statement(load, 32), // argument 2: new_limit pointer, low half
        jump(equal, 0, 1, 0),
        statement(ret, denied),
        statement(load, 36), // high half
        jump(equal, 0, 1, 0),
        statement(ret, denied),
        statement(ret, libc::SECCOMP_RET_ALLOW),
        statement(ret, libc::SECCOMP_RET_ALLOW),
    ]);
    filter
}

fn spawn(executable: &Path, args: &[&str], limits: WorkerLimits) -> Result<Process, WorkerError> {
    let parent = unsafe { libc::getpid() };
    let filter = process_filter();
    let mut command = Command::new(executable);
    command
        .args(args)
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        command.pre_exec(move || {
            // SIGKILL also covers the race where the parent dies between fork
            // and installing PDEATHSIG. No child code has executed at this point.
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                libc::_exit(125);
            }
            let memory = libc::rlimit {
                rlim_cur: limits.memory_bytes as libc::rlim_t,
                rlim_max: limits.memory_bytes as libc::rlim_t,
            };
            let no_core = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &memory) != 0
                || libc::setrlimit(libc::RLIMIT_CORE, &no_core) != 0
                || libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
            {
                return Err(io::Error::last_os_error());
            }
            let program = libc::sock_fprog {
                len: filter.len() as u16,
                filter: filter.as_ptr() as *mut libc::sock_filter,
            };
            if libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(Process(command.spawn()?))
}

fn read_bounded(mut pipe: impl Read, limit: usize) -> Result<Vec<u8>, WorkerError> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let size = pipe.read(&mut buffer)?;
        if size == 0 {
            return Ok(output);
        }
        if output.len() + size > limit {
            return Err(WorkerError::OutputLimit);
        }
        output.extend_from_slice(&buffer[..size]);
    }
}

pub(super) fn run(
    executable: &Path,
    args: &[&str],
    input: &[u8],
    limits: WorkerLimits,
) -> Result<Vec<u8>, WorkerError> {
    limits.validate()?;
    if input.len() > limits.input_bytes {
        return Err(WorkerError::InputLimit);
    }
    let started = Instant::now();
    let mut process = spawn(executable, args, limits)?;
    let mut stdin = process
        .0
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("worker stdin missing"))?;
    let stdout = process
        .0
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("worker stdout missing"))?;
    let stderr = process
        .0
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("worker stderr missing"))?;
    let (tx, rx) = mpsc::channel();
    let out_tx = tx.clone();
    let err_tx = tx.clone();
    let input = input.to_vec();
    let writer = thread::spawn(move || {
        let result = stdin
            .write_all(&input)
            .map(|_| Vec::new())
            .map_err(WorkerError::Io);
        drop(stdin);
        let _ = tx.send((0, result));
    });
    let out = thread::spawn(move || {
        let _ = out_tx.send((1, read_bounded(stdout, limits.output_bytes)));
    });
    let err = thread::spawn(move || {
        let _ = err_tx.send((2, read_bounded(stderr, 65536)));
    });
    let mut output = None;
    let mut error = None;
    let mut exit = None;
    let mut completed = 0;
    loop {
        while let Ok((kind, result)) = rx.try_recv() {
            completed += 1;
            match result {
                Ok(bytes) if kind == 1 => output = Some(bytes),
                Ok(_) => {}
                Err(failure) => error = Some(failure),
            }
        }
        if error.is_some() {
            break;
        }
        if exit.is_none() {
            match process.0.try_wait() {
                Ok(status) => exit = status,
                Err(failure) => {
                    error = Some(failure.into());
                    break;
                }
            }
        }
        if exit.is_some() && completed == 3 {
            break;
        }
        if started.elapsed() >= Duration::from_millis(limits.deadline_ms.into()) {
            error = Some(WorkerError::Deadline);
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    // No process descendants can keep these pipes open: process clone/fork is
    // forbidden, while all permitted threads die with this sole process.
    drop(process);
    let _ = writer.join();
    let _ = out.join();
    let _ = err.join();
    if let Some(error) = error {
        return Err(error);
    }
    let status = exit.ok_or_else(|| io::Error::other("worker exit missing"))?;
    if !status.success() {
        return Err(WorkerError::ProcessExit(
            status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)) as u32,
        ));
    }
    output.ok_or_else(|| WorkerError::Protocol("missing worker response".into()))
}
