import { useEffect, useMemo, useState } from "react";
import {
  normalizeArtifactPreviewWindowPayload,
  type Artifact,
  type ArtifactWindowMetadata,
  type GateStatus,
  type Layer,
  type StudioSnapshot
} from "@layrs/client-sdk";
import { LensDiffHost, type LensSurfaceMetadata } from "@layrs/lenses";
import type { DiffModel } from "@layrs/lens-sdk";
import { StatusPill } from "@layrs/ui";
import { EmptyState, PanelTitle, formatTime } from "./common";
import { resolveLensForArtifact, type LensRegistryState } from "./LensFileViewer";

type StepFeedState =
  | { status: "idle" | "loading" }
  | { status: "available"; steps: LayerStepSummary[] }
  | { status: "fallback"; message?: string };

type StepDiffState =
  | { status: "idle" | "loading" }
  | { status: "available"; diff: DiffModel; window?: ArtifactWindowMetadata; source: "step" | "artifact" | "embedded" }
  | { status: "unavailable"; message: string };

interface LayerStepSummary {
  id: string;
  spaceId?: string;
  layerId: string;
  name: string;
  status: GateStatus;
  actor: "human" | "automation";
  startedAt: string;
  completedAt?: string;
  artifactIds: string[];
  proofIds: string[];
  changedPaths: string[];
  files: StepChangedFile[];
  parentStepId?: string;
  originLayerId?: string;
  originLayerName?: string;
  originStepId?: string;
  stepKind?: string;
  baseLayerId?: string;
  baseTreeId?: string;
  rootTreeId?: string;
  sourceClientId?: string;
  syncBatchId?: string;
  diffStats: {
    files: number;
    additions: number;
    removals: number;
  };
}

interface StepChangedFile {
  path: string;
  state: "added" | "modified" | "deleted" | string;
  artifactId?: string;
  lensId?: string;
  title?: string;
  diff?: DiffModel;
  artifact?: Artifact;
}

const STEP_DIFF_WINDOW_LIMIT = 400;

export function LayerStepsPanel({
  artifacts,
  layer,
  lensRegistry,
  refreshKey = 0,
  snapshotSteps,
  spaceId,
  workspaceId
}: {
  artifacts: Artifact[];
  layer?: Layer;
  lensRegistry: LensRegistryState;
  refreshKey?: number;
  snapshotSteps: StudioSnapshot["steps"];
  spaceId: string;
  workspaceId: string;
}) {
  const snapshotLayerSteps = useMemo(
    () => normalizeLayerSteps(snapshotSteps, artifacts, layer?.id, spaceId),
    [artifacts, layer?.id, snapshotSteps, spaceId]
  );
  const feedState = useLayerStepsFeed({ layer, refreshKey, spaceId, workspaceId, artifacts });
  const steps = feedState.status === "available" ? feedState.steps : snapshotLayerSteps;
  const [selectedStepId, setSelectedStepId] = useState<string>();
  const selectedStep = steps.find((step) => step.id === selectedStepId) ?? steps[0];
  const files = useMemo(() => filesForStep(selectedStep, artifacts), [artifacts, selectedStep]);
  const [selectedPath, setSelectedPath] = useState<string>();
  const selectedFile = files.find((file) => file.path === selectedPath) ?? files[0];
  const [windowStart, setWindowStart] = useState(0);
  const diffState = useStepFileDiff({ file: selectedFile, layer, step: selectedStep, windowStart, workspaceId, spaceId });

  useEffect(() => {
    if (selectedStepId && steps.some((step) => step.id === selectedStepId)) {
      return;
    }

    setSelectedStepId(steps[0]?.id);
  }, [selectedStepId, steps]);

  useEffect(() => {
    if (selectedPath && files.some((file) => file.path === selectedPath)) {
      return;
    }

    setSelectedPath(files[0]?.path);
  }, [files, selectedPath]);

  useEffect(() => {
    setWindowStart(0);
  }, [selectedStep?.id, selectedFile?.path]);

  if (!layer) {
    return (
      <section className="studio-panel studio-panel--wide" id="steps">
        <PanelTitle eyebrow="Layer Steps" title="No Layer selected" />
        <EmptyState title="No Layer selected" detail="Choose a Layer to inspect captured steps and changed files." />
      </section>
    );
  }

  return (
    <section className="studio-panel studio-panel--wide" id="steps">
      <PanelTitle eyebrow="Layer Steps" title={`${layer.name} steps`} />
      {feedState.status === "fallback" && feedState.message ? <p className="studio-runtime-note">{feedState.message}</p> : null}
      {steps.length === 0 ? (
        <EmptyState title="No steps" detail="This Layer has no captured steps yet." />
      ) : (
        <div className="studio-steps-layout">
          <div className="studio-step-list" aria-label="Layer steps">
            {steps.map((step) => (
              <button
                aria-pressed={selectedStep?.id === step.id}
                className={selectedStep?.id === step.id ? "studio-step-row is-selected" : "studio-step-row"}
                key={step.id}
                onClick={() => setSelectedStepId(step.id)}
                type="button"
              >
                <span>
                  <strong>{step.name}</strong>
                  <small>{formatTime(step.startedAt)}</small>
                </span>
                <p>{step.changedPaths.length || step.diffStats.files} changed file(s)</p>
                <div>
                  <StatusPill status={step.status} label={step.status === "passing" ? "captured" : undefined} />
                  <em>{step.originLayerName?.trim() || step.actor}</em>
                </div>
              </button>
            ))}
          </div>

          <div className="studio-step-detail">
            <StepSummary step={selectedStep} />
            <ChangedFilesList files={files} selectedPath={selectedFile?.path} onSelect={setSelectedPath} />
            <StepDiffViewer
              diffState={diffState}
              file={selectedFile}
              lensRegistry={lensRegistry}
              onWindowStartChange={setWindowStart}
            />
          </div>
        </div>
      )}
    </section>
  );
}

function StepSummary({ step }: { step?: LayerStepSummary }) {
  if (!step) {
    return null;
  }

  return (
    <div className="studio-step-summary">
      <dl className="studio-diff-stats">
        <div>
          <dt>Files</dt>
          <dd>{step.diffStats.files}</dd>
        </div>
        <div>
          <dt>Added</dt>
          <dd>+{step.diffStats.additions}</dd>
        </div>
        <div>
          <dt>Removed</dt>
          <dd>-{step.diffStats.removals}</dd>
        </div>
        <div>
          <dt>Root tree</dt>
          <dd>{shortId(step.rootTreeId) ?? "unknown"}</dd>
        </div>
      </dl>
    </div>
  );
}

function ChangedFilesList({
  files,
  onSelect,
  selectedPath
}: {
  files: StepChangedFile[];
  onSelect: (path: string) => void;
  selectedPath?: string;
}) {
  if (files.length === 0) {
    return <EmptyState title="No changed files" detail="This step does not report changed paths yet." />;
  }

  return (
    <div className="studio-step-files" aria-label="Changed files">
      {files.map((file) => (
        <button
          className={selectedPath === file.path ? "studio-step-file is-selected" : "studio-step-file"}
          key={file.path}
          onClick={() => onSelect(file.path)}
          type="button"
        >
          <span>{file.state}</span>
          <strong>{file.path}</strong>
          <small>{file.lensId ?? file.artifact?.type ?? "raw"}</small>
        </button>
      ))}
    </div>
  );
}

function StepDiffViewer({
  diffState,
  file,
  lensRegistry,
  onWindowStartChange
}: {
  diffState: StepDiffState;
  file?: StepChangedFile;
  lensRegistry: LensRegistryState;
  onWindowStartChange: (start: number) => void;
}) {
  const window = diffState.status === "available" ? diffState.window : undefined;
  const isLoading = diffState.status === "loading";

  return (
    <div className="studio-step-diff">
      <div className="studio-step-diff__meta">
        <span>{file?.lensId ?? "Lens diff"}</span>
        <strong>{file?.path ?? "No file selected"}</strong>
        {diffState.status === "available" ? <p>{diffState.source === "artifact" ? "Artifact diff fallback" : "Step diff"}</p> : null}
        {diffState.status === "unavailable" ? <p>{diffState.message}</p> : null}
        {window ? (
          <div className="studio-file-viewer__window-controls">
            <small>
              {window.totalLines === undefined
                ? `Lines ${window.start + 1}-${window.start + window.count}`
                : `Lines ${window.start + 1}-${window.start + window.count} of ${window.totalLines}`}
            </small>
            <div>
              <button disabled={isLoading || window.start === 0} onClick={() => onWindowStartChange(Math.max(0, window.start - window.limit))} type="button">
                Previous
              </button>
              <button disabled={isLoading || !window.hasMore} onClick={() => onWindowStartChange(window.start + window.limit)} type="button">
                Next
              </button>
            </div>
          </div>
        ) : null}
      </div>
      <div className="studio-file-preview studio-file-preview--code">
        {isLoading ? (
          <div className="studio-file-preview__placeholder">
            <strong>{file?.path ?? "Diff"}</strong>
            <span>Loading diff...</span>
          </div>
        ) : (
          <LensDiffHost
            diff={diffState.status === "available" ? diffState.diff : null}
            emptyMessage="Select a changed file to inspect its Lens diff."
            metadata={metadataForFile(file, lensRegistry)}
            title={file?.path ?? "Lens diff"}
          />
        )}
      </div>
    </div>
  );
}

function useLayerStepsFeed({
  artifacts,
  layer,
  refreshKey,
  spaceId,
  workspaceId
}: {
  artifacts: Artifact[];
  layer?: Layer;
  refreshKey: number;
  spaceId: string;
  workspaceId: string;
}): StepFeedState {
  const [state, setState] = useState<StepFeedState>({ status: "idle" });

  useEffect(() => {
    if (!layer || isMockMode()) {
      setState({ status: "idle" });
      return;
    }

    const controller = new AbortController();
    const baseUrl = runtimeApiBaseUrl();
    const path = `/v1/workspaces/${encodeURIComponent(workspaceId)}/spaces/${encodeURIComponent(spaceId)}/layers/${encodeURIComponent(layer.id)}/steps`;

    setState({ status: "loading" });
    void fetchJson(`${baseUrl}${path}`, controller.signal)
      .then((payload) => {
        if (!controller.signal.aborted) {
          setState({ status: "available", steps: normalizeLayerSteps(payload, artifacts, layer.id, spaceId) });
        }
      })
      .catch((error) => {
        if (!controller.signal.aborted) {
          setState({
            status: "fallback",
            message: `Layer steps endpoint unavailable. Studio is using snapshot steps. ${optionalStatus(error)}`
          });
        }
      });

    return () => controller.abort();
  }, [artifacts, layer, refreshKey, spaceId, workspaceId]);

  return state;
}

function useStepFileDiff({
  file,
  layer,
  spaceId,
  step,
  windowStart,
  workspaceId
}: {
  file?: StepChangedFile;
  layer?: Layer;
  spaceId: string;
  step?: LayerStepSummary;
  windowStart: number;
  workspaceId: string;
}): StepDiffState {
  const [state, setState] = useState<StepDiffState>({ status: "idle" });

  useEffect(() => {
    if (!file || !layer || !step) {
      setState({ status: "idle" });
      return;
    }

    if (file.diff && windowStart === 0) {
      setState({ status: "available", diff: file.diff, source: "embedded" });
      return;
    }

    const controller = new AbortController();
    setState({ status: "loading" });

    void loadStepFileDiff({ file, layer, spaceId, step, windowStart, workspaceId, signal: controller.signal })
      .then((next) => {
        if (!controller.signal.aborted) {
          setState(next);
        }
      })
      .catch((error) => {
        if (!controller.signal.aborted) {
          setState({
            status: "unavailable",
            message: `Diff is unavailable for this step file. ${optionalStatus(error)}`
          });
        }
      });

    return () => controller.abort();
  }, [file, layer, spaceId, step, windowStart, workspaceId]);

  return state;
}

async function loadStepFileDiff({
  file,
  layer,
  signal,
  spaceId,
  step,
  windowStart,
  workspaceId
}: {
  file: StepChangedFile;
  layer: Layer;
  signal: AbortSignal;
  spaceId: string;
  step: LayerStepSummary;
  windowStart: number;
  workspaceId: string;
}): Promise<Extract<StepDiffState, { status: "available" }>> {
  const baseUrl = runtimeApiBaseUrl();
  const windowQuery = new URLSearchParams({
    path: file.path,
    start: String(windowStart),
    limit: String(STEP_DIFF_WINDOW_LIMIT)
  });
  const stepDiffPath = [
    "/v1/workspaces",
    encodeURIComponent(workspaceId),
    "spaces",
    encodeURIComponent(spaceId),
    "layers",
    encodeURIComponent(layer.id),
    "steps",
    encodeURIComponent(step.id),
    "diff"
  ].join("/");

  const stepPayload = await fetchJson(`${baseUrl}${stepDiffPath}?${windowQuery}`, signal).catch(() => undefined);
  const stepDiff = stepPayload ? diffFromWindowPayload(stepPayload) : undefined;
  if (stepDiff) {
    return { status: "available", ...stepDiff, source: "step" };
  }

  if (!file.artifact) {
    throw new Error("No linked artifact for changed path.");
  }

  const artifactQuery = new URLSearchParams({
    start: String(windowStart),
    limit: String(STEP_DIFF_WINDOW_LIMIT)
  });
  if (step.baseLayerId) {
    artifactQuery.set("baseLayerId", step.baseLayerId);
  }
  const artifactPath = [
    "/v1/workspaces",
    encodeURIComponent(workspaceId),
    "spaces",
    encodeURIComponent(spaceId),
    "layers",
    encodeURIComponent(layer.id),
    "artifacts",
    encodeURIComponent(file.artifact.id),
    "diff"
  ].join("/");
  const artifactPayload = await fetchJson(`${baseUrl}${artifactPath}?${artifactQuery}`, signal);
  const artifactDiff = diffFromWindowPayload(artifactPayload);
  if (!artifactDiff) {
    throw new Error("Diff payload did not include a Lens diff.");
  }

  return { status: "available", ...artifactDiff, source: "artifact" };
}

function normalizeLayerSteps(payload: unknown, artifacts: Artifact[], layerId?: string, spaceId?: string): LayerStepSummary[] {
  const items = arrayPayload(payload, "items") ?? arrayPayload(payload, "steps") ?? (Array.isArray(payload) ? payload : []);
  const artifactById = new Map(artifacts.map((artifact) => [artifact.id, artifact]));

  return items
    .map((item) => stepFromPayload(item, artifactById, spaceId))
    .filter((step): step is LayerStepSummary => Boolean(step))
    .filter((step) => (!layerId || step.layerId === layerId) && (!spaceId || !step.spaceId || step.spaceId === spaceId))
    .sort((a, b) => new Date(b.startedAt).getTime() - new Date(a.startedAt).getTime());
}

function stepFromPayload(
  value: unknown,
  artifactById: Map<string, Artifact>,
  fallbackSpaceId?: string
): LayerStepSummary | undefined {
  const record = objectPayload(value);
  if (!record) {
    return undefined;
  }

  const id = stringPayload(record.id) ?? stringPayload(record.stepId) ?? stringPayload(record.step_id);
  const layerId = stringPayload(record.layerId) ?? stringPayload(record.layer_id);
  if (!id || !layerId) {
    return undefined;
  }

  const artifactIds = stringArray(record.artifactIds ?? record.artifact_ids);
  const files = filesFromPayload(record, artifactById, artifactIds);
  const changedPaths =
    stringArray(record.changedPaths ?? record.changed_paths).length > 0
      ? stringArray(record.changedPaths ?? record.changed_paths)
      : files.map((file) => file.path);
  const startedAt =
    stringPayload(record.startedAt) ??
    stringPayload(record.started_at) ??
    stringPayload(record.capturedAt) ??
    stringPayload(record.captured_at) ??
    isoFromUnix(record.capturedAtUnix ?? record.captured_at_unix) ??
    new Date(0).toISOString();
  const diffStats = diffStatsFromPayload(record.diffStats ?? record.diff_stats, files, changedPaths);

  return {
    id,
    spaceId: stringPayload(record.spaceId) ?? stringPayload(record.space_id) ?? fallbackSpaceId,
    layerId,
    name: stringPayload(record.name) ?? `Step ${shortId(id) ?? id}`,
    status: gateStatusPayload(record.status) ?? "passing",
    actor: record.actor === "automation" ? "automation" : "human",
    startedAt,
    completedAt: stringPayload(record.completedAt) ?? stringPayload(record.completed_at),
    artifactIds,
    proofIds: stringArray(record.proofIds ?? record.proof_ids),
    changedPaths,
    files,
    parentStepId: stringPayload(record.parentStepId) ?? stringPayload(record.parent_step_id),
    originLayerId: stringPayload(record.originLayerId) ?? stringPayload(record.origin_layer_id),
    originLayerName: stringPayload(record.originLayerName) ?? stringPayload(record.origin_layer_name),
    originStepId: stringPayload(record.originStepId) ?? stringPayload(record.origin_step_id),
    stepKind: stringPayload(record.stepKind) ?? stringPayload(record.step_kind),
    baseLayerId: stringPayload(record.baseLayerId) ?? stringPayload(record.base_layer_id),
    baseTreeId: stringPayload(record.baseTreeId) ?? stringPayload(record.base_tree_id),
    rootTreeId: stringPayload(record.rootTreeId) ?? stringPayload(record.root_tree_id),
    sourceClientId: stringPayload(record.sourceClientId) ?? stringPayload(record.source_client_id),
    syncBatchId: stringPayload(record.syncBatchId) ?? stringPayload(record.sync_batch_id),
    diffStats
  };
}

function filesFromPayload(record: Record<string, unknown>, artifactById: Map<string, Artifact>, artifactIds: string[]): StepChangedFile[] {
  const rawFiles = arrayPayload(record, "files") ?? arrayPayload(record, "changedFiles") ?? arrayPayload(record, "diffs");
  if (rawFiles && rawFiles.length > 0) {
    return rawFiles.map((item) => fileFromPayload(item, artifactById)).filter((file): file is StepChangedFile => Boolean(file));
  }

  const changedPaths = stringArray(record.changedPaths ?? record.changed_paths);
  if (changedPaths.length > 0) {
    return changedPaths.map((path) => enrichFileWithArtifact({ path, state: "modified" }, artifactById));
  }

  return artifactIds
    .map((artifactId) => artifactById.get(artifactId))
    .filter((artifact): artifact is Artifact => Boolean(artifact))
    .map((artifact) => ({
      path: artifact.location,
      state: "modified",
      artifactId: artifact.id,
      artifact
    }));
}

function fileFromPayload(value: unknown, artifactById: Map<string, Artifact>): StepChangedFile | undefined {
  const record = objectPayload(value);
  if (!record) {
    if (typeof value === "string") {
      return enrichFileWithArtifact({ path: value, state: "modified" }, artifactById);
    }
    return undefined;
  }

  const path =
    stringPayload(record.path) ??
    stringPayload(record.logicalPath) ??
    stringPayload(record.logical_path) ??
    stringPayload(record.location);
  if (!path) {
    return undefined;
  }

  return enrichFileWithArtifact(
    {
      path,
      state:
        stringPayload(record.state) ??
        stringPayload(record.action) ??
        stringPayload(record.changeState) ??
        stringPayload(record.change_state) ??
        "modified",
      artifactId: stringPayload(record.artifactId) ?? stringPayload(record.artifact_id),
      lensId: stringPayload(record.lensId) ?? stringPayload(record.lens_id),
      title: stringPayload(record.title),
      diff: diffModelPayload(record.diff)
    },
    artifactById
  );
}

function enrichFileWithArtifact(file: StepChangedFile, artifactById: Map<string, Artifact>): StepChangedFile {
  const artifact =
    (file.artifactId ? artifactById.get(file.artifactId) : undefined) ??
    [...artifactById.values()].find((candidate) => candidate.location === file.path);

  return {
    ...file,
    artifact,
    artifactId: file.artifactId ?? artifact?.id
  };
}

function filesForStep(step: LayerStepSummary | undefined, artifacts: Artifact[]): StepChangedFile[] {
  if (!step) {
    return [];
  }

  const artifactById = new Map(artifacts.map((artifact) => [artifact.id, artifact]));
  if (step.files.length > 0) {
    return step.files.map((file) => enrichFileWithArtifact(file, artifactById));
  }

  return step.changedPaths.map((path) => enrichFileWithArtifact({ path, state: "modified" }, artifactById));
}

function diffFromWindowPayload(payload: unknown): { diff: DiffModel; window?: ArtifactWindowMetadata } | undefined {
  const windowed = normalizeArtifactPreviewWindowPayload(payload);
  if (windowed?.diff) {
    return { diff: windowed.diff, window: windowed.window };
  }

  const record = objectPayload(payload);
  const diff = diffModelPayload(record?.diff ?? payload);
  return diff ? { diff } : undefined;
}

function diffModelPayload(value: unknown): DiffModel | undefined {
  const record = objectPayload(value);
  if (!record) {
    return undefined;
  }

  const kind = stringPayload(record.kind);
  const summary = stringPayload(record.summary);
  if (!kind || !summary) {
    return undefined;
  }

  return {
    kind: kind as DiffModel["kind"],
    summary,
    hunks: (arrayPayload(record, "hunks") ?? []).map(diffHunkPayload).filter((hunk): hunk is DiffModel["hunks"][number] => Boolean(hunk)),
    metadata: objectPayload(record.metadata) as DiffModel["metadata"],
    fields: objectPayload(record.fields) ?? {}
  };
}

function diffHunkPayload(value: unknown): DiffModel["hunks"][number] | undefined {
  const record = objectPayload(value);
  if (!record) {
    return undefined;
  }

  const oldStart = numberPayload(record.oldStart ?? record.old_start);
  const oldLines = numberPayload(record.oldLines ?? record.old_lines);
  const newStart = numberPayload(record.newStart ?? record.new_start);
  const newLines = numberPayload(record.newLines ?? record.new_lines);
  if (oldStart === undefined || oldLines === undefined || newStart === undefined || newLines === undefined) {
    return undefined;
  }

  return {
    oldStart,
    oldLines,
    newStart,
    newLines,
    lines: (arrayPayload(record, "lines") ?? []).map(diffLinePayload).filter((line): line is DiffModel["hunks"][number]["lines"][number] => Boolean(line))
  };
}

function diffLinePayload(value: unknown): DiffModel["hunks"][number]["lines"][number] | undefined {
  const record = objectPayload(value);
  if (!record) {
    return undefined;
  }

  const op = stringPayload(record.op);
  const text = typeof record.text === "string" ? record.text : undefined;
  if ((op !== "equal" && op !== "insert" && op !== "delete") || text === undefined) {
    return undefined;
  }

  return {
    op,
    oldLine: numberPayload(record.oldLine ?? record.old_line),
    newLine: numberPayload(record.newLine ?? record.new_line),
    text
  };
}

function metadataForFile(file: StepChangedFile | undefined, lensRegistry: LensRegistryState): LensSurfaceMetadata | undefined {
  if (!file?.artifact) {
    return file
      ? {
          artifactId: file.artifactId ?? file.path,
          lensId: file.lensId ?? "layrs.raw",
          kind: "raw",
          mediaType: "application/octet-stream",
          byteLen: 0,
          contentHash: "",
          fields: { path: file.path, state: file.state }
        }
      : undefined;
  }

  const lens = resolveLensForArtifact(file.artifact, lensRegistry.manifests);
  return {
    artifactId: file.artifact.id,
    lensId: lens?.id ?? file.lensId ?? "layrs.raw",
    kind: file.artifact.type,
    mediaType: mediaTypeForArtifact(file.artifact),
    byteLen: file.artifact.byteLen ?? 0,
    contentHash: file.artifact.contentHash ?? "",
    fields: {
      location: file.artifact.location,
      state: file.state,
      sizeLabel: file.artifact.sizeLabel
    }
  };
}

function mediaTypeForArtifact(artifact: Artifact): string {
  const hintedMediaType = stringPayload((artifact as unknown as Record<string, unknown>).mediaType);
  if (hintedMediaType) {
    return hintedMediaType;
  }

  if (artifact.type === "image") {
    return "image/*";
  }
  if (artifact.type === "file" || artifact.type === "step-output") {
    return "text/plain";
  }
  return "application/octet-stream";
}

function diffStatsFromPayload(value: unknown, files: StepChangedFile[], changedPaths: string[]) {
  const record = objectPayload(value);
  if (record) {
    return {
      files: numberPayload(record.files) ?? Math.max(files.length, changedPaths.length),
      additions: numberPayload(record.additions) ?? 0,
      removals: numberPayload(record.removals ?? record.deletions) ?? 0
    };
  }

  return files.reduce(
    (stats, file) => {
      if (file.diff) {
        for (const hunk of file.diff.hunks) {
          for (const line of hunk.lines) {
            if (line.op === "insert") {
              stats.additions += 1;
            } else if (line.op === "delete") {
              stats.removals += 1;
            }
          }
        }
      }
      return stats;
    },
    { files: Math.max(files.length, changedPaths.length), additions: 0, removals: 0 }
  );
}

function objectPayload(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
}

function arrayPayload(value: unknown, key: string): unknown[] | undefined {
  const record = objectPayload(value);
  const field = record?.[key];
  return Array.isArray(field) ? field : undefined;
}

function stringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string" && item.length > 0) : [];
}

function stringPayload(value: unknown): string | undefined {
  return typeof value === "string" && value.trim().length > 0 ? value : undefined;
}

function numberPayload(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function gateStatusPayload(value: unknown): GateStatus | undefined {
  return value === "passing" || value === "blocked" || value === "needs-proof" || value === "pending" ? value : undefined;
}

function isoFromUnix(value: unknown): string | undefined {
  const numberValue = numberPayload(value);
  return numberValue === undefined ? undefined : new Date(numberValue * 1000).toISOString();
}

function shortId(value: string | undefined): string | undefined {
  return value && value.length > 14 ? `${value.slice(0, 10)}...` : value;
}

function runtimeEnv(): Record<string, string | undefined> {
  return (import.meta as unknown as { env?: Record<string, string | undefined> }).env ?? {};
}

function runtimeApiBaseUrl(): string {
  return (runtimeEnv().VITE_LAYRS_API_URL ?? "").replace(/\/$/, "");
}

function isMockMode(): boolean {
  const env = runtimeEnv();
  return env.VITE_LAYRS_STUDIO_MODE === "mock" || env.VITE_LAYRS_API_MOCK === "true";
}

async function fetchJson(url: string, signal: AbortSignal): Promise<unknown> {
  const response = await fetch(url, { credentials: "include", signal });
  const payload = await readOptionalJson(response);

  if (!response.ok) {
    throw new Error(errorMessageFromPayload(payload) ?? response.statusText);
  }

  return payload;
}

async function readOptionalJson(response: Response): Promise<unknown> {
  if (response.status === 204) {
    return undefined;
  }

  const text = await response.text();
  if (!text) {
    return undefined;
  }

  try {
    return JSON.parse(text) as unknown;
  } catch {
    return undefined;
  }
}

function errorMessageFromPayload(payload: unknown): string | undefined {
  const record = objectPayload(payload);
  const error = objectPayload(record?.error);
  return stringPayload(record?.message) ?? stringPayload(error?.message) ?? stringPayload(record?.code) ?? stringPayload(error?.code);
}

function optionalStatus(error: unknown): string {
  return error instanceof Error && error.message ? `(${error.message})` : "";
}
