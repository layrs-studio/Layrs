import { LENS_CONTRACT_VERSION } from "./contracts";
import type {
  CanonicalLensConflictBlock,
  CanonicalLensReconcileContent,
  CanonicalLensReconcileResult,
  LensContractMetadata,
  LensConflictBlock,
  LensPin,
  LensReconcileContent,
  LensReconcileResponse,
  LensReconcileResult,
  ResolutionMethod,
  ResolutionScope,
  ScopedResolutionMethod
} from "./contracts";

export function toCanonicalLensReconcileResult(result: LensReconcileResult): CanonicalLensReconcileResult {
  const canonical: CanonicalLensReconcileResult = {
    status: result.status,
    summary: result.summary,
    ...(result.resolved ? { resolved: canonicalContent(result.resolved) } : {}),
    ...(result.conflict ? { conflict: canonicalContent(result.conflict) } : {}),
    resolutionMethods: scopedMethods(result.resolutionMethods, result.fileMethods, "file"),
    blocks: result.blocks.map(canonicalBlock),
    segments: result.segments.map((segment) => ({ ...segment })),
    fields: canonicalMetadata(result.fields)
  };

  if (canonical.status === "auto_resolved" && !canonical.resolved) {
    throw new Error("An auto_resolved result requires resolved content");
  }
  if (canonical.status === "conflicted" && !canonical.conflict) {
    throw new Error("A conflicted result requires conflict content");
  }
  if (canonical.status === "unsupported" && (canonical.resolved || canonical.conflict)) {
    throw new Error("An unsupported result cannot carry resolved or conflict content");
  }

  return canonical;
}

function canonicalMetadata(fields: Record<string, unknown>): LensContractMetadata {
  const canonical: LensContractMetadata = {};
  for (const [key, value] of Object.entries(fields)) {
    if (
      typeof value === "string" ||
      typeof value === "boolean" ||
      (typeof value === "number" && Number.isFinite(value)) ||
      (Array.isArray(value) && value.every((item) => typeof item === "string"))
    ) {
      canonical[key] = value;
      continue;
    }
    throw new Error(`Unsupported canonical metadata value for ${key}`);
  }
  return canonical;
}

export function toLensReconcileResponse(
  lens: LensPin,
  result: LensReconcileResult
): LensReconcileResponse {
  return {
    contractVersion: LENS_CONTRACT_VERSION,
    lens: { ...lens },
    result: toCanonicalLensReconcileResult(result)
  };
}

function canonicalContent(content: LensReconcileContent): CanonicalLensReconcileContent {
  return {
    exists: content.exists,
    bytes: Array.from(content.bytes ?? [])
  };
}

function canonicalBlock(block: LensConflictBlock): CanonicalLensConflictBlock {
  const existing = block.existing ?? block.ours;
  const incoming = block.incoming ?? block.theirs;
  if (existing === undefined || incoming === undefined) {
    throw new Error(`Conflict block ${block.blockId} requires existing and incoming content`);
  }

  return {
    blockId: block.blockId,
    base: block.base,
    existing,
    incoming,
    resolutionMethods: scopedMethods(
      block.resolutionMethods,
      block.methods ?? block.supportedResolutions,
      "block"
    )
  };
}

function scopedMethods(
  canonical: ScopedResolutionMethod[] | undefined,
  legacy: Array<ResolutionMethod | string> | undefined,
  scope: ResolutionScope
): ScopedResolutionMethod[] {
  if (canonical) {
    for (const method of canonical) {
      if (method.scope !== scope) {
        throw new Error(`Expected ${scope} resolution method, found ${method.scope}`);
      }
    }
    return canonical.map((method) => ({ ...method }));
  }

  return (legacy ?? []).map((method) => ({
    method: method as ResolutionMethod,
    scope
  }));
}
