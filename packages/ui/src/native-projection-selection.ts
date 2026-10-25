import { nativePathKey } from "./native-project-files-model";

/** Local intent, independent of permissions and verified content residency. */
export type NativeProjectionSelection =
  | { kind: "all" }
  | { kind: "selected"; paths: string[]; subtrees: string[] };

export type ProjectionPathSemantics = "case_sensitive" | "portable_windows";

export function projectionSelectionIncludes(selection: NativeProjectionSelection, path: string): boolean {
  return projectionSelectionMatcher(selection)(path);
}

/** Compile once for a bounded registry; avoid comparing every entry to every rule. */
export function projectionSelectionMatcher(selection: NativeProjectionSelection): (path: string) => boolean {
  if (selection.kind === "all") return () => true;
  const paths = new Set(selection.paths), subtrees = new Set(selection.subtrees);
  return path => {
    if (paths.has(path) || subtrees.has(path)) return true;
    for (let slash = path.indexOf("/"); slash !== -1; slash = path.indexOf("/", slash + 1)) {
      if (subtrees.has(path.slice(0, slash))) return true;
    }
    return false;
  };
}

/** Validate the wire shape before using it to classify files or send a request. */
export function validateProjectionSelection(value: unknown, semantics: ProjectionPathSemantics): NativeProjectionSelection {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Sélection de fichiers invalide.");
  const selection = value as Record<string, unknown>;
  if (selection.kind === "all" && Object.keys(selection).length === 1) return { kind: "all" };
  if (selection.kind !== "selected" || Object.keys(selection).some(key => !["kind", "paths", "subtrees"].includes(key)) ||
      !Array.isArray(selection.paths) || !Array.isArray(selection.subtrees) || selection.paths.length + selection.subtrees.length > 100_000) {
    throw new Error("Sélection de fichiers invalide.");
  }
  const prefixes = new Map<string, string>();
  let bytes = 0;
  for (const path of [...selection.paths, ...selection.subtrees]) {
    const platform = semantics === "portable_windows" ? "windows" : "posix";
    const mode = semantics === "portable_windows" ? "insensitive" : "sensitive";
    if (typeof path !== "string" || !nativePathKey(path, platform, mode)) {
      throw new Error("Utilisez des chemins relatifs exacts séparés par /, sans .. ni dossier de métadonnées.");
    }
    bytes += new TextEncoder().encode(path).length;
    if (bytes > 8 * 1024 * 1024) throw new Error("La sélection dépasse la limite de taille.");
    const parts = path.split("/");
    for (let n = 1; n <= parts.length; n++) {
      const prefix = parts.slice(0, n).join("/");
      const key = nativePathKey(prefix, platform, mode)!;
      if (prefixes.has(key) && prefixes.get(key) !== prefix) throw new Error("Deux chemins de la sélection désignent le même nom sur ce système.");
      prefixes.set(key, prefix);
    }
  }
  return { kind: "selected", paths: [...selection.paths], subtrees: [...selection.subtrees] } as NativeProjectionSelection;
}

/** Forms accept literal paths only; no glob syntax is interpreted. */
export function selectedProjectionPaths(paths: string[], subtrees: string[], semantics: ProjectionPathSemantics): NativeProjectionSelection {
  if (!paths.length && !subtrees.length) throw new Error("Ajoutez au moins un chemin exact ou un sous-arbre.");
  if ([...paths, ...subtrees].some(path => /[*?\[\]{}]/.test(path))) throw new Error("Les motifs glob ne sont pas acceptés. Indiquez un chemin exact ou un sous-arbre.");
  return validateProjectionSelection({ kind: "selected", paths, subtrees }, semantics);
}

