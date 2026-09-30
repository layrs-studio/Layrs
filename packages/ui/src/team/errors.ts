/*
 * Server errors become French sentences that say what did not happen and
 * what to do next. Codes stay out of the main message; the server only
 * returns a few closed codes and statuses, never private detail.
 */

type HttpLike = { status?: unknown; code?: unknown; message?: unknown };

const codes: Record<string, string> = {
  authorization_epoch_changed: "Les droits du projet ont changé entre-temps. Rechargez puis recommencez ; rien n’a été modifié.",
  custom_policy_root_active:
    "Ce projet utilise des règles d’accès personnalisées : les équipes, rôles et invitations sont suspendus. Rétablissez la gestion par équipes dans Réglages → Avancé.",
  invalid_capability: "Le serveur a refusé cette action : vos droits actuels ne la permettent pas.",
  unauthorized: "Session expirée ou révoquée. Reconnectez-vous.",
  not_found: "Élément introuvable, déjà supprimé ou hors de votre accès.",
  destination_not_allowed: "Cette adresse n’est pas autorisée par la configuration de l’instance. Demandez à l’administrateur de l’instance de l’ajouter.",
  invalid_event_kinds: "Choisissez entre 1 et 8 événements.",
  backend_error: "Le serveur est momentanément indisponible. Réessayez dans un instant.",
  io_error: "Le serveur est momentanément indisponible. Réessayez dans un instant.",
};

/**
 * @param context what the user tried, used when the server only says "refused"
 * (e.g. "L’invitation n’a pas été créée.").
 */
export function describeError(error: unknown, context?: string) {
  const value = (error && typeof error === "object" ? error : {}) as HttpLike;
  const message = typeof value.message === "string" ? value.message : String(error);
  const code = typeof value.code === "string" ? value.code : /^[a-z_]{3,64}$/.test(message) ? message : null;
  const status = typeof value.status === "number" ? value.status : null;
  const prefix = context ? `${context} ` : "";
  if (code && codes[code]) return `${prefix}${codes[code]}`;
  if (status === 403) return `${prefix}${codes.invalid_capability}`;
  if (status === 404) return `${prefix}${codes.not_found}`;
  if (status === 409) return `${prefix}${codes.authorization_epoch_changed}`;
  if (status === 400 || code === "invalid_descriptor")
    return `${prefix}Le serveur a refusé les valeurs envoyées ou l’état a changé. Vérifiez le formulaire, rechargez puis réessayez.`;
  if (code) return `${prefix}Le serveur a refusé l’action (${code}).`;
  return `${prefix}${message}`.trim();
}

export function isForbidden(error: unknown) {
  const value = (error && typeof error === "object" ? error : {}) as HttpLike;
  return value.status === 403 || value.code === "invalid_capability" || value.message === "invalid_capability";
}
