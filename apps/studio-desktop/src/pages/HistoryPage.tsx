import { useCallback, useEffect, useRef, useState } from "react";
import { Filter, Redo2, RotateCcw, Undo2 } from "lucide-react";
import {
  Badge,
  Banner,
  Button,
  Card,
  EmptyState,
  Loading,
  PageHeader,
  SelectField,
  Tabs,
  TechnicalId,
  TextField,
  formatDateTime,
  formatRelativeTime,
  useLocation,
} from "@layrs/ui";
import { useWorkspace } from "../app/workspace";
import { shortStep } from "../merge/model";
import { nativeWorkspace } from "../tauri";
import { LimitedHistory } from "./limited/LimitedHistory";

type Tab = "captures" | "operations";
type Step = { stepId: string; parentStepIds: string[] };
type Observation = { observationId: string; observedAt: string; actorId: string | null; captureReason: string };
type OperationRow = { operationId: string; summary: string; createdAt: string };
type Page = { anchor: string; operations?: OperationRow[]; steps?: Step[]; observations?: Record<string, Observation[]>; next: unknown | null };
type Query = { action: string; [key: string]: unknown };

/** Converts a local calendar day to an inclusive RFC 3339 bound. */
function dayBound(day: string, end: boolean) {
  if (!day) return null;
  const date = new Date(`${day}T${end ? "23:59:59.999" : "00:00:00"}`);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}

export function HistoryPage() {
  const { kind } = useWorkspace();
  return kind === "projection" ? <LimitedHistory /> : <CompleteHistory />;
}

/** P07/P08 on Desktop: captures and local operations, restore through a preview. */
function CompleteHistory() {
  const workspace = useWorkspace();
  const { data, busy } = workspace;
  const { query } = useLocation();
  const [tab, setTab] = useState<Tab>("captures");
  const locked = Boolean(busy) || Boolean(data?.needsRepair);
  return (
    <div className="layrs-page">
      <PageHeader
        title="Historique"
        description="Ce qui a été capturé et ce qui s’est passé sur cet ordinateur. Rien n’est modifié avant un aperçu confirmé."
        aside={
          <>
            <Button
              icon={<Undo2 aria-hidden size={16} />}
              disabledReason={locked ? "Opération en cours" : (data?.operations.length ?? 0) < 2 ? "Rien à annuler." : undefined}
              onClick={() => void workspace.run({ action: "undo" }, "Annulation…")}
            >
              Annuler
            </Button>
            <Button
              icon={<Redo2 aria-hidden size={16} />}
              disabledReason={locked ? "Opération en cours" : data?.state.operation.kind !== "undo" ? "Rien à rétablir." : undefined}
              onClick={() => void workspace.run({ action: "redo", operation: null }, "Rétablissement…")}
            >
              Rétablir
            </Button>
          </>
        }
      />
      <Tabs<Tab>
        label="Vues de l’historique"
        value={tab}
        onChange={setTab}
        items={[
          { id: "captures", label: "Captures" },
          { id: "operations", label: "Opérations locales" },
        ]}
      >
        {tab === "captures" ? <CaptureHistory initialPath={query.get("path") ?? ""} locked={locked} /> : <OperationHistory locked={locked} />}
      </Tabs>
    </div>
  );
}

function usePagedHistory(root: string) {
  const [page, setPage] = useState<Page | null>(null);
  const [steps, setSteps] = useState<Step[]>([]);
  const [observations, setObservations] = useState<Record<string, Observation[]>>({});
  const [operations, setOperations] = useState<OperationRow[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const generation = useRef(0);
  const last = useRef<Query | null>(null);

  const load = useCallback(
    async (request: Query | null, more = false) => {
      const selected = more ? last.current : request;
      if (!selected) return;
      const version = ++generation.current;
      setLoading(true);
      setError("");
      try {
        const result = await nativeWorkspace<Page>(root, { ...selected, cursor: more ? page?.next ?? null : null });
        if (version !== generation.current) return;
        last.current = selected;
        setPage(result);
        setSteps((current) => (more ? [...current, ...(result.steps ?? [])] : result.steps ?? []));
        setObservations((current) => (more ? { ...current, ...(result.observations ?? {}) } : result.observations ?? {}));
        setOperations((current) => (more ? [...current, ...(result.operations ?? [])] : result.operations ?? []));
      } catch (cause) {
        if (version === generation.current) {
          setError(String(cause));
          setPage(null);
        }
      } finally {
        if (version === generation.current) setLoading(false);
      }
    },
    [root, page],
  );
  return { page, steps, observations, operations, loading, error, load };
}

function CaptureHistory({ initialPath, locked }: { initialPath: string; locked: boolean }) {
  const workspace = useWorkspace();
  const layers = workspace.data?.state.layers ?? [];
  const [layer, setLayer] = useState(workspace.activeLayer?.name ?? layers[0]?.name ?? "");
  const [reference, setReference] = useState("");
  const [path, setPath] = useState(initialPath);
  const [actor, setActor] = useState("");
  const [since, setSince] = useState("");
  const [until, setUntil] = useState("");
  const [me, setMe] = useState<{ actorId: string; displayName: string | null } | null>(null);
  const history = usePagedHistory(workspace.root);
  const selector = reference.trim() || (layer ? `@layer:${layer}` : "");

  const request = useCallback(
    (): Query => ({
      action: "stepHistory",
      selector,
      path: path.trim() || null,
      filter: { actor: actor.trim() || null, since: dayBound(since, false), until: dayBound(until, true) },
      limit: 30,
    }),
    [selector, path, actor, since, until],
  );

  // Initial page, and a new anchor when the Layer changes.
  const operation = workspace.data?.state.operation.operationId;
  useEffect(() => {
    if (selector) void history.load(request());
    // Filters apply on demand; the Layer and new operations reload.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [layer, operation]);

  useEffect(() => {
    let active = true;
    nativeWorkspace<{ identity: { actorId: string; displayName: string | null } | null }>(workspace.root, { action: "localIdentity" })
      .then((result) => active && setMe(result.identity))
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, [workspace.root]);

  function restore(stepId: string) {
    const target = path.trim() || null;
    void workspace.inspect(
      { action: "previewRestore", step: stepId, path: target },
      { action: "restore", step: stepId, path: target },
      target ? `Restaurer ${target}` : "Restaurer le projet",
    );
  }

  const automatic = (step: Step) => history.observations[step.stepId]?.some((item) => item.captureReason === "automatic_idle") === true;
  const groups: Step[][] = [];
  for (const step of history.steps) {
    const previous = groups.at(-1);
    if (automatic(step) && previous && automatic(previous[0])) previous.push(step);
    else groups.push([step]);
  }
  const actorName = (id: string | null) => (!id ? "Acteur non renseigné" : id === me?.actorId ? `Vous${me.displayName ? ` (${me.displayName})` : ""}` : id);

  const row = (step: Step) => {
    const seen = history.observations[step.stepId] ?? [];
    const first = seen[0];
    return (
      <li key={step.stepId} className="history-row">
        <div className="history-row__text">
          <p className="history-row__title">
            Capture {shortStep(step.stepId)}
            {automatic(step) && <Badge tone="info">Automatique</Badge>}
            {step.parentStepIds.length > 1 && <Badge tone="accent">Fusion de {step.parentStepIds.length} captures</Badge>}
          </p>
          {first ? (
            <p className="history-row__meta">
              <time dateTime={first.observedAt} title={formatDateTime(first.observedAt)}>
                {formatRelativeTime(first.observedAt)}
              </time>{" "}
              · {actorName(first.actorId)}
              {seen.length > 1 && ` · ${seen.length} observations`}
            </p>
          ) : (
            <p className="history-row__meta">Aucune observation locale</p>
          )}
          <details className="merge__technical">
            <summary>Identifiants</summary>
            <TechnicalId value={step.stepId} label="Copier la capture" />
            {seen.map((item) => item.actorId && <code key={item.observationId}>{item.actorId}</code>)}
          </details>
        </div>
        <Button size="sm" icon={<RotateCcw aria-hidden size={14} />} disabledReason={locked ? "Opération en cours" : undefined} onClick={() => restore(step.stepId)}>
          {path.trim() ? "Restaurer ce chemin…" : "Restaurer…"}
        </Button>
      </li>
    );
  };

  return (
    <div className="history">
      <form
        className="history-filters"
        aria-label="Filtres des captures"
        onSubmit={(event) => {
          event.preventDefault();
          void history.load(request());
        }}
      >
        <SelectField label="Layer" value={layer} disabled={Boolean(reference.trim())} onChange={(event) => setLayer(event.target.value)}>
          {layers.map((item) => (
            <option key={item.layerId} value={item.name}>
              {item.name}
            </option>
          ))}
        </SelectField>
        <TextField label="Chemin exact" value={path} placeholder="Tout le projet" onChange={(event) => setPath(event.target.value)} />
        <details className="history-filters__more">
          <summary>
            <Filter aria-hidden size={14} /> Plus de filtres
          </summary>
          <div className="history-filters__grid">
            <TextField label="Acteur exact" value={actor} placeholder="actor:…" onChange={(event) => setActor(event.target.value)} />
            {me && (
              <Button size="sm" variant="ghost" onClick={() => setActor(me.actorId)}>
                Mes captures
              </Button>
            )}
            <TextField label="Depuis le" type="date" value={since} onChange={(event) => setSince(event.target.value)} />
            <TextField label="Jusqu’au" type="date" value={until} onChange={(event) => setUntil(event.target.value)} />
            <TextField
              label="Référence exacte"
              value={reference}
              placeholder="@marker:nom ou blake3:…"
              hint="Remplace le Layer : repère, version ou capture."
              onChange={(event) => setReference(event.target.value)}
            />
          </div>
        </details>
        <Button type="submit" variant="primary" disabledReason={!selector ? "Choisissez un Layer ou une référence." : undefined}>
          Afficher
        </Button>
      </form>
      {history.error && (
        <Banner tone="danger" title="L’historique n’a pas pu être lu.">
          <code className="sync-detail">{history.error}</code>
        </Banner>
      )}
      {history.loading && !history.page ? (
        <Loading label="Lecture de l’historique…" />
      ) : history.steps.length ? (
        <Card>
          <div className="history-list-wrap" aria-label="Captures" role="region">
            {groups.map((group) =>
              group.length > 1 ? (
                <details key={group[0].stepId} className="history-group">
                  <summary>{group.length} captures automatiques successives</summary>
                  <ol className="history-rows">{group.map(row)}</ol>
                </details>
              ) : (
                <ol key={group[0].stepId} className="history-rows">
                  {row(group[0])}
                </ol>
              ),
            )}
          </div>
          <footer className="history-footer">
            <p>Dates et acteurs proviennent des observations de cet ordinateur ; ils ne prouvent pas l’auteur d’une capture. Les renommages ne sont pas déduits.</p>
            <Button size="sm" disabledReason={!history.page?.next ? "Fin de l’historique." : undefined} busy={history.loading} onClick={() => void history.load(null, true)}>
              Captures plus anciennes
            </Button>
          </footer>
        </Card>
      ) : (
        history.page && (
          <EmptyState title="Aucune capture ne correspond">Modifiez les filtres, ou capturez votre travail depuis Changements.</EmptyState>
        )
      )}
      <RestoreByReference locked={locked} />
    </div>
  );
}

function RestoreByReference({ locked }: { locked: boolean }) {
  const workspace = useWorkspace();
  const [step, setStep] = useState("");
  const [path, setPath] = useState("");
  return (
    <details className="versions-hidden">
      <summary>Restaurer depuis une référence exacte</summary>
      <div className="versions-hidden__body">
        <TextField label="Capture ou point nommé" value={step} placeholder="blake3:… ou @marker:nom" onChange={(event) => setStep(event.target.value)} />
        <TextField label="Chemin facultatif" value={path} placeholder="Vide pour le projet entier" onChange={(event) => setPath(event.target.value)} />
        <div>
          <Button
            disabledReason={locked ? "Opération en cours" : !step.trim() ? "Indiquez la capture à restaurer." : undefined}
            onClick={() =>
              void workspace.inspect(
                { action: "previewRestore", step: step.trim(), path: path.trim() || null },
                { action: "restore", step: step.trim(), path: path.trim() || null },
                path.trim() ? `Restaurer ${path.trim()}` : "Restaurer le projet",
              )
            }
          >
            Prévisualiser
          </Button>
        </div>
      </div>
    </details>
  );
}

function OperationHistory({ locked }: { locked: boolean }) {
  const workspace = useWorkspace();
  const [search, setSearch] = useState("");
  const [actor, setActor] = useState("");
  const [since, setSince] = useState("");
  const [until, setUntil] = useState("");
  const history = usePagedHistory(workspace.root);
  const request = (): Query => ({
    action: "operationHistory",
    search: search.trim(),
    filter: { actor: actor.trim() || null, since: dayBound(since, false), until: dayBound(until, true) },
    limit: 30,
  });
  const operation = workspace.data?.state.operation.operationId;
  useEffect(() => {
    void history.load(request());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [operation]);

  return (
    <div className="history">
      <form
        className="history-filters"
        aria-label="Filtres des opérations"
        onSubmit={(event) => {
          event.preventDefault();
          void history.load(request());
        }}
      >
        <TextField label="Rechercher" value={search} placeholder="Résumé, identifiant ou acteur" onChange={(event) => setSearch(event.target.value)} />
        <details className="history-filters__more">
          <summary>
            <Filter aria-hidden size={14} /> Plus de filtres
          </summary>
          <div className="history-filters__grid">
            <TextField label="Acteur exact" value={actor} placeholder="actor:…" onChange={(event) => setActor(event.target.value)} />
            <TextField label="Depuis le" type="date" value={since} onChange={(event) => setSince(event.target.value)} />
            <TextField label="Jusqu’au" type="date" value={until} onChange={(event) => setUntil(event.target.value)} />
          </div>
        </details>
        <Button type="submit" variant="primary">
          Afficher
        </Button>
      </form>
      {history.error && (
        <Banner tone="danger" title="L’historique n’a pas pu être lu.">
          <code className="sync-detail">{history.error}</code>
        </Banner>
      )}
      {history.loading && !history.page ? (
        <Loading label="Lecture des opérations…" />
      ) : history.operations.length ? (
        <Card>
          <ol className="history-rows" aria-label="Opérations locales">
            {history.operations.map((item) => (
              <li key={item.operationId} className="history-row">
                <div className="history-row__text">
                  <p className="history-row__title">{item.summary}</p>
                  <p className="history-row__meta">
                    <time dateTime={item.createdAt} title={formatDateTime(item.createdAt)}>
                      {formatRelativeTime(item.createdAt)}
                    </time>
                  </p>
                  <details className="merge__technical">
                    <summary>Identifiant</summary>
                    <TechnicalId value={item.operationId} label="Copier l’opération" />
                  </details>
                </div>
                <Button
                  size="sm"
                  icon={<RotateCcw aria-hidden size={14} />}
                  disabledReason={locked ? "Opération en cours" : undefined}
                  onClick={() => void workspace.inspect({ action: "previewView", operation: item.operationId }, { action: "restoreView", operation: item.operationId }, "Revenir à cet état")}
                >
                  Revenir à cet état…
                </Button>
              </li>
            ))}
          </ol>
          <footer className="history-footer">
            <p>Historique ancré sur un état fixe : les nouvelles opérations apparaissent après actualisation.</p>
            <Button size="sm" disabledReason={!history.page?.next ? "Fin de l’historique." : undefined} busy={history.loading} onClick={() => void history.load(null, true)}>
              Opérations plus anciennes
            </Button>
          </footer>
        </Card>
      ) : (
        history.page && <EmptyState title="Aucune opération ne correspond">Captures, changements de Layer et restaurations apparaîtront ici.</EmptyState>
      )}
    </div>
  );
}
