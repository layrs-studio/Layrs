import type { LensPin } from "./contracts";

const SHA256_DIGEST = /^sha256:[0-9a-f]{64}$/;

export function isSha256Digest(value: unknown): value is string {
  return typeof value === "string" && SHA256_DIGEST.test(value);
}

export function assertSha256Digest(value: unknown, label = "digest"): asserts value is string {
  if (!isSha256Digest(value)) {
    throw new Error(`${label} must match sha256: followed by 64 lowercase hexadecimal characters`);
  }
}

export function assertLensPin(pin: LensPin): void {
  if (!pin || typeof pin !== "object") {
    throw new Error("Lens pin must be an object");
  }
  assertSha256Digest(pin.digest, "Lens pin digest");
}
