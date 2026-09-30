import { useEffect, useRef, useState } from "react";
import { ShieldCheck } from "lucide-react";
import { Banner, Button, Card, PageHeader, StatusDot, formatDateTime, formatRelativeTime } from "@layrs/ui";
import { useProjection, type ResumeKind } from "../app/projection";
import { useWorkspace } from "../app/workspace";
import { nativeProjection, nativeWorkspace } from "../tauri";

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
  const { kind } = useWorkspace();
  return (
    <div className="layrs-page layrs-page--narrow">
      <PageHeader
        title="Diagnostic et reprise"
        description="État local de ce projet, guidé par le journal. Aucune réparation destructrice n’est lancée automatiquement."
      />
      {kind === "projection" ? <LimitedHealth /> : <CompleteHealth />}
    </div>
  );
}

function CompleteHealth() {
  const workspace = useWorkspace();
  const { data, busy } = workspace;
  const operation = data?.state.operation;
  return (
    <>
      <Card className="health-summary">
        <div className="health-summary__row">
          <StatusDot tone={!data ? "info" : data.needsRepair ? "danger" : "success"}>
            <strong>{!data ? "Lecture de l’état local…" : data.needsRepair ? "Reprise nécessaire" : "Aucune opération interrompue"}</strong>
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
  const { data } = projection;
  return (
    <>
      <Card className="health-summary">
        <StatusDot tone={!data ? "warning" : data.needsRepair ? "danger" : "success"}>
          <strong>{!data ? "État local à relire" : data.needsRepair ? "Reprise nécessaire" : "Aucune opération interrompue signalée"}</strong>
        </StatusDot>
        {data?.pendingPublication && (
          <p className="sync-muted">Une publication (capture {data.pendingPublication.sequence}) attend son reçu : relancez-la depuis Synchronisation.</p>
        )}
        <div>
          <Button size="sm" disabledReason={workspace.busy ? "Opération en cours." : undefined} onClick={() => void projection.refresh()}>
            Actualiser l’accès limité
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
            <Button key={item.kind} size="sm" disabledReason={workspace.busy ? "Opération en cours." : undefined} onClick={() => void projection.resume(item.kind)}>
              {item.label}
            </Button>
          ))}
        </Card>
      </section>
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
