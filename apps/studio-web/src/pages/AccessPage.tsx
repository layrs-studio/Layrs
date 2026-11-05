import { useCallback } from "react";
import { AccessPanel, Loading, PageHeader, segment, useDirectory, type NativeProjectionPageRequest } from "@layrs/ui";
import { useTeamContext } from "./team-context";

/** P16 · Équipe et accès : fichiers. */
export function AccessPage() {
  const context = useTeamContext();
  return (
    <div className="layrs-page">
      <PageHeader
        title="Accès aux fichiers"
        description="Qui peut lire ou modifier chaque fichier. Choisissez une personne, puis un chemin pour comprendre ou changer son accès."
      />
      {context.capabilities.loading && !context.capabilities.value ? <Loading label="Lecture de vos droits sur le projet…" /> : <AccessContent context={context} />}
    </div>
  );
}

function AccessContent({ context }: { context: ReturnType<typeof useTeamContext> }) {
  const { project, api, scope, capabilities, policy, self, instanceAdmin } = context;
  const managePolicy = capabilities.flags ? capabilities.flags.managePolicy : null;
  const directory = useDirectory({ scope, administration: project.administration, policy, instanceAdmin, self, administer: managePolicy });
  const base = `/v3/spaces/${segment(project.spaceId)}`;
  const loadContext = useCallback(() => api(`${base}/bootstrap`), [api, base]);
  const loadPage = useCallback((request: NativeProjectionPageRequest) => api(`${base}/projection`, "POST", request), [api, base]);
  return (
    <AccessPanel
      scope={scope}
      directory={directory}
      policy={policy}
      loadContext={loadContext}
      loadPage={loadPage}
      self={self}
      projectName={project.name}
      canChange={managePolicy}
      onChanged={() => void capabilities.reload()}
    />
  );
}
