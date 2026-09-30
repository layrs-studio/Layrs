/*
 * French explanations of native failures: what did not happen, what is kept
 * and the next step. The technical code stays available for diagnosis.
 */

export type ErrorContext = "publish" | "candidates" | "receive" | "clone" | "login" | "session" | "remote" | "local" | "projection";

export type Explained = {
  title: string;
  body: string;
  /** Raw native message, shown as technical detail. */
  detail: string;
  /** Session is missing or expired: the connection must be redone. */
  signedOut?: boolean;
  /** The server may have accepted the work; only its confirmation is missing. */
  unconfirmed?: boolean;
  cancelled?: boolean;
};

export function explainError(cause: unknown, context: ErrorContext = "local"): Explained {
  const detail = String(cause instanceof Error ? cause.message : cause).replace(/^Error:\s*/, "");
  const publication = context === "publish" || context === "candidates";
  const make = (title: string, body: string, extra: Partial<Explained> = {}): Explained => ({ title, body, detail, ...extra });

  if (detail.includes("remote_transfer_cancelled"))
    return publication
      ? make(
          "Publication annulée avant envoi de la transaction.",
          "Les têtes distantes, les fichiers et l’historique local sont conservés.",
          { cancelled: true },
        )
      : make("Transfert annulé avant application.", "Les fichiers et l’historique local sont conservés.", { cancelled: true });
  if (detail.includes("publication_pending"))
    return make(
      "Une publication précédente attend son reçu.",
      "Rejouez la publication pour connaître son résultat : la même opération est renvoyée, sans doublon. Son annulation ne peut pas être confirmée. Vos captures locales sont conservées.",
      { unconfirmed: true },
    );
  if (/remote_unauthorized|Sign in to this server first|No device login in progress/.test(detail))
    return make(
      context === "login" ? "La connexion n’a pas été confirmée." : "Session absente ou expirée.",
      "Connectez de nouveau Desktop à ce serveur. Aucun fichier local n’a été modifié.",
      { signedOut: context !== "login" || detail.includes("remote_unauthorized") },
    );
  if (detail.includes("remote_forbidden"))
    return make("Le serveur a refusé cette action.", "Vos droits ne permettent pas cette opération. Votre travail local est conservé.");
  if (detail.includes("content_access_restricted"))
    return make(
      "Votre accès est limité à certains fichiers.",
      "Le clonage et la réception complète sont indisponibles avec ces droits. Récupérez le projet en accès limité ou consultez les fichiers autorisés depuis Équipe. Vos fichiers locaux sont conservés.",
    );
  if (/projection_dirty|capture or preserve edits/.test(detail))
    return make("Capturez vos modifications avant ce transfert annulable.", "Aucun fichier n’a été envoyé ni remplacé.");
  if (detail.includes("remote_transport_failed"))
    return make(
      publication ? "La publication n’a pas été confirmée." : "Le serveur n’a pas répondu.",
      publication
        ? "Le serveur a peut-être reçu la publication, mais sa réponse n’est pas arrivée. Relancez Publier : la même publication est rejouée sans doublon. Votre capture locale est conservée."
        : "Vérifiez le réseau et le certificat du serveur (étape avancée de la connexion). Votre travail local est conservé.",
      { unconfirmed: publication },
    );
  if (/OS credential store unavailable/.test(detail))
    return make(
      "Le coffre de mots de passe du système est indisponible.",
      "Desktop y conserve le jeton de connexion ; déverrouillez-le puis recommencez. Aucun jeton n’a été enregistré ailleurs.",
    );
  if (/stale|cas_mismatch|changed since|projection_stale/i.test(detail))
    return make(
      "Le projet a changé depuis l’aperçu.",
      "Rien n’a été appliqué. Actualisez l’aperçu avant d’appliquer ; votre travail est conservé.",
    );
  if (detail.includes("worktree_needs_repair"))
    return make("Une écriture a été interrompue.", "Vos sauvegardes sont conservées. Ouvrez le diagnostic pour reprendre l’opération.");
  if (detail.includes("operation_integration_required"))
    return make(
      "Des opérations concurrentes doivent d’abord être réunies.",
      "Recevez ou fusionnez les têtes concurrentes, puis capturez à nouveau. Vos fichiers ne sont pas modifiés.",
    );
  if (detail.includes("projection_capture_refused"))
    return make(
      "La capture a été refusée.",
      "Un fichier réservé ou interdit a été créé ou modifié hors de Layrs. Ses octets sont conservés ; retirez-le ou déplacez-le, puis actualisez.",
    );
  if (detail.includes("reserved_projection_path"))
    return make("Ce nom est réservé dans ce dossier.", "Choisissez un autre nom. Aucun fichier n’a été créé.");
  if (detail.includes("only inside the Tauri desktop runtime"))
    return make("Commandes Desktop indisponibles.", "Cette fenêtre ne s’exécute pas dans l’application Layrs Desktop.");
  if (context === "publish" || context === "candidates")
    return make("La publication n’a pas abouti.", "Votre capture locale est conservée.");
  if (context === "receive" || context === "clone")
    return make("La réception n’a pas abouti.", "Les fichiers et l’historique local sont conservés.");
  if (context === "login" || context === "session") return make("La connexion n’a pas abouti.", "Aucun fichier local n’a été modifié.");
  return make("L’opération n’a pas abouti.", "Votre travail local est conservé.");
}
