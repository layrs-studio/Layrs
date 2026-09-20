import { useEffect, useRef, useState } from "react";
import { NativeCompareSurface } from "./native-compare";

type Command = { action: string; [key: string]: unknown };
type Step = { stepId: string; parentStepIds: string[]; rootTreeId: string };
type Filter = {actorId:string|null;since:string|null;until:string|null};
type Observation = {observation:{observationId:string;actorId:string|null;observedAt:string;captureReason:string};provenance:"server_admission"|"client_assertion";principalId:string;receivedAt:string};
type Page = { anchor: string; steps: Step[]; next: string | null; path?: string | null; filter:Filter; observations:Record<string,Observation[]> };
export function NativeHistorySurface({ scope, transport, busy, onBusy }: {
  scope: string; transport: <T>(command: Command) => Promise<T>; busy: boolean; onBusy: (busy: boolean) => void;
}) {
  const [anchor, setAnchor] = useState("");
  const [path, setPath] = useState("");
  const [actor,setActor]=useState("");
  const [since,setSince]=useState("");
  const [until,setUntil]=useState("");
  const [page, setPage] = useState<Page | null>(null);
  const [error, setError] = useState("");
  const [comparison, setComparison] = useState<{ before: string; after: string } | null>(null);
  const generation = useRef(0);
  const inFlight = useRef(false);
  useEffect(() => {
    generation.current++; inFlight.current = false; setPage(null); setError(""); setAnchor(""); setPath(""); setActor(""); setSince(""); setUntil(""); setComparison(null);
    return () => { generation.current++; };
  }, [scope]);
  async function load(next = false) {
    if (busy || inFlight.current) return;
    const current = generation.current;
    inFlight.current = true; onBusy(true); setError(""); setComparison(null);
    try {
      const result = await transport<Page>({action:"stepHistory",anchor:next ? page?.anchor : anchor,cursor:next ? page?.next : null,limit:30,path:next ? page?.path ?? null : path || null,filter:next ? page?.filter : {actorId:actor||null,since:since||null,until:until||null}});
      if (generation.current === current) setPage(result);
    } catch (e) { if (generation.current === current) { setError(String(e)); setPage(null); } }
    finally { if (generation.current === current) { inFlight.current = false; onBusy(false); } }
  }
  return <section aria-label="Historique publié"><h2>Historique publié</h2>
    <div className="native-open"><label>Capture publiée à explorer<input value={anchor} disabled={busy} onChange={e => setAnchor(e.target.value)} placeholder="blake3:…" /></label><button disabled={busy || !anchor} onClick={() => void load()}>Lire l’historique publié</button></div>
    <label>Chemin exact publié, facultatif<input value={path} disabled={busy} maxLength={256} onChange={e => setPath(e.target.value)} placeholder="src/index.ts" /></label>
    <details><summary>Filtrer les observations reçues sur le serveur</summary>
      <label>Acteur de publication, facultatif<input value={actor} disabled={busy} onChange={e=>setActor(e.target.value)} placeholder="actor:…" /></label>
      <label>Reçue depuis, facultatif<input value={since} disabled={busy} onChange={e=>setSince(e.target.value)} placeholder="2026-09-01T00:00:00Z" /></label>
      <label>Reçue jusqu’au, facultatif<input value={until} disabled={busy} onChange={e=>setUntil(e.target.value)} placeholder="2026-09-30T23:59:59Z" /></label>
      <p>Dates au format RFC3339, fuseau inclus. Les filtres s’appliquent à la même observation de réception.</p>
    </details>
    {page && <><p>Parcours ancré sur <code>{page.anchor}</code>. Les publications suivantes apparaissent dans un nouveau parcours. Les captures n’ont pas de date ou d’auteur intrinsèques.</p>
      <p>Les observations distinguent la première réception d’une capture et les déclarations des comptes authentifiés. Les filtres de dates portent sur la réception serveur ; une date déclarée par un client n’est pas une date de création vérifiée.</p>
      {page.path && <p>Changements de <code>{page.path}</code> par rapport à au moins un parent, sans suivi automatique des renommages.</p>}
      <ol className="native-history">{page.steps.map(step => <li key={step.stepId}><div><code>{step.stepId}</code><small>{step.parentStepIds.length} parent(s)</small>
        {(page.observations[step.stepId]||[]).map(value=><p key={value.observation.observationId}>{value.provenance === "client_assertion" ? <>Déclaration du client · date déclarée : <time dateTime={value.observation.observedAt}>{new Date(value.observation.observedAt).toLocaleString("fr-FR")}</time> · motif <code>{value.observation.captureReason}</code> · </> : "Première réception serveur · "}Reçue le <time dateTime={value.receivedAt}>{new Date(value.receivedAt).toLocaleString("fr-FR")}</time> · acteur <code>{value.observation.actorId}</code> · compte <code>{value.principalId}</code></p>)}
        {!page.observations[step.stepId]?.length && <small>Réception non documentée pour cette capture.</small>}
        {step.parentStepIds.map(parent => <p key={parent}><code>{parent}</code><button disabled={busy} onClick={() => setComparison({before:parent,after:step.stepId})}>Comparer avec ce parent</button></p>)}
      </div></li>)}</ol><button disabled={busy || !page.next} onClick={() => void load(true)}>Page suivante de l’historique publié</button></>}
    {comparison && <NativeCompareSurface key={`${scope}:${comparison.before}:${comparison.after}`} transport={transport} busy={busy} onBusy={onBusy} initialBefore={comparison.before} after={comparison.after} />}
    {error && <p role="alert" className="native-error">{error}</p>}
  </section>;
}
