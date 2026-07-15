import {
  LENS_BUNDLE_FORMAT,
  LENS_CONTRACT_VERSION,
  type EsmLensEntrypoint,
  type LensManifest,
  type LensResourceQuotas,
  type ScopedResolutionMethod,
  type WasiLensEntrypoint
} from "./contracts";
import { assertLensId, assertSemanticVersion, assertSha256Digest } from "./validation";

const MANIFEST_KEYS = new Set([
  "contractVersion",
  "id",
  "name",
  "version",
  "digest",
  "analyzer",
  "viewer",
  "resolutionMethods",
  "bundle"
]);
const BUNDLE_PATH = /^(?!\/)(?!.*(?:^|\/)\.\.(?:\/|$))[A-Za-z0-9._/-]+$/;

export function assertLensManifest(value: unknown): asserts value is LensManifest {
  const manifest = record(value, "Lens manifest");
  onlyKeys(manifest, MANIFEST_KEYS, "Lens manifest");
  if (manifest.contractVersion !== LENS_CONTRACT_VERSION) {
    throw new Error(`Lens manifest contractVersion must be ${LENS_CONTRACT_VERSION}`);
  }
  assertLensId(manifest.id, "Lens manifest id");
  nonEmptyString(manifest.name, "Lens manifest name");
  assertSemanticVersion(manifest.version, "Lens manifest version");
  if (manifest.digest !== undefined) {
    assertSha256Digest(manifest.digest, "Lens manifest digest");
  }

  validateAnalyzer(manifest.analyzer);
  validateViewer(manifest.viewer);
  validateResolutionMethods(manifest.resolutionMethods);
  if (manifest.bundle !== undefined) {
    if (manifest.digest === undefined) {
      throw new Error("A bundled lens manifest requires an explicit identity digest");
    }
    validateBundle(manifest.bundle, manifest.digest);
  }

  const capabilities = (manifest.analyzer as { capabilities: unknown[] }).capabilities;
  const methods = manifest.resolutionMethods as ScopedResolutionMethod[];
  if (methods.length > 0 && !capabilities.includes("reconcile")) {
    throw new Error("Lens manifests with resolutionMethods must declare the reconcile capability");
  }
}

export function parseLensManifest(value: unknown): LensManifest {
  assertLensManifest(value);
  return value;
}

export function parseLensManifestList(values: unknown[]): LensManifest[] {
  const manifests = values.map((value, index) => {
    try {
      return parseLensManifest(value);
    } catch (error) {
      throw new Error(`Lens manifest ${index} is invalid: ${errorMessage(error)}`);
    }
  });
  const ids = new Set<string>();
  for (const manifest of manifests) {
    if (ids.has(manifest.id)) {
      throw new Error(`Lens registry contains duplicate id ${manifest.id}`);
    }
    ids.add(manifest.id);
  }
  return manifests;
}

function validateAnalyzer(value: unknown): void {
  const analyzer = record(value, "Lens analyzer");
  onlyKeys(analyzer, new Set(["supportedMediaTypes", "fileExtensions", "capabilities"]), "Lens analyzer");
  stringArray(analyzer.supportedMediaTypes, "Lens analyzer supportedMediaTypes");
  stringArray(analyzer.fileExtensions, "Lens analyzer fileExtensions", /^[A-Za-z0-9][A-Za-z0-9_-]*$/);
  const capabilities = stringArray(analyzer.capabilities, "Lens analyzer capabilities");
  if (capabilities.length === 0) {
    throw new Error("Lens analyzer capabilities must not be empty");
  }
}

function validateViewer(value: unknown): void {
  const viewer = record(value, "Lens viewer");
  onlyKeys(
    viewer,
    new Set([
      "viewerId",
      "schemaVersion",
      "component",
      "previewKinds",
      "diffKinds",
      "reconcileStatuses",
      "inspectorFields"
    ]),
    "Lens viewer"
  );
  nonEmptyString(viewer.viewerId, "Lens viewer viewerId");
  if (viewer.schemaVersion !== "layrs.viewer.v1") {
    throw new Error("Lens viewer schemaVersion must be layrs.viewer.v1");
  }
  nonEmptyString(viewer.component, "Lens viewer component");
  stringArray(viewer.previewKinds, "Lens viewer previewKinds");
  stringArray(viewer.diffKinds, "Lens viewer diffKinds");
  const statuses = stringArray(viewer.reconcileStatuses, "Lens viewer reconcileStatuses");
  for (const status of statuses) {
    if (status !== "unsupported" && status !== "needs_manual_resolution" && status !== "auto_resolvable") {
      throw new Error(`Lens viewer uses unsupported reconcile status ${status}`);
    }
  }
  if (!Array.isArray(viewer.inspectorFields)) {
    throw new Error("Lens viewer inspectorFields must be an array");
  }
  viewer.inspectorFields.forEach((field, index) => validateInspectorField(field, index));
}

function validateInspectorField(value: unknown, index: number): void {
  const field = record(value, `Lens inspector field ${index}`);
  onlyKeys(field, new Set(["key", "label", "valueType"]), `Lens inspector field ${index}`);
  nonEmptyString(field.key, `Lens inspector field ${index} key`);
  nonEmptyString(field.label, `Lens inspector field ${index} label`);
  if (!new Set(["string", "number", "boolean", "stringList"]).has(String(field.valueType))) {
    throw new Error(`Lens inspector field ${index} has an unsupported valueType`);
  }
}

function validateResolutionMethods(value: unknown): void {
  if (!Array.isArray(value)) {
    throw new Error("Lens manifest resolutionMethods must be an array");
  }
  const seen = new Set<string>();
  value.forEach((entry, index) => {
    const method = record(entry, `Lens resolution method ${index}`);
    onlyKeys(method, new Set(["method", "scope"]), `Lens resolution method ${index}`);
    if (!new Set(["existing", "incoming", "both", "manual"]).has(String(method.method))) {
      throw new Error(`Lens resolution method ${index} has an unsupported method`);
    }
    if (method.scope !== "file" && method.scope !== "block") {
      throw new Error(`Lens resolution method ${index} has an unsupported scope`);
    }
    const key = `${method.scope}:${method.method}`;
    if (seen.has(key)) {
      throw new Error(`Lens manifest contains duplicate resolution method ${key}`);
    }
    seen.add(key);
  });
}

function validateBundle(value: unknown, manifestDigest: string): void {
  const bundle = record(value, "Lens bundle");
  onlyKeys(bundle, new Set(["format", "entrypoints", "signature"]), "Lens bundle");
  if (bundle.format !== LENS_BUNDLE_FORMAT) {
    throw new Error(`Lens bundle format must be ${LENS_BUNDLE_FORMAT}`);
  }
  const entrypoints = record(bundle.entrypoints, "Lens bundle entrypoints");
  onlyKeys(entrypoints, new Set(["analyzer", "reconcile", "viewer"]), "Lens bundle entrypoints");
  if (
    entrypoints.analyzer === undefined &&
    entrypoints.reconcile === undefined &&
    entrypoints.viewer === undefined
  ) {
    throw new Error("Lens bundle must declare at least one entrypoint");
  }
  if (entrypoints.analyzer !== undefined) {
    validateEntrypoint(entrypoints.analyzer, "wasi", "Lens analyzer entrypoint");
  }
  if (entrypoints.reconcile !== undefined) {
    validateEntrypoint(entrypoints.reconcile, "wasi", "Lens reconcile entrypoint");
  }
  if (entrypoints.viewer !== undefined) {
    validateEntrypoint(entrypoints.viewer, "esm", "Lens viewer entrypoint");
  }
  if (bundle.signature !== undefined) {
    const signature = record(bundle.signature, "Lens bundle signature");
    onlyKeys(signature, new Set(["algorithm", "keyId", "signedDigest", "signature"]), "Lens bundle signature");
    nonEmptyString(signature.algorithm, "Lens bundle signature algorithm");
    nonEmptyString(signature.keyId, "Lens bundle signature keyId");
    assertSha256Digest(signature.signedDigest, "Lens bundle signedDigest");
    if (signature.signedDigest !== manifestDigest) {
      throw new Error("Lens bundle signedDigest must equal the manifest identity digest");
    }
    if (typeof signature.signature !== "string" || !/^base64:[A-Za-z0-9+/]+={0,2}$/.test(signature.signature)) {
      throw new Error("Lens bundle signature must be base64-prefixed data");
    }
  }
}

function validateEntrypoint(value: unknown, runtime: "wasi" | "esm", label: string): void {
  const entrypoint = record(value, label) as unknown as WasiLensEntrypoint | EsmLensEntrypoint;
  onlyKeys(entrypoint as unknown as Record<string, unknown>, new Set(["runtime", "path", "export", "digest", "resources"]), label);
  if (entrypoint.runtime !== runtime) {
    throw new Error(`${label} runtime must be ${runtime}`);
  }
  if (typeof entrypoint.path !== "string" || !BUNDLE_PATH.test(entrypoint.path)) {
    throw new Error(`${label} path must be a safe relative bundle path`);
  }
  nonEmptyString(entrypoint.export, `${label} export`);
  assertSha256Digest(entrypoint.digest, `${label} digest`);
  validateResources(entrypoint.resources, label);
}

function validateResources(value: LensResourceQuotas | unknown, label: string): void {
  const resources = record(value, `${label} resources`);
  onlyKeys(resources, new Set(["maxMemoryBytes", "maxExecutionMs", "maxInputBytes", "maxOutputBytes", "maxFuel"]), `${label} resources`);
  for (const key of ["maxMemoryBytes", "maxExecutionMs", "maxInputBytes", "maxOutputBytes"] as const) {
    positiveInteger(resources[key], `${label} resources ${key}`);
  }
  if (resources.maxFuel !== undefined) {
    positiveInteger(resources.maxFuel, `${label} resources maxFuel`);
  }
}

function record(value: unknown, label: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value as Record<string, unknown>;
}

function onlyKeys(value: Record<string, unknown>, allowed: Set<string>, label: string): void {
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new Error(`${label} contains unknown field ${key}`);
    }
  }
}

function nonEmptyString(value: unknown, label: string): asserts value is string {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${label} must be a non-empty string`);
  }
}

function stringArray(value: unknown, label: string, pattern?: RegExp): string[] {
  if (!Array.isArray(value)) {
    throw new Error(`${label} must be an array`);
  }
  const strings = value.map((entry, index) => {
    nonEmptyString(entry, `${label}[${index}]`);
    if (pattern && !pattern.test(entry)) {
      throw new Error(`${label}[${index}] has an invalid value`);
    }
    return entry;
  });
  if (new Set(strings).size !== strings.length) {
    throw new Error(`${label} must contain unique values`);
  }
  return strings;
}

function positiveInteger(value: unknown, label: string): void {
  if (!Number.isSafeInteger(value) || Number(value) <= 0) {
    throw new Error(`${label} must be a positive safe integer`);
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
