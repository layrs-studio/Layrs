import { useEffect, useRef, useState } from "react";
import type { NativeAdminCommand } from "./native-admin-route";
import { nativeMetrics, type NativeMetrics } from "./native-metrics-model";

export function NativeMetricsPanel({ transport, disabled }: {
  transport: <T>(command: NativeAdminCommand) => Promise<T>; disabled: boolean;
}) {
  const [report, setReport] = useState<NativeMetrics | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const alive = useRef(true), running = useRef(false);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  async function load() {
    if (disabled || running.current) return;
    running.current = true; setLoading(true); setError(""); setReport(null);
    try {
      const result = nativeMetrics(await transport<unknown>({ action: "metrics" }));
      if (alive.current) setReport(result);
    } catch {
      if (alive.current) { setReport(null); setError("Lecture des métriques impossible. Vérifiez votre connexion et vos droits d’administrateur, puis réessayez."); }
    } finally { running.current = false; if (alive.current) setLoading(false); }
  }
  const average = report && report.http.completed > 0 ? (report.http.headersLatencyMicrosecondsTotal / report.http.completed / 1000).toFixed(2) : null;
  return <details><summary>Activité du serveur</summary>
    <p>Lecture réservée à l’administrateur de l’instance. Les compteurs couvrent le processus courant et repartent de zéro à son redémarrage.</p>
    <button type="button" disabled={disabled || loading} onClick={() => void load()}>Lire les métriques</button>
    {loading && <p role="status">Lecture des métriques…</p>}
    {error && <p role="alert" className="native-error">{error}</p>}
    {report && <section aria-label="Métriques du serveur">
      <p>Observation du {new Date(report.observedUnixMs).toLocaleString()} · processus actif depuis {report.uptimeSeconds} secondes.</p>
      <p>{report.http.completed} réponses terminées · {report.http.inFlight} requêtes en cours · {report.http.interruptedBeforeHeaders} interruptions avant les headers.</p>
      <table><caption>Réponses HTTP</caption><thead><tr><th>Classe</th><th>Nombre</th></tr></thead><tbody>{report.http.statusClasses.map(row => <tr key={row.class}><td>{row.class}{row.class === "5xx" ? " — erreurs serveur" : row.class === "4xx" ? " — requêtes refusées" : ""}</td><td>{row.count}</td></tr>)}</tbody></table>
      <p>Latence moyenne jusqu’aux headers : {average === null ? "aucune réponse mesurée" : `${average} ms`}.</p>
      <table><caption>Latences cumulatives jusqu’aux headers</caption><thead><tr><th>Durée maximale</th><th>Réponses</th></tr></thead><tbody>{report.http.headersLatencyBuckets.map(row => <tr key={row.lessOrEqualMs}><td>≤ {row.lessOrEqualMs} ms</td><td>{row.count}</td></tr>)}</tbody></table>
      <p>Sondes de disponibilité : {report.readiness.checks} · échecs PostgreSQL : {report.readiness.databaseFailures} · échecs CAS : {report.readiness.casFailures}.</p>
      <p>Observation ponctuelle et approximative, sans historique. Les sondes ne recensent pas toutes les erreurs internes ; la latence exclut la fin des téléchargements. Les interruptions après les headers ne sont pas comptées. Le stockage et les sauvegardes ont leurs propres indicateurs.</p>
    </section>}
  </details>;
}
