import { useState } from "react";
import { GitBranch } from "lucide-react";
import {
  Badge,
  Card,
  NativeAdminSurface,
  NativeHistorySurface,
  NativeReleaseSurface,
  NativeRemoteProjectFiles,
  NativeReviewSurface,
  PageHeader,
  TechnicalId,
  navigate,
  segment,
  type NativeReviewContext,
} from "@layrs/ui";
import { useProject } from "../app/project";
import { useSession } from "../app/session";
import { administrationTransport } from "../app/transports";

/*
 * First routed version of the project pages. The shared surfaces keep their
 * proven behaviour (pagination, drafts, exact versions) until their redesign
 * lot; each page owns its busy state so reading another page stays possible.
 */

function scopeOf(principalId: string, spaceId: string) {
  return `${principalId}:${spaceId}`;
}

export function FilesPage() {
  const project = useProject();
  const { api, principal } = useSession();
  const [busy, setBusy] = useState(false);
  const base = `/v3/spaces/${encodeURIComponent(project.spaceId)}`;
  return (
    <div className="layrs-page">
      <PageHeader title="Fichiers" description="Version publiée du projet, selon vos droits d’accès." />
      <div className="layrs-legacy">
        <NativeRemoteProjectFiles
          key={`files:${principal?.principalId}:${project.spaceId}`}
          scope={`${location.origin}:${scopeOf(principal?.principalId ?? "", project.spaceId)}`}
          busy={busy}
          onBusy={setBusy}
          loadContext={() => api(`${base}/bootstrap`)}
          loadPage={(request) => api(`${base}/projection`, "POST", request)}
        />
      </div>
    </div>
  );
}

export function HistoryPage() {
  const project = useProject();
  const { principal } = useSession();
  const [busy, setBusy] = useState(false);
  return (
    <div className="layrs-page">
      <PageHeader title="Historique" description="Captures publiées et leur provenance, sans copier d’identifiant." />
      <div className="layrs-legacy">
        <NativeHistorySurface
          key={`history:${principal?.principalId}:${project.spaceId}`}
          scope={scopeOf(principal?.principalId ?? "", project.spaceId)}
          transport={project.collaboration}
          busy={busy}
          onBusy={setBusy}
        />
      </div>
    </div>
  );
}

export function ReviewsPage({ selected }: { selected: NativeReviewContext | null }) {
  const project = useProject();
  const { principal } = useSession();
  const [busy, setBusy] = useState(false);
  return (
    <div className="layrs-page">
      <PageHeader title="Revues" description="Propositions, discussions et décisions sur des versions précises." />
      <div className="layrs-legacy">
        <NativeReviewSurface
          key={`reviews:${principal?.principalId}:${project.spaceId}`}
          spaceId={project.spaceId}
          selectedReview={selected}
          draftIdentity={{ origin: location.origin, principalId: principal?.principalId ?? "" }}
          scope={scopeOf(principal?.principalId ?? "", project.spaceId)}
          busy={busy}
          onBusy={setBusy}
          transport={project.collaboration}
        />
      </div>
    </div>
  );
}

export function VersionsPage() {
  const project = useProject();
  const { principal } = useSession();
  const [busy, setBusy] = useState(false);
  const bootstrap = project.bootstrap;
  return (
    <div className="layrs-page">
      <PageHeader title="Versions et Layers" description="Lignes de travail publiées, repères et versions rendues disponibles à l’équipe." />
      {bootstrap && (
        <section aria-label="Layers et captures publiées" className="settings-section">
          <h2>Layers et captures publiées</h2>
          <Card>
            <ul className="layer-list">
              {bootstrap.layers.map((layer) => (
                <li key={layer.record.layerId} className="layer-list__item">
                  <GitBranch aria-hidden size={18} />
                  <div className="layer-list__text">
                    <p className="layer-list__name">{layer.record.name}</p>
                    {layer.headStepIds.map((id) => (
                      <TechnicalId key={id} value={id} label="Copier la capture" />
                    ))}
                  </div>
                  {layer.headStepIds.length > 1 && <Badge tone="warning">Plusieurs têtes : intégration nécessaire</Badge>}
                </li>
              ))}
            </ul>
          </Card>
          <h2>Versions nommées</h2>
          <Card>
            <ul className="layer-list">
              {bootstrap.markers.map((marker, index) => (
                <li key={`marker-${index}`} className="layer-list__item">
                  <div className="layer-list__text">
                    <p className="layer-list__name">{marker.record.name}</p>
                    <TechnicalId value={marker.record.targetStepId} label="Copier la capture" />
                  </div>
                  <Badge>Repère</Badge>
                </li>
              ))}
              {bootstrap.releases.map((release, index) => (
                <li key={`release-${index}`} className="layer-list__item">
                  <div className="layer-list__text">
                    <p className="layer-list__name">{release.record.version}</p>
                    <TechnicalId value={release.record.targetStepId} label="Copier la capture" />
                  </div>
                  <Badge tone="accent">Version</Badge>
                </li>
              ))}
              {!bootstrap.markers.length && !bootstrap.releases.length && <li className="layer-list__item">Aucune version nommée.</li>}
            </ul>
          </Card>
        </section>
      )}
      <div className="layrs-legacy">
        <NativeReleaseSurface
          key={`release:${principal?.principalId}:${project.spaceId}`}
          scope={scopeOf(principal?.principalId ?? "", project.spaceId)}
          transport={project.collaboration}
          busy={busy}
          onBusy={setBusy}
        />
      </div>
    </div>
  );
}

export function TeamPage() {
  const project = useProject();
  const { principal } = useSession();
  const [busy, setBusy] = useState(false);
  return (
    <div className="layrs-page">
      <PageHeader title="Équipe et accès" description="Membres, invitations, équipes, protections et intégrations de ce projet." />
      <div className="layrs-legacy">
        <NativeAdminSurface
          key={`admin:${principal?.principalId}:${project.spaceId}`}
          scope={scopeOf(principal?.principalId ?? "", project.spaceId)}
          principalKind={principal?.kind || undefined}
          transport={project.administration}
          busy={busy}
          onBusy={setBusy}
          onOpenReview={(context) =>
            navigate(`/projects/${segment(project.spaceId)}/reviews/${segment(context.reviewId)}`, { query: { version: String(context.version) } })
          }
        />
      </div>
    </div>
  );
}

/** Instance administration, outside any project. */
export function AdminPage() {
  const { api, principal } = useSession();
  const [busy, setBusy] = useState(false);
  return (
    <div className="layrs-page">
      <PageHeader title="Administration de l’instance" description="Comptes, comptes de service et état du serveur. Réservé aux administrateurs." />
      <div className="layrs-legacy">
        <NativeAdminSurface
          key={`instance:${principal?.principalId}`}
          scope={`${principal?.principalId}:`}
          principalKind={principal?.kind || undefined}
          transport={administrationTransport(api, "")}
          busy={busy}
          onBusy={setBusy}
        />
      </div>
    </div>
  );
}
