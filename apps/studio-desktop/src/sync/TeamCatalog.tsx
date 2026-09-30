import { useCallback, useEffect, useRef, useState } from "react";
import { FolderGit2, Search } from "lucide-react";
import { Badge, Banner, Button, EmptyState, Loading, TextField, formatRelativeTime, type SpaceCatalogEntry, type SpaceCatalogPage } from "@layrs/ui";
import { explainError } from "../app/errors";
import { useRemote } from "../app/remote";

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
  const [entries, setEntries] = useState<SpaceCatalogEntry[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [filter, setFilter] = useState("");
  const generation = useRef(0);
  const connected = remote.session.state === "connected";

  const load = useCallback(
    async (next: string | null) => {
      const version = ++generation.current;
      setLoading(true);
      setError("");
      try {
        const page = await remote.transport<SpaceCatalogPage>({ action: "listSpaces", limit: 50, ...(next ? { cursor: next } : {}) });
        if (version !== generation.current) return;
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
    [remote],
  );

  // A new session (or account) starts a new listing; old pages are never mixed in.
  useEffect(() => {
    generation.current++;
    setEntries([]);
    setCursor(null);
    setHasMore(false);
    if (connected) void load(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connected, remote.origin, remote.epoch]);

  useEffect(() => {
    if (!preselect || selected) return;
    const entry = entries.find((item) => item.spaceId === preselect);
    if (entry) onSelect(entry);
  }, [entries, preselect, selected, onSelect]);

  if (!connected) return null;
  const query = filter.trim().toLocaleLowerCase("fr");
  const shown = query ? entries.filter((entry) => `${entry.name} ${entry.workspace.name}`.toLocaleLowerCase("fr").includes(query)) : entries;
  const names = new Map<string, number>();
  for (const entry of entries) names.set(entry.name, (names.get(entry.name) ?? 0) + 1);

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
      {loading && !entries.length ? (
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
          <EmptyState compact icon={<FolderGit2 size={22} />} title={entries.length ? "Aucun projet ne correspond" : "Aucun projet accessible sur ce serveur"}>
            {entries.length ? "Modifiez la recherche, ou chargez la suite de la liste." : "Demandez une invitation à la personne responsable du projet."}
          </EmptyState>
        )
      )}
      {hasMore && (
        <Button size="sm" busy={loading} busyLabel="Lecture…" onClick={() => void load(cursor)}>
          Afficher plus de projets
        </Button>
      )}
    </div>
  );
}
