import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import type { SpaceCatalogPage } from "@layrs/ui";
import { administrationTransport, collaborationTransport, type Bootstrap, type Command } from "./transports";
import { describeError, useSession } from "./session";

/*
 * Projects seen during this session, for names and the sidebar. Kept in
 * memory only: nothing about the account's projects is written to storage.
 */
const known = new Map<string, { spaceId: string; name: string }>();

export function rememberProject(spaceId: string, name?: string) {
  const previous = known.get(spaceId);
  known.set(spaceId, { spaceId, name: name || previous?.name || spaceId });
}

export function knownProjects() {
  return [...known.values()];
}

export function forgetKnownProjects() {
  known.clear();
}

type ProjectApi = {
  spaceId: string;
  name: string;
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
  const [bootstrap, setBootstrap] = useState<Bootstrap | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [name, setName] = useState(() => known.get(spaceId)?.name ?? spaceId);

  // A deep link skips the Projects page: find the name in the account's own
  // catalog so pages never lead with the technical reference.
  useEffect(() => {
    const cached = known.get(spaceId)?.name;
    if (cached && cached !== spaceId) return;
    let cancelled = false;
    void (async () => {
      let cursor: string | null = null;
      for (let page = 0; page < 5 && !cancelled; page++) {
        const query = new URLSearchParams({ limit: "100" });
        if (cursor) query.set("cursor", cursor);
        const result: SpaceCatalogPage | null = await api<SpaceCatalogPage>(`/v3/spaces?${query}`).catch(() => null);
        if (!result || cancelled) return;
        result.spaces.forEach((space) => rememberProject(space.spaceId, space.name));
        const found = result.spaces.find((space) => space.spaceId === spaceId);
        if (found) return setName(found.name);
        if (!result.hasMore || !result.nextCursor) return;
        cursor = result.nextCursor;
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [api, spaceId]);

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
      bootstrap,
      error,
      loading,
      reload,
      collaboration: collaborationTransport(api, spaceId),
      administration: administrationTransport(api, spaceId),
    }),
    [api, spaceId, name, bootstrap, error, loading, reload],
  );
  return <ProjectContext.Provider value={value}>{children}</ProjectContext.Provider>;
}
