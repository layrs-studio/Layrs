/** Native projection DTOs. These are not the legacy LayerAccessPolicy API. */
export type NativeProjectionAnchor = {
  origin: string; principalId: string; spaceId: string; stepId: string; rootTreeId: string;
  policySetId: string; authorizationEpoch: number; projectionId: string; generation: number;
};
export type NativeProjectFile = {
  path: string; kind: "file" | "directory" | "symlink" | "conflict" | "tombstone" | "absent";
  contentState: "materialized" | "not_materialized" | "reserved" | "unavailable" | "absent";
  access: "read_write" | "read_only" | "denied" | "unknown";
  objectId?: string;
};
export type NativePathReservation = { path: string; reason: "content_denied" | "local_preserved" | "collision" | "unknown" };
export type NativeProjectFilesSnapshot = {
  schemaVersion: "layrs.project-files.v1";
  anchor: NativeProjectionAnchor;
  state: "ready" | "stale" | "incomplete";
  caseMode: "sensitive" | "insensitive";
  pathPlatform: "windows" | "posix";
  /** Complete namespace and reservation registry, not just the visible page. */
  namespaceComplete: boolean;
  entries: NativeProjectFile[];
  reservedPaths: NativePathReservation[];
};
export type NativeNamespaceMutation =
  | { kind: "create"; destination: string }
  | { kind: "rename" | "move"; source: string; destination: string };
export type NativeNamespacePreviewRequest = { anchor: NativeProjectionAnchor; mutation: NativeNamespaceMutation };
export type NativeNamespaceCheck = { allowed: true } | { allowed: false; code: "projection_unavailable" | "invalid_path" | "reserved_path" | "destination_exists" | "source_unavailable"; message: string };
const deny = (code: Exclude<NativeNamespaceCheck, {allowed: true}>["code"], message: string): NativeNamespaceCheck => ({allowed:false, code, message});
const MAX_ENTRIES = 100_000;

/** Conservative UI check; the filesystem engine must revalidate under its commit boundary. */
export function nativePathKey(path: string, platform: "windows" | "posix", caseMode: "sensitive" | "insensitive"): string | null {
  if (typeof path !== "string" || !path || new TextEncoder().encode(path).length > 4096 || /[\\\x00-\x1f\x7f]/.test(path)) return null;
  const parts = path.split("/");
  if (parts.length > 128 || path.normalize("NFC") !== path || parts.some(p => !p || p === "." || p === ".." || [".layrs", ".git"].includes(p.toLowerCase()))) return null;
  if (platform === "windows" && parts.some(p => p.length > 255 || /[<>:"|?*~]/.test(p) || /[. ]$/.test(p) || /^(con|prn|aux|nul|clock\$|conin\$|conout\$|com[1-9¹²³]|lpt[1-9¹²³])$/i.test(p.split(".")[0].replace(/ +$/, "")))) return null;
  // NFC/case folding intentionally also refuses ambiguous spellings conservatively.
  const normalized = path.normalize("NFC");
  return caseMode === "insensitive" ? normalized.toUpperCase() : normalized;
}
const intersects = (a: string, b: string) => a === b || a.startsWith(`${b}/`) || b.startsWith(`${a}/`);

export function nativeProjectionReady(snapshot: NativeProjectFilesSnapshot | null): boolean {
  if (!snapshot || snapshot.schemaVersion !== "layrs.project-files.v1" || snapshot.state !== "ready" || snapshot.namespaceComplete !== true || !["windows","posix"].includes(snapshot.pathPlatform) || !["sensitive","insensitive"].includes(snapshot.caseMode)) return false;
  const a = snapshot.anchor;
  if (!a || ![a.origin,a.principalId,a.spaceId,a.stepId,a.rootTreeId,a.policySetId,a.projectionId].every(v => typeof v === "string" && v.length > 0 && v.length <= 4096) || !Number.isSafeInteger(a.authorizationEpoch) || a.authorizationEpoch < 0 || !Number.isSafeInteger(a.generation) || a.generation < 0) return false;
  if (!Array.isArray(snapshot.entries) || !Array.isArray(snapshot.reservedPaths) || snapshot.entries.length + snapshot.reservedPaths.length > MAX_ENTRIES) return false;
  const keys = new Set<string>();
  const prefixes = new Map<string,string>();
  for (const entry of snapshot.entries) {
    if (!entry || !["file","directory","symlink","conflict","tombstone","absent"].includes(entry.kind) || !["materialized","not_materialized","reserved","absent"].includes(entry.contentState) || !["read_write","read_only","denied"].includes(entry.access)) return false;
    if((entry.kind==="absent")!==(entry.contentState==="absent")||(entry.kind==="absent"&&entry.access!=="read_write"))return false;
    const key = nativePathKey(entry.path, snapshot.pathPlatform, snapshot.caseMode);
    if (!key || keys.has(key) || (entry.access === "denied" && entry.contentState === "materialized")) return false;
    keys.add(key);
    const parts=entry.path.split("/");
    for(let n=1;n<=parts.length;n++){const prefix=parts.slice(0,n).join("/");const folded=nativePathKey(prefix,snapshot.pathPlatform,snapshot.caseMode)!;if(prefixes.has(folded)&&prefixes.get(folded)!==prefix)return false;prefixes.set(folded,prefix);}
  }
  return snapshot.reservedPaths.every(r => r && ["content_denied","local_preserved","collision"].includes(r.reason) && nativePathKey(r.path,snapshot.pathPlatform,snapshot.caseMode) !== null);
}

export function checkNativeNamespaceMutation(snapshot: NativeProjectFilesSnapshot | null, mutation: NativeNamespaceMutation): NativeNamespaceCheck {
  if (!nativeProjectionReady(snapshot)) return deny("projection_unavailable", "Projection périmée ou incomplète : actualisez avant de modifier les chemins.");
  const s = snapshot!;
  const key = (path: string) => nativePathKey(path,s.pathPlatform,s.caseMode);
  const destination = key(mutation.destination);
  if (!destination || !["create","rename","move"].includes(mutation.kind)) return deny("invalid_path", "Chemin invalide ou alias ambigu pour ce système.");
  const protectedPaths = [...s.reservedPaths.map(r => key(r.path)!), ...s.entries.filter(e => e.access !== "read_write" || !["materialized","absent"].includes(e.contentState)).map(e => key(e.path)!)];
  if (protectedPaths.some(p => intersects(p,destination))) return deny("reserved_path", "Ce chemin, un parent ou un descendant est réservé ou non modifiable.");
  if (s.entries.some(e => {const existing = key(e.path)!; if(e.kind==="absent"&&existing===destination)return false; return existing === destination || existing.startsWith(`${destination}/`) || (e.kind !== "directory" && destination.startsWith(`${existing}/`));})) return deny("destination_exists", "La destination existe ou entre en collision avec un fichier.");
  if (mutation.kind !== "create") {
    const source = key(mutation.source);
    if (!source) return deny("invalid_path", "Chemin source invalide.");
    const entry = s.entries.find(e => key(e.path) === source);
    if (!entry || entry.access !== "read_write" || entry.contentState !== "materialized" || protectedPaths.some(p => intersects(p,source)) || intersects(source,destination)) return deny("source_unavailable", "La source n’est pas entièrement modifiable ou le déplacement se recouvre.");
  }
  return {allowed:true};
}

export function nativeProjectFileLabel(entry: NativeProjectFile): string {
  if(entry.kind==="absent"&&entry.contentState==="absent"&&entry.access==="read_write")return "Absence observée · création à revalider avant application";
  if (entry.access === "denied" && entry.contentState === "materialized") return "État incohérent · actualisation nécessaire";
  if (entry.access === "denied") return entry.contentState === "reserved" ? "🔒 Chemin réservé · contenu interdit" : "🔒 Contenu non accessible · non matérialisé";
  if (entry.contentState === "reserved") return "🔒 Chemin réservé";
  if (entry.access === "unknown" || entry.contentState === "unavailable") return "État incomplet · actualisation nécessaire";
  if (entry.contentState === "not_materialized") return "Contenu non matérialisé";
  if (entry.contentState !== "materialized" || !["read_write","read_only"].includes(entry.access)) return "État inconnu · actualisation nécessaire";
  return entry.access === "read_only" ? "Lecture seule" : "Matérialisé · modifiable";
}
