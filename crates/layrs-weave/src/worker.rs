//! Resource-limited native worker. A process Job is NOT an ambient-capability
//! sandbox: these results cannot create ReceiptHostAttestation or accepted Weaves.
use crate::reconciliation::*;
use layrs_lens_sdk::LensReconcileRequest;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    path::Path,
};

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub(crate) mod linux;
#[cfg(test)]
mod tests;
#[cfg(windows)]
mod windows;

pub const PROTOCOL: &str = "layrs.native-lens-worker.v1";
const MAX_WIRE: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerLimits {
    /// Windows committed memory or Linux RLIMIT_AS for the entire sole worker.
    /// This is not an RSS measurement or a bound on the parent application.
    pub memory_bytes: usize,
    pub deadline_ms: u32,
    pub input_bytes: usize,
    pub output_bytes: usize,
}
impl Default for WorkerLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 128 * 1024 * 1024,
            deadline_ms: 5000,
            input_bytes: MAX_WIRE,
            output_bytes: MAX_WIRE,
        }
    }
}
impl WorkerLimits {
    pub(crate) fn validate(self) -> Result<(), WorkerError> {
        if !(4 * 1024 * 1024..=512 * 1024 * 1024).contains(&self.memory_bytes)
            || !(1..=30000).contains(&self.deadline_ms)
            || self.input_bytes == 0
            || self.input_bytes > MAX_WIRE
            || self.output_bytes == 0
            || self.output_bytes > MAX_WIRE
        {
            return Err(WorkerError::InvalidLimits);
        }
        Ok(())
    }
}
#[derive(Debug)]
pub enum WorkerError {
    UnsupportedPlatform,
    InvalidLimits,
    InputLimit,
    OutputLimit,
    FuelLimit,
    WasmGrowthLimit,
    Deadline,
    ProcessExit(u32),
    Io(io::Error),
    Protocol(String),
    Reconciliation(FileMergeError),
}
impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FuelLimit => return f.write_str("Lens WASM fuel budget exhausted"),
            Self::WasmGrowthLimit => {
                return f.write_str("Lens WASM memory/table growth limit exceeded");
            }
            _ => {}
        }
        write!(f, "native Lens worker: {self:?}")
    }
}
impl std::error::Error for WorkerError {}
impl From<io::Error> for WorkerError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug)]
pub struct SupervisedResult {
    pub plan: FileMergePlan,
    pub enforced_limits: WorkerLimits,
}
impl SupervisedResult {
    /// Deliberately false until a separate ambient-capability sandbox exists.
    pub fn eligible_for_semantic_receipt(&self) -> bool {
        false
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub(crate) protocol: String,
    pub(crate) request: LensReconcileRequest,
    pub(crate) policy: FileMergePolicy,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reply {
    pub(crate) protocol: String,
    pub(crate) result: Result<FileMergePlan, FileMergeError>,
}

/// Execute only the explicitly supplied native worker binary. No PATH lookup,
/// inherited environment, shell, unbounded pipe or in-process fallback is used.
/// This is resource supervision, not authorization to execute external Lenses.
pub fn reconcile_in_worker(
    executable: &Path,
    request: &LensReconcileRequest,
    policy: FileMergePolicy,
    limits: WorkerLimits,
) -> Result<SupervisedResult, WorkerError> {
    limits.validate()?;
    let sizes = [
        request.contract_version.len(),
        request.path.len(),
        request.media_type.as_ref().map_or(0, String::len),
        request.labels.existing.len(),
        request.labels.incoming.len(),
        request.lens.lens_id.len(),
        request.lens.version.len(),
        request.lens.digest.len(),
        request.base.bytes.len(),
        request.existing.bytes.len(),
        request.incoming.bytes.len(),
    ];
    if sizes
        .into_iter()
        .try_fold(0usize, usize::checked_add)
        .is_none_or(|size| size > limits.input_bytes)
    {
        return Err(WorkerError::InputLimit);
    }
    request
        .validate()
        .map_err(|e| WorkerError::Protocol(e.to_string()))?;
    let known = [NativeLens::Text, NativeLens::Code]
        .iter()
        .any(|lens| native_manifest(*lens).pin().as_ref() == Some(&request.lens));
    if !known {
        return Err(WorkerError::Reconciliation(FileMergeError::PinMismatch));
    }
    let payload = canonical(&Request {
        protocol: PROTOCOL.into(),
        request: request.clone(),
        policy,
    })?;
    if payload.len() > limits.input_bytes {
        return Err(WorkerError::InputLimit);
    }
    #[cfg(windows)]
    let bytes = windows::run(executable, &["--protocol-v1"], &payload, limits)?;
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    let bytes = linux::run(executable, &["--protocol-v1"], &payload, limits)?;
    #[cfg(not(any(windows, all(target_os = "linux", target_arch = "x86_64"))))]
    let bytes = unsupported_platform(executable, &payload, limits)?;
    let reply: Reply =
        serde_json::from_slice(&bytes).map_err(|e| WorkerError::Protocol(e.to_string()))?;
    if canonical(&reply)? != bytes || reply.protocol != PROTOCOL {
        return Err(WorkerError::Protocol(
            "noncanonical or unknown protocol".into(),
        ));
    }
    let plan = reply.result.map_err(WorkerError::Reconciliation)?;
    if plan.request != *request || plan.policy != policy {
        return Err(WorkerError::Protocol(
            "worker changed its canonical request or limits".into(),
        ));
    }
    if let FileMergeOutcome::Semantic(result) = &plan.outcome {
        result
            .validate()
            .map_err(|e| WorkerError::Protocol(e.to_string()))?;
    }
    Ok(SupervisedResult {
        plan,
        enforced_limits: limits,
    })
}
#[cfg(not(any(windows, all(target_os = "linux", target_arch = "x86_64"))))]
fn unsupported_platform(_: &Path, _: &[u8], _: WorkerLimits) -> Result<Vec<u8>, WorkerError> {
    Err(WorkerError::UnsupportedPlatform)
}
pub(crate) fn canonical(value: &impl Serialize) -> Result<Vec<u8>, WorkerError> {
    layrs_core::scm::canonical_json(value).map_err(|e| WorkerError::Protocol(e.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn run_process(
    executable: &Path,
    args: &[&str],
    input: &[u8],
    limits: WorkerLimits,
) -> Result<Vec<u8>, WorkerError> {
    #[cfg(windows)]
    {
        windows::run(executable, args, input, limits)
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        linux::run(executable, args, input, limits)
    }
    #[cfg(not(any(windows, all(target_os = "linux", target_arch = "x86_64"))))]
    {
        let _ = (executable, args, input, limits);
        Err(WorkerError::UnsupportedPlatform)
    }
}

/// One bounded request and one canonical response, then process termination.
pub fn serve(input: impl Read, mut output: impl Write) -> Result<(), WorkerError> {
    let mut bytes = Vec::new();
    input.take((MAX_WIRE + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_WIRE {
        return Err(WorkerError::InputLimit);
    }
    let request: Request =
        serde_json::from_slice(&bytes).map_err(|e| WorkerError::Protocol(e.to_string()))?;
    if request.protocol != PROTOCOL || canonical(&request)? != bytes {
        return Err(WorkerError::Protocol(
            "noncanonical or unknown request".into(),
        ));
    }
    let reply = Reply {
        protocol: PROTOCOL.into(),
        result: reconcile_file(&request.request, request.policy),
    };
    let bytes = canonical(&reply)?;
    if bytes.len() > MAX_WIRE {
        return Err(WorkerError::OutputLimit);
    }
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}
