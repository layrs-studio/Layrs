import { normalizeDiffColumnWindow } from "./lenses/diff";
import type {
  DiffColumnWindow,
  DiffLineWindow,
  DiffModel,
  DiffModelMetadata,
  PreviewKind,
  PreviewModel
} from "./lenses/contracts";
import {
  layerAccessPolicyFromWire,
  layerAccessPolicyToLegacyRegistry,
  legacyRegistryToLayerAccessPolicy,
  type LayerAccessPolicyWire
} from "./access";
import type {
  Artifact,
  ArtifactAccessDecision,
  ArtifactContentPayload,
  ArtifactPreviewWindowPayload,
  ArtifactType,
  ChunkMetadata,
  FileObjectMetadata,
  Layer,
  LayerAccessPolicy,
  LayerHeadMetadata,
  LayerStep,
  LayerStepDiffStats,
  LayrsId,
  StepChangedFile,
  StepDiffWindow,
  StudioFixture,
  StudioSnapshot,
  TeamMember,
  WorkspaceMember,
  Invitation
} from "./types";

export type StudioSnapshotWire = Partial<StudioFixture> & {
  layers?: Array<Layer | LayerWire>;
  artifacts?: Array<Artifact | ArtifactWire>;
  layer_access_policies?: LayerAccessPolicyWire[];
  layerAccessPolicies?: Array<LayerAccessPolicy | LayerAccessPolicyWire>;
  workspace_members?: WorkspaceMember[];
  workspaceMembers?: WorkspaceMember[];
  team_members?: TeamMember[];
  teamMembers?: TeamMember[];
  invitations?: Invitation[];
};

type LayerWire = Partial<Layer> & {
  space_id?: LayrsId;
  parent_id?: LayrsId;
  artifact_ids?: LayrsId[];
  step_ids?: LayrsId[];
  gate_ids?: LayrsId[];
  root_tree_id?: LayrsId;
  policy_epoch?: number;
  server_cursor?: string;
  layer_head?: LayerHeadWire;
  layerHead?: LayerHeadWire;
  head?: LayerHeadWire;
};

type LayerHeadWire = Partial<LayerHeadMetadata> & {
  layer_state_id?: LayrsId;
  layerStateId?: LayrsId;
  root_tree_id?: LayrsId;
  rootTreeId?: LayrsId;
  policy_epoch?: number;
  policyEpoch?: number;
  server_cursor?: string;
  serverCursor?: string;
  updated_at?: string;
  updatedAt?: string;
  updated_by_account_id?: LayrsId;
  updatedByAccountId?: LayrsId;
};

type ArtifactWire = Partial<Artifact> & {
  artifact_id?: LayrsId;
  space_id?: LayrsId;
  layer_id?: LayrsId;
  artifact_kind?: ArtifactType | string;
  logical_path?: string;
  path?: string;
  updated_at?: string;
  size_label?: string;
  size_bytes?: number;
  sizeBytes?: number;
  proof_ids?: LayrsId[];
  media_type?: string;
  content_hash?: string;
  sha256?: string;
  byte_len?: number;
  byteLen?: number;
  lens_id?: string;
  root_tree_id?: LayrsId;
  current_tree_id?: LayrsId;
  current_file_object_id?: LayrsId;
  file_object_id?: LayrsId;
  fileObjectId?: LayrsId;
  file_object?: FileObjectWire;
  fileObject?: FileObjectWire;
  chunks?: ChunkWire[];
};

type FileObjectWire = Partial<FileObjectMetadata> & {
  file_object_id?: LayrsId;
  fileObjectId?: LayrsId;
  size_bytes?: number;
  sizeBytes?: number;
  media_type?: string;
  mediaType?: string;
  chunk_count?: number;
  chunkCount?: number;
  chunks?: ChunkWire[];
};

type ChunkWire = Partial<ChunkMetadata> & {
  chunk_id?: LayrsId;
  chunkId?: LayrsId;
  size_bytes?: number;
  sizeBytes?: number;
  byte_offset?: number;
  byteOffset?: number;
  chunk_index?: number;
  chunkIndex?: number;
  media_type?: string;
  mediaType?: string;
  object_key?: string;
  objectKey?: string;
  content?: unknown;
  data?: unknown;
};

export function normalizeStudioSnapshot(snapshot: StudioSnapshotWire): StudioSnapshot {
  const accessRegistries = snapshot.accessRegistries ?? [];
  const wirePolicies = snapshot.layerAccessPolicies ?? snapshot.layer_access_policies;
  const layerAccessPolicies =
    wirePolicies && wirePolicies.length > 0
      ? wirePolicies.map((policy) => layerAccessPolicyFromWire(policy))
      : accessRegistries.map(legacyRegistryToLayerAccessPolicy);

  return {
    ...(snapshot as StudioFixture),
    layers: (snapshot.layers ?? []).map(layerFromWire),
    artifacts: (snapshot.artifacts ?? []).map(artifactFromWire),
    layerAccessPolicies,
    accessRegistries: accessRegistries.length > 0 ? accessRegistries : layerAccessPolicies.map(layerAccessPolicyToLegacyRegistry),
    workspaceMembers: snapshot.workspaceMembers ?? snapshot.workspace_members ?? [],
    teamMembers: snapshot.teamMembers ?? snapshot.team_members ?? [],
    invitations: snapshot.invitations ?? []
  };
}

export function normalizeArtifactCollection(payload: unknown): Artifact[] {
  const record = recordValue(payload);
  const items =
    (Array.isArray(payload) ? payload : undefined) ??
    arrayValue(record?.items) ??
    arrayValue(record?.artifacts) ??
    [];

  return items.map((item) => artifactFromWire(item as ArtifactWire));
}

export function normalizeArtifactContentPayload(payload: unknown): ArtifactContentPayload | undefined {
  const record = recordValue(payload);
  if (!record) {
    return undefined;
  }

  const contentRecord = recordValue(record.content) ?? record;
  const fileObject = fileObjectFromWire(
    firstRecord(contentRecord.fileObject, contentRecord.file_object, record.fileObject, record.file_object) as
      | FileObjectWire
      | undefined
  );
  const mediaType =
    stringFrom(contentRecord.mediaType) ??
    stringFrom(contentRecord.media_type) ??
    stringFrom(record.mediaType) ??
    stringFrom(record.media_type) ??
    fileObject?.mediaType ??
    "application/octet-stream";
  const encoding = normalizeContentEncoding(
    stringFrom(contentRecord.encoding) ??
      stringFrom(record.encoding) ??
      stringFrom(contentRecord.contentEncoding) ??
      stringFrom(contentRecord.content_encoding)
  );
  const chunks = normalizeChunks([
    ...arrayValue(contentRecord.chunks),
    ...arrayValue(record.chunks),
    ...(fileObject?.chunks ?? [])
  ]);
  const sourceValue = contentSourceValue(
    contentRecord.value ?? contentRecord.content ?? contentRecord.body ?? contentRecord.data,
    chunks
  );
  const decoded = decodeContentValue(sourceValue, encoding, mediaType);
  const sha256 =
    stringFrom(contentRecord.sha256) ??
    stringFrom(contentRecord.contentHash) ??
    stringFrom(contentRecord.content_hash) ??
    stringFrom(record.sha256) ??
    stringFrom(record.contentHash) ??
    stringFrom(record.content_hash) ??
    fileObject?.sha256;
  const source = recordValue(record.source);
  const fields = compactRecord({
    encoding,
    contentHash: sha256,
    sha256,
    byteLen: decoded.bytes?.byteLength,
    base64: decoded.base64,
    data: decoded.base64,
    dataUrl: decoded.dataUrl,
    storage: stringFrom(contentRecord.storage) ?? stringFrom(record.storage),
    fileObjectId: fileObject?.fileObjectId,
    fileObjectSha256: fileObject?.sha256,
    fileObjectSizeBytes: fileObject?.sizeBytes,
    chunkCount: fileObject?.chunkCount ?? chunks.length,
    chunks: chunks.map((chunk) => ({
      chunkId: chunk.chunkId,
      sha256: chunk.sha256,
      sizeBytes: chunk.sizeBytes,
      byteOffset: chunk.byteOffset
    })),
    source
  });

  return {
    artifactId: stringFrom(record.artifactId) ?? stringFrom(record.artifact_id),
    workspaceId: stringFrom(record.workspaceId) ?? stringFrom(record.workspace_id),
    spaceId: stringFrom(record.spaceId) ?? stringFrom(record.space_id),
    layerId: stringFrom(record.layerId) ?? stringFrom(record.layer_id),
    path: stringFrom(record.path) ?? stringFrom(record.logicalPath) ?? stringFrom(record.logical_path),
    type: artifactTypeFromWire(record.type ?? record.artifact_kind),
    content: {
      encoding,
      mediaType,
      sha256,
      value: decoded.value,
      bytes: decoded.bytes,
      base64: decoded.base64,
      dataUrl: decoded.dataUrl,
      fileObject,
      chunks,
      storage: stringFrom(contentRecord.storage) ?? stringFrom(record.storage)
    },
    source,
    fields
  };
}

export function normalizeArtifactPreviewWindowPayload(payload: unknown): ArtifactPreviewWindowPayload | undefined {
  const record = recordValue(payload);
  if (!record) {
    return undefined;
  }

  const windowRecord = recordValue(record.window);
  const start = numberFrom(windowRecord?.start);
  const limit = numberFrom(windowRecord?.limit);
  const count = numberFrom(windowRecord?.count);
  if (start === undefined || limit === undefined || count === undefined) {
    return undefined;
  }

  const preview = previewModelFromWire(record.preview);
  const diff = diffModelFromWire(record.diff);
  const source = recordValue(record.source);
  const columnWindowRecord = recordValue(record.columnWindow ?? record.column_window) ?? recordValue(diff?.fields.columnWindow);
  const columnStart = numberFrom(columnWindowRecord?.columnStart ?? columnWindowRecord?.column_start);
  const columnLimit = numberFrom(columnWindowRecord?.columnLimit ?? columnWindowRecord?.column_limit);
  const hasLongLines = Boolean(columnWindowRecord?.hasLongLines ?? columnWindowRecord?.has_long_lines ?? diff?.fields.hasLongLines);
  const fields = compactRecord({
    ...(recordValue(record.fields) ?? {}),
    window: {
      start,
      limit,
      count,
      totalLines: numberFrom(windowRecord?.totalLines ?? windowRecord?.total_lines),
      hasMore: Boolean(windowRecord?.hasMore ?? windowRecord?.has_more),
      hasMoreBefore: Boolean(windowRecord?.hasMoreBefore ?? windowRecord?.has_more_before),
      hasMoreAfter: Boolean(windowRecord?.hasMoreAfter ?? windowRecord?.has_more_after),
      columnStart,
      columnLimit,
      hasLongLines
    },
    source
  });

  return {
    artifactId: stringFrom(record.artifactId) ?? stringFrom(record.artifact_id),
    workspaceId: stringFrom(record.workspaceId) ?? stringFrom(record.workspace_id),
    spaceId: stringFrom(record.spaceId) ?? stringFrom(record.space_id),
    layerId: stringFrom(record.layerId) ?? stringFrom(record.layer_id),
    baseLayerId: stringFrom(record.baseLayerId) ?? stringFrom(record.base_layer_id),
    path: stringFrom(record.path) ?? stringFrom(record.logicalPath) ?? stringFrom(record.logical_path),
    type: artifactTypeFromWire(record.type ?? record.artifact_kind),
    preview,
    diff,
    window: {
      start,
      limit,
      count,
      totalLines: numberFrom(windowRecord?.totalLines ?? windowRecord?.total_lines),
      hasMore: Boolean(windowRecord?.hasMore ?? windowRecord?.has_more),
      hasMoreBefore: Boolean(windowRecord?.hasMoreBefore ?? windowRecord?.has_more_before),
      hasMoreAfter: Boolean(windowRecord?.hasMoreAfter ?? windowRecord?.has_more_after),
      columnStart,
      columnLimit,
      hasLongLines
    },
    source,
    fields
  };
}

export function normalizeLayerStep(payload: unknown): LayerStep | undefined {
  const record = recordValue(payload);
  if (!record) {
    return undefined;
  }

  const stepId = stringFrom(record.stepId) ?? stringFrom(record.step_id) ?? stringFrom(record.id);
  const layerId = stringFrom(record.layerId) ?? stringFrom(record.layer_id);
  if (!stepId || !layerId) {
    return undefined;
  }

  const diffs = normalizeStepDiffWindows(record.diffs);
  const files = normalizeStepChangedFiles(record.files);
  const diffStats = layerStepDiffStatsFromWire(record.diffStats ?? record.diff_stats, diffs);
  const fields = compactRecord({
    ...(recordValue(record.fields) ?? {}),
    source: recordValue(record.source)
  });

  return {
    id: stringFrom(record.id) ?? stepId,
    stepId,
    layerId,
    baseLayerId: stringFrom(record.baseLayerId) ?? stringFrom(record.base_layer_id),
    baseTreeId: stringFrom(record.baseTreeId) ?? stringFrom(record.base_tree_id),
    rootTreeId: stringFrom(record.rootTreeId) ?? stringFrom(record.root_tree_id),
    capturedAt: numberLikeFrom(record.capturedAt ?? record.captured_at),
    startedAt: stringFrom(record.startedAt) ?? stringFrom(record.started_at),
    completedAt: stringFrom(record.completedAt) ?? stringFrom(record.completed_at),
    changedFiles: numberLikeFrom(record.changedFiles ?? record.changed_files) ?? files.length ?? diffs.length,
    diffStats,
    files,
    diffs,
    fields
  };
}

export function normalizeLayerSteps(payload: unknown): LayerStep[] {
  const items = arrayValue(recordValue(payload)?.items) ?? arrayValue(payload);
  return items
    .map(normalizeLayerStep)
    .filter((step): step is LayerStep => Boolean(step));
}

export function normalizeStepChangedFiles(payload: unknown): StepChangedFile[] {
  return arrayValue(payload)
    .map(normalizeStepChangedFile)
    .filter((file): file is StepChangedFile => Boolean(file));
}

function normalizeStepChangedFile(payload: unknown): StepChangedFile | undefined {
  const record = recordValue(payload);
  if (!record) {
    return undefined;
  }
  const path = stringFrom(record.path);
  if (!path) {
    return undefined;
  }
  return {
    path,
    name: stringFrom(record.name) ?? path.split("/").pop() ?? path,
    action: stringFrom(record.action) ?? stringFrom(record.state) ?? "modified",
    lensId: stringFrom(record.lensId) ?? stringFrom(record.lens_id),
    mediaType: stringFrom(record.mediaType) ?? stringFrom(record.media_type),
    baseLayerId: stringFrom(record.baseLayerId) ?? stringFrom(record.base_layer_id),
    baseFileObjectId: stringFrom(record.baseFileObjectId) ?? stringFrom(record.base_file_object_id),
    targetFileObjectId: stringFrom(record.targetFileObjectId) ?? stringFrom(record.target_file_object_id),
    sizeBytes: numberLikeFrom(record.sizeBytes ?? record.size_bytes),
    access: recordValue(record.access) as ArtifactAccessDecision | undefined
  };
}

export function normalizeStepDiffWindow(payload: unknown): StepDiffWindow | undefined {
  const record = recordValue(payload);
  if (!record) {
    return undefined;
  }

  const diff = diffModelFromWire(record.diff) ?? diffModelFromWire(record);
  if (!diff) {
    return undefined;
  }

  const fields = diff.fields ?? {};
  const metadata = diff.metadata;
  const path = stringFrom(record.path) ?? stringFrom(fields.path) ?? metadata?.path ?? "";
  const title = (stringFrom(record.title) ?? path) || "Lens diff";
  const lineWindow = metadata?.lineWindow ?? diffLineWindowFromWire(fields.lineWindow);
  const columnWindow =
    metadata?.columnWindow ??
    normalizeDiffColumnWindow(fields.columnWindow) ??
    normalizeDiffColumnWindow(fields);
  const totalDiffLineCount =
    metadata?.totalDiffLineCount ??
    numberLikeFrom(fields.totalDiffLineCount) ??
    numberLikeFrom(fields.totalDiffLines);
  const totalLineCount =
    metadata?.totalLineCount ??
    numberLikeFrom(fields.totalLineCount) ??
    totalDiffLineCount;
  const renderedLineCount =
    metadata?.renderedLineCount ??
    numberLikeFrom(fields.renderedLineCount) ??
    diff.hunks.reduce((count, hunk) => count + hunk.lines.length, 0);
  const windowStart =
    numberLikeFrom(fields.windowStart) ??
    numberLikeFrom(fields.start) ??
    (lineWindow ? Math.max(0, lineWindow.startLine - 1) : undefined);
  const windowEnd =
    numberLikeFrom(fields.windowEnd) ??
    numberLikeFrom(fields.end) ??
    (lineWindow ? lineWindow.endLine : undefined);
  const windowLimit =
    numberLikeFrom(fields.windowLimit) ??
    numberLikeFrom(fields.limit) ??
    (windowStart !== undefined && windowEnd !== undefined ? Math.max(0, windowEnd - windowStart) : undefined);
  const hasMoreBefore =
    metadata?.hasMoreBefore ??
    booleanFrom(fields.hasMoreBefore) ??
    (windowStart !== undefined ? windowStart > 0 : false);
  const hasMoreAfter =
    metadata?.hasMoreAfter ??
    booleanFrom(fields.hasMoreAfter) ??
    booleanFrom(fields.hasMore) ??
    (windowEnd !== undefined && totalDiffLineCount !== undefined ? windowEnd < totalDiffLineCount : false);
  const hasMoreColumns =
    metadata?.hasMoreColumns ??
    booleanFrom(fields.hasMoreColumns) ??
    booleanFrom(fields.lineTextTruncated) ??
    Boolean(columnWindow?.hasMoreColumns);

  return {
    path,
    state: stringFrom(record.state) ?? stringFrom(fields.state) ?? metadata?.state,
    lensId: stringFrom(record.lensId) ?? stringFrom(record.lens_id) ?? stringFrom(fields.lensId) ?? metadata?.lensId,
    title,
    diff,
    message: stringFrom(record.message),
    source: stringFrom(record.source) ?? stringFrom(fields.source) ?? metadata?.source,
    layerId: stringFrom(record.layerId) ?? stringFrom(record.layer_id) ?? stringFrom(fields.layerId) ?? metadata?.layerId,
    stepId: stringFrom(record.stepId) ?? stringFrom(record.step_id) ?? stringFrom(fields.stepId) ?? metadata?.stepId,
    lineWindow,
    columnWindow,
    totalLineCount,
    totalDiffLineCount,
    renderedLineCount,
    windowStart,
    windowEnd,
    windowLimit,
    hasMoreBefore,
    hasMoreAfter,
    hasMoreColumns,
    fields: {
      ...(recordValue(record.fields) ?? {}),
      ...fields
    }
  };
}

export function normalizeStepDiffWindows(payload: unknown): StepDiffWindow[] {
  return arrayValue(payload)
    .map(normalizeStepDiffWindow)
    .filter((diff): diff is StepDiffWindow => Boolean(diff));
}

function previewModelFromWire(value: unknown): PreviewModel | undefined {
  const record = recordValue(value);
  if (!record) {
    return undefined;
  }
  const kind = stringFrom(record.kind);
  const title = stringFrom(record.title);
  const mediaType = stringFrom(record.mediaType) ?? stringFrom(record.media_type);
  if (!kind || !title || !mediaType) {
    return undefined;
  }

  return {
    kind: kind as PreviewKind,
    title,
    body: stringFrom(record.body),
    mediaType,
    language: stringFrom(record.language),
    fields: recordValue(record.fields) ?? {}
  };
}

function diffModelFromWire(value: unknown): DiffModel | undefined {
  const record = recordValue(value);
  if (!record) {
    return undefined;
  }
  const kind = stringFrom(record.kind);
  const summary = stringFrom(record.summary);
  if (!kind || !summary) {
    return undefined;
  }
  const fields = recordValue(record.fields) ?? {};
  const hunks = arrayValue(record.hunks).map(diffHunkFromWire).filter((hunk): hunk is DiffModel["hunks"][number] => Boolean(hunk));
  const metadata = diffModelMetadataFromWire(record.metadata, fields, hunks);

  return {
    kind: kind as DiffModel["kind"],
    summary,
    hunks,
    ...(metadata ? { metadata } : {}),
    fields
  };
}

function diffHunkFromWire(value: unknown): DiffModel["hunks"][number] | undefined {
  const record = recordValue(value);
  if (!record) {
    return undefined;
  }
  const oldStart = numberFrom(record.oldStart ?? record.old_start);
  const oldLines = numberFrom(record.oldLines ?? record.old_lines);
  const newStart = numberFrom(record.newStart ?? record.new_start);
  const newLines = numberFrom(record.newLines ?? record.new_lines);
  if (oldStart === undefined || oldLines === undefined || newStart === undefined || newLines === undefined) {
    return undefined;
  }

  return {
    oldStart,
    oldLines,
    newStart,
    newLines,
    lines: arrayValue(record.lines).map(diffLineFromWire).filter((line): line is DiffModel["hunks"][number]["lines"][number] => Boolean(line))
  };
}

function diffLineFromWire(value: unknown): DiffModel["hunks"][number]["lines"][number] | undefined {
  const record = recordValue(value);
  if (!record) {
    return undefined;
  }
  const op = stringFrom(record.op);
  const text =
    typeof record.text === "string"
      ? record.text
      : typeof record.textSegment === "string"
        ? record.textSegment
        : typeof record.text_segment === "string"
          ? record.text_segment
          : undefined;
  if ((op !== "equal" && op !== "insert" && op !== "delete") || text === undefined) {
    return undefined;
  }

  return {
    op,
    oldLine: numberFrom(record.oldLine ?? record.old_line),
    newLine: numberFrom(record.newLine ?? record.new_line),
    text,
    ...diffLineSegmentFromWire(record, text)
  };
}

function diffModelMetadataFromWire(
  value: unknown,
  fields: Record<string, unknown>,
  hunks: DiffModel["hunks"]
): DiffModelMetadata | undefined {
  const record = recordValue(value) ?? {};
  const actualLineCount = hunks.reduce((count, hunk) => count + hunk.lines.length, 0);
  const windowStart = numberLikeFrom(record.windowStart ?? record.window_start ?? fields.windowStart ?? fields.start);
  const windowEnd = numberLikeFrom(record.windowEnd ?? record.window_end ?? fields.windowEnd ?? fields.end);
  const lineWindow =
    diffLineWindowFromWire(record.lineWindow ?? record.line_window ?? fields.lineWindow) ??
    diffLineWindowFromLegacyFields(windowStart, windowEnd);
  const columnWindow =
    normalizeDiffColumnWindow(record.columnWindow ?? record.column_window ?? fields.columnWindow) ??
    normalizeDiffColumnWindow(record) ??
    normalizeDiffColumnWindow(fields);
  const totalDiffLineCount =
    numberLikeFrom(record.totalDiffLineCount ?? record.total_diff_line_count) ??
    numberLikeFrom(fields.totalDiffLineCount) ??
    numberLikeFrom(fields.totalDiffLines);
  const totalLineCount =
    numberLikeFrom(record.totalLineCount ?? record.total_line_count) ??
    numberLikeFrom(fields.totalLineCount) ??
    totalDiffLineCount;
  const renderedLineCount =
    numberLikeFrom(record.renderedLineCount ?? record.rendered_line_count) ??
    numberLikeFrom(fields.renderedLineCount) ??
    actualLineCount;
  const hasMoreBefore =
    booleanFrom(record.hasMoreBefore ?? record.has_more_before) ??
    booleanFrom(fields.hasMoreBefore) ??
    (windowStart !== undefined ? windowStart > 0 : undefined);
  const hasMoreAfter =
    booleanFrom(record.hasMoreAfter ?? record.has_more_after) ??
    booleanFrom(fields.hasMoreAfter) ??
    booleanFrom(fields.hasMore) ??
    (windowEnd !== undefined && totalDiffLineCount !== undefined ? windowEnd < totalDiffLineCount : undefined);
  const hasMoreColumns =
    booleanFrom(record.hasMoreColumns ?? record.has_more_columns) ??
    booleanFrom(fields.hasMoreColumns) ??
    booleanFrom(fields.lineTextTruncated) ??
    columnWindow?.hasMoreColumns;
  const virtualization = diffVirtualizationFromWire(record.virtualization ?? fields.virtualization);
  const metadata = compactRecord({
    totalLineCount,
    totalDiffLineCount,
    renderedLineCount,
    lineWindow,
    columnWindow,
    hasMoreBefore,
    hasMoreAfter,
    hasMoreColumns,
    truncated:
      booleanFrom(record.truncated) ??
      booleanFrom(fields.truncated) ??
      booleanFrom(fields.oldTruncated) ??
      booleanFrom(fields.newTruncated),
    lineTextTruncated: booleanFrom(record.lineTextTruncated ?? record.line_text_truncated) ?? booleanFrom(fields.lineTextTruncated),
    virtualization,
    source: stringFrom(record.source) ?? stringFrom(fields.source),
    layerId: stringFrom(record.layerId) ?? stringFrom(record.layer_id) ?? stringFrom(fields.layerId),
    stepId: stringFrom(record.stepId) ?? stringFrom(record.step_id) ?? stringFrom(fields.stepId),
    path: stringFrom(record.path) ?? stringFrom(fields.path),
    state: stringFrom(record.state) ?? stringFrom(fields.state),
    lensId: stringFrom(record.lensId) ?? stringFrom(record.lens_id) ?? stringFrom(fields.lensId)
  }) as DiffModelMetadata;

  return Object.keys(metadata).length > 0 ? metadata : undefined;
}

function diffLineSegmentFromWire(record: Record<string, unknown>, fallbackText: string): Partial<DiffModel["hunks"][number]["lines"][number]> {
  const textSegment = stringValue(record.textSegment ?? record.text_segment);
  const columnWindow = normalizeDiffColumnWindow(record);
  const textLength =
    numberLikeFrom(record.textLength ?? record.text_length) ??
    columnWindow?.textLength ??
    (textSegment !== undefined ? Array.from(fallbackText).length : undefined);
  const columnStart =
    numberLikeFrom(record.columnStart ?? record.column_start) ??
    columnWindow?.columnStart;
  const columnEnd =
    numberLikeFrom(record.columnEnd ?? record.column_end) ??
    columnWindow?.columnEnd;
  const hasMoreColumns =
    booleanFrom(record.hasMoreColumns ?? record.has_more_columns) ??
    columnWindow?.hasMoreColumns;

  return compactRecord({
    textSegment,
    textLength,
    columnStart,
    columnEnd,
    hasMoreColumns
  }) as Partial<DiffModel["hunks"][number]["lines"][number]>;
}

function diffLineWindowFromWire(value: unknown): DiffLineWindow | undefined {
  const record = recordValue(value);
  if (!record) {
    return undefined;
  }

  const startLine = numberLikeFrom(record.startLine ?? record.start_line ?? record.start);
  const limit = numberLikeFrom(record.limit);
  const endLine =
    numberLikeFrom(record.endLine ?? record.end_line ?? record.end) ??
    (startLine !== undefined && limit !== undefined ? startLine + limit - 1 : undefined);

  return startLine !== undefined && endLine !== undefined
    ? { startLine, endLine: Math.max(startLine, endLine) }
    : undefined;
}

function diffLineWindowFromLegacyFields(windowStart: number | undefined, windowEnd: number | undefined): DiffLineWindow | undefined {
  if (windowStart === undefined || windowEnd === undefined) {
    return undefined;
  }

  const startLine = Math.max(1, windowStart + 1);
  return {
    startLine,
    endLine: Math.max(startLine, windowEnd)
  };
}

function diffVirtualizationFromWire(value: unknown): DiffModelMetadata["virtualization"] | undefined {
  const record = recordValue(value);
  if (!record) {
    return undefined;
  }

  const virtualization = compactRecord({
    strategy: stringFrom(record.strategy),
    maxRenderedLineCount: numberLikeFrom(record.maxRenderedLineCount ?? record.max_rendered_line_count),
    maxRenderedColumnCount: numberLikeFrom(record.maxRenderedColumnCount ?? record.max_rendered_column_count),
    rowHeightPx: numberLikeFrom(record.rowHeightPx ?? record.row_height_px),
    overscanLineCount: numberLikeFrom(record.overscanLineCount ?? record.overscan_line_count)
  }) as DiffModelMetadata["virtualization"];

  return virtualization && Object.keys(virtualization).length > 0 ? virtualization : undefined;
}

function layerStepDiffStatsFromWire(value: unknown, diffs: StepDiffWindow[]): LayerStepDiffStats {
  const record = recordValue(value);
  const additions =
    numberLikeFrom(record?.additions) ??
    diffs.reduce((count, diff) => count + (numberLikeFrom(diff.diff.fields.additions) ?? 0), 0);
  const deletions =
    numberLikeFrom(record?.deletions) ??
    numberLikeFrom(record?.removals) ??
    diffs.reduce((count, diff) => count + (numberLikeFrom(diff.diff.fields.deletions) ?? 0), 0);
  const files = numberLikeFrom(record?.files) ?? diffs.length;

  return {
    files,
    additions,
    deletions,
    removals: numberLikeFrom(record?.removals) ?? deletions
  };
}

export function createPreviewModelFromArtifactContent(input: {
  payload: ArtifactContentPayload;
  artifact?: Artifact;
  kind?: PreviewKind;
  title?: string;
  fields?: Record<string, unknown>;
}): PreviewModel | undefined {
  const { payload, artifact } = input;
  const value = payload.content.dataUrl ?? payload.content.value ?? payload.content.base64;
  const mediaType = payload.content.mediaType;
  const kind = input.kind ?? inferPreviewKindFromContent(mediaType, payload.path ?? artifact?.location);
  const fields = {
    ...(artifact?.fileObject ? { fileObjectId: artifact.fileObject.fileObjectId } : {}),
    ...(artifact?.rootTreeId ? { rootTreeId: artifact.rootTreeId } : {}),
    ...payload.fields,
    ...(payload.content.base64 ? { data: payload.content.base64, base64: payload.content.base64 } : {}),
    ...(payload.content.dataUrl ? { dataUrl: payload.content.dataUrl, src: payload.content.dataUrl } : {}),
    ...(payload.content.bytes ? { byteLen: payload.content.bytes.byteLength } : {}),
    ...(input.fields ?? {})
  };

  if (!value && !payload.content.bytes && payload.content.chunks.length === 0 && !payload.content.fileObject) {
    return undefined;
  }

  return {
    kind,
    title: input.title ?? artifact?.name ?? payload.path ?? "Artifact preview",
    body: value ?? "",
    mediaType,
    fields
  };
}

function layerFromWire(layer: Layer | LayerWire): Layer {
  const wire = layer as LayerWire;
  const head = layerHeadFromWire(wire.head ?? wire.layerHead ?? wire.layer_head);
  const rootTreeId = wire.rootTreeId ?? wire.root_tree_id ?? head?.rootTreeId;

  return {
    id: wire.id ?? "",
    spaceId: wire.spaceId ?? wire.space_id ?? "",
    parentId: wire.parentId ?? wire.parent_id,
    name: wire.name ?? "Untitled Layer",
    kind: wire.kind ?? "proposal",
    status: wire.status ?? "active",
    summary: wire.summary ?? "",
    artifactIds: wire.artifactIds ?? wire.artifact_ids ?? [],
    stepIds: wire.stepIds ?? wire.step_ids ?? [],
    gateIds: wire.gateIds ?? wire.gate_ids ?? [],
    rootTreeId,
    policyEpoch: wire.policyEpoch ?? wire.policy_epoch ?? head?.policyEpoch,
    serverCursor: wire.serverCursor ?? wire.server_cursor ?? head?.serverCursor,
    head
  };
}

function layerHeadFromWire(head: LayerHeadWire | undefined): LayerHeadMetadata | undefined {
  if (!head) {
    return undefined;
  }

  return compactRecord({
    layerStateId: head.layerStateId ?? head.layer_state_id,
    rootTreeId: head.rootTreeId ?? head.root_tree_id,
    policyEpoch: head.policyEpoch ?? head.policy_epoch,
    serverCursor: head.serverCursor ?? head.server_cursor,
    updatedAt: head.updatedAt ?? head.updated_at,
    updatedByAccountId: head.updatedByAccountId ?? head.updated_by_account_id
  }) as LayerHeadMetadata;
}

function artifactFromWire(artifact: Artifact | ArtifactWire): Artifact {
  const wire = artifact as ArtifactWire;
  const fileObject = fileObjectFromWire(wire.fileObject ?? wire.file_object);
  const chunks = normalizeChunks(wire.chunks);
  const fileObjectId =
    wire.fileObjectId ??
    wire.file_object_id ??
    wire.current_file_object_id ??
    fileObject?.fileObjectId;
  const byteLen = numberFrom(wire.byteLen) ?? numberFrom(wire.byte_len) ?? numberFrom(wire.sizeBytes) ?? numberFrom(wire.size_bytes);
  const contentHash = wire.contentHash ?? wire.content_hash ?? wire.sha256 ?? fileObject?.sha256;

  return {
    id: wire.id ?? wire.artifact_id ?? "",
    spaceId: wire.spaceId ?? wire.space_id ?? "",
    layerId: wire.layerId ?? wire.layer_id,
    name: wire.name ?? wire.path ?? wire.logical_path ?? "Untitled artifact",
    type: artifactTypeFromWire(wire.type ?? wire.artifact_kind) ?? "file",
    summary: wire.summary ?? "",
    location: wire.location ?? wire.logical_path ?? wire.path ?? "",
    updatedAt: wire.updatedAt ?? wire.updated_at ?? new Date(0).toISOString(),
    sizeLabel: wire.sizeLabel ?? wire.size_label ?? (byteLen === undefined ? "" : formatByteLabel(byteLen)),
    proofIds: wire.proofIds ?? wire.proof_ids ?? [],
    access: wire.access,
    mediaType: wire.mediaType ?? wire.media_type ?? fileObject?.mediaType,
    contentHash,
    byteLen,
    lensId: wire.lensId ?? wire.lens_id,
    rootTreeId: wire.rootTreeId ?? wire.root_tree_id,
    currentTreeId: wire.currentTreeId ?? wire.current_tree_id,
    fileObjectId,
    fileObject,
    chunks: chunks.length > 0 ? chunks : fileObject?.chunks,
    preview: wire.preview
  };
}

function fileObjectFromWire(fileObject: FileObjectWire | undefined): FileObjectMetadata | undefined {
  if (!fileObject) {
    return undefined;
  }

  const fileObjectId = fileObject.fileObjectId ?? fileObject.file_object_id ?? fileObject.id;
  if (!fileObjectId) {
    return undefined;
  }

  const chunks = normalizeChunks(fileObject.chunks);
  return {
    id: fileObjectId,
    fileObjectId,
    sha256: fileObject.sha256,
    sizeBytes: numberFrom(fileObject.sizeBytes) ?? numberFrom(fileObject.size_bytes),
    mediaType: fileObject.mediaType ?? fileObject.media_type,
    chunkCount: numberFrom(fileObject.chunkCount) ?? numberFrom(fileObject.chunk_count) ?? chunks.length,
    chunks
  };
}

function chunkFromWire(chunk: ChunkWire | undefined): ChunkMetadata | undefined {
  if (!chunk) {
    return undefined;
  }

  const chunkId = chunk.chunkId ?? chunk.chunk_id ?? chunk.id;
  if (!chunkId) {
    return undefined;
  }

  return {
    id: chunkId,
    chunkId,
    sha256: chunk.sha256,
    sizeBytes: numberFrom(chunk.sizeBytes) ?? numberFrom(chunk.size_bytes),
    byteOffset: numberFrom(chunk.byteOffset) ?? numberFrom(chunk.byte_offset),
    chunkIndex: numberFrom(chunk.chunkIndex) ?? numberFrom(chunk.chunk_index),
    mediaType: chunk.mediaType ?? chunk.media_type,
    compression: chunk.compression,
    state: chunk.state,
    objectKey: chunk.objectKey ?? chunk.object_key,
    value: stringFrom(chunk.value) ?? stringFrom(chunk.content) ?? stringFrom(chunk.data),
    encoding: chunk.encoding
  };
}

function normalizeChunks(chunks: unknown): ChunkMetadata[] {
  return arrayValue(chunks)
    .map((chunk) => chunkFromWire(chunk as ChunkWire))
    .filter((chunk): chunk is ChunkMetadata => Boolean(chunk))
    .sort((left, right) => (left.chunkIndex ?? 0) - (right.chunkIndex ?? 0));
}

interface DecodedContentValue {
  value?: string;
  bytes?: Uint8Array;
  base64?: string;
  dataUrl?: string;
}

function contentSourceValue(value: unknown, chunks: ChunkMetadata[]): string | undefined {
  const direct = contentScalarValue(value);
  if (direct !== undefined) {
    return direct;
  }

  const chunkValues = chunks.map((chunk) => chunk.value).filter((chunkValue): chunkValue is string => Boolean(chunkValue));
  return chunkValues.length > 0 ? chunkValues.join("") : undefined;
}

function decodeContentValue(value: string | undefined, encoding: string | undefined, mediaType: string): DecodedContentValue {
  if (value === undefined) {
    return {};
  }

  if (encoding === "base64") {
    const base64 = normalizeBase64(value);
    const bytes = decodeBase64(base64);
    return decodedBytesValue(bytes, mediaType, base64);
  }

  if (encoding === "hex") {
    const bytes = decodeHex(value);
    return decodedBytesValue(bytes, mediaType, encodeBase64(bytes));
  }

  if (isBinaryMediaType(mediaType) && looksLikeDataUrl(value)) {
    return {
      value,
      dataUrl: value,
      base64: value.slice(value.indexOf(",") + 1)
    };
  }

  return { value };
}

function decodedBytesValue(bytes: Uint8Array, mediaType: string, base64: string): DecodedContentValue {
  if (isTextMediaType(mediaType)) {
    return {
      value: decodeUtf8(bytes),
      bytes,
      base64
    };
  }

  if (mediaType.startsWith("image/")) {
    return {
      value: `data:${mediaType};base64,${base64}`,
      bytes,
      base64,
      dataUrl: `data:${mediaType};base64,${base64}`
    };
  }

  return {
    value: base64,
    bytes,
    base64
  };
}

function contentScalarValue(value: unknown): string | undefined {
  if (typeof value === "string") {
    return value;
  }

  if (value === undefined || value === null) {
    return undefined;
  }

  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }

  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return undefined;
  }
}

function normalizeContentEncoding(encoding: string | undefined): string | undefined {
  const normalized = encoding?.trim().toLowerCase();
  if (normalized === "base64" || normalized === "hex" || normalized === "json" || normalized === "text" || normalized === "utf8") {
    return normalized;
  }
  return normalized;
}

function isTextMediaType(mediaType: string): boolean {
  return (
    mediaType.startsWith("text/") ||
    mediaType.includes("json") ||
    mediaType.includes("xml") ||
    mediaType.includes("javascript") ||
    mediaType.includes("typescript") ||
    mediaType.includes("toml") ||
    mediaType.includes("yaml")
  );
}

function isBinaryMediaType(mediaType: string): boolean {
  return mediaType.startsWith("image/") || mediaType === "application/octet-stream" || mediaType.startsWith("audio/") || mediaType.startsWith("video/");
}

function looksLikeDataUrl(value: string): boolean {
  return /^data:[^,]+;base64,/i.test(value);
}

function normalizeBase64(value: string): string {
  const commaIndex = value.indexOf(",");
  const raw = looksLikeDataUrl(value) && commaIndex >= 0 ? value.slice(commaIndex + 1) : value;
  return raw.replace(/\s+/g, "");
}

function decodeUtf8(bytes: Uint8Array): string {
  return new TextDecoder("utf-8", { fatal: false }).decode(bytes);
}

function decodeHex(value: string): Uint8Array {
  const clean = value.trim().replace(/^0x/i, "").replace(/\s+/g, "");
  if (clean.length % 2 !== 0 || /[^0-9a-f]/i.test(clean)) {
    return new Uint8Array();
  }

  const bytes = new Uint8Array(clean.length / 2);
  for (let index = 0; index < clean.length; index += 2) {
    bytes[index / 2] = Number.parseInt(clean.slice(index, index + 2), 16);
  }
  return bytes;
}

const BASE64_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

function decodeBase64(value: string): Uint8Array {
  const clean = normalizeBase64(value).replace(/=+$/, "");
  const bytes: number[] = [];
  let buffer = 0;
  let bits = 0;

  for (const char of clean) {
    const sixBits = BASE64_ALPHABET.indexOf(char);
    if (sixBits < 0) {
      continue;
    }

    buffer = (buffer << 6) | sixBits;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      bytes.push((buffer >> bits) & 0xff);
    }
  }

  return new Uint8Array(bytes);
}

function encodeBase64(bytes: Uint8Array): string {
  let output = "";
  let index = 0;

  for (; index + 2 < bytes.length; index += 3) {
    output +=
      BASE64_ALPHABET[bytes[index] >> 2] +
      BASE64_ALPHABET[((bytes[index] & 0x03) << 4) | (bytes[index + 1] >> 4)] +
      BASE64_ALPHABET[((bytes[index + 1] & 0x0f) << 2) | (bytes[index + 2] >> 6)] +
      BASE64_ALPHABET[bytes[index + 2] & 0x3f];
  }

  if (index < bytes.length) {
    output += BASE64_ALPHABET[bytes[index] >> 2];
    if (index + 1 < bytes.length) {
      output += BASE64_ALPHABET[((bytes[index] & 0x03) << 4) | (bytes[index + 1] >> 4)];
      output += BASE64_ALPHABET[(bytes[index + 1] & 0x0f) << 2];
      output += "=";
    } else {
      output += BASE64_ALPHABET[(bytes[index] & 0x03) << 4];
      output += "==";
    }
  }

  return output;
}

function inferPreviewKindFromContent(mediaType: string, path: string | undefined): PreviewKind {
  if (mediaType.startsWith("image/")) {
    return "image";
  }

  const extension = extensionFromPath(path);
  if (
    mediaType.includes("javascript") ||
    mediaType.includes("json") ||
    mediaType.includes("typescript") ||
    ["css", "html", "js", "json", "jsx", "rs", "ts", "tsx"].includes(extension ?? "")
  ) {
    return "code";
  }

  if (mediaType.startsWith("text/") || ["md", "mdx", "txt"].includes(extension ?? "")) {
    return "text";
  }

  return "raw";
}

function artifactTypeFromWire(value: unknown): ArtifactType | undefined {
  return value === "file" ||
    value === "note" ||
    value === "image" ||
    value === "report" ||
    value === "proof" ||
    value === "step-output"
    ? value
    : undefined;
}

function compactRecord(values: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(values).filter(([, value]) => value !== undefined && value !== null)
  );
}

function firstRecord(...values: unknown[]): Record<string, unknown> | undefined {
  for (const value of values) {
    const record = recordValue(value);
    if (record) {
      return record;
    }
  }
  return undefined;
}

function recordValue(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
}

function arrayValue(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function stringFrom(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function numberFrom(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function numberLikeFrom(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isFinite(value)) {
    return Math.floor(value);
  }

  if (typeof value === "string" && value.trim().length > 0) {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? Math.floor(parsed) : undefined;
  }

  return undefined;
}

function booleanFrom(value: unknown): boolean | undefined {
  if (typeof value === "boolean") {
    return value;
  }

  if (value === "true") {
    return true;
  }

  if (value === "false") {
    return false;
  }

  return undefined;
}

function extensionFromPath(path: string | undefined): string | undefined {
  const name = path?.split(/[\\/]/).pop();
  const dotIndex = name?.lastIndexOf(".") ?? -1;
  if (!name || dotIndex < 0 || dotIndex === name.length - 1) {
    return undefined;
  }
  return name.slice(dotIndex + 1).toLowerCase();
}

function formatByteLabel(bytes: number): string {
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

