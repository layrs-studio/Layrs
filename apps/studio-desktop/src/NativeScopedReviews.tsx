import {useEffect,useRef,useState} from "react";
import {nativeRemote} from "./tauri";

type Review={reviewId:string;candidateId:string;title:string|null;changedPaths:string[];lifecycle:string};
type Action={actionId:string;actorPrincipalId:string;action:string;body:string|null;bodyRedacted:boolean};
type Detail={review:Review;actions:Action[];decisionState:string;integration:{state:string;approvals:number;requiredApprovals:number;blockingDecisions:number;proofStatus:string}};
export function NativeScopedReviews({root,origin,ca,disabled,candidateId}:{root:string;origin:string;ca:string;disabled:boolean;candidateId:string}) {
  const [reviews,setReviews]=useState<Review[]>([]),[detail,setDetail]=useState<Detail|null>(null);
  const [candidate,setCandidate]=useState(candidateId),[title,setTitle]=useState(""),[body,setBody]=useState("");
  const [busy,setBusy]=useState(false),[message,setMessage]=useState("");
  const active=useRef(false),revision=useRef(0),pending=useRef<{key:string;command:unknown}|null>(null);
  useEffect(()=>{setCandidate(candidateId);},[candidateId]);
  useEffect(()=>{revision.current++;active.current=false;pending.current=null;setDetail(null);setReviews([]);setMessage("");return()=>{revision.current++;};},[root,origin]);
  async function request<T>(command:unknown):Promise<T>{return nativeRemote<T>(root,{origin,caPem:ca||null,action:{action:"scopedReview",command}});}
  async function run(work:(version:number)=>Promise<void>){if(active.current||disabled)return;active.current=true;setBusy(true);setMessage("");const version=revision.current;try{await work(version);}catch(error){if(version===revision.current)setMessage(String(error));}finally{if(version===revision.current){active.current=false;setBusy(false);}}}
  async function list(){await run(async version=>{const page=await request<{reviews:Review[];nextCursor:string|null}>({action:"list",query:{limit:50}});if(version===revision.current){setReviews(page.reviews);if(page.nextCursor)setMessage("Les 50 premières revues sont affichées.");}});}
  async function open(reviewId:string){await run(async version=>{const value=await request<Detail>({action:"detail",reviewId});if(version===revision.current)setDetail(value);});}
  async function mutate(command:Record<string,unknown>,requestBody:Record<string,unknown>){await run(async version=>{
    // Preserve the exact command in memory after a lost response; a retry never
    // generates a second comment or targets another candidate.
    const key=JSON.stringify({command,requestBody});
    if(pending.current&&pending.current.key!==key)throw new Error("Une réponse est en attente. Réessayez la même action avant d’en préparer une autre.");
    const saved=pending.current??{key,command:{...command,request:{...requestBody,idempotencyKey:`studio-${crypto.randomUUID()}`}}};
    pending.current=saved;
    const result=await request<Review|Action|{receipt:{status:string}}>(saved.command);
    if(version!==revision.current)return;
    pending.current=null;
    if("receipt" in result){if(result.receipt.status!=="applied")throw new Error("Intégration refusée : actualisez la revue et vérifiez ses contrôles.");setMessage("Candidat intégré. Recevez la projection distante pour mettre à jour votre dossier.");}
    else {setMessage("Action enregistrée.");const reviewId="candidateId" in result?result.reviewId:detail?.review.reviewId;if(reviewId){const next=await request<Detail>({action:"detail",reviewId});if(version===revision.current)setDetail(next);}}
  });}
  const blocked=disabled||busy;
  return <section aria-label="Revues des fichiers autorisés"><h2>Revues des fichiers autorisés</h2>
    <p>Chaque revue porte sur les chemins de son candidat. Les textes rédigés avec des droits plus larges peuvent être masqués pour les autres lecteurs.</p>
    <button disabled={blocked} onClick={()=>void list()}>Charger les revues accessibles</button>
    <ul>{reviews.map(review=><li key={review.reviewId}><button disabled={blocked} onClick={()=>void open(review.reviewId)}>{review.title??"Titre masqué"} · {review.lifecycle}</button></li>)}</ul>
    <label>Candidat à proposer<input value={candidate} disabled={blocked} onChange={e=>setCandidate(e.target.value)}/></label>
    <label>Titre de la revue<input value={title} maxLength={200} disabled={blocked} onChange={e=>setTitle(e.target.value)}/></label>
    <button disabled={blocked||!candidate.trim()||!title.trim()} onClick={()=>void mutate({action:"propose"},{candidateId:candidate.trim(),title:title.trim()})}>Proposer ce candidat</button>
    {detail&&<article aria-label="Détail de la revue"><button disabled={blocked} onClick={()=>void open(detail.review.reviewId)}>Actualiser cette revue</button><h3>{detail.review.title??"Titre masqué"}</h3>
      <p>Chemins concernés :</p><ul>{detail.review.changedPaths.map(path=><li key={path}><code>{path}</code></li>)}</ul>
      <p>Approbations : {detail.integration.approvals}/{detail.integration.requiredApprovals} · Décisions bloquantes : {detail.integration.blockingDecisions} · Vérifications : {detail.integration.proofStatus}</p>
      <ul>{detail.actions.map(action=><li key={action.actionId}><strong>{action.actorPrincipalId}</strong> · {action.action}<p>{action.bodyRedacted?"Texte masqué selon vos droits actuels.":action.body}</p></li>)}</ul>
      <label>Commentaire<textarea value={body} disabled={blocked} maxLength={16000} onChange={e=>setBody(e.target.value)}/></label>
      <button disabled={blocked||!body.trim()} onClick={()=>void mutate({action:"act",reviewId:detail.review.reviewId},{action:"comment",body,anchorPath:null,replyTo:null})}>Commenter</button>
      <button disabled={blocked} onClick={()=>void mutate({action:"act",reviewId:detail.review.reviewId},{action:"approve",body,anchorPath:null,replyTo:null})}>Approuver ce candidat</button>
      <button disabled={blocked} onClick={()=>void mutate({action:"act",reviewId:detail.review.reviewId},{action:"request_changes",body,anchorPath:null,replyTo:null})}>Demander des modifications</button>
      <button disabled={blocked||detail.integration.state!=="approved"||!["passed","not_required"].includes(detail.integration.proofStatus)} onClick={()=>void mutate({action:"integrate",reviewId:detail.review.reviewId},{})}>Intégrer le candidat approuvé</button>
    </article>}
    {message&&<p role="status">{message}</p>}
  </section>;
}
