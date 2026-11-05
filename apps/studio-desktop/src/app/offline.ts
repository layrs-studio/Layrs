import { nativePathKey, type NativeCapturedProjectionPatch, type NativeProjectionStatus } from "@layrs/ui";

export type Version =
  | { kind: "generation"; worktreeId: string; generation: number; digest: string }
  | { kind: "capture"; worktreeId: string; sequence: number; digest: string }
  | { kind: "remote"; stepId: string; rootTreeId: string; policySetId: string; authorizationEpoch: number; observationDigest: string };
export type Operation = "read" | "edit" | "compareMetadata" | "compareContent";
export type AvailabilityRequest = { version: Version; other: Version | null; paths: string[]; operation: Operation };
export const blockers = {
  unknownPath: "Chemin inconnu", missingContent: "Contenu absent du stockage local", protectedPath: "Accès protégé par les droits",
  excludedPath: "Exclu de la sélection", notMaterialized: "Non matérialisé", unsupportedTarget: "Type non pris en charge",
  pendingOperation: "Opération en attente", quota: "Quota insuffisant", changedCheckout: "Dossier modifié depuis l’observation",
} as const;
export type PathAvailability = {
  path: string; operand: number; absent: boolean; fileId: string | null; manifestId: string | null;
  selected: boolean; observedWritable: boolean; materialized: boolean; retained: boolean; blockers: (keyof typeof blockers)[];
};
export type Availability = { request: AvailabilityRequest; effectivePaths: string[]; paths: PathAvailability[]; blockers: (keyof typeof blockers)[]; available: boolean; prepared: boolean };
export type Preparation = { id: string; request: AvailabilityRequest; status: "acquiring" | "ready" | "released" };
export type Difference = { path: string; before: string | null; after: string | null; beforeExecutable: boolean; afterExecutable: boolean };
export type Usage = { contentBytes: number; reservedBytes: number; limitBytes: number; protectionRoots: number; windowsPowerLossQualified: boolean };
export const previewLimit = 1024 * 1024;
const rawDigest = (v: unknown): v is string => typeof v === "string" && /^[a-f0-9]{64}$/.test(v);
const objectId = (v: unknown): v is string => typeof v === "string" && /^blake3:[a-f0-9]{64}$/.test(v);
const integer = (v: unknown): v is number => Number.isSafeInteger(v) && (v as number) >= 0;
const text = (v: unknown): v is string => typeof v === "string" && v.length > 0 && v.length <= 4096;
const fail = (message = "Réponse hors ligne invalide. Actualisez avant de poursuivre."): never => { throw new Error(message); };
const record = (v: unknown): Record<string, unknown> => v && typeof v === "object" && !Array.isArray(v) ? v as Record<string, unknown> : fail();

export function version(value: unknown): Version {
  const v = record(value);
  if (v.kind === "generation" && text(v.worktreeId) && integer(v.generation) && rawDigest(v.digest))
    return { kind: v.kind, worktreeId: v.worktreeId, generation: v.generation, digest: v.digest };
  if (v.kind === "capture" && text(v.worktreeId) && integer(v.sequence) && v.sequence > 0 && objectId(v.digest))
    return { kind: v.kind, worktreeId: v.worktreeId, sequence: v.sequence, digest: v.digest };
  if (v.kind === "remote" && objectId(v.stepId) && objectId(v.rootTreeId) && objectId(v.policySetId) && integer(v.authorizationEpoch) && v.authorizationEpoch > 0 && rawDigest(v.observationDigest))
    return { kind: v.kind, stepId: v.stepId, rootTreeId: v.rootTreeId, policySetId: v.policySetId, authorizationEpoch: v.authorizationEpoch, observationDigest: v.observationDigest };
  return fail("Version exacte invalide : l’identifiant et son empreinte doivent provenir du moteur.");
}
export const versionKey = (v: Version) => JSON.stringify(version(v));
export function versionLabel(v: Version) {
  return v.kind === "generation" ? `Génération ${v.generation}` : v.kind === "capture" ? `Capture ${v.sequence}` : `Observation distante · ${v.stepId.slice(0, 19)}…`;
}
export function currentVersion(reply: unknown, status: NativeProjectionStatus): Version {
  const r = record(reply), v = version(r.version);
  if (r.shared !== true || v.kind !== "generation" || v.worktreeId !== status.identity.worktreeId || v.generation !== status.projection.generation || v.digest !== status.projection.digest)
    return fail("La version courante a changé. Actualisez les fichiers avant de recommencer.");
  return v;
}
export function historyVersions(status: NativeProjectionStatus, history: NativeCapturedProjectionPatch[]): Version[] {
  const result = new Map<string, Version>();
  for (const patch of history) {
    // Imported history keeps source provenance; native resolution binds its exact
    // sequence/digest to the currently opened Worktree, not that source checkout.
    for (const v of [
      version({ kind: "capture", worktreeId: status.identity.worktreeId, sequence: patch.sequence, digest: patch.digest }),
      version({ kind: "generation", worktreeId: status.identity.worktreeId, generation: patch.projectionGeneration, digest: patch.projectionDigest }),
    ]) result.set(versionKey(v), v);
  }
  return [...result.values()];
}
function utf8Compare(a: string, b: string) {
  const x = new TextEncoder().encode(a), y = new TextEncoder().encode(b);
  for (let i = 0; i < Math.min(x.length, y.length); i++) if (x[i] !== y[i]) return x[i] - y[i];
  return x.length - y.length;
}
export function exactPaths(input: string, semantics: "case_sensitive" | "portable_windows"): string[] {
  const paths = input.split(/\r?\n/).filter(path => path !== "");
  if (!paths.length || paths.length > 1024) return fail("Indiquez entre 1 et 1 024 chemins exacts, un par ligne.");
  const keys = new Set<string>();
  for (const path of paths) {
    const key = nativePathKey(path, semantics === "portable_windows" ? "windows" : "posix", semantics === "portable_windows" ? "insensitive" : "sensitive");
    if (!key || /[*?\[\]{}]/.test(path) || keys.has(key)) return fail("Chemin invalide, motif ou doublon : utilisez des chemins exacts distincts.");
    keys.add(key);
  }
  return paths.sort(utf8Compare);
}
export function availabilityRequest(v: Version, other: Version | null, paths: string[], operation: Operation): AvailabilityRequest {
  if (!["read", "edit", "compareMetadata", "compareContent"].includes(operation)) return fail("Cette opération ne peut pas être préparée hors ligne.");
  if (operation.startsWith("compare") !== Boolean(other)) return fail("La comparaison requiert deux versions exactes.");
  const ordered = exactPaths(paths.join("\n"), "case_sensitive");
  if (JSON.stringify(ordered) !== JSON.stringify(paths)) return fail("Les chemins doivent être uniques et triés.");
  return { version: version(v), other: other ? version(other) : null, paths, operation };
}
function request(value: unknown): AvailabilityRequest {
  const r = record(value);
  if (!Array.isArray(r.paths) || !r.paths.every(text)) return fail();
  return availabilityRequest(version(r.version), r.other === null ? null : version(r.other), r.paths as string[], r.operation as Operation);
}
function blockList(value: unknown): (keyof typeof blockers)[] {
  if (!Array.isArray(value) || value.length > 9 || !value.every(v => typeof v === "string" && Object.hasOwn(blockers, v))) return fail();
  return value;
}
export function availability(value: unknown, expected: AvailabilityRequest): Availability {
  const r = record(value), actual = request(r.request);
  if (JSON.stringify(actual) !== JSON.stringify(expected) || typeof r.available !== "boolean" || typeof r.prepared !== "boolean" || !Array.isArray(r.paths) || r.paths.length > 2048 || !Array.isArray(r.effectivePaths) || !r.effectivePaths.every(text)) return fail();
  const seen = new Set<string>();
  const paths = r.paths.map(raw => {
    const row = record(raw), key = `${row.operand}:${row.path}`;
    if (!text(row.path) || !expected.paths.includes(row.path) || ![0, ...(expected.other ? [1] : [])].includes(row.operand as number) || seen.has(key) || !["absent", "selected", "observedWritable", "materialized", "retained"].every(k => typeof row[k] === "boolean") || ![row.fileId, row.manifestId].every(id => id === null || objectId(id))) return fail();
    seen.add(key);
    return { ...row, blockers: blockList(row.blockers) } as PathAvailability;
  });
  const blocked = blockList(r.blockers);
  if (paths.length !== expected.paths.length * (expected.other ? 2 : 1) || JSON.stringify(r.effectivePaths) !== JSON.stringify(expected.paths) || r.available !== (blocked.length === 0 && paths.every(p => p.blockers.length === 0)) || r.prepared && !r.available) return fail();
  return { request: actual, paths, effectivePaths: r.effectivePaths as string[], blockers: blocked, available: r.available, prepared: r.prepared };
}
export function preparation(value: unknown, expected?: AvailabilityRequest): Preparation {
  const r = record(value), actual = request(r.request);
  if (!text(r.id) || r.id.length > 256 || !["acquiring", "ready", "released"].includes(r.status as string) || expected && JSON.stringify(actual) !== JSON.stringify(expected)) return fail();
  return { id: r.id, request: actual, status: r.status as Preparation["status"] };
}
export function preparations(value: unknown): Preparation[] {
  const r = record(value);
  if (!Array.isArray(r.preparations) || r.preparations.length > 1088) return fail();
  const result = r.preparations.map(v => preparation(v));
  const released = result.filter(p => p.status === "released").length;
  if (released > 64 || result.length - released > 1024 || new Set(result.map(p => p.id)).size !== result.length) return fail();
  return result;
}
export function preparedReply(value: unknown, expected: AvailabilityRequest, id?: string): Preparation {
  const r = record(value), p = preparation(r.preparation, expected);
  if (r.status !== "projectionOfflinePrepared" || p.status === "released" || id && p.id !== id) return fail();
  return p;
}
export function observedReply(value: unknown, step: string): Version {
  const r = record(value), v = version(r.version);
  if (r.status !== "projectionVersionObserved" || v.kind !== "remote" || v.stepId !== step) return fail();
  return v;
}
export function networkRequest(status: NativeProjectionStatus, expectedGeneration: number) {
  if (status.offline?.shared !== true || status.needsRepair || status.pendingPublication || !integer(expectedGeneration) || expectedGeneration !== status.projection.generation) return fail("État local modifié ou indisponible. Actualisez avant cette action réseau.");
  return { expectedGeneration };
}
export function exactStep(value: string): string {
  if (!objectId(value)) return fail("Indiquez un Step exact (blake3: suivi de 64 caractères hexadécimaux).");
  return value;
}
export function preview(value: unknown, expected: Version, path: string): { bytes: number; text: string | null } {
  const r = record(value);
  if (r.path !== path || versionKey(version(r.version)) !== versionKey(expected) || r.encoding !== "base64" || !integer(r.bytes) || r.bytes > previewLimit || typeof r.content !== "string" || r.content.length !== 4 * Math.ceil(r.bytes / 3) || /[^A-Za-z0-9+/=]/.test(r.content)) return fail("Aperçu invalide ou supérieur à 1 Mio.");
  let binary: string;
  try { binary = atob(r.content); } catch { return fail("Aperçu base64 invalide."); }
  if (binary.length !== r.bytes || btoa(binary) !== r.content) return fail();
  const bytes = Uint8Array.from(binary, c => c.charCodeAt(0));
  try {
    const decoded = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    return { bytes: bytes.length, text: /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(decoded) ? null : decoded };
  } catch { return { bytes: bytes.length, text: null }; }
}
export function differences(value: unknown, expected: AvailabilityRequest): Difference[] {
  const rows = record(value).differences;
  if (!Array.isArray(rows) || rows.length > expected.paths.length) return fail();
  const seen = new Set<string>();
  return rows.map(value => {
    const r = record(value);
    if (!text(r.path) || !expected.paths.includes(r.path) || seen.has(r.path) || ![r.before, r.after].every(id => id === null || objectId(id)) || typeof r.beforeExecutable !== "boolean" || typeof r.afterExecutable !== "boolean") return fail();
    seen.add(r.path);
    return r as Difference;
  });
}
export function quotaBytes(value: string): number {
  if (!/^(0|[1-9]\d*)$/.test(value) || !integer(Number(value))) return fail("Indiquez un nombre entier d’octets, inférieur ou égal à 9 007 199 254 740 991.");
  return Number(value);
}
export function usage(value: unknown): Usage {
  const r = record(record(value).usage);
  if (!["contentBytes", "reservedBytes", "limitBytes", "protectionRoots"].every(k => integer(r[k])) || typeof r.windowsPowerLossQualified !== "boolean") return fail();
  return r as Usage;
}
function pathKey(value: string) { return value.replace(/^\\\\\?\\/, "").replace(/\\/g, "/").replace(/\/$/, "").toLowerCase(); }
export type ConversionSource = { identity: NativeProjectionStatus["identity"] };
export const recoveryReasons = {
  familyRollback: "Base du stockage partagé à reprendre", worktreeRollback: "Base du Worktree à reprendre",
  initializing: "Initialisation du dossier inachevée", conversion: "Conversion inachevée",
  filesystemOperation: "Opération sur les fichiers à reprendre", publication: "Publication à confirmer",
} as const;
export type RecoveryReason = keyof typeof recoveryReasons;
export type RecoveryBinding = { identity: NativeProjectionStatus["identity"]; checkoutIdentity: string; markerDigest: string; databaseIdentity: string; familyMarkerDigest: string | null };
export type ConversionInspection = ConversionSource & { binding: RecoveryBinding; shared: boolean; recovery: RecoveryReason[]; conversion: { allowed: boolean; reason: string | null } };
export function conversionInspection(value: unknown): ConversionInspection {
  const r = record(value);
  const invalid = () => fail("Inspection locale invalide. Le dossier reste fermé.");
  if (!r.identity || !r.binding || !r.conversion || !Array.isArray(r.recovery) || r.recovery.length > 6 || new Set(r.recovery).size !== r.recovery.length || !r.recovery.every(v => typeof v === "string" && Object.hasOwn(recoveryReasons, v))) return invalid();
  const identity = record(r.identity), permission = record(r.conversion);
  if (!["storeId", "worktreeId", "spaceId", "origin", "principalId"].every(k => text(identity[k])) || typeof r.shared !== "boolean" || typeof permission.allowed !== "boolean" || (permission.allowed ? r.shared || r.recovery.length > 0 || permission.reason !== null : !text(permission.reason))) return invalid();
  const binding = record(r.binding), boundIdentity = record(binding.identity);
  const identityKeys = ["storeId", "worktreeId", "spaceId", "origin", "principalId"];
  if (Object.keys(binding).length !== 5 || !["identity", "checkoutIdentity", "markerDigest", "databaseIdentity", "familyMarkerDigest"].every(k => Object.hasOwn(binding, k)) || Object.keys(identity).length !== 5 || Object.keys(boundIdentity).length !== 5 || !identityKeys.every(k => boundIdentity[k] === identity[k]) || ![binding.checkoutIdentity, binding.databaseIdentity].every(v => typeof v === "string" && v.length > 0 && new TextEncoder().encode(v).length <= 512) || !objectId(binding.markerDigest) || (r.shared ? !objectId(binding.familyMarkerDigest) : binding.familyMarkerDigest !== null)) return invalid();
  return r as ConversionInspection;
}
export function databaseRecoveryRequest(inspection: ConversionInspection) {
  if (!conversionInspection(inspection).recovery.some(r => r === "familyRollback" || r === "worktreeRollback")) return fail("L’inspection ne demande aucune reprise de base de données.");
  return { action: "recoverDatabases", expected: inspection.binding };
}
export function filesystemRecoveryRequest(inspection: ConversionInspection, action: string) {
  if (recoveryActionBlocker(inspection, "filesystem") || !["resumeInstallation", "resumeNamespace", "resumeRefresh", "resumeRestore", "resumeRebase"].includes(action)) return fail("L’inspection ne permet pas cette reprise de fichiers.");
  return { action, expected: inspection.binding };
}
export function reservationRecoveryRequest(inspection: ConversionInspection) {
  if (recoveryActionBlocker(inspection, "reservation")) return fail("L’inspection ne permet pas cette annulation de réservation.");
  return { action: "cancelSharedReservation", expected: inspection.binding };
}
export function recoveryActionBlocker(inspection: ConversionInspection | null, action: "filesystem" | "reservation"): string | undefined {
  if (!inspection) return "Une inspection valide du dossier est nécessaire avant cette reprise.";
  let source: ConversionInspection;
  try { source = conversionInspection(inspection); }
  catch { return "Une inspection valide du dossier est nécessaire avant cette reprise."; }
  if (action === "reservation") {
    if (!source.shared) return "Cette action nécessite un stockage partagé.";
    if (source.recovery.length) return "Traitez les besoins de reprise signalés avant d’annuler cette réservation.";
  } else {
    if (!source.recovery.includes("filesystemOperation")) return "Aucune opération sur les fichiers à reprendre n’est signalée.";
    if (source.recovery.some(r => ["familyRollback", "worktreeRollback", "initializing", "conversion"].includes(r))) return "Reprenez les bases, l’initialisation ou la conversion avant cette opération sur les fichiers.";
  }
}
export function initializationRecoveryRequest(inspection: ConversionInspection) {
  const { recovery } = conversionInspection(inspection);
  if (recovery.length !== 1 || recovery[0] !== "initializing") return fail("La reprise de l’initialisation exige de traiter les autres besoins d’abord.");
  return { action: "resumeInitialization", expected: inspection.binding };
}
export function recoveredInspection(reply: unknown, before: ConversionInspection): ConversionInspection {
  const next = conversionInspection(reply);
  const sameBinding = (["checkoutIdentity", "databaseIdentity", "markerDigest", "familyMarkerDigest"] as const).every(k => next.binding[k] === before.binding[k]);
  if (!sameBinding || next.shared !== before.shared || !["storeId", "worktreeId", "spaceId", "origin", "principalId"].every(k => next.identity[k as keyof typeof next.identity] === before.identity[k as keyof typeof before.identity])) return fail("La reprise ne correspond pas au dossier inspecté.");
  return next;
}
export function createdFamily(value: unknown, expectedPath: string, status: ConversionSource): string {
  const reply = record(value), identity = record(reply.identity);
  usage(reply);
  if (!text(reply.path) || pathKey(reply.path) !== pathKey(expectedPath) || !text(identity.storeId) || !["spaceId", "origin", "principalId"].every(key => identity[key] === status.identity[key as keyof typeof status.identity])) return fail("Identité ou dossier du stockage créé incohérent.");
  return reply.path;
}
export function releaseGenerationRequest(input: string, status: NativeProjectionStatus) {
  if (!/^(0|[1-9]\d*)$/.test(input) || !integer(Number(input))) return fail("Indiquez un numéro de génération entier et exact.");
  const generation = Number(input);
  if (status.offline?.shared !== true || status.needsRepair || status.pendingPublication) return fail("Actualisez ou reprenez le dossier partagé avant de libérer une protection.");
  if (generation >= status.projection.generation) return fail("Choisissez une génération antérieure à la base courante ; celle-ci reste protégée.");
  return { action: "releaseGenerationRetention", generation };
}
export function releasedGeneration(value: unknown, generation: number): number {
  const reply = record(value);
  if (reply.generation !== generation || !integer(reply.releasedClaims)) return fail("La libération ne correspond pas à la génération demandée.");
  return reply.releasedClaims;
}
export function cancelledReservation(value: unknown): true {
  if (record(value).reservationCancelled !== true) return fail("L’annulation de la réservation n’a pas été confirmée.");
  return true;
}
export function abandonConversionRequest(familyRoot: string, destination: string) {
  // Recovery deliberately does not require the original source to open or exist.
  separateDirectories(familyRoot, destination);
  return { action: "abandonConversion", familyRoot, destination };
}
export function abandonedConversion(value: unknown, destination: string): string {
  const reply = record(value);
  if (!text(reply.destination) || pathKey(reply.destination) !== pathKey(destination) || reply.abandoned !== true || reply.filesRetained !== true) return fail("Abandon non confirmé : le reçu doit identifier la destination et confirmer la conservation des fichiers.");
  return reply.destination;
}
export function separateDirectories(source: string, destination: string, family?: string) {
  const paths = [source, destination, ...(family === undefined ? [] : [family])];
  if (paths.some(p => !text(p) || p !== p.trim() || /[\u0000-\u001f]/.test(p) || !/^(?:[a-zA-Z]:[\\/]|\/)/.test(p) || /(?:^|[\\/])\.{1,2}(?:[\\/]|$)/.test(p))) return fail("Indiquez des dossiers absolus distincts, sans segment relatif.");
  const keys = paths.map(pathKey);
  for (let i = 0; i < keys.length; i++) for (let j = i + 1; j < keys.length; j++)
    if (keys[i] === keys[j] || keys[i].startsWith(`${keys[j]}/`) || keys[j].startsWith(`${keys[i]}/`)) return fail("La source, la destination et le stockage doivent être séparés, sans imbrication.");
}
export type Conversion = { path: string; worktreeId: string; source: string; captures: number; digest: string };
export function conversion(value: unknown, source: string, destination: string, status: ConversionSource): Conversion {
  const r = record(value), receipt = record(r.receipt), from = record(receipt.sourceIdentity), to = record(receipt.destinationIdentity), identity = record(r.identity);
  separateDirectories(source, destination);
  if (typeof r.path !== "string" || pathKey(r.path) !== pathKey(destination) || typeof receipt.sourceCheckout !== "string" || pathKey(receipt.sourceCheckout) !== pathKey(source) || receipt.sourceRetained !== true || !integer(receipt.importedCaptureCount) || !rawDigest(receipt.checkoutDigest) || !rawDigest(receipt.sourceDatabaseDigest) || !rawDigest(receipt.sourceProjectionDigest) || !rawDigest(receipt.publicationHistoryDigest) || !integer(receipt.sourceGeneration) || !text(to.worktreeId) || to.worktreeId === status.identity.worktreeId) return fail("Reçu de conversion incohérent ; la destination ne peut pas être ouverte depuis ce reçu.");
  for (const key of ["storeId", "worktreeId", "spaceId", "origin", "principalId"] as const) {
    if (!text(to[key]) || from[key] !== status.identity[key] || to[key] !== identity[key]) return fail();
    if (!["storeId", "worktreeId"].includes(key) && to[key] !== from[key]) return fail();
  }
  return { path: r.path, worktreeId: to.worktreeId, source: receipt.sourceCheckout, captures: receipt.importedCaptureCount, digest: receipt.checkoutDigest };
}
