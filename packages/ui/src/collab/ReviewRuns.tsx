import { useEffect, useMemo, useState } from "react";
import { ArrowLeft, FlaskConical, ScrollText } from "lucide-react";
import { Button } from "../components/Button";
import { Badge, Banner, EmptyState, type Tone } from "../components/Feedback";
import { Dialog } from "../components/Dialog";
import { TextAreaField } from "../components/Field";
import { SegmentedControl } from "../components/SegmentedControl";
import { useToast } from "../components/Toast";
import { formatDateTime } from "../foundation/format";
import { nativeRunPresentation, type NativeRunPresentation } from "../native-run-state";
import { captureTitle, denied, personInSentence, personLabel } from "./model";
import { CollabError, DelayedRows, TechnicalDetails, When } from "./parts";
import type { CollabCapabilities, CollabTransport, PeopleDirectory, ProjectReferences, RunDetailData, RunSummary } from "./types";
import { useScopedTasks } from "./useScopedTasks";

const PAGE_SIZE = 50;

export function runTone(presentation: NativeRunPresentation): Tone {
  if (presentation.state === "completed") return presentation.label === "Réussi" ? "success" : "danger";
  if (presentation.state === "cancelled" || presentation.state === "unknown") return "warning";
  if (presentation.state === "expired") return "warning";
  return "info";
}

/**
 * Validations of a review. The runs API lists the whole project page by page;
 * the capture filter applies to the pages already loaded and says so.
 */
export function ReviewValidations({
  scope,
  transport,
  stepId,
  references,
  people,
  onOpenRun,
  capabilities,
}: {
  scope: string;
  transport: CollabTransport;
  capabilities?: CollabCapabilities;
  stepId: string;
  references: ProjectReferences;
  people?: PeopleDirectory;
  onOpenRun: (runId: string) => void;
}) {
  const tasks = useScopedTasks(scope);
  const [runs, setRuns] = useState<RunSummary[] | null>(null);
  const [cursor, setCursor] = useState<string | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [scopeFilter, setScopeFilter] = useState<"capture" | "project">("capture");

  function load(next = false) {
    return tasks.run(
      "runs",
      async (alive) => {
        setError(null);
        const result = await transport<{ runs: RunSummary[]; nextCursor: string | null }>({ action: "listRunsPage", cursor: next ? cursor : null, limit: PAGE_SIZE });
        if (!alive()) return;
        setRuns((current) => (next && current ? [...current, ...result.runs.filter((run) => !current.some((known) => known.runId === run.runId))] : result.runs));
        setCursor(result.nextCursor);
      },
      setError,
    );
  }

  useEffect(() => {
    setRuns(null);
    setCursor(null);
    if (!denied(capabilities, "readRuns")) void load();
  }, [scope]);

  const shown = useMemo(() => (runs ?? []).filter((run) => scopeFilter === "project" || run.stepId === stepId), [runs, scopeFilter, stepId]);
  const loading = tasks.isBusy("runs");

  if (denied(capabilities, "readRuns"))
    return (
      <div className="collab-validations" role="region" aria-label="Validations">
        <Banner tone="warning" title="Validations non accessibles">
          Votre compte ne peut pas consulter les exécutions de validation de ce projet.
        </Banner>
      </div>
    );

  return (
    <div className="collab-validations" role="region" aria-label="Validations">
      <div className="collab-toolbar">
        <SegmentedControl
          label="Exécutions affichées"
          value={scopeFilter}
          onChange={setScopeFilter}
          options={[
            { value: "capture", label: "Capture proposée" },
            { value: "project", label: "Tout le projet" },
          ]}
        />
        <Button busy={loading} busyLabel="Actualisation…" onClick={() => void load()}>
          Actualiser
        </Button>
      </div>
      {runs && (
        <p className="collab-muted">
          {shown.length} exécution{shown.length > 1 ? "s" : ""} parmi les {runs.length} chargée{runs.length > 1 ? "s" : ""}
          {cursor ? " ; d’autres restent à charger." : "."} Les preuves exigées sont revérifiées par le serveur au moment de l’intégration.
        </p>
      )}
      {error ? <CollabError error={error} title="Validations non chargées" onRetry={() => void load()} /> : null}
      <DelayedRows active={loading && !runs} label="Chargement des validations…" />
      {runs && !shown.length && (
        <EmptyState compact icon={<FlaskConical size={20} />} title={scopeFilter === "capture" ? "Aucune validation pour cette capture" : "Aucune exécution"}>
          {scopeFilter === "capture" ? "Aucune exécution chargée ne porte sur la capture proposée." : "Aucun exécuteur n’a encore déclaré de validation dans ce projet."}
        </EmptyState>
      )}
      {shown.length > 0 && (
        <ul className="collab-cards" aria-label="Exécutions de validation">
          {shown.map((run) => {
            const presentation = nativeRunPresentation(run);
            return (
              <li key={run.runId} className="collab-run-row">
                <Badge tone={runTone(presentation)}>{presentation.label}</Badge>
                <div className="collab-review-row__text">
                  <span className="collab-review-row__title">Validation sur « {captureTitle(run.stepId, references)} »</span>
                  <span className="collab-row__meta">
                    {presentation.detail}
                    {run.createdAt ? (
                      <>
                        {" "}
                        · déclarée <When value={run.createdAt} />
                      </>
                    ) : null}
                    {run.runnerPrincipalId ? ` · exécuteur : ${personInSentence(run.runnerPrincipalId, people)}` : ""}
                  </span>
                  <TechnicalDetails items={[{ label: "Exécution", value: run.runId }, { label: "Capture", value: run.stepId }, { label: "Preuve", value: run.proofObjectId }]} />
                </div>
                <Button size="sm" icon={<ScrollText aria-hidden size={14} />} onClick={() => onOpenRun(run.runId)}>
                  Consulter la preuve et le journal
                </Button>
              </li>
            );
          })}
        </ul>
      )}
      {cursor && (
        <div className="collab-pager">
          <Button size="sm" busy={loading} busyLabel="Chargement…" onClick={() => void load(true)}>
            Exécutions suivantes
          </Button>
        </div>
      )}
    </div>
  );
}

/**
 * P13b — one validation run: exact capture, result, runner, recipe and log.
 * Result and log availability are shown separately. Cancelling refuses any
 * later result; it does not stop a remote container.
 */
export function RunDetail({
  scope,
  transport,
  runId,
  references,
  people,
  onBack,
  backLabel = "Retour à la revue",
}: {
  scope: string;
  transport: CollabTransport;
  runId: string;
  references: ProjectReferences;
  people?: PeopleDirectory;
  onBack?: () => void;
  backLabel?: string;
}) {
  const key = `${scope}|${runId}`;
  const tasks = useScopedTasks(key);
  const toast = useToast();
  const [run, setRun] = useState<RunDetailData | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [cancelling, setCancelling] = useState(false);
  const [reason, setReason] = useState("");
  const [cancelError, setCancelError] = useState<unknown>(null);
  const [cancelKey, setCancelKey] = useState({ signature: "", key: "" });

  function load() {
    return tasks.run(
      "run",
      async (alive) => {
        setError(null);
        const result = await transport<RunDetailData>({ action: "runDetail", run: runId });
        if (alive()) setRun(result);
      },
      setError,
    );
  }

  useEffect(() => {
    setRun(null);
    setError(null);
    setCancelling(false);
    setReason("");
    void load();
  }, [key]);

  function cancel() {
    const signature = JSON.stringify([runId, reason]);
    const idempotencyKey = cancelKey.signature === signature ? cancelKey.key : crypto.randomUUID();
    setCancelKey({ signature, key: idempotencyKey });
    void tasks.run(
      "cancel",
      async (alive) => {
        setCancelError(null);
        const result = await transport<Pick<RunSummary, "status" | "cancellation">>({ action: "cancelRun", run: runId, request: { reason, idempotencyKey } });
        if (!alive()) return;
        setRun((current) => (current ? { ...current, ...result } : current));
        setCancelKey({ signature: "", key: "" });
        setCancelling(false);
        setReason("");
        toast.show({ tone: "success", title: "Acceptation du résultat annulée", description: "Toute preuve ultérieure de cette exécution sera refusée." });
      },
      setCancelError,
    );
  }

  const presentation = run ? nativeRunPresentation(run) : null;
  const logs = run?.evidence.filter((entry) => entry.schemaVersion === "layrs.run-verdict.v1") ?? [];

  return (
    <section className="collab-run" aria-label="Preuve et journal de validation">
      {onBack && (
        <div>
          <Button variant="ghost" size="sm" icon={<ArrowLeft aria-hidden size={16} />} onClick={onBack}>
            {backLabel}
          </Button>
        </div>
      )}
      {error ? <CollabError error={error} title="Validation non chargée" onRetry={() => void load()} /> : null}
      <DelayedRows active={tasks.isBusy("run") && !run} rows={3} label="Chargement de la validation…" />
      {run && presentation && (
        <>
          <div className="collab-run__summary">
            <Badge tone={runTone(presentation)}>{presentation.label}</Badge>
            <p>{presentation.detail}</p>
          </div>
          <dl className="collab-facts collab-facts--grid">
            <div>
              <dt>Capture exacte</dt>
              <dd>{captureTitle(run.stepId, references)}</dd>
            </div>
            <div>
              <dt>Exécuteur</dt>
              <dd>{personLabel(run.runnerPrincipalId, people)}</dd>
            </div>
            {run.createdAt && (
              <div>
                <dt>Déclarée</dt>
                <dd>{formatDateTime(run.createdAt)}</dd>
              </div>
            )}
            <div>
              <dt>Terminée</dt>
              <dd>{run.completedAt ? formatDateTime(run.completedAt) : "Pas de résultat reçu"}</dd>
            </div>
            {run.expiresAt && (
              <div>
                <dt>Échéance</dt>
                <dd>{formatDateTime(run.expiresAt)}</dd>
              </div>
            )}
          </dl>
          {run.cancellation && (
            <Banner tone="warning" title="Acceptation du résultat annulée">
              <p>
                Le {formatDateTime(run.cancellation.record.cancelledAt)} par {personInSentence(run.cancellation.record.cancelledBy, people)}.
              </p>
              <p className="collab-thread__text">{run.cancellation.record.reason}</p>
            </Banner>
          )}
          <section aria-label="Journal de l’exécution" className="collab-log">
            <h2>Journal</h2>
            {logs.length ? (
              logs.map((entry, index) => (
                <pre key={index} className="collab-log__text">
                  {entry.log || "Journal vide."}
                </pre>
              ))
            ) : (
              <EmptyState compact title="Journal indisponible">
                Aucun journal n’a été reçu pour cette exécution ; cela ne dit rien de son résultat.
              </EmptyState>
            )}
            <p className="collab-muted">Journal tel que transmis par l’exécuteur, borné par la taille maximale de la recette.</p>
          </section>
          <TechnicalDetails
            summary="Provenance"
            items={[
              { label: "Exécution", value: run.runId },
              { label: "Capture", value: run.stepId, copy: "Copier la capture" },
              { label: "Recette", value: run.recipeObjectId },
              { label: "Exécuteur", value: run.runnerPrincipalId },
              { label: "Preuve", value: run.proofObjectId },
              { label: "Preuve d’annulation", value: run.cancellation?.evidenceObjectId },
            ]}
          />
          {presentation.canCancel && (
            <div className="collab-actions">
              <Button variant="danger" onClick={() => setCancelling(true)}>
                Annuler l’acceptation du résultat…
              </Button>
            </div>
          )}
        </>
      )}
      <Dialog
        open={cancelling}
        onClose={() => setCancelling(false)}
        title="Annuler l’acceptation du résultat"
        description="Le serveur refusera toute preuve ultérieure de cette exécution. Cette action ne termine pas un conteneur distant : son délai maximal reste applicable."
        footer={
          <>
            <Button onClick={() => setCancelling(false)}>Conserver l’exécution</Button>
            <Button variant="danger" busy={tasks.isBusy("cancel")} busyLabel="Annulation…" disabledReason={!reason.trim() ? "Indiquez un motif." : undefined} onClick={cancel}>
              Confirmer l’annulation
            </Button>
          </>
        }
      >
        <TextAreaField label="Motif d’annulation" value={reason} maxLength={4096} onChange={(event) => setReason(event.target.value)} />
        {cancelError ? <CollabError error={cancelError} title="Annulation non confirmée" kept="Réessayer réutilise la même demande, sans doublon." /> : null}
      </Dialog>
    </section>
  );
}
