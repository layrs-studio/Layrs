export type PartialMergeChoice = "base" | "local" | "remote" | "delete";
type Draft = {files: {path: string; conflict: boolean}[]};
type Binding = {key: string; digest: string};
type Cache = Pick<Storage, "getItem" | "setItem" | "removeItem">;
const limit = 1024 * 1024;

// Native responses and JSON values can have different object key order.
function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value !== null && typeof value === "object") {
    const object = value as Record<string, unknown>;
    return `{${Object.keys(object).sort().map(key => `${JSON.stringify(key)}:${canonical(object[key])}`).join(",")}}`;
  }
  const encoded = JSON.stringify(value);
  if (encoded === undefined) throw new Error("Invalid merge draft");
  return encoded;
}
async function digest(value: unknown): Promise<string> {
  const hash = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(canonical(value)));
  return Array.from(new Uint8Array(hash), byte => byte.toString(16).padStart(2, "0")).join("");
}
export async function bindPartialMergeChoices(root: string, origin: string, preview: Draft): Promise<Binding> {
  return {key: `layrs.partial-merge-choices.v1.${await digest([root, origin])}`, digest: await digest(preview)};
}
function validate(choices: unknown, preview: Draft): Record<string, PartialMergeChoice> {
  if (!choices || typeof choices !== "object" || Array.isArray(choices)) throw new Error("Invalid saved merge choices");
  const conflicts = new Set(preview.files.filter(file => file.conflict).map(file => file.path));
  const result: Record<string, PartialMergeChoice> = Object.create(null);
  for (const [path, choice] of Object.entries(choices)) {
    if (!conflicts.has(path) || !["base", "local", "remote", "delete"].includes(choice as string)) throw new Error("Invalid saved merge choice");
    result[path] = choice as PartialMergeChoice;
  }
  return result;
}
export function loadPartialMergeChoices(cache: Cache, binding: Binding, preview: Draft): Record<string, PartialMergeChoice> {
  const raw = cache.getItem(binding.key);
  if (raw === null) return {};
  if (raw.length > limit) throw new Error("Saved merge choices exceed the size limit");
  const record = JSON.parse(raw);
  if (record?.schema !== 1 || record.digest !== binding.digest) return {};
  return validate(record.choices, preview);
}
export function savePartialMergeChoices(cache: Cache, binding: Binding, preview: Draft, choices: Record<string, PartialMergeChoice>): void {
  const raw = JSON.stringify({schema: 1, digest: binding.digest, choices: validate(choices, preview)});
  if (raw.length > limit) throw new Error("Merge choices exceed the size limit");
  cache.setItem(binding.key, raw);
}
export function clearPartialMergeChoices(cache: Cache, binding: Binding): void {
  const raw = cache.getItem(binding.key);
  // Another open checkout window may have saved a newer draft meanwhile.
  if (raw !== null && raw.length <= limit && JSON.parse(raw)?.digest === binding.digest) cache.removeItem(binding.key);
}
