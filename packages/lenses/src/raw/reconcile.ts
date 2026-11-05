import type {
  LensReconcileContent,
  LensReconcileInput,
  LensReconcileResult,
  LensReconcileSide,
  ResolutionMethod
} from "@layrs/lens-sdk";
import { assertLensReconcileSide } from "@layrs/lens-sdk";
import { rawManifest } from "../manifests";
import { declaredResolutionMethods } from "../shared/resolutionMethods";

export function prepareRawReconcile(request: LensReconcileInput): LensReconcileResult {
  const inputError = reconcileInputError(request);
  if (inputError) {
    return unsupported(inputError);
  }

  if (sameSide(request.ours, request.theirs)) {
    return autoResolved("Raw sides are identical", contentFromSide(request.ours));
  }
  if (sameSide(request.base, request.ours)) {
    return autoResolved("Existing side is unchanged; taking incoming", contentFromSide(request.theirs));
  }
  if (sameSide(request.base, request.theirs)) {
    return autoResolved("Incoming side is unchanged; keeping existing", contentFromSide(request.ours));
  }

  return {
    status: "conflicted",
    summary: "Raw content changed differently on both sides",
    conflict: contentFromSide(request.ours),
    resolutionMethods: declaredResolutionMethods(rawManifest, "file").map((method) => ({
      method,
      scope: "file"
    })),
    blocks: [],
    segments: [],
    fields: {}
  };
}

export function resolveRawConflict(
  request: Pick<LensReconcileInput, "ours" | "theirs">,
  method: ResolutionMethod
): LensReconcileContent {
  const declared = declaredResolutionMethods(rawManifest, "file");
  if (!declared.includes(method)) {
    throw new Error(`Raw file resolution ${String(method)} is not supported`);
  }

  const selected = method === "existing" ? request.ours : request.theirs;
  assertLensReconcileSide(selected, `Raw ${method}`);

  switch (method) {
    case "existing":
      return contentFromSide(request.ours);
    case "incoming":
      return contentFromSide(request.theirs);
    default:
      throw new Error(`Raw file resolution ${String(method)} is not supported`);
  }
}

function autoResolved(summary: string, resolved: LensReconcileContent): LensReconcileResult {
  return {
    status: "auto_resolved",
    summary,
    resolved,
    blocks: [],
    segments: [],
    fields: {}
  };
}

function unsupported(summary: string): LensReconcileResult {
  return {
    status: "unsupported",
    summary,
    blocks: [],
    segments: [],
    fields: {}
  };
}

function reconcileInputError(request: LensReconcileInput): string | undefined {
  for (const [label, side] of [
    ["base", request.base],
    ["existing", request.ours],
    ["incoming", request.theirs]
  ] as const) {
    try {
      assertLensReconcileSide(side, `Raw ${label}`);
    } catch (error) {
      return error instanceof Error ? error.message : String(error);
    }
  }
  return undefined;
}

function sameSide(left: LensReconcileSide, right: LensReconcileSide): boolean {
  if (left.exists !== right.exists) {
    return false;
  }
  if (!left.exists) {
    return true;
  }

  const leftBytes = left.bytes;
  const rightBytes = right.bytes;
  if (!leftBytes || !rightBytes || leftBytes.length !== rightBytes.length) {
    return false;
  }
  return leftBytes.every((byte, index) => byte === rightBytes[index]);
}

function contentFromSide(side: LensReconcileSide): LensReconcileContent {
  assertLensReconcileSide(side, "Raw selected side");
  return side.exists
    ? { exists: true, bytes: new Uint8Array(side.bytes) }
    : { exists: false, bytes: new Uint8Array() };
}
