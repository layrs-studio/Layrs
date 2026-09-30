import { useCallback, useMemo } from "react";
import { segment, useCapabilities, type Person, type SpaceCapabilities } from "@layrs/ui";
import { webPolicyTransport } from "../app/policy/transport";
import { useProject } from "../app/project";
import { useSession } from "../app/session";

/**
 * What the team, access and settings pages share for one project: a scope
 * that changes with account, origin and project (so late answers are
 * dropped), the capability hint, and the Web policy transport.
 */
export function useTeamContext() {
  const project = useProject();
  const { api, principal } = useSession();
  const scope = `${location.origin}:${principal?.principalId ?? ""}:${project.spaceId}`;
  const load = useCallback(() => api<SpaceCapabilities>(`/v3/spaces/${segment(project.spaceId)}/capabilities`), [api, project.spaceId]);
  const capabilities = useCapabilities(scope, load);
  // One transport per project and session: its client identity scopes retries.
  const policy = useMemo(() => webPolicyTransport(api, project.spaceId), [api, project.spaceId]);
  const self: Person | null = useMemo(
    () => (principal ? { principalId: principal.principalId, displayName: principal.displayName || "Vous", kind: principal.kind } : null),
    [principal],
  );
  return { project, api, principal, scope, capabilities, policy, self, instanceAdmin: principal?.instanceAdministrator === true };
}
