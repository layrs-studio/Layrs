import { Camera, CheckCircle2, GitMerge, Lock, RefreshCw } from "lucide-react";
import { Badge, Banner, Button, EmptyState, Link, PageHeader, StatusDot, formatBytes, navigate, segment } from "@layrs/ui";
import { useProjection } from "../../app/projection";
import { useWorkspace } from "../../app/workspace";

/** P05 for a limited-access folder: same page and landmarks, actions per real capabilities. */
export function LimitedChanges() {
  const workspace = useWorkspace();
  const projection = useProjection();
  const { data, patch, ready, files } = projection;
  const key = segment(workspace.root);
  const reserved = files?.entries.filter((entry) => entry.access === "denied" || entry.contentState === "reserved").length ?? 0;
  const captureReason = workspace.busy
    ? `Opération en cours : ${workspace.busy}`
    : !data
      ? "Actualisez l’accès limité avant de capturer."
      : !ready
        ? "Registre incomplet ou reprise nécessaire : ouvrez le diagnostic."
        : data.capabilities.capture !== true
          ? "La capture n’est pas disponible tant qu’une publication attend son reçu."
          : undefined;

  return (
    <div className="changes">
      <div className="changes__header">
        <PageHeader
          title="Changements"
          description={
            patch
              ? `Dernière capture locale : ${patch.changes.length} chemin${patch.changes.length > 1 ? "s" : ""} modifié${patch.changes.length > 1 ? "s" : ""}`
              : "Capturez vos modifications des fichiers autorisés"
          }
          aside={
            <div className="changes__auto-status">
              <StatusDot tone="info">
                <span className="changes__auto-title">Accès limité</span>
              </StatusDot>
              <span className="changes__auto-meta">Capture automatique indisponible pour ce type de dossier</span>
            </div>
          }
        />
      </div>
      {data?.latestCaptureImported === true && <Banner tone="info" title="Historique importé conservé">Créez une nouvelle capture locale dans cette copie avant de publier ou de proposer un candidat.</Banner>}
      {data?.needsRepair && (
        <Banner tone="danger" title="Une opération interrompue doit être reprise." actions={<Button size="sm" onClick={() => navigate(`/projects/${key}/health`)}>Ouvrir le diagnostic</Button>}>
          Les modifications sont bloquées jusqu’à la reprise confirmée par le journal local.
        </Banner>
      )}
      {!data && !workspace.busy && (
        <Banner tone="warning" title="État local à relire." actions={<Button size="sm" icon={<RefreshCw aria-hidden size={14} />} onClick={() => void projection.refresh()}>Actualiser l’accès limité</Button>}>
          Après une erreur ou un changement externe, relisez l’état avant toute écriture.
        </Banner>
      )}
      {data?.pendingPublication && (
        <Banner tone="warning" title="Une publication attend son reçu." actions={<Button size="sm" onClick={() => navigate(`/projects/${key}/sync`)}>Ouvrir la synchronisation</Button>}>
          Rejouez la capture {data.pendingPublication.sequence} vers sa cible enregistrée ; aucune nouvelle capture n’est possible avant.
        </Banner>
      )}
      <div className="changes__body changes__body--limited">
        <section className="changes__files" aria-label="Dernière capture locale">
          <h2 className="changes__files-title">
            Dernière capture locale <span className="changes__count">({patch?.changes.length ?? 0})</span>
          </h2>
          {patch?.changes.length ? (
            <ul className="changes__list">
              {patch.changes.slice(0, 1000).map((change) => (
                <li key={change.path} className="limited-change">
                  <span className="changes__file-text">
                    <span className="changes__file-name">{change.path.split("/").pop()}</span>
                    <span className="changes__file-dir">{change.path}</span>
                  </span>
                  <Badge tone={change.expected === null ? "success" : change.replacement === null ? "danger" : "info"}>
                    {change.expected === null ? "Ajout" : change.replacement === null ? "Suppression" : "Modification"}
                  </Badge>
                  {change.replacement && <span className="changes__file-kind">{formatBytes(change.replacement.bytes)}</span>}
                </li>
              ))}
            </ul>
          ) : (
            <EmptyState compact icon={<CheckCircle2 size={22} />} title={patch ? "Aucune modification capturée." : "Aucune capture pour l’instant."}>
              Le moteur compare le dossier au moment de la capture : la liste apparaît ensuite.
            </EmptyState>
          )}
        </section>
        <section className="changes__diff limited-summary" aria-label="Portée de l’accès limité">
          <div className="limited-summary__body">
            <h2>
              <Lock aria-hidden size={18} /> Ce que couvre cet accès
            </h2>
            <p>
              Seuls les fichiers autorisés par votre compte sont présents. Les noms interdits restent visibles avec un cadenas, sans contenu :{" "}
              <strong>
                {reserved} chemin{reserved > 1 ? "s" : ""} réservé{reserved > 1 ? "s" : ""}
              </strong>
              . <Link to={`/projects/${key}/files`}>Voir les fichiers</Link>
            </p>
            <p>
              Une ancienne copie locale ne donne aucun droit sur le serveur. Créer ou modifier un nom réservé hors de Layrs bloque la capture ; ses octets sont
              conservés.
            </p>
            <div className="limited-summary__actions">
              <Button icon={<GitMerge aria-hidden size={16} />} onClick={() => navigate(`/projects/${key}/merge`)}>
                Fusionner avec la version publiée
              </Button>
              <Button variant="ghost" icon={<RefreshCw aria-hidden size={16} />} disabledReason={workspace.busy ? "Opération en cours." : undefined} onClick={() => void projection.refresh()}>
                Actualiser l’accès limité
              </Button>
            </div>
          </div>
        </section>
      </div>
      <footer className="changes__composer" aria-label="Créer une capture">
        <div className="changes__composer-text">
          <p className="changes__composer-title">Capture des modifications autorisées</p>
          <p className="changes__composer-hint">Enregistrée sur cet ordinateur, elle n’est pas publiée. Publiez-la ensuite depuis Synchronisation.</p>
        </div>
        <Button
          variant="primary"
          size="lg"
          icon={<Camera aria-hidden size={18} />}
          busy={workspace.busy === "Capture…"}
          busyLabel="Capture…"
          disabledReason={workspace.busy === "Capture…" ? undefined : captureReason}
          onClick={() => void projection.capture()}
        >
          Capturer les modifications autorisées
        </Button>
      </footer>
    </div>
  );
}
