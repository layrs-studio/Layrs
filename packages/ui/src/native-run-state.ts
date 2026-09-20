export type NativeRunStateInput = {
  status?: string;
  outcome?: string | null;
  proofObjectId?: string | null;
  expiresAt?: string;
};

export type NativeRunPresentation = {
  state: "queued" | "awaiting_result" | "running" | "completed" | "cancelled" | "expired" | "unknown";
  label: string;
  detail: string;
  canCancel: boolean;
};

/** Describes the server declaration, never an inferred Docker process or queue. */
export function nativeRunPresentation(run: NativeRunStateInput, now = Date.now()): NativeRunPresentation {
  const unknown: NativeRunPresentation = { state: "unknown", label: "Statut à actualiser", detail: "État absent, inconnu ou incohérent ; aucun verdict ne peut être déduit.", canCancel: false };
  const hasProof = typeof run.proofObjectId === "string" && run.proofObjectId.length > 0;
  const noResult = run.outcome == null && run.proofObjectId == null;
  if (run.status !== undefined && !["queued", "running", "completed", "cancelled", "expired"].includes(run.status)) return unknown;
  if (run.status === "cancelled") return noResult
    ? { state: "cancelled", label: "Annulé", detail: "Publication du résultat annulée ; cela ne confirme pas l’arrêt d’un exécuteur distant.", canCancel: false }
    : unknown;
  if (run.status === "completed" || hasProof) {
    if ((run.status !== undefined && run.status !== "completed") || !hasProof || !["passed", "failed"].includes(run.outcome ?? "")) return unknown;
    return { state: "completed", label: run.outcome === "passed" ? "Réussi" : "Échoué", detail: "Terminé : un verdict et sa preuve ont été reçus.", canCancel: false };
  }
  if (!noResult) return unknown;
  if (run.status === "expired") return { state: "expired", label: "Expiré sans résultat", detail: "Le serveur indique une expiration ; aucun échec ni succès n’est déduit.", canCancel: true };
  const expires = run.expiresAt === undefined ? NaN : Date.parse(run.expiresAt);
  if (Number.isFinite(expires) && Number.isFinite(now) && expires <= now) return { state: "expired", label: "Échéance dépassée sans résultat", detail: "Échéance dépassée selon l’horloge locale ; actualisez le statut serveur.", canCancel: true };
  if (run.status === "queued") return {state:"queued",label:"En attente de démarrage",detail:"Tentative enregistrée ; l’exécuteur prépare les fichiers ou attend de démarrer. Aucun verdict reçu.",canCancel:true};
  if (run.status === "running") return { state: "running", label: "En cours · résultat attendu", detail: "Exécution déclarée au serveur ; le processus distant n’est pas supervisé ici.", canCancel: true };
  return { state: "awaiting_result", label: "En attente de résultat", detail: "Aucun verdict reçu et aucun état d’exécution déclaré ; aucune file d’attente n’est supposée.", canCancel: true };
}
