import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { OrganisationSelector, navigate, type ProjectContextEntry, type ProjectContexts } from "@layrs/ui";
import { explainError } from "./errors";
import { useRemote } from "./remote";

type ContextState = {
  data: ProjectContexts | null;
  selected: ProjectContextEntry | null;
  loading: boolean;
  error: string;
  origin: string;
  principalId: string;
  select: (id: string) => void;
  adopt: (id: string) => void;
  openAllProjects: () => void;
  refresh: (preferredId?: string) => Promise<void>;
};
const Contexts = createContext<ContextState | null>(null);
const STORAGE_PREFIX = "layrs.desktop.context.v1:";
const ORGANISATION_PREFIX = "layrs.desktop.organisation.v1:";

function storageKey(origin: string, principalId: string) { return `${STORAGE_PREFIX}${origin}:${principalId}`; }
function remembered(origin: string, principalId: string) {
  try { return localStorage.getItem(storageKey(origin, principalId)) ?? ""; } catch { return ""; }
}
function organisationKey(origin: string, principalId: string) { return `${ORGANISATION_PREFIX}${origin}:${principalId}`; }
function rememberedOrganisation(origin: string, principalId: string) {
  try { return localStorage.getItem(organisationKey(origin, principalId)) ?? ""; } catch { return ""; }
}
function rememberOrganisation(origin: string, principalId: string, id: string) {
  try { localStorage.setItem(organisationKey(origin, principalId), id); } catch { /* optional */ }
}

export function useDesktopContexts() {
  const value = useContext(Contexts);
  if (!value) throw new Error("useDesktopContexts requires DesktopContextsProvider");
  return value;
}

/** Context preferences contain only a workspace ID and are scoped to server and account. */
export function DesktopContextsProvider({ children }: { children: ReactNode }) {
  const remote = useRemote();
  const principalId = remote.session.state === "connected" ? remote.session.principalId : "";
  const identity = `${remote.origin}|${principalId}`;
  const generation = useRef(0);
  const requestSequence = useRef(0);
  const [data, setData] = useState<ProjectContexts | null>(null);
  const [selectedId, setSelectedId] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const refresh = useCallback(async (preferredId?: string) => {
    if (!principalId) return;
    const version = generation.current;
    const request = ++requestSequence.current;
    setLoading(true);
    setError("");
    try {
      const next = await remote.transport<ProjectContexts>({ action: "listContexts" }, { root: remote.root, origin: remote.origin, principalId });
      if (version !== generation.current || request !== requestSequence.current) return;
      setData(next);
      if (preferredId && next.contexts.some((item) => item.id === preferredId)) {
        try { localStorage.setItem(storageKey(remote.origin, principalId), preferredId); } catch { /* optional */ }
        if (next.contexts.find((item) => item.id === preferredId)?.kind !== "personal") rememberOrganisation(remote.origin, principalId, preferredId);
      }
      setSelectedId((current) => {
        const candidate = preferredId ?? (current || remembered(remote.origin, principalId));
        return next.contexts.some((item) => item.id === candidate) ? candidate : next.defaultContextId;
      });
    } catch (cause) {
      if (version === generation.current && request === requestSequence.current) setError(explainError(cause, "remote").detail);
    } finally {
      if (version === generation.current && request === requestSequence.current) setLoading(false);
    }
  }, [remote.transport, remote.origin, principalId]);
  useEffect(() => {
    generation.current++;
    requestSequence.current++;
    setData(null);
    setSelectedId("");
    setError("");
    if (principalId) void refresh(remembered(remote.origin, principalId));
  }, [identity, remote.epoch, refresh]);
  useEffect(() => () => { generation.current++; requestSequence.current++; }, []);
  useEffect(() => {
    if (!principalId) return;
    const reload = () => { void refresh(); };
    window.addEventListener("layrs-organisations-changed", reload);
    window.addEventListener("focus", reload);
    return () => {
      window.removeEventListener("layrs-organisations-changed", reload);
      window.removeEventListener("focus", reload);
    };
  }, [principalId, refresh]);
  const select = useCallback((id: string) => {
    const context = data?.contexts.find((item) => item.id === id);
    if (!context || !principalId) return;
    setSelectedId(id);
    try { localStorage.setItem(storageKey(remote.origin, principalId), id); } catch { /* optional */ }
    if (context.kind !== "personal") rememberOrganisation(remote.origin, principalId, id);
    navigate("/projects");
  }, [data, remote.origin, principalId]);
  const adopt = useCallback((id: string) => {
    const context = data?.contexts.find((item) => item.id === id);
    if (!context || !principalId) return;
    setSelectedId(id);
    try { localStorage.setItem(storageKey(remote.origin, principalId), id); } catch { /* optional */ }
    if (context.kind !== "personal") rememberOrganisation(remote.origin, principalId, id);
  }, [data, remote.origin, principalId]);
  const selected = data?.contexts.find((item) => item.id === selectedId) ?? null;
  const openAllProjects = useCallback(() => {
    if (selected?.kind === "personal") {
      const shared = data?.contexts.filter((item) => item.kind !== "personal") ?? [];
      const preferred = rememberedOrganisation(remote.origin, principalId);
      const target = shared.find((item) => item.id === preferred)
        ?? shared.find((item) => item.id === data?.defaultContextId)
        ?? shared[0];
      if (target) { select(target.id); return; }
    }
    navigate("/projects");
  }, [data, selected, remote.origin, principalId, select]);
  const value = useMemo(() => ({ data, selected, loading, error, origin: remote.origin, principalId, select, adopt, openAllProjects, refresh }), [data, selected, loading, error, remote.origin, principalId, select, adopt, openAllProjects, refresh]);
  return <Contexts.Provider value={value}>{children}</Contexts.Provider>;
}

export function DesktopContextSelector() {
  const { data, selected, select } = useDesktopContexts();
  if (!data?.multiOrganisationEnabled) return null;
  return <OrganisationSelector
    contexts={data.contexts}
    selectedId={selected?.id ?? null}
    canCreateOrganisation={data.canCreateOrganisation}
    onSelect={select}
    onCreateOrganisation={() => navigate("/organisations/new")}
  />;
}
