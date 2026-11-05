import { useState } from "react";
type Change = { path: string; status: string; binary: boolean; textOmitted: boolean; beforeBytes: number | null; afterBytes: number | null; textDiff: { op: string; text: string }[] | null };
export function NativeCompareSurface({ transport, busy, onBusy, after = "", initialBefore = "" }: { transport: <T>(command: { action: string; [key: string]: unknown }) => Promise<T>; busy: boolean; onBusy: (busy: boolean) => void; after?: string; initialBefore?: string }) {
  const [before, setBefore] = useState(initialBefore);
  const [target, setTarget] = useState(after);
  const [changes, setChanges] = useState<Change[] | null>(null);
  const [error, setError] = useState("");
  async function compare() {
    if (busy) return;
    onBusy(true); setError(""); setChanges(null);
    try { setChanges((await transport<{ changes: Change[] }>({ action: "compare", before, after: target })).changes); }
    catch (e) { setError(String(e)); } finally { onBusy(false); }
  }
  return <details><summary>Comparer deux captures publiées</summary><label>Capture de départ<input value={before} onChange={e => { setBefore(e.target.value); setChanges(null); }} placeholder="blake3:…" /></label><label>Capture d’arrivée<input value={target} onChange={e => { setTarget(e.target.value); setChanges(null); }} placeholder="blake3:…" /></label><button disabled={busy || !before || !target} onClick={() => void compare()}>Comparer</button>
    {changes && !changes.length && <p>Les fichiers sont identiques.</p>}
    {changes?.map(change => <details className="native-changes" key={change.path}><summary>{change.status} · {change.path} · {change.beforeBytes ?? "absent"} → {change.afterBytes ?? "absent"} octets</summary>{change.binary ? <p>Contenu binaire.</p> : change.textOmitted ? <p>Contenu trop volumineux pour cet aperçu.</p> : <pre>{change.textDiff?.map((line, i) => <span key={i} data-op={line.op}>{line.op === "insert" ? "+" : line.op === "delete" ? "−" : " "}{line.text}{!line.text.endsWith("\n") && "\n[Sans retour final]\n"}</span>)}</pre>}</details>)}
    {error && <p role="alert" className="native-error">{error}</p>}
  </details>;
}
