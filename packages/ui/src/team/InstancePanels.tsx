import { useCallback, useEffect, useRef, useState } from "react";
import { Database, Gauge, HardDrive, RefreshCw } from "lucide-react";
import { Button } from "../components/Button";
import { Badge, Banner, Card, Loading } from "../components/Feedback";
import { formatBytes, formatDateTime } from "../foundation/format";
import { nativeMetrics, type NativeMetrics } from "../native-metrics-model";
import type { TeamTransport } from "./types";
import { useTask } from "./useTask";

type StorageReport = {
  severity: string;
  databaseBytes: number;
  globalUsage: { chargedBytes: number; objects: number; cataloguedBytes: number; reservedOnlyBytes: number };
  casVolume: { totalBytes: number; availableBytes: number; freeBytes: number };
  readings: { resource: string; unit: string; used: number; warningAt: number | null; criticalAt: number | null; monitored: boolean; severity: string }[];
};

const resources: Record<string, string> = {
  global_cas_bytes: "Stockage logique de l’instance",
  global_cas_objects: "Objets de l’instance",
  space_cas_bytes: "Stockage logique du projet",
  space_cas_objects: "Objets du projet",
  cas_volume_used: "Occupation du disque de stockage",
  database_bytes: "Base PostgreSQL",
  postgres_database: "Base PostgreSQL",
  cas_volume_unavailable: "Disque de stockage occupé ou indisponible",
};
const severity: Record<string, { label: string; tone: "success" | "warning" | "danger" }> = {
  normal: { label: "Normal", tone: "success" },
  warning: { label: "Alerte", tone: "warning" },
  critical: { label: "Critique", tone: "danger" },
};
const sev = (value: string) => severity[value] ?? { label: "Normal", tone: "success" as const };

function number(value: unknown) {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

/** Accepts only the known fields; anything else is refused, not displayed. */
function checkedStorage(value: unknown): StorageReport {
  const report = value as StorageReport;
  if (
    !report ||
    typeof report !== "object" ||
    typeof report.severity !== "string" ||
    !number(report.databaseBytes) ||
    !report.globalUsage ||
    !number(report.globalUsage.chargedBytes) ||
    !number(report.globalUsage.objects) ||
    !number(report.globalUsage.reservedOnlyBytes) ||
    !report.casVolume ||
    !number(report.casVolume.totalBytes) ||
    !number(report.casVolume.availableBytes) ||
    !Array.isArray(report.readings)
  )
    throw new Error("Rapport de stockage incomplet.");
  return report;
}

function useRead<T>(scope: string, read: () => Promise<T>, context: string) {
  const task = useTask(scope);
  const [value, setValue] = useState<T | null>(null);
  const load = useCallback(
    () =>
      task.run("load", async (live) => {
        const next = await read();
        if (live()) setValue(next);
      }, context),
    [task, read, context],
  );
  const started = useRef("");
  useEffect(() => {
    if (started.current === scope) return;
    started.current = scope;
    void load();
  }, [scope, load]);
  return { task, value, load };
}

/** P21 · Stockage: logical budgets, reservations, disk and database, kept apart. */
export function StoragePanel({ scope, administration }: { scope: string; administration: TeamTransport }) {
  const read = useCallback(async () => checkedStorage(await administration<unknown>({ action: "storageBudget", space: null })), [administration]);
  const { task, value, load } = useRead(scope, read, "Le stockage n’a pas pu être lu.");
  return (
    <div className="team-panel team-panel--narrow">
      <div className="team-toolbar">
        <p className="team-muted">Les budgets logiques comptent les réservations ; le disque et PostgreSQL sont mesurés séparément et ne s’additionnent pas.</p>
        <Button icon={<RefreshCw aria-hidden size={16} />} busy={task.pending === "load"} busyLabel="Lecture…" onClick={() => void load()}>
          Actualiser
        </Button>
      </div>
      {task.error && <Banner tone="danger">{task.error}</Banner>}
      {!value ? (
        task.pending && <Loading label="Lecture du stockage…" />
      ) : (
        <>
          <div className="team-stats">
            <Card className="team-stat">
              <p className="team-stat__label">État général</p>
              <Badge tone={sev(value.severity).tone}>{sev(value.severity).label}</Badge>
            </Card>
            <Card className="team-stat">
              <p className="team-stat__label">
                <Database aria-hidden size={14} /> Stockage logique
              </p>
              <p className="team-stat__value">{formatBytes(value.globalUsage.chargedBytes)}</p>
              <p className="team-row__meta">
                {value.globalUsage.objects} objets · réservations seules {formatBytes(value.globalUsage.reservedOnlyBytes)}
              </p>
            </Card>
            <Card className="team-stat">
              <p className="team-stat__label">
                <HardDrive aria-hidden size={14} /> Disque de stockage
              </p>
              <p className="team-stat__value">{formatBytes(value.casVolume.availableBytes)} disponibles</p>
              <p className="team-row__meta">sur {formatBytes(value.casVolume.totalBytes)}</p>
            </Card>
            <Card className="team-stat">
              <p className="team-stat__label">
                <Database aria-hidden size={14} /> PostgreSQL
              </p>
              <p className="team-stat__value">{formatBytes(value.databaseBytes)}</p>
            </Card>
          </div>
          <Card className="settings-card">
            <table className="team-table">
              <caption>Seuils surveillés</caption>
              <thead>
                <tr>
                  <th scope="col">Mesure</th>
                  <th scope="col">Utilisation</th>
                  <th scope="col">Alerte / critique</th>
                  <th scope="col">État</th>
                </tr>
              </thead>
              <tbody>
                {value.readings.map((reading) => {
                  const show = (amount: number | null) => (amount === null ? "—" : reading.unit === "bytes" ? formatBytes(amount) : String(amount));
                  return (
                    <tr key={reading.resource}>
                      <th scope="row">{resources[reading.resource] ?? reading.resource}</th>
                      <td>{show(reading.used)}</td>
                      <td>
                        {show(reading.warningAt)} / {show(reading.criticalAt)}
                      </td>
                      <td>{reading.monitored ? <Badge tone={sev(reading.severity).tone}>{sev(reading.severity).label}</Badge> : "Seuil non configuré"}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
            <p className="team-muted team-small">Cette observation ne supprime aucun objet. Les quotas logiques ne réservent pas physiquement l’espace disque.</p>
          </Card>
        </>
      )}
    </div>
  );
}

/** P21 · État du serveur: counters of the current process, not a history. */
export function ServerStatusPanel({ scope, administration }: { scope: string; administration: TeamTransport }) {
  const read = useCallback(async () => nativeMetrics(await administration<unknown>({ action: "metrics" })), [administration]);
  const { task, value, load } = useRead<NativeMetrics>(scope, read, "Les métriques n’ont pas pu être lues.");
  const average = value && value.http.completed > 0 ? (value.http.headersLatencyMicrosecondsTotal / value.http.completed / 1000).toFixed(2).replace(".", ",") : null;
  return (
    <div className="team-panel team-panel--narrow">
      <div className="team-toolbar">
        <p className="team-muted">Compteurs du processus serveur en cours : ils repartent de zéro à chaque redémarrage et ne constituent pas un historique d’activité.</p>
        <Button icon={<RefreshCw aria-hidden size={16} />} busy={task.pending === "load"} busyLabel="Lecture…" onClick={() => void load()}>
          Actualiser
        </Button>
      </div>
      {task.error && <Banner tone="danger">{task.error}</Banner>}
      {!value ? (
        task.pending && <Loading label="Lecture des métriques…" />
      ) : (
        <>
          <div className="team-stats">
            <Card className="team-stat">
              <p className="team-stat__label">
                <Gauge aria-hidden size={14} /> Observation
              </p>
              <p className="team-stat__value">{formatDateTime(value.observedUnixMs)}</p>
              <p className="team-row__meta">processus actif depuis {Math.round(value.uptimeSeconds / 60)} min</p>
            </Card>
            <Card className="team-stat">
              <p className="team-stat__label">Réponses terminées</p>
              <p className="team-stat__value">{value.http.completed}</p>
              <p className="team-row__meta">
                {value.http.inFlight} en cours · {value.http.interruptedBeforeHeaders} interrompues
              </p>
            </Card>
            <Card className="team-stat">
              <p className="team-stat__label">Latence moyenne</p>
              <p className="team-stat__value">{average === null ? "—" : `${average} ms`}</p>
              <p className="team-row__meta">jusqu’aux en-têtes</p>
            </Card>
            <Card className="team-stat">
              <p className="team-stat__label">Sondes de disponibilité</p>
              <p className="team-stat__value">{value.readiness.checks}</p>
              <p className="team-row__meta">
                échecs PostgreSQL {value.readiness.databaseFailures} · stockage {value.readiness.casFailures}
              </p>
            </Card>
          </div>
          <div className="team-grid-2">
            <Card className="settings-card">
              <table className="team-table">
                <caption>Réponses HTTP par classe</caption>
                <thead>
                  <tr>
                    <th scope="col">Classe</th>
                    <th scope="col">Nombre</th>
                  </tr>
                </thead>
                <tbody>
                  {value.http.statusClasses.map((row) => (
                    <tr key={row.class}>
                      <th scope="row">
                        {row.class}
                        {row.class === "5xx" ? " · erreurs serveur" : row.class === "4xx" ? " · requêtes refusées" : ""}
                      </th>
                      <td>{row.count}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </Card>
            <Card className="settings-card">
              <table className="team-table">
                <caption>Latences cumulées jusqu’aux en-têtes</caption>
                <thead>
                  <tr>
                    <th scope="col">Durée maximale</th>
                    <th scope="col">Réponses</th>
                  </tr>
                </thead>
                <tbody>
                  {value.http.headersLatencyBuckets.map((row) => (
                    <tr key={row.lessOrEqualMs}>
                      <th scope="row">≤ {row.lessOrEqualMs} ms</th>
                      <td>{row.count}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </Card>
          </div>
          <p className="team-muted team-small">
            Observation ponctuelle et approximative. Les sondes ne recensent pas toutes les erreurs internes ; la latence exclut la fin des téléchargements. Le stockage et les
            sauvegardes ont leurs propres indicateurs.
          </p>
        </>
      )}
    </div>
  );
}
