use super::*;
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

struct ChildGuard(std::process::Child);
impl std::ops::Deref for ChildGuard {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(crate) struct Git {
    root: std::path::PathBuf,
    deadline: Instant,
    processes: std::cell::Cell<usize>,
    cache: std::cell::RefCell<BTreeMap<String, (String, Vec<u8>)>>,
    cache_only: bool,
    export: Option<std::sync::Arc<layrs_store_local::anchored_export::ExportLease>>,
    export_source: Option<ExpectedBridgeState>,
    import_source: Option<layrs_store_local::anchored_export::GitImportSource>,
}
impl Git {
    #[cfg(test)]
    pub fn write_object(&self, kind: &str, bytes: &[u8]) -> Result<String> {
        let output = self.run(
            &["hash-object", "-w", "-t", kind, "--stdin"],
            Some(bytes),
            80,
        )?;
        Ok(hex_id(utf8(&output)?.trim())?.to_owned())
    }
    pub fn new(root: &Path, limits: GitBridgeLimits) -> Result<Self> {
        let root = std::path::absolute(root)?;
        if root.to_str().is_none_or(|path| path.contains(['\n', '\r'])) {
            return Err(invalid("Git host path must be UTF-8 without newlines"));
        }
        Ok(Self {
            root,
            processes: std::cell::Cell::new(0),
            cache: Default::default(),
            cache_only: false,
            export: None,
            export_source: None,
            import_source: None,
            deadline: Instant::now()
                .checked_add(Duration::from_millis(limits.max_duration_ms))
                .ok_or(BridgeError::Limit("duration"))?,
        })
    }
    pub fn processes(&self) -> usize {
        self.processes.get()
    }
    pub fn for_import(root: &Path, limits: GitBridgeLimits) -> Result<Self> {
        let mut git = Self::new(root, limits)?;
        git.import_source =
            Some(layrs_store_local::anchored_export::GitImportSource::capture(root)?);
        git.root = Path::new(".").to_path_buf();
        Ok(git)
    }
    pub fn import_common_path(&self) -> Result<&Path> {
        self.import_source
            .as_ref()
            .map(|source| source.common_path())
            .ok_or_else(|| invalid("Git source binding missing"))
    }
    pub fn verify_import_source(&self) -> Result<()> {
        if let Some(source) = &self.import_source {
            source.verify()?;
        }
        Ok(())
    }
    pub fn for_export(
        lease: std::sync::Arc<layrs_store_local::anchored_export::ExportLease>,
        expected: &ExpectedBridgeState,
        limits: GitBridgeLimits,
    ) -> Result<Self> {
        let mut git = Self::new(Path::new("."), limits)?;
        // prepare_command sets the actual cwd through an anchored capability.
        git.root = Path::new(".").to_path_buf();
        git.export = Some(lease);
        git.export_source = Some(expected.clone());
        Ok(git)
    }
    pub(crate) fn cached(
        objects: BTreeMap<String, (String, Vec<u8>)>,
        limits: GitBridgeLimits,
    ) -> Result<Self> {
        let mut git = Self::new(Path::new("."), limits)?;
        git.cache = std::cell::RefCell::new(objects);
        git.cache_only = true;
        Ok(git)
    }
    pub(crate) fn take_cache(&self) -> BTreeMap<String, (String, Vec<u8>)> {
        self.cache.take()
    }
    pub fn run(&self, args: &[&str], input: Option<&[u8]>, cap: usize) -> Result<Vec<u8>> {
        self.run_owned(args, input.map(<[u8]>::to_vec), cap)
    }
    pub fn run_owned(&self, args: &[&str], input: Option<Vec<u8>>, cap: usize) -> Result<Vec<u8>> {
        if self.cache_only {
            return Err(invalid("frozen Git scan cannot launch a subprocess"));
        }
        if Instant::now() >= self.deadline {
            return Err(BridgeError::Limit("Git deadline"));
        }
        let mut command = Command::new("git");
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                command.env_remove(key);
            }
        }
        let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", null)
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .args([
                "-c",
                &format!("core.hooksPath={null}"),
                "-c",
                "core.fsmonitor=false",
                "-c",
                "protocol.allow=never",
                "-c",
                "gc.auto=0",
                "-c",
                "maintenance.auto=false",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
                "-C",
            ])
            .arg(&self.root);
        if self.export.is_some() || self.import_source.is_some() {
            // Never discover a repository above the anchored directory. In
            // particular, a damaged resume HEAD must not select its parent.
            command.arg("--git-dir=.");
        }
        command
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        if let Some(source) = &self.export_source {
            source.accept_guard().map_err(invalid)?;
        }
        let _export_guard = self
            .export
            .as_ref()
            .map(|export| export.prepare_command(&mut command))
            .transpose()?;
        let _import_guard = self
            .import_source
            .as_ref()
            .map(|source| source.prepare_command(&mut command))
            .transpose()?;
        let mut child = ChildGuard(command.spawn()?);
        self.processes.set(self.processes.get() + 1);
        let stdin = input.map(|input| {
            let mut pipe = child.stdin.take().unwrap();
            let bytes = input;
            std::thread::spawn(move || pipe.write_all(&bytes))
        });
        let output = child.stdout.take().unwrap();
        let error = child.stderr.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let err_sender = sender.clone();
        let out_thread = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = output
                .take(cap.saturating_add(1) as u64)
                .read_to_end(&mut bytes);
            let _ = sender.send((false, result.map(|_| bytes)));
        });
        let err_thread = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = error.take(65537).read_to_end(&mut bytes);
            let _ = err_sender.send((true, result.map(|_| bytes)));
        });
        let mut stdout = None;
        let mut stderr = None;
        let mut limit = None;
        let status = loop {
            while let Ok((is_error, result)) = receiver.try_recv() {
                let bytes = result?;
                if bytes.len() > if is_error { 65536 } else { cap } {
                    limit = Some("Git output");
                }
                if is_error {
                    stderr = Some(bytes);
                } else {
                    stdout = Some(bytes);
                }
            }
            if Instant::now() >= self.deadline {
                limit = Some("Git deadline");
            }
            if limit.is_some() {
                let _ = child.kill();
                break child.wait()?;
            }
            if let Some(status) = child.try_wait()? {
                break status;
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        out_thread
            .join()
            .map_err(|_| invalid("Git output reader failed"))?;
        err_thread
            .join()
            .map_err(|_| invalid("Git error reader failed"))?;
        while let Ok((is_error, result)) = receiver.try_recv() {
            let bytes = result?;
            if bytes.len() > if is_error { 65536 } else { cap } {
                limit = Some("Git output");
            }
            if is_error {
                stderr = Some(bytes);
            } else {
                stdout = Some(bytes);
            }
        }
        if let Some(thread) = stdin {
            let _ = thread.join();
        }
        if let Some(export) = &self.export {
            export.verify()?;
        }
        if let Some(source) = &self.export_source {
            source.accept_guard().map_err(invalid)?;
        }
        self.verify_import_source()?;
        if let Some(limit) = limit {
            return Err(BridgeError::Limit(limit));
        }
        if !status.success() {
            return Err(BridgeError::Git(
                String::from_utf8_lossy(&stderr.unwrap_or_default()).into_owned(),
            ));
        }
        Ok(stdout.unwrap_or_default())
    }
    pub fn object(&self, kind: &str, id: &str, limits: GitBridgeLimits) -> Result<Vec<u8>> {
        hex_id(id)?;
        if let Some((actual, bytes)) = self.cache.borrow().get(id) {
            if actual != kind {
                return Err(invalid("Git object kind mismatch"));
            }
            return Ok(bytes.clone());
        }
        let mut batch = self.read_batch(&[id], limits)?;
        let (actual, bytes) = batch
            .remove(id)
            .ok_or_else(|| invalid("Git object missing"))?;
        if actual != kind {
            return Err(invalid("Git object kind mismatch"));
        }
        self.cache
            .borrow_mut()
            .insert(id.to_owned(), (actual, bytes.clone()));
        Ok(bytes)
    }
    pub fn prefetch(&self, ids: &[&str], limits: GitBridgeLimits) -> Result<()> {
        self.cache
            .borrow_mut()
            .extend(self.read_batch(ids, limits)?);
        Ok(())
    }
    fn read_batch(
        &self,
        ids: &[&str],
        limits: GitBridgeLimits,
    ) -> Result<BTreeMap<String, (String, Vec<u8>)>> {
        if ids.len() > limits.max_objects {
            return Err(BridgeError::Limit("Git objects"));
        }
        let mut input = String::new();
        for id in ids {
            hex_id(id)?;
            input.push_str(id);
            input.push('\n');
        }
        let output = self.run(
            &["cat-file", "--batch"],
            Some(input.as_bytes()),
            limits
                .max_total_bytes
                .saturating_add(ids.len().saturating_mul(128)),
        )?;
        let mut remaining = output.as_slice();
        let mut total = 0usize;
        let mut result = BTreeMap::new();
        for expected in ids {
            let newline = remaining
                .iter()
                .position(|b| *b == b'\n')
                .ok_or_else(|| invalid("batch header missing"))?;
            if newline > 127 {
                return Err(invalid("batch header limit"));
            }
            let fields = utf8(&remaining[..newline])?.split(' ').collect::<Vec<_>>();
            if fields.len() != 3
                || fields[0] != *expected
                || !["commit", "tree", "blob", "tag"].contains(&fields[1])
            {
                return Err(invalid("batch object binding"));
            }
            let size: usize = fields[2].parse().map_err(invalid)?;
            if size > limits.max_object_bytes {
                return Err(BridgeError::Limit("Git object bytes"));
            }
            total = total
                .checked_add(size)
                .ok_or(BridgeError::Limit("Git batch bytes"))?;
            if total > limits.max_total_bytes {
                return Err(BridgeError::Limit("Git batch bytes"));
            }
            let kind = fields[1].to_owned();
            remaining = &remaining[newline + 1..];
            if remaining.len() <= size || remaining[size] != b'\n' {
                return Err(invalid("batch content framing"));
            }
            let bytes = &remaining[..size];
            verify_git_id(&kind, expected, bytes)?;
            result.insert((*expected).to_owned(), (kind, bytes.to_vec()));
            remaining = &remaining[size + 1..];
        }
        if !remaining.is_empty() {
            return Err(invalid("unexpected batch suffix"));
        }
        Ok(result)
    }
}

pub(crate) fn verify_git_id(kind: &str, id: &str, bytes: &[u8]) -> Result<()> {
    use sha2::Digest;
    let header = format!("{kind} {}\0", bytes.len());
    let digest = if id.len() == 40 {
        let mut h = sha1::Sha1::new();
        h.update(header.as_bytes());
        h.update(bytes);
        format!("{:x}", h.finalize())
    } else {
        let mut h = sha2::Sha256::new();
        h.update(header.as_bytes());
        h.update(bytes);
        format!("{:x}", h.finalize())
    };
    if digest != id {
        return Err(invalid("Git object digest mismatch"));
    }
    Ok(())
}
