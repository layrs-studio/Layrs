export const LENS_CONTRACT_VERSION = "layrs.lens.v1" as const;
export const LENS_BUNDLE_FORMAT = "layrslens.v1" as const;
export const MAX_PORTABLE_INTEGER = 9_007_199_254_740_991;

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
export type ResolutionMethod = "existing" | "incoming" | "both" | "manual";
export type LensReconcileActionScope = "file" | "block";
export type ResolutionScope = LensReconcileActionScope;
export type LensPreviewWindowUnit = "bytes" | "lines" | "items";

export interface LensPin {
  lensId: LensId;
  version: string;
  digest: string;
}

export interface ScopedResolutionMethod {
  method: ResolutionMethod;
  scope: ResolutionScope;
}

export interface LensManifest {
  contractVersion: typeof LENS_CONTRACT_VERSION;
  id: LensId;
  name: string;
  version: string;
  digest?: string;
  analyzer: AnalyzerContract;
  viewer: ViewerContract;
  resolutionMethods: ScopedResolutionMethod[];
  bundle?: LensBundleContract;
}

export interface AnalyzerContract {
  supportedMediaTypes: string[];
  fileExtensions: string[];
  capabilities: LensCapability[];
}

export interface LensResourceQuotas {
  maxMemoryBytes: number;
  maxExecutionMs: number;
  maxInputBytes: number;
  maxOutputBytes: number;
  maxFuel?: number;
}

export interface WasiLensEntrypoint {
  runtime: "wasi";
  path: string;
  export: string;
  digest: string;
  resources: LensResourceQuotas;
}

export interface EsmLensEntrypoint {
  runtime: "esm";
  path: string;
  export: string;
  digest: string;
  resources: LensResourceQuotas;
}

export interface LensBundleEntrypoints {
  analyzer?: WasiLensEntrypoint;
  reconcile?: WasiLensEntrypoint;
  viewer?: EsmLensEntrypoint;
}

export interface LensSignatureMetadata {
  algorithm: string;
  keyId: string;
  signedDigest: string;
  signature: string;
}

export interface LensBundleContract {
  format: typeof LENS_BUNDLE_FORMAT;
  entrypoints: LensBundleEntrypoints;
  signature?: LensSignatureMetadata;
}

export interface LensWindowRange {
  start: number;
  endExclusive: number;
  total: number;
}

export interface LensPreviewWindow {
  contractVersion: typeof LENS_CONTRACT_VERSION;
  lens: LensPin;
  artifactId: string;
  unit: LensPreviewWindowUnit;
  window: LensWindowRange;
}

export interface LensDiffWindow {
  contractVersion: typeof LENS_CONTRACT_VERSION;
  lens: LensPin;
  path: string;
  lines: LensWindowRange;
  columns?: LensWindowRange;
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

export interface PresentLensReconcileSide {
  exists: true;
  bytes: Uint8Array;
  contentHash?: string;
  size: number;
}

export interface AbsentLensReconcileSide {
  exists: false;
  bytes?: Uint8Array;
  contentHash?: never;
  size: 0;
}

export type LensReconcileSide = PresentLensReconcileSide | AbsentLensReconcileSide;

export interface LensReconcileInput {
  path: string;
  mediaType?: string;
  base: LensReconcileSide;
  ours: LensReconcileSide;
  theirs: LensReconcileSide;
  oursLabel: string;
  theirsLabel: string;
}

export interface PresentLensReconcileSideContract {
  exists: true;
  bytes: number[];
  contentHash?: string;
  size: number;
}

export interface AbsentLensReconcileSideContract {
  exists: false;
  bytes: [];
  contentHash?: never;
  size: 0;
}

export type LensReconcileSideContract =
  | PresentLensReconcileSideContract
  | AbsentLensReconcileSideContract;

export interface LensReconcileRequest {
  contractVersion: typeof LENS_CONTRACT_VERSION;
  lens: LensPin;
  path: string;
  mediaType?: string;
  base: LensReconcileSideContract;
  existing: LensReconcileSideContract;
  incoming: LensReconcileSideContract;
  labels: LensReconcileLabels;
}

export type LensReconcileContent =
  | { exists: true; bytes: Uint8Array }
  | { exists: false; bytes?: Uint8Array };

export interface LensConflictBlock {
  blockId: string;
  base: string;
  ours?: string;
  theirs?: string;
  existing?: string;
  incoming?: string;
  methods?: ResolutionMethod[];
  supportedResolutions?: string[];
  resolutionMethods?: ScopedResolutionMethod[];
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
  fileMethods?: ResolutionMethod[];
  resolutionMethods?: ScopedResolutionMethod[];
  blocks: LensConflictBlock[];
  segments: LensConflictSegment[];
  fields: Record<string, unknown>;
}

export type CanonicalLensReconcileContent =
  | { exists: true; bytes: number[] }
  | { exists: false; bytes: [] };

export interface CanonicalLensConflictBlock {
  blockId: string;
  base: string;
  existing: string;
  incoming: string;
  resolutionMethods: ScopedResolutionMethod[];
}

export type LensContractMetadataValue = string | number | boolean | string[];
export type LensContractMetadata = Record<string, LensContractMetadataValue>;

interface CanonicalLensReconcileResultBase {
  summary: string;
  resolutionMethods: ScopedResolutionMethod[];
  blocks: CanonicalLensConflictBlock[];
  segments: LensConflictSegment[];
  fields: LensContractMetadata;
}

export type CanonicalLensReconcileResult =
  | (CanonicalLensReconcileResultBase & {
      status: "auto_resolved";
      resolved: CanonicalLensReconcileContent;
      conflict?: never;
    })
  | (CanonicalLensReconcileResultBase & {
      status: "conflicted";
      resolved?: never;
      conflict: CanonicalLensReconcileContent;
    })
  | (CanonicalLensReconcileResultBase & {
      status: "unsupported";
      resolved?: never;
      conflict?: never;
    });

export interface LensReconcileResponse {
  contractVersion: typeof LENS_CONTRACT_VERSION;
  lens: LensPin;
  result: CanonicalLensReconcileResult;
}

export interface LensReconcileConflictBlock {
  blockId: string;
  status: string;
  base?: string;
  existing: string;
  incoming: string;
  resolution?: string;
  supportedMethods?: ResolutionMethod[];
}

export interface LensReconcileConflictSegment {
  kind: LensConflictSegmentKind;
  text?: string;
  blockId?: string;
}

export interface LensReconcileConflict {
  conflictId: string;
  path: string;
  lensId: LensId;
  status: string;
  message?: string;
  resolution?: string;
  blocks: LensReconcileConflictBlock[];
  segments?: LensReconcileConflictSegment[];
  supportedMethods?: ResolutionMethod[];
  fields?: Record<string, unknown>;
}

export interface LensReconcileAction {
  method: ResolutionMethod;
  scope: LensReconcileActionScope;
  label: string;
  resolution: string;
  blockId?: string;
  requiresManualText?: boolean;
}

export interface LensReconcileResolution {
  path: string;
  lensId: LensId;
  method: ResolutionMethod;
  scope: LensReconcileActionScope;
  resolution: string;
  blockId?: string;
  manualText?: string;
}

export interface LensReconcileLabels {
  existing: string;
  incoming: string;
}

export interface LensReconcileRendererProps {
  conflict: LensReconcileConflict;
  title: string;
  emptyMessage: string;
  className?: string;
  disabled?: boolean;
  busy?: boolean;
  labels?: Partial<LensReconcileLabels>;
  onResolve?: (resolution: LensReconcileResolution) => void;
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
