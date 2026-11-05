import { useEffect, useRef, useState } from "react";
import { ShieldCheck } from "lucide-react";
import { Banner, Button, Card, Link, PageHeader, StatusDot, TechnicalId, formatDateTime, formatRelativeTime, segment } from "@layrs/ui";
import { useProjection, type ResumeKind } from "../app/projection";
import { useWorkspace } from "../app/workspace";
import { nativeProjection, nativeWorkspace } from "../tauri";
import { currentWorktree, worktreeDestination, type WorktreeReply } from "../app/worktrees";
import { ConversionRecovery } from "./limited/ConversionRecovery";
import { SharedReservationRecovery } from "./limited/SharedReservationRecovery";
import { useOfflineTask } from "../app/useOfflineTask";
import { useProjectionInspection } from "../app/useConversionInspection";
import { databaseRecoveryRequest, initializationRecoveryRequest, recoveredInspection, recoveryActionBlocker, recoveryReasons } from "../app/offline";

type Report = {
  clean: boolean;
  complete: boolean;
  checkedObjects: number;
  checkedRows: number;
  reachableObjects: number;
  unreachableObjects: number;
  reconstructibleProjections: number;
  findings: { code: string; severity: string; path: string | null; message: string }[];
};

/** P23: readable local state, recovery deduced from the journal, technical details folded. */
export function HealthPage() {
  const { kind, data } = useWorkspace();
  return (
    <div className="layrs-page layrs-page--narrow">
      <PageHeader
        title="Diagnostic et reprise"
        description="État local de ce projet, guidé par le journal. Aucune réparation destructrice n’est lancée automatiquement."
      />
      {kind === "projection" ? <LimitedHealth /> : <CompleteHealth />}
      {kind !== "projection" && !data && <ConversionRecovery />}
    </div>
  );
}

function CompleteHealth() {
  const workspace = useWorkspace();
  const { data, busy } = workspace;
  const operation = data?.state.operation;
  const pendingCreation = data && currentWorktree(data.state)?.status === "stale";
  async function resumeCreation() {
    const result = await workspace.task("Reprise de la création…", async () => {
      const reply = await nativeWorkspace<WorktreeReply>(workspace.root, { action: "resumeWorktreeCreation" });
      worktreeDestination(reply);
      return reply;
    });
    if (result) await workspace.refresh();
  }
  return (
    <>
      <Card className="health-summary">
        <div className="health-summary__row">
          <StatusDot tone={!data ? "info" : data.needsRepair ? "danger" : pendingCreation ? "warning" : "success"}>
            <strong>{!data ? "Lecture de l’état local…" : data.needsRepair ? "Reprise nécessaire" : pendingCreation ? "Worktree en attente de préparation" : "Aucune opération interrompue"}</strong>
          </StatusDot>
        </div>
        {operation && (
          <p className="sync-muted">
            Dernière opération : {operation.summary} ·{" "}
            <time dateTime={operation.createdAt} title={formatDateTime(operation.createdAt)}>
              {formatRelativeTime(operation.createdAt)}
            </time>
          </p>
        )}
      </Card>
      {data?.needsRepair && (
        <Banner
          tone="danger"
          title="Une écriture a été interrompue"
          actions={
            <Button variant="primary" busy={busy === "Reprise…"} busyLabel="Reprise…" disabledReason={busy && busy !== "Reprise…" ? "Opération en cours" : undefined} onClick={() => void workspace.run({ action: "recover" }, "Reprise…")}>
              Reprendre après interruption
            </Button>
          }
        >
          Les sauvegardes sont conservées. La reprise termine ou annule l’opération selon le journal ; le moteur confirme ensuite le retour à un état utilisable.
        </Banner>
      )}
      {(pendingCreation || !data) && (
        <Banner tone="warning" title={pendingCreation ? "La création de ce Worktree reste à terminer" : "Reprendre une création interrompue dans ce dossier"} actions={
          <Button variant="primary" busy={busy === "Reprise de la création…"} busyLabel="Reprise…" disabledReason={data?.needsRepair ? "Reprenez d’abord l’écriture interrompue ci-dessus." : busy ? "Opération en cours" : undefined} onClick={() => void resumeCreation()}>
            Reprendre la création du Worktree
          </Button>
        }>
          Le moteur recherche l’intention de création enregistrée dans ce dossier, y compris avant son inscription dans la liste. La Step de départ et les fichiers sont conservés ; les collisions sont vérifiées avant de déclarer le Worktree prêt.
        </Banner>
      )}
      <Integrity check={() => nativeWorkspace<Report>(workspace.root, { action: "diagnose" })} render={(report) => <ReportView report={report as Report} />} />
    </>
  );
}

function ReportView({ report }: { report: Report }) {
  return (
    <section aria-label="Résultat du diagnostic" className="health-report">
      <StatusDot tone={!report.complete ? "warning" : report.clean ? "success" : "danger"}>
        <span role="status">
          {!report.complete
            ? "Vérification incomplète : limite de diagnostic atteinte."
            : report.clean
              ? "Aucune erreur d’intégrité détectée."
              : "Des erreurs d’intégrité nécessitent une intervention."}
        </span>
      </StatusDot>
      <p className="sync-muted">
        {report.checkedObjects} objets et {report.checkedRows} lignes vérifiés · {report.reachableObjects} objets accessibles · {report.unreachableObjects} sans
        référence · {report.reconstructibleProjections} projections reconstructibles.
      </p>
      {report.findings.length > 0 && (
        <ul className="health-findings">
          {report.findings.map((finding, index) => (
            <li key={index}>
              <strong>{finding.message}</strong>
              <span className="sync-muted">
                {" "}
                · {finding.severity} · <code>{finding.code}</code>
                {finding.path && (
                  <>
                    {" "}
                    · <code>{finding.path}</code>
                  </>
                )}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/** Read-only integrity check, available even if the project no longer opens. */
function Integrity({ check, render }: { check: () => Promise<unknown>; render: (report: unknown) => JSX.Element }) {
  const workspace = useWorkspace();
  const [report, setReport] = useState<unknown>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const generation = useRef(0);
  useEffect(() => {
    generation.current++;
    setReport(null);
    setError("");
    setLoading(false);
  }, [workspace.root]);

  async function run() {
    const current = ++generation.current;
    setLoading(true);
    setError("");
    setReport(null);
    try {
      const result = await check();
      if (generation.current === current) setReport(result);
    } catch (cause) {
      if (generation.current === current) setError(String(cause));
    } finally {
      if (generation.current === current) setLoading(false);
    }
  }

  return (
    <section className="settings-section" aria-labelledby="health-integrity">
      <h2 id="health-integrity">Intégrité du dossier</h2>
      <p className="settings-section__hint">Vérification en lecture seule. Les fichiers et preuves sont conservés ; aucune réparation n’est lancée.</p>
      <Card className="settings-card">
        <div>
          <Button icon={<ShieldCheck aria-hidden size={16} />} busy={loading} busyLabel="Vérification…" onClick={() => void run()}>
            Vérifier l’intégrité
          </Button>
        </div>
        {error && (
          <Banner tone="danger" title="La vérification n’a pas abouti.">
            <code className="sync-detail">{error}</code>
          </Banner>
        )}
        {report !== null && render(report)}
      </Card>
    </section>
  );
}

const resumeKinds: { kind: ResumeKind; label: string }[] = [
  { kind: "resumeInstallation", label: "Reprendre la récupération locale" },
  { kind: "resumeNamespace", label: "Reprendre une création ou un déplacement" },
  { kind: "resumeRefresh", label: "Reprendre la réception locale" },
  { kind: "resumeRestore", label: "Reprendre la restauration locale" },
  { kind: "resumeRebase", label: "Reprendre la fusion locale" },
];

function LimitedHealth() {
  const workspace = useWorkspace();
  const projection = useProjection();
  const inspection = useProjectionInspection("inspectRecovery");
  const { source, error, loading } = inspection;
  const offlineTask = useOfflineTask();
  const [recovered, setRecovered] = useState<"databases" | "initialization" | null>(null);
  const databaseRecoveryNeeded = source?.recovery.some(r => r === "familyRollback" || r === "worktreeRollback");
  const initializationReady = source?.recovery.length === 1 && source.recovery[0] === "initializing";
  const filesystemBlocked = workspace.busy ? "Opération en cours." : recoveryActionBlocker(source, "filesystem");
  async function resumeFilesystem(kind: ResumeKind) {
    if (filesystemBlocked || !source) return;
    inspection.invalidate();
    await projection.resume(kind, source);
  }
  async function recover(kind: "databases" | "initialization") {
    if (!source || !(kind === "databases" ? databaseRecoveryNeeded : initializationReady) || loading || offlineTask.busy) return;
    setRecovered(null);
    const result = await offlineTask.execute(kind === "databases" ? "Reprise des bases locales…" : "Reprise de l’initialisation…", async () => {
      try { return recoveredInspection(await offlineTask.local(kind === "databases" ? databaseRecoveryRequest(source) : initializationRecoveryRequest(source)), source); }
      catch (cause) { inspection.invalidate(); throw cause; }
    });
    if (result) { inspection.replace(result); setRecovered(kind); }
  }
  return (
    <>
      <Card className="health-summary">
        <StatusDot tone={error || source?.recovery.length ? "warning" : "info"}>
          <strong>{loading ? "Inspection en lecture seule…" : !source ? "État local non déterminé" : source.recovery.length ? "Des reprises restent nécessaires" : "Aucun besoin de reprise signalé par l’inspection"}</strong>
        </StatusDot>
        <p>Ce diagnostic inspecte le dossier sans migration ni reprise automatique. Chaque besoin restant doit être traité explicitement. Les pages Fichiers et Changements utilisent l’ouverture ordinaire du dossier, qui peut mettre à jour son format.</p>
        {source && <dl className="settings-facts"><dt>Worktree</dt><dd><TechnicalId value={source.identity.worktreeId} /></dd><dt>Stockage</dt><dd>{source.shared ? "Famille partagée" : "Propre à ce dossier"}</dd></dl>}
        {source && source.recovery.length > 0 && <ul aria-label="Besoins de reprise restants">{source.recovery.map(reason => <li key={reason}>{recoveryReasons[reason]}</li>)}</ul>}
        {source?.recovery.includes("initializing") && !initializationReady && <p>Traitez les autres besoins de reprise avant de terminer l’initialisation. Aucune ouverture ordinaire n’est lancée.</p>}
        {source?.recovery.includes("publication") && <p>La publication reste à confirmer séparément depuis <Link to={`/projects/${segment(workspace.root)}/sync`}>Synchronisation</Link>.</p>}
        {databaseRecoveryNeeded && <Button disabledReason={workspace.busy || loading ? "Opération ou inspection en cours." : undefined} onClick={() => void recover("databases")}>Reprendre les bases de données locales</Button>}
        {initializationReady && <Button disabledReason={workspace.busy || loading ? "Opération ou inspection en cours." : undefined} onClick={() => void recover("initialization")}>Reprendre l’initialisation du dossier</Button>}
        {recovered && <p role="status">{recovered === "databases" ? "Reprise des bases exécutée." : "Reprise de l’initialisation exécutée."} Les besoins restants sont indiqués ci-dessus ; aucune autre reprise n’a été lancée automatiquement.</p>}
        {error && <Banner tone="danger" title="Inspection en lecture seule indisponible"><code>{error}</code></Banner>}
        <div>
          <Button size="sm" busy={loading} busyLabel="Inspection en lecture seule…" disabledReason={workspace.busy ? "Opération en cours." : undefined} onClick={() => void inspection.refresh()}>
            Relire sans modifier le dossier
          </Button>
        </div>
      </Card>
      <section className="settings-section" aria-labelledby="health-resume">
        <h2 id="health-resume">Reprise après interruption</h2>
        <p className="settings-section__hint">
          Si Layrs s’est arrêté pendant une écriture, reprenez l’opération qui était en cours. Le journal local doit la confirmer : une reprise sans opération
          correspondante est refusée sans rien modifier. Le moteur n’indique pas encore laquelle attend ; choisissez celle que vous aviez lancée.
        </p>
        <Card className="settings-card health-resume">
          {resumeKinds.map((item) => (
            <Button key={item.kind} size="sm" disabledReason={filesystemBlocked} onClick={() => void resumeFilesystem(item.kind)}>
              {item.label}
            </Button>
          ))}
        </Card>
      </section>
      <SharedReservationRecovery inspection={source} invalidate={inspection.invalidate} />
      <ConversionRecovery execute={offlineTask.execute} />
      <Integrity
        check={() => projection.serial(() => nativeProjection<Record<string, unknown>>(workspace.root, { action: "fsck" }))}
        render={(report) => (
          <section aria-label="Résultat du diagnostic" className="health-report">
            <StatusDot tone="success">
              <span role="status">Vérification terminée sans erreur signalée.</span>
            </StatusDot>
            <dl className="settings-facts">
              {Object.entries(report as Record<string, unknown>)
                .filter(([, value]) => typeof value === "number" || typeof value === "boolean")
                .map(([name, value]) => (
                  <div key={name} className="health-fact">
                    <dt>
                      <code>{name}</code>
                    </dt>
                    <dd>{String(value)}</dd>
                  </div>
                ))}
            </dl>
          </section>
        )}
      />
    </>
  );
}
