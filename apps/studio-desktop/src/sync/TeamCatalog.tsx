import { useCallback, useEffect, useRef, useState } from "react";
import { FolderGit2, Search } from "lucide-react";
import { Badge, Banner, Button, EmptyState, Loading, TextField, formatRelativeTime, type SpaceCatalogEntry, type SpaceCatalogPage } from "@layrs/ui";
import { explainError } from "../app/errors";
import { useRemote } from "../app/remote";
import { useDesktopContexts } from "../app/contexts";

const roleLabels: Record<string, string> = { owner: "Propriétaire", administrator: "Administration", member: "Membre", auditor: "Lecture" };

/**
 * Team projects the connected account may open, by name (server catalog).
 * Homonyms are told apart by their organization; the reference stays a detail.
 */
export function TeamCatalog({
  selected,
  onSelect,
  actionLabel,
  preselect,
}: {
  selected?: string;
  onSelect: (entry: SpaceCatalogEntry) => void;
  actionLabel?: string;
  /** Project to select once it appears in the loaded pages (link from another page). */
  preselect?: string | null;
}) {
  const remote = useRemote();
  const contexts = useDesktopContexts();
  const [entries, setEntries] = useState<SpaceCatalogEntry[]>([]);
  const [loadedContextId, setLoadedContextId] = useState("");
  const [cursor, setCursor] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [filter, setFilter] = useState("");
  const generation = useRef(0);
  const connected = remote.session.state === "connected";
  const contextId = remote.session.state === "connected" && remote.origin === contexts.origin && remote.session.principalId === contexts.principalId ? contexts.selected?.id : undefined;

  const load = useCallback(
    async (next: string | null) => {
      if (!contextId) return;
      const version = ++generation.current;
      setLoading(true);
      setError("");
      try {
        const page = await remote.transport<SpaceCatalogPage>({ action: "listSpaces", limit: 50, contextId, ...(next ? { cursor: next } : {}) }, { root: remote.root, origin: remote.origin, principalId: contexts.principalId });
        if (version !== generation.current) return;
        setLoadedContextId(contextId);
        setEntries((current) => {
          const known = next ? current : [];
          const seen = new Set(known.map((entry) => entry.spaceId));
          return [...known, ...page.spaces.filter((entry) => !seen.has(entry.spaceId))];
        });
        setCursor(page.nextCursor);
        setHasMore(page.hasMore && Boolean(page.nextCursor));
      } catch (cause) {
        if (version === generation.current) setError(explainError(cause, "remote").detail);
      } finally {
        if (version === generation.current) setLoading(false);
      }
    },
    [remote.transport, contextId],
  );

  // A new session (or account) starts a new listing; old pages are never mixed in.
  useEffect(() => {
    generation.current++;
    setEntries([]);
    setLoadedContextId("");
    setCursor(null);
    setHasMore(false);
    if (contextId) void load(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connected, remote.origin, remote.epoch, contextId]);

  useEffect(() => {
    if (!preselect || selected) return;
    const entry = loadedContextId === contextId ? entries.find((item) => item.spaceId === preselect) : undefined;
    if (entry) onSelect(entry);
  }, [entries, loadedContextId, contextId, preselect, selected, onSelect]);

  if (!connected) return null;
  if (!contextId) return contexts.error
    ? <Banner tone="danger" title="Espaces indisponibles" actions={<Button size="sm" onClick={() => void contexts.refresh()}>Réessayer</Button>}>{contexts.error}</Banner>
    : contexts.data?.contexts.length === 0 && !contexts.loading && remote.origin === contexts.origin && remote.session.state === "connected" && remote.session.principalId === contexts.principalId
      ? <EmptyState compact icon={<FolderGit2 size={22} />} title="Aucun espace accessible sur ce serveur">Demandez accès à une organisation ou activez les projets personnels.</EmptyState>
    : <Loading label="Chargement de l’espace…" />;
  const visibleEntries = loadedContextId === contextId ? entries : [];
  const query = filter.trim().toLocaleLowerCase("fr");
  const shown = query ? visibleEntries.filter((entry) => `${entry.name} ${entry.workspace.name}`.toLocaleLowerCase("fr").includes(query)) : visibleEntries;
  const names = new Map<string, number>();
  for (const entry of visibleEntries) names.set(entry.name, (names.get(entry.name) ?? 0) + 1);

  return (
    <div className="catalog">
      <TextField
        label="Rechercher un projet"
        hideLabel
        value={filter}
        placeholder="Rechercher parmi les projets chargés"
        trailing={<Search aria-hidden size={16} className="catalog__search-icon" />}
        onChange={(event) => setFilter(event.target.value)}
      />
      {error && (
        <Banner tone="danger" title="La liste des projets n’a pas pu être lue." actions={<Button size="sm" onClick={() => void load(null)}>Réessayer</Button>}>
          <code className="sync-detail">{error}</code>
        </Banner>
      )}
      {loading && !visibleEntries.length ? (
        <Loading label="Lecture des projets de l’équipe…" />
      ) : shown.length ? (
        <ul className="catalog__list" aria-label="Projets de l’équipe">
          {shown.map((entry) => (
            <li key={entry.spaceId}>
              <button
                type="button"
                className="catalog__item"
                aria-pressed={selected === entry.spaceId}
                onClick={() => onSelect(entry)}
              >
                <span className="project-list__mark" aria-hidden>
                  {entry.name.slice(0, 1).toUpperCase() || "?"}
                </span>
                <span className="project-list__text">
                  <span className="project-list__name">{entry.name}</span>
                  <span className="project-list__path">
                    {entry.workspace.name}
                    {(names.get(entry.name) ?? 0) > 1 && ` · ${entry.slug}`} · {roleLabels[entry.workspaceRole] ?? entry.workspaceRole}
                    {entry.lastActivityAt && ` · activité ${formatRelativeTime(entry.lastActivityAt)}`}
                  </span>
                </span>
                <Badge tone={entry.access === "limited" ? "warning" : "info"}>{entry.access === "limited" ? "Accès limité" : "Accès complet"}</Badge>
                {actionLabel && <span className="catalog__action">{actionLabel}</span>}
              </button>
            </li>
          ))}
        </ul>
      ) : (
        !loading &&
        !error && (
          <EmptyState compact icon={<FolderGit2 size={22} />} title={visibleEntries.length ? "Aucun projet ne correspond" : "Aucun projet accessible sur ce serveur"}>
            {visibleEntries.length ? "Modifiez la recherche, ou chargez la suite de la liste." : "Demandez une invitation à la personne responsable du projet."}
          </EmptyState>
        )
      )}
      {loadedContextId === contextId && hasMore && (
        <Button size="sm" busy={loading} busyLabel="Lecture…" onClick={() => void load(cursor)}>
          Afficher plus de projets
        </Button>
      )}
    </div>
  );
}
