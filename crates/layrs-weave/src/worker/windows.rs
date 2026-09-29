//! A Job is attached atomically by CreateProcess's JOB_LIST attribute. There is
//! no pre-assignment execution window and no suspended orphan on parent crash.
use super::*;
use std::{
    ffi::{OsStr, c_void},
    fs::File,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    ptr,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::SECURITY_ATTRIBUTES,
    System::{JobObjects::*, Pipes::CreatePipe, Threading::*},
};

fn checked(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}
fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}
fn quoted(value: &str) -> String {
    let mut output = String::from("\"");
    let mut slash = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slash += 1;
            continue;
        }
        if ch == '"' {
            output.extend(std::iter::repeat_n('\\', slash * 2 + 1));
        } else {
            output.extend(std::iter::repeat_n('\\', slash));
        }
        slash = 0;
        output.push(ch);
    }
    output.extend(std::iter::repeat_n('\\', slash * 2));
    output.push('"');
    output
}
fn pipe(parent_reads: bool) -> io::Result<(File, OwnedHandle)> {
    unsafe {
        let attrs = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: ptr::null_mut(),
            bInheritHandle: 1,
        };
        let (mut read, mut write) = (ptr::null_mut(), ptr::null_mut());
        checked(CreatePipe(&mut read, &mut write, &attrs, 0))?;
        let read = owned(read)?;
        let write = owned(write)?;
        let (parent, child) = if parent_reads {
            (read, write)
        } else {
            (write, read)
        };
        checked(SetHandleInformation(
            parent.as_raw_handle(),
            HANDLE_FLAG_INHERIT,
            0,
        ))?;
        Ok((File::from(parent), child))
    }
}
struct Attributes {
    _storage: Vec<usize>,
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.list) }
    }
}
struct Process {
    handle: OwnedHandle,
    job: OwnedHandle,
}
impl Process {
    fn kill(&self) {
        unsafe {
            TerminateJobObject(self.job.as_raw_handle(), 1);
            WaitForSingleObject(self.handle.as_raw_handle(), 5000);
        }
    }
    fn exited(&self) -> io::Result<Option<u32>> {
        unsafe {
            match WaitForSingleObject(self.handle.as_raw_handle(), 0) {
                WAIT_OBJECT_0 => {
                    let mut code = 0;
                    checked(GetExitCodeProcess(self.handle.as_raw_handle(), &mut code))?;
                    Ok(Some(code))
                }
                WAIT_TIMEOUT => Ok(None),
                _ => Err(io::Error::last_os_error()),
            }
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.kill();
    }
}

fn spawn(
    executable: &Path,
    args: &[&str],
    limits: WorkerLimits,
) -> io::Result<(Process, File, File, File)> {
    let executable = executable.canonicalize()?;
    let exe = executable
        .to_str()
        .ok_or_else(|| io::Error::other("worker executable path must be Unicode"))?;
    let application = wide(executable.as_os_str());
    let cwd = wide(
        executable
            .parent()
            .ok_or_else(|| io::Error::other("worker has no parent"))?
            .as_os_str(),
    );
    let command = std::iter::once(exe)
        .chain(args.iter().copied())
        .map(quoted)
        .collect::<Vec<_>>()
        .join(" ");
    let mut command = wide(OsStr::new(&command));
    let (stdin, inherited_stdin) = pipe(false)?;
    let (stdout, inherited_stdout) = pipe(true)?;
    let (stderr, inherited_stderr) = pipe(true)?;
    unsafe {
        let job = owned(CreateJobObjectW(ptr::null(), ptr::null()))?;
        let mut information: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        information.BasicLimitInformation.ActiveProcessLimit = 1;
        information.ProcessMemoryLimit = limits.memory_bytes;
        checked(SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            &information as *const _ as *const c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ))?;
        let mut size = 0;
        InitializeProcThreadAttributeList(ptr::null_mut(), 2, 0, &mut size);
        if size == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
        let list = storage.as_mut_ptr().cast();
        checked(InitializeProcThreadAttributeList(list, 2, 0, &mut size))?;
        let attributes = Attributes {
            _storage: storage,
            list,
        };
        let jobs = [job.as_raw_handle()];
        checked(UpdateProcThreadAttribute(
            attributes.list,
            0,
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            jobs.as_ptr().cast(),
            size_of::<HANDLE>(),
            ptr::null_mut(),
            ptr::null(),
        ))?;
        let handles = [
            inherited_stdin.as_raw_handle(),
            inherited_stdout.as_raw_handle(),
            inherited_stderr.as_raw_handle(),
        ];
        checked(UpdateProcThreadAttribute(
            attributes.list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            handles.as_ptr().cast(),
            size_of_val(&handles),
            ptr::null_mut(),
            ptr::null(),
        ))?;
        let mut startup: STARTUPINFOEXW = zeroed();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = handles[0];
        startup.StartupInfo.hStdOutput = handles[1];
        startup.StartupInfo.hStdError = handles[2];
        startup.lpAttributeList = attributes.list;
        let mut process: PROCESS_INFORMATION = zeroed();
        let environment = [0u16, 0];
        checked(CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            environment.as_ptr().cast(),
            cwd.as_ptr(),
            &startup.StartupInfo,
            &mut process,
        ))?;
        let handle = owned(process.hProcess)?;
        drop(owned(process.hThread)?);
        Ok((Process { handle, job }, stdin, stdout, stderr))
    }
}

fn read_bounded(mut pipe: File, limit: usize) -> Result<Vec<u8>, WorkerError> {
    let mut result = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let n = pipe.read(&mut buffer)?;
        if n == 0 {
            return Ok(result);
        }
        if result.len() + n > limit {
            return Err(WorkerError::OutputLimit);
        }
        result.extend_from_slice(&buffer[..n]);
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
    let start = Instant::now();
    let (process, mut stdin, stdout, stderr) = spawn(executable, args, limits)?;
    let (tx, rx) = mpsc::channel();
    let out_tx = tx.clone();
    let err_tx = tx.clone();
    let input = input.to_vec();
    let writer = thread::spawn(move || {
        let result = stdin
            .write_all(&input)
            .map(|_| vec![])
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
    let mut completed = 0;
    let mut exit = None;
    loop {
        while let Ok((kind, result)) = rx.try_recv() {
            completed += 1;
            match result {
                Ok(bytes) if kind == 1 => output = Some(bytes),
                Ok(_) => {}
                Err(e) => {
                    error = Some(e);
                }
            }
        }
        if error.is_some() {
            break;
        }
        if exit.is_none() {
            match process.exited() {
                Ok(value) => exit = value,
                Err(e) => {
                    error = Some(e.into());
                    break;
                }
            }
        }
        if exit.is_some() && completed == 3 {
            break;
        }
        if start.elapsed() >= Duration::from_millis(limits.deadline_ms.into()) {
            error = Some(WorkerError::Deadline);
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    process.kill();
    let _ = writer.join();
    let _ = out.join();
    let _ = err.join();
    if let Some(error) = error {
        return Err(error);
    }
    if exit != Some(0) {
        return Err(WorkerError::ProcessExit(exit.unwrap_or(1)));
    }
    output.ok_or_else(|| WorkerError::Protocol("missing worker response".into()))
}
