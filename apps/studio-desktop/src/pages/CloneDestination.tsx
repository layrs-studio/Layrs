import { useEffect, useRef, useState } from "react";
import { Banner, Button, navigate, segment } from "@layrs/ui";
import { useRemote } from "../app/remote";
import { useDesktopContexts } from "../app/contexts";
import { rememberOrigin } from "../app/serverMemory";
import { rememberProject } from "../app/recents";
import { rememberRecentContext } from "../app/recentContext";
import { explainError } from "../app/errors";

export type Destination = {
  kind: "empty" | "sameProject" | "otherProject" | "unpublishedProject" | "blocked";
  localSpaceId?: string;
  name?: string;
  contextId?: string;
  reason?: string;
};

export function useCloneDestination(space?: string) {
  const remote = useRemote();
  const contexts = useDesktopContexts();
  const principalId = remote.session.state === "connected" ? remote.session.principalId : "";
  const contextId = contexts.origin === remote.origin && contexts.principalId === principalId ? contexts.selected?.id : undefined;
  const key = JSON.stringify([remote.root, remote.origin, principalId, remote.epoch, space, contextId]);
  const [attempt, setAttempt] = useState(0);
  const [state, setState] = useState<{ key: string; result?: Destination; error?: string } | null>(null);
  const enabled = Boolean(principalId && remote.root.trim());
  useEffect(() => {
    if (!enabled) return;
    let active = true;
    setState(null);
    const timer = window.setTimeout(() => {
      void remote.transport<Destination>(
        { action: "inspectCloneDestination", space: space || undefined, contextId },
        { root: remote.root, origin: remote.origin, principalId },
      ).then((result) => { if (active) setState({ key, result }); })
        .catch((cause) => { if (active) setState({ key, error: explainError(cause, "remote").detail }); });
    }, 180);
    return () => { active = false; window.clearTimeout(timer); };
  }, [key, enabled, attempt, remote.transport]);
  const current = state?.key === key ? state : null;
  return {
    key,
    result: current?.result,
    error: current?.error,
    loading: enabled && !current,
    refresh: () => setAttempt((value) => value + 1),
  };
}

const blockedReasons: Record<string, string> = {
  unreadableDestination: "Layrs ne peut pas lire ce dossier. Vérifiez son chemin et ses permissions.",
  linkedDestination: "Ce chemin contient un lien vers un autre dossier. Choisissez le chemin réel du projet.",
  notDirectory: "Ce chemin ne désigne pas un dossier.",
  invalidLayrsStore: "Les données Layrs de ce dossier sont incomplètes ou invalides. Le clonage ne peut pas les remplacer.",
  projectRootMismatch: "Ce dossier ne correspond pas à la racine enregistrée du projet. Ouvrez le dossier d’origine.",
  repairRequired: "Une opération interrompue doit être reprise. Ouvrez ce projet et son diagnostic.",
  invalidRemoteBinding: "La connexion enregistrée de ce projet est invalide. Ouvrez-le pour la vérifier.",
  foreignOrPendingBinding: "Ce projet est lié à un autre serveur ou possède une publication en attente. Ouvrez-le pour vérifier sa synchronisation.",
  projectStateMismatch: "Le projet local et celui du serveur n’ont pas un état compatible pour établir ce lien.",
  projectIdentityMismatch: "Les données locales ne désignent pas toutes le même projet. Ouvrez le diagnostic avant de continuer.",
  projectAccessUnavailable: "Ce compte ne permet pas de lier ce projet sur le serveur choisi.",
  projectContextUnavailable: "L’organisation de ce projet n’est pas accessible avec cette connexion.",
  boundProjectUnavailable: "Le projet lié à ce dossier n’est pas accessible sur le serveur. Vérifiez le compte et la connexion du projet existant.",
  notEmpty: "Ce dossier contient des fichiers qui ne font pas partie d’un projet Layrs reconnu. Choisissez un autre dossier.",
  notLayrsProject: "Ce dossier contient des fichiers, mais aucun projet Layrs valide. Le clonage ne peut pas les remplacer.",
  otherServer: "Ce projet est lié à un autre serveur. Ouvrez-le depuis Vos projets pour retrouver sa connexion.",
  pendingPublication: "Ce projet a une publication en attente. Ouvrez-le pour reprendre cette publication avant de modifier son lien.",
  projection: "Ce dossier contient un projet à accès limité. Ouvrez-le depuis Vos projets pour le synchroniser.",
};

/** Existing directories are opened or prepared explicitly; they are never clone targets. */
export function CloneDestinationNotice({ inspection, projection = false }: {
  inspection: ReturnType<typeof useCloneDestination>;
  projection?: boolean;
}) {
  const remote = useRemote();
  const contexts = useDesktopContexts();
  const [error, setError] = useState("");
  const currentKey = useRef(inspection.key);
  currentKey.current = inspection.key;
  useEffect(() => { setError(""); }, [inspection.key]);
  useEffect(() => () => { currentKey.current = ""; }, []);
  const result = inspection.result;
  const contextMatches = remote.session.state === "connected" && contexts.origin === remote.origin && contexts.principalId === remote.session.principalId;

  async function openExisting() {
    if (!result?.localSpaceId || remote.session.state !== "connected" || remote.transfer) return;
    const key = inspection.key;
    const { origin } = remote;
    const principalId = remote.session.principalId;
    const publish = result.kind === "unpublishedProject";
    if (publish && (!contextMatches || !contexts.selected?.canCreateProject)) return;
    const contextId = publish ? contexts.selected?.id : result.contextId;
    setError("");
    const reply = await remote.operate(
      { kind: publish ? "provision" : "clone", label: publish ? "Préparation de la publication du projet local" : "Lien avec le projet local", busy: "Vérification du projet…", context: "remote" },
      { action: publish ? "prepareExistingPublication" : "linkExisting", space: result.localSpaceId, ...(publish ? { contextId } : {}) },
    );
    if (currentKey.current !== key) return;
    if (!reply) { inspection.refresh(); return; }
    const openedRoot = reply.state?.worktree.checkoutRoot;
    if (!openedRoot) { setError("Le dossier n’a pas pu être vérifié. Vérifiez-le à nouveau avant de continuer."); return; }
    rememberOrigin(openedRoot, origin);
    rememberProject({ root: openedRoot, name: result.name ?? "", kind: "complete" });
    if (!publish && contextId) {
      rememberRecentContext(openedRoot, origin, principalId, contextId, result.localSpaceId);
      contexts.adopt(contextId);
    }
    navigate(`/projects/${segment(openedRoot)}/${result.kind === "sameProject" ? "changes" : "sync"}`, publish ? { query: { publish: "1" } } : undefined);
  }

  if (inspection.loading) return <p role="status">Vérification du dossier et de son projet…</p>;
  if (inspection.error) return <Banner tone="danger" title="Le dossier n’a pas pu être vérifié" actions={<Button onClick={inspection.refresh}>Réessayer</Button>}>{inspection.error}</Banner>;
  if (!result || result.kind === "empty") return null;
  if (result.kind === "blocked" || projection) return <Banner tone="warning" title="Ce dossier ne peut pas recevoir ce clonage" actions={<Button onClick={inspection.refresh}>Vérifier à nouveau</Button>}>
    {projection ? "La récupération en accès limité nécessite un dossier vide. Le projet existant et ses fichiers sont conservés." : blockedReasons[result.reason ?? ""] ?? "Le dossier contient un projet incompatible, inaccessible ou dont l’état ne permet pas cette opération. Ses fichiers sont conservés ; choisissez un autre dossier ou ouvrez le projet existant."}
  </Banner>;
  const same = result.kind === "sameProject";
  const publish = result.kind === "unpublishedProject";
  return <Banner tone="info" title={same ? "Ce projet est déjà présent dans ce dossier" : publish ? "Un projet local peut être publié" : "Ce dossier contient un autre projet Layrs"} actions={
    <Button busy={Boolean(remote.transfer)} disabledReason={publish && (!contextMatches || !contexts.selected?.canCreateProject) ? "Choisissez une organisation dans laquelle vous pouvez créer un projet." : undefined} onClick={() => void openExisting()}>
      {same ? "Lier et ouvrir ce projet" : publish ? "Préparer la publication de ce projet" : "Synchroniser ce projet local"}
    </Button>
  }>
    <strong>{result.name ?? result.localSpaceId}</strong>. {same
      ? "Vous pouvez utiliser cette copie existante. Aucun fichier n’est téléchargé ni remplacé lors du lien."
      : publish
        ? "Ce projet n’existe pas encore sur ce serveur. Choisissez son identifiant, puis confirmez sa création et sa publication depuis Synchronisation."
        : "Il correspond à un autre projet du serveur. Ouvrez sa synchronisation pour publier ou recevoir ses changements ; le projet sélectionné ne sera pas cloné ici."}
    {error && <p role="alert">{error}</p>}
  </Banner>;
}
