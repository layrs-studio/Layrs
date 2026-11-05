/*
 * Object identities the server recomputes: evidence, Policy products,
 * PolicySets and request digests. The server verifies every identity, so a
 * mismatch is a refusal, never a grant.
 */
import { blake3 } from "@noble/hashes/blake3";

export function hex(bytes: Uint8Array) {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

const encoder = new TextEncoder();

/** `blake3:<hex>` of a domain label, a NUL separator (when given) and the bytes. */
export function objectId(domain: string, bytes: Uint8Array, separator = true) {
  const prefix = encoder.encode(domain);
  const input = new Uint8Array(prefix.length + (separator ? 1 : 0) + bytes.length);
  input.set(prefix, 0);
  input.set(bytes, prefix.length + (separator ? 1 : 0));
  return `blake3:${hex(blake3(input))}`;
}
