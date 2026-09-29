import { useEffect, useMemo, useState, type FormEvent } from "react";
import { ArrowLeft, Camera, GitMerge, History, Monitor } from "lucide-react";
import { Button } from "../components/Button";
import { Badge, Banner, EmptyState } from "../components/Feedback";
import { SelectField, TextField } from "../components/Field";
import { formatDateTime } from "../foundation/format";
import { ComparePanel } from "./ComparePanel";
import {
  captureNames,
  captureReasonLabel,
  captureTitle,
  emptyHistoryFilters,
  explainCollabError,
  historyFilterRequest,
  namedCaptures,
  personInSentence,
  type HistoryFilters,
} from "./model";
import { CollabError, DelayedRows, TechnicalDetails, When } from "./parts";
import type { CollabTransport, HistoryObservation, HistoryPageData, HistoryStep, PeopleDirectory, ProjectReferences } from "./types";
import { useScopedTasks } from "./useScopedTasks";

const PAGE_SIZE = 30;
const OTHER = "__other";

function defaultAnchor(references: ProjectReferences) {
  return references.layers.find((layer) => layer.heads.length)?.heads[0] ?? "";
}

/**
 * P07 — published captures and their observations. Receipt by the server and
 * dates declared by a client stay distinct; unknown authors stay unknown.
 * Filters live with the caller (e.g. in the URL) so they survive a detail visit.
 */
export function ProjectHistory({
  scope,
  transport,
  references,
  people,
  filters,
  onFiltersChange,
  onOpenCapture,
}: {
  scope: string;
  transport: CollabTransport;
  /** A `restricted` account cannot read the whole-project history; nothing is requested. */
  references: ProjectReferences;
  people?: PeopleDirectory;
  filters: HistoryFilters;
  onFiltersChange: (filters: HistoryFilters) => void;
  onOpenCapture: (stepId: string) => void;
}) {
  const tasks = useScopedTasks(scope);
  const anchor = filters.from || defaultAnchor(references);
  const [form, setForm] = useState<HistoryFilters>({ ...filters, from: anchor });
  const [custom, setCustom] = useState(false);
  const [steps, setSteps] = useState<HistoryStep[]>([]);
  const [page, setPage] = useState<HistoryPageData | null>(null);
  const [observations, setObservations] = useState<Record<string, HistoryObservation[]>>({});
  const [error, setError] = useState<unknown>(null);
  const [attempt, setAttempt] = useState(0);
  const options = useMemo(() => namedCaptures(references), [references]);
  const applied = JSON.stringify([scope, anchor, filters.path, filters.since, filters.until, filters.actor, attempt]);

  useEffect(() => {
    setForm({ ...filters, from: anchor });
    setCustom(Boolean(anchor) && !options.some((option) => option.stepId === anchor));
    setSteps([]);
    setPage(null);
    setObservations({});
    setError(null);
    if (!anchor || references.restricted) return;
    void tasks.run(
      "history",
      async (alive) => {
        const result = await transport<HistoryPageData>({
          action: "stepHistory",
          anchor,
          cursor: null,
          limit: PAGE_SIZE,
          path: filters.path.trim() || null,
          filter: historyFilterRequest(filters),
        });
        if (!alive()) return;
        setPage(result);
        setSteps(result.steps);
        setObservations(result.observations ?? {});
      },
      setError,
    );
  }, [applied]);

  function more() {
    if (!page?.next) return;
    const previous = page;
    void tasks.run(
      "history",
      async (alive) => {
        setError(null);
        const result = await transport<HistoryPageData>({
          action: "stepHistory",
          anchor: previous.anchor,
          cursor: previous.next,
          limit: PAGE_SIZE,
          path: previous.path ?? null,
          filter: previous.filter,
        });
        if (!alive()) return;
        setPage(result);
        setSteps((current) => [...current, ...result.steps.filter((step) => !current.some((known) => known.stepId === step.stepId))]);
        setObservations((current) => ({ ...current, ...(result.observations ?? {}) }));
      },
      setError,
    );
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    onFiltersChange({ ...form, from: form.from.trim() });
    // Reading again with the same filters is a refresh, not a no-op.
    setAttempt((value) => value + 1);
  }

  const loading = tasks.isBusy("history");
  const forbidden = error ? explainCollabError(error).forbidden : false;

  return (
    <section className="collab-history" aria-label="Historique publié">
      <form className="collab-filters" onSubmit={submit} aria-label="Filtres de l’historique">
        <SelectField
          label="Point de départ"
          value={custom ? OTHER : form.from}
          onChange={(event) => {
            const value = event.target.value;
            setCustom(value === OTHER);
            setForm({ ...form, from: value === OTHER ? "" : value });
          }}
        >
          {!options.length && <option value="">Aucune capture nommée</option>}
          {["Layers", "Versions", "Repères"].map((group) => {
            const items = options.filter((option) => option.group === group);
            return items.length ? (
              <optgroup key={group} label={group}>
                {items.map((option) => (
                  <option key={`${group}:${option.stepId}`} value={option.stepId}>
                    {option.label}
                  </option>
                ))}
              </optgroup>
            ) : null;
          })}
          <option value={OTHER}>Autre capture (référence technique)…</option>
        </SelectField>
        {custom && (
          <TextField label="Référence de la capture" value={form.from} placeholder="blake3:…" onChange={(event) => setForm({ ...form, from: event.target.value })} />
        )}
        <TextField
          label="Chemin exact"
          value={form.path}
          maxLength={4096}
          placeholder="docs/guide.md"
          hint="Seulement les captures qui modifient ce chemin."
          onChange={(event) => setForm({ ...form, path: event.target.value })}
        />
        <TextField label="Reçue depuis le" type="date" value={form.since} onChange={(event) => setForm({ ...form, since: event.target.value })} />
        <TextField label="Reçue jusqu’au" type="date" value={form.until} onChange={(event) => setForm({ ...form, until: event.target.value })} />
        <details className="collab-details collab-filters__advanced">
          <summary>Filtre avancé</summary>
          <TextField
            label="Acteur de publication (référence technique)"
            value={form.actor}
            placeholder="actor:…"
            onChange={(event) => setForm({ ...form, actor: event.target.value })}
          />
        </details>
        <div className="collab-actions">
          <Button type="submit" variant="primary" busy={loading} busyLabel="Lecture…" disabledReason={!form.from.trim() ? "Choisissez un point de départ." : undefined}>
            Afficher l’historique
          </Button>
          <Button type="button" variant="ghost" onClick={() => onFiltersChange({ ...emptyHistoryFilters })}>
            Effacer les filtres
          </Button>
        </div>
      </form>

      <p className="collab-muted">
        Les dates de filtre portent sur la réception par le serveur. Une date déclarée par un client n’est pas une date de création vérifiée.
      </p>

      {references.restricted && (
        <Banner tone="warning" title="Historique non accessible">
          Votre accès à ce projet est limité à certains fichiers : l’historique publié complet ne vous est pas ouvert. Les fichiers autorisés restent
          consultables dans Fichiers.
        </Banner>
      )}
      {!anchor && !references.restricted && (
        <EmptyState icon={<History size={22} />} title="Aucune capture publiée">
          L’historique apparaîtra après la première publication d’un Layer depuis Desktop.
        </EmptyState>
      )}
      {error ? (
        forbidden ? (
          <Banner tone="warning" title="Historique non accessible">
            L’historique publié de ce projet n’est pas ouvert à votre compte. Les fichiers autorisés restent consultables dans Fichiers.
          </Banner>
        ) : (
          <CollabError error={error} title="Historique non chargé" kept="Vos filtres sont conservés." onRetry={() => setAttempt((value) => value + 1)} />
        )
      ) : null}
      <DelayedRows active={loading && !steps.length} label="Chargement de l’historique…" />
      {page && (
        <>
          <p className="collab-muted">
            {page.path ? (
              <>Captures qui modifient <strong className="collab-break">{page.path}</strong> par rapport à au moins un parent, sans suivi des renommages. </>
            ) : null}
            Historique figé au moment de la lecture : actualisez pour voir les publications suivantes.
          </p>
          {steps.length ? (
            <ol className="collab-timeline" aria-label="Captures publiées">
              {steps.map((step) => (
                <HistoryRow key={step.stepId} step={step} references={references} people={people} observations={observations[step.stepId] ?? []} onOpen={() => onOpenCapture(step.stepId)} />
              ))}
            </ol>
          ) : (
            <EmptyState compact icon={<History size={20} />} title="Aucune capture ne correspond">
              Élargissez la période ou retirez le filtre de chemin.
            </EmptyState>
          )}
          <div className="collab-pager">
            <p className="collab-muted" role="status">
              {steps.length} capture{steps.length > 1 ? "s" : ""} affichée{steps.length > 1 ? "s" : ""}
              {page.next ? " · historique partiel" : " · début de l’historique atteint"}
            </p>
            {page.next && (
              <Button size="sm" busy={loading} busyLabel="Chargement…" onClick={more}>
                Captures plus anciennes
              </Button>
            )}
          </div>
        </>
      )}
    </section>
  );
}

function Provenance({ observations, people }: { observations: HistoryObservation[]; people?: PeopleDirectory }) {
  if (!observations.length) return <p className="collab-muted">Réception non documentée pour cette capture.</p>;
  return (
    <ul className="collab-provenance">
      {observations.map((item) => (
        <li key={item.observation.observationId}>
          {item.provenance === "client_assertion" ? (
            <>
              <Monitor aria-hidden size={14} />
              <span>
                Date déclarée par un client : <time dateTime={item.observation.observedAt}>{formatDateTime(item.observation.observedAt)}</time> ·{" "}
                {captureReasonLabel(item.observation.captureReason)} · déclaration de {personInSentence(item.principalId, people)}, reçue <When value={item.receivedAt} />
              </span>
            </>
          ) : (
            <>
              <Camera aria-hidden size={14} />
              <span>
                Reçue par le serveur <When value={item.receivedAt} /> · publiée par {personInSentence(item.principalId, people)}
              </span>
            </>
          )}
        </li>
      ))}
    </ul>
  );
}

function HistoryRow({
  step,
  references,
  people,
  observations,
  onOpen,
}: {
  step: HistoryStep;
  references: ProjectReferences;
  people?: PeopleDirectory;
  observations: HistoryObservation[];
  onOpen: () => void;
}) {
  const names = captureNames(step.stepId, references);
  const admission = observations.find((item) => item.provenance === "server_admission") ?? observations[0];
  return (
    <li className="collab-timeline__item">
      <span className="collab-timeline__mark" aria-hidden>
        {step.parentStepIds.length > 1 ? <GitMerge size={16} /> : <Camera size={16} />}
      </span>
      <div className="collab-timeline__body">
        <div className="collab-timeline__head">
          <button type="button" className="collab-link collab-timeline__title" onClick={onOpen}>
            {names[0] ?? (admission ? `Capture reçue le ${formatDateTime(admission.receivedAt)}` : "Capture publiée")}
          </button>
          {names.slice(1).map((name) => (
            <Badge key={name} tone="accent">
              {name}
            </Badge>
          ))}
          {step.parentStepIds.length === 0 && <Badge>Première capture</Badge>}
          {step.parentStepIds.length > 1 && <Badge tone="warning">Fusion de {step.parentStepIds.length} captures</Badge>}
        </div>
        <Provenance observations={observations} people={people} />
        <TechnicalDetails
          items={[
            { label: "Capture", value: step.stepId, copy: "Copier la capture" },
            ...step.parentStepIds.map((parent, index) => ({ label: `Parent ${index + 1}`, value: parent, copy: "Copier" })),
          ]}
        />
      </div>
    </li>
  );
}

/**
 * P08 on the Web — read-only summary and comparison of one capture. Restoring
 * or rewriting a local folder is a Desktop action and is not simulated here.
 */
export function CaptureDetail({
  scope,
  transport,
  references,
  people,
  stepId,
  onBack,
}: {
  scope: string;
  transport: CollabTransport;
  references: ProjectReferences;
  people?: PeopleDirectory;
  stepId: string;
  onBack?: () => void;
}) {
  const key = `${scope}|${stepId}`;
  const tasks = useScopedTasks(key);
  const [step, setStep] = useState<HistoryStep | null>(null);
  const [observations, setObservations] = useState<HistoryObservation[]>([]);
  const [error, setError] = useState<unknown>(null);
  const [before, setBefore] = useState("");
  const [custom, setCustom] = useState(false);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    setStep(null);
    setObservations([]);
    setError(null);
    setBefore("");
    void tasks.run(
      "capture",
      async (alive) => {
        const result = await transport<HistoryPageData>({ action: "stepHistory", anchor: stepId, cursor: null, limit: 1, path: null, filter: { actorId: null, since: null, until: null } });
        if (!alive()) return;
        const found = result.steps.find((item) => item.stepId === stepId);
        if (!found) throw new Error("Cette capture n’est pas présente dans l’historique publié accessible.");
        setStep(found);
        setObservations(result.observations?.[stepId] ?? []);
        setBefore(found.parentStepIds[0] ?? "");
      },
      setError,
    );
  }, [key, attempt]);

  const options = namedCaptures(references).filter((option) => option.stepId !== stepId);
  const loading = tasks.isBusy("capture");
  const title = captureTitle(stepId, references);

  return (
    <section className="collab-capture" aria-label="Capture publiée">
      {onBack && (
        <div>
          <Button variant="ghost" size="sm" icon={<ArrowLeft aria-hidden size={16} />} onClick={onBack}>
            Retour à l’historique
          </Button>
        </div>
      )}
      <header className="collab-capture__head">
        <h2>{title}</h2>
        <div className="collab-badges">
          {captureNames(stepId, references).slice(1).map((name) => (
            <Badge key={name} tone="accent">
              {name}
            </Badge>
          ))}
        </div>
      </header>
      <Banner tone="info">Consultation seule sur le Web : la restauration d’une capture se prépare et s’applique depuis Desktop, avec aperçu.</Banner>
      {error ? <CollabError error={error} title="Capture non chargée" onRetry={() => setAttempt((value) => value + 1)} /> : null}
      <DelayedRows active={loading} rows={2} label="Chargement de la capture…" />
      {step && (
        <>
          <Provenance observations={observations} people={people} />
          <TechnicalDetails items={[{ label: "Capture", value: stepId, copy: "Copier la capture" }, ...step.parentStepIds.map((parent, index) => ({ label: `Parent ${index + 1}`, value: parent, copy: "Copier" }))]} />
          <div className="collab-toolbar">
            <SelectField
              label="Comparer avec"
              value={custom ? OTHER : before}
              onChange={(event) => {
                const value = event.target.value;
                setCustom(value === OTHER);
                if (value !== OTHER) setBefore(value);
              }}
            >
              {!step.parentStepIds.length && <option value="">Aucune capture parente</option>}
              {step.parentStepIds.map((parent, index) => (
                <option key={parent} value={parent}>
                  {step.parentStepIds.length > 1 ? `Capture parente ${index + 1}` : "Capture parente"}
                  {captureNames(parent, references)[0] ? ` · ${captureNames(parent, references)[0]}` : ""}
                </option>
              ))}
              {options.map((option) => (
                <option key={`${option.group}:${option.stepId}`} value={option.stepId}>
                  {option.label}
                </option>
              ))}
              <option value={OTHER}>Autre capture (référence technique)…</option>
            </SelectField>
            {custom && <TextField label="Référence de la capture de départ" value={before} placeholder="blake3:…" onChange={(event) => setBefore(event.target.value.trim())} />}
          </div>
          {before ? (
            <ComparePanel scope={scope} transport={transport} before={before} after={stepId} beforeLabel={captureTitle(before, references, "Avant")} afterLabel={title} />
          ) : (
            <EmptyState compact title="Première capture">Cette capture n’a pas de parent : choisissez une autre capture pour comparer.</EmptyState>
          )}
        </>
      )}
    </section>
  );
}
