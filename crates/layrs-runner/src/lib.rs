//! External exact-Step runner. Repository commands run only in an isolated
//! Docker container; credentials remain in the parent and are never mounted.
mod execution;
pub use execution::{Execution, execute_recipe};
use layrs_client_core::native_scm::NativeRemote;
use layrs_core::scm::{LayerId, RepoPath, SpaceId, StepId};
use layrs_store_local::scm_v1::checkout::{CheckoutContent, CheckoutEntry};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Recipe {
    pub image: String,
    pub command: Vec<String>,
    pub timeout_seconds: u64,
    pub maximum_log_bytes: usize,
}
impl Recipe {
    pub fn validate(&self) -> Result<()> {
        let hash = self
            .image
            .rsplit_once("sha256:")
            .ok_or("image must be pinned by sha256 digest")?
            .1;
        if hash.len() != 64
            || !hash.bytes().all(|b| b.is_ascii_hexdigit())
            || !(self.image.starts_with("sha256:") || self.image.contains("@sha256:"))
            || self.image.len() > 512
            || self.image.chars().any(char::is_whitespace)
        {
            return Err("invalid pinned image".into());
        }
        if self.command.is_empty()
            || self.command.len() > 64
            || !self.command[0].starts_with('/')
            || self
                .command
                .iter()
                .any(|a| a.len() > 8192 || a.contains('\0'))
            || !(1..=1800).contains(&self.timeout_seconds)
            || !(1024..=240 * 1024).contains(&self.maximum_log_bytes)
        {
            return Err("invalid bounded recipe".into());
        }
        Ok(())
    }
    pub fn object_id(&self) -> Result<String> {
        self.validate()?;
        let bytes = serde_jcs::to_vec(
            &serde_json::json!({"schemaVersion":"layrs.recipe.v1","recipe":self}),
        )?;
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-evidence-v1\0");
        hash.update(&bytes);
        Ok(format!("blake3:{}", hash.finalize().to_hex()))
    }
}
pub struct Job {
    pub origin: String,
    pub space: SpaceId,
    pub step: StepId,
    pub layer: LayerId,
    pub recipe: Recipe,
    pub directory: PathBuf,
    pub ca_pem: Option<Vec<u8>>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Completion {
    pub origin: String,
    pub space_id: SpaceId,
    pub step_id: StepId,
    pub run_id: String,
    pub outcome: String,
    pub log: String,
}

pub fn run(job: Job, token: &str) -> Result<Completion> {
    job.recipe.validate()?;
    if !job.directory.is_absolute()
        || job
            .directory
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err("run directory must be a new absolute path".into());
    }
    let remote = match &job.ca_pem {
        Some(pem) => NativeRemote::with_trusted_ca(&job.origin, token, pem)?,
        None => NativeRemote::new(&job.origin, token)?,
    };
    let client = client(job.ca_pem.as_deref())?;
    let base = format!("{}v3/spaces/{}", remote.origin(), job.space);
    let protection = request(
        client
            .get(format!("{base}/layers/{}/protection", job.layer))
            .bearer_auth(token),
    )?;
    if protection["recipeObjectId"].as_str() != Some(&job.recipe.object_id()?) {
        return Err("local recipe does not match the configured immutable recipe".into());
    }
    create_private(&job.directory)?;
    let checkout = job.directory.join("checkout");
    fs::create_dir(&checkout)?;
    let attempt = format!("runner-attempt-{}", uuid::Uuid::new_v4());
    let started=request(client.post(format!("{base}/runs/pending")).bearer_auth(token).json(&serde_json::json!({"idempotencyKey":attempt,"stepId":job.step,"recipeObjectId":job.recipe.object_id()?})))?;
    let run_id = started["runId"]
        .as_str()
        .ok_or("server omitted run ID")?
        .to_owned();
    validate_run_id(&run_id)?;
    write_new(
        &job.directory.join("run.json"),
        &serde_json::json!({"origin":remote.origin(),"spaceId":job.space,"stepId":job.step,"runId":run_id,"attempt":attempt,"recipe":job.recipe}),
    )?;
    // Registered preparation is visible while fetching/verifying the exact Step.
    // A failed preparation leaves no success claim and remains cancellable.
    let entries = remote.fetch_step_files(&job.space, &job.step)?;
    materialize(&checkout, entries)?;
    request(
        client
            .post(format!("{base}/runs/{run_id}/begin"))
            .bearer_auth(token)
            .json(&serde_json::json!({})),
    )?;
    let execution = execute_recipe(&job.recipe, &checkout)?;
    let completion = Completion {
        origin: remote.origin().into(),
        space_id: job.space,
        step_id: job.step,
        run_id,
        outcome: if execution.passed { "passed" } else { "failed" }.into(),
        log: execution.log,
    };
    // Durable completion precedes callback. Retry uses identical bytes after a lost reply.
    write_new(&job.directory.join("completion.json"), &completion)?;
    submit(&completion, token, job.ca_pem.as_deref())?;
    Ok(completion)
}
pub fn submit(completion: &Completion, token: &str, ca: Option<&[u8]>) -> Result<()> {
    validate_run_id(&completion.run_id)?;
    if !matches!(completion.outcome.as_str(), "passed" | "failed")
        || completion.log.len() > 256 * 1024
    {
        return Err("invalid completion".into());
    }
    let remote = match ca {
        Some(pem) => NativeRemote::with_trusted_ca(&completion.origin, token, pem)?,
        None => NativeRemote::new(&completion.origin, token)?,
    };
    request(
        client(ca)?
            .post(format!(
                "{}v3/spaces/{}/runs/{}/complete",
                remote.origin(),
                completion.space_id,
                completion.run_id
            ))
            .bearer_auth(token)
            .json(&serde_json::json!({"outcome":completion.outcome,"log":completion.log})),
    )?;
    Ok(())
}
fn validate_run_id(id: &str) -> Result<()> {
    if !id.starts_with("run:")
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b))
    {
        return Err("invalid run ID".into());
    }
    Ok(())
}
fn client(ca: Option<&[u8]>) -> Result<reqwest::blocking::Client> {
    let mut builder = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30));
    if let Some(pem) = ca {
        builder = builder.add_root_certificate(reqwest::Certificate::from_pem(pem)?)
    }
    Ok(builder.build()?)
}
fn request(builder: reqwest::blocking::RequestBuilder) -> Result<serde_json::Value> {
    let response = builder.send().map_err(|_| "runner HTTP transport failed")?;
    if !response.status().is_success() {
        return Err(format!("runner HTTP status {}", response.status().as_u16()).into());
    }
    let mut bytes = vec![];
    response.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err("runner response too large".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}
fn create_private(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or("run directory has no parent")?;
    layrs_store_local::scm_v1::checkout::CheckoutReader::open(parent, None)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    #[cfg(not(unix))]
    fs::create_dir(path)?;
    Ok(())
}
fn write_new<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&serde_jcs::to_vec(value)?)?;
    file.sync_all()?;
    Ok(())
}
fn materialize(root: &Path, entries: Vec<CheckoutEntry>) -> Result<()> {
    // Validate the entire namespace before writing any checkout entry.
    #[cfg(windows)]
    {
        let mut names = std::collections::BTreeMap::<String, String>::new();
        for entry in &entries {
            RepoPath::parse(&entry.path)?;
            let mut prefix = String::new();
            for component in entry.path.split('/') {
                let base = component
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .to_ascii_uppercase();
                if component.ends_with(['.', ' '])
                    || component.chars().any(|c| c < ' ' || "<>:\"|?*".contains(c))
                    || matches!(
                        base.as_str(),
                        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                    )
                    || ["COM", "LPT"].iter().any(|name| {
                        base.strip_prefix(name).is_some_and(|number| {
                            matches!(
                                number,
                                "1" | "2"
                                    | "3"
                                    | "4"
                                    | "5"
                                    | "6"
                                    | "7"
                                    | "8"
                                    | "9"
                                    | "¹"
                                    | "²"
                                    | "³"
                            )
                        })
                    })
                {
                    return Err("checkout contains a nonportable Windows path".into());
                }
                if !prefix.is_empty() {
                    prefix.push('/')
                }
                prefix.push_str(component);
                let folded = prefix.to_lowercase();
                if names
                    .get(&folded)
                    .is_some_and(|previous| previous != &prefix)
                {
                    return Err("checkout contains a case-colliding Windows path".into());
                }
                names.insert(folded, prefix.clone());
            }
            if matches!(
                entry.content,
                CheckoutContent::File {
                    executable: true,
                    ..
                }
            ) {
                return Err("runner cannot preserve executable bits on Windows".into());
            }
        }
    }
    for entry in entries {
        RepoPath::parse(&entry.path)?;
        let path = root.join(&entry.path);
        let parent = path.parent().ok_or("invalid checkout entry")?;
        fs::create_dir_all(parent)?;
        // No links are accepted by the initial runner: a repository cannot use
        // an alias to reach a mount outside its immutable checkout.
        let CheckoutContent::File { bytes, executable } = entry.content else {
            return Err("runner recipe does not support symlinks".into());
        };
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        file.write_all(&bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(if executable {
                0o755
            } else {
                0o644
            }))?;
        }
        #[cfg(not(unix))]
        let _ = executable;
    }
    Ok(())
}
