import {useEffect,useId,useRef,useState} from "react";
import {validateNativeProjectionContext,validateNativeProjectionPage} from "./native-remote-project-files-model";
import type {NativeProjectionContext,NativeProjectionPage,NativeProjectionPageRequest} from "./native-remote-project-files-model";

export function NativeRemoteProjectFiles({scope,busy,onBusy,loadContext,loadPage}:{
  scope:string;busy:boolean;onBusy:(busy:boolean)=>void;
  loadContext:()=>Promise<NativeProjectionContext>;
  loadPage:(request:NativeProjectionPageRequest)=>Promise<NativeProjectionPage>;
}){
  const [context,setContext]=useState<NativeProjectionContext|null>(null),[step,setStep]=useState("");
  const [page,setPage]=useState<NativeProjectionPage|null>(null),[error,setError]=useState("");
  const revision=useRef(0),active=useRef(false),seenCursors=useRef(new Set<string>()),seenPaths=useRef(new Set<string>());
  const id=useId();
  useEffect(()=>{revision.current++;active.current=false;setContext(null);setStep("");setPage(null);setError("");seenCursors.current.clear();seenPaths.current.clear();return()=>{revision.current++;};},[scope]);
  async function perform(work:(current:number)=>Promise<void>){
    if(busy||active.current)return;const current=revision.current;active.current=true;onBusy(true);setError("");
    try{await work(current);}catch(e){if(current===revision.current){setPage(null);setContext(null);seenCursors.current.clear();seenPaths.current.clear();setError(String(e));}}
    finally{if(current===revision.current){active.current=false;onBusy(false);}}
  }
  async function refresh(){await perform(async current=>{
    setPage(null);setContext(null);seenCursors.current.clear();seenPaths.current.clear();
    const next=validateNativeProjectionContext(await loadContext());if(current!==revision.current)return;
    setContext(next);setStep(next.layers.flatMap(l=>l.headStepIds)[0]||"");
  });}
  async function read(next:boolean){if(!context||!step)return;await perform(async current=>{
    const previous=next?page:null,cursor=previous?.nextCursor||null;
    if(next&&!cursor)throw new Error("Aucune page suivante.");
    if(!next){seenCursors.current.clear();seenPaths.current.clear();setPage(null);}
    const request={stepId:step,expectedPolicySetId:context.currentPolicySetId,expectedAuthorizationEpoch:context.authorizationEpoch,cursor,limit:100};
    const result=validateNativeProjectionPage(await loadPage(request),context,request,previous);if(current!==revision.current)return;
    if(result.nextCursor&&seenCursors.current.has(result.nextCursor))throw new Error("Curseur répété : actualisez le projet.");
    if(seenPaths.current.size+result.entries.length>100000||result.entries.some(e=>seenPaths.current.has(e.path)))throw new Error("Registre incomplet ou répété : actualisez le projet.");
    if(result.nextCursor)seenCursors.current.add(result.nextCursor);for(const entry of result.entries)seenPaths.current.add(entry.path);
    setPage(result);
  });}
  return <details><summary>Fichiers distants et droits d’accès</summary>
    <p>Les chemins restent visibles avec un cadenas si leur contenu est interdit. Cette lecture décrit le serveur ; elle ne matérialise aucun fichier et ne décrit pas les copies déjà présentes sur cet ordinateur.</p>
    <button disabled={busy} onClick={()=>void refresh()}>Charger les captures du projet</button>
    {context&&<><label htmlFor={id}>Capture à explorer</label><select id={id} disabled={busy} value={step} onChange={e=>{setStep(e.target.value);setPage(null);seenCursors.current.clear();seenPaths.current.clear();}}>{context.layers.flatMap(layer=>layer.headStepIds.map(head=><option key={`${layer.record.layerId}:${head}`} value={head}>{layer.record.name} · {head}</option>))}</select><button disabled={busy||!step} onClick={()=>void read(false)}>Afficher les chemins</button></>}
    {page&&<section aria-label="Chemins du projet distant"><p>Capture <code>{page.stepId}</code> · Révision des droits {page.authorizationEpoch}. Les téléchargements et écritures doivent revérifier ces droits.</p>
      <ul>{page.entries.map(entry=><li key={entry.path}><code>{entry.path}</code> — {entry.contentReadable ? entry.writable ? "Lecture et écriture autorisées au serveur" : "Lecture autorisée · écriture interdite" : "🔒 Contenu interdit"} {entry.target.kind==="tree"&&" · dossier"}</li>)}</ul>
      {!page.entries.length&&<p>Aucun chemin dans cette capture.</p>}
      <p>{page.entries.length ? `Entrées ${page.offset+1} à ${page.offset+page.entries.length}` : "Aucune entrée"}{page.nextCursor ? " · liste incomplète" : " · fin du parcours"}. Aucun registre de projection locale n’est déduit de ces pages.</p>
      {page.nextCursor&&<button disabled={busy} onClick={()=>void read(true)}>Afficher les chemins suivants</button>}
    </section>}
    {error&&<p role="alert">{error}</p>}
  </details>;
}
