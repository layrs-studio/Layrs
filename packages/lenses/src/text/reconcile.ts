import type {
  LensConflictBlock,
  LensConflictSegment,
  LensReconcileContent,
  LensReconcileInput,
  LensReconcileResult,
  LensReconcileSide,
  ResolutionMethod
} from "@layrs/lens-sdk";
import { assertLensReconcileSide } from "@layrs/lens-sdk";
import { textManifest } from "../manifests";
import { declaredResolutionMethods } from "../shared/resolutionMethods";

interface TextChangeRange {
  baseStart: number;
  baseEnd: number;
  replacement: string[];
}

export function prepareTextReconcile(request: LensReconcileInput): LensReconcileResult {
  const inputError = reconcileInputError(request);
  if (inputError) {
    return unsupported(inputError);
  }
  if (sameSide(request.ours, request.theirs)) {
    return autoResolved("Text sides are identical", contentFromSide(request.ours));
  }
  if (sameSide(request.base, request.ours)) {
    return autoResolved("Existing text is unchanged; taking incoming", contentFromSide(request.theirs));
  }
  if (sameSide(request.base, request.theirs)) {
    return autoResolved("Incoming text is unchanged; keeping existing", contentFromSide(request.ours));
  }
  if (!request.base.exists || !request.ours.exists || !request.theirs.exists) {
    return unsupported("Text delete/modify conflicts require explicit resolution");
  }

  const baseText = decodeSide(request.base, "Base");
  if (typeof baseText !== "string") {
    return baseText;
  }
  const existingText = decodeSide(request.ours, "Existing");
  if (typeof existingText !== "string") {
    return existingText;
  }
  const incomingText = decodeSide(request.theirs, "Incoming");
  if (typeof incomingText !== "string") {
    return incomingText;
  }

  const baseLines = splitTextLines(baseText);
  const existingChanges = textChangeRanges(baseLines, splitTextLines(existingText));
  const incomingChanges = textChangeRanges(baseLines, splitTextLines(incomingText));
  const blocks: LensConflictBlock[] = [];
  const merged: string[] = [];
  const segments: LensConflictSegment[] = [];
  let baseIndex = 0;
  let existingIndex = 0;
  let incomingIndex = 0;

  while (existingIndex < existingChanges.length || incomingIndex < incomingChanges.length) {
    const existingChange = existingChanges[existingIndex];
    const incomingChange = incomingChanges[incomingIndex];

    if (existingChange && incomingChange && existingChange.baseEnd <= incomingChange.baseStart) {
      appendPlainLines(merged, segments, baseLines, baseIndex, existingChange.baseStart);
      appendPlainText(merged, segments, joinLines(existingChange.replacement));
      baseIndex = existingChange.baseEnd;
      existingIndex += 1;
      continue;
    }
    if (existingChange && incomingChange && incomingChange.baseEnd <= existingChange.baseStart) {
      appendPlainLines(merged, segments, baseLines, baseIndex, incomingChange.baseStart);
      appendPlainText(merged, segments, joinLines(incomingChange.replacement));
      baseIndex = incomingChange.baseEnd;
      incomingIndex += 1;
      continue;
    }
    if (existingChange && incomingChange) {
      const start = Math.min(existingChange.baseStart, incomingChange.baseStart);
      let end = start;
      const existingGroup: TextChangeRange[] = [];
      const incomingGroup: TextChangeRange[] = [];

      while (true) {
        let advanced = false;
        while (existingChanges[existingIndex]?.baseStart <= end) {
          const change = existingChanges[existingIndex];
          end = Math.max(end, change.baseEnd);
          existingGroup.push(change);
          existingIndex += 1;
          advanced = true;
        }
        while (incomingChanges[incomingIndex]?.baseStart <= end) {
          const change = incomingChanges[incomingIndex];
          end = Math.max(end, change.baseEnd);
          incomingGroup.push(change);
          incomingIndex += 1;
          advanced = true;
        }
        if (!advanced) {
          break;
        }
      }

      appendPlainLines(merged, segments, baseLines, baseIndex, start);
      const blockBase = joinLines(baseLines.slice(start, end));
      const blockExisting = applyTextChanges(baseLines, start, end, existingGroup);
      const blockIncoming = applyTextChanges(baseLines, start, end, incomingGroup);
      if (blockExisting === blockIncoming) {
        appendPlainText(merged, segments, blockExisting);
      } else {
        const blockId = `block-${blocks.length + 1}`;
        appendConflictMarker(
          merged,
          request.oursLabel,
          request.theirsLabel,
          blockExisting,
          blockIncoming
        );
        segments.push({ kind: "block", blockId });
        blocks.push({
          blockId,
          base: blockBase,
          existing: blockExisting,
          incoming: blockIncoming,
          resolutionMethods: textBlockMethods().map((method) => ({ method, scope: "block" }))
        });
      }
      baseIndex = end;
      continue;
    }
    if (existingChange) {
      appendPlainLines(merged, segments, baseLines, baseIndex, existingChange.baseStart);
      appendPlainText(merged, segments, joinLines(existingChange.replacement));
      baseIndex = existingChange.baseEnd;
      existingIndex += 1;
      continue;
    }
    if (incomingChange) {
      appendPlainLines(merged, segments, baseLines, baseIndex, incomingChange.baseStart);
      appendPlainText(merged, segments, joinLines(incomingChange.replacement));
      baseIndex = incomingChange.baseEnd;
      incomingIndex += 1;
    }
  }

  appendPlainLines(merged, segments, baseLines, baseIndex, baseLines.length);
  const mergedContent = presentContent(joinLines(merged));
  if (blocks.length === 0) {
    return {
      status: "auto_resolved",
      summary: "Text auto-merged without conflicts",
      resolved: mergedContent,
      blocks: [],
      segments: [],
      fields: {}
    };
  }

  return {
    status: "conflicted",
    summary: `Text merge has ${blocks.length} unresolved block(s)`,
    conflict: mergedContent,
    blocks,
    segments,
    fields: {}
  };
}

export function resolveTextBlockChoice(
  existing: string,
  incoming: string,
  method: ResolutionMethod,
  manualText?: string
): string {
  if (!textBlockMethods().includes(method)) {
    throw new Error(`Text block resolution ${String(method)} is not supported`);
  }

  switch (method) {
    case "existing":
      return existing;
    case "incoming":
      return incoming;
    case "both":
      return `${ensureTrailingNewline(existing)}${incoming}`;
    case "manual":
      if (manualText === undefined) {
        throw new Error("Manual text block resolution requires replacement text");
      }
      return manualText;
    default:
      throw new Error(`Text block resolution ${String(method)} is not supported`);
  }
}

function decodeSide(side: LensReconcileSide, label: string): string | LensReconcileResult {
  assertLensReconcileSide(side, `${label} text`);
  if (!side.exists) {
    return unsupported(`${label} text is absent`);
  }
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(side.bytes);
  } catch {
    return unsupported(`${label} text is not valid UTF-8`);
  }
}

function reconcileInputError(request: LensReconcileInput): string | undefined {
  for (const [label, side] of [
    ["Base text", request.base],
    ["Existing text", request.ours],
    ["Incoming text", request.theirs]
  ] as const) {
    try {
      assertLensReconcileSide(side, label);
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
  if (!left.exists || !right.exists) {
    return true;
  }
  return left.bytes.length === right.bytes.length && left.bytes.every((byte, index) => byte === right.bytes[index]);
}

function contentFromSide(side: LensReconcileSide): LensReconcileContent {
  assertLensReconcileSide(side, "Selected text side");
  return side.exists
    ? { exists: true, bytes: new Uint8Array(side.bytes) }
    : { exists: false, bytes: new Uint8Array() };
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

function textBlockMethods(): ResolutionMethod[] {
  return declaredResolutionMethods(textManifest, "block");
}

function presentContent(text: string): LensReconcileContent {
  return { exists: true, bytes: new TextEncoder().encode(text) };
}

function splitTextLines(text: string): string[] {
  const lines: string[] = [];
  let start = 0;
  while (start < text.length) {
    const newline = text.indexOf("\n", start);
    if (newline < 0) {
      lines.push(text.slice(start));
      break;
    }
    lines.push(text.slice(start, newline + 1));
    start = newline + 1;
  }
  return lines;
}

function appendPlainLines(
  output: string[],
  segments: LensConflictSegment[],
  base: string[],
  start: number,
  end: number
) {
  appendPlainText(output, segments, joinLines(base.slice(start, end)));
}

function appendPlainText(output: string[], segments: LensConflictSegment[], text: string) {
  if (!text) {
    return;
  }
  output.push(text);
  segments.push({ kind: "text", text });
}

function appendConflictMarker(
  output: string[],
  existingLabel: string,
  incomingLabel: string,
  existing: string,
  incoming: string
) {
  output.push(`<<<<<<< target:${existingLabel}\n`);
  output.push(ensureTrailingNewline(existing));
  output.push("=======\n");
  output.push(ensureTrailingNewline(incoming));
  output.push(`>>>>>>> source:${incomingLabel}\n`);
}

function ensureTrailingNewline(text: string): string {
  return !text || text.endsWith("\n") ? text : `${text}\n`;
}

function joinLines(lines: string[]): string {
  return lines.join("");
}

function applyTextChanges(
  base: string[],
  start: number,
  end: number,
  changes: TextChangeRange[]
): string {
  const output: string[] = [];
  let cursor = start;
  const ordered = [...changes].sort(
    (left, right) => left.baseStart - right.baseStart || left.baseEnd - right.baseEnd
  );
  for (const change of ordered) {
    output.push(...base.slice(cursor, change.baseStart));
    output.push(...change.replacement);
    cursor = change.baseEnd;
  }
  output.push(...base.slice(cursor, end));
  return joinLines(output);
}

function textChangeRanges(base: string[], variant: string[]): TextChangeRange[] {
  if (sameLines(base, variant)) {
    return [];
  }
  const matches = lcsLineMatches(base, variant);
  if (!matches) {
    return [{ baseStart: 0, baseEnd: base.length, replacement: variant }];
  }

  const ranges: TextChangeRange[] = [];
  let baseCursor = 0;
  let variantCursor = 0;
  for (const [baseMatch, variantMatch] of [...matches, [base.length, variant.length] as const]) {
    if (baseCursor !== baseMatch || variantCursor !== variantMatch) {
      ranges.push({
        baseStart: baseCursor,
        baseEnd: baseMatch,
        replacement: variant.slice(variantCursor, variantMatch)
      });
    }
    baseCursor = baseMatch + 1;
    variantCursor = variantMatch + 1;
  }
  return ranges;
}

function lcsLineMatches(base: string[], variant: string[]): Array<readonly [number, number]> | undefined {
  const cells = (base.length + 1) * (variant.length + 1);
  if (cells > 4_000_000) {
    return undefined;
  }

  const width = variant.length + 1;
  const table = new Uint32Array(cells);
  for (let baseIndex = base.length - 1; baseIndex >= 0; baseIndex -= 1) {
    for (let variantIndex = variant.length - 1; variantIndex >= 0; variantIndex -= 1) {
      const index = baseIndex * width + variantIndex;
      table[index] =
        base[baseIndex] === variant[variantIndex]
          ? table[(baseIndex + 1) * width + variantIndex + 1] + 1
          : Math.max(
              table[(baseIndex + 1) * width + variantIndex],
              table[baseIndex * width + variantIndex + 1]
            );
    }
  }

  const matches: Array<readonly [number, number]> = [];
  let baseIndex = 0;
  let variantIndex = 0;
  while (baseIndex < base.length && variantIndex < variant.length) {
    if (base[baseIndex] === variant[variantIndex]) {
      matches.push([baseIndex, variantIndex]);
      baseIndex += 1;
      variantIndex += 1;
    } else if (
      table[(baseIndex + 1) * width + variantIndex] >=
      table[baseIndex * width + variantIndex + 1]
    ) {
      baseIndex += 1;
    } else {
      variantIndex += 1;
    }
  }
  return matches;
}

function sameLines(left: string[], right: string[]): boolean {
  return left.length === right.length && left.every((line, index) => line === right[index]);
}
