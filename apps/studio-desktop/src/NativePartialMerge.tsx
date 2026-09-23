import {useEffect,useRef,useState} from "react";
import {nativeProjection,nativeRemote} from "./tauri";

type Choice="base"|"local"|"remote"|"delete";
type Preview={historical?:{stepId:string;paths:string[]}|null;files:{path:string;conflict:boolean;automatic:unknown}[];local:{captureSequence:number;target:{baseStepId:string}}};
export function NativePartialMerge({root,origin,ca,layer,sequence,disabled,onApplied}:{root:string;origin:string;ca:string;layer:string;sequence:number|null;disabled:boolean;onApplied:()=>Promise<void>}){
  const [preview,setPreview]=useState<Preview|null>(null),[choices,setChoices]=useState<Record<string,Choice>>({});
  const [busy,setBusy]=useState(false),[message,setMessage]=useState("");
  const [historicalStep,setHistoricalStep]=useState(""),[historicalPaths,setHistoricalPaths]=useState("");
  const revision=useRef(0),active=useRef(false);
  useEffect(()=>{revision.current++;active.current=false;setBusy(false);setPreview(null);setChoices({});setMessage("");return()=>{revision.current++;};},[root,origin]);
  async function run(work:(version:number)=>Promise<void>){if(active.current||disabled)return;active.current=true;setBusy(true);setMessage("");const version=revision.current;try{await work(version);}catch(error){if(version===revision.current)setMessage(String(error));}finally{if(version===revision.current){active.current=false;setBusy(false);}}}
  async function prepare(){if(sequence===null||!layer.trim())return;await run(async version=>{
    const value=await nativeRemote<Preview>(root,{origin,caPem:ca||null,action:{action:"previewProjectionRebase",patchId:sequence,layerId:layer.trim()}});
    if(version===revision.current){setPreview(value);setChoices({});setMessage("Brouillon enregistré. Vos fichiers restent inchangés jusqu’à l’application.");}
  });}
  async function reload(){await run(async version=>{
    const value=await nativeProjection<Preview|null>(root,{action:"rebaseDraft"});
    if(version===revision.current){setPreview(value);setChoices({});if(!value)setMessage("Aucun brouillon de fusion enregistré.");}
  });}
  async function prepareHistorical(){if(sequence===null||!layer.trim()||!historicalStep.trim())return;await run(async version=>{
    const paths=[...new Set(historicalPaths.split(/\r?\n/).map(path=>path.trim()).filter(Boolean))].sort();
    const value=await nativeRemote<Preview>(root,{origin,caPem:ca||null,action:{action:"previewProjectionHistoricalRestore",patchId:sequence,layerId:layer.trim(),selection:{stepId:historicalStep.trim(),paths}}});
    if(version===revision.current){setPreview(value);setChoices({});setMessage("Aperçu historique enregistré. Vérifiez les fichiers avant de restaurer.");}
  });}
  async function apply(){if(!preview)return;await run(async version=>{
    await nativeRemote(root,{origin,caPem:ca||null,action:{action:"applyProjectionRebase",preview,choices}});
    if(version!==revision.current)return;
    await onApplied();
    if(version!==revision.current)return;
    setPreview(null);setChoices({});setMessage(preview.historical?"Version historique restaurée et capturée localement. Publiez la nouvelle capture pour la partager.":"Fusion appliquée et capturée localement. Publiez la nouvelle capture pour la partager.");
  });}
  const blocked=disabled||busy;
  return <section aria-label="Fusion de la projection"><h2>Fusionner avec la version distante</h2>
    <p>Capturez vos modifications, puis préparez la fusion. Les modifications de texte compatibles sont réunies automatiquement ; choisissez une version pour chaque conflit.</p>
    <button disabled={blocked||sequence===null||!layer.trim()} onClick={()=>void prepare()}>Préparer la fusion partielle</button>
    <button disabled={blocked} onClick={()=>void reload()}>Recharger le brouillon de fusion</button>
    <details><summary>Restaurer une version publiée</summary><p>Les fichiers ordinaires actuellement modifiables peuvent être restaurés. Une liste vide sélectionne tous ces fichiers connus ; les fichiers protégés restent réservés. La base distante reste inchangée jusqu’à publication.</p><label>Identifiant de la version publiée<input value={historicalStep} disabled={blocked} onChange={e=>setHistoricalStep(e.target.value)}/></label><label>Chemins historiques à restaurer, un par ligne<textarea value={historicalPaths} disabled={blocked} onChange={e=>setHistoricalPaths(e.target.value)}/></label><button disabled={blocked||sequence===null||!layer.trim()||!historicalStep.trim()} onClick={()=>void prepareHistorical()}>Préparer la restauration historique</button></details>
    {preview&&<><p>Capture {preview.local.captureSequence} · Nouvelle base <code>{preview.local.target.baseStepId}</code></p>
      <ul>{preview.files.map(file=><li key={file.path}><code>{file.path}</code>{file.conflict?<label>Version à conserver pour {file.path}<select value={choices[file.path]??""} disabled={blocked} onChange={event=>setChoices(previous=>({...previous,[file.path]:event.target.value as Choice}))}><option value="" disabled>Choisir une résolution</option><option value="local">Ma version locale</option><option value="remote">Version distante</option><option value="base">Version de départ</option><option value="delete">Supprimer le fichier</option></select></label>:<span> — {file.automatic===null?"Suppression résolue":"Résolu automatiquement"}</span>}</li>)}</ul>
      <button disabled={blocked||preview.files.some(file=>file.conflict&&!choices[file.path])} onClick={()=>void apply()}>{preview.historical?"Appliquer la restauration historique":"Appliquer la fusion résolue"}</button>
    </>}
    {message&&<p role="status">{message}</p>}
  </section>;
}
