//! Built-in WebAssembly Lens execution: zero imports, bounded fuel/linear memory,
//! with a separately pinned worker process enforcing global memory and deadline.
use crate::{
    reconciliation::*,
    worker::{self, WorkerError, WorkerLimits},
};
use layrs_lens_sdk::{
    LensManifest, LensPin, LensReconcileRequest, LensResourceQuotas, sha256_digest,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use wasmi::{Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

pub const MODULE: &[u8] = include_bytes!("../wasm/native-lenses-v1.wasm");
pub const HOST_ID: &str = "layrs.wasmi-no-imports-process";
pub const HOST_VERSION: &str = "1";
const WIRE: &str = "layrs.wasm-lens-worker.v1";

pub fn module_digest() -> String {
    sha256_digest(MODULE)
}
pub fn manifest(lens: NativeLens) -> LensManifest {
    let mut manifest = native_manifest(lens);
    manifest.digest = Some(module_digest());
    manifest
}

/// Validate the supported receipt contract, not the authenticity of a remote
/// execution. Transport admission still needs its normal authenticated author.
pub fn validate_receipt_host(
    record: &layrs_sync::v3::WeaveReceiptRecord,
) -> Result<(), WorkerError> {
    let profile = &record.determinism_profile;
    if profile.host_policy.id != HOST_ID
        || profile.host_policy.version != HOST_VERSION
        || !(4 * 1024 * 1024..=512 * 1024 * 1024).contains(&profile.limits.max_memory_bytes)
        || !(1..=30_000).contains(&profile.limits.max_execution_ms)
        || !profile
            .limits
            .max_fuel
            .as_ref()
            .is_some_and(|fuel| (1..=1_000_000_000).contains(fuel))
    {
        return Err(protocol("unsupported WASM receipt host or budgets"));
    }
    let pins = [
        manifest(NativeLens::Text).pin().unwrap(),
        manifest(NativeLens::Code).pin().unwrap(),
    ];
    for execution in &record.lens_executions {
        let built_in = pins.iter().any(|pin| {
            pin.lens_id == execution.pin.lens_id
                && pin.version == execution.pin.version
                && pin.digest == execution.pin.digest.as_str()
        }) && execution.implementation_digest.as_str() == module_digest();
        // Package membership and exact Space selection are checked against the
        // server catalogue at admission; this pure contract check cannot query it.
        let package = execution
            .implementation_digest
            .as_str()
            .starts_with("sha256:");
        if (!built_in && !package) || execution.determinism_profile_id != profile.profile_id {
            return Err(protocol(
                "receipt execution does not pin a supported WASM module",
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SandboxLimits {
    pub process: WorkerLimits,
    pub linear_memory_bytes: usize,
    pub fuel: u64,
}
impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            process: WorkerLimits {
                memory_bytes: 256 * 1024 * 1024,
                deadline_ms: 10000,
                ..WorkerLimits::default()
            },
            linear_memory_bytes: 32 * 1024 * 1024,
            fuel: 100_000_000,
        }
    }
}
impl SandboxLimits {
    fn validate(self) -> Result<(), WorkerError> {
        self.process.validate()?;
        if !(2 * 1024 * 1024..=128 * 1024 * 1024).contains(&self.linear_memory_bytes)
            || self.linear_memory_bytes >= self.process.memory_bytes
            || self.fuel == 0
            || self.fuel > 1_000_000_000
        {
            return Err(WorkerError::InvalidLimits);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct EnforcedProfile {
    pub(crate) module_digest: String,
    pub(crate) worker_digest: String,
    pub(crate) limits: SandboxLimits,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) packages: Vec<EnforcedPackage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) catalogue_pins: Vec<LensPin>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) associations: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) project_selection: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnforcedPackage {
    pub(crate) pin: LensPin,
    pub(crate) module_digest: String,
    pub(crate) resources: LensResourceQuotas,
}

struct PackageModule {
    manifest: LensManifest,
    module: Vec<u8>,
    module_digest: String,
    limits: SandboxLimits,
    guest_input_bytes: usize,
    guest_output_bytes: usize,
}

fn verified_manifest_pin(manifest: &LensManifest) -> Result<LensPin, WorkerError> {
    let pin = manifest
        .try_pin()
        .map_err(protocol)?
        .ok_or_else(|| protocol("package has no pin"))?;
    let mut identity = manifest.clone();
    identity.digest = None;
    identity
        .bundle
        .as_mut()
        .ok_or_else(|| protocol("package has no bundle"))?
        .signature = None;
    if sha256_digest(&worker::canonical(&identity)?) != pin.digest {
        return Err(protocol("package manifest identity digest mismatch"));
    }
    Ok(pin)
}

fn project_lens_id<'a>(
    path: &str,
    catalogue: &'a BTreeMap<String, LensManifest>,
    associations: &'a BTreeMap<String, String>,
) -> Option<&'a str> {
    let extension = path
        .rsplit('/')
        .next()?
        .rsplit_once('.')?
        .1
        .to_ascii_lowercase();
    if let Some(lens_id) = associations.get(&format!(".{extension}")) {
        return Some(lens_id);
    }
    let mut matches = catalogue.iter().filter(|(_, manifest)| {
        manifest
            .analyzer
            .file_extensions
            .iter()
            .any(|item| item == &extension)
    });
    let (lens_id, _) = matches.next()?;
    matches.next().is_none().then_some(lens_id.as_str())
}

pub struct WasmHost {
    executable: PathBuf,
    // Windows: ancestors/file held against rename/write. Linux: immutable sealed
    // executable snapshot held until this host capability is dropped.
    _pinned: Vec<File>,
    profile: EnforcedProfile,
    packages: BTreeMap<String, PackageModule>,
    catalogue: BTreeMap<String, LensManifest>,
    associations: BTreeMap<String, String>,
    project_selection: bool,
}
impl WasmHost {
    /// Expected digest is a trusted deployment input. Do not accept executable
    /// paths or expected hashes from a remote repository's content or metadata.
    pub fn open(
        executable: &Path,
        expected_worker_digest: &str,
        limits: SandboxLimits,
    ) -> Result<Self, WorkerError> {
        limits.validate()?;
        #[cfg(not(any(windows, all(target_os = "linux", target_arch = "x86_64"))))]
        {
            let _ = (executable, expected_worker_digest);
            Err(WorkerError::UnsupportedPlatform)
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            let (executable, image) = worker::linux::pin_image(executable, expected_worker_digest)?;
            Ok(Self {
                executable,
                _pinned: vec![image],
                profile: EnforcedProfile {
                    module_digest: module_digest(),
                    worker_digest: expected_worker_digest.into(),
                    limits,
                    packages: vec![],
                    catalogue_pins: vec![],
                    associations: BTreeMap::new(),
                    project_selection: false,
                },
                packages: BTreeMap::new(),
                catalogue: BTreeMap::new(),
                associations: BTreeMap::new(),
                project_selection: false,
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
            let executable = std::path::absolute(executable)?;
            let mut pinned = Vec::new();
            let mut path = PathBuf::new();
            for component in executable.components() {
                if matches!(component, std::path::Component::ParentDir) {
                    return Err(protocol("worker path contains parent traversal"));
                }
                path.push(component.as_os_str());
                if !matches!(component, std::path::Component::Normal(_)) {
                    continue;
                }
                let metadata = std::fs::symlink_metadata(&path)?;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(protocol("worker path contains reparse point"));
                }
                let mut options = std::fs::OpenOptions::new();
                if metadata.is_dir() {
                    options
                        .access_mode(0x80)
                        .share_mode(3)
                        .custom_flags(0x0200_0000 | 0x0020_0000);
                } else {
                    options.read(true).share_mode(1).custom_flags(0x0020_0000);
                }
                let file = options.open(&path)?;
                if file.metadata()?.file_attributes() & 0x400 != 0 {
                    return Err(protocol("worker changed to reparse point"));
                }
                pinned.push(file);
            }
            let file = pinned
                .last()
                .ok_or_else(|| protocol("worker file missing"))?;
            if !file.metadata()?.is_file() || file.metadata()?.len() > 256 * 1024 * 1024 {
                return Err(protocol("worker image must be a bounded regular file"));
            }
            let mut bytes = Vec::new();
            file.take(256 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
            let digest = sha256_digest(&bytes);
            if digest != expected_worker_digest {
                return Err(protocol("worker image digest mismatch"));
            }
            Ok(Self {
                executable,
                _pinned: pinned,
                profile: EnforcedProfile {
                    module_digest: module_digest(),
                    worker_digest: digest,
                    limits,
                    packages: vec![],
                    catalogue_pins: vec![],
                    associations: BTreeMap::new(),
                    project_selection: false,
                },
                packages: BTreeMap::new(),
                catalogue: BTreeMap::new(),
                associations: BTreeMap::new(),
                project_selection: false,
            })
        }
    }
    pub(crate) fn profile(&self) -> EnforcedProfile {
        self.profile.clone()
    }
    /// Registers an exact project pin from the authenticated server catalogue.
    /// The caller supplies bytes obtained for that pin, never a repository path.
    pub fn with_project_manifest(mut self, manifest: LensManifest) -> Result<Self, WorkerError> {
        let pin = verified_manifest_pin(&manifest)?;
        if self.catalogue.insert(pin.lens_id, manifest).is_some() {
            return Err(protocol("duplicate project Lens manifest"));
        }
        self.refresh_catalogue_profile();
        Ok(self)
    }

    fn refresh_catalogue_profile(&mut self) {
        self.profile.catalogue_pins = self
            .catalogue
            .values()
            .filter_map(LensManifest::pin)
            .collect();
    }

    pub fn with_package(
        mut self,
        manifest: LensManifest,
        module: Vec<u8>,
    ) -> Result<Self, WorkerError> {
        let pin = verified_manifest_pin(&manifest)?;
        let entrypoint = manifest
            .bundle
            .as_ref()
            .and_then(|bundle| bundle.entrypoints.reconcile.as_ref())
            .ok_or_else(|| protocol("package has no reconcile entrypoint"))?;
        if entrypoint.export != "layrs_reconcile" {
            return Err(protocol("unsupported reconcile export"));
        }
        let module_digest = entrypoint.digest.clone();
        let resources = entrypoint.resources.clone();
        let process_memory = self
            .profile
            .limits
            .process
            .memory_bytes
            .min(resources.max_memory_bytes as usize);
        let limits = SandboxLimits {
            process: WorkerLimits {
                memory_bytes: process_memory,
                deadline_ms: self
                    .profile
                    .limits
                    .process
                    .deadline_ms
                    .min(resources.max_execution_ms.min(u32::MAX as u64) as u32),
                ..self.profile.limits.process
            },
            linear_memory_bytes: self
                .profile
                .limits
                .linear_memory_bytes
                .min(process_memory / 2),
            fuel: self
                .profile
                .limits
                .fuel
                .min(resources.max_fuel.unwrap_or(self.profile.limits.fuel)),
        };
        limits.validate()?;
        let guest_input_bytes = self
            .profile
            .limits
            .process
            .input_bytes
            .min(resources.max_input_bytes as usize);
        let guest_output_bytes = self
            .profile
            .limits
            .process
            .output_bytes
            .min(resources.max_output_bytes as usize);
        validate_guest_module(&module, &module_digest)?;
        if self.packages.contains_key(&pin.lens_id) {
            return Err(protocol("duplicate package lens ID"));
        }
        match self.catalogue.get(&pin.lens_id) {
            Some(known) if known != &manifest => {
                return Err(protocol("project Lens manifest disagrees with module"));
            }
            None => {
                self.catalogue.insert(pin.lens_id.clone(), manifest.clone());
                self.refresh_catalogue_profile();
            }
            _ => {}
        }
        if self
            .packages
            .insert(
                pin.lens_id.clone(),
                PackageModule {
                    manifest,
                    module,
                    module_digest: module_digest.clone(),
                    limits,
                    guest_input_bytes,
                    guest_output_bytes,
                },
            )
            .is_some()
        {
            unreachable!("checked duplicate package")
        }
        self.profile.packages.push(EnforcedPackage {
            pin,
            module_digest,
            resources,
        });
        self.profile
            .packages
            .sort_by(|a, b| a.pin.lens_id.cmp(&b.pin.lens_id));
        Ok(self)
    }

    pub fn assign_extension(mut self, extension: &str, lens_id: &str) -> Result<Self, WorkerError> {
        if extension.len() < 2
            || !extension.starts_with('.')
            || !extension[1..]
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(protocol("invalid package extension"));
        }
        let package = self
            .catalogue
            .get(lens_id)
            .ok_or_else(|| protocol("assigned package missing"))?;
        if !package
            .analyzer
            .file_extensions
            .iter()
            .any(|item| item == &extension[1..])
        {
            return Err(protocol("package does not support assigned extension"));
        }
        if self
            .associations
            .insert(extension.into(), lens_id.into())
            .is_some()
        {
            return Err(protocol("duplicate package extension"));
        }
        self.profile.associations = self.associations.clone();
        Ok(self)
    }

    pub fn restrict_to_project(mut self) -> Self {
        self.project_selection = true;
        self.profile.project_selection = true;
        self
    }

    pub(crate) fn allows_native(&self) -> bool {
        !self.project_selection
    }

    pub(crate) fn selected_package(&self, path: &str) -> Option<&LensManifest> {
        let lens_id = project_lens_id(path, &self.catalogue, &self.associations)?;
        self.packages.get(lens_id).map(|package| &package.manifest)
    }

    pub(crate) fn implementation_digest(&self, pin: &LensPin) -> Option<&str> {
        self.packages
            .get(&pin.lens_id)
            .filter(|p| p.manifest.pin().as_ref() == Some(pin))
            .map(|p| p.module_digest.as_str())
            .or_else(|| {
                [NativeLens::Text, NativeLens::Code]
                    .into_iter()
                    .any(|lens| manifest(lens).pin().as_ref() == Some(pin))
                    .then_some(self.profile.module_digest.as_str())
            })
    }
    pub fn reconcile(
        &self,
        request: &LensReconcileRequest,
        policy: FileMergePolicy,
    ) -> Result<FileMergePlan, WorkerError> {
        request.validate().map_err(protocol)?;
        layrs_core::scm::RepoPath::parse(&request.path).map_err(protocol)?;
        let native_pin = [NativeLens::Code, NativeLens::Text]
            .into_iter()
            .find(|lens| manifest(*lens).pin().as_ref() == Some(&request.lens));
        let package = self
            .packages
            .get(&request.lens.lens_id)
            .filter(|package| package.manifest.pin().as_ref() == Some(&request.lens));
        if native_pin.is_none() && package.is_none() {
            return Err(WorkerError::Reconciliation(FileMergeError::PinMismatch));
        }
        let mut native_request = request.clone();
        let bundled_native = package
            .filter(|package| package.module_digest == module_digest())
            .and_then(|_| {
                [NativeLens::Text, NativeLens::Code]
                    .into_iter()
                    .find(|lens| native_manifest(*lens).id == request.lens.lens_id)
            });
        if let Some(lens) = native_pin.or(bundled_native) {
            native_request.lens = native_manifest(lens).pin().unwrap();
        }
        let implementation_digest = package.map_or_else(module_digest, |p| p.module_digest.clone());
        let limits = package.map_or(self.profile.limits, |p| p.limits);
        let envelope = SandboxRequest {
            protocol: WIRE.into(),
            module_digest: implementation_digest.clone(),
            module: package.map(|p| p.module.clone()),
            guest_input_bytes: package.map(|p| p.guest_input_bytes),
            guest_output_bytes: package.map(|p| p.guest_output_bytes),
            limits,
            request: worker::Request {
                protocol: worker::PROTOCOL.into(),
                request: native_request.clone(),
                policy,
            },
        };
        if package.is_some_and(|p| {
            worker::canonical(&envelope.request)
                .map_or(true, |bytes| bytes.len() > p.guest_input_bytes)
        }) {
            return Err(WorkerError::InputLimit);
        }
        let bytes = worker::canonical(&envelope)?;
        if bytes.len() > limits.process.input_bytes {
            return Err(WorkerError::InputLimit);
        }
        let response =
            worker::run_process(&self.executable, &["--wasm-v1"], &bytes, limits.process)?;
        let output = decode_reply(&response, &implementation_digest, limits)?;
        let inner: worker::Reply = serde_json::from_slice(&output).map_err(protocol)?;
        if worker::canonical(&inner)? != output || inner.protocol != worker::PROTOCOL {
            return Err(protocol("guest response is not canonical"));
        }
        let mut plan = inner.result.map_err(WorkerError::Reconciliation)?;
        if plan.request != native_request || plan.policy != policy {
            return Err(protocol("guest changed exact input or policy"));
        }
        if let FileMergeOutcome::Semantic(result) = &plan.outcome {
            result.validate().map_err(protocol)?;
        }
        plan.request = request.clone();
        Ok(plan)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SandboxRequest {
    protocol: String,
    module_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    module: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guest_input_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guest_output_bytes: Option<usize>,
    limits: SandboxLimits,
    request: worker::Request,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SandboxReply {
    protocol: String,
    module_digest: String,
    limits: SandboxLimits,
    output: Vec<u8>,
    // Additive v1 extension: successful replies retain their exact wire shape.
    // No unbounded diagnostic string crosses this error channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure: Option<SandboxFailure>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SandboxFailure {
    Fuel,
    Growth,
    Input,
    Output,
}
impl SandboxFailure {
    fn from_error(error: WorkerError) -> Result<Self, WorkerError> {
        match error {
            WorkerError::FuelLimit => Ok(Self::Fuel),
            WorkerError::WasmGrowthLimit => Ok(Self::Growth),
            WorkerError::InputLimit => Ok(Self::Input),
            WorkerError::OutputLimit => Ok(Self::Output),
            other => Err(other),
        }
    }
    fn into_error(self) -> WorkerError {
        match self {
            Self::Fuel => WorkerError::FuelLimit,
            Self::Growth => WorkerError::WasmGrowthLimit,
            Self::Input => WorkerError::InputLimit,
            Self::Output => WorkerError::OutputLimit,
        }
    }
}

fn decode_reply(bytes: &[u8], digest: &str, limits: SandboxLimits) -> Result<Vec<u8>, WorkerError> {
    let reply: SandboxReply = serde_json::from_slice(bytes).map_err(protocol)?;
    if bytes.len() > limits.process.output_bytes
        || worker::canonical(&reply)? != bytes
        || reply.protocol != WIRE
        || reply.module_digest != digest
        || reply.limits != limits
    {
        return Err(protocol("sandbox response identity mismatch"));
    }
    if let Some(failure) = reply.failure {
        if !reply.output.is_empty() {
            return Err(protocol("sandbox failure cannot carry a successful output"));
        }
        return Err(failure.into_error());
    }
    Ok(reply.output)
}

pub fn serve(input: impl Read, mut output: impl Write) -> Result<(), WorkerError> {
    let mut bytes = Vec::new();
    input.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(WorkerError::InputLimit);
    }
    let request: SandboxRequest = serde_json::from_slice(&bytes).map_err(protocol)?;
    request.limits.validate()?;
    if request.protocol != WIRE || worker::canonical(&request)? != bytes {
        return Err(protocol("sandbox request identity mismatch"));
    }
    let module = request.module.as_deref().unwrap_or(MODULE);
    validate_guest_module(module, &request.module_digest)?;
    if request.module.is_none() && request.module_digest != module_digest() {
        return Err(protocol("built-in module identity mismatch"));
    }
    let input = worker::canonical(&request.request)?;
    if input.len()
        > request
            .guest_input_bytes
            .unwrap_or(request.limits.process.input_bytes)
    {
        return Err(WorkerError::InputLimit);
    }
    let (result, failure) = match execute_module(module, &input, request.limits) {
        Ok(result)
            if result.len()
                <= request
                    .guest_output_bytes
                    .unwrap_or(request.limits.process.output_bytes) =>
        {
            (result, None)
        }
        Ok(_) => (vec![], Some(SandboxFailure::Output)),
        Err(error) => (vec![], Some(SandboxFailure::from_error(error)?)),
    };
    let mut reply = SandboxReply {
        protocol: WIRE.into(),
        module_digest: request.module_digest,
        limits: request.limits,
        output: result,
        failure,
    };
    let mut bytes = worker::canonical(&reply)?;
    if bytes.len() > request.limits.process.output_bytes && reply.failure.is_none() {
        reply.output.clear();
        reply.failure = Some(SandboxFailure::Output);
        bytes = worker::canonical(&reply)?;
    }
    if bytes.len() > request.limits.process.output_bytes {
        return Err(WorkerError::OutputLimit);
    }
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}

fn protocol(error: impl std::fmt::Display) -> WorkerError {
    WorkerError::Protocol(error.to_string())
}

fn execution_error(error: wasmi::Error) -> WorkerError {
    match error.as_trap_code() {
        Some(wasmi::core::TrapCode::OutOfFuel) => WorkerError::FuelLimit,
        Some(wasmi::core::TrapCode::GrowthOperationLimited) => WorkerError::WasmGrowthLimit,
        _ => protocol(error),
    }
}

/// Check the only executable ABI this host can admit. The SDK's `wasi`
/// metadata is historical; these modules receive no WASI or other imports.
pub fn validate_guest_module(module: &[u8], expected_digest: &str) -> Result<(), WorkerError> {
    if module.len() > 2 * 1024 * 1024 || !module.starts_with(b"\0asm\x01\0\0\0") {
        return Err(protocol("invalid or oversized WASM Lens module"));
    }
    if sha256_digest(module) != expected_digest {
        return Err(protocol("WASM Lens module digest mismatch"));
    }
    let engine = Engine::default();
    let module = Module::new(&engine, module).map_err(protocol)?;
    if module.imports().next().is_some() {
        return Err(protocol("WASM Lens imports are forbidden"));
    }
    let bounds = StoreLimitsBuilder::new()
        .memory_size(32 * 1024 * 1024)
        .memories(1)
        .tables(1)
        .table_elements(10000)
        .instances(1)
        .trap_on_grow_failure(true)
        .build();
    let mut store = Store::new(&engine, bounds);
    store.limiter(|state: &mut StoreLimits| state);
    let linker = Linker::<StoreLimits>::new(&engine);
    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(protocol)?
        .ensure_no_start(&mut store)
        .map_err(protocol)?;
    instance
        .get_memory(&store, "memory")
        .ok_or_else(|| protocol("guest memory missing"))?;
    instance
        .get_typed_func::<u32, u32>(&store, "layrs_allocate")
        .map_err(protocol)?;
    instance
        .get_typed_func::<(u32, u32), u64>(&store, "layrs_reconcile")
        .map_err(protocol)?;
    Ok(())
}

/// No imports of any kind are linked. Module validation, instantiation and
/// execution all happen inside the already resource-bound worker process.
fn execute_module(
    module: &[u8],
    input: &[u8],
    limits: SandboxLimits,
) -> Result<Vec<u8>, WorkerError> {
    limits.validate()?;
    if module.len() > 2 * 1024 * 1024 || input.len() > limits.process.input_bytes {
        return Err(WorkerError::InputLimit);
    }
    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    let module = Module::new(&engine, module).map_err(protocol)?;
    if module.imports().next().is_some() {
        return Err(protocol("WASM Lens imports are forbidden"));
    }
    let bounds = StoreLimitsBuilder::new()
        .memory_size(limits.linear_memory_bytes)
        .memories(1)
        .tables(1)
        .table_elements(10000)
        .instances(1)
        .trap_on_grow_failure(true)
        .build();
    let mut store = Store::new(&engine, bounds);
    store.limiter(|state: &mut StoreLimits| state);
    store.set_fuel(limits.fuel).map_err(protocol)?;
    let linker = Linker::<StoreLimits>::new(&engine);
    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(protocol)?
        .ensure_no_start(&mut store)
        .map_err(protocol)?;
    let memory = instance
        .get_memory(&store, "memory")
        .ok_or_else(|| protocol("guest memory missing"))?;
    let allocate = instance
        .get_typed_func::<u32, u32>(&store, "layrs_allocate")
        .map_err(protocol)?;
    let reconcile = instance
        .get_typed_func::<(u32, u32), u64>(&store, "layrs_reconcile")
        .map_err(protocol)?;
    let pointer = allocate
        .call(&mut store, input.len() as u32)
        .map_err(execution_error)?;
    if pointer == 0 {
        return Err(protocol("guest allocation refused"));
    }
    memory
        .write(&mut store, pointer as usize, input)
        .map_err(protocol)?;
    let packed = reconcile
        .call(&mut store, (pointer, input.len() as u32))
        .map_err(execution_error)?;
    let (pointer, length) = ((packed >> 32) as usize, (packed as u32) as usize);
    if pointer == 0 || length == 0 {
        return Err(protocol("guest rejected request"));
    }
    if length > limits.process.output_bytes {
        return Err(WorkerError::OutputLimit);
    }
    let mut output = vec![0; length];
    memory
        .read(&store, pointer, &mut output)
        .map_err(protocol)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_lens_sdk::{LensReconcileLabelsContract, LensReconcileSideContract as Side};
    #[test]
    fn project_selector_honors_unique_ambiguous_and_explicit_viewer_only_lenses() {
        let packets: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../../deploy/lenses/builtin-packets.json"))
                .unwrap();
        let text: LensManifest = serde_json::from_value(
            packets
                .iter()
                .find(|packet| packet["manifest"]["id"] == "layrs.text")
                .unwrap()["manifest"]
                .clone(),
        )
        .unwrap();
        let markdown_packet: serde_json::Value = serde_json::from_str(include_str!(
            "../../../deploy/lenses/example-markdown.layrslens.json"
        ))
        .unwrap();
        let markdown: LensManifest =
            serde_json::from_value(markdown_packet["manifest"].clone()).unwrap();
        assert!(
            markdown
                .bundle
                .as_ref()
                .unwrap()
                .entrypoints
                .reconcile
                .is_none()
        );
        let mut catalogue = BTreeMap::new();
        catalogue.insert(text.id.clone(), text);
        let text_pin = catalogue["layrs.text"].pin().unwrap();
        let mut associations = BTreeMap::new();
        assert_eq!(
            project_lens_id("docs/readme.txt", &catalogue, &associations),
            Some("layrs.text")
        );
        assert_eq!(
            project_lens_id("docs/readme.md", &catalogue, &associations),
            Some("layrs.text")
        );
        catalogue.insert(markdown.id.clone(), markdown);
        let markdown_pin = catalogue["example.markdown"].pin().unwrap();
        let profile = EnforcedProfile {
            module_digest: module_digest(),
            worker_digest: "sha256:trusted-worker".into(),
            limits: SandboxLimits::default(),
            packages: vec![],
            catalogue_pins: vec![text_pin.clone()],
            associations: BTreeMap::new(),
            project_selection: true,
        };
        let with_viewer = EnforcedProfile {
            catalogue_pins: vec![markdown_pin, text_pin],
            ..profile.clone()
        };
        assert_ne!(
            sha256_digest(&worker::canonical(&profile).unwrap()),
            sha256_digest(&worker::canonical(&with_viewer).unwrap())
        );
        assert_eq!(
            project_lens_id("docs/readme.md", &catalogue, &associations),
            None
        );
        associations.insert(".md".into(), "example.markdown".into());
        assert_eq!(
            project_lens_id("docs/readme.md", &catalogue, &associations),
            Some("example.markdown")
        );
        associations.insert(".md".into(), "layrs.text".into());
        assert_eq!(
            project_lens_id("docs/readme.md", &catalogue, &associations),
            Some("layrs.text")
        );
    }
    fn input() -> Vec<u8> {
        worker::canonical(&worker::Request {
            protocol: worker::PROTOCOL.into(),
            request: LensReconcileRequest::new(
                native_manifest(NativeLens::Text).pin().unwrap(),
                "a.txt",
                None,
                Side::present(b"a\nb\nc\n", None),
                Side::present(b"A\nb\nc\n", None),
                Side::present(b"a\nb\nC\n", None),
                LensReconcileLabelsContract {
                    existing: "a".into(),
                    incoming: "b".into(),
                },
            ),
            policy: FileMergePolicy {
                unsupported: FailureAction::StructuralFallback,
                quota: FailureAction::Reject,
                limits: FileMergeLimits::default(),
            },
        })
        .unwrap()
    }
    #[test]
    fn real_compiled_guest_has_no_imports_and_executes_deterministically() {
        let module = Module::new(&Engine::default(), MODULE).unwrap();
        assert_eq!(module.imports().count(), 0);
        assert_eq!(
            module_digest(),
            format!(
                "sha256:{}",
                include_str!("../wasm/native-lenses-v1.sha256").trim()
            )
        );
        let input = input();
        let first = execute_module(MODULE, &input, SandboxLimits::default()).unwrap();
        assert_eq!(
            first,
            execute_module(MODULE, &input, SandboxLimits::default()).unwrap()
        );
        let reply: worker::Reply = serde_json::from_slice(&first).unwrap();
        let FileMergeOutcome::Semantic(result) = reply.result.unwrap().outcome else {
            panic!("semantic result expected")
        };
        assert_eq!(result.resolved.unwrap().bytes, b"A\nb\nC\n");
    }
    #[test]
    fn guest_import_fuel_memory_and_output_violations_reject() {
        let imports = b"\0asm\x01\0\0\0\x01\x04\x01\x60\0\0\x02\x0d\x01\x03env\x05clock\0\0";
        let error = execute_module(imports, b"{}", SandboxLimits::default()).unwrap_err();
        assert!(
            error.to_string().contains("imports are forbidden"),
            "{error}"
        );
        let error = execute_module(
            MODULE,
            &input(),
            SandboxLimits {
                fuel: 1,
                ..SandboxLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(error, WorkerError::FuelLimit), "{error}");
        let error = execute_module(
            MODULE,
            &vec![b' '; 2 * 1024 * 1024],
            SandboxLimits {
                linear_memory_bytes: 2 * 1024 * 1024,
                ..SandboxLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(error, WorkerError::WasmGrowthLimit), "{error}");
        assert!(matches!(
            execute_module(
                MODULE,
                &input(),
                SandboxLimits {
                    process: WorkerLimits {
                        output_bytes: 16,
                        ..WorkerLimits::default()
                    },
                    ..SandboxLimits::default()
                }
            ),
            Err(WorkerError::OutputLimit)
        ));
    }

    #[test]
    fn canonical_failure_reply_keeps_identity_and_never_becomes_fallback() {
        let limits = SandboxLimits {
            fuel: 1,
            ..SandboxLimits::default()
        };
        let request = SandboxRequest {
            protocol: WIRE.into(),
            module_digest: module_digest(),
            module: None,
            guest_input_bytes: None,
            guest_output_bytes: None,
            limits,
            request: serde_json::from_slice(&input()).unwrap(),
        };
        let bytes = worker::canonical(&request).unwrap();
        let mut output = Vec::new();
        serve(bytes.as_slice(), &mut output).unwrap();
        assert!(output.len() < 1024);
        assert!(matches!(
            decode_reply(&output, &module_digest(), limits),
            Err(WorkerError::FuelLimit)
        ));
        assert!(matches!(
            decode_reply(&output, "sha256:wrong", limits),
            Err(WorkerError::Protocol(_))
        ));
        assert!(matches!(
            decode_reply(&output, &module_digest(), SandboxLimits::default()),
            Err(WorkerError::Protocol(_))
        ));
        let mut reply: SandboxReply = serde_json::from_slice(&output).unwrap();
        reply.output = b"untrusted result".to_vec();
        assert!(matches!(
            decode_reply(
                &worker::canonical(&reply).unwrap(),
                &module_digest(),
                limits
            ),
            Err(WorkerError::Protocol(_))
        ));
        let mut wrong = request;
        wrong.module_digest = "sha256:wrong".into();
        let mut rejected = Vec::new();
        assert!(matches!(
            serve(worker::canonical(&wrong).unwrap().as_slice(), &mut rejected),
            Err(WorkerError::Protocol(_))
        ));
        assert!(rejected.is_empty());
    }
}
