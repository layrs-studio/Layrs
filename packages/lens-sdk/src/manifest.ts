import type {
  DiffKind,
  LensCapability,
  LensManifest,
  PreviewKind,
  ReconcileStatus
} from "./contracts";

const RECONCILE_STATUSES: ReconcileStatus[] = [
  "unsupported",
  "needs_manual_resolution",
  "auto_resolvable"
];

export function createLensManifest(input: {
  id: string;
  name: string;
  version?: string;
  component: string;
  previewKinds: PreviewKind[];
  diffKinds: DiffKind[];
  supportedMediaTypes: string[];
  fileExtensions: string[];
  capabilities: LensCapability[];
}): LensManifest {
  return {
    id: input.id,
    name: input.name,
    version: input.version ?? "0.0.0",
    analyzer: {
      supportedMediaTypes: input.supportedMediaTypes,
      fileExtensions: input.fileExtensions,
      capabilities: input.capabilities
    },
    viewer: {
      viewerId: `layrs.viewer.${input.id.replace(/^layrs\./, "")}`,
      schemaVersion: "layrs.viewer.v1",
      component: input.component,
      previewKinds: input.previewKinds,
      diffKinds: input.diffKinds,
      reconcileStatuses: RECONCILE_STATUSES,
      inspectorFields: []
    }
  };
}
