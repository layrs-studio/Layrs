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
import { assertLensManifest } from "./manifestValidation";
import { assertSha256Digest } from "./validation";

export interface VerifiedLensReconcileExecution {
  manifest: LensManifest;
  pin: LensPin;
  implementationDigest: string;
}

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
  const manifest: LensManifest = {
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
  assertLensManifest(manifest);
  return manifest;
}

export function lensPinFromManifest(manifest: LensManifest): LensPin | undefined {
  assertLensManifest(manifest);
  if (manifest.digest === undefined) {
    return undefined;
  }
  return {
    lensId: manifest.id,
    version: manifest.version,
    digest: manifest.digest
  };
}

export function verifyLensReconcileExecution(
  manifest: LensManifest,
  implementationDigest: string
): VerifiedLensReconcileExecution {
  assertLensManifest(manifest);
  assertSha256Digest(implementationDigest, "Executed Lens implementation digest");
  const pin = lensPinFromManifest(manifest);
  if (!pin) {
    throw new Error(`Executed Lens manifest ${manifest.id} is not pinned`);
  }
  const expectedImplementationDigest = manifest.bundle?.entrypoints.reconcile?.digest ?? manifest.digest;
  if (!expectedImplementationDigest) {
    throw new Error(`Lens manifest ${manifest.id} does not declare a reconcile implementation digest`);
  }
  if (implementationDigest !== expectedImplementationDigest) {
    throw new Error("Executed Lens implementation digest does not match the validated manifest");
  }
  return { manifest, pin, implementationDigest };
}

export function supportsLensCapability(manifest: LensManifest, capability: LensCapability): boolean {
  return manifest.analyzer.capabilities.includes(capability);
}
