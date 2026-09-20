import { useState } from "react";
import { nativeWorkspace } from "./tauri";

type Term = { termId: string; kind: string; provenance: { stepId: string } };
type Comparison = { available: boolean; beforeTerm: string; afterTerm: string; changes: null | { path: string; executableChanged: boolean; textOmitted: boolean; textDiff: null | { op: string; text: string }[] }[] };

export function NativeConflictComparison({ root, layer, conflict, terms, disabled, perform }: {
  root: string; layer: string; conflict: string; terms: Term[]; disabled: boolean;
  perform: (work: () => Promise<void>) => Promise<void>;
}) {
  const [before, setBefore] = useState(terms[0]?.termId || "");
  const [after, setAfter] = useState(terms.at(-1)?.termId || "");
  const [result, setResult] = useState<Comparison | null>(null);
  const options = terms.map(term => <option key={`${term.kind}:${term.termId}`} value={term.termId}>{term.kind} · {term.provenance.stepId} · {term.termId}</option>);
  return <details><summary>Comparer les variantes et la base</summary>
    <label>Variante de départ<select value={before} disabled={disabled} onChange={event => { setBefore(event.target.value); setResult(null); }}>{options}</select></label>
    <label>Variante d’arrivée<select value={after} disabled={disabled} onChange={event => { setAfter(event.target.value); setResult(null); }}>{options}</select></label>
    <button disabled={disabled || !before || !after} onClick={() => void perform(async () => {
      setResult(null);
      setResult(await nativeWorkspace<Comparison>(root, { action: "compareConflictVariants", layer, conflict, before, after }));
    })}>Comparer ces variantes</button>
    {result && <section aria-label="Comparaison des variantes"><p>Aperçu en lecture seule. Aucune résolution enregistrée.</p><code>{result.beforeTerm} → {result.afterTerm}</code>
      {!result.available ? <p>Diff textuel indisponible : contenu binaire, volumineux ou structurel. Les variantes restent sélectionnables avec leurs identifiants exacts.</p> : !result.changes?.length ? <p>Contenu et mode identiques.</p> : result.changes.map(change => <div key={change.path} className="native-changes"><h4>{change.path}</h4>
        {change.executableChanged && <p>Le mode exécutable change.</p>}
        {change.textOmitted ? <p>Le diff dépasse les limites de cet aperçu.</p> : <pre>{change.textDiff?.map((line, index) => <span key={index} data-op={line.op}>{line.op === "insert" ? "+" : line.op === "delete" ? "−" : " "}{line.text}{!line.text.endsWith("\n") && "\n[Sans retour final]\n"}</span>)}</pre>}
      </div>)}
    </section>}
  </details>;
}
