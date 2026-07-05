import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import type {
  AvailableSpaceView,
  DesktopShortcutSettings,
  LensDiffEntry,
  LocalDiffStats,
  LocalLayerSummary,
  LocalSpaceSummary,
  LocalStepSummary,
  WorkingTreeScan
} from "./tauri";
import type { ChangeState, CreateDraft, DesktopPage, FileState, LayerFile, LocalChange, TimelineItem } from "./desktopTypes";

export function buildLayerFiles(scan: WorkingTreeScan | undefined, layer: LocalLayerSummary | null): LayerFile[] {
  const access = layer?.access ?? "open";
  if (access === "blocked" || access === "redacted") {
    return [
      {
        path: layer?.path ?? "layer contents",
        kind: "Document",
        state: "redacted",
        sizeLabel: "restricted",
        redacted: true
      }
    ];
  }

  if (!scan) {
    return [];
  }

  const changed = new Map<string, FileState>();
  for (const path of scan.added) {
    changed.set(path, "added");
  }
  for (const path of scan.modified) {
    changed.set(path, "modified");
  }
  for (const path of scan.deleted) {
    changed.set(path, "deleted");
  }

  const currentFiles = scan.files.map((file) => ({
    path: file.path,
    kind: fileKind(file.path),
    state: changed.get(file.path) ?? "clean",
    lensId: lensForPath(file.path),
    sizeLabel: formatBytes(file.size)
  }));

  const deletedFiles = scan.deleted
    .filter((path) => !scan.files.some((file) => file.path === path))
    .map((path) => ({
      path,
      kind: fileKind(path),
      state: "deleted" as FileState,
      lensId: lensForPath(path),
      sizeLabel: "deleted"
    }));

  return [...currentFiles, ...deletedFiles];
}

export function buildChanges(scan: WorkingTreeScan | undefined, selectedStepId: string | null): LocalChange[] {
  if (!scan) {
    return [];
  }

  const selectedStep = selectedStepId ? (scan.steps ?? []).find((step) => step.stepId === selectedStepId) : undefined;
  const diffs = selectedStep?.diffs ?? scan.diffs;

  return diffs.map((diff) => ({
    path: diff.path,
    state: normalizeChangeState(diff.state),
    summary: diff.diff.summary || diff.title,
    lensId: diff.lensId,
    diff
  }));
}

interface LayerWithStepActivity {
  layer: LocalLayerSummary;
  latestStepAt: number;
  stepCount: number;
}

export function layersByLatestStep(
  space: LocalSpaceSummary,
  scan: WorkingTreeScan | undefined,
  query: string
): LayerWithStepActivity[] {
  const normalizedQuery = query.trim().toLowerCase();
  const activity = new Map<string, { latestStepAt: number; stepCount: number }>();

  for (const layerActivity of scan?.layerActivities ?? []) {
    activity.set(layerActivity.layerId, {
      latestStepAt: layerActivity.latestStepAt,
      stepCount: layerActivity.stepCount
    });
  }

  if (!scan?.layerActivities?.length) {
    for (const step of scan?.steps ?? []) {
      const current = activity.get(step.layerId) ?? { latestStepAt: 0, stepCount: 0 };
      activity.set(step.layerId, {
        latestStepAt: Math.max(current.latestStepAt, step.capturedAt),
        stepCount: current.stepCount + 1
      });
    }
  }

  return space.layers
    .filter((layer) => {
      if (!normalizedQuery) {
        return true;
      }
      return (
        layer.displayName.toLowerCase().includes(normalizedQuery) ||
        layer.layerId.toLowerCase().includes(normalizedQuery)
      );
    })
    .map((layer) => ({
      layer,
      latestStepAt: activity.get(layer.layerId)?.latestStepAt ?? 0,
      stepCount: activity.get(layer.layerId)?.stepCount ?? 0
    }))
    .sort((left, right) => {
      if (left.latestStepAt !== right.latestStepAt) {
        return right.latestStepAt - left.latestStepAt;
      }
      if (left.layer.layerId === space.activeLayerId && right.layer.layerId !== space.activeLayerId) {
        return -1;
      }
      if (right.layer.layerId === space.activeLayerId && left.layer.layerId !== space.activeLayerId) {
        return 1;
      }
      return left.layer.displayName.localeCompare(right.layer.displayName, undefined, { sensitivity: "base" });
    });
}

export function buildTimeline(
  space: LocalSpaceSummary | null,
  scan: WorkingTreeScan | undefined,
  selectedStepId: string | null
): TimelineItem[] {
  if (!space) {
    return [];
  }

  const changedCount = scan ? scan.added.length + scan.modified.length + scan.deleted.length : 0;
  const scanStats = scan
    ? diffStatsFromDiffs(scan.diffs)
    : undefined;
  const base: TimelineItem[] = [
    {
      id: "working-tree",
      kind: "scan",
      title: "Working tree",
      actor: displayPath(space.rootPath),
      at: scan ? "Latest" : "Pending",
      summary: scan ? `${changedCount} local changes before publication.` : "Run Scan to load files and local changes.",
      isActive: selectedStepId === null,
      diffStats: scanStats
    }
  ];

  const steps = (scan?.steps ?? [])
    .slice()
    .reverse()
    .map((step) => timelineItemFromStep(step, selectedStepId, stepLayerOriginLabel(space, step)));

  return [...base, ...steps];
}

function timelineItemFromStep(step: LocalStepSummary, selectedStepId: string | null, layerName: string): TimelineItem {
  return {
    id: step.stepId,
    kind: "step",
    title: `Step ${shortId(step.stepId)}`,
    actor: layerName,
    at: formatUnixTime(step.capturedAt),
    summary: `${step.changedFiles} changed files captured in this Layer step.`,
    isActive: selectedStepId === step.stepId,
    diffStats: step.diffStats
  };
}

function stepLayerOriginLabel(space: LocalSpaceSummary, step: LocalStepSummary): string {
  return step.originLayerName?.trim() || layerDisplayName(space, step.originLayerId ?? step.layerId);
}

function normalizeChangeState(state: string): ChangeState {
  return state === "added" || state === "deleted" ? state : "modified";
}

function diffStatsFromDiffs(diffs: LensDiffEntry[]): LocalDiffStats {
  return diffs.reduce<LocalDiffStats>(
    (stats, diff) => {
      stats.files += 1;
      for (const hunk of diff.diff.hunks) {
        for (const line of hunk.lines) {
          if (line.op === "insert") {
            stats.additions += 1;
          } else if (line.op === "delete") {
            stats.deletions += 1;
          }
        }
      }
      return stats;
    },
    { files: 0, additions: 0, deletions: 0 }
  );
}

export function diffWindowKey(localSpaceId: string | undefined, selectedStepId: string | null, path: string | undefined): string {
  return localSpaceId && path ? `${localSpaceId}::${selectedStepId ?? "workingTree"}::${path}` : "";
}

interface DesktopDiffWindowState {
  canLoadNext: boolean;
  canLoadPrevious: boolean;
  isWindowed: boolean;
  label: string;
  limit: number;
  nextStart: number;
  previousStart: number;
}

export function diffWindowState(entry: LensDiffEntry): DesktopDiffWindowState {
  const fields = entry.diff.fields ?? {};
  const metadata = entry.diff.metadata;
  const metadataWindow = metadata?.lineWindow;
  const windowStart =
    numberField(fields, "windowStart") ??
    numberField(fields, "start") ??
    (metadataWindow?.startLine ? metadataWindow.startLine - 1 : 0);
  const windowLimit =
    numberField(fields, "windowLimit") ??
    numberField(fields, "limit") ??
    metadata?.renderedLineCount ??
    entry.diff.hunks.reduce((count, hunk) => count + hunk.lines.length, 0);
  const windowEnd =
    numberField(fields, "windowEnd") ??
    numberField(fields, "end") ??
    (metadataWindow?.endLine ? metadataWindow.endLine : windowStart + windowLimit);
  const totalLines =
    numberField(fields, "totalDiffLines") ??
    numberField(fields, "totalLineCount") ??
    metadata?.totalLineCount ??
    windowEnd;
  const hasMore =
    booleanField(fields, "hasMore") ??
    metadata?.hasMoreAfter ??
    windowEnd < totalLines;
  const hasMoreBefore =
    booleanField(fields, "hasMoreBefore") ??
    metadata?.hasMoreBefore ??
    windowStart > 0;
  const isWindowed =
    Boolean(booleanField(fields, "largeDiff")) ||
    hasMore ||
    hasMoreBefore ||
    totalLines > windowLimit;
  const safeLimit = Math.max(1, windowLimit || 400);
  const previousStart = Math.max(0, windowStart - safeLimit);
  const nextStart = Math.min(Math.max(0, totalLines - 1), windowStart + safeLimit);
  const displayStart = totalLines === 0 ? 0 : windowStart + 1;
  const displayEnd = Math.min(windowEnd, totalLines);

  return {
    canLoadNext: hasMore && nextStart !== windowStart,
    canLoadPrevious: hasMoreBefore && previousStart !== windowStart,
    isWindowed,
    label: `Diff lines ${displayStart}-${displayEnd} of ${totalLines}`,
    limit: safeLimit,
    nextStart,
    previousStart
  };
}

function numberField(fields: Record<string, unknown>, key: string): number | undefined {
  const value = fields[key];
  if (typeof value === "number" && Number.isFinite(value)) {
    return value;
  }
  if (typeof value === "string") {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : undefined;
  }
  return undefined;
}

function booleanField(fields: Record<string, unknown>, key: string): boolean | undefined {
  const value = fields[key];
  if (typeof value === "boolean") {
    return value;
  }
  if (typeof value === "string") {
    if (value === "true") {
      return true;
    }
    if (value === "false") {
      return false;
    }
  }
  return undefined;
}

export function formatUnixTime(value: number): string {
  if (!Number.isFinite(value) || value <= 0) {
    return "Unknown";
  }
  return new Date(value * 1000).toLocaleString(undefined, {
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit"
  });
}

function shortId(value: string): string {
  return value.length > 14 ? value.slice(0, 14) : value;
}

export function activeLayerLabel(space: LocalSpaceSummary): string {
  const activeLayer = space.layers.find((layer) => layer.layerId === space.activeLayerId);
  return activeLayer?.displayName ?? space.activeLayerId ?? "No active Layer";
}

export function layerDisplayName(space: LocalSpaceSummary, layerId: string): string {
  return space.layers.find((layer) => layer.layerId === layerId)?.displayName ?? shortId(layerId);
}

export function syncStatusLabel(status: string): string {
  if (status === "local-only") {
    return "Local only";
  }
  if (status === "local") {
    return "Local draft";
  }
  if (status === "linked") {
    return "Linked";
  }
  return status;
}

export function activeLayerCaption(space: LocalSpaceSummary): string {
  return space.activeLayerId ? `Layer: ${activeLayerLabel(space)}` : activeLayerLabel(space);
}

export function displayPath(value: string): string {
  return value.replace(/^\\\\\?\\UNC\\/i, "\\\\").replace(/^\\\\\?\\/, "");
}

export function compactPath(value: string, maxLength: number): string {
  const normalized = displayPath(value);
  if (normalized.length <= maxLength) {
    return normalized;
  }

  const separator = normalized.includes("/") && !normalized.includes("\\") ? "/" : "\\";
  const parts = normalized.split(/[\\/]+/).filter(Boolean);
  if (parts.length <= 2) {
    return normalized.length <= maxLength ? normalized : `...${normalized.slice(-(maxLength - 3))}`;
  }

  const prefix = /^[A-Za-z]:/.test(normalized) ? normalized.slice(0, 2) : normalized.startsWith("\\\\") ? "\\\\" : "";
  const tailCount = maxLength < 42 ? 1 : 2;
  const tail = parts.slice(-tailCount).join(separator);
  const compacted = prefix ? `${prefix}${separator}...${separator}${tail}` : `...${separator}${tail}`;

  if (compacted.length <= maxLength) {
    return compacted;
  }
  return `...${normalized.slice(-(maxLength - 3))}`;
}

export function defaultCreateDraft(space: AvailableSpaceView, defaultLocalRoot: string): CreateDraft {
  const root = trimPath(defaultLocalRoot || ".");
  return {
    targetFolder: `${root}\\${slug(space.name)}`,
    layerId: space.currentLayerId ?? ""
  };
}

function fileKind(path: string): LayerFile["kind"] {
  const lower = path.toLowerCase();
  if (/\.(ts|tsx|js|jsx|rs|css|json|toml|yaml|yml)$/.test(lower)) {
    return "Code";
  }
  if (/\.(png|jpg|jpeg|gif|webp|svg)$/.test(lower)) {
    return "Image";
  }
  if (/\.(csv|parquet|db|sqlite)$/.test(lower)) {
    return "Data";
  }
  return "Document";
}

function lensForPath(path: string) {
  const kind = fileKind(path);
  if (kind === "Image") {
    return "layrs.image";
  }
  if (kind === "Code") {
    return "layrs.code";
  }
  return kind === "Document" ? "layrs.text" : "layrs.raw";
}

function formatBytes(size: number) {
  if (size < 1024) {
    return `${size} B`;
  }
  if (size < 1024 * 1024) {
    return `${Math.round(size / 1024)} KB`;
  }
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

export function pageFromHash(hash: string): DesktopPage {
  if (hash.includes("desktop-local")) {
    return "local";
  }
  if (hash.includes("desktop-draft")) {
    return "draft";
  }
  if (hash.includes("desktop-spaceSettings")) {
    return "spaceSettings";
  }
  if (hash.includes("desktop-weaves")) {
    return "weaves";
  }
  if (hash.includes("desktop-settings")) {
    return "settings";
  }
  return "available";
}

export function pageTitle(page: DesktopPage, selectedSpace: LocalSpaceSummary | null) {
  if (page === "local") {
    return selectedSpace?.name ?? "Local Spaces";
  }
  if (page === "settings") {
    return "Settings";
  }
  if (page === "spaceSettings") {
    return selectedSpace ? `${selectedSpace.name} Settings` : "Space Settings";
  }
  if (page === "weaves") {
    return selectedSpace ? `${selectedSpace.name} Weaves` : "Weaves";
  }
  if (page === "draft") {
    return "Local setup";
  }
  return "Distant Spaces";
}

export function isEditableShortcutTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  const tagName = target.tagName.toLowerCase();
  return tagName === "input" || tagName === "textarea" || tagName === "select" || target.isContentEditable;
}

export function shortcutFromKeyboardEvent(event: KeyboardEvent | ReactKeyboardEvent): string {
  const key = event.key;
  if (!key || ["Control", "Shift", "Alt", "Meta"].includes(key)) {
    return "";
  }
  const parts: string[] = [];
  if (event.ctrlKey || event.metaKey) {
    parts.push("Ctrl");
  }
  if (event.shiftKey) {
    parts.push("Shift");
  }
  if (event.altKey) {
    parts.push("Alt");
  }
  parts.push(normalizeShortcutKey(key));
  return parts.join("+");
}

export function normalizeShortcutKey(key: string): string {
  if (key.length === 1) {
    return key.toUpperCase();
  }
  if (key === " ") {
    return "Space";
  }
  return key
    .replace(/^Arrow/, "")
    .replace("Escape", "Esc")
    .replace("Delete", "Del");
}

export function normalizeShortcut(value: string): string {
  return value
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => {
      const lower = part.toLowerCase();
      if (lower === "control" || lower === "cmd" || lower === "meta" || lower === "ctrl") {
        return "Ctrl";
      }
      if (lower === "shift") {
        return "Shift";
      }
      if (lower === "alt" || lower === "option") {
        return "Alt";
      }
      return normalizeShortcutKey(part);
    })
    .join("+");
}

export function shortcutMatches(pressed: string, configured: string): boolean {
  return normalizeShortcut(pressed) === normalizeShortcut(configured);
}

export function validateShortcuts(shortcuts: DesktopShortcutSettings): string | null {
  if (!shortcuts.enabled) {
    return null;
  }
  const saveStep = normalizeShortcut(shortcuts.saveStep);
  const publish = normalizeShortcut(shortcuts.publish);
  if (!saveStep || !publish) {
    return "Shortcut fields cannot be empty while shortcuts are enabled.";
  }
  if (saveStep === publish) {
    return "Save Step and Publish shortcuts must be different.";
  }
  return null;
}

function trimPath(value: string) {
  return value.replace(/[\\/]+$/, "");
}

export function nameFromFolder(value: string) {
  const normalized = trimPath(displayPath(value));
  const parts = normalized.split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] ?? "";
}

function slug(value: string) {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/(^-|-$)/g, "");
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function isConsumedDeviceCodeError(error: unknown): boolean {
  const message = errorMessage(error).toLowerCase();
  return message.includes("already consumed") || message.includes("device code was already consumed");
}
