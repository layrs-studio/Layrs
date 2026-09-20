import { useEffect, useRef, useState } from "react";
import { nativeWorkspace } from "./tauri";

type Operation = { operationId: string; summary: string; createdAt: string };
type Step = { stepId: string; parentStepIds: string[] };
type Observation = { observationId: string; observedAt: string; actorId: string | null; captureReason: string };
type Page = { anchor: string; operations?: Operation[]; steps?: Step[]; observations?: Record<string, Observation[]>; next: unknown | null };
export function NativeHistoryPanel({ root, disabled, onOperation, onStep }: { root: string; disabled: boolean; onOperation: (id: string) => void; onStep: (id: string) => void }) {
  const [mode, setMode] = useState("operations");
  const [search, setSearch] = useState("");
  const [actor, setActor] = useState("");
  const [since, setSince] = useState("");
  const [until, setUntil] = useState("");
  const [selector, setSelector] = useState("@layer:main");
  const [path, setPath] = useState("");
  const [page, setPage] = useState<Page | null>(null);
  const [query, setQuery] = useState<{ action: string; [key: string]: unknown } | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const generation = useRef(0);
  const inFlight = useRef(false);
  async function load(next = false) {
    if (inFlight.current) return;
    inFlight.current = true;
    const current = generation.current;
    const filter = { actor: actor || null, since: since || null, until: until || null };
    const selected = next && query ? query : mode === "operations" ? { action: "operationHistory", search, filter, limit: 30 } : { action: "stepHistory", selector, path: path || null, filter, limit: 30 };
    setLoading(true); setError("");
    try { const result = await nativeWorkspace<Page>(root, { ...selected, cursor: next ? page?.next : null }); if (generation.current === current) { setPage(result); setQuery(selected); } }
    catch (e) { if (generation.current === current) { setError(String(e)); setPage(null); } }
    finally { if (generation.current === current) { setLoading(false); inFlight.current = false; } }
  }
  useEffect(() => { generation.current++; inFlight.current = false; setLoading(false); setError(""); setPage(null); setQuery(null); return () => { generation.current++; }; }, [root]);
  return <section aria-label="Explorer l’historique"><h2>Explorer l’historique</h2>
    <div className="native-open"><label>Afficher<select value={mode} disabled={loading} onChange={e => setMode(e.target.value)}><option value="operations">Opérations locales</option><option value="steps">Graphe des captures</option></select></label>
      {mode === "operations" ? <label>Résumé, identifiant ou acteur<input value={search} disabled={loading} onChange={e => setSearch(e.target.value)} /></label> : <><label>Capture ou référence<input value={selector} disabled={loading} onChange={e => setSelector(e.target.value)} /></label><label>Chemin exact, facultatif<input value={path} disabled={loading} onChange={e => setPath(e.target.value)} /></label></>}
      <button disabled={loading || disabled} onClick={() => void load()}>Rechercher / actualiser</button>
    </div>
    <div className="native-open">
      <label>Acteur exact, facultatif<input value={actor} disabled={loading} placeholder="actor:…" onChange={e => setActor(e.target.value)} /></label>
      <label>Depuis, inclus<input value={since} disabled={loading} placeholder="2026-09-20T00:00:00+02:00" onChange={e => setSince(e.target.value)} /></label>
      <label>Jusqu’à, inclus<input value={until} disabled={loading} placeholder="2026-09-21T00:00:00+02:00" onChange={e => setUntil(e.target.value)} /></label>
    </div>
    {page && <><p className="native-muted">Historique ancré sur un état fixe. Les nouvelles captures apparaîtront après actualisation. Les renommages ne sont pas déduits.</p>
      <ol className="native-history">{page.operations?.map(op => <li key={op.operationId}><div><strong>{op.summary}</strong><time>{op.createdAt}</time><code>{op.operationId}</code></div><button disabled={disabled || loading} onClick={() => onOperation(op.operationId)}>Prévisualiser cet état</button></li>)}</ol>
      {page.steps && <StepRows steps={page.steps} observations={page.observations || {}} disabled={disabled || loading} onStep={onStep} />}
      {page.steps && <p className="native-muted">Dates et acteurs proviennent des observations locales. Ils ne constituent pas une preuve d’auteur de la capture.</p>}
      <button disabled={loading || disabled || !page.next} onClick={() => void load(true)}>Page suivante</button></>}
    {loading && <p role="status">Lecture de l’historique…</p>}{error && <p className="native-error" role="alert">{error}</p>}
  </section>;
}

function StepRows({ steps, observations, disabled, onStep }: { steps: Step[]; observations: Record<string, Observation[]>; disabled: boolean; onStep: (id: string) => void }) {
  const groups: Step[][] = [];
  const automatic = (step: Step) => observations[step.stepId]?.some(o => o.captureReason === "automatic_idle") === true;
  for (const step of steps) {
    const previous = groups.at(-1);
    if (automatic(step) && previous && automatic(previous[0])) previous.push(step);
    else groups.push([step]);
  }
  const row = (step: Step) => <li key={step.stepId}><div>
    <code>{step.stepId}</code><small>{step.parentStepIds.length} parent(s)</small>
    {step.parentStepIds.map(id => <code key={id}>{id}</code>)}
    {(observations[step.stepId] || []).map(o => <p key={o.observationId}><time>{o.observedAt}</time> · {o.actorId || "Acteur non renseigné"}{o.captureReason === "automatic_idle" && " · Capture automatique"}</p>)}
  </div><button disabled={disabled} onClick={() => onStep(step.stepId)}>Prévisualiser la capture</button></li>;
  return <>{groups.map(group => group.length > 1 ? <details key={group[0].stepId}><summary>{group.length} captures automatiques successives</summary><ol className="native-history">{group.map(row)}</ol></details> : <ol key={group[0].stepId} className="native-history">{row(group[0])}</ol>)}</>;
}
