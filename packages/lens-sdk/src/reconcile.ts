import { LENS_CONTRACT_VERSION } from "./contracts";
import type {
  CanonicalLensConflictBlock,
  CanonicalLensReconcileContent,
  CanonicalLensReconcileResult,
  LensConflictSegment,
  LensContractMetadata,
  LensConflictBlock,
  LensPin,
  LensReconcileContent,
  LensReconcileResponse,
  LensReconcileResult,
  LensReconcileResultStatus,
  ResolutionMethod,
  ResolutionScope,
  ScopedResolutionMethod
} from "./contracts";
import { assertLensPin } from "./validation";

const RESOLUTION_METHOD_ALIASES: Readonly<Record<string, ResolutionMethod>> = {
  existing: "existing",
  ours: "existing",
  target: "existing",
  incoming: "incoming",
  theirs: "incoming",
  source: "incoming",
  both: "both",
  both_ours_then_theirs: "both",
  both_theirs_then_ours: "both",
  both_existing_then_incoming: "both",
  manual: "manual"
};

const RESULT_KEYS = new Set([
  "status",
  "summary",
  "resolved",
  "conflict",
  "fileMethods",
  "resolutionMethods",
  "blocks",
  "segments",
  "fields"
]);

export function toCanonicalLensReconcileResult(result: LensReconcileResult): CanonicalLensReconcileResult {
  assertRecord(result, "Lens reconcile result");
  assertOnlyKeys(result, RESULT_KEYS, "Lens reconcile result");
  const status = canonicalStatus(result.status);
  if (typeof result.summary !== "string") {
    throw new Error("Lens reconcile result summary must be a string");
  }
  if (!Array.isArray(result.blocks) || !Array.isArray(result.segments)) {
    throw new Error("Lens reconcile result blocks and segments must be arrays");
  }

  const resolutionMethods = scopedMethods(
    result.resolutionMethods,
    [result.fileMethods],
    "file",
    "Lens reconcile result"
  );
  const blocks = result.blocks.map(canonicalBlock);
  const segments = result.segments.map(canonicalSegment);
  const fields = canonicalMetadata(result.fields);
  const resolved =
    result.resolved === undefined ? undefined : canonicalContent(result.resolved, "resolved");
  const conflict =
    result.conflict === undefined ? undefined : canonicalContent(result.conflict, "conflict");
  const common = {
    summary: result.summary,
    resolutionMethods,
    blocks,
    segments,
    fields
  };

  switch (status) {
    case "auto_resolved":
      if (!resolved) {
        throw new Error("An auto_resolved result requires resolved content");
      }
      if (conflict) {
        throw new Error("An auto_resolved result cannot carry conflict content");
      }
      assertNoConflictArtifacts(status, resolutionMethods, blocks, segments);
      return { status, resolved, ...common };
    case "conflicted":
      if (!conflict) {
        throw new Error("A conflicted result requires conflict content");
      }
      if (resolved) {
        throw new Error("A conflicted result cannot carry resolved content");
      }
      validateConflictGraph(blocks, segments);
      return { status, conflict, ...common };
    case "unsupported":
      if (resolved || conflict) {
        throw new Error("An unsupported result cannot carry resolved or conflict content");
      }
      assertNoConflictArtifacts(status, resolutionMethods, blocks, segments);
      return { status, ...common };
  }
}

export function toLensReconcileResponse(
  lens: LensPin,
  result: LensReconcileResult
): LensReconcileResponse {
  assertLensPin(lens);
  return {
    contractVersion: LENS_CONTRACT_VERSION,
    lens: { ...lens },
    result: toCanonicalLensReconcileResult(result)
  };
}

function canonicalStatus(value: unknown): LensReconcileResultStatus {
  if (value === "auto_resolved" || value === "conflicted" || value === "unsupported") {
    return value;
  }
  throw new Error(`Unsupported Lens reconcile status ${String(value)}`);
}

function canonicalMetadata(fields: unknown): LensContractMetadata {
  assertRecord(fields, "Lens reconcile result fields");
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

function canonicalContent(content: LensReconcileContent, label: string): CanonicalLensReconcileContent {
  assertRecord(content, `${label} content`);
  assertOnlyKeys(content, new Set(["exists", "bytes"]), `${label} content`);
  if (typeof content.exists !== "boolean") {
    throw new Error(`${label} content exists must be a boolean`);
  }
  if (content.bytes !== undefined && !(content.bytes instanceof Uint8Array)) {
    throw new Error(`${label} content bytes must be a Uint8Array`);
  }
  return {
    exists: content.exists,
    bytes: Array.from(content.bytes ?? [])
  };
}

function canonicalBlock(block: LensConflictBlock, index: number): CanonicalLensConflictBlock {
  assertRecord(block, `Conflict block ${index}`);
  assertOnlyKeys(
    block,
    new Set([
      "blockId",
      "base",
      "ours",
      "theirs",
      "existing",
      "incoming",
      "methods",
      "supportedResolutions",
      "resolutionMethods"
    ]),
    `Conflict block ${index}`
  );
  if (typeof block.blockId !== "string" || block.blockId.length === 0) {
    throw new Error(`Conflict block ${index} requires a non-empty blockId`);
  }
  if (typeof block.base !== "string") {
    throw new Error(`Conflict block ${block.blockId} base must be a string`);
  }

  return {
    blockId: block.blockId,
    base: block.base,
    existing: aliasedString(block.existing, block.ours, "existing", "ours", block.blockId),
    incoming: aliasedString(block.incoming, block.theirs, "incoming", "theirs", block.blockId),
    resolutionMethods: scopedMethods(
      block.resolutionMethods,
      [block.methods, block.supportedResolutions],
      "block",
      `Conflict block ${block.blockId}`
    )
  };
}

function canonicalSegment(segment: LensConflictSegment, index: number): LensConflictSegment {
  assertRecord(segment, `Conflict segment ${index}`);
  assertOnlyKeys(segment, new Set(["kind", "text", "blockId"]), `Conflict segment ${index}`);
  if (segment.kind === "text") {
    if (typeof segment.text !== "string" || segment.blockId !== undefined) {
      throw new Error(`Text conflict segment ${index} requires text and forbids blockId`);
    }
    return { kind: "text", text: segment.text };
  }
  if (segment.kind === "block") {
    if (typeof segment.blockId !== "string" || segment.blockId.length === 0 || segment.text !== undefined) {
      throw new Error(`Block conflict segment ${index} requires blockId and forbids text`);
    }
    return { kind: "block", blockId: segment.blockId };
  }
  throw new Error(`Unsupported conflict segment kind ${String(segment.kind)}`);
}

function scopedMethods(
  canonical: ScopedResolutionMethod[] | undefined,
  legacyCandidates: Array<Array<ResolutionMethod | string> | undefined>,
  scope: ResolutionScope,
  label: string
): ScopedResolutionMethod[] {
  const representations: ScopedResolutionMethod[][] = [];
  if (canonical !== undefined) {
    if (!Array.isArray(canonical)) {
      throw new Error(`${label} resolutionMethods must be an array`);
    }
    representations.push(
      canonical.map((entry, index) => canonicalScopedMethod(entry, scope, `${label} method ${index}`))
    );
  }
  for (const legacy of legacyCandidates) {
    if (legacy === undefined) {
      continue;
    }
    if (!Array.isArray(legacy)) {
      throw new Error(`${label} legacy resolution methods must be an array`);
    }
    representations.push(
      legacy.map((method, index) => ({
        method: canonicalLegacyMethod(method, `${label} legacy method ${index}`),
        scope
      }))
    );
  }

  for (const methods of representations) {
    assertUniqueMethods(methods, label);
  }
  const selected = representations[0] ?? [];
  for (const candidate of representations.slice(1)) {
    if (!sameMethods(selected, candidate)) {
      throw new Error(`${label} has contradictory canonical and legacy resolution methods`);
    }
  }
  return selected.map((method) => ({ ...method }));
}

function canonicalScopedMethod(
  entry: ScopedResolutionMethod,
  scope: ResolutionScope,
  label: string
): ScopedResolutionMethod {
  assertRecord(entry, label);
  assertOnlyKeys(entry, new Set(["method", "scope"]), label);
  if (entry.scope !== scope) {
    throw new Error(`Expected ${scope} resolution method, found ${String(entry.scope)}`);
  }
  return { method: canonicalMethod(entry.method, label), scope };
}

function canonicalMethod(value: unknown, label: string): ResolutionMethod {
  if (value === "existing" || value === "incoming" || value === "both" || value === "manual") {
    return value;
  }
  throw new Error(`${label} uses unsupported resolution method ${String(value)}`);
}

function canonicalLegacyMethod(value: unknown, label: string): ResolutionMethod {
  if (typeof value !== "string" || !Object.hasOwn(RESOLUTION_METHOD_ALIASES, value)) {
    throw new Error(`${label} uses unsupported legacy resolution method ${String(value)}`);
  }
  return RESOLUTION_METHOD_ALIASES[value];
}

function aliasedString(
  canonical: unknown,
  legacy: unknown,
  canonicalName: string,
  legacyName: string,
  blockId: string
): string {
  if (canonical !== undefined && typeof canonical !== "string") {
    throw new Error(`Conflict block ${blockId} ${canonicalName} must be a string`);
  }
  if (legacy !== undefined && typeof legacy !== "string") {
    throw new Error(`Conflict block ${blockId} ${legacyName} must be a string`);
  }
  if (canonical !== undefined && legacy !== undefined && canonical !== legacy) {
    throw new Error(
      `Conflict block ${blockId} has contradictory ${canonicalName}/${legacyName} content`
    );
  }
  const selected = canonical ?? legacy;
  if (selected === undefined) {
    throw new Error(`Conflict block ${blockId} requires ${canonicalName} or ${legacyName} content`);
  }
  return selected;
}

function assertNoConflictArtifacts(
  status: LensReconcileResultStatus,
  methods: ScopedResolutionMethod[],
  blocks: CanonicalLensConflictBlock[],
  segments: LensConflictSegment[]
): void {
  if (methods.length > 0 || blocks.length > 0 || segments.length > 0) {
    throw new Error(`${status} results cannot carry resolution methods, conflict blocks, or segments`);
  }
}

function validateConflictGraph(
  blocks: CanonicalLensConflictBlock[],
  segments: LensConflictSegment[]
): void {
  const blockIds = new Set<string>();
  for (const block of blocks) {
    if (blockIds.has(block.blockId)) {
      throw new Error(`Duplicate conflict block ${block.blockId}`);
    }
    blockIds.add(block.blockId);
  }
  if (blocks.length === 0 && segments.length > 0) {
    throw new Error("Conflict segments require conflict blocks");
  }

  const references = new Map<string, number>();
  for (const segment of segments) {
    if (segment.kind !== "block") {
      continue;
    }
    const blockId = segment.blockId as string;
    if (!blockIds.has(blockId)) {
      throw new Error(`Conflict segment references unknown block ${blockId}`);
    }
    references.set(blockId, (references.get(blockId) ?? 0) + 1);
  }
  for (const blockId of blockIds) {
    if (references.get(blockId) !== 1) {
      throw new Error(`Conflict block ${blockId} must be referenced by exactly one segment`);
    }
  }
}

function assertUniqueMethods(methods: ScopedResolutionMethod[], label: string): void {
  const seen = new Set<string>();
  for (const method of methods) {
    const key = `${method.scope}:${method.method}`;
    if (seen.has(key)) {
      throw new Error(`${label} contains duplicate resolution method ${method.method}`);
    }
    seen.add(key);
  }
}

function sameMethods(left: ScopedResolutionMethod[], right: ScopedResolutionMethod[]): boolean {
  return (
    left.length === right.length &&
    left.every(
      (method, index) =>
        method.method === right[index]?.method && method.scope === right[index]?.scope
    )
  );
}

function assertRecord(value: unknown, label: string): asserts value is Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
}

function assertOnlyKeys(value: object, allowed: Set<string>, label: string): void {
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new Error(`${label} contains unknown field ${key}`);
    }
  }
}
