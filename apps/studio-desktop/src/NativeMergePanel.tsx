import { useRef, useState } from "react";
import { nativeWorkspace } from "./tauri";
import { NativeConflictComparison } from "./NativeConflictComparison";
import { NativeConflictBlocks, type BlockDraft, type ConflictBlock } from "./NativeConflictBlocks";

type Request = { action: string; [key: string]: unknown };
type Preview = { expectedOperation: string; expectedStep: string; conflicts: unknown[] };
type Term = { termId: string; provenance: { stepId: string }; values: unknown[] };
type Conflict = { conflictId: string; document: { placements: { path: string }[]; sides: Term[]; bases?: Term[]; blocks?: ConflictBlock[]; segments?: {kind:string;blockId?:string}[] } };
export function NativeMergePanel({ root, layer, disabled, onBusy, onChanged }: { root: string; layer: string; disabled: boolean; onBusy: (busy: boolean) => void; onChanged: () => void }) {
  const [source, setSource] = useState("");
  const [conflicts, setConflicts] = useState<Conflict[]>([]);
  const [preview, setPreview] = useState<{ value: Preview; apply: Request } | null>(null);
  const [error, setError] = useState("");
  const inFlight = useRef(false);
  const blockDrafts = useRef(new Map<string,BlockDraft>());
  async function perform(work: () => Promise<void>) {
    if (disabled || inFlight.current) return;
    inFlight.current = true;
    onBusy(true); setError("");
    try { await work(); } catch (e) { setError(String(e)); }
    finally { inFlight.current = false; onBusy(false); }
  }
  async function inspect(request: Request, apply: Request) {
    const value = await nativeWorkspace<Preview>(root, request);
    setPreview({ value, apply: { ...apply, expectedOperation: value.expectedOperation, expectedStep: value.expectedStep } });
  }
  return <section aria-label="Fusion et résolution"><h2>Fusionner et résoudre</h2>
    <p>Les changements du Layer source seront réunis dans le Layer actif. Pour réunir plusieurs têtes du même Layer, indiquez son nom.</p>
    <label>Source de la fusion<input disabled={disabled} value={source} onChange={e => { setSource(e.target.value); setPreview(null); }} placeholder="Nom du Layer ou @marker:version" /></label>
    <button disabled={disabled || !source} onClick={() => void perform(() => inspect({ action: "previewWeave", source, target: layer }, { action: "applyWeave", source, target: layer }))}>Prévisualiser la fusion</button>
    <button disabled={disabled} onClick={() => void perform(async () => { setConflicts((await nativeWorkspace<{ conflicts: Conflict[] }>(root, { action: "conflicts", layer })).conflicts); setPreview(null); })}>Afficher les conflits</button>
    {conflicts.map(conflict => <ConflictEditor key={conflict.conflictId} conflict={conflict} root={root} layer={layer} disabled={disabled} perform={perform} inspect={inspect} invalidate={() => setPreview(null)} blockDrafts={blockDrafts.current} />)}
    {preview && <section className="native-preview" aria-label="Aperçu de fusion"><p>{preview.value.conflicts.length} conflits resteront après cette opération.</p><p>{preview.value.conflicts.length ? "Les variantes seront conservées. Les fichiers actuels restent en place jusqu’à la résolution complète." : "Les fichiers résultants seront installés ; l’état précédent reste dans l’historique."}</p><code>{preview.value.expectedStep}</code><button disabled={disabled} onClick={() => void perform(async () => { await nativeWorkspace(root, preview.apply); setPreview(null); setConflicts([]); onChanged(); })}>Appliquer cet aperçu</button><button onClick={() => setPreview(null)}>Fermer</button></section>}
    {error && <p className="native-error" role="alert">{error}</p>}
  </section>;
}

function ConflictEditor({ conflict, root, layer, disabled, perform, inspect, invalidate, blockDrafts }: {
  conflict: Conflict; root: string; layer: string; disabled: boolean;
  perform: (work: () => Promise<void>) => Promise<void>;
  inspect: (request: Request, apply: Request) => Promise<void>; invalidate: () => void;
  blockDrafts: Map<string,BlockDraft>;
}) {
  const [draft, setDraft] = useState("");
  const [executable, setExecutable] = useState(false);
  const [contents, setContents] = useState<Record<string, { text: string | null; byteLength?: number }>>({});
  const coupled = conflict.document.placements.length > 1;
  return <article><h3>{conflict.document.placements.map(p => p.path).join(", ")}</h3><code>{conflict.conflictId}</code>
    {!!conflict.document.blocks?.length && <NativeConflictBlocks root={root} layer={layer} conflict={conflict.conflictId} blocks={conflict.document.blocks} segments={conflict.document.segments} disabled={disabled} perform={perform} inspect={inspect} invalidate={invalidate} drafts={blockDrafts} />}
    <p>{coupled ? "Ce conflit concerne plusieurs chemins liés par un renommage. Choisir une variante applique ensemble ses créations, suppressions et contenus à tous les chemins indiqués. La comparaison et l’édition d’un fichier seul ne sont pas disponibles ici." : "Choisissez une variante conservée dans l’historique ou rédigez le contenu final."}</p>
    {!coupled && <NativeConflictComparison root={root} layer={layer} conflict={conflict.conflictId} terms={[...(conflict.document.bases || []).map(term => ({ ...term, kind: "Base" })), ...conflict.document.sides.map(term => ({ ...term, kind: "Variante" }))]} disabled={disabled} perform={perform} />}
    {conflict.document.sides.map(side => <details key={side.termId}><summary>Variante de {side.provenance.stepId}</summary>
      <p>Identifiant de variante : <code>{side.termId}</code></p>
      <pre>{JSON.stringify(side.values, null, 2)}</pre>
      {!coupled && <button disabled={disabled} onClick={() => void perform(async () => {
        const content = await nativeWorkspace<{ text: string | null; byteLength?: number }>(root, { action: "conflictVariant", layer, conflict: conflict.conflictId, term: side.termId });
        setContents(values => ({ ...values, [side.termId]: content }));
      })}>Lire le contenu de cette variante</button>}
      {contents[side.termId] && (contents[side.termId].text === null ? <p>Contenu non textuel ou trop volumineux pour cet éditeur. Sélectionnez la variante ou utilisez un fichier avec la CLI.</p> : <>
        <pre>{contents[side.termId].text}</pre><button disabled={disabled} onClick={() => { setDraft(contents[side.termId].text!); invalidate(); }}>Utiliser comme brouillon</button>
      </>)}
      <button disabled={disabled} onClick={() => void perform(() => {
        const choices = { [conflict.conflictId]: side.termId };
        return inspect({ action: "previewResolution", layer, choices }, { action: "applyResolution", layer, choices });
      })}>Prévisualiser cette variante</button>
    </details>)}
    {!coupled && <details><summary>Résolution manuelle du texte</summary>
      <p>Le brouillon reste dans cet écran jusqu’à application. Un texte vide crée un fichier vide. Fermer le projet abandonne ce brouillon.</p>
      <label>Contenu final<textarea value={draft} disabled={disabled} maxLength={1024 * 1024} rows={12} onChange={e => { setDraft(e.target.value); invalidate(); }} /></label>
      <label className="native-check"><input type="checkbox" checked={executable} disabled={disabled} onChange={e => { setExecutable(e.target.checked); invalidate(); }} />Fichier exécutable (Unix)</label>
      <button disabled={disabled} onClick={() => void perform(() => {
        const params = { layer, conflict: conflict.conflictId, text: draft, executable };
        return inspect({ action: "previewReplacement", ...params }, { action: "applyReplacement", ...params });
      })}>Prévisualiser le texte final</button>
    </details>}
  </article>;
}
