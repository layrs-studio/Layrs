/** Bounded local data only. Each schema supplies closed scope/input adapters. */
export type DraftStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;
export type LocalDraft<Input> = Input & { idempotencyKey: string; updatedAt: number };
export const DRAFT_MAX_BYTES = 64 * 1024;
export const DRAFT_TOTAL_BYTES = 256 * 1024;
export const DRAFT_MAX_ENTRIES = 20;
export const DRAFT_TTL_MS = 30 * 24 * 60 * 60 * 1000;
const encoder = new TextEncoder();
const bytes = (value: unknown) => encoder.encode(JSON.stringify(value)).byteLength;
const fields = (value: object, allowed: string[]) => Object.keys(value).every(key => allowed.includes(key));
const validKey = (value: unknown): value is string => typeof value === "string" && value.length >= 16 && value.length <= 128 && /^[A-Za-z0-9_.:-]+$/.test(value);
const fail = (message: string): never => { throw new Error(message); };

export function createDraftStore<Scope extends object, Input extends object>(schema: string, cleanScope: (value: Scope) => Scope, cleanInput: (value: Input) => Input) {
  type Entry = { scope: Scope; draft: LocalDraft<Input> };
  const key = (scope: Scope) => JSON.stringify(scope);
  function load(storage: DraftStorage, now: number): Entry[] {
    const raw = storage.getItem(schema);
    if (raw === null) return [];
    if (raw.length > DRAFT_TOTAL_BYTES || encoder.encode(raw).byteLength > DRAFT_TOTAL_BYTES) fail("Stockage des brouillons trop volumineux.");
    let value: { schema: string; entries: Entry[] };
    try { value = JSON.parse(raw); } catch { return fail("Stockage des brouillons illisible."); }
    if (!value || typeof value !== "object" || !fields(value, ["schema", "entries"]) || value.schema !== schema || !Array.isArray(value.entries) || value.entries.length > DRAFT_MAX_ENTRIES) fail("Format des brouillons incompatible.");
    const entries: Entry[] = [];
    const seen = new Set<string>();
    for (const entry of value.entries) {
      if (!entry || typeof entry !== "object" || !fields(entry, ["scope", "draft"]) || !entry.scope || !entry.draft) fail("Brouillon invalide.");
      const scope = cleanScope(entry.scope);
      const input = cleanInput(entry.draft);
      const draft = entry.draft;
      if (!fields(entry.scope, Object.keys(scope)) || !fields(draft, [...Object.keys(input), "idempotencyKey", "updatedAt"])
        || !Number.isSafeInteger(draft.updatedAt) || draft.updatedAt < 0 || !validKey(draft.idempotencyKey)
        || bytes(entry) > DRAFT_MAX_BYTES || seen.has(key(scope))) fail("Brouillon invalide.");
      seen.add(key(scope));
      if (now - draft.updatedAt <= DRAFT_TTL_MS && draft.updatedAt <= now + 300000) entries.push({ scope, draft: { ...input, updatedAt: draft.updatedAt, idempotencyKey: draft.idempotencyKey } });
    }
    return entries;
  }
  function persist(storage: DraftStorage, entries: Entry[]) {
    if (!entries.length) { storage.removeItem(schema); return; }
    const value = { schema, entries };
    if (entries.length > DRAFT_MAX_ENTRIES || bytes(value) > DRAFT_TOTAL_BYTES) fail("Limite locale atteinte : supprimez un ancien brouillon dans sa revue.");
    storage.setItem(schema, JSON.stringify(value));
  }
  return {
    scopes(storage: DraftStorage, now = Date.now()): Scope[] { return load(storage, now).map(entry => entry.scope); },
    read(storage: DraftStorage, scope: Scope, now = Date.now()): LocalDraft<Input> | null {
      const wanted = key(cleanScope(scope));
      return load(storage, now).find(entry => key(entry.scope) === wanted)?.draft ?? null;
    },
    save(storage: DraftStorage, scope: Scope, value: Input, now = Date.now(), newKey = () => crypto.randomUUID()): LocalDraft<Input> {
      const canonical = cleanScope(scope);
      const wanted = key(canonical);
      const clean = cleanInput(value);
      const entries = load(storage, now);
      const previous = entries.find(entry => key(entry.scope) === wanted)?.draft;
      const same = previous && JSON.stringify(cleanInput(previous)) === JSON.stringify(clean);
      const draft = { ...clean, updatedAt: now, idempotencyKey: same ? previous.idempotencyKey : newKey() };
      if (!Number.isSafeInteger(now) || now < 0 || !validKey(draft.idempotencyKey)) fail("Horloge ou identifiant du brouillon invalide.");
      const entry = { scope: canonical, draft };
      if (bytes(entry) > DRAFT_MAX_BYTES) fail("Brouillon trop volumineux pour la sauvegarde locale.");
      persist(storage, [...entries.filter(entry => key(entry.scope) !== wanted), entry]);
      return draft;
    },
    remove(storage: DraftStorage, scope: Scope, expectedKey?: string, now = Date.now()): boolean {
      const wanted = key(cleanScope(scope));
      const entries = load(storage, now);
      const current = entries.find(entry => key(entry.scope) === wanted);
      if (!current || (expectedKey && current.draft.idempotencyKey !== expectedKey)) return false;
      persist(storage, entries.filter(entry => key(entry.scope) !== wanted));
      return true;
    },
  };
}
