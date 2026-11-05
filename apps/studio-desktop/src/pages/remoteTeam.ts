import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  emptyReferences,
  projectReferences,
  useCapabilities,
  type CollabCapabilities,
  type PeopleDirectory,
  type Person,
  type NativeProjectionContext,
  type NativeProjectionPage,
  type NativeProjectionPageRequest,
  type PolicyTransport,
  type SpaceCapabilities,
  type TeamTransport,
  type WhoAmI,
} from "@layrs/ui";
import { useRemote } from "../app/remote";
import { useWorkspace } from "../app/workspace";

type Bootstrap = {
  spaceId: string;
  layers?: { record: { layerId: string; name: string }; headStepIds: string[] }[];
};
type Command = { action: string; [key: string]: unknown };
const collaborationReads = new Set([
  "projectionContext", "projectionPage", "stepHistory", "listReleases", "releasePublications",
  "compare", "runDetail", "listReviews", "listReviewsPage", "review", "listRuns", "listRunsPage", "protection",
]);
const administrationReads = new Set([
  "listLenses", "lensCatalog", "projectLenses", "lensPacket", "filePreview",
  "listTeams", "policyManagement", "eligibleTeamMembers", "teamHistory", "metrics", "invitationInbox",
  "listInvitations", "storageBudget", "listAccounts", "listTokens", "protection", "listWebhooks",
  "webhookDeliveries", "notifications", "organisationSettings",
]);
const policyReads = new Set(["readPolicyRules", "readPendingPolicy"]);

/** Tauri rejects with strings; shared panels need HTTP status and dispatch state. */
export function normalizeTeamError(cause: unknown): Error & { code?: string; status?: number; dispatched?: boolean } {
  const message = cause instanceof Error ? cause.message : String(cause);
  const error = new Error(message) as Error & { code?: string; status?: number; dispatched?: boolean };
  error.code = /^([a-z_]+):/.exec(message)?.[1];
  const status = /remote HTTP status (\d{3})/.exec(message)?.[1];
  if (status) error.status = Number(status);
  if (["remote_scope_changed", "remote_principal_changed", "remote_identity_check_failed", "remote_lock_unavailable", "workspace_busy"].includes(error.code ?? ""))
    error.dispatched = false;
  return error;
}

/** Bind all remote data to the folder, server and authenticated account. */
export function useRemoteTeam() {
  const workspace = useWorkspace();
  const remote = useRemote();
  const principalId = remote.session.state === "connected" ? remote.session.principalId : "";
  const spaceId = workspace.data?.state.space.spaceId ?? "";
  const scope = `${workspace.root}|${remote.origin}|${remote.epoch}|${principalId}|${spaceId}`;
  const [bootstrap, setBootstrap] = useState<{ scope: string; value: Bootstrap } | null>(null);
  const [who, setWho] = useState<{ scope: string; value: WhoAmI } | null>(null);
  const [error, setError] = useState("");
  const [whoError, setWhoError] = useState("");
  const generation = useRef(0);

  const send = useCallback(<T,>(action: Command): Promise<T> =>
    remote.transport<T>(action, { root: workspace.root, origin: remote.origin, principalId }).catch((cause) => { throw normalizeTeamError(cause); }),
  [remote.transport, remote.origin, workspace.root, principalId]);

  const locked = useCallback(async <T,>(label: string, work: () => Promise<T>): Promise<T> => {
    if (!workspace.acquire(label)) {
      const error = normalizeTeamError("workspace_busy: Une autre opération est en cours dans ce dossier. Réessayez après sa fin.");
      throw error;
    }
    try {
      return await work();
    } finally {
      workspace.release();
    }
  }, [workspace.acquire, workspace.release]);

  const collaboration = useCallback(
    <T,>(command: Command) => {
      const work = () => send<T>({ action: "collaboration", command });
      return collaborationReads.has(command.action) ? work() : locked("Collaboration…", work);
    },
    [send, locked],
  );
  const administration = useCallback(
    <T,>(command: Command) => {
      const work = () => send<T>({ action: "administration", command });
      return administrationReads.has(command.action) ? work() : locked("Administration…", work);
    },
    [send, locked],
  );
  const policy = useCallback<PolicyTransport>(
    <T,>(command: Command) => {
      if (command.action === "collaboration") {
        const inner = command.command as Command;
        return collaboration<T>(inner);
      }
      const work = () => send<T>(command);
      return policyReads.has(command.action) ? work() : locked("Règles d’accès…", work);
    },
    [send, collaboration, locked],
  );
  const loadCapabilities = useCallback(
    () => send<SpaceCapabilities>({ action: "spaceCapabilities", space: spaceId }),
    [send, spaceId],
  );
  const capabilities = useCapabilities(scope, spaceId && principalId ? loadCapabilities : undefined);

  useEffect(() => {
    const current = ++generation.current;
    setBootstrap(null);
    setWho(null);
    setError("");
    setWhoError("");
    if (!principalId) return () => { generation.current++; };
    // Instance administration only needs whoami. An inaccessible local
    // project's bootstrap must not hide instance accounts or status.
    void send<WhoAmI>({ action: "whoami" }).then(
      (person) => {
        if (current !== generation.current) return;
        if (person.principal_id !== principalId) {
          setWhoError("La réponse du serveur appartient à un autre compte. Actualisez la session.");
          return;
        }
        setWho({ scope, value: person });
      },
      (cause) => {
        if (current === generation.current) setWhoError(String(cause));
      },
    );
    if (spaceId) void collaboration<Bootstrap>({ action: "projectionContext" }).then(
      (next) => {
        if (current !== generation.current) return;
        if (next.spaceId !== spaceId) {
          setError("La réponse du serveur appartient à un autre projet. Actualisez la session.");
          return;
        }
        setBootstrap({ scope, value: next });
      },
      (cause) => {
        if (current === generation.current) setError(String(cause));
      },
    );
    return () => { generation.current++; };
  }, [scope, spaceId, principalId, collaboration, send]);

  const validBootstrap = bootstrap?.scope === scope ? bootstrap.value : null;
  const validWho = who?.scope === scope ? who.value : null;
  const references = useMemo(() => (validBootstrap ? projectReferences(validBootstrap) : emptyReferences), [validBootstrap]);
  const people = useMemo<PeopleDirectory>(
    () => ({
      currentPrincipalId: principalId || undefined,
      names: validWho ? { [principalId]: validWho.display_name } : remote.session.state === "connected" && remote.session.displayName ? { [principalId]: remote.session.displayName } : {},
    }),
    [principalId, validWho, remote.session],
  );
  const self = useMemo<Person | null>(
    () => validWho ? { principalId, displayName: validWho.display_name || "Vous", kind: validWho.kind } : null,
    [principalId, validWho],
  );

  return {
    scope,
    spaceId,
    projectName: workspace.data?.state.space.name ?? "Projet",
    bootstrap: validBootstrap,
    who: validWho,
    error: error || whoError,
    whoError,
    references,
    people,
    self,
    capabilities,
    flags: capabilities.value?.spaceId === spaceId ? capabilities.flags as CollabCapabilities | null : null,
    collaboration,
    administration: administration as TeamTransport,
    policy,
    loadContext: useCallback(() => collaboration<NativeProjectionContext>({ action: "projectionContext" }), [collaboration]),
    loadPage: useCallback(
      (request: NativeProjectionPageRequest) =>
        collaboration<NativeProjectionPage>({ action: "projectionPage", ...request }),
      [collaboration],
    ),
    busy: Boolean(workspace.busy),
  };
}

export type RemoteTeam = ReturnType<typeof useRemoteTeam>;
