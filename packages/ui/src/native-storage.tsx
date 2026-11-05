import { useEffect, useRef, useState } from "react";

type Report = {
  severity: string; databaseBytes: number;
  globalUsage: { chargedBytes: number; objects: number; cataloguedBytes: number; reservedOnlyBytes: number };
  casVolume: { totalBytes: number; availableBytes: number; freeBytes: number };
  readings: { resource: string; unit: string; used: number; warningAt: number | null; criticalAt: number | null; monitored: boolean; severity: string }[];
};
const resources: Record<string, string> = { global_cas_bytes: "CAS logique global", global_cas_objects: "Objets CAS globaux", space_cas_bytes: "CAS logique du projet", space_cas_objects: "Objets du projet", cas_volume_used: "Occupation du volume CAS", database_bytes: "Base PostgreSQL" };
const severity = (value: string) => value === "critical" ? "Critique" : value === "warning" ? "Alerte" : "Normal";
const bytes = (value: number) => `${(value / 1024 / 1024).toFixed(1)} MiB`;
export function NativeStoragePanel({ transport, disabled }: { transport: <T>(command: { action: string; [key: string]: unknown }) => Promise<T>; disabled: boolean }) {
  const [report, setReport] = useState<Report | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const alive = useRef(true);
  const running = useRef(false);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  async function load() {
    if (running.current) return;
    running.current = true; setLoading(true); setError(""); setReport(null);
    try { const result = await transport<Report>({ action: "storageBudget", space: null }); if (alive.current) setReport(result); }
    catch (e) { if (alive.current) setError(String(e)); }
    finally { running.current = false; if (alive.current) setLoading(false); }
  }
  return <details><summary>Stockage de l’instance</summary>
    <p>Réservé à l’administrateur de l’instance. Les budgets logiques incluent les réservations ; la capacité du disque et PostgreSQL sont mesurés séparément.</p>
    <button disabled={disabled || loading} onClick={() => void load()}>Vérifier le stockage</button>
    {loading && <p role="status">Lecture du stockage…</p>}
    {error && <p role="alert" className="native-error">{error}</p>}
    {report && <section aria-label="État du stockage"><p>État : {severity(report.severity)}</p>
      <p>CAS logique : {bytes(report.globalUsage.chargedBytes)}, {report.globalUsage.objects} objets. Réservations seules : {bytes(report.globalUsage.reservedOnlyBytes)}.</p>
      <p>Volume CAS : {bytes(report.casVolume.availableBytes)} disponibles sur {bytes(report.casVolume.totalBytes)}. PostgreSQL : {bytes(report.databaseBytes)}.</p>
      <ul>{report.readings.map(reading => <li key={reading.resource}>{resources[reading.resource] || reading.resource} : {reading.unit === "bytes" ? bytes(reading.used) : reading.used} — {reading.monitored ? severity(reading.severity) : "Seuil non configuré"}</li>)}</ul>
      <p>Cette observation ne supprime aucun objet. Les quotas CAS ne réservent pas physiquement l’espace disque.</p>
    </section>}
  </details>;
}
