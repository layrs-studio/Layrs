import type { LensPin } from "./contracts";

const SHA256_DIGEST = /^sha256:[0-9a-f]{64}$/;
const LENS_ID = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;
const SEMVER = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;

export function isSha256Digest(value: unknown): value is string {
  return typeof value === "string" && SHA256_DIGEST.test(value);
}

export function assertSha256Digest(value: unknown, label = "digest"): asserts value is string {
  if (!isSha256Digest(value)) {
    throw new Error(`${label} must match sha256: followed by 64 lowercase hexadecimal characters`);
  }
}

export function isLensId(value: unknown): value is string {
  return typeof value === "string" && value.length <= 128 && LENS_ID.test(value);
}

export function assertLensId(value: unknown, label = "Lens id"): asserts value is string {
  if (!isLensId(value)) {
    throw new Error(`${label} must start with an ASCII letter or digit and use only letters, digits, dots, underscores, or dashes`);
  }
}

export function isSemanticVersion(value: unknown): value is string {
  return typeof value === "string" && SEMVER.test(value);
}

export function assertSemanticVersion(value: unknown, label = "version"): asserts value is string {
  if (!isSemanticVersion(value)) {
    throw new Error(`${label} must be a valid semantic version`);
  }
}

export function assertLensPin(pin: unknown): asserts pin is LensPin {
  if (!pin || typeof pin !== "object") {
    throw new Error("Lens pin must be an object");
  }
  const record = pin as Record<string, unknown>;
  for (const key of Object.keys(record)) {
    if (key !== "lensId" && key !== "version" && key !== "digest") {
      throw new Error(`Lens pin contains unknown field ${key}`);
    }
  }
  assertLensId(record.lensId, "Lens pin lensId");
  assertSemanticVersion(record.version, "Lens pin version");
  assertSha256Digest(record.digest, "Lens pin digest");
}

export function lensPinsEqual(left: LensPin, right: LensPin): boolean {
  return left.lensId === right.lensId && left.version === right.version && left.digest === right.digest;
}
