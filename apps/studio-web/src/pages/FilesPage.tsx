import { Badge, PageHeader, ProjectFiles, navigate } from "@layrs/ui";
import { useCollabContext } from "../app/collab";
import { useSession } from "../app/session";

/** P06 — published files of the project, as the server authorizes them. */
export function FilesPage() {
  const { project, scope, base, references, capabilities } = useCollabContext();
  const { api } = useSession();
  const space = `/v3/spaces/${encodeURIComponent(project.spaceId)}`;
  return (
    <div className="layrs-page">
      <PageHeader
        title="Fichiers"
        description="Version publiée du projet, selon vos droits d’accès. Rien n’est écrit sur cet ordinateur."
        aside={references.restricted ? <Badge tone="warning">Accès limité</Badge> : undefined}
      />
      <ProjectFiles
        key={scope}
        scope={scope}
        capabilities={capabilities}
        loadContext={() => api(`${space}/bootstrap`)}
        loadPage={(request) => api(`${space}/projection`, "POST", request)}
        onOpenHistory={(path) => navigate(`${base}/history`, { query: { path } })}
      />
    </div>
  );
}
