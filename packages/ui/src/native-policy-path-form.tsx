import {useEffect,useRef,useState} from "react";
import type {PolicyTransport} from "./native-policy-editor";
import type {NativeAdminCommand} from "./native-admin-route";
import {changeNativeMemberPathRules,pathRules} from "./native-policy-path-form-model";
import type {NativePolicyRulesRead} from "./native-policy-path-form-model";
import {validateNativeProjectionContext,validateNativeProjectionPage} from "./native-remote-project-files-model";
import type {NativeProjectionContext,NativeProjectionPage} from "./native-remote-project-files-model";

type Member={principalId:string;displayName:string};
export function NativePolicyPathForm({scope,busy,onBusy,transport,administration,onPreview,onInvalidate}:{scope:string;busy:boolean;onBusy:(b:boolean)=>void;transport:PolicyTransport;administration:<T>(c:NativeAdminCommand)=>Promise<T>;onPreview:(preview:unknown)=>void;onInvalidate:()=>void}){
  const [data,setData]=useState<NativePolicyRulesRead|null>(null),[members,setMembers]=useState<Member[]>([]),[context,setContext]=useState<NativeProjectionContext|null>(null);
  const [policy,setPolicy]=useState(""),[member,setMember]=useState(""),[step,setStep]=useState(""),[path,setPath]=useState(""),[kind,setKind]=useState<"exact"|"subtree">("exact"),[access,setAccess]=useState<"none"|"read"|"write">("read");
  const [page,setPage]=useState<NativeProjectionPage|null>(null),[error,setError]=useState("");
  const active=useRef(false),revision=useRef(0);
  useEffect(()=>{revision.current++;active.current=false;setData(null);setMembers([]);setContext(null);setPage(null);setPolicy("");setMember("");setStep("");setPath("");setError("");return()=>{revision.current++;};},[scope]);
  async function perform(work:(current:number)=>Promise<void>){if(busy||active.current)return;const current=revision.current;active.current=true;onBusy(true);setError("");onInvalidate();try{await work(current);}catch(e){if(current===revision.current)setError(String(e));}finally{if(current===revision.current){active.current=false;onBusy(false);}}}
  async function load(){await perform(async current=>{
    setData(null);setPage(null);setContext(null);setMembers([]);setPath("");
    const [rules,directory,bootstrap]=await Promise.all([transport<NativePolicyRulesRead>({action:"readPolicyRules"}),administration<{members:Member[]}>({action:"eligibleTeamMembers"}),transport<NativeProjectionContext>({action:"collaboration",command:{action:"projectionContext"}})]);
    const checked=validateNativeProjectionContext(bootstrap);
    if(rules.principalId!==checked.principalId||rules.spaceId!==checked.spaceId||rules.policySetId!==checked.currentPolicySetId||rules.authorizationEpoch!==checked.authorizationEpoch||!Array.isArray(rules.policies)||rules.policies.length>32||!Array.isArray(directory.members)||directory.members.length>256||directory.members.some(m=>!m.principalId?.startsWith("principal:")||typeof m.displayName!=="string"))throw new Error("Les droits ont changé ou l’annuaire est incomplet. Rechargez le formulaire.");
    if(current!==revision.current)return;setData(rules);setContext(checked);setMembers(directory.members);setPolicy("");setMember(directory.members[0]?.principalId||"");setStep(checked.layers.flatMap(l=>l.headStepIds)[0]||"");
  });}
  async function paths(next=false){if(!context||!step)return;await perform(async current=>{
    const previous=next?page:null;const request={stepId:step,expectedPolicySetId:context.currentPolicySetId,expectedAuthorizationEpoch:context.authorizationEpoch,cursor:previous?.nextCursor||null,limit:100};
    const response=validateNativeProjectionPage(await transport({action:"collaboration",command:{action:"projectionPage",...request}}),context,request,previous);
    if(current===revision.current){setPage(response);setPath("");}
  });}
  async function preview(){if(!data||!member||!path)return;await perform(async current=>{
    const selected=policy?data.policies.find(p=>p.policyId===policy):null;if(policy&&!selected)throw new Error("Politique absente.");
    const rules=changeNativeMemberPathRules(selected?.rules??null,member,path,kind,access);
    const result=await transport<{preview:{expectedPolicySetId:string;expectedAuthorizationEpoch:number}}>({action:policy?"previewReplacePolicyRules":"previewPolicyRules",...(policy?{policyId:policy}:{}),rules});
    if(result.preview.expectedPolicySetId!==data.policySetId||result.preview.expectedAuthorizationEpoch!==data.authorizationEpoch)throw new Error("La politique a changé depuis la lecture. Rechargez avant de préparer l’aperçu.");
    if(current===revision.current)onPreview(result.preview);
  });}
  function invalidate(){onInvalidate();setError("");}
  return <details><summary>Droits d’un membre sur un fichier ou dossier</summary>
    <p>Ce formulaire ajoute ou remplace une politique v3. Les autres membres et règles sont conservés. Une interdiction d’un dossier parent ou d’une autre politique reste prioritaire. Les actions autorisées sur le projet ne sont pas élargies.</p>
    <button disabled={busy} onClick={()=>void load()}>Charger les membres et politiques</button>
    {data&&<fieldset disabled={busy}><legend>Choisir le membre et le chemin</legend>
      <label>Politique à modifier<select value={policy} onChange={e=>{setPolicy(e.target.value);invalidate();}}><option value="">Nouvelle politique de chemins</option>{data.policies.map(p=>{let editable=true;try{pathRules(p.rules);}catch{editable=false;}return <option key={p.policyId} value={p.policyId} disabled={!editable}>{p.policyId}{editable?"":" · ancien schéma, éditeur JSON requis"}</option>;})}</select></label>
      <label>Membre<select value={member} onChange={e=>{setMember(e.target.value);invalidate();}}>{members.map(m=><option key={m.principalId} value={m.principalId}>{m.displayName} · {m.principalId}</option>)}</select></label>
      <label>Capture pour choisir les chemins<select value={step} onChange={e=>{setStep(e.target.value);setPage(null);setPath("");invalidate();}}>{context?.layers.flatMap(l=>l.headStepIds.map(s=><option key={`${l.record.layerId}:${s}`} value={s}>{l.record.name} · {s}</option>))}</select></label>
      <button disabled={!step} onClick={()=>void paths()}>Charger les chemins de la capture</button>
      {page&&<><label>Fichier ou dossier<select value={path} onChange={e=>{setPath(e.target.value);setKind(page.entries.find(p=>p.path===e.target.value)?.target.kind==="tree"?"subtree":"exact");invalidate();}}><option value="">Choisir un chemin</option>{page.entries.filter(e=>e.target.kind==="file"||e.target.kind==="tree").map(e=><option key={e.path} value={e.path}>{e.path}{e.contentReadable?"":" · 🔒"}</option>)}</select></label>{page.nextCursor&&<button onClick={()=>void paths(true)}>Autres chemins pour les droits</button>}</>}
      <label>Portée<select value={kind} onChange={e=>{setKind(e.target.value as "exact"|"subtree");invalidate();}}><option value="exact">Ce chemin exact</option><option value="subtree">Ce chemin et ses descendants</option></select></label>
      <label>Accès au contenu<select value={access} onChange={e=>{setAccess(e.target.value as "none"|"read"|"write");invalidate();}}><option value="none">Interdire lecture et écriture</option><option value="read">Autoriser la lecture, interdire l’écriture</option><option value="write">Autoriser lecture et écriture</option></select></label>
      {!policy&&<p>Dans cette nouvelle politique, tout autre chemin est interdit à ce membre par défaut.</p>}
      <button disabled={!member||!path} onClick={()=>void preview()}>Préparer les droits de ce membre</button>
    </fieldset>}{error&&<p role="alert">{error}</p>}
  </details>;
}
