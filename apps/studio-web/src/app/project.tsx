import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { ProjectLensProvider, type SpaceCapabilities, type SpaceCatalogPage } from "@layrs/ui";
import { administrationTransport, collaborationTransport, type Bootstrap, type Command } from "./transports";
import { describeError, useSession } from "./session";
import { useProjectContexts } from "./contexts";

/*
 * Projects seen during this session, for names and the sidebar. Kept in
 * memory only: nothing about the account's projects is written to storage.
 */
const known = new Map<string, { spaceId: string; name: string; contextId?: string }>();

export function rememberProject(spaceId: string, name?: string, contextId?: string) {
  const previous = known.get(spaceId);
  known.set(spaceId, { spaceId, name: name || previous?.name || spaceId, contextId: contextId || previous?.contextId });
}

export function knownProjects(contextId?: string) {
  return [...known.values()].filter((entry) => !contextId || entry.contextId === contextId);
}

export function forgetKnownProjects() {
  known.clear();
}

type ProjectApi = {
  spaceId: string;
  name: string;
  contextKind: SpaceCapabilities["contextKind"] | null;
  bootstrap: Bootstrap | null;
  error: string;
  loading: boolean;
  reload: () => Promise<void>;
  collaboration: <T>(command: Command) => Promise<T>;
  administration: <T>(command: Command) => Promise<T>;
};

const ProjectContext = createContext<ProjectApi | null>(null);

export function useProject() {
  const value = useContext(ProjectContext);
  if (!value) throw new Error("useProject must be used inside <ProjectProvider>");
  return value;
}

export function ProjectProvider({ spaceId, children }: { spaceId: string; children: ReactNode }) {
  const { api } = useSession();
  const contexts = useProjectContexts();
  const [bootstrap, setBootstrap] = useState<Bootstrap | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [name, setName] = useState(() => known.get(spaceId)?.name ?? spaceId);
  const [contextKind, setContextKind] = useState<SpaceCapabilities["contextKind"] | null>(null);

  // A deep link skips the Projects page: find the name in the account's own
  // catalog so pages never lead with the technical reference.
  useEffect(() => {
    const cached = known.get(spaceId);
    if (!contexts.data || !contexts.selected) return;
    let cancelled = false;
    void (async () => {
      const capabilities = await api<SpaceCapabilities>(`/v3/spaces/${encodeURIComponent(spaceId)}/capabilities`).catch(() => null);
      if (!capabilities || cancelled || !contexts.data?.contexts.some((item) => item.id === capabilities.contextId)) return;
      setContextKind(capabilities.contextKind);
      contexts.adopt(capabilities.contextId);
      if (cached?.contextId === capabilities.contextId && cached.name !== spaceId) return setName(cached.name);
      let cursor: string | null = null;
      do {
        const query = new URLSearchParams({ limit: "100", contextId: capabilities.contextId });
        if (cursor) query.set("cursor", cursor);
        const result: SpaceCatalogPage | null = await api<SpaceCatalogPage>(`/v3/spaces?${query}`).catch(() => null);
        if (!result || cancelled) return;
        result.spaces.forEach((space) => rememberProject(space.spaceId, space.name, capabilities.contextId));
        const found = result.spaces.find((space) => space.spaceId === spaceId);
        if (found) return setName(found.name);
        cursor = result.hasMore ? result.nextCursor : null;
      } while (cursor && !cancelled);
    })();
    return () => {
      cancelled = true;
    };
  }, [api, spaceId, contexts.data, contexts.selected?.id, contexts.adopt]);

  const reload = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const next = await api<Bootstrap>(`/v3/spaces/${encodeURIComponent(spaceId)}/bootstrap`);
      setBootstrap(next);
      rememberProject(spaceId);
    } catch (cause) {
      setBootstrap(null);
      setError(describeError(cause));
    } finally {
      setLoading(false);
    }
  }, [api, spaceId]);

  useEffect(() => {
    setBootstrap(null);
    void reload();
  }, [reload]);

  const value = useMemo<ProjectApi>(
    () => ({
      spaceId,
      name,
      contextKind,
      bootstrap,
      error,
      loading,
      reload,
      collaboration: collaborationTransport(api, spaceId),
      administration: administrationTransport(api, spaceId),
    }),
    [api, spaceId, name, contextKind, bootstrap, error, loading, reload],
  );
  return <ProjectContext.Provider value={value}><ProjectLensProvider scope={`${location.origin}|${spaceId}`} hostUrl="/v3/lens-runtime/host" transport={value.administration}>{children}</ProjectLensProvider></ProjectContext.Provider>;
}
