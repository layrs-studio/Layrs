export type NativeProjectionContext = {
  principalId: string; spaceId: string; currentPolicySetId: string; authorizationEpoch: number;
  layers: {record:{layerId:string;name:string};headStepIds:string[]}[];
};
export type NativeProjectionPageRequest = { stepId:string; expectedPolicySetId:string; expectedAuthorizationEpoch:number; cursor:string|null; limit:number };
export type NativeProjectionTarget = {kind:"file";fileId:string}|{kind:"tree";treeId:string}|{kind:"conflict";conflictId:string}|{kind:"symlink";symlinkId:string}|{kind:"tombstone";tombstoneId:string};
export type NativeProjectionPageEntry = {path:string;target:NativeProjectionTarget;executable:boolean;contentReadable:boolean;writable:boolean};
export type NativeProjectionPage = {
  schemaVersion:"layrs.projection-page.v1";principalId:string;spaceId:string;stepId:string;rootTreeId:string;policySetId:string;authorizationEpoch:number;
  offset:number;entries:NativeProjectionPageEntry[];nextCursor:string|null;
};
const hash = (v:unknown):v is string => typeof v === "string" && /^blake3:[a-f0-9]{64}$/.test(v);
const text = (v:unknown):v is string => typeof v === "string" && v.length>0 && v.length<=4096;
const uint = (v:unknown):v is number => Number.isSafeInteger(v) && Number(v)>=0;
const object = (v:unknown):v is Record<string,unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const path = (v:unknown):v is string => text(v) && new TextEncoder().encode(v).length<=4096 && !/[\\\x00-\x1f\x7f]/.test(v) && v.split("/").every(p=>p!=="" && p!=="." && p!=="..");

export function validateNativeProjectionContext(value:unknown):NativeProjectionContext {
  if(!object(value) || !text(value.principalId) || !text(value.spaceId) || !hash(value.currentPolicySetId) || !uint(value.authorizationEpoch) || !Array.isArray(value.layers) || value.layers.length>10000)throw new Error("Contexte du projet incomplet. Actualisez la connexion.");
  let heads=0;
  for(const layer of value.layers){
    if(!object(layer) || !object(layer.record) || !text(layer.record.layerId) || !text(layer.record.name) || !Array.isArray(layer.headStepIds) || !layer.headStepIds.every(hash))throw new Error("Captures du projet invalides.");
    heads+=layer.headStepIds.length;if(heads>10000)throw new Error("Trop de captures pour ce sélecteur.");
  }
  return value as NativeProjectionContext;
}

/** Pages are observations at one authorization epoch, never reusable capabilities. */
export function validateNativeProjectionPage(value:unknown,context:NativeProjectionContext,request:NativeProjectionPageRequest,previous:NativeProjectionPage|null):NativeProjectionPage {
  const fail=()=>{throw new Error("Page de fichiers incohérente ou périmée. Rechargez le projet.");};
  if(!object(value))return fail();
  const fields=["schemaVersion","principalId","spaceId","stepId","rootTreeId","policySetId","authorizationEpoch","offset","entries","nextCursor"];
  if(Object.keys(value).length!==fields.length || Object.keys(value).some(k=>!fields.includes(k)) || value.schemaVersion!=="layrs.projection-page.v1" || value.principalId!==context.principalId || value.spaceId!==context.spaceId || value.stepId!==request.stepId || value.policySetId!==context.currentPolicySetId || request.expectedPolicySetId!==context.currentPolicySetId || value.authorizationEpoch!==context.authorizationEpoch || request.expectedAuthorizationEpoch!==context.authorizationEpoch || !hash(value.rootTreeId) || !uint(value.offset) || !Array.isArray(value.entries) || value.entries.length>request.limit || request.limit<1 || request.limit>200 || !Number.isSafeInteger(request.limit))return fail();
  if(value.offset!==(previous ? previous.offset+previous.entries.length : 0) || (!!previous !== (request.cursor!==null)) || (previous && (previous.nextCursor!==request.cursor || previous.rootTreeId!==value.rootTreeId || previous.stepId!==value.stepId)))return fail();
  if(value.nextCursor!==null && (!text(value.nextCursor) || value.entries.length===0 || value.nextCursor===request.cursor))return fail();
  const seen=new Set<string>();
  for(const entry of value.entries){
    if(!object(entry) || Object.keys(entry).length!==5 || !["path","target","executable","contentReadable","writable"].every(k=>k in entry) || !path(entry.path) || seen.has(entry.path) || !object(entry.target) || typeof entry.executable!=="boolean" || typeof entry.contentReadable!=="boolean" || typeof entry.writable!=="boolean" || (entry.writable && !entry.contentReadable))return fail();
    const ids:Record<string,string>={file:"fileId",tree:"treeId",conflict:"conflictId",symlink:"symlinkId",tombstone:"tombstoneId"};
    const id=ids[String(entry.target.kind)];if(!id || Object.keys(entry.target).length!==2 || !hash(entry.target[id]))return fail();
    seen.add(entry.path);
  }
  return value as NativeProjectionPage;
}
