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

const EXECUTION_CONSTRUCTOR = Symbol("Lens execution constructor");
const verifiedExecutions = new WeakSet<VerifiedLensReconcileExecution>();

export class VerifiedLensReconcileExecution {
  readonly manifest: LensManifest;
  readonly pin: LensPin;
  readonly implementationDigest: string;

  constructor(
    constructorToken: typeof EXECUTION_CONSTRUCTOR,
    manifest: LensManifest,
    pin: LensPin,
    implementationDigest: string
  ) {
    if (constructorToken !== EXECUTION_CONSTRUCTOR) {
      throw new Error("Lens execution proofs can only be created by the implementation loader");
    }
    this.manifest = manifest;
    this.pin = pin;
    this.implementationDigest = implementationDigest;
    verifiedExecutions.add(this);
    Object.freeze(this);
  }
}

export class LoadedLensReconcileImplementation {
  readonly #implementationBytes: Uint8Array;
  readonly #execution: VerifiedLensReconcileExecution;

  private constructor(
    implementationBytes: Uint8Array,
    execution: VerifiedLensReconcileExecution
  ) {
    this.#implementationBytes = implementationBytes;
    this.#execution = execution;
  }

  static async load(
    manifest: LensManifest,
    implementationBytes: Uint8Array
  ): Promise<LoadedLensReconcileImplementation> {
    const manifestSnapshot = immutableManifestSnapshot(manifest);
    const pin = lensPinFromManifest(manifestSnapshot);
    if (!pin) {
      throw new Error(`Executed Lens manifest ${manifestSnapshot.id} is not pinned`);
    }
    const expectedImplementationDigest =
      manifestSnapshot.bundle?.entrypoints.reconcile?.digest ?? manifestSnapshot.digest;
    if (!expectedImplementationDigest) {
      throw new Error(
        `Lens manifest ${manifestSnapshot.id} does not declare a reconcile implementation digest`
      );
    }
    const bytesSnapshot = Uint8Array.from(implementationBytes);
    const implementationDigest = await sha256Digest(bytesSnapshot);
    if (implementationDigest !== expectedImplementationDigest) {
      throw new Error("Executed Lens implementation bytes do not match the validated manifest");
    }
    const execution = new VerifiedLensReconcileExecution(
      EXECUTION_CONSTRUCTOR,
      manifestSnapshot,
      Object.freeze(pin),
      implementationDigest
    );
    return new LoadedLensReconcileImplementation(bytesSnapshot, execution);
  }

  implementationBytes(): Uint8Array {
    return Uint8Array.from(this.#implementationBytes);
  }

  execution(): VerifiedLensReconcileExecution {
    return this.#execution;
  }
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

export function assertVerifiedLensReconcileExecution(
  value: unknown
): asserts value is VerifiedLensReconcileExecution {
  if (
    !value ||
    typeof value !== "object" ||
    !verifiedExecutions.has(value as VerifiedLensReconcileExecution)
  ) {
    throw new Error("Lens reconcile execution proof was not created by the implementation loader");
  }
}

export function supportsLensCapability(manifest: LensManifest, capability: LensCapability): boolean {
  return manifest.analyzer.capabilities.includes(capability);
}

async function sha256Digest(bytes: Uint8Array): Promise<string> {
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) {
    throw new Error("Web Crypto SHA-256 is required to load Lens implementations");
  }
  const input = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(input).set(bytes);
  const digest = new Uint8Array(await subtle.digest("SHA-256", input));
  return `sha256:${Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}

function immutableManifestSnapshot(manifest: LensManifest): LensManifest {
  const snapshot = JSON.parse(JSON.stringify(manifest)) as unknown;
  assertLensManifest(snapshot);
  return deepFreeze(snapshot);
}

function deepFreeze<T>(value: T): T {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const child of Object.values(value)) {
      deepFreeze(child);
    }
  }
  return value;
}
