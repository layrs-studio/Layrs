import { useState } from "react";
import { nativeWorkspace } from "./tauri";

export type ConflictBlock = {
  blockId: string;
  baseValues: {termId: string}[];
  sideValues: {termId: string}[];
  resolution: unknown | null;
};
type Request = {action: string; [key: string]: unknown};
export type BlockDraft = {text: string; sources: string[]};

export function NativeConflictBlocks({root,layer,conflict,blocks,segments,disabled,perform,inspect,invalidate,drafts}: {
  root: string; layer: string; conflict: string; blocks: ConflictBlock[];
  segments?: {kind: string; blockId?: string}[]; disabled: boolean;
  perform: (work: () => Promise<void>) => Promise<void>;
  inspect: (request: Request, apply: Request) => Promise<void>;
  invalidate: () => void; drafts: Map<string,BlockDraft>;
}) {
  const order = segments?.filter(segment=>segment.kind==="block").map(segment=>segment.blockId) || [];
  const ordered = [...blocks].sort((a,b)=>order.indexOf(a.blockId)-order.indexOf(b.blockId));
  return <section aria-label="Résolution par blocs"><h4>Résoudre les passages en conflit</h4>
    <p>{blocks.filter(block=>!block.resolution).length} passages restent à résoudre. Chaque application conserve une capture intermédiaire ; le fichier sera installé après le dernier passage.</p>
    <p>Les brouillons restent disponibles après une résolution partielle ou une actualisation des conflits. Fermer le projet ou changer de Layer les abandonne.</p>
    {ordered.map((block,index)=><BlockEditor key={block.blockId} {...{root,layer,conflict,block,index,disabled,perform,inspect,invalidate,drafts}} />)}
  </section>;
}

function BlockEditor({root,layer,conflict,block,index,disabled,perform,inspect,invalidate,drafts}: {
  root: string; layer: string; conflict: string; block: ConflictBlock; index: number; disabled: boolean;
  perform: (work: () => Promise<void>) => Promise<void>;
  inspect: (request: Request, apply: Request) => Promise<void>;
  invalidate: () => void; drafts: Map<string,BlockDraft>;
}) {
  const terms = [...block.baseValues.map(term=>({...term,label:"Base"})),...block.sideValues.map((term,i)=>({...term,label:`Variante ${i+1}`}))];
  const [draft,setDraft] = useState<BlockDraft>(()=>drafts.get(block.blockId) || {text:"",sources:terms.map(term=>term.termId).sort()});
  const [contents,setContents] = useState<Record<string,{text: string|null; byteLength?:number}>>({});
  function update(value: BlockDraft) {setDraft(value);drafts.set(block.blockId,value);invalidate();}
  function preview(choice: unknown) {
    const params = {layer,conflict,block:block.blockId,choice};
    return inspect({action:"previewBlockResolution",...params},{action:"applyBlockResolution",...params});
  }
  return <details><summary>Passage {index+1} · {block.resolution ? "Résolu" : "À résoudre"}</summary>
    <code>{block.blockId}</code>
    {block.resolution ? <p>Ce choix est enregistré dans la capture. L’historique conserve les variantes et les captures précédentes.</p> : <>
      {terms.map(term=><section key={term.termId} aria-label={`${term.label} du passage ${index+1}`}>
        <h5>{term.label}</h5><code>{term.termId}</code>
        <button disabled={disabled} onClick={()=>void perform(async()=>{
          const content=await nativeWorkspace<{text:string|null;byteLength?:number}>(root,{action:"conflictBlockVariant",layer,conflict,block:block.blockId,term:term.termId});
          setContents(previous=>({...previous,[term.termId]:content}));
        })}>Lire ce passage</button>
        {contents[term.termId] && (contents[term.termId].text===null ? <p>Passage trop volumineux pour cet éditeur. Utilisez la CLI.</p> : <><pre>{contents[term.termId].text}</pre><button disabled={disabled} onClick={()=>update({text:contents[term.termId].text!,sources:[term.termId]})}>Copier ce passage dans le brouillon</button></>)}
        <button disabled={disabled} onClick={()=>void perform(()=>preview({method:"selectTerm",term:term.termId}))}>Prévisualiser ce choix de passage</button>
      </section>)}
      <label>Texte du passage {index+1}<textarea rows={6} maxLength={1024*1024} disabled={disabled} value={draft.text} onChange={e=>update({...draft,text:e.target.value})} /></label>
      <fieldset disabled={disabled}><legend>Sources du passage {index+1}</legend>{terms.map(term=><label key={term.termId}><input type="checkbox" checked={draft.sources.includes(term.termId)} onChange={e=>update({...draft,sources:e.target.checked ? [...draft.sources,term.termId].sort() : draft.sources.filter(id=>id!==term.termId)})} />{term.label}</label>)}</fieldset>
      <p>Un texte vide supprime ce passage. Les sources choisies sont enregistrées avec la résolution.</p>
      <button disabled={disabled || !draft.sources.length} onClick={()=>void perform(()=>preview({method:"manual",text:draft.text,sourceTermIds:draft.sources}))}>Prévisualiser le texte de ce passage</button>
    </>}
  </details>;
}
