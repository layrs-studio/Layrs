/*
 * Pure presentation rules of the collaboration pages. No request, no storage:
 * the functions only turn contract data into names and honest wording.
 */
import type { Tone } from "../components/Feedback";
import type { DiffLine } from "../components/diff-model";
import type { NativeProjectionPageEntry } from "../native-remote-project-files-model";
import type { CollabCapabilities, CompareChange, DecisionState, HistoryFilterRequest, PeopleDirectory, ProjectReferences } from "./types";

/** True only when the server hint explicitly refuses the action. */
export function denied(capabilities: CollabCapabilities | undefined, flag: keyof CollabCapabilities) {
  return capabilities?.[flag] === false;
}

type Loose = Record<string, unknown>;
const object = (value: unknown): value is Loose => Boolean(value) && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown) => (typeof value === "string" ? value : "");

/** Tolerant reading of a bootstrap: unknown fields are ignored, missing ones empty. */
export function projectReferences(bootstrap: unknown): ProjectReferences {
  const source = object(bootstrap) ? bootstrap : {};
  const list = (value: unknown) => (Array.isArray(value) ? value.filter(object) : []);
  const record = (item: Loose) => (object(item.record) ? item.record : {});
  return {
    layers: list(source.layers)
      .map((item) => ({
        layerId: text(record(item).layerId),
        name: text(record(item).name) || "Layer sans nom",
        heads: Array.isArray(item.headStepIds) ? item.headStepIds.filter((id): id is string => typeof id === "string") : [],
      }))
      .filter((layer) => layer.layerId),
    markers: list(source.markers)
      .map((item) => {
        const value = record(item);
        return {
          markerId: text(value.markerId),
          name: text(value.name),
          title: text(value.title),
          notes: text(value.notes),
          targetStepId: text(value.targetStepId),
          createdAt: text(value.createdAt),
        };
      })
      .filter((marker) => marker.targetStepId),
    releases: list(source.releases)
      .map((item) => {
        const value = record(item);
        return {
          releaseId: text(value.releaseId),
          version: text(value.version),
          title: text(value.title),
          notes: text(value.notes),
          targetStepId: text(value.targetStepId),
          createdAt: text(value.createdAt),
        };
      })
      .filter((release) => release.targetStepId),
    restricted: object(source.contentAccess) && source.contentAccess.mode === "restricted",
  };
}

export const emptyReferences: ProjectReferences = { layers: [], markers: [], releases: [], restricted: false };

/** Names attached to a capture: Layer heads, versions and markers. */
export function captureNames(stepId: string, references: ProjectReferences): string[] {
  const names: string[] = [];
  for (const layer of references.layers) if (layer.heads.includes(stepId)) names.push(`Dernière capture de ${layer.name}`);
  for (const release of references.releases) if (release.targetStepId === stepId) names.push(`Version ${release.version}`);
  for (const marker of references.markers) if (marker.targetStepId === stepId) names.push(`Repère ${marker.name}`);
  return names;
}

export function captureTitle(stepId: string, references: ProjectReferences, fallback = "Capture publiée") {
  return captureNames(stepId, references)[0] ?? fallback;
}

export function layerName(layerId: string, references: ProjectReferences) {
  return references.layers.find((layer) => layer.layerId === layerId)?.name ?? "Layer non listé";
}

/** Captures that can be chosen by name, for selectors. */
export function namedCaptures(references: ProjectReferences) {
  const options: { stepId: string; label: string; group: string }[] = [];
  const seen = new Set<string>();
  const add = (stepId: string, label: string, group: string) => {
    const key = `${group}:${stepId}`;
    if (seen.has(key)) return;
    seen.add(key);
    options.push({ stepId, label, group });
  };
  for (const layer of references.layers)
    layer.heads.forEach((head, index) =>
      add(head, layer.heads.length > 1 ? `${layer.name} · tête ${index + 1} sur ${layer.heads.length}` : layer.name, "Layers"),
    );
  for (const release of references.releases) add(release.targetStepId, `Version ${release.version}`, "Versions");
  for (const marker of references.markers) add(marker.targetStepId, `Repère ${marker.name}`, "Repères");
  return options;
}

/** Never an invented name: unknown people stay unknown. */
export function personLabel(principalId: string | null | undefined, people: PeopleDirectory = {}) {
  if (!principalId) return "Compte inconnu";
  if (people.currentPrincipalId && principalId === people.currentPrincipalId) return "Vous";
  return people.names?.[principalId] ?? "Autre compte";
}

/** The same person inside a sentence: « par vous », « par un autre compte », « par Alice ». */
export function personInSentence(principalId: string | null | undefined, people: PeopleDirectory = {}) {
  if (!principalId) return "un compte inconnu";
  if (people.currentPrincipalId && principalId === people.currentPrincipalId) return "vous";
  return people.names?.[principalId] ?? "un autre compte";
}

/** Name used for initials; unknown people get a neutral mark, never invented initials. */
export function personAvatarName(principalId: string | null | undefined, people: PeopleDirectory = {}) {
  return (principalId && people.names?.[principalId]) || "?";
}

export function reviewLifecycle(lifecycle: string): { label: string; tone: Tone | "accent" } {
  if (lifecycle === "integrated") return { label: "Intégrée", tone: "success" };
  if (lifecycle === "closed") return { label: "Fermée", tone: "info" };
  if (lifecycle === "open") return { label: "Ouverte", tone: "accent" };
  return { label: "État inconnu", tone: "warning" };
}

/** Wording of the server decision state. Counts are the server's, never inferred. */
export function decisionSummary(state: DecisionState | undefined): { label: string; tone: Tone; detail: string } | null {
  if (!state) return null;
  const counts = `${state.approvals}/${state.requiredApprovals} approbation${state.requiredApprovals > 1 ? "s" : ""}`;
  if (state.stale) return { label: "Version à actualiser", tone: "warning", detail: `${counts} · décisions à renouveler sur la version actuelle` };
  if (state.state === "changes_requested" || state.blockingDecisions > 0)
    return {
      label: "Modifications demandées",
      tone: "danger",
      detail: `${counts} · ${state.blockingDecisions} demande${state.blockingDecisions > 1 ? "s" : ""} de modification`,
    };
  if (state.state === "approved") return { label: "Approbations réunies", tone: "success", detail: counts };
  return { label: "En attente d’approbation", tone: "info", detail: counts };
}

export const decisionActions = ["approve", "request_changes"];

export function actionVerb(action: string) {
  switch (action) {
    case "comment":
      return "a commenté";
    case "approve":
      return "a approuvé";
    case "request_changes":
      return "a demandé des modifications";
    case "close":
      return "a fermé la revue";
    case "reopen":
      return "a rouvert la revue";
    default:
      return "a agi sur la revue";
  }
}

/** A decision only counts for the version it was made on. */
export function decisionSuperseded(action: { action: string; version: number }, currentVersion: number) {
  return decisionActions.includes(action.action) && action.version < currentVersion;
}

export function changeStatusLabel(status: string) {
  if (status === "added") return "Ajouté";
  if (status === "deleted") return "Supprimé";
  if (status === "modified") return "Modifié";
  return "Changé";
}

export function diffLines(change: CompareChange): DiffLine[] {
  return (change.textDiff ?? []).map((line) => ({ op: line.op, text: line.text }));
}

export function fileName(path: string) {
  return path.split("/").at(-1) || path;
}

export function parentFolder(path: string) {
  const index = path.lastIndexOf("/");
  return index < 0 ? "" : path.slice(0, index);
}

export type FileRow = {
  path: string;
  name: string;
  kind: "file" | "tree" | "symlink" | "conflict" | "tombstone";
  readable: boolean;
  writable: boolean;
  executable: boolean;
  /** Folder deduced from deeper paths: no entry of its own was listed yet. */
  implicit: boolean;
};

function row(entry: NativeProjectionPageEntry): FileRow {
  return {
    path: entry.path,
    name: fileName(entry.path),
    kind: entry.target.kind,
    readable: entry.contentReadable,
    writable: entry.writable,
    executable: entry.executable,
    implicit: false,
  };
}

const collator = new Intl.Collator("fr", { numeric: true, sensitivity: "base" });

/** Direct children of `folder` among the loaded entries, folders first. */
export function folderChildren(entries: NativeProjectionPageEntry[], folder: string): FileRow[] {
  const prefix = folder ? `${folder}/` : "";
  const children = new Map<string, FileRow>();
  for (const entry of entries) {
    if (!entry.path.startsWith(prefix)) continue;
    const rest = entry.path.slice(prefix.length);
    if (!rest) continue;
    const slash = rest.indexOf("/");
    if (slash < 0) {
      children.set(entry.path, row(entry));
    } else {
      const path = prefix + rest.slice(0, slash);
      if (!children.has(path))
        children.set(path, { path, name: rest.slice(0, slash), kind: "tree", readable: true, writable: false, executable: false, implicit: true });
    }
  }
  return [...children.values()].sort((a, b) => Number(b.kind === "tree") - Number(a.kind === "tree") || collator.compare(a.name, b.name));
}

/** Name search bounded to what is loaded; the caller states that scope. */
export function searchFiles(entries: NativeProjectionPageEntry[], query: string, limit = 200): FileRow[] {
  const needle = query.trim().toLocaleLowerCase("fr");
  if (!needle) return [];
  return entries
    .filter((entry) => fileName(entry.path).toLocaleLowerCase("fr").includes(needle))
    .slice(0, limit)
    .map(row);
}

export function findFile(entries: NativeProjectionPageEntry[], path: string): FileRow | null {
  const entry = entries.find((item) => item.path === path);
  if (entry) return row(entry);
  return entries.some((item) => item.path.startsWith(`${path}/`))
    ? { path, name: fileName(path), kind: "tree", readable: true, writable: false, executable: false, implicit: true }
    : null;
}

export function accessLabel(file: Pick<FileRow, "readable" | "writable">): { label: string; tone: Tone } {
  if (!file.readable) return { label: "Contenu non accessible", tone: "warning" };
  return file.writable ? { label: "Lecture et écriture", tone: "success" } : { label: "Lecture seule", tone: "info" };
}

export function kindLabel(kind: FileRow["kind"]) {
  return { file: "Fichier", tree: "Dossier", symlink: "Lien symbolique", conflict: "Conflit enregistré", tombstone: "Suppression enregistrée" }[kind];
}

export type HistoryFilters = { from: string; path: string; since: string; until: string; actor: string };
export const emptyHistoryFilters: HistoryFilters = { from: "", path: "", since: "", until: "", actor: "" };

function isoWithoutMillis(date: Date) {
  return date.toISOString().replace(/\.\d{3}Z$/, "Z");
}

/** `yyyy-mm-dd` in the local calendar to an RFC 3339 instant, or null. */
export function localDayBoundary(day: string, end: boolean): string | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day);
  if (!match) return null;
  const date = end
    ? new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]), 23, 59, 59)
    : new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]), 0, 0, 0);
  return Number.isNaN(date.getTime()) ? null : isoWithoutMillis(date);
}

export function historyFilterRequest(filters: HistoryFilters): HistoryFilterRequest {
  return {
    actorId: filters.actor.trim() || null,
    since: localDayBoundary(filters.since, false),
    until: localDayBoundary(filters.until, true),
  };
}

export function captureReasonLabel(reason: string) {
  const labels: Record<string, string> = {
    manual_snapshot: "capture manuelle",
    automatic_snapshot: "capture automatique",
    auto_snapshot: "capture automatique",
    merge: "fusion",
    restore: "restauration",
    import: "import",
  };
  return labels[reason] ?? "motif non détaillé";
}

export type CollabErrorInfo = { message: string; code: string | null; forbidden: boolean; stale: boolean };

const codeMessages: Record<string, { message: string; stale?: boolean; forbidden?: boolean }> = {
  invalid_capability: { message: "Vous n’avez pas accès à cette ressource avec vos droits actuels.", forbidden: true },
  expired_capability: { message: "Votre autorisation a expiré. Actualisez la page.", stale: true },
  not_found: { message: "Élément introuvable ou inaccessible." },
  authorization_epoch_changed: { message: "Les droits du projet ont changé depuis le chargement. Actualisez avant de réessayer.", stale: true },
  stale_release_state: { message: "La publication a changé depuis son chargement. Actualisez puis réessayez.", stale: true },
  release_gate_rejected: {
    message: "Publication refusée par la protection du projet : les validations exigées pour cette capture ne sont pas réunies.",
  },
  invalid_cursor: { message: "La position de lecture a expiré. Rechargez la liste.", stale: true },
  limit_exceeded: { message: "Limite du serveur atteinte pour cette lecture." },
  custom_policy_root_active: { message: "Une politique d’accès personnalisée est active ; cette gestion n’est pas disponible." },
  backend_error: { message: "Le serveur est momentanément indisponible. Réessayez dans un instant." },
  io_error: { message: "Le serveur est momentanément indisponible. Réessayez dans un instant." },
};

/** Human, French wording of a transport error. The technical code stays available for details. */
export function explainCollabError(error: unknown): CollabErrorInfo {
  const raw = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  const status = object(error) && typeof error.status === "number" ? error.status : null;
  const declared = object(error) && typeof error.code === "string" ? error.code : null;
  const code = declared ?? (/^[a-z][a-z0-9_]{2,95}$/.test(raw) ? raw : (Object.keys(codeMessages).find((key) => raw.includes(key)) ?? null));
  const known = code ? codeMessages[code] : undefined;
  if (known) return { message: known.message, code, forbidden: Boolean(known.forbidden) || status === 403, stale: Boolean(known.stale) };
  if (status === 403) return { message: "Vous n’avez pas accès à cette ressource avec vos droits actuels.", code, forbidden: true, stale: false };
  if (status === 404) return { message: "Élément introuvable ou inaccessible.", code, forbidden: false, stale: false };
  if (status === 409) return { message: "L’état a changé depuis le chargement. Actualisez puis réessayez.", code, forbidden: false, stale: true };
  if (code) return { message: "Le serveur a refusé l’opération.", code, forbidden: false, stale: false };
  return { message: raw || "L’opération n’a pas abouti.", code: null, forbidden: false, stale: false };
}
