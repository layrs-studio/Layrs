export type LensCapability =
  | "view"
  | "reconcile"
  | "metadata"
  | "preview"
  | "diff"
  | "references"
  | "proofRecipes"
  | (string & {});

export type CanonicalLensId = "layrs.code" | "layrs.text" | "layrs.image" | "layrs.raw";
export type LensId = CanonicalLensId | (string & {});
export type ArtifactKind = "raw" | "text" | "code" | "image" | (string & {});
export type PreviewKind = "raw" | "text" | "code" | "image" | (string & {});
export type DiffKind = "textLines" | "binary" | "imageMetadata" | (string & {});
export type ReferenceKind = "relativePath" | "urlFunction" | "externalUrl" | (string & {});
export type InspectorValueType = "string" | "number" | "boolean" | "stringList";
export type ProofStatus = "pass" | "warn" | "notEvaluated";
export type ReconcileStatus = "unsupported" | "needs_manual_resolution" | "auto_resolvable";
export type LensReconcileResultStatus = "auto_resolved" | "conflicted" | "unsupported";
export type LensConflictSegmentKind = "text" | "block";

export interface LensManifest {
  id: LensId;
  name: string;
  version: string;
  analyzer: AnalyzerContract;
  viewer: ViewerContract;
}

export interface AnalyzerContract {
  supportedMediaTypes: string[];
  fileExtensions: string[];
  capabilities: LensCapability[];
}

export interface ArtifactMetadata {
  artifactId: string;
  lensId: LensId;
  kind: ArtifactKind;
  mediaType: string;
  byteLen: number;
  contentHash: string;
  fields: Record<string, unknown>;
}

export interface PreviewModel {
  kind: PreviewKind;
  title: string;
  body?: string;
  mediaType: string;
  language?: string;
  dimensions?: Dimensions;
  fields: Record<string, unknown>;
}

export interface Dimensions {
  width: number;
  height: number;
}

export interface DiffModel {
  kind: DiffKind;
  summary: string;
  hunks: DiffHunk[];
  metadata?: DiffModelMetadata;
  fields: DiffModelFields;
}

export interface DiffModelFields extends Record<string, unknown> {
  totalLineCount?: number;
  totalDiffLineCount?: number;
  totalDiffLines?: number;
  renderedLineCount?: number;
  lineWindow?: DiffLineWindow;
  columnWindow?: DiffColumnWindow;
  hasMoreBefore?: boolean;
  hasMoreAfter?: boolean;
  hasMoreColumns?: boolean;
  truncated?: boolean;
  lineTextTruncated?: boolean;
  virtualization?: DiffVirtualizationMetadata;
  source?: string;
  layerId?: string;
  stepId?: string;
  path?: string;
  state?: string;
  lensId?: LensId;
}

export interface DiffModelMetadata {
  totalLineCount?: number;
  totalDiffLineCount?: number;
  renderedLineCount?: number;
  lineWindow?: DiffLineWindow;
  columnWindow?: DiffColumnWindow;
  hasMoreBefore?: boolean;
  hasMoreAfter?: boolean;
  hasMoreColumns?: boolean;
  truncated?: boolean;
  lineTextTruncated?: boolean;
  virtualization?: DiffVirtualizationMetadata;
  source?: string;
  layerId?: string;
  stepId?: string;
  path?: string;
  state?: string;
  lensId?: LensId;
}

export interface DiffLineWindow {
  startLine: number;
  endLine: number;
}

export interface DiffColumnWindow {
  columnStart: number;
  columnEnd: number;
  textLength?: number;
  hasMoreColumns?: boolean;
}

export interface DiffVirtualizationMetadata {
  strategy?: "clientWindow" | "serverWindow" | (string & {});
  maxRenderedLineCount?: number;
  maxRenderedColumnCount?: number;
  rowHeightPx?: number;
  overscanLineCount?: number;
}

export interface ReconcileModel {
  status: ReconcileStatus;
  summary: string;
  fields: Record<string, unknown>;
}

export interface LensReconcileSide {
  exists: boolean;
  bytes?: Uint8Array;
  contentHash?: string;
  size: number;
}

export interface LensReconcileInput {
  path: string;
  mediaType?: string;
  base: LensReconcileSide;
  ours: LensReconcileSide;
  theirs: LensReconcileSide;
  oursLabel: string;
  theirsLabel: string;
}

export interface LensReconcileContent {
  exists: boolean;
  bytes?: Uint8Array;
}

export interface LensConflictBlock {
  blockId: string;
  base: string;
  ours: string;
  theirs: string;
  supportedResolutions: string[];
}

export interface LensConflictSegment {
  kind: LensConflictSegmentKind;
  text?: string;
  blockId?: string;
}

export interface LensReconcileResult {
  status: LensReconcileResultStatus;
  summary: string;
  resolved?: LensReconcileContent;
  conflict?: LensReconcileContent;
  blocks: LensConflictBlock[];
  segments: LensConflictSegment[];
  fields: Record<string, unknown>;
}

export interface LensAnalysisOutput {
  metadata: ArtifactMetadata;
  preview?: PreviewModel;
  diff?: DiffModel;
  reconcile: ReconcileModel;
  references: ExtractedReference[];
  proofRecipes: ProofRecipe[];
}

export interface DiffHunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: DiffLine[];
}

export interface DiffLine {
  op: "equal" | "insert" | "delete";
  oldLine?: number;
  newLine?: number;
  text: string;
  textSegment?: string;
  textLength?: number;
  columnStart?: number;
  columnEnd?: number;
  hasMoreColumns?: boolean;
}

export interface ExtractedReference {
  target: string;
  kind: ReferenceKind;
  span: TextSpan;
}

export interface TextSpan {
  start: number;
  end: number;
  line: number;
  column: number;
}

export interface ViewerContract {
  viewerId: string;
  schemaVersion: "layrs.viewer.v1" | string;
  component: string;
  previewKinds: PreviewKind[];
  diffKinds: DiffKind[];
  reconcileStatuses: ReconcileStatus[];
  inspectorFields: InspectorField[];
}

export interface InspectorField {
  key: string;
  label: string;
  valueType: InspectorValueType;
}

export interface ProofRecipe {
  id: string;
  title: string;
  description: string;
  checks: ProofCheck[];
}

export interface ProofCheck {
  subject: string;
  expectation: string;
  observed?: string;
  status: ProofStatus;
}
