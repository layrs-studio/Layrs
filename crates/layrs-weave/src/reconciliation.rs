//! Pure, bounded reconciliation of a single regular file. This is a building
//! block, not an accepted Weave: it creates no Step, receipt, CAS or references.
//! The request retains exact values, including Absent. Tree modes, DAG bases,
//! canonical ConflictObject term IDs and receipt profiles belong to the next
//! integration layer and must not be inferred from these execution-local roles.

use layrs_lens_sdk::{
    LensManifest, LensReconcileContent, LensReconcileRequest, LensReconcileResult,
    LensReconcileResultStatus, LensReconcileSideContract, sha256_digest,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NativeLens {
    Text,
    Code,
}

/// Deterministic work limits, not a wall-clock timeout or sandbox memory quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileMergeLimits {
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
    pub max_diff_cells: usize,
    pub max_blocks: usize,
    pub max_segments: usize,
}

impl Default for FileMergeLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 1024 * 1024,
            max_output_bytes: 4 * 1024 * 1024,
            max_diff_cells: 1_000_000,
            max_blocks: 4096,
            max_segments: 16384,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FailureAction {
    Reject,
    StructuralFallback,
}

/// An explicit local execution policy. This is not the complete, hashed
/// DeterminismProfile from ADR 0010; admission must record that profile later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileMergePolicy {
    pub unsupported: FailureAction,
    pub quota: FailureAction,
    pub limits: FileMergeLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FallbackReason {
    UnsupportedLens,
    UnsupportedContent,
    OrderDependentOutput,
    InputLimit,
    DiffWorkLimit,
    OutputLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FileMergeError {
    InvalidInput(String),
    PinMismatch,
    ContentDigestMismatch,
    InvalidOutput(String),
    Rejected(FallbackReason),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FileMergeOutcome {
    /// Includes clean values or structured semantic blocks. Conflict marker
    /// bytes are preview data; callers must persist variants, not those markers.
    Semantic(LensReconcileResult),
    /// No side was selected and no bytes were decoded or rewritten.
    Structural(FallbackReason),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FileMergePlan {
    pub request: LensReconcileRequest,
    pub policy: FileMergePolicy,
    pub outcome: FileMergeOutcome,
}

/// Whole-value choices use execution-local roles only. The graph layer must map
/// these to explicit canonical term IDs before offering persistent resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FileValueChoice {
    Base,
    Existing,
    Incoming,
    Delete,
}

impl FileMergePlan {
    pub fn choose_value(&self, choice: FileValueChoice) -> LensReconcileContent {
        match choice {
            FileValueChoice::Base => content(&self.request.base),
            FileValueChoice::Existing => content(&self.request.existing),
            FileValueChoice::Incoming => content(&self.request.incoming),
            FileValueChoice::Delete => LensReconcileContent::absent(),
        }
    }
}

/// Pins the opt-in native implementation, including the wrapper and SDK sources.
/// Source line endings are normalized so Windows and Unix checkouts agree.
/// This pin is deliberately distinct from legacy registry pins.
pub fn native_manifest(lens: NativeLens) -> LensManifest {
    let mut manifest = match lens {
        NativeLens::Code => layrs_lens_code::reconciliation_manifest(),
        NativeLens::Text => layrs_lens_text::manifest(),
    };
    static DIGEST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    manifest.digest = Some(
        DIGEST
            .get_or_init(|| {
                let sources = [
                    include_str!("../../layrs-core/src/scm/canonical.rs"),
                    include_str!("../../layrs-core/src/scm/path.rs"),
                    include_str!("../../layrs-lens-code/src/lib.rs"),
                    include_str!("../../layrs-lens-sdk/src/contract.rs"),
                    include_str!("../../layrs-lens-sdk/src/lib.rs"),
                    include_str!("../../layrs-lens-sdk/src/manifest_validation.rs"),
                    include_str!("../../layrs-lens-sdk/src/selection.rs"),
                    include_str!("../../layrs-lens-sdk/src/sha256.rs"),
                    include_str!("../../layrs-lens-sdk/src/validation.rs"),
                    include_str!("../../layrs-lens-sdk/src/wire.rs"),
                    include_str!("../../layrs-lens-text/src/lib.rs"),
                    include_str!("reconciliation.rs"),
                ];
                let mut bytes = b"layrs-native-file-merge-v1\0".to_vec();
                for source in sources {
                    let normalized = source.replace("\r\n", "\n");
                    bytes.extend_from_slice(&(normalized.len() as u64).to_be_bytes());
                    bytes.extend_from_slice(normalized.as_bytes());
                }
                sha256_digest(&bytes)
            })
            .clone(),
    );
    manifest
}

/// Reuses canonical Lens request/result validation. Malformed values, wrong
/// content digests and mismatching known pins always reject, even when fallback
/// is enabled. Unknown Lenses may only produce structural output.
pub fn reconcile_file(
    request: &LensReconcileRequest,
    policy: FileMergePolicy,
) -> Result<FileMergePlan, FileMergeError> {
    request
        .validate()
        .map_err(|e| FileMergeError::InvalidInput(e.to_string()))?;
    layrs_core::scm::RepoPath::parse(request.path.as_str())
        .map_err(|e| FileMergeError::InvalidInput(e.to_string()))?;
    let lens = match request.lens.lens_id.as_str() {
        layrs_lens_text::TEXT_LENS_ID => Some(NativeLens::Text),
        layrs_lens_code::CODE_LENS_ID => Some(NativeLens::Code),
        _ => None,
    };
    if let Some(lens) = lens {
        if native_manifest(lens).pin().as_ref() != Some(&request.lens) {
            return Err(FileMergeError::PinMismatch);
        }
    }
    // Input bytes include labels and path; markers cannot amplify unbounded
    // caller-controlled labels. Check before cloning any payload.
    let input_size = [
        request.base.bytes.len(),
        request.existing.bytes.len(),
        request.incoming.bytes.len(),
        request.path.len(),
        request.labels.existing.len(),
        request.labels.incoming.len(),
        request.media_type.as_ref().map_or(0, String::len),
        request.lens.lens_id.len(),
        request.lens.version.len(),
        request.lens.digest.len(),
    ]
    .into_iter()
    .try_fold(0usize, usize::checked_add);
    if input_size.is_none_or(|size| size > policy.limits.max_input_bytes) {
        // A fallback would still clone an unbounded request. Admission must
        // reject this input rather than pretending a fallback meets its quota.
        return Err(FileMergeError::Rejected(FallbackReason::InputLimit));
    }
    for side in [&request.base, &request.existing, &request.incoming] {
        if side
            .content_hash
            .as_ref()
            .is_some_and(|digest| *digest != sha256_digest(&side.bytes))
        {
            return Err(FileMergeError::ContentDigestMismatch);
        }
    }
    let Some(lens) = lens else {
        return fallback(
            request,
            policy,
            FallbackReason::UnsupportedLens,
            policy.unsupported,
        );
    };
    // Trivial byte-wise rules also handle binary data and Absent faithfully.
    let resolved = if same_value(&request.existing, &request.incoming) {
        Some(&request.existing)
    } else if same_value(&request.base, &request.existing) {
        Some(&request.incoming)
    } else if same_value(&request.base, &request.incoming) {
        Some(&request.existing)
    } else {
        None
    };
    if let Some(side) = resolved {
        if side.bytes.len() > policy.limits.max_output_bytes {
            return fallback(request, policy, FallbackReason::OutputLimit, policy.quota);
        }
        return Ok(plan(
            request,
            policy,
            LensReconcileResult::auto_resolved("Identical or unchanged whole value", content(side)),
        ));
    }
    if [&request.base, &request.existing, &request.incoming]
        .iter()
        .any(|side| !side.exists || std::str::from_utf8(&side.bytes).is_err())
    {
        return fallback(
            request,
            policy,
            FallbackReason::UnsupportedContent,
            policy.unsupported,
        );
    }
    let lines = |side: &LensReconcileSideContract| -> usize {
        if side.bytes.is_empty() {
            0
        } else {
            side.bytes.iter().filter(|b| **b == b'\n').count()
                + usize::from(side.bytes.last() != Some(&b'\n'))
        }
    };
    let base = lines(&request.base) + 1;
    // The symmetry check below runs both orders, so account for both passes.
    let cells = (lines(&request.existing) + 1)
        .checked_add(lines(&request.incoming) + 1)
        .and_then(|n| n.checked_mul(base))
        .and_then(|n| n.checked_mul(2));
    if cells.is_none_or(|n| n > policy.limits.max_diff_cells) {
        return fallback(request, policy, FallbackReason::DiffWorkLimit, policy.quota);
    }
    // Conservatively budget repeated marker labels and retained block/segment
    // text before the Lens allocates them, then check the actual output below.
    let line_sum = base
        .checked_add(lines(&request.existing) + 1)
        .and_then(|n| n.checked_add(lines(&request.incoming) + 1));
    let label_cost = request
        .labels
        .existing
        .len()
        .checked_add(request.labels.incoming.len())
        .and_then(|n| n.checked_add(128));
    let output_bound = line_sum
        .zip(label_cost)
        .and_then(|(n, width)| n.checked_mul(width))
        .zip(input_size)
        .and_then(|(markers, bytes)| {
            bytes
                .checked_mul(8)
                .and_then(|bytes| bytes.checked_add(markers))
        });
    if output_bound.is_none_or(|n| n > policy.limits.max_output_bytes) {
        return fallback(request, policy, FallbackReason::OutputLimit, policy.quota);
    }
    let input = request
        .try_as_execution_input()
        .map_err(|e| FileMergeError::InvalidInput(e.to_string()))?;
    let execute = |input| match lens {
        NativeLens::Text => layrs_lens_text::reconcile_text(input),
        NativeLens::Code => layrs_lens_code::reconcile_code(input),
    };
    let result = execute(input);
    result
        .validate()
        .map_err(|e| FileMergeError::InvalidOutput(e.to_string()))?;
    if result.status == LensReconcileResultStatus::Unsupported {
        return fallback(
            request,
            policy,
            FallbackReason::UnsupportedContent,
            policy.unsupported,
        );
    }
    if result.status == LensReconcileResultStatus::AutoResolved {
        let reverse = execute(layrs_lens_sdk::LensReconcileInput {
            ours: input.theirs,
            theirs: input.ours,
            ours_label: input.theirs_label,
            theirs_label: input.ours_label,
            ..input
        });
        reverse
            .validate()
            .map_err(|e| FileMergeError::InvalidOutput(e.to_string()))?;
        // Legacy line merging can concatenate competing insertions at the same
        // boundary in side order. Such ordering must never select accepted bytes.
        if reverse.status != result.status || reverse.resolved != result.resolved {
            return fallback(
                request,
                policy,
                FallbackReason::OrderDependentOutput,
                policy.unsupported,
            );
        }
    }
    let bytes = result
        .resolved
        .iter()
        .chain(result.conflict.iter())
        .map(|v| v.bytes.len())
        .chain(
            result
                .blocks
                .iter()
                .flat_map(|b| [b.base.len(), b.ours.len(), b.theirs.len()]),
        )
        .chain(
            result
                .segments
                .iter()
                .map(|s| s.text.as_ref().map_or(0, String::len)),
        )
        .try_fold(0usize, usize::checked_add);
    if result.blocks.len() > policy.limits.max_blocks
        || result.segments.len() > policy.limits.max_segments
        || bytes.is_none_or(|n| n > policy.limits.max_output_bytes)
    {
        return fallback(request, policy, FallbackReason::OutputLimit, policy.quota);
    }
    Ok(plan(request, policy, result))
}

fn same_value(a: &LensReconcileSideContract, b: &LensReconcileSideContract) -> bool {
    a.exists == b.exists && a.bytes == b.bytes
}

fn content(side: &LensReconcileSideContract) -> LensReconcileContent {
    LensReconcileContent {
        exists: side.exists,
        bytes: side.bytes.clone(),
    }
}

fn plan(
    request: &LensReconcileRequest,
    policy: FileMergePolicy,
    result: LensReconcileResult,
) -> FileMergePlan {
    FileMergePlan {
        request: request.clone(),
        policy,
        outcome: FileMergeOutcome::Semantic(result),
    }
}

fn fallback(
    request: &LensReconcileRequest,
    policy: FileMergePolicy,
    reason: FallbackReason,
    action: FailureAction,
) -> Result<FileMergePlan, FileMergeError> {
    match action {
        FailureAction::Reject => Err(FileMergeError::Rejected(reason)),
        FailureAction::StructuralFallback => Ok(FileMergePlan {
            request: request.clone(),
            policy,
            outcome: FileMergeOutcome::Structural(reason),
        }),
    }
}

#[cfg(test)]
mod tests;
