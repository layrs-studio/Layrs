import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Banner, Button, Loading, OrganisationSelector, navigate, type ProjectContextEntry, type ProjectContexts } from "@layrs/ui";
import { describeError, useSession } from "./session";

type ContextState = {
  data: ProjectContexts | null;
  selected: ProjectContextEntry | null;
  loading: boolean;
  error: string;
  select: (id: string) => void;
  adopt: (id: string) => void;
  openAllProjects: () => void;
  refresh: (preferredId?: string) => Promise<void>;
};

const Contexts = createContext<ContextState | null>(null);
const STORAGE_PREFIX = "layrs.web.context.v1:";
const ORGANISATION_PREFIX = "layrs.web.organisation.v1:";

function storageKey(origin: string, principalId: string) { return `${STORAGE_PREFIX}${origin}:${principalId}`; }
function remembered(origin: string, principalId: string) {
  try { return localStorage.getItem(storageKey(origin, principalId)) ?? ""; } catch { return ""; }
}
function remember(origin: string, principalId: string, id: string) {
  try { localStorage.setItem(storageKey(origin, principalId), id); } catch { /* Optional preference. */ }
}
function organisationKey(origin: string, principalId: string) { return `${ORGANISATION_PREFIX}${origin}:${principalId}`; }
function rememberedOrganisation(origin: string, principalId: string) {
  try { return localStorage.getItem(organisationKey(origin, principalId)) ?? ""; } catch { return ""; }
}
function rememberOrganisation(origin: string, principalId: string, id: string) {
  try { localStorage.setItem(organisationKey(origin, principalId), id); } catch { /* Optional preference. */ }
}

export function useProjectContexts() {
  const value = useContext(Contexts);
  if (!value) throw new Error("useProjectContexts requires ProjectContextsProvider");
  return value;
}

export function ProjectContextsProvider({ children }: { children: ReactNode }) {
  const { api, principal } = useSession();
  const principalId = principal?.principalId ?? "";
  const origin = location.origin;
  const [data, setData] = useState<ProjectContexts | null>(null);
  const [selectedId, setSelectedId] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const requestSequence = useRef(0);
  const refresh = useCallback(async (preferredId?: string) => {
    const request = ++requestSequence.current;
    setLoading(true);
    setError("");
    try {
      const next = await api<ProjectContexts>("/v3/contexts");
      if (request !== requestSequence.current) return;
      setData(next);
      const preferred = next.contexts.find((item) => item.id === preferredId);
      if (preferred) {
        remember(origin, principalId, preferred.id);
        if (preferred.kind !== "personal") rememberOrganisation(origin, principalId, preferred.id);
      }
      setSelectedId((current) => {
        const candidate = preferredId ?? (current || remembered(origin, principalId));
        return next.contexts.some((item) => item.id === candidate) ? candidate : next.defaultContextId;
      });
    } catch (cause) {
      if (request === requestSequence.current) setError(describeError(cause));
    } finally {
      if (request === requestSequence.current) setLoading(false);
    }
  }, [api, origin, principalId]);
  useEffect(() => () => { requestSequence.current++; }, []);
  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => {
    const reload = () => { void refresh(); };
    window.addEventListener("layrs-organisations-changed", reload);
    window.addEventListener("focus", reload);
    return () => {
      window.removeEventListener("layrs-organisations-changed", reload);
      window.removeEventListener("focus", reload);
    };
  }, [refresh]);
  const select = useCallback((id: string) => {
    const context = data?.contexts.find((item) => item.id === id);
    if (!context) return;
    setSelectedId(id);
    remember(origin, principalId, id);
    if (context.kind !== "personal") rememberOrganisation(origin, principalId, id);
    navigate("/projects");
  }, [data, origin, principalId]);
  const adopt = useCallback((id: string) => {
    const context = data?.contexts.find((item) => item.id === id);
    if (context) {
      setSelectedId(id);
      remember(origin, principalId, id);
      if (context.kind !== "personal") rememberOrganisation(origin, principalId, id);
    }
  }, [data, origin, principalId]);
  const selected = data?.contexts.find((item) => item.id === selectedId) ?? null;
  const openAllProjects = useCallback(() => {
    if (selected?.kind === "personal") {
      const shared = data?.contexts.filter((item) => item.kind !== "personal") ?? [];
      const preferred = rememberedOrganisation(origin, principalId);
      const target = shared.find((item) => item.id === preferred)
        ?? shared.find((item) => item.id === data?.defaultContextId)
        ?? shared[0];
      if (target) { select(target.id); return; }
    }
    navigate("/projects");
  }, [data, selected, origin, principalId, select]);
  const value = useMemo(() => ({ data, selected, loading, error, select, adopt, openAllProjects, refresh }), [data, selected, loading, error, select, adopt, openAllProjects, refresh]);
  return <Contexts.Provider value={value}>
    {error && !data ? <div className="layrs-page"><Banner tone="danger" title="Contextes indisponibles" actions={<Button onClick={() => void refresh()}>Réessayer</Button>}>{error}</Banner></div>
      : loading && !data ? <div className="layrs-page"><Loading label="Chargement des espaces…" /></div> : children}
  </Contexts.Provider>;
}

export function ContextSelector() {
  const { data, selected, select } = useProjectContexts();
  if (!data?.multiOrganisationEnabled) return null;
  return <OrganisationSelector
    contexts={data.contexts}
    selectedId={selected?.id ?? null}
    canCreateOrganisation={data.canCreateOrganisation}
    onSelect={select}
    onCreateOrganisation={() => navigate("/organisations/new")}
  />;
}
