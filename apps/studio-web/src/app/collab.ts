import { useEffect, useMemo, useState } from "react";
import { projectReferences, segment, type CollabCapabilities, type PeopleDirectory, type SpaceCapabilities } from "@layrs/ui";
import { useProject } from "./project";
import { useSession } from "./session";

/**
 * Context shared by the collaboration pages of one project: the scope that
 * binds every response to this origin, account and project, names known to
 * the client, and the L0 capability hints (the server still decides).
 */
export function useCollabContext() {
  const project = useProject();
  const { api, principal } = useSession();
  const principalId = principal?.principalId ?? "";
  const scope = `${location.origin}|${principalId}|${project.spaceId}`;
  const [capabilities, setCapabilities] = useState<{ scope: string; flags: CollabCapabilities } | null>(null);

  useEffect(() => {
    let current = true;
    api<SpaceCapabilities>(`/v3/spaces/${encodeURIComponent(project.spaceId)}/capabilities`).then(
      (answer) => {
        if (current && answer?.spaceId === project.spaceId) setCapabilities({ scope, flags: answer.capabilities });
      },
      // Unknown capabilities keep every action offered; the server answers each request.
      () => undefined,
    );
    return () => {
      current = false;
    };
  }, [api, scope]);

  const references = useMemo(() => projectReferences(project.bootstrap), [project.bootstrap]);
  const people = useMemo<PeopleDirectory>(
    () => ({ currentPrincipalId: principalId || undefined, names: principalId && principal?.displayName ? { [principalId]: principal.displayName } : {} }),
    [principalId, principal?.displayName],
  );

  return {
    project,
    scope,
    base: `/projects/${segment(project.spaceId)}`,
    references,
    people,
    capabilities: capabilities?.scope === scope ? capabilities.flags : undefined,
    draftIdentity: principalId ? { origin: location.origin, principalId } : undefined,
  };
}
