/*
 * RFC 8785 canonical JSON for the closed documents this client signs: objects,
 * arrays, strings, booleans, null and safe non-negative integers. Anything
 * else (floats, huge numbers, undefined) is refused instead of approximated.
 */

export function canonicalJson(value: unknown): string {
  if (value === null || typeof value === "boolean") return JSON.stringify(value);
  if (typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value)) throw new Error("Nombre non canonique refusé.");
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>);
    if (entries.some(([, item]) => item === undefined)) throw new Error("Champ indéfini refusé.");
    // Default string order compares UTF-16 code units, as RFC 8785 requires.
    entries.sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0));
    return `{${entries.map(([key, item]) => `${JSON.stringify(key)}:${canonicalJson(item)}`).join(",")}}`;
  }
  throw new Error("Valeur JSON non canonique refusée.");
}

const encoder = new TextEncoder();

export function canonicalBytes(value: unknown) {
  return encoder.encode(canonicalJson(value));
}
