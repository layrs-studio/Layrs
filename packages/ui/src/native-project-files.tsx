import { useEffect, useId, useRef, useState } from "react";
import { checkNativeNamespaceMutation, nativeProjectionReady, nativeProjectFileLabel } from "./native-project-files-model";
import type { NativeNamespaceMutation, NativeNamespacePreviewRequest, NativeProjectFilesSnapshot } from "./native-project-files-model";

export function NativeProjectFiles({scope,snapshot,busy,onRefresh,onPreviewMutation}: {
  scope: string; snapshot: NativeProjectFilesSnapshot | null; busy: boolean;
  onRefresh?: () => Promise<void>;
  /** Must call the real engine's preview; this component never mutates files. */
  onPreviewMutation?: (request: NativeNamespacePreviewRequest) => Promise<void>;
}) {
  const [kind,setKind] = useState<NativeNamespaceMutation["kind"]>("create");
  const [source,setSource] = useState("");
  const [destination,setDestination] = useState("");
  const [error,setError] = useState("");
  const [pending,setPending] = useState(false);
  const revision = useRef(0), active = useRef(false);
  const id = useId();
  const identity = JSON.stringify([scope,snapshot?.anchor,snapshot?.state,snapshot?.namespaceComplete]);
  useEffect(() => { revision.current++; active.current=false; setPending(false); setSource("");setDestination("");setError(""); return () => {revision.current++;}; },[identity]);
  const mutation: NativeNamespaceMutation = kind === "create" ? {kind,destination} : {kind,source,destination};
  const check = checkNativeNamespaceMutation(snapshot,mutation);
  const ready = nativeProjectionReady(snapshot);
  async function perform(work: () => Promise<void>) {
    if (busy || active.current) return;
    const current = revision.current; active.current=true; setPending(true);setError("");
    try { await work(); } catch(e) {if(current===revision.current)setError(String(e));}
    finally {if(current===revision.current){active.current=false;setPending(false);}}
  }
  return <section aria-label="Fichiers du projet">
    <h2>Fichiers du projet</h2>
    <p>Les noms et chemins restent visibles. Un cadenas indique un contenu interdit ou un chemin réservé ; il ne donne pas accès aux octets.</p>
    {onRefresh && <button disabled={busy || pending} onClick={() => void perform(onRefresh)}>Actualiser les fichiers</button>}
    {!ready && <p role="status">Projection absente, périmée ou incomplète. Les modifications de chemins sont indisponibles.</p>}
    {snapshot && <p>Capture : <code>{snapshot.anchor?.stepId}</code> · Révision des droits : {snapshot.anchor?.authorizationEpoch} · Projection : {snapshot.anchor?.generation}</p>}
    <ul>{snapshot?.entries?.slice(0,1000).map((entry,index) => <li key={`${entry.path}:${index}`}><code>{entry.path}</code> — {nativeProjectFileLabel(entry)}</li>)}</ul>
    {(snapshot?.entries?.length ?? 0)>1000 && <p>Affichage limité aux 1 000 premières entrées ; validation sur le registre complet.</p>}
    {!!snapshot?.reservedPaths?.length && <details><summary>Chemins réservés</summary><ul>{snapshot.reservedPaths.slice(0,1000).map((r,index)=><li key={`${r.path}:${index}`}><code>{r.path}</code> — 🔒 {r.reason === "local_preserved" ? "Données locales conservées" : r.reason === "content_denied" ? "Contenu interdit" : r.reason === "collision" ? "Collision à résoudre" : "État inconnu"}</li>)}</ul></details>}
    {!onPreviewMutation ? <p>La modification des chemins n’est pas disponible dans cette connexion.</p> : <fieldset disabled={busy || pending || !ready}>
      <legend>Préparer une modification de chemin</legend>
      <label htmlFor={`${id}-kind`}>Action</label><select id={`${id}-kind`} value={kind} onChange={e=>setKind(e.target.value as NativeNamespaceMutation["kind"])}><option value="create">Créer</option><option value="rename">Renommer</option><option value="move">Déplacer</option></select>
      {kind!=="create" && <><label htmlFor={`${id}-source`}>Chemin source</label><input id={`${id}-source`} value={source} onChange={e=>setSource(e.target.value)}/></>}
      <label htmlFor={`${id}-destination`}>Chemin de destination</label><input id={`${id}-destination`} value={destination} onChange={e=>setDestination(e.target.value)}/>
      {!check.allowed && <p>{check.message}</p>}
      <button disabled={!check.allowed} onClick={()=>void perform(async()=>{const current=checkNativeNamespaceMutation(snapshot,mutation);if(!current.allowed)throw new Error(current.message);await onPreviewMutation({anchor:snapshot!.anchor,mutation});})}>Préparer l’aperçu du changement</button>
    </fieldset>}
    {error && <p role="alert">{error}</p>}
  </section>;
}
