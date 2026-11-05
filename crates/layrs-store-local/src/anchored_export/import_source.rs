//! Exact Git acquisition roots; directory discovery is never delegated upward.
use super::*;

pub struct GitImportSource {
    source: PinnedDirectory,
    git: PinnedDirectory,
    common: PinnedDirectory,
    git_path: PathBuf,
    common_path: PathBuf,
    locator: Option<FileSnapshot>,
    common_locator: Option<FileSnapshot>,
    _locks: Vec<File>,
}

/// Canonical Windows paths use the verbatim prefix, where the OS does not
/// normalize `..`. Resolve Git's relative locators before canonicalization.
/// Only leading parents are safe to collapse: `link/..` may resolve elsewhere
/// when Git follows a symlink, so refuse parents after any normal component.
fn resolve_locator(base: &Path, target: &str) -> io::Result<PathBuf> {
    use std::path::Component;
    if target.is_empty() || !base.is_absolute() {
        return Err(stale());
    }
    let target = Path::new(target);
    let absolute = target.is_absolute();
    let mut path = if absolute {
        PathBuf::new()
    } else {
        base.to_path_buf()
    };
    let mut normal_seen = false;
    for part in target.components() {
        match part {
            Component::Prefix(_) | Component::RootDir if absolute => path.push(part.as_os_str()),
            Component::Prefix(_) | Component::RootDir => return Err(stale()),
            Component::ParentDir => {
                if absolute
                    || normal_seen
                    || !matches!(path.components().next_back(), Some(Component::Normal(_)))
                    || !path.pop()
                {
                    return Err(stale());
                }
            }
            Component::CurDir => {}
            Component::Normal(name) => {
                normal_seen = true;
                path.push(name);
            }
        }
    }
    std::fs::canonicalize(path)
}
fn link_text(
    directory: &PinnedDirectory,
    name: &str,
) -> io::Result<Option<(String, FileSnapshot)>> {
    let mut file = match directory.open_file(OsStr::new(name)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.file_mut().take(16385).read_to_end(&mut bytes)?;
    if bytes.len() > 16384 || !file.same_file(&directory.open_file(OsStr::new(name))?) {
        return Err(stale());
    }
    // Parse exactly the bytes being bound, never a second unbound locator read.
    let snapshot = FileSnapshot {
        length: bytes.len() as u64,
        hash: *blake3::hash(&bytes).as_bytes(),
    };
    let value = String::from_utf8(bytes).map_err(io::Error::other)?;
    let value = value.trim_end_matches(['\n', '\r']);
    if value.is_empty() || value.contains(['\n', '\r', '\0']) {
        return Err(stale());
    }
    Ok(Some((value.to_owned(), snapshot)))
}
impl GitImportSource {
    pub fn capture(source: &Path) -> io::Result<Self> {
        let source_path = std::fs::canonicalize(source)?;
        let source = PinnedDirectory::open(&source_path)?;
        let (git_path, locator) = match source.open_child_for_traversal(OsStr::new(".git")) {
            Ok(_) => (source_path.join(".git"), None),
            Err(_) => match link_text(&source, ".git")? {
                Some((value, snapshot)) => {
                    let target = value.strip_prefix("gitdir: ").ok_or_else(stale)?;
                    (resolve_locator(&source_path, target)?, Some(snapshot))
                }
                None => (source_path.clone(), None),
            },
        };
        let git = PinnedDirectory::open(&git_path)?;
        // An arbitrary subfolder has neither a local locator nor its own HEAD.
        // It must fail here instead of selecting an enclosing repository.
        git.open_file(OsStr::new("HEAD"))?;
        let (common_path, common_locator) = match link_text(&git, "commondir")? {
            Some((value, snapshot)) => (resolve_locator(&git_path, &value)?, Some(snapshot)),
            None => (git_path.clone(), None),
        };
        let common = PinnedDirectory::open(&common_path)?;
        common.open_child_for_traversal(OsStr::new("objects"))?;
        common.open_child_for_traversal(OsStr::new("refs"))?;
        let locks = lock_paths(&[source_path, git_path.clone(), common_path.clone()])?;
        let result = Self {
            source,
            git,
            common,
            git_path,
            common_path,
            locator,
            common_locator,
            _locks: locks,
        };
        result.verify()?;
        Ok(result)
    }
    pub fn common_path(&self) -> &Path {
        &self.common_path
    }
    pub fn verify(&self) -> io::Result<()> {
        self.source.verify_path_identity()?;
        self.git.verify_path_identity()?;
        self.common.verify_path_identity()?;
        // For directory locators, opening as a file fails; the pinned git
        // directory identity above binds that case without following links.
        if (self.locator.is_some()
            && read_optional(&self.source, OsStr::new(".git"), 16384)? != self.locator)
            || read_optional(&self.git, OsStr::new("commondir"), 16384)? != self.common_locator
        {
            return Err(stale());
        }
        Ok(())
    }
    pub fn prepare_command(&self, command: &mut Command) -> io::Result<ExportCommandGuard> {
        self.verify()?;
        // All acquisition commands are read-only; explicit gitdir plus the
        // retained cwd disallows fallback even if HEAD becomes corrupt.
        anchor_command(&self.git, &self.git_path, command)?;
        Ok(ExportCommandGuard { _locks: vec![] })
    }
}
