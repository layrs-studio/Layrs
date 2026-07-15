import type {
  LensBundleContract,
  DiffKind,
  LensCapability,
  LensPin,
  LensManifest,
  PreviewKind,
  ReconcileStatus,
  ScopedResolutionMethod
} from "./contracts";
import { LENS_CONTRACT_VERSION } from "./contracts";
import { assertSha256Digest } from "./validation";

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
  digest?: string;
  bundle?: LensBundleContract;
  resolutionMethods?: ScopedResolutionMethod[];
  reconcileStatuses?: ReconcileStatus[];
}): LensManifest {
  if (input.digest !== undefined) {
    assertSha256Digest(input.digest, "Lens manifest digest");
  }
  if (input.bundle && input.digest === undefined) {
    throw new Error("A bundled lens manifest requires an explicit digest");
  }

  return {
    contractVersion: LENS_CONTRACT_VERSION,
    id: input.id,
    name: input.name,
    version: input.version ?? "0.0.0",
    ...(input.digest ? { digest: input.digest } : {}),
    analyzer: {
      supportedMediaTypes: [...input.supportedMediaTypes],
      fileExtensions: [...input.fileExtensions],
      capabilities: [...input.capabilities]
    },
    viewer: {
      viewerId: `layrs.viewer.${input.id.replace(/^layrs\./, "")}`,
      schemaVersion: "layrs.viewer.v1",
      component: input.component,
      previewKinds: [...input.previewKinds],
      diffKinds: [...input.diffKinds],
      reconcileStatuses: [...(input.reconcileStatuses ?? [])],
      inspectorFields: []
    },
    resolutionMethods: [...(input.resolutionMethods ?? [])],
    ...(input.bundle ? { bundle: input.bundle } : {})
  };
}

export function lensPinFromManifest(manifest: LensManifest): LensPin | undefined {
  if (manifest.digest === undefined) {
    return undefined;
  }
  assertSha256Digest(manifest.digest, "Lens manifest digest");
  return {
    lensId: manifest.id,
    version: manifest.version,
    digest: manifest.digest
  };
}

export function supportsLensCapability(manifest: LensManifest, capability: LensCapability): boolean {
  return manifest.analyzer.capabilities.includes(capability);
}
