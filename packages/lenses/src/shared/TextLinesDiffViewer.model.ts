import type { DiffColumnWindow, DiffLine, DiffLineWindow, DiffModel, Dimensions, LensSurfaceMetadata } from "@layrs/lens-sdk";
import {
  DEFAULT_DIFF_OVERSCAN_LINE_COUNT,
  DEFAULT_DIFF_RENDER_LINE_COUNT,
  DEFAULT_DIFF_ROW_HEIGHT_PX,
  DEFAULT_WRAPPED_DIFF_ROW_HEIGHT_PX,
  MAX_DIFF_RENDER_LINE_COUNT,
  MAX_DIFF_ROW_HEIGHT_PX,
  MIN_DIFF_RENDER_LINE_COUNT,
  MIN_DIFF_ROW_HEIGHT_PX
} from "./TextLinesDiffViewer.constants";
import type {
  DiffColumnRow,
  DiffEntry,
  DiffRenderMetadata,
  DiffScrollFrame,
  DiffViewMode,
  DiffVirtualizationState,
  DiffVirtualRange,
  FieldRecord
} from "./TextLinesDiffViewer.types";

export function flattenDiffEntries(diff: DiffModel, viewMode: DiffViewMode): DiffEntry[] {
  return diff.hunks.flatMap((hunk, hunkIndex) => {
    const hunkKey = getHunkKey(hunk, hunkIndex);
    const lineEntries =
      viewMode === "wholeFile"
        ? hunk.lines.map((line, lineIndex) => createDiffLineEntry(hunk, hunkIndex, line, lineIndex))
        : compactChangedDiffEntries(hunk, hunkIndex);

    if (lineEntries.length === 0) {
      return [];
    }

    const entries: DiffEntry[] = [
      {
        kind: "hunk",
        key: `${hunkKey}-title`,
        hunk,
        hunkIndex
      }
    ];

    entries.push(...lineEntries);

    return entries;
  });
}

export function flattenColumnDiffEntries(diff: DiffModel, viewMode: DiffViewMode): DiffEntry[] {
  return diff.hunks.flatMap((hunk, hunkIndex) => {
    const hunkKey = getHunkKey(hunk, hunkIndex);
    const lineEntries = createColumnDiffEntries(hunk, hunkIndex, viewMode);

    if (lineEntries.length === 0) {
      return [];
    }

    const entries: DiffEntry[] = [
      {
        kind: "hunk",
        key: `${hunkKey}-title`,
        hunk,
        hunkIndex
      }
    ];

    entries.push(...lineEntries);

    return entries;
  });
}

function createColumnDiffEntries(
  hunk: DiffModel["hunks"][number],
  hunkIndex: number,
  viewMode: DiffViewMode
): DiffEntry[] {
  const entries: DiffEntry[] = [];
  let hiddenStartIndex: number | undefined;
  let hasChangeRow = false;
  let lineIndex = 0;

  const flushHiddenLines = (endIndex: number) => {
    if (viewMode === "wholeFile" || hiddenStartIndex === undefined || endIndex <= hiddenStartIndex) {
      hiddenStartIndex = undefined;
      return;
    }

    const hiddenLines = hunk.lines.slice(hiddenStartIndex, endIndex);
    const first = hiddenLines[0];
    const last = hiddenLines.at(-1);
    entries.push({
      kind: "hidden",
      key: `${hunkIndex}-column-hidden-${hiddenStartIndex}-${endIndex}`,
      count: hiddenLines.length,
      oldStart: first?.oldLine,
      oldEnd: last?.oldLine,
      newStart: first?.newLine,
      newEnd: last?.newLine
    });
    hiddenStartIndex = undefined;
  };

  while (lineIndex < hunk.lines.length) {
    const line = hunk.lines[lineIndex];
    if (!line) {
      lineIndex += 1;
      continue;
    }

    if (line.op === "equal") {
      if (viewMode === "wholeFile") {
        entries.push(createColumnDiffLineEntry(hunk, hunkIndex, { fileLine: line, lineIndex }));
      } else {
        hiddenStartIndex ??= lineIndex;
      }
      lineIndex += 1;
      continue;
    }

    flushHiddenLines(lineIndex);

    const blockStart = lineIndex;
    const insertLines: Array<{ line: DiffLine; lineIndex: number }> = [];
    const deleteLines: Array<{ line: DiffLine; lineIndex: number }> = [];

    while (lineIndex < hunk.lines.length && hunk.lines[lineIndex]?.op !== "equal") {
      const changedLine = hunk.lines[lineIndex];
      if (changedLine?.op === "insert") {
        insertLines.push({ line: changedLine, lineIndex });
      } else if (changedLine?.op === "delete") {
        deleteLines.push({ line: changedLine, lineIndex });
      }
      lineIndex += 1;
    }

    const rowCount = Math.max(insertLines.length, deleteLines.length);
    for (let rowIndex = 0; rowIndex < rowCount; rowIndex += 1) {
      entries.push(
        createColumnDiffLineEntry(hunk, hunkIndex, {
          insertLine: insertLines[rowIndex]?.line,
          deleteLine: deleteLines[rowIndex]?.line,
          lineIndex: Math.min(insertLines[rowIndex]?.lineIndex ?? Number.MAX_SAFE_INTEGER, deleteLines[rowIndex]?.lineIndex ?? Number.MAX_SAFE_INTEGER, blockStart + rowIndex)
        })
      );
      hasChangeRow = true;
    }
  }

  flushHiddenLines(hunk.lines.length);

  return viewMode === "wholeFile" || hasChangeRow ? entries : [];
}

function compactChangedDiffEntries(hunk: DiffModel["hunks"][number], hunkIndex: number): DiffEntry[] {
  const entries: DiffEntry[] = [];
  let hiddenStartIndex: number | undefined;

  const flushHiddenLines = (endIndex: number) => {
    if (hiddenStartIndex === undefined || endIndex <= hiddenStartIndex) {
      hiddenStartIndex = undefined;
      return;
    }

    const hiddenLines = hunk.lines.slice(hiddenStartIndex, endIndex);
    const first = hiddenLines[0];
    const last = hiddenLines.at(-1);
    entries.push({
      kind: "hidden",
      key: `${hunkIndex}-hidden-${hiddenStartIndex}-${endIndex}`,
      count: hiddenLines.length,
      oldStart: first?.oldLine,
      oldEnd: last?.oldLine,
      newStart: first?.newLine,
      newEnd: last?.newLine
    });
    hiddenStartIndex = undefined;
  };

  hunk.lines.forEach((line, lineIndex) => {
    if (line.op === "equal") {
      hiddenStartIndex ??= lineIndex;
      return;
    }

    flushHiddenLines(lineIndex);
    entries.push(createDiffLineEntry(hunk, hunkIndex, line, lineIndex));
  });

  flushHiddenLines(hunk.lines.length);

  return entries.some((entry) => entry.kind === "line") ? entries : [];
}

function createDiffLineEntry(
  hunk: DiffModel["hunks"][number],
  hunkIndex: number,
  line: DiffLine,
  lineIndex: number
): DiffEntry {
  return {
    kind: "line",
    key: `${hunkIndex}-${lineIndex}-${line.op}-${line.oldLine ?? ""}-${line.newLine ?? ""}`,
    row: {
      hunk,
      hunkIndex,
      line,
      lineIndex
    }
  };
}

function createColumnDiffLineEntry(
  hunk: DiffModel["hunks"][number],
  hunkIndex: number,
  input: {
    fileLine?: DiffLine;
    insertLine?: DiffLine;
    deleteLine?: DiffLine;
    lineIndex: number;
  }
): DiffEntry {
  return {
    kind: "columnLine",
    key: `${hunkIndex}-column-${input.lineIndex}-${input.fileLine?.newLine ?? ""}-${input.insertLine?.newLine ?? ""}-${input.deleteLine?.oldLine ?? ""}`,
    row: {
      hunk,
      hunkIndex,
      fileLine: input.fileLine,
      insertLine: input.insertLine,
      deleteLine: input.deleteLine,
      lineIndex: input.lineIndex
    }
  };
}

export function columnRowFromLine(line: DiffLine): DiffColumnRow {
  return {
    hunk: {
      oldStart: line.oldLine ?? 1,
      oldLines: line.oldLine === undefined ? 0 : 1,
      newStart: line.newLine ?? 1,
      newLines: line.newLine === undefined ? 0 : 1,
      lines: [line]
    },
    hunkIndex: 0,
    fileLine: line.op === "equal" ? line : undefined,
    insertLine: line.op === "insert" ? line : undefined,
    deleteLine: line.op === "delete" ? line : undefined,
    lineIndex: 0
  };
}

export function getHiddenUnchangedLineCount(entries: DiffEntry[]): number {
  return entries.reduce((count, entry) => (entry.kind === "hidden" ? count + entry.count : count), 0);
}

export function getRenderedEntryLineCount(entries: DiffEntry[]): number {
  return entries.reduce((count, entry) => (entry.kind === "line" || entry.kind === "columnLine" ? count + 1 : count), 0);
}

export function getDiffRenderMetadata(diff: DiffModel, actualLineCount: number): DiffRenderMetadata {
  const fields = diff.fields;
  const lineWindow = diff.metadata?.lineWindow ?? getLineWindow(fields);
  const columnWindow =
    diff.metadata?.columnWindow ??
    getColumnWindow(fields);
  const totalDiffLineCount =
    getPositiveNumber(diff.metadata?.totalDiffLineCount) ??
    getPositiveNumber(fields.totalDiffLineCount) ??
    getPositiveNumber(fields.totalDiffLines);
  const totalLineCount =
    getPositiveNumber(diff.metadata?.totalLineCount) ??
    totalDiffLineCount ??
    getPositiveNumber(fields.totalLineCount) ??
    actualLineCount;
  const renderedLineCount =
    getPositiveNumber(diff.metadata?.renderedLineCount) ??
    getPositiveNumber(fields.renderedLineCount) ??
    actualLineCount;
  const hasMoreBefore =
    diff.metadata?.hasMoreBefore ??
    getBooleanField(fields, "hasMoreBefore") ??
    Boolean(lineWindow && lineWindow.startLine > 1);
  const explicitHasMoreAfter =
    diff.metadata?.hasMoreAfter ??
    getBooleanField(fields, "hasMoreAfter");
  const hasMoreAfter =
    explicitHasMoreAfter ??
    getBooleanField(fields, "hasMore") ??
    (lineWindow ? lineWindow.endLine < totalLineCount : totalLineCount > actualLineCount);
  const hasMoreColumns =
    diff.metadata?.hasMoreColumns ??
    getBooleanField(fields, "hasMoreColumns") ??
    getBooleanField(fields, "lineTextTruncated") ??
    Boolean(columnWindow?.hasMoreColumns);

  return {
    totalLineCount,
    totalDiffLineCount,
    renderedLineCount,
    lineWindow,
    columnWindow,
    hasMoreBefore,
    hasMoreAfter,
    hasMoreColumns,
    truncated:
      diff.metadata?.truncated ??
      getBooleanField(fields, "truncated") ??
      getBooleanField(fields, "oldTruncated") ??
      getBooleanField(fields, "newTruncated") ??
      false
  };
}

export function getDiffVirtualization(diff: DiffModel): DiffVirtualizationState {
  const virtualization = getObjectField(diff.fields, "virtualization");
  const hasWrappedRows = diffHasWrappedRows(diff);
  const requestedRowHeight =
    getPositiveNumber(diff.metadata?.virtualization?.rowHeightPx) ??
    getPositiveNumber(virtualization?.rowHeightPx) ??
    (hasWrappedRows ? DEFAULT_WRAPPED_DIFF_ROW_HEIGHT_PX : DEFAULT_DIFF_ROW_HEIGHT_PX);

  return {
    maxRenderedLineCount: clampLineCount(
      getPositiveNumber(diff.metadata?.virtualization?.maxRenderedLineCount) ??
      getPositiveNumber(virtualization?.maxRenderedLineCount) ??
      getPositiveNumber(diff.fields.maxRenderedLineCount) ??
      DEFAULT_DIFF_RENDER_LINE_COUNT
    ),
    overscanLineCount: clampOverscanLineCount(
      getPositiveNumber(diff.metadata?.virtualization?.overscanLineCount) ??
      getPositiveNumber(virtualization?.overscanLineCount) ??
      DEFAULT_DIFF_OVERSCAN_LINE_COUNT
    ),
    rowHeightPx: clampRowHeight(hasWrappedRows ? Math.max(requestedRowHeight, DEFAULT_WRAPPED_DIFF_ROW_HEIGHT_PX) : requestedRowHeight)
  };
}

function diffHasWrappedRows(diff: DiffModel): boolean {
  if (
    diff.metadata?.hasMoreColumns ||
    getBooleanField(diff.fields, "hasMoreColumns") ||
    getBooleanField(diff.fields, "hasLongLines") ||
    getBooleanField(diff.fields, "lineTextTruncated")
  ) {
    return true;
  }

  return diff.hunks.some((hunk) =>
    hunk.lines.some((line) => {
      const renderedText = line.textSegment ?? line.text ?? "";
      return (
        renderedText.length > 140 ||
        line.hasMoreColumns === true ||
        (line.textLength !== undefined && line.textLength > renderedText.length)
      );
    })
  );
}

export function getVirtualRange(
  entryCount: number,
  scrollFrame: DiffScrollFrame,
  virtualization: DiffVirtualizationState
): DiffVirtualRange {
  if (entryCount === 0) {
    return {
      startIndex: 0,
      endIndex: 0,
      maxDomRows: 0,
      offsetY: 0,
      totalHeight: 0
    };
  }

  const visibleRows = Math.ceil(scrollFrame.viewportHeight / virtualization.rowHeightPx);
  const maxDomRows = Math.min(
    virtualization.maxRenderedLineCount,
    Math.max(1, visibleRows + virtualization.overscanLineCount * 2)
  );
  const rawStart = Math.floor(scrollFrame.scrollTop / virtualization.rowHeightPx) - virtualization.overscanLineCount;
  const startIndex = Math.max(0, Math.min(Math.max(0, entryCount - 1), rawStart));
  const endIndex = Math.min(entryCount, startIndex + maxDomRows);

  return {
    startIndex,
    endIndex,
    maxDomRows,
    offsetY: startIndex * virtualization.rowHeightPx,
    totalHeight: entryCount * virtualization.rowHeightPx
  };
}

function getLineWindow(fields: FieldRecord): DiffLineWindow | undefined {
  const value = getObjectField(fields, "lineWindow");
  if (value) {
    const startLine = getPositiveNumber(value.startLine ?? value.start);
    const limit = getPositiveNumber(value.limit);
    const endLine =
      getPositiveNumber(value.endLine ?? value.end) ??
      (startLine !== undefined && limit !== undefined ? startLine + limit - 1 : undefined);

    return startLine !== undefined && endLine !== undefined
      ? { startLine, endLine: Math.max(startLine, endLine) }
      : undefined;
  }

  const windowStart = getNonNegativeNumber(fields.windowStart ?? fields.start);
  const windowEnd = getNonNegativeNumber(fields.windowEnd ?? fields.end);
  if (windowStart === undefined || windowEnd === undefined) {
    return undefined;
  }

  const startLine = windowStart + 1;
  return {
    startLine,
    endLine: Math.max(startLine, windowEnd)
  };
}

function getColumnWindow(fields: FieldRecord): DiffColumnWindow | undefined {
  const value = getObjectField(fields, "columnWindow");
  const source = value ?? fields;
  const columnStart = getNonNegativeNumber(source.columnStart ?? source.startColumn ?? source.start);
  const limit = getPositiveNumber(source.columnLimit ?? source.limit);
  const columnEnd =
    getNonNegativeNumber(source.columnEnd ?? source.endColumn ?? source.end) ??
    (columnStart !== undefined && limit !== undefined ? columnStart + limit : undefined);
  const textLength = getNonNegativeNumber(source.textLength ?? source.totalColumns ?? source.totalLength);
  const hasMoreColumns =
    getBooleanValue(source.hasMoreColumns) ??
    (columnStart !== undefined && columnEnd !== undefined && textLength !== undefined
      ? columnStart > 0 || columnEnd < textLength
      : undefined);

  return columnStart !== undefined && columnEnd !== undefined
    ? {
        columnStart,
        columnEnd: Math.max(columnStart, columnEnd),
        ...(textLength !== undefined ? { textLength } : {}),
        ...(hasMoreColumns !== undefined ? { hasMoreColumns } : {})
      }
    : undefined;
}

export function formatDiffWindowLabel(
  metadata: DiffRenderMetadata,
  visibleEntries: DiffEntry[],
  rowCount: number,
  viewMode: DiffViewMode
): string {
  if (rowCount === 0) {
    return viewMode === "changesOnly"
      ? "No changed lines"
      : metadata.totalLineCount > 0 ? `No rendered lines of ${metadata.totalLineCount}` : "No rendered lines";
  }

  const visibleLineNumbers = visibleEntries
    .map(getEntryDisplayLineNumber)
    .filter((lineNumber): lineNumber is number => lineNumber !== undefined);
  const firstLine = visibleLineNumbers[0] ?? metadata.lineWindow?.startLine ?? 1;
  const lastLine = visibleLineNumbers.at(-1) ?? metadata.lineWindow?.endLine ?? firstLine;
  const total = metadata.totalDiffLineCount ?? metadata.totalLineCount;
  const totalSuffix = total > 0 ? ` of ${total}` : "";
  const columnSuffix = formatColumnWindowLabel(metadata);
  const modeSuffix = metadata.truncated ? " (truncated)" : "";
  const prefix = viewMode === "changesOnly" ? "Changed lines" : "Lines";
  return `${prefix} ${firstLine}-${lastLine}${totalSuffix}${columnSuffix}${modeSuffix}`;
}

function getEntryDisplayLineNumber(entry: DiffEntry): number | undefined {
  if (entry.kind === "line") {
    return entry.row.line.newLine ?? entry.row.line.oldLine;
  }

  if (entry.kind === "columnLine") {
    return (
      entry.row.insertLine?.newLine ??
      entry.row.deleteLine?.oldLine ??
      entry.row.fileLine?.newLine ??
      entry.row.fileLine?.oldLine
    );
  }

  return undefined;
}

export function formatHiddenLineRange(start: number | undefined, end: number | undefined): string {
  if (start === undefined || end === undefined) {
    return "...";
  }

  return start === end ? String(start) : `${start}-${end}`;
}

function formatColumnWindowLabel(metadata: DiffRenderMetadata): string {
  if (!metadata.columnWindow) {
    return metadata.hasMoreColumns ? ", columns clipped" : "";
  }

  const start = metadata.columnWindow.columnStart + 1;
  const end = metadata.columnWindow.columnEnd;
  const totalSuffix = metadata.columnWindow.textLength ? ` of ${metadata.columnWindow.textLength}` : "";
  return `, columns ${start}-${end}${totalSuffix}`;
}

function getHunkKey(hunk: DiffModel["hunks"][number], hunkIndex: number): string {
  return `${hunk.oldStart}-${hunk.newStart}-${hunkIndex}`;
}

export function formatHunkTitle(hunk: DiffModel["hunks"][number]): string {
  return `@@ -${hunk.oldStart},${hunk.oldLines} +${hunk.newStart},${hunk.newLines} @@`;
}

interface RenderableLineSegment {
  text: string;
  hasMoreBefore: boolean;
  hasMoreAfter: boolean;
}

export function getRenderableLineSegment(line: DiffLine): RenderableLineSegment {
  const sourceSegment = line.text ?? line.textSegment ?? "";
  const renderedLength = Array.from(sourceSegment).length;
  const fullTextLength = line.textLength ?? Array.from(line.text ?? sourceSegment).length;
  if (line.text !== undefined && renderedLength >= fullTextLength) {
    return {
      text: sourceSegment,
      hasMoreBefore: false,
      hasMoreAfter: false
    };
  }

  const baseColumnStart = Math.max(0, line.columnStart ?? 0);
  const explicitColumnEnd = line.columnEnd ?? baseColumnStart + renderedLength;
  return {
    text: sourceSegment,
    hasMoreBefore: baseColumnStart > 0,
    hasMoreAfter: explicitColumnEnd < fullTextLength
  };
}

function getStringField(fields: FieldRecord, key: string): string | undefined {
  const value = fields[key];
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function getBooleanField(fields: FieldRecord, key: string): boolean | undefined {
  const value = fields[key];
  return typeof value === "boolean" ? value : undefined;
}

function getObjectField(fields: FieldRecord, key: string): FieldRecord | undefined {
  const value = fields[key];
  return value && typeof value === "object" && !Array.isArray(value) ? (value as FieldRecord) : undefined;
}

function mergeMetadata(
  metadata: LensSurfaceMetadata | null | undefined,
  patch: LensSurfaceMetadata
): LensSurfaceMetadata {
  return {
    ...(metadata ?? {}),
    ...patch
  };
}

function getMetadataRows(metadata?: LensSurfaceMetadata | null, fields: FieldRecord = {}) {
  const mergedFields = {
    ...(metadata?.fields ?? {}),
    ...fields
  };

  const dimensions = metadata?.dimensions ?? getDimensions(mergedFields);
  const rows = [
    { label: "Kind", value: stringifyValue(metadata?.kind ?? mergedFields.kind) },
    { label: "Media type", value: stringifyValue(metadata?.mediaType ?? mergedFields.mediaType) },
    { label: "Size", value: formatBytes(getNumberValue(metadata?.byteLen ?? mergedFields.byteLen ?? mergedFields.size)) },
    { label: "Root tree", value: shortenHash(stringifyValue(mergedFields.rootTreeId)) },
    { label: "File object", value: shortenHash(stringifyValue(mergedFields.fileObjectId)) },
    { label: "Chunks", value: stringifyValue(mergedFields.chunkCount) },
    { label: "Dimensions", value: formatDimensions(dimensions) },
    { label: "Lens", value: stringifyValue(metadata?.lensId ?? mergedFields.lensId) },
    { label: "Hash", value: shortenHash(stringifyValue(metadata?.contentHash ?? mergedFields.contentHash)) }
  ];

  return rows.filter((row): row is { label: string; value: string } => Boolean(row.value));
}

function getDimensions(fields: FieldRecord): Dimensions | undefined {
  const width = getNumberValue(fields.width);
  const height = getNumberValue(fields.height);

  return width && height ? { width, height } : undefined;
}

function getNumberValue(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function getPositiveNumber(value: unknown): number | undefined {
  const numberValue = getNumberValue(value);
  return numberValue !== undefined && numberValue > 0 ? Math.floor(numberValue) : undefined;
}

function getNonNegativeNumber(value: unknown): number | undefined {
  const numberValue = getNumberValue(value);
  return numberValue !== undefined && numberValue >= 0 ? Math.floor(numberValue) : undefined;
}

function getBooleanValue(value: unknown): boolean | undefined {
  return typeof value === "boolean" ? value : undefined;
}

function clampLineCount(value: number): number {
  return Math.min(MAX_DIFF_RENDER_LINE_COUNT, Math.max(MIN_DIFF_RENDER_LINE_COUNT, Math.floor(value)));
}

function clampOverscanLineCount(value: number): number {
  return Math.min(100, Math.max(0, Math.floor(value)));
}

function clampRowHeight(value: number): number {
  return Math.min(MAX_DIFF_ROW_HEIGHT_PX, Math.max(MIN_DIFF_ROW_HEIGHT_PX, Math.floor(value)));
}

function formatBytes(bytes: number | undefined): string | undefined {
  if (bytes === undefined) {
    return undefined;
  }

  if (bytes < 1024) {
    return `${bytes} B`;
  }

  const units = ["KB", "MB", "GB"];
  let size = bytes / 1024;
  let unitIndex = 0;

  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex += 1;
  }

  return `${size.toFixed(size >= 10 ? 0 : 1)} ${units[unitIndex]}`;
}

function formatDimensions(dimensions: Dimensions | undefined): string | undefined {
  return dimensions ? `${dimensions.width} x ${dimensions.height}` : undefined;
}

export function formatLineNumber(value: number | undefined): string {
  return value === undefined ? "" : String(value);
}

export function formatColumnCellLineNumber(line: DiffLine): string {
  return formatLineNumber(line.op === "insert" ? line.newLine : line.op === "delete" ? line.oldLine : line.newLine ?? line.oldLine);
}

function stringifyValue(value: unknown): string | undefined {
  if (value === undefined || value === null || value === "") {
    return undefined;
  }

  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }

  return JSON.stringify(value);
}

function shortenHash(value: string | undefined): string | undefined {
  return value && value.length > 16 ? `${value.slice(0, 16)}...` : value;
}

export function joinClassNames(...classNames: Array<string | undefined>): string | undefined {
  return classNames.filter(Boolean).join(" ") || undefined;
}
