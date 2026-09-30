import { useEffect, useMemo, useState } from "react";
import { FileMinus, FilePen, FilePlus, FileText } from "lucide-react";
import { DiffView, type DiffMode } from "../components/DiffView";
import { diffStats } from "../components/diff-model";
import { Banner, EmptyState } from "../components/Feedback";
import { SegmentedControl } from "../components/SegmentedControl";
import { formatBytes } from "../foundation/format";
import { changeStatusLabel, diffLines, explainCollabError, fileName, parentFolder } from "./model";
import { CollabError, DelayedRows } from "./parts";
import type { CollabTransport, CompareChange } from "./types";
import { useScopedTasks } from "./useScopedTasks";

/**
 * Files and text comparison between two published captures. Content the
 * account may not read is never summarised: an access refusal replaces the
 * whole comparison, without sizes or partial previews.
 */
export function ComparePanel({
  scope,
  transport,
  before,
  after,
  beforeLabel = "Avant",
  afterLabel = "Après",
}: {
  scope: string;
  transport: CollabTransport;
  before: string;
  after: string;
  beforeLabel?: string;
  afterLabel?: string;
}) {
  const key = `${scope}|${before}|${after}`;
  const tasks = useScopedTasks(key);
  const [changes, setChanges] = useState<CompareChange[] | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [selected, setSelected] = useState("");
  const [mode, setMode] = useState<DiffMode>(() => (typeof window !== "undefined" && window.innerWidth < 1100 ? "inline" : "split"));

  function load() {
    return tasks.run(
      "compare",
      async (alive) => {
        setError(null);
        setChanges(null);
        const result = await transport<{ changes: CompareChange[] }>({ action: "compare", before, after });
        if (!alive()) return;
        const list = Array.isArray(result?.changes) ? result.changes : [];
        setChanges(list);
        setSelected((current) => (list.some((change) => change.path === current) ? current : (list[0]?.path ?? "")));
      },
      setError,
    );
  }

  useEffect(() => {
    setChanges(null);
    setError(null);
    if (before && after) void load();
  }, [key]);

  const change = changes?.find((item) => item.path === selected) ?? null;
  const lines = useMemo(() => (change ? diffLines(change) : []), [change]);

  if (!before || !after) return <EmptyState compact title="Rien à comparer">Choisissez deux captures.</EmptyState>;
  if (error) {
    const info = explainCollabError(error);
    return info.forbidden ? (
      <Banner tone="warning" title="Contenu non accessible">
        La comparaison de ces captures n’est pas ouverte à votre compte. Aucun nom, taille ni extrait n’est affiché.
      </Banner>
    ) : (
      <CollabError error={error} title="Comparaison indisponible" onRetry={() => void load()} />
    );
  }
  if (!changes) return <DelayedRows active label="Comparaison en cours…" />;
  if (!changes.length) return <EmptyState compact icon={<FileText size={20} />} title="Aucune différence">Les fichiers des deux captures sont identiques.</EmptyState>;

  return (
    <div className="collab-compare">
      <div className="collab-compare__files">
        <p className="collab-compare__count">
          {changes.length} fichier{changes.length > 1 ? "s" : ""} modifié{changes.length > 1 ? "s" : ""}
        </p>
        <ul className="collab-rows" aria-label="Fichiers comparés">
          {changes.map((item) => {
            const stats = item.textDiff ? diffStats(diffLines(item)) : null;
            return (
              <li key={item.path}>
                <button type="button" className="collab-row" aria-current={item.path === selected ? "true" : undefined} onClick={() => setSelected(item.path)}>
                  <span className={`collab-row__icon collab-status--${item.status}`} aria-hidden>
                    {item.status === "added" ? <FilePlus size={18} /> : item.status === "deleted" ? <FileMinus size={18} /> : <FilePen size={18} />}
                  </span>
                  <span className="collab-row__text">
                    <span className="collab-row__title">{fileName(item.path)}</span>
                    <span className="collab-row__meta">
                      {changeStatusLabel(item.status)}
                      {parentFolder(item.path) ? ` · ${parentFolder(item.path)}` : ""}
                    </span>
                  </span>
                  {stats && (
                    <span className="collab-stats" aria-label={`${stats.added} lignes ajoutées, ${stats.removed} lignes supprimées`}>
                      <span className="collab-added">+{stats.added}</span>
                      <span className="collab-removed">−{stats.removed}</span>
                    </span>
                  )}
                </button>
              </li>
            );
          })}
        </ul>
      </div>
      <div className="collab-compare__diff" aria-label="Comparaison du fichier sélectionné" role="region">
        {change && (
          <>
            <div className="collab-compare__head">
              <span className="collab-compare__path">{change.path}</span>
              <span className="collab-muted">
                {formatBytes(change.beforeBytes)} → {formatBytes(change.afterBytes)}
              </span>
              <SegmentedControl
                size="sm"
                label="Présentation de la comparaison"
                value={mode}
                onChange={setMode}
                options={[
                  { value: "split", label: "Côte à côte" },
                  { value: "inline", label: "En ligne" },
                ]}
              />
            </div>
            {change.binary ? (
              <EmptyState compact title="Contenu binaire">Pas d’aperçu texte pour ce format.</EmptyState>
            ) : change.textOmitted || !change.textDiff ? (
              <EmptyState compact title="Aperçu texte indisponible">Contenu trop volumineux ou non textuel pour cette comparaison.</EmptyState>
            ) : (
              <DiffView lines={lines} mode={mode} beforeLabel={beforeLabel} afterLabel={afterLabel} />
            )}
          </>
        )}
      </div>
    </div>
  );
}
