import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { FolderTree, Search } from "lucide-react";
import {
  Badge,
  Banner,
  Button,
  Card,
  EmptyState,
  Loading,
  PageHeader,
  TextField,
  formatRelativeTime,
  navigate,
  segment,
  type SpaceCatalogEntry,
  type SpaceCatalogPage,
} from "@layrs/ui";
import { rememberProject } from "../app/project";
import { useProjectContexts } from "../app/contexts";
import { describeError, useSession } from "../app/session";

export function openProject(spaceId: string) {
  navigate(`/projects/${segment(spaceId)}/files`);
}

const roleLabels: Record<string, string> = {
  owner: "Propriétaire",
  administrator: "Administrateur",
  member: "Membre",
  auditor: "Auditeur",
};

function normalize(value: string) {
  return value.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

/** Projects authorized within the selected server context. */
export function ProjectsPage() {
  const { api } = useSession();
  const { selected } = useProjectContexts();
  const [entries, setEntries] = useState<SpaceCatalogEntry[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [search, setSearch] = useState("");
  const [reference, setReference] = useState("");
  const generation = useRef(0);

  const load = useCallback(
    async (from: string | null) => {
      if (!selected) return;
      const requestGeneration = generation.current;
      setLoading(true);
      setError("");
      try {
        const query = new URLSearchParams({ limit: "50", contextId: selected.id });
        if (from) query.set("cursor", from);
        const page = await api<SpaceCatalogPage>(`/v3/spaces?${query}`);
        if (requestGeneration !== generation.current) return;
        page.spaces.forEach((space) => rememberProject(space.spaceId, space.name, selected.id));
        setEntries((current) => {
          const seen = new Set(current.map((entry) => entry.spaceId));
          return [...current, ...page.spaces.filter((space) => !seen.has(space.spaceId))];
        });
        setCursor(page.nextCursor);
        setHasMore(page.hasMore);
      } catch (cause) {
        if (requestGeneration === generation.current) setError(describeError(cause));
      } finally {
        if (requestGeneration === generation.current) setLoading(false);
      }
    },
    [api, selected?.id],
  );

  useEffect(() => {
    generation.current++;
    setEntries([]);
    setCursor(null);
    setHasMore(false);
    setSearch("");
    if (selected) void load(null);
    else setLoading(false);
  }, [load, selected?.id]);

  const filtered = useMemo(() => {
    const terms = normalize(search).split(/\s+/).filter(Boolean);
    if (!terms.length) return entries;
    return entries.filter((entry) => {
      const haystack = normalize(`${entry.name} ${entry.slug} ${entry.workspace.name}`);
      return terms.every((term) => haystack.includes(term));
    });
  }, [entries, search]);

  const groups = useMemo(() => {
    const map = new Map<string, { name: string; entries: SpaceCatalogEntry[] }>();
    for (const entry of filtered) {
      const group = map.get(entry.workspace.workspaceId) ?? { name: entry.workspace.name, entries: [] };
      group.entries.push(entry);
      map.set(entry.workspace.workspaceId, group);
    }
    return [...map.values()];
  }, [filtered]);

  function submitReference(event: FormEvent) {
    event.preventDefault();
    if (reference.trim()) openProject(reference.trim());
  }

  return (
    <div className="layrs-page">
      <PageHeader title="Vos projets" description={selected ? `Projets accessibles dans ${selected.kind === "personal" ? "votre espace personnel" : selected.name}.` : "Votre compte n’a accès à aucune organisation active. Demandez un accès à un administrateur."} />
      <div className="projects-actions">
        {selected?.canCreateProject && <Button onClick={() => navigate("/projects/new")}>Nouveau projet</Button>}
      </div>
      {error && (
        <Banner
          tone="danger"
          title="La liste des projets n’a pas pu être chargée"
          actions={
            <Button size="sm" onClick={() => void load(cursor)}>
              Réessayer
            </Button>
          }
        >
          {error}
        </Banner>
      )}
      {entries.length > 0 && (
        <TextField
          label="Rechercher un projet"
          hideLabel
          value={search}
          placeholder="Rechercher par nom ou identifiant…"
          hint={hasMore ? "La recherche porte sur les projets déjà chargés." : undefined}
          trailing={<Search aria-hidden size={16} className="projects-search__icon" />}
          onChange={(event) => setSearch(event.target.value)}
        />
      )}
      {loading && !entries.length ? (
        <Loading label="Chargement des projets…" />
      ) : !entries.length && !error ? (
        <EmptyState icon={<FolderTree size={22} />} title="Aucun projet accessible pour l’instant">
          Créez un projet ou demandez une invitation. Vos invitations en attente apparaissent dans la boîte de réception.
        </EmptyState>
      ) : (
        groups.map((group) => (
          <section key={group.name + group.entries[0].workspace.workspaceId} className="projects-section" aria-label={group.name}>
            <h2>{group.name}</h2>
            <Card>
              <ul className="project-list">
                {group.entries.map((entry) => (
                  <li key={entry.spaceId} className="project-list__item">
                    <button type="button" className="project-list__open" onClick={() => openProject(entry.spaceId)}>
                      <span className="project-list__mark" aria-hidden>
                        {entry.name.slice(0, 1).toUpperCase()}
                      </span>
                      <span className="project-list__text">
                        <span className="project-list__name">{entry.name}</span>
                        <span className="project-list__path">
                          {roleLabels[entry.workspaceRole] ?? entry.workspaceRole}
                          {entry.lastActivityAt && ` · activité ${formatRelativeTime(entry.lastActivityAt)}`}
                        </span>
                      </span>
                      {entry.access === "limited" && <Badge tone="warning">Accès limité</Badge>}
                    </button>
                  </li>
                ))}
              </ul>
            </Card>
          </section>
        ))
      )}
      {search && entries.length > 0 && !filtered.length && <p className="projects-note">Aucun projet chargé ne correspond à « {search} ».</p>}
      {hasMore && (
        <div>
          <Button busy={loading} busyLabel="Chargement…" onClick={() => void load(cursor)}>
            Afficher plus de projets
          </Button>
        </div>
      )}
      <details className="projects-advanced">
        <summary>Ouvrir un projet par sa référence technique</summary>
        <form className="settings-card open-by-reference" onSubmit={submitReference}>
          <TextField
            label="Référence du projet"
            value={reference}
            placeholder="space:…"
            hint="Pour un lien reçu hors de Layrs. L’accès reste vérifié par le serveur."
            onChange={(event) => setReference(event.target.value)}
          />
          <div>
            <Button type="submit" disabledReason={!reference.trim() ? "Saisissez une référence." : undefined}>
              Ouvrir le projet
            </Button>
          </div>
        </form>
      </details>
    </div>
  );
}
