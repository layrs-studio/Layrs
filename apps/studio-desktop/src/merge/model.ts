export type Request = { action: string; [key: string]: unknown };

/** Preview returned by every weave/resolution preview: applying it is refused if the project moved. */
export type MergePreview = { expectedOperation: string; expectedStep: string; conflicts: unknown[] };

export type PendingMerge = { title: string; value: MergePreview; apply: Request };

export type Term = { termId: string; provenance: { stepId: string }; values: unknown[] };

export type ConflictBlock = {
  blockId: string;
  baseValues: { termId: string }[];
  sideValues: { termId: string }[];
  resolution: unknown | null;
};

export type Conflict = {
  conflictId: string;
  document: {
    placements: { path: string }[];
    sides: Term[];
    bases?: Term[];
    blocks?: ConflictBlock[];
    segments?: { kind: string; blockId?: string }[];
  };
};

export type VariantContent = { text: string | null; byteLength?: number };

export type Comparison = {
  available: boolean;
  beforeTerm: string;
  afterTerm: string;
  changes: null | { path: string; executableChanged: boolean; textOmitted: boolean; textDiff: null | { op: string; text: string }[] }[];
};

export type BlockDraft = { text: string; sources: string[] };

export type NamedTerm = { termId: string; label: string; stepId?: string };

export function shortStep(stepId?: string) {
  if (!stepId) return "";
  const hash = stepId.replace(/^blake3:/, "");
  return hash.slice(0, 10);
}

/** Bases first, then variants, each with an explicit readable source name. */
export function namedTerms(conflict: Conflict): NamedTerm[] {
  const bases = conflict.document.bases ?? [];
  return [
    ...bases.map((term, index) => ({ termId: term.termId, stepId: term.provenance.stepId, label: bases.length > 1 ? `Base ${index + 1}` : "Base" })),
    ...conflict.document.sides.map((term, index) => ({ termId: term.termId, stepId: term.provenance.stepId, label: `Variante ${index + 1}` })),
  ];
}

export function conflictPaths(conflict: Conflict) {
  return conflict.document.placements.map((placement) => placement.path);
}

export function remainingLabel(count: number) {
  return count === 0 ? "Aucun conflit ne restera après cette opération." : `${count} conflit${count > 1 ? "s" : ""} ${count > 1 ? "resteront" : "restera"} après cette opération.`;
}
