import { useEffect, useRef, useState } from "react";
import { nativeWorkspace } from "./tauri";

type Report = {
  clean: boolean; complete: boolean; checkedObjects: number; checkedRows: number;
  reachableObjects: number; unreachableObjects: number; reconstructibleProjections: number;
  findings: { code: string; severity: string; path: string | null; message: string }[];
};

export function NativeDiagnosticsPanel({ root, disabled }: { root: string; disabled: boolean }) {
  const [report, setReport] = useState<Report | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const generation = useRef(0);
  const inFlight = useRef(false);
  useEffect(() => {
    generation.current++; inFlight.current = false;
    setReport(null); setError(""); setLoading(false);
    return () => { generation.current++; };
  }, [root]);
  async function diagnose() {
    if (inFlight.current) return;
    const current = generation.current;
    inFlight.current = true; setLoading(true); setError(""); setReport(null);
    try {
      const result = await nativeWorkspace<Report>(root, { action: "diagnose" });
      if (generation.current === current) setReport(result);
    } catch (e) { if (generation.current === current) setError(String(e)); }
    finally { if (generation.current === current) { inFlight.current = false; setLoading(false); } }
  }
  return <details aria-label="Diagnostic du projet"><summary>Diagnostic du projet</summary>
    <p>Vérification en lecture seule du dossier sélectionné, disponible même si le projet ne s’ouvre plus. Les fichiers et preuves sont conservés.</p>
    <button disabled={disabled || loading || !root} onClick={() => void diagnose()}>Vérifier l’intégrité</button>
    {loading && <p role="status">Vérification en cours…</p>}
    {error && <p className="native-error" role="alert">{error}</p>}
    {report && <section aria-label="Résultat du diagnostic">
      <p role="status">{!report.complete ? "Vérification incomplète : limite de diagnostic atteinte." : report.clean ? "Aucune erreur d’intégrité détectée." : "Des erreurs d’intégrité nécessitent une intervention."}</p>
      <p>{report.checkedObjects} objets et {report.checkedRows} lignes vérifiés. {report.reachableObjects} objets accessibles ; {report.unreachableObjects} objets sans référence ; {report.reconstructibleProjections} projections reconstructibles.</p>
      <ul>{report.findings.map((finding, index) => <li key={index}><strong>{finding.severity} · {finding.code}</strong> {finding.message}{finding.path && <code>{finding.path}</code>}</li>)}</ul>
    </section>}
  </details>;
}
