import { useEffect, useState } from "react";
import { CloudDownload, CloudUpload, Plug, RefreshCw } from "lucide-react";
import { Badge, Banner, Button, Spinner, StatusDot, formatDateTime, formatRelativeTime, navigate, segment, type Tone } from "@layrs/ui";
import { useProjection } from "../app/projection";
import { useRemote, type Notice, type Session, type Transfer } from "../app/remote";
import { hostOf, type ActivityEntry, type ActivityOutcome } from "../app/serverMemory";
import { useWorkspace } from "../app/workspace";

export function sessionSummary(session: Session): { tone: Tone | "accent"; label: string } {
  switch (session.state) {
    case "connected":
      return { tone: "success", label: "Connecté" };
    case "checking":
      return { tone: "info", label: "Vérification…" };
    case "signedOut":
      return { tone: "warning", label: "Session à renouveler" };
    case "unreachable":
      return { tone: "danger", label: "Serveur injoignable" };
    case "unknown":
      return { tone: "info", label: "Session non vérifiée" };
    default:
      return { tone: "info", label: "Aucun serveur" };
  }
}

export function connectPath(root: string) {
  return `/projects/${segment(root)}/connect`;
}

/** Server, account and the next step to reach a working session. */
export function ServerSummary({ compact }: { compact?: boolean }) {
  const remote = useRemote();
  const { session, origin } = remote;
  const summary = sessionSummary(session);
  const connect = () => navigate(connectPath(remote.root), { query: { from: window.location.hash.slice(1) } });
  return (
    <div className="sync-server">
      <div className="sync-server__text">
        <StatusDot tone={summary.tone}>
          <span className="sync-server__state">{summary.label}</span>
        </StatusDot>
        <p className="sync-server__meta">
          {origin ? (
            <>
              <span title={origin}>{hostOf(origin)}</span>
              {session.state === "connected" && <> · {session.displayName || session.principalId}</>}
            </>
          ) : (
            "Ce projet n’est relié à aucun serveur."
          )}
        </p>
        {session.state === "unreachable" && !compact && <p className="sync-server__hint">Vérifiez le réseau ou le certificat d’entreprise du serveur.</p>}
      </div>
      {session.state === "checking" ? (
        <Spinner size={14} label="Vérification de la session" />
      ) : session.state === "connected" ? null : session.state === "unreachable" || session.state === "unknown" ? (
        <Button size="sm" icon={<RefreshCw aria-hidden size={14} />} onClick={() => void remote.check()}>
          Réessayer
        </Button>
      ) : (
        <Button size="sm" variant={compact ? "secondary" : "primary"} icon={<Plug aria-hidden size={14} />} onClick={connect}>
          {origin ? "Se connecter…" : "Connecter un serveur…"}
        </Button>
      )}
    </div>
  );
}

export function NoticeView({ notice, onClose }: { notice: Notice | null; onClose?: () => void }) {
  if (!notice) return null;
  return (
    <Banner
      tone={notice.tone}
      title={notice.title}
      actions={
        onClose && (
          <Button size="sm" variant="ghost" onClick={onClose}>
            Fermer
          </Button>
        )
      }
    >
      {notice.body}
      {notice.detail && <code className="sync-detail">{notice.detail}</code>}
    </Banner>
  );
}

function useNow(active: boolean) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}

function elapsed(from: string, now: number) {
  const seconds = Math.max(0, Math.round((now - Date.parse(from)) / 1000));
  return seconds < 60 ? `${seconds} s` : `${Math.floor(seconds / 60)} min ${seconds % 60} s`;
}

/** The running transfer, its observed steps and cancellation when the protocol allows it. */
export function TransferView({ transfer }: { transfer: Transfer }) {
  const remote = useRemote();
  const now = useNow(true);
  return (
    <section className="sync-transfer" aria-label="Transfert en cours">
      <div className="sync-transfer__head">
        <Spinner size={14} />
        <p className="sync-transfer__title">{transfer.label}</p>
        <span className="sync-transfer__time">{elapsed(transfer.startedAt, now)}</span>
      </div>
      <ol className="sync-steps">
        {transfer.steps.map((step, index) => (
          <li key={index} role={index === transfer.steps.length - 1 ? "status" : undefined}>
            {step}
          </li>
        ))}
      </ol>
      {transfer.transferId && (
        <Button size="sm" disabledReason={transfer.cancelling ? "Arrêt déjà demandé." : undefined} onClick={() => void remote.cancelTransfer()}>
          Annuler le transfert
        </Button>
      )}
    </section>
  );
}

const outcomeLabels: Record<ActivityOutcome, { label: string; tone: Tone | "accent" }> = {
  running: { label: "En cours", tone: "accent" },
  success: { label: "Terminé", tone: "success" },
  cancelled: { label: "Annulé", tone: "info" },
  failed: { label: "Échec", tone: "danger" },
  unconfirmed: { label: "À confirmer", tone: "warning" },
  interrupted: { label: "Interrompu", tone: "warning" },
};

export function ActivityList({ entries, limit }: { entries: ActivityEntry[]; limit?: number }) {
  const shown = limit ? entries.slice(0, limit) : entries;
  if (!shown.length) return <p className="sync-muted">Aucun transfert sur cet ordinateur pour ce projet.</p>;
  return (
    <ol className="sync-activity">
      {shown.map((entry) => {
        const outcome = outcomeLabels[entry.outcome];
        return (
          <li key={entry.id} className="sync-activity__item">
            <div className="sync-activity__text">
              <p className="sync-activity__title">
                {entry.label} <Badge tone={outcome.tone}>{outcome.label}</Badge>
              </p>
              {entry.message && <p className="sync-activity__message">{entry.message}</p>}
              {!limit && entry.steps.length > 1 && <p className="sync-activity__steps">Étapes observées : {entry.steps.join(" → ")}</p>}
            </div>
            <time dateTime={entry.startedAt} title={formatDateTime(entry.startedAt)}>
              {formatRelativeTime(entry.startedAt)}
            </time>
          </li>
        );
      })}
    </ol>
  );
}

/** Publish and Receive, distinct, for the current kind of folder. */
export function SyncActions() {
  const workspace = useWorkspace();
  return workspace.kind === "projection" ? <LimitedSyncActions /> : <CompleteSyncActions />;
}

function CompleteSyncActions() {
  const workspace = useWorkspace();
  const remote = useRemote();
  const { busy, data } = workspace;
  const connected = remote.session.state === "connected";
  const reason = !remote.origin
    ? "Connectez d’abord un serveur."
    : !connected
      ? "Connectez-vous d’abord à ce serveur."
      : remote.transfer
        ? "Un transfert est déjà en cours."
        : busy
          ? `Opération en cours : ${busy}`
          : data?.needsRepair
            ? "Une opération interrompue doit d’abord être reprise."
            : undefined;
  return (
    <div className="sync-actions">
      <Button
        variant="primary"
        icon={<CloudUpload aria-hidden size={16} />}
        busy={busy === "Publication…"}
        busyLabel="Publication…"
        disabledReason={busy === "Publication…" ? undefined : reason}
        onClick={() => void remote.operate({ kind: "publish", label: "Publication des captures", busy: "Publication…", context: "publish", cancellable: true }, { action: "publish", integrate: false })}
      >
        Publier
      </Button>
      <Button
        icon={<CloudDownload aria-hidden size={16} />}
        busy={busy === "Réception…"}
        busyLabel="Réception…"
        disabledReason={busy === "Réception…" ? undefined : reason}
        onClick={() => void remote.operate({ kind: "receive", label: "Réception de l’état de l’équipe", busy: "Réception…", context: "receive", cancellable: true }, { action: "receive" })}
      >
        Recevoir
      </Button>
    </div>
  );
}

function LimitedSyncActions() {
  const workspace = useWorkspace();
  const projection = useProjection();
  const remote = useRemote();
  const { data, patch, ready, target } = projection;
  const connected = remote.session.state === "connected";
  const common = !connected
    ? "Connectez-vous d’abord au serveur de cet accès limité."
    : workspace.busy
      ? `Opération en cours : ${workspace.busy}`
      : !data || !ready
        ? "Actualisez l’accès limité avant de continuer."
        : !target?.layerId
          ? "Choisissez le Layer distant cible dans Synchronisation."
          : undefined;
  const publishReason =
    common ??
    (data?.latestCaptureImported === true
      ? "Créez une capture locale dans cette copie avant de publier son historique importé."
      : data?.capabilities.publish !== true
      ? "La publication n’est pas disponible dans cette connexion."
      : !patch
        ? "Capturez d’abord vos modifications autorisées."
        : data.pendingPublication?.intent === "candidate"
          ? "Un envoi pour revue attend son reçu : relancez-le depuis Équipe."
          : undefined);
  const receiveReason =
    common ??
    (data?.capabilities.receive !== true
      ? "La réception n’est pas disponible dans cet état."
      : data.pendingPublication
        ? "Une publication attend son reçu : relancez-la d’abord."
        : undefined);
  return (
    <div className="sync-actions">
      <Button
        variant="primary"
        icon={<CloudUpload aria-hidden size={16} />}
        busy={workspace.busy === "Publication…"}
        busyLabel="Publication…"
        disabledReason={workspace.busy === "Publication…" ? undefined : publishReason}
        onClick={() => void projection.publish()}
      >
        {data?.pendingPublication?.intent === "advance" ? "Relancer la publication" : "Publier la capture"}
      </Button>
      <Button
        icon={<CloudDownload aria-hidden size={16} />}
        busy={workspace.busy === "Réception…"}
        busyLabel="Réception…"
        disabledReason={workspace.busy === "Réception…" ? undefined : receiveReason}
        onClick={() => void projection.receive()}
      >
        Recevoir
      </Button>
    </div>
  );
}

/** Durable facts that make a transfer incomplete, read from the local store. */
export function PendingState() {
  const workspace = useWorkspace();
  if (workspace.kind !== "projection") return null;
  return <LimitedPending />;
}

function LimitedPending() {
  const { data, target } = useProjection();
  const pending = data?.pendingPublication;
  return (
    <>
      <p className="sync-target">
        Layer distant cible : <strong>{target?.name || target?.layerId || "non choisi"}</strong>
      </p>
      {pending && (
        <Banner tone="warning" title={pending.intent === "candidate" ? "Un envoi pour revue attend son reçu." : "Une publication attend son reçu."}>
          Rejouez la capture {pending.sequence} vers sa cible enregistrée : le serveur renvoie le même reçu, sans doublon. La capture locale est conservée.
        </Banner>
      )}
    </>
  );
}
