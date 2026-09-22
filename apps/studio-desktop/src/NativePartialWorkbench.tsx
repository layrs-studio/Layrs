import {useEffect,useRef,useState} from "react";
import {NativeProjectFiles,nativeProjectionReady,projectionStoreFiles,projectionCaptureMatches} from "@layrs/ui";
import type {NativeProjectionStatus,NativeCapturedProjectionPatch,NativeNamespacePreviewRequest} from "@layrs/ui";
import {nativeProjection,nativeRemote} from "./tauri";
type NamespacePreview={mutation:{kind:"create"|"rename"|"move";path:string;source?:string};local:{projectionGeneration:number}};
type RestorePreview={local:{before:Record<string,unknown>;after:Record<string,unknown>;operationId:string}};

export function NativePartialWorkbench({root,onClose}:{root:string;onClose:()=>void}){
  const [data,setData]=useState<NativeProjectionStatus|null>(null),[patch,setPatch]=useState<NativeCapturedProjectionPatch|null>(null);
  const [preview,setPreview]=useState<NamespacePreview|null>(null),[busy,setBusy]=useState(false),[error,setError]=useState(""),[notice,setNotice]=useState("");
  const [layer,setLayer]=useState(""),[ca,setCa]=useState("");
  const [history,setHistory]=useState<NativeCapturedProjectionPatch[]>([]),[restorePreview,setRestorePreview]=useState<RestorePreview|null>(null),[restoreSequence,setRestoreSequence]=useState(""),[restorePaths,setRestorePaths]=useState(""),[lastRestore,setLastRestore]=useState("");
  const generation=useRef(0),active=useRef(false);
  async function perform(work:(revision:number)=>Promise<void>){
    if(active.current)return;const revision=generation.current;active.current=true;setBusy(true);setError("");setNotice("");
    try{await work(revision);}catch(e){if(revision===generation.current){setData(null);setPreview(null);setRestorePreview(null);setError(String(e));}}
    finally{if(revision===generation.current){active.current=false;setBusy(false);}}
  }
  function acceptStatus(next:NativeProjectionStatus){projectionStoreFiles(next);setData(next);setPatch(next.latestCapture&&projectionCaptureMatches(next,next.latestCapture)?next.latestCapture:null);if(next.pendingPublication)setLayer(next.pendingPublication.layerId);}
  async function refresh(){await perform(async revision=>{const next=await nativeProjection<NativeProjectionStatus>(root,{action:"status"});if(revision===generation.current){acceptStatus(next);setPreview(null);}});}
  async function resume(action:"resumeInstallation"|"resumeNamespace"|"resumeRefresh"|"resumeRestore"){await perform(async revision=>{await nativeProjection(root,{action});const next=await nativeProjection<NativeProjectionStatus>(root,{action:"status"});if(revision===generation.current){acceptStatus(next);setPreview(null);setRestorePreview(null);setNotice("Opération locale reprise et vérifiée.");}});}
  useEffect(()=>{generation.current++;active.current=false;setData(null);setPatch(null);setPreview(null);setRestorePreview(null);setHistory([]);setRestoreSequence("");setRestorePaths("");setLastRestore("");setLayer("");setCa("");setError("");void refresh();return()=>{generation.current++;};},[root]);
  let files=null;
  try{if(data)files=projectionStoreFiles(data);}catch{/* A malformed status remains non-writable. */}
  const ready=files!==null&&nativeProjectionReady(files)&&data?.needsRepair===false;
  async function loadHistory(){await perform(async revision=>{const result=await nativeProjection<{captures:NativeCapturedProjectionPatch[]}>(root,{action:"history",before:null,limit:50});if(revision===generation.current)setHistory(result.captures);});}
  async function prepareRestore(inverse=false){if(!data||!ready||data.capabilities.restore!==true)return;await perform(async revision=>{
    const paths=[...new Set(restorePaths.split(/\r?\n/).map(path=>path.trim()).filter(Boolean))];
    const sequence=Number(restoreSequence);
    if(!inverse&&(!Number.isSafeInteger(sequence)||sequence<1||!paths.length))throw new Error("Sélectionnez une capture et au moins un chemin exact.");
    const source=inverse?{kind:"inverse",operationId:lastRestore}:{kind:"capture",sequence,paths};
    const result=await nativeRemote<RestorePreview>(root,{origin:data.identity.origin,caPem:ca||null,action:{action:"previewProjectionRestore",expectedGeneration:data.projection.generation,source}});
    if(revision===generation.current){setPreview(null);setRestorePreview(result);}
  });}
  async function applyRestore(){if(!data||!ready||!restorePreview||data.capabilities.restore!==true)return;await perform(async revision=>{
    const result=await nativeRemote<{operationId:string;capture:NativeCapturedProjectionPatch}>(root,{origin:data.identity.origin,caPem:ca||null,action:{action:"applyProjectionRestore",preview:restorePreview}});
    const next=await nativeProjection<NativeProjectionStatus>(root,{action:"status"});
    if(!projectionCaptureMatches(next,result.capture))throw new Error("La capture restaurée ne correspond plus à la projection.");
    if(revision===generation.current){acceptStatus(next);setPatch(result.capture);setLastRestore(result.operationId);setRestorePreview(null);setNotice("Fichiers restaurés localement. Les octets précédents sont conservés pour annuler ; publiez la capture pour changer le projet distant.");}
  });}
  async function namespace(request:NativeNamespacePreviewRequest){
    if(!data||!ready||data.capabilities?.namespace!==true)throw new Error("Modification de chemins indisponible dans cet état.");
    await perform(async revision=>{
      const {mutation}=request;
      const result=await nativeRemote<NamespacePreview>(root,{origin:data.identity.origin,caPem:ca||null,action:{action:"previewProjectionNamespace",expectedGeneration:request.anchor.generation,mutation:{kind:mutation.kind,path:mutation.destination,...(mutation.kind!=="create"?{source:mutation.source}:{})}}});
      const next=await nativeProjection<NativeProjectionStatus>(root,{action:"status"});
      if(revision===generation.current){acceptStatus(next);setPreview(result);}
    });
  }
  async function applyNamespace(){if(!data||!ready||!preview||data.capabilities.namespace!==true)return;await perform(async revision=>{
    const captured=await nativeRemote<NativeCapturedProjectionPatch>(root,{origin:data.identity.origin,caPem:ca||null,action:{action:"applyProjectionNamespace",preview}});
    const next=await nativeProjection<NativeProjectionStatus>(root,{action:"status"});
    if(!projectionCaptureMatches(next,captured))throw new Error("La capture du changement ne correspond plus à la projection. Actualisez avant de poursuivre.");
    if(revision===generation.current){acceptStatus(next);setPatch(captured);setPreview(null);setNotice("Changement appliqué et capturé localement. Publiez cette capture pour modifier le projet distant.");}
  });}
  async function receive(){if(!data||!ready||data.capabilities.receive!==true||!layer.trim())return;await perform(async revision=>{
    await nativeRemote(root,{origin:data.identity.origin,caPem:ca||null,action:{action:"receiveProjection",layerId:layer.trim()}});
    const next=await nativeProjection<NativeProjectionStatus>(root,{action:"status"});
    if(revision===generation.current){acceptStatus(next);setPreview(null);setNotice("Projection reçue et vérifiée. Les chemins réservés sont conservés.");}
  });}
  async function capture(){if(!data||!ready||data.capabilities?.capture!==true)return;await perform(async revision=>{
    const result=await nativeProjection<NativeCapturedProjectionPatch>(root,{action:"capture",expectedGeneration:data.projection.generation});
    if(!projectionCaptureMatches(data,result))throw new Error("Capture incohérente avec la projection ouverte.");
    if(revision===generation.current){setPatch(result);setNotice("Modifications autorisées capturées localement. Aucune publication n’a été effectuée.");setPreview(null);}
  });}
  async function publish(){if(!data||!patch||!ready||data.capabilities?.publish!==true||!layer.trim())return;await perform(async revision=>{
    const result=await nativeRemote<{receipt:{receipt:{status:string;rejections?:{code:string}[]}};pendingLocalAcceptance:boolean}>(root,{origin:data.identity.origin,caPem:ca||null,action:{action:"publishProjection",patchId:patch.sequence,layerId:layer.trim()}});
    const receipt=result.receipt?.receipt;
    if(receipt?.status!=="applied")throw new Error(`Publication refusée : ${receipt?.rejections?.map(r=>r.code).join(", ")||receipt?.status||"reçu absent"}. La capture locale est conservée.`);
    if(revision!==generation.current)return;
    setNotice(result.pendingLocalAcceptance ? "Publication confirmée par le serveur ; l’acceptation locale reste en attente. Réessayez la même capture et la même cible pour terminer." : "Publication confirmée par le serveur.");
    try{const next=await nativeProjection<NativeProjectionStatus>(root,{action:"status"});if(revision===generation.current){acceptStatus(next);setPreview(null);}}
    catch{if(revision===generation.current){setData(null);setNotice("Publication confirmée ; la relecture de l’état local est indisponible. Actualisez la projection avant de poursuivre.");}}
  });}
  return <main className="native-workbench"><header><div><p className="native-eyebrow">LAYRS · PROJECTION PARTIELLE</p><h1>Fichiers autorisés de votre projet</h1><code>{root}</code></div><button disabled={busy} onClick={onClose}>Changer de dossier</button></header>
    <p>Les noms interdits restent visibles avec un cadenas. Leur contenu n’est pas téléchargé. Une ancienne copie locale conservée ne constitue pas un droit d’accès au serveur.</p>
    <button disabled={busy} onClick={()=>void refresh()}>Actualiser la projection locale</button>
    <details><summary>Reprendre une opération locale interrompue</summary><p>Choisissez l’opération qui était en cours au moment de la coupure. Le journal local doit la confirmer avant toute reprise.</p><button disabled={busy} onClick={()=>void resume("resumeInstallation")}>Reprendre le clonage local</button><button disabled={busy} onClick={()=>void resume("resumeNamespace")}>Reprendre une création ou un déplacement</button><button disabled={busy} onClick={()=>void resume("resumeRefresh")}>Reprendre la réception locale</button><button disabled={busy} onClick={()=>void resume("resumeRestore")}>Reprendre la restauration locale</button></details>
    {data&&<p>Projet <code>{data.identity.spaceId}</code> · Compte <code>{data.identity.principalId}</code> · Serveur {data.identity.origin}</p>}
    {data?.needsRepair&&<p role="alert">Une opération interrompue nécessite une reprise par le moteur. Les modifications sont bloquées.</p>}
    <NativeProjectFiles scope={`${root}:${data?.identity.principalId||""}`} snapshot={files} busy={busy} onPreviewMutation={data?.capabilities?.namespace===true?namespace:undefined}/>
    <section aria-label="Actions de la projection"><button disabled={busy||!ready||data?.capabilities?.capture!==true} onClick={()=>void capture()}>Capturer les modifications autorisées</button>
      {data?.capabilities?.capture!==true&&<p>La capture n’est pas disponible dans cet état du moteur.</p>}
      {patch&&<section aria-label="Capture partielle locale"><p>Capture {patch.sequence} · <code>{patch.digest}</code></p><ul>{patch.changes.slice(0,1000).map(change=><li key={change.path}><code>{change.path}</code> — {change.expected===null?"Ajout":change.replacement===null?"Suppression":"Modification"}</li>)}</ul>{!patch.changes.length&&<p>Aucune modification capturée.</p>}</section>}
      {data?.capabilities?.publish===true&&<><label>Layer distant cible<input value={layer} disabled={busy||!!data.pendingPublication} onChange={e=>setLayer(e.target.value)} placeholder="layer:…"/></label><label>Autorité de certification privée PEM<textarea value={ca} disabled={busy} onChange={e=>setCa(e.target.value)}/></label><p>Après une réponse perdue, conservez cette capture et la même cible pour retrouver le reçu exact.</p><button disabled={busy||!ready||!patch||!layer.trim()} onClick={()=>void publish()}>Publier la capture partielle</button></>}
      {data?.capabilities?.publish!==true&&<p>La publication partielle n’est pas disponible dans cette connexion.</p>}
      {data?.pendingPublication&&<p role="status">Une publication est en attente : rejouez la capture {data.pendingPublication.sequence} sur sa cible enregistrée.</p>}
      {data?.capabilities.receive===true&&<button disabled={busy||!ready||!layer.trim()||!!data.pendingPublication} onClick={()=>void receive()}>Recevoir la projection distante</button>}
    </section>
    {preview!==null&&<section aria-label="Aperçu de modification des chemins"><h2>{preview.mutation.kind==="create"?"Créer un fichier vide":preview.mutation.kind==="rename"?"Renommer le fichier":"Déplacer le fichier"}</h2>{preview.mutation.source&&<p>Source : <code>{preview.mutation.source}</code></p>}<p>Destination : <code>{preview.mutation.path}</code></p><p>Les droits et les fichiers locaux sont vérifiés à nouveau avant application.</p><button disabled={busy||!ready||data?.capabilities.namespace!==true} onClick={()=>void applyNamespace()}>Appliquer ce changement de chemin</button></section>}
    <details><summary>Historique et restauration locale</summary><button disabled={busy||!data} onClick={()=>void loadHistory()}>Charger les 50 dernières captures</button><ul>{history.map(item=><li key={item.sequence}><button disabled={busy} onClick={()=>{setRestoreSequence(String(item.sequence));setRestorePaths(item.changes.map(change=>change.path).join("\n"));setRestorePreview(null);}}>Capture {item.sequence} · {item.changes.length} chemin(s)</button></li>)}</ul><p>Choisissez les fichiers d’une capture locale à rétablir. Les droits actuels sont vérifiés ; les autres fichiers restent inchangés.</p><label>Numéro de capture à restaurer<input value={restoreSequence} disabled={busy} onChange={e=>{setRestoreSequence(e.target.value);setRestorePreview(null);}}/></label><label>Chemins à restaurer, un par ligne<textarea value={restorePaths} disabled={busy} onChange={e=>{setRestorePaths(e.target.value);setRestorePreview(null);}}/></label><button disabled={busy||!ready||data?.capabilities.restore!==true||!restoreSequence||!restorePaths.trim()} onClick={()=>void prepareRestore()}>Préparer la restauration locale</button>{lastRestore&&<button disabled={busy||!ready||data?.capabilities.restore!==true} onClick={()=>void prepareRestore(true)}>Préparer l’annulation de la dernière restauration</button>}</details>
    {restorePreview&&<section aria-label="Aperçu de restauration locale"><h2>Fichiers à restaurer</h2><ul>{Object.entries(restorePreview.local.after).map(([path,target])=><li key={path}><code>{path}</code> — {target===null?"Suppression locale":restorePreview.local.before[path]===null?"Création locale":"Remplacement local"}</li>)}</ul><p>Les octets remplacés sont conservés dans le journal pour permettre l’annulation.</p><button disabled={busy||!ready||data?.capabilities.restore!==true} onClick={()=>void applyRestore()}>Appliquer la restauration locale</button></section>}
    {busy&&<p role="status">Opération en cours…</p>}{notice&&<p role="status">{notice}</p>}{error&&<p role="alert">{error}</p>}
  </main>;
}
