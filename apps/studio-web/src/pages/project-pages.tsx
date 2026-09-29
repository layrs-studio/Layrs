import { useState } from "react";
import { NativeAdminSurface, PageHeader, navigate, segment } from "@layrs/ui";
import { useProject } from "../app/project";
import { useSession } from "../app/session";
import { administrationTransport } from "../app/transports";

/*
 * Team and instance administration still embed the shared legacy surface
 * until their redesign lot. The collaboration pages (files, history, reviews,
 * versions) live in their own files.
 */

function scopeOf(principalId: string, spaceId: string) {
  return `${principalId}:${spaceId}`;
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
