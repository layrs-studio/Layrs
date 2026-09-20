//! Bounded native V3 HTTP synchronization. Bearer secrets never enter the Store.
//! Network work holds no native Store capability; acceptance uses exact local CAS.
use super::{prepare, *};
use layrs_core::scm::*;
use layrs_graph::scm::{normalize_heads, StepDag};
use layrs_store::scm::{
    CanonicalConflict, CanonicalObject, CanonicalProduct, EntityRecord, FileAvailability,
    OperationHeadsCas, RepositoryViewCas, ScmReader, StoreMutation, StoreObjectId,
    WorktreeActiveOperationCas, WriteBatch,
};
use layrs_store_local::scm_v1::checkout::{CheckoutContent, CheckoutEntry};
use layrs_store_local::scm_v1::{self, NativeScmReader};
use layrs_sync::v3::*;
use reqwest::{
    blocking::{Client, Response},
    Url,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
    time::Duration,
};

const MAX_OBJECT: u64 = 64 * 1024 * 1024;
const MAX_TOTAL: usize = 256 * 1024 * 1024;
const MAX_FILE_STREAM: u64 = 1024 * 1024 * 1024;
const MAX_STREAM_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_OBJECTS: usize = 100_000;
pub(super) type RemoteResult<T> = std::result::Result<T, NativeRemoteError>;

#[derive(Debug)]
pub enum NativeRemoteError {
    ContentRestricted,
    Local(NativeClientError),
    Io(std::io::Error),
    Json(serde_json::Error),
    Invalid(&'static str),
    Http(u16),
    Service { status: u16, code: String },
    Transport,
    Cancelled,
    PublicationPending,
}
impl std::fmt::Display for NativeRemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ContentRestricted => f.write_str("content access is restricted; a complete clone or receive cannot change this checkout"),
            Self::Cancelled => f.write_str("remote transfer cancelled before commit"),
            Self::PublicationPending => f.write_str("an earlier publication may already be committed; cancellation cannot confirm its outcome, retry the exact pending publication"),
            Self::Local(e) => e.fmt(f),
            Self::Io(e) => e.fmt(f),
            Self::Json(e) => e.fmt(f),
            Self::Invalid(e) => f.write_str(e),
            Self::Http(s) => write!(f, "remote HTTP status {s}"),
            Self::Service { status, code } => write!(f, "remote HTTP status {status}: {code}"),
            Self::Transport => {
                f.write_str("remote transport failed; pending publication remains retryable")
            }
        }
    }
}
impl std::error::Error for NativeRemoteError {}
impl NativeRemoteError {
    pub fn code(&self) -> &str {
        match self {
            Self::ContentRestricted => "content_access_restricted",
            Self::Cancelled => "remote_transfer_cancelled",
            Self::PublicationPending => "publication_pending",
            Self::Local(_) => "native_local_error",
            Self::Io(_) => "native_remote_journal_io",
            Self::Json(_) => "invalid_remote_json",
            Self::Invalid(_) => "invalid_native_remote_state",
            Self::Http(401) | Self::Service { status: 401, .. } => "remote_unauthorized",
            Self::Http(403) | Self::Service { status: 403, .. } => "remote_forbidden",
            Self::Service { code, .. } => code,
            Self::Http(_) => "remote_http_error",
            Self::Transport => "remote_transport_failed",
        }
    }
}
impl From<NativeClientError> for NativeRemoteError {
    fn from(e: NativeClientError) -> Self {
        Self::Local(e)
    }
}
impl From<std::io::Error> for NativeRemoteError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for NativeRemoteError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
impl From<layrs_core::scm::ScmError> for NativeRemoteError {
    fn from(e: layrs_core::scm::ScmError) -> Self {
        Self::Local(e.into())
    }
}
impl From<layrs_store::scm::StoreError> for NativeRemoteError {
    fn from(e: layrs_store::scm::StoreError) -> Self {
        Self::Local(e.into())
    }
}
impl From<scm_v1::NativeStoreError> for NativeRemoteError {
    fn from(e: scm_v1::NativeStoreError) -> Self {
        Self::Local(e.into())
    }
}
impl From<reqwest::Error> for NativeRemoteError {
    fn from(_: reqwest::Error) -> Self {
        Self::Transport
    }
}

/// Synchronous facade for CLI/Desktop workers. Do not call from a Tokio task.
pub struct NativeRemote {
    http: Client,
    origin: Url,
    token: String,
    transfer: Option<NativeTransferControl>,
    download_cache: Option<std::path::PathBuf>,
}
/// Secret-bearing result intentionally has no Debug/Serialize implementation.
#[derive(Deserialize)]
pub struct NativeRemoteSession {
    pub principal_id: PrincipalId,
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeDeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}
pub enum NativeDevicePoll {
    Pending { interval: u64 },
    Authorized(NativeRemoteSession),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteProvisionedSpace {
    pub workspace_id: String,
    pub space_id: SpaceId,
    pub layer_id: LayerId,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveReport {
    pub operation_id: OperationId,
    pub divergent: bool,
    pub materialized: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    origin: String,
    space_id: SpaceId,
    client_id: ClientId,
    clone_pending: bool,
    #[serde(default)]
    observed: Option<SpaceBootstrap>,
    pending: Option<SyncTransactionRequest>,
    last_receipt: Option<SyncTransactionReceipt>,
}

impl NativeRemote {
    pub fn new(origin: &str, token: &str) -> RemoteResult<Self> {
        Self::build(origin, token, None)
    }
    /// Trust an explicit private CA in addition to the platform's public roots.
    /// Certificate hostname and chain verification remain enabled.
    pub fn with_trusted_ca(origin: &str, token: &str, pem: &[u8]) -> RemoteResult<Self> {
        Self::build(origin, token, Some(pem))
    }
    pub fn unauthenticated(origin: &str, ca: Option<&[u8]>) -> RemoteResult<Self> {
        Self::build(origin, "anonymous", ca)
    }
    pub fn origin(&self) -> &str {
        self.origin.as_str()
    }
    pub fn whoami(&self) -> RemoteResult<serde_json::Value> {
        self.get("v3/auth/whoami")
    }
    pub fn logout(&self) -> RemoteResult<()> {
        checked(
            self.http
                .post(self.url("v3/auth/logout")?)
                .bearer_auth(&self.token)
                .send()?,
        )?;
        Ok(())
    }
    pub fn device_start(&self, label: &str) -> RemoteResult<NativeDeviceStart> {
        let value: NativeDeviceStart = json(
            self.http
                .post(self.url("v3/auth/devices/start")?)
                .json(&serde_json::json!({"label":label}))
                .send()?,
        )?;
        let uri = self.transfer_url(&value.verification_uri)?;
        if uri.path() != "/auth/device"
            || value.device_code.len() != 64
            || !value.device_code.bytes().all(|b| b.is_ascii_hexdigit())
            || value.user_code.len() > 32
            || value.user_code.chars().any(char::is_control)
            || !(1..=60).contains(&value.interval)
            || !(1..=3600).contains(&value.expires_in)
        {
            return Err(NativeRemoteError::Invalid(
                "invalid device authorization response",
            ));
        }
        Ok(value)
    }
    pub fn device_poll(&self, code: &str) -> RemoteResult<NativeDevicePoll> {
        if code.len() != 64 || !code.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(NativeRemoteError::Invalid("invalid device code"));
        }
        let response = self
            .http
            .post(self.url("v3/auth/devices/poll")?)
            .json(&serde_json::json!({"deviceCode":code}))
            .send()?;
        if response.status().as_u16() == 429 {
            return Ok(NativeDevicePoll::Pending { interval: 10 });
        }
        if response.status().as_u16() == 202 {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Pending {
                status: String,
                interval: u64,
            }
            let pending: Pending = json(response)?;
            if pending.status != "authorization_pending" || !(1..=60).contains(&pending.interval) {
                return Err(NativeRemoteError::Invalid(
                    "invalid device pending response",
                ));
            }
            return Ok(NativeDevicePoll::Pending {
                interval: pending.interval,
            });
        }
        let session: NativeRemoteSession = json(response)?;
        validate_session(&session)?;
        Ok(NativeDevicePoll::Authorized(session))
    }
    pub fn login(
        origin: &str,
        login: &str,
        password: &str,
        ca: Option<&[u8]>,
    ) -> RemoteResult<NativeRemoteSession> {
        let remote = Self::build(origin, "login-pending", ca)?;
        let session: NativeRemoteSession = json(
            remote
                .http
                .post(remote.url("v3/auth/login")?)
                .json(&serde_json::json!({"login":login,"password":password}))
                .send()?,
        )?;
        validate_session(&session)?;
        Ok(session)
    }
    fn build(origin: &str, token: &str, ca: Option<&[u8]>) -> RemoteResult<Self> {
        let origin =
            Url::parse(origin).map_err(|_| NativeRemoteError::Invalid("invalid remote URL"))?;
        if origin.scheme() != "https"
            || origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
        {
            return Err(NativeRemoteError::Invalid(
                "remote requires a HTTPS origin without credentials, path or query",
            ));
        }
        if token.is_empty()
            || token.len() > 4000
            || token
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        {
            return Err(NativeRemoteError::Invalid("invalid bearer token"));
        }
        let mut builder = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120));
        if let Some(ca) = ca {
            if ca.len() > 1024 * 1024 {
                return Err(NativeRemoteError::Invalid("CA PEM exceeds 1 MiB"));
            }
            builder = builder.add_root_certificate(reqwest::Certificate::from_pem(ca)?);
        }
        let http = builder.build()?;
        Ok(Self {
            http,
            origin,
            token: token.into(),
            transfer: None,
            download_cache: None,
        })
    }
    fn url(&self, path: &str) -> RemoteResult<Url> {
        self.origin
            .join(path)
            .map_err(|_| NativeRemoteError::Invalid("invalid remote endpoint"))
    }
    fn transfer_url(&self, value: &str) -> RemoteResult<Url> {
        let url =
            Url::parse(value).map_err(|_| NativeRemoteError::Invalid("invalid transfer URL"))?;
        if url.origin() != self.origin.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(NativeRemoteError::Invalid(
                "transfer URL escapes configured origin",
            ));
        }
        Ok(url)
    }
    pub(super) fn post<T: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &T,
    ) -> RemoteResult<R> {
        self.transfer_check()?;
        self.transfer_result(
            self.http
                .post(self.url(path)?)
                .bearer_auth(&self.token)
                .json(body)
                .timeout(self.transfer_timeout())
                .send()
                .map_err(NativeRemoteError::from)
                .and_then(json),
        )
    }
    pub(super) fn get<R: DeserializeOwned>(&self, path: &str) -> RemoteResult<R> {
        self.transfer_check()?;
        self.transfer_result(
            self.http
                .get(self.url(path)?)
                .bearer_auth(&self.token)
                .timeout(self.transfer_timeout())
                .send()
                .map_err(NativeRemoteError::from)
                .and_then(json),
        )
    }
    pub(super) fn admin_request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> RemoteResult<serde_json::Value> {
        let mut request = self
            .http
            .request(method, self.url(path)?)
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(body)
        }
        let response = checked(request.send()?)?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            Ok(serde_json::Value::Null)
        } else {
            json(response)
        }
    }
    pub fn bootstrap(&self, space: &SpaceId) -> RemoteResult<SpaceBootstrap> {
        let value: SpaceBootstrap = self.get(&format!("v3/spaces/{space}/bootstrap"))?;
        value
            .validate_v3()
            .map_err(|_| NativeRemoteError::Invalid("invalid bootstrap"))?;
        if &value.space_id != space {
            return Err(NativeRemoteError::Invalid("bootstrap Space mismatch"));
        }
        Ok(value)
    }
    /// Verified immutable checkout inputs for an isolated recipe runner. This
    /// performs no checkout writes and does not select or publish Layer heads.
    pub fn fetch_step_files(
        &self,
        space: &SpaceId,
        step: &StepId,
    ) -> RemoteResult<Vec<CheckoutEntry>> {
        let mut bootstrap = self.bootstrap(space)?;
        bootstrap.graph_root_step_ids = vec![step.clone()];
        bootstrap.markers.clear();
        bootstrap.releases.clear();
        bootstrap.product_roots.clear();
        let binding = Binding {
            origin: self.origin.to_string(),
            space_id: space.clone(),
            client_id: ClientId::parse(format!("client:{}", uuid::Uuid::new_v4())).expect("UUID"),
            clone_pending: false,
            observed: None,
            pending: None,
            last_receipt: None,
        };
        self.download_graph(&binding, &bootstrap)?.entries(step)
    }
}

fn checked(response: Response) -> RemoteResult<Response> {
    if response.status().is_success() {
        Ok(response)
    } else {
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        if response.take(4097).read_to_end(&mut bytes).is_ok() && bytes.len() <= 4096 {
            if let Some(code) = service_error_code(&bytes) {
                return Err(NativeRemoteError::Service { status, code });
            }
        }
        Err(NativeRemoteError::Http(status))
    }
}
fn service_error_code(bytes: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let code = value.get("error")?.get("code")?.as_str()?;
    if code.is_empty()
        || code.len() > 96
        || !code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    Some(code.into())
}
fn validate_session(session: &NativeRemoteSession) -> RemoteResult<()> {
    if session.token_type != "Bearer"
        || session.access_token.len() != 64
        || !session.access_token.bytes().all(|b| b.is_ascii_hexdigit())
        || session.expires_in == 0
        || session.expires_in > 90 * 24 * 3600
    {
        return Err(NativeRemoteError::Invalid("invalid authentication session"));
    }
    Ok(())
}
fn projection_dirty(workspace: &NativeWorkspace, state: &WorkspaceState) -> RemoteResult<bool> {
    if workspace.needs_repair()? {
        return Err(NativeRemoteError::Local(NativeClientError::NeedsRepair));
    }
    let (_, snapshot) =
        workspace.capture_projection_streaming(state, super::restore_projection::limits())?;
    let expected = state
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == workspace.worktree_id())
        .and_then(|w| w.materialized_tree_id());
    Ok(expected != Some(snapshot.root_tree_id()))
}
fn validate_fetch_descriptors(plan: &FetchPlanResponse, claim: &FetchClaim) -> RemoteResult<()> {
    match claim {
        FetchClaim::Object { object, .. } => {
            if !plan.file_manifest_selections.is_empty()
                || plan.download_capabilities.len() != 1
                || plan.download_capabilities[0].descriptor.kind != object.kind
                || plan.download_capabilities[0].descriptor.object_id != object.object_id
            {
                return Err(NativeRemoteError::Invalid(
                    "object fetch substituted requested identity",
                ));
            }
        }
        FetchClaim::File { file_id, .. } => {
            if plan.file_manifest_selections.len() != 1 {
                return Err(NativeRemoteError::Invalid(
                    "file fetch requires one selected manifest",
                ));
            }
            let selection = &plan.file_manifest_selections[0];
            if &selection.file_id != file_id
                || selection.manifest_descriptor.kind != ObjectKind::FileManifest
                || selection.manifest_descriptor.object_id.as_str()
                    != selection.file_manifest_id.as_str()
                || selection
                    .chunk_descriptors
                    .iter()
                    .any(|d| d.kind != ObjectKind::Chunk)
            {
                return Err(NativeRemoteError::Invalid("file fetch selection mismatch"));
            }
            let descriptors: BTreeMap<_, _> = std::iter::once(&selection.manifest_descriptor)
                .chain(&selection.chunk_descriptors)
                .map(|d| (&d.object_id, d))
                .collect();
            let actual: BTreeMap<_, _> = plan
                .download_capabilities
                .iter()
                .map(|c| (&c.descriptor.object_id, &c.descriptor))
                .collect();
            if descriptors != actual || actual.len() != plan.download_capabilities.len() {
                return Err(NativeRemoteError::Invalid(
                    "file fetch capabilities do not match manifest selection",
                ));
            }
        }
        _ => return Err(NativeRemoteError::Invalid("unsupported fetch claim")),
    }
    Ok(())
}
fn json<T: DeserializeOwned>(response: Response) -> RemoteResult<T> {
    let mut bytes = Vec::new();
    checked(response)?
        .take(MAX_OBJECT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_OBJECT {
        return Err(NativeRemoteError::Invalid("remote JSON exceeds limit"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}
fn new_request_id() -> RequestId {
    RequestId::parse(format!("request:{}", uuid::Uuid::new_v4())).expect("UUID")
}
fn graph_error(_: layrs_graph::scm::ScmGraphError) -> NativeRemoteError {
    NativeRemoteError::Invalid("invalid or incomplete Step DAG")
}
fn bounded_file(path: &Path, max: u64) -> RemoteResult<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(NativeRemoteError::Invalid("journal exceeds limit"));
    }
    Ok(bytes)
}
fn remote_lock(workspace: &NativeWorkspace) -> RemoteResult<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(workspace.store_root().join("native-remote.lock"))?;
    fs2::FileExt::try_lock_exclusive(&file)?;
    Ok(file)
}
fn save_binding(workspace: &NativeWorkspace, binding: &Binding) -> RemoteResult<()> {
    let path = workspace.store_root().join("native-remote.json");
    let temp = workspace
        .store_root()
        .join(format!("native-remote-{}.tmp", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)?;
    file.write_all(&serde_json::to_vec(binding)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temp, path)?;
    #[cfg(unix)]
    File::open(workspace.store_root())?.sync_all()?;
    Ok(())
}
fn encode(object: &CanonicalObject) -> RemoteResult<(ObjectKind, Vec<u8>)> {
    Ok(match object {
        CanonicalObject::ChunkRaw(o) => (ObjectKind::Chunk, o.bytes().to_vec()),
        CanonicalObject::FileManifest(o) => (ObjectKind::FileManifest, serde_jcs::to_vec(o)?),
        CanonicalObject::Tree(o) => (ObjectKind::Tree, serde_jcs::to_vec(o)?),
        CanonicalObject::Symlink(o) => (ObjectKind::Symlink, serde_jcs::to_vec(o)?),
        CanonicalObject::Tombstone(o) => (ObjectKind::Tombstone, serde_jcs::to_vec(o)?),
        CanonicalObject::Step(o) => (ObjectKind::Step, serde_jcs::to_vec(o)?),
        CanonicalObject::Product(o) => (ObjectKind::Product, o.canonical_json().to_vec()),
        CanonicalObject::Conflict(o) => (ObjectKind::Conflict, o.canonical_json().to_vec()),
        _ => {
            return Err(NativeRemoteError::Invalid(
                "unsupported object in native transfer",
            ));
        }
    })
}
fn decode(kind: ObjectKind, bytes: &[u8]) -> RemoteResult<CanonicalObject> {
    let object = match kind {
        ObjectKind::Chunk => CanonicalObject::ChunkRaw(Chunk::new(bytes.to_vec())?),
        ObjectKind::FileManifest => CanonicalObject::FileManifest(serde_json::from_slice(bytes)?),
        ObjectKind::Tree => CanonicalObject::Tree(serde_json::from_slice(bytes)?),
        ObjectKind::Symlink => CanonicalObject::Symlink(serde_json::from_slice(bytes)?),
        ObjectKind::Tombstone => CanonicalObject::Tombstone(serde_json::from_slice(bytes)?),
        ObjectKind::Step => CanonicalObject::Step(serde_json::from_slice(bytes)?),
        ObjectKind::Conflict => {
            let parsed = layrs_weave::graph_merge::PlannedConflict::from_canonical_bytes(bytes)
                .map_err(|_| NativeRemoteError::Invalid("invalid structural Conflict"))?;
            CanonicalObject::Conflict(CanonicalConflict::new(parsed.id(), bytes)?)
        }
        ObjectKind::Product => {
            let envelope: ProductEnvelope = serde_json::from_slice(bytes)?;
            CanonicalObject::Product(CanonicalProduct::new(envelope.object_id().clone(), bytes)?)
        }
        _ => {
            return Err(NativeRemoteError::Invalid(
                "unsupported downloaded object kind",
            ));
        }
    };
    if encode(&object)?.1 != bytes {
        return Err(NativeRemoteError::Invalid("download is not canonical"));
    }
    Ok(object)
}

mod chunk_spool;
mod file;
mod graph;
mod observations;
use graph::GraphObjects;

fn conflict_references(
    conflict: &CanonicalConflict,
) -> RemoteResult<(Vec<StepId>, Vec<TreeEntryTarget>)> {
    let parsed =
        layrs_weave::graph_merge::PlannedConflict::from_canonical_bytes(conflict.canonical_json())
            .map_err(|_| NativeRemoteError::Invalid("invalid structural Conflict"))?;
    let document = parsed.document();
    if !document["lensPin"].is_null() {
        use layrs_weave::{reconciliation::NativeLens, wasm_host};
        let supported = [NativeLens::Text, NativeLens::Code]
            .into_iter()
            .any(|lens| {
                serde_json::to_value(wasm_host::manifest(lens).pin().expect("built-in pin"))
                    .ok()
                    .as_ref()
                    == Some(&document["lensPin"])
            });
        if !supported || document["lensImplementationDigest"] != wasm_host::module_digest() {
            return Err(NativeRemoteError::Invalid("unsupported Conflict Lens pin"));
        }
    }
    let mut origins = BTreeSet::new();
    let mut targets = Vec::new();
    for role in ["bases", "sides"] {
        for term in document[role].as_array().expect("validated terms") {
            origins.insert(StepId::parse(
                term["provenance"]["stepId"]
                    .as_str()
                    .expect("validated provenance"),
            )?);
            for placed in term["values"].as_array().expect("validated placements") {
                let value = &placed["value"];
                let kind = value["kind"].as_str().expect("validated kind");
                if kind == "absent" {
                    continue;
                }
                let id = value["objectId"].as_str().expect("validated object");
                targets.push(match kind {
                    "file" => TreeEntryTarget::file(FileId::parse(id)?),
                    "tree" => TreeEntryTarget::tree(TreeId::parse(id)?),
                    "symlink" => TreeEntryTarget::symlink(SymlinkId::parse(id)?),
                    _ => return Err(NativeRemoteError::Invalid("unsupported Conflict term")),
                });
            }
        }
    }
    for file in parsed
        .payload_file_ids()
        .map_err(|_| NativeRemoteError::Invalid("invalid Conflict payload"))?
    {
        let target = TreeEntryTarget::file(file);
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    Ok((origins.into_iter().collect(), targets))
}

#[cfg(test)]
#[path = "remote/tests.rs"]
mod tests;

mod control;
#[path = "remote/download.rs"]
mod download;
mod download_refresh;
mod download_resume;
#[path = "remote/publish.rs"]
mod publish;
#[path = "remote/receive.rs"]
mod receive;
#[path = "remote/rename_validation.rs"]
mod rename_validation;
mod unchanged;

mod policy;
mod policy_journal;
#[path = "remote/publication.rs"]
mod publication;
pub use policy::{NativeInlinePolicyRules, NativePolicyPreview};
