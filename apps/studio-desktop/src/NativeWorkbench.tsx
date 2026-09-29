import { useEffect, useRef, useState } from "react";
import { nativeWorkspace, nativeCheckoutKind, selectFolder } from "./tauri";
import { NativePartialWorkbench } from "./NativePartialWorkbench";
import "./native-workbench.css";
import { NativeRemotePanel } from "./NativeRemotePanel";
import { NativeMergePanel } from "./NativeMergePanel";
import { NativeHistoryPanel } from "./NativeHistoryPanel";
import { NativeDiagnosticsPanel } from "./NativeDiagnosticsPanel";
import { NativeIdentityPanel } from "./NativeIdentityPanel";
import { NativeRewriteImpact, type RewriteSummary } from "./NativeRewriteImpact";

type Request = { action: string; [key: string]: unknown };
type Change = { path: string; status: string; binary: boolean; textOmitted: boolean; textDiff: { op: string; text: string }[] | null; beforeBytes: number | null; afterBytes: number | null };
type Operation = { operationId: string; summary: string; kind: string; createdAt: string };
type Point = { markerId?: string; releaseId?: string; name?: string; version?: string; targetStepId: string; notes: string };
type Snapshot = {
  state: { space: { name: string; spaceId: string }; worktree: { worktreeId: string; checkoutRoot: string }; operation: Operation;
    layers: { layerId: string; name: string; settings: { automaticCapture: boolean; automaticCaptureIdleMs: number } }[];
    view: { worktrees: { worktreeId: string; activeLayerId: string | null; status: string }[] } };
  needsRepair: boolean; changes: Change[] | null; operations: Operation[]; markers: Point[]; releases: Point[];
};
type Preview = { expected?: unknown; collisions?: {path:string;reason:string}[]; changes?: Change[]; layerHeads?: unknown[]; markerIds?: string[]; releaseIds?: string[]; rewrite?: RewriteSummary };

export function NativeWorkbench() {
  const [partialRoot,setPartialRoot]=useState("");
  const [root, setRoot] = useState(() => localStorage.getItem("layrs.native.checkout") || "");
  const [openedRoot, setOpenedRoot] = useState("");
  const [name, setName] = useState("");
  const [data, setData] = useState<Snapshot | null>(null);
  const [busy, setBusy] = useState(false);
  const running = useRef(false);
  const foregroundRevision = useRef(0);
  function setWorking(value: boolean) {
    foregroundRevision.current++;
    running.current = value;
    setBusy(value);
  }
  const [error, setError] = useState("");
  const [automatic, setAutomatic] = useState(true);
  const idle = useRef({ signature: "", since: 0 });
  const [newLayer, setNewLayer] = useState("");
  const [pointName, setPointName] = useState("");
  const [hiddenMarker,setHiddenMarker] = useState("");
  const [notes, setNotes] = useState("");
  const [step, setStep] = useState("");
  const [path, setPath] = useState("");
  const [rewriteKind, setRewriteKind] = useState("replay");
  const [rewriteSource, setRewriteSource] = useState("");
  const [mainline, setMainline] = useState("");
  const [preview, setPreview] = useState<{ data: Preview; request: Request } | null>(null);
  const activeId = data?.state.view.worktrees.find(w => w.worktreeId === data.state.worktree.worktreeId)?.activeLayerId;
  const conflicted = data?.state.view.worktrees.find(w => w.worktreeId === data.state.worktree.worktreeId)?.status === "conflicted";
  const activeLayer = data?.state.layers.find(l => l.layerId === activeId);

  async function run(request: Request, selectedRoot = openedRoot || root) {
    if (running.current) return;
    setWorking(true); setError("");
    try {
      if(request.action==="status" && await nativeCheckoutKind(selectedRoot)==="projection"){
        setData(null);setOpenedRoot("");setPreview(null);setPartialRoot(selectedRoot);localStorage.setItem("layrs.native.checkout",selectedRoot);return;
      }
      const next = await nativeWorkspace<Snapshot>(selectedRoot, request);
      setData(next); setOpenedRoot(next.state.worktree.checkoutRoot); setPreview(null);
      localStorage.setItem("layrs.native.checkout", next.state.worktree.checkoutRoot);
      idle.current = { signature: "", since: 0 };
    } catch (e) { setError(String(e)); }
    finally { setWorking(false); }
  }

  async function inspect(request: Request, apply: Request) {
    if (running.current) return;
    setWorking(true); setError("");
    try {
      const next = await nativeWorkspace<Preview>(openedRoot, request);
      setPreview({ data: next, request: next.rewrite ? { ...apply, expectedOperation: next.rewrite.expectedOperationId, expectedStep: next.rewrite.proposedStep.stepId } : next.expected ? { ...apply, action: apply.action === "restore" ? "applyRestore" : "applyView", expected: next.expected } : apply });
    }
    catch (e) { setError(String(e)); }
    finally { setWorking(false); }
  }

  useEffect(() => {
    if (!openedRoot) return;
    let disposed = false;
    let polling = false;
    const timer = window.setInterval(async () => {
      if (polling || running.current || document.visibilityState !== "visible" || preview) return;
      polling = true;
      const revision = foregroundRevision.current;
      let capturing = false;
      try {
        let next = await nativeWorkspace<Snapshot>(openedRoot, { action: "status" });
        if (disposed || revision !== foregroundRevision.current || running.current) return;
        const signature = JSON.stringify(next.changes);
        const now = Date.now();
        const layerId = next.state.view.worktrees.find(w => w.worktreeId === next.state.worktree.worktreeId)?.activeLayerId;
        const settings = next.state.layers.find(l => l.layerId === layerId)?.settings;
        if (signature !== idle.current.signature) idle.current = { signature, since: now };
        else if (next.state.view.worktrees.find(w => w.worktreeId === next.state.worktree.worktreeId)?.status !== "conflicted" && automatic && settings?.automaticCapture && next.changes?.length && !next.needsRepair && now - idle.current.since >= Math.max(500, settings.automaticCaptureIdleMs)) {
          capturing = true; setWorking(true);
          next = await nativeWorkspace<Snapshot>(openedRoot, { action: "captureAutomatic" });
          idle.current = { signature: "", since: 0 };
        }
        if (!disposed) setData(next);
      } catch (e) { if (!disposed && (capturing || revision === foregroundRevision.current)) setError(String(e)); }
      finally { polling = false; if (capturing) setWorking(false); }
    }, 1500);
    return () => { disposed = true; clearInterval(timer); };
  }, [openedRoot, automatic, preview]);

  function restorePoint(target: string) {
    setStep(target); setPath("");
    void inspect({ action: "previewRestore", step: target, path: null }, { action: "restore", step: target, path: null });
  }

  if(partialRoot)return <NativePartialWorkbench root={partialRoot} onClose={()=>{setPartialRoot("");setOpenedRoot("");setData(null);setError("");}}/>;
  return <main className="native-workbench">
    <header><div><p className="native-eyebrow">LAYRS · PROJET LOCAL</p><h1>{data?.state.space.name || "Votre projet, son histoire"}</h1></div></header>
    <section className="native-open" aria-label="Ouvrir un projet">
      <label>Dossier<input value={root} onChange={e => setRoot(e.target.value)} disabled={busy} placeholder="Chemin du projet" /></label>
      <button disabled={busy} onClick={() => void selectFolder(root).then(p => { if (p) setRoot(p); }).catch(e => setError(String(e)))}>Parcourir…</button>
      <button disabled={busy || !root} onClick={() => void run({ action: "status" }, root)}>Ouvrir</button>
      <label>Nom du nouveau projet<input value={name} onChange={e => setName(e.target.value)} disabled={busy} /></label>
      <button disabled={busy || !root || !name} onClick={() => void run({ action: "initialize", name }, root)}>Initialiser</button>
    </section>
    <NativeDiagnosticsPanel root={root} disabled={busy} />
    {error && <p className="native-error" role="alert">{error}</p>}
    {busy && <p role="status">Opération en cours…</p>}
    <NativeRemotePanel root={openedRoot || root} localSpaceId={data?.state.space.spaceId} opened={!!data} busy={busy} onBusy={setWorking} onChanged={next => void run({ action: "status" }, next)} />
    {data && <>
      <NativeIdentityPanel key={openedRoot} root={openedRoot} disabled={busy || data.needsRepair} />
      <section className="native-toolbar" aria-label="Actions du projet">
        <label>Layer<select value={activeId || ""} disabled={busy || data.needsRepair} onChange={e => void run({ action: "switchLayer", layer: e.target.value })}>{data.state.layers.map(l => <option key={l.layerId} value={l.layerId}>{l.name}</option>)}</select></label>
        <button disabled={busy || data.needsRepair || conflicted} onClick={() => void run({ action: "capture" })}>Capturer</button>
        <button disabled={busy || data.needsRepair || data.operations.length < 2} onClick={() => void run({ action: "undo" })}>Annuler</button>
        <button disabled={busy || data.needsRepair || data.state.operation.kind !== "undo"} onClick={() => void run({ action: "redo", operation: null })}>Rétablir</button>
        <label className="native-check"><input type="checkbox" checked={automatic} onChange={e => setAutomatic(e.target.checked)} />Capture automatique</label>
        <span>{automatic && activeLayer?.settings.automaticCapture ? "Après une période de calme, pendant l’ouverture de ce projet." : "Les changements de Layer et restaurations capturent toujours le travail courant."}</span>
      </section>
      {conflicted && <p role="status">Cette fusion contient des conflits. Les fichiers affichés correspondent encore à la projection précédente. Ouvrez « Afficher les conflits » pour choisir les variantes.</p>}
      {data.needsRepair && <section className="native-error"><p>Une écriture a été interrompue. Les sauvegardes sont conservées ; reprenez l’opération avant de continuer.</p><button disabled={busy} onClick={() => void run({ action: "recover" })}>Reprendre après interruption</button></section>}
      <div className="native-columns"><section><h2>Modifications présentes <small>{data.changes?.length || 0}</small></h2><Changes changes={data.changes || []} />
        <h2>Restaurer un fichier ou le projet</h2><label>Step<input value={step} onChange={e => setStep(e.target.value)} placeholder="Step ou point nommé sélectionné ci-dessous" /></label><label>Chemin facultatif<input value={path} onChange={e => setPath(e.target.value)} placeholder="Vide pour le projet entier" /></label><button disabled={busy || !step || data.needsRepair} onClick={() => void inspect({ action: "previewRestore", step, path: path || null }, { action: "restore", step, path: path || null })}>Prévisualiser</button>
        {preview && <section className="native-preview" aria-label="Prévisualisation de restauration"><h2>{preview.data.rewrite ? "Effet de la transformation" : "Effet de la restauration"}</h2>{preview.data.changes && <Changes changes={preview.data.changes} />}{Boolean(preview.data.collisions?.length) && <div role="alert"><p>Restauration bloquée par des collisions :</p><ul>{preview.data.collisions?.map(c => <li key={c.path}>{c.path} : {c.reason}</li>)}</ul></div>}{preview.data.rewrite && <NativeRewriteImpact rewrite={preview.data.rewrite} layers={data.state.layers} points={[...data.markers, ...data.releases]} />}{preview.data.layerHeads && <p>La visibilité des Layers, markers et releases sera aussi restaurée : {preview.data.layerHeads.length} Layers, {preview.data.markerIds?.length || 0} markers, {preview.data.releaseIds?.length || 0} releases.</p>}<p>Le travail présent sera capturé et les fichiers remplacés seront sauvegardés.</p><button disabled={busy || Boolean(preview.data.collisions?.length)} onClick={() => void run(preview.request)}>{preview.data.rewrite ? "Appliquer la transformation" : "Appliquer la restauration"}</button><button onClick={() => setPreview(null)}>Fermer l’aperçu</button></section>}
        <h2>Historique des opérations</h2><ol className="native-history">{data.operations.map(op => <li key={op.operationId}><div><strong>{op.summary}</strong><time>{op.createdAt}</time><code>{op.operationId}</code></div><button disabled={busy || data.needsRepair} onClick={() => void inspect({ action: "previewView", operation: op.operationId }, { action: "restoreView", operation: op.operationId })}>Prévisualiser cet état</button></li>)}</ol><p>Les 200 opérations les plus proches sont affichées ; la CLI permet d’en demander davantage.</p>
        <NativeHistoryPanel root={openedRoot} disabled={busy || data.needsRepair} onStep={restorePoint} onOperation={operation => void inspect({ action: "previewView", operation }, { action: "restoreView", operation })} />
      </section><aside><h2>Créer un Layer</h2><label>Nom<input value={newLayer} onChange={e => setNewLayer(e.target.value)} /></label><button disabled={busy || !newLayer || data.needsRepair} onClick={() => void run({ action: "createLayer", name: newLayer })}>Créer depuis le travail courant</button>
        <NativeMergePanel key={`${openedRoot}:${activeId}`} root={openedRoot} layer={activeId || ""} disabled={busy || data.needsRepair} onBusy={setWorking} onChanged={() => { idle.current = { signature: "", since: 0 }; }} />
        <h2>Transformer des changements</h2><label>Action<select value={rewriteKind} disabled={busy} onChange={e => setRewriteKind(e.target.value)}><option value="replay">Rejouer un changement</option><option value="revert">Inverser un changement</option><option value="squash">Regrouper jusqu’à la capture courante</option></select></label><label>Capture ou référence<input value={rewriteSource} onChange={e => setRewriteSource(e.target.value)} placeholder="blake3:… ou @marker:version" /></label>{rewriteKind !== "squash" && <label>Parent de référence si fusion<input value={mainline} onChange={e => setMainline(e.target.value)} placeholder="Obligatoire pour une capture à plusieurs parents" /></label>}<button disabled={busy || !rewriteSource || data.needsRepair} onClick={() => { const params = { kind: rewriteKind, source: rewriteSource, mainline: rewriteKind === "squash" ? null : mainline || null }; void inspect({ action: "previewRewrite", ...params }, { action: "applyRewrite", ...params }); }}>Prévisualiser la transformation</button>
        <h2>Points nommés</h2><label>Nom ou version<input value={pointName} onChange={e => setPointName(e.target.value)} /></label><label>Notes<textarea value={notes} onChange={e => setNotes(e.target.value)} /></label><div className="native-buttons"><button disabled={busy || !pointName || data.needsRepair} onClick={() => void run({ action: "createMarker", name: pointName, notes })}>Créer un marker</button><button disabled={busy || !pointName || data.needsRepair} onClick={() => void run({ action: "createRelease", version: pointName, notes })}>Créer une release</button></div>
        <details><summary>Rétablir un marker masqué</summary><label>Identifiant du marker<input value={hiddenMarker} onChange={e => setHiddenMarker(e.target.value)} placeholder="marker:…" /></label><button disabled={busy || !hiddenMarker || data.needsRepair} onClick={() => void run({action:"showMarker",id:hiddenMarker})}>Rétablir ce marker</button><p>Son identité et sa capture restent identiques. Le travail courant est conservé.</p></details>
        {[...data.markers, ...data.releases].map(p => <article key={p.markerId || p.releaseId}><h3>{p.name || p.version}</h3><code>{p.markerId || p.releaseId}</code><p>{p.notes}</p><code>{p.targetStepId}</code><div className="native-buttons"><button disabled={busy || data.needsRepair} onClick={() => restorePoint(p.targetStepId)}>Prévisualiser</button>{p.markerId && <button disabled={busy || data.needsRepair} onClick={() => void run({ action: "hideMarker", id: p.markerId })}>Masquer</button>}</div></article>)}
      </aside></div>
    </>}
  </main>;
}

function Changes({ changes }: { changes: Change[] }) {
  if (!changes.length) return <p className="native-muted">Aucune modification de fichier.</p>;
  return <div className="native-changes">{changes.map(c => <details key={c.path}><summary><span>{c.status === "added" ? "+" : c.status === "deleted" ? "−" : "~"}</span> {c.path} <small>{c.beforeBytes ?? "absent"} → {c.afterBytes ?? "absent"} octets</small></summary>{c.binary ? <p>Contenu binaire modifié.</p> : c.textOmitted ? <p>Fichier trop volumineux pour cet aperçu.</p> : <pre>{c.textDiff?.map((line, i) => <span key={i} data-op={line.op}>{line.op === "insert" ? "+" : line.op === "delete" ? "−" : " "}{line.text}{!line.text.endsWith("\n") && "\n[Sans retour final]\n"}</span>)}</pre>}</details>)}</div>;
}
