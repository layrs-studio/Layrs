import {
  selectedProjectionPaths, validateProjectionSelection,
  type NativeProjectionSelection, type NativeProjectionStatus, type ProjectionPathSemantics,
} from "@layrs/ui";

export type SelectionRule = { id: number; kind: "path" | "subtree"; path: string };

export function selectionFromRules(rules: SelectionRule[], semantics: ProjectionPathSemantics): NativeProjectionSelection {
  return selectedProjectionPaths(rules.filter(rule => rule.kind === "path").map(rule => rule.path), rules.filter(rule => rule.kind === "subtree").map(rule => rule.path), semantics);
}

export function cloneSelectedRequest(space: string, step: string, selection: NativeProjectionSelection) {
  if (!space.trim()) throw new Error("Choisissez le projet à récupérer.");
  if (!/^blake3:[a-f0-9]{64}$/.test(step.trim())) throw new Error("Indiquez l’identifiant exact de la capture publiée.");
  if (selection.kind !== "selected") throw new Error("Choisissez les chemins à récupérer.");
  return { action: "cloneProjectionSelected", space: space.trim(), step: step.trim(), selection };
}

export function expandSelectionRequest(status: NativeProjectionStatus, expectedGeneration: number, additions: NativeProjectionSelection) {
  if (!Number.isSafeInteger(expectedGeneration) || expectedGeneration < 0 || expectedGeneration !== status.projection.generation) {
    throw new Error("La sélection a changé. Actualisez les fichiers avant de recommencer.");
  }
  if (status.needsRepair || status.pendingPublication) throw new Error("Terminez la reprise ou la publication en attente avant d’élargir la sélection.");
  if (!status.selection) throw new Error("La sélection locale n’est pas disponible. Actualisez les fichiers.");
  return {
    action: "expandProjectionSelection", expectedGeneration,
    // The command adapter unions these additions with persisted intent atomically.
    // Do not resend prior rules: that would count them twice against native limits.
    selection: validateProjectionSelection(additions, status.projection.snapshot.pathSemantics),
  };
}
