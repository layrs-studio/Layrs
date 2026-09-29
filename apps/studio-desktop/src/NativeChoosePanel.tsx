import { useRef, useState } from "react";
import { nativeRemote } from "./tauri";

type Preview = {
  origin: string; spaceId: string; layerId: string; expectedViewId: string;
  expectedGeneration: number; previousHeads: string[]; chosenHeads: string[];
  discardedHeads: string[]; decisionProductObjectId: string;
};
export function NativeChoosePanel({root,origin,ca,busy,onBusy,onPublishCandidates}: {
  root:string;origin:string;ca:string;busy:boolean;onBusy:(value:boolean)=>void;onPublishCandidates:()=>void;
}) {
  const [layer,setLayer]=useState("");
  const [heads,setHeads]=useState("");
  const [reason,setReason]=useState("");
  const [preview,setPreview]=useState<Preview|null>(null);
  const [message,setMessage]=useState("");
  const [error,setError]=useState("");
  const inFlight=useRef(false);
  async function perform(work:()=>Promise<void>) {
    if(busy || inFlight.current)return;
    inFlight.current=true;onBusy(true);setError("");setMessage("");
    try {await work();}catch(e){setError(String(e));}finally{inFlight.current=false;onBusy(false);}
  }
  function invalidate(){setPreview(null);setMessage("");setError("");}
  const list=(ids:string[])=>ids.length ? <ul>{ids.map(id=><li key={id}><code>{id}</code></li>)}</ul> : <p>Aucune</p>;
  return <details><summary>Choisir les têtes distantes · administration</summary>
    <p>Un administrateur peut remplacer explicitement les têtes publiées d’un Layer. Les anciennes captures restent dans l’historique. Ce choix ne modifie pas vos fichiers locaux ; utilisez ensuite Recevoir.</p>
    <p>Après un regroupement ou une reprise de changements, envoyez d’abord les captures candidates. Vous pourrez ensuite prévisualiser leur sélection comme têtes distantes.</p>
    <button disabled={busy || !origin} onClick={()=>{invalidate();onPublishCandidates();}}>Envoyer les captures candidates</button>
    <label>Layer distant à modifier<input disabled={busy} value={layer} placeholder="layer:…" onChange={e=>{setLayer(e.target.value);invalidate();}} /></label>
    <label>Captures à conserver comme têtes<textarea disabled={busy} value={heads} placeholder="Un identifiant blake3:… par ligne. Vide pour retirer toutes les têtes." onChange={e=>{setHeads(e.target.value);invalidate();}} /></label>
    <label>Motif du choix distant<textarea disabled={busy} value={reason} maxLength={4096} onChange={e=>{setReason(e.target.value);invalidate();}} /></label>
    <button disabled={busy || !origin || !layer.trim() || !reason.trim()} onClick={()=>void perform(async()=>{
      const chosenHeads=[...new Set(heads.split(/\s+/).filter(Boolean))].sort();
      const value=await nativeRemote<{preview:Preview}>(root,{origin,caPem:ca||null,action:{action:"previewChoose",layer:layer.trim(),chosenHeads,reason}});
      setPreview(value.preview);
    })}>Prévisualiser le choix distant</button>
    {preview && <section aria-label="Aperçu du choix distant">
      <h3>Effet sur les têtes du serveur</h3><p>Génération attendue : {preview.expectedGeneration}</p><code>{preview.expectedViewId}</code>
      <h4>Têtes actuelles</h4>{list(preview.previousHeads)}<h4>Têtes après application</h4>{list(preview.chosenHeads)}<h4>Têtes écartées explicitement</h4>{list(preview.discardedHeads)}
      {!preview.chosenHeads.length && <p>Le Layer n’aura plus aucune tête publiée. Ses anciennes captures restent conservées.</p>}
      <p>Si le serveur a changé, cette décision sera refusée. Après une réponse perdue, réessayez cet aperçu pour retrouver le même reçu.</p>
      <button disabled={busy} onClick={()=>void perform(async()=>{
        const result=await nativeRemote<{receipt:{status:string;rejections?:{code:string}[]}}>(root,{origin,caPem:ca||null,action:{action:"applyChoose",preview}});
        if(result.receipt.status!=="applied") {
          if(result.receipt.rejections?.some(item=>item.code==="gate_rejected")) throw new Error("Décision refusée par les protections du Layer. Votre aperçu est conservé.");
          throw new Error("Décision refusée par le serveur. Vérifiez vos droits, puis recréez l’aperçu si la vue a changé.");
        }
        setPreview(null);setMessage("Choix publié. Utilisez Recevoir pour examiner la nouvelle vue distante.");
      })}>Appliquer ce choix sur le serveur</button>
    </section>}
    {message && <p role="status">{message}</p>}{error && <p role="alert" className="native-error">{error}</p>}
  </details>;
}
