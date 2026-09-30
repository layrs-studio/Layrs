import { useState } from "react";
import { Send, Users } from "lucide-react";
import {
  Button,
  Card,
  EmptyState,
  NativeAdminSurface,
  NativeHistorySurface,
  NativeReleaseSurface,
  NativeRemoteProjectFiles,
  NativeReviewSurface,
  PageHeader,
  type NativeReviewContext,
} from "@layrs/ui";
import { NativeScopedReviews } from "../NativeScopedReviews";
import { useProjection } from "../app/projection";
import { useRemote } from "../app/remote";
import { useWorkspace } from "../app/workspace";
import { ServerSummary } from "../sync/parts";

/**
 * Team area of a project, reached after connection. The shared review, files
 * and administration surfaces keep their own behaviour here until their
 * redesign lands; this page only frames and connects them.
 */
export function TeamPage() {
  const workspace = useWorkspace();
  const remote = useRemote();
  const connected = remote.session.state === "connected";
  return (
    <div className="layrs-page">
      <PageHeader
        title="Équipe"
        description="Revues, versions publiées, fichiers du serveur et administration du projet. Ces vues lisent le serveur ; elles ne modifient pas ce dossier."
      />
      {!connected ? (
        <Card className="team-connect">
          <EmptyState icon={<Users size={22} />} title="Connectez ce projet au serveur de votre équipe">
            Les revues et l’administration exigent une session sur le serveur. Vos captures locales restent utilisables sans connexion.
          </EmptyState>
          <ServerSummary />
        </Card>
      ) : workspace.kind === "projection" ? (
        <LimitedTeam />
      ) : (
        <CompleteTeam />
      )}
    </div>
  );
}

function useSurfaceLock() {
  const workspace = useWorkspace();
  const remote = useRemote();
  return {
    busy: Boolean(workspace.busy) || !remote.origin,
    onBusy: (value: boolean) => workspace.setForeground(value ? "Équipe…" : null),
  };
}

function CompleteTeam() {
  const workspace = useWorkspace();
  const remote = useRemote();
  const { busy, onBusy } = useSurfaceLock();
  const [selectedReview, setSelectedReview] = useState<NativeReviewContext | null>(null);
  const principal = remote.session.state === "connected" ? remote.session.principalId : "";
  const scope = `${remote.epoch}:${remote.origin}:${workspace.root}:${principal}`;
  const collaboration = <T,>(command: { action: string; [key: string]: unknown }) => remote.transport<T>({ action: "collaboration", command });
  return (
    <section className="team layrs-legacy" aria-label="Équipe">
      <Card className="team__card">
        <h2>Revues, versions et historique publiés</h2>
        <NativeReviewSurface
          key={`reviews:${scope}`}
          spaceId={workspace.data?.state.space.spaceId}
          selectedReview={selectedReview}
          draftIdentity={principal ? { origin: remote.origin, principalId: principal } : undefined}
          scope={`${workspace.root}|${remote.origin}|${principal}`}
          busy={busy}
          onBusy={onBusy}
          transport={collaboration}
        />
        <NativeReleaseSurface scope={`${workspace.root}|${remote.origin}`} busy={busy} onBusy={onBusy} transport={collaboration} />
        <NativeHistorySurface scope={`${workspace.root}|${remote.origin}|${principal}`} busy={busy || !principal} onBusy={onBusy} transport={collaboration} />
      </Card>
      <Card className="team__card">
        <h2>Fichiers du serveur</h2>
        <NativeRemoteProjectFiles
          key={`files:${scope}`}
          scope={`${scope}:${remote.authority}`}
          busy={busy}
          onBusy={onBusy}
          loadContext={() => remote.transport({ action: "collaboration", command: { action: "projectionContext" } })}
          loadPage={(request) => remote.transport({ action: "collaboration", command: { action: "projectionPage", ...request } })}
        />
      </Card>
      <Card className="team__card">
        <h2>Administration et accès</h2>
        <NativeAdminSurface
          key={`admin:${scope}`}
          scope={scope}
          busy={busy}
          onBusy={onBusy}
          onOpenReview={(review) => {
            setSelectedReview(review);
            document.querySelector(".team__card")?.scrollIntoView({ block: "start" });
          }}
          transport={<T,>(command: { action: string; [key: string]: unknown }) => remote.transport<T>({ action: "administration", command })}
          policyTransport={<T,>(action: { action: string; [key: string]: unknown }) => remote.transport<T>(action)}
        />
      </Card>
    </section>
  );
}

function LimitedTeam() {
  const workspace = useWorkspace();
  const projection = useProjection();
  const remote = useRemote();
  const { data, patch, ready, target } = projection;
  const reason = workspace.busy
    ? `Opération en cours : ${workspace.busy}`
    : !data || !ready
      ? "Actualisez l’accès limité avant de continuer."
      : !patch
        ? "Capturez d’abord vos modifications autorisées."
        : !target?.layerId
          ? "Choisissez le Layer distant cible dans Synchronisation."
          : data.capabilities.publish !== true
            ? "La publication n’est pas disponible dans cette connexion."
            : data.pendingPublication?.intent === "advance"
              ? "Une publication attend son reçu : relancez-la d’abord."
              : undefined;
  return (
    <section className="team" aria-label="Équipe">
      <Card className="team__card team__candidate">
        <div>
          <h2>Proposer votre capture</h2>
          <p className="sync-muted">
            Envoyez la dernière capture locale comme candidat : le Layer reste à sa version actuelle jusqu’à l’intégration d’une revue approuvée.
          </p>
        </div>
        <Button icon={<Send aria-hidden size={16} />} disabledReason={reason} onClick={() => void projection.publish(true)}>
          {data?.pendingPublication?.intent === "candidate" ? "Relancer l’envoi pour revue" : "Envoyer la capture pour revue"}
        </Button>
      </Card>
      {data && (
        <div className="layrs-legacy team__card">
          <NativeScopedReviews
            key={`${workspace.root}:${data.identity.principalId}:${remote.epoch}`}
            root={workspace.root}
            origin={data.identity.origin}
            ca={remote.authority}
            disabled={Boolean(workspace.busy) || !ready}
            candidateId={projection.candidateId}
          />
        </div>
      )}
    </section>
  );
}
