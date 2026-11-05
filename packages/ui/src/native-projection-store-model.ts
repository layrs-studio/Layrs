import type {NativeProjectFile,NativeProjectFilesSnapshot} from "./native-project-files-model";
import {nativeProjectionReady} from "./native-project-files-model";
import type {NativeProjectionTarget} from "./native-remote-project-files-model";
import { projectionSelectionMatcher, validateProjectionSelection, type NativeProjectionSelection } from "./native-projection-selection";

export type NativeProjectionIdentity={storeId:string;worktreeId:string;spaceId:string;origin:string;principalId:string};
export type NativeStoreProjection={generation:number;digest:string;snapshot:{schemaVersion:"layrs.projection-snapshot.v1";baseStepId:string;baseRootTreeId:string;observedPolicySetId:string;observedAuthorizationEpoch:number;pathSemantics:"case_sensitive"|"portable_windows";entries:{path:string;target:NativeProjectionTarget|null;executable:boolean;writable:boolean;state:"materialized"|"reserved"|"unavailable"|"absent"}[]}};
export type NativeProjectionStatus={identity:NativeProjectionIdentity;projection:NativeStoreProjection;selection?:NativeProjectionSelection;offline?:{shared:boolean};latestCaptureImported?:boolean;needsRepair:boolean;capabilities:{capture:boolean;publish:boolean;namespace:boolean;receive?:boolean;restore?:boolean};latestCapture?:NativeCapturedProjectionPatch|null;pendingPublication?:{intent?:"advance"|"candidate";sequence:number;layerId:string;requestDigest:string;receiptRecorded:boolean}|null};
export type NativeCapturedProjectionPatch={schemaVersion:string;sequence:number;projectionGeneration:number;projectionDigest:string;identity:NativeProjectionIdentity;changes:{path:string;expected:{fileId:string;executable:boolean}|null;replacement:{fileId:string;executable:boolean;bytes:number}|null}[];digest:string};
const hash=(v:unknown)=>typeof v==="string"&&/^blake3:[a-f0-9]{64}$/.test(v);
const stateDigest=(v:unknown)=>typeof v==="string"&&/^[a-f0-9]{64}$/.test(v);
const text=(v:unknown)=>typeof v==="string"&&!!v&&v.length<=4096;
// Rust String/BTreeMap ordering is UTF-8 byte order, not JS UTF-16 code-unit order.
function pathCompare(a:string,b:string):number{const x=new TextEncoder().encode(a),y=new TextEncoder().encode(b);for(let i=0;i<Math.min(x.length,y.length);i++)if(x[i]!==y[i])return x[i]-y[i];return x.length-y.length;}
export function projectionStoreFiles(status:NativeProjectionStatus):NativeProjectFilesSnapshot{
  if(status?.offline!==undefined&&(!status.offline||typeof status.offline.shared!=="boolean")||status?.latestCaptureImported!==undefined&&typeof status.latestCaptureImported!=="boolean")throw new Error("État hors ligne invalide.");
  const identity=status?.identity,version=status?.projection,s=version?.snapshot;
  if(!identity||!version||!s||![identity.storeId,identity.worktreeId,identity.spaceId,identity.origin,identity.principalId].every(text)||!Number.isSafeInteger(version.generation)||version.generation<0||!stateDigest(version.digest)||s.schemaVersion!=="layrs.projection-snapshot.v1"||![s.baseStepId,s.baseRootTreeId,s.observedPolicySetId].every(hash)||!Number.isSafeInteger(s.observedAuthorizationEpoch)||s.observedAuthorizationEpoch<=0||!Array.isArray(s.entries)||s.entries.length>100000||!["case_sensitive","portable_windows"].includes(s.pathSemantics)||typeof status.needsRepair!=="boolean")throw new Error("Registre de projection invalide ou incomplet.");
  // Older All checkouts did not expose intent; malformed explicit intent fails closed.
  const selection = validateProjectionSelection(status.selection === undefined ? {kind:"all"} : status.selection, s.pathSemantics);
  const includes = projectionSelectionMatcher(selection);
  const entries:NativeProjectFile[]=[];let incomplete=false;let previous="";
  const capture=status.latestCapture&&projectionCaptureMatches(status,status.latestCapture)?status.latestCapture:null;
  const captured=new Map(capture?.changes.map(change=>[change.path,change]));
  for(const entry of s.entries){
    if(!entry||!text(entry.path)||pathCompare(previous,entry.path)>=0||typeof entry.writable!=="boolean"||typeof entry.executable!=="boolean")throw new Error("Entrées de projection invalides.");previous=entry.path;
    const target=entry.target;
    const included = includes(entry.path);
    if (!included && ["materialized", "absent"].includes(entry.state)) throw new Error("Un chemin exclu ne peut pas être matérialisé ni déclaré absent.");
    if(entry.state==="absent"){
      if(target!==null)throw new Error("Absence contradictoire.");
      // Only the explicit writable observation recorded by the authenticated
      // classification flow can accompany absence. Absence alone grants nothing.
      const local=captured.get(entry.path);
      if(entry.writable&&local?.expected===null&&local.replacement)entries.push({path:entry.path,kind:"file",access:"read_write",contentState:"materialized",objectId:local.replacement.fileId});
      else if(entry.writable)entries.push({path:entry.path,kind:"absent",access:"read_write",contentState:"absent"});
      else {incomplete=true;entries.push({path:entry.path,kind:"absent",access:"unknown",contentState:"unavailable"});}
      continue;
    }
    const ids:Record<string,string>={file:"fileId",tree:"treeId",symlink:"symlinkId",conflict:"conflictId",tombstone:"tombstoneId"};
    const id=target&&ids[target.kind];
    if(!id||!target||!hash((target as unknown as Record<string,unknown>)[id]))throw new Error("Cible canonique de projection inconnue.");
    if(entry.state==="reserved"&&entry.writable)throw new Error("Chemin réservé déclaré modifiable.");
    if(entry.executable&&target.kind!=="file")throw new Error("Mode exécutable incohérent.");
    const kind=target.kind==="tree"?"directory":target.kind;
    let access:NativeProjectFile["access"]="unknown",contentState:NativeProjectFile["contentState"]="unavailable";
    if(entry.state==="materialized"){access=entry.writable?"read_write":"read_only";contentState="materialized";}
    else if(entry.state==="reserved"){access="denied";contentState="reserved";}
    else if(entry.state==="unavailable"){access=entry.writable?"read_write":"read_only";contentState=included?"not_materialized":"excluded";}
    else incomplete=true;
    entries.push({path:entry.path,kind,access,contentState,selection:included?"included":"excluded",objectId:(target as unknown as Record<string,string>)[id]});
  }
  const result:NativeProjectFilesSnapshot={schemaVersion:"layrs.project-files.v1",anchor:{origin:identity.origin,principalId:identity.principalId,spaceId:identity.spaceId,stepId:s.baseStepId,rootTreeId:s.baseRootTreeId,policySetId:s.observedPolicySetId,authorizationEpoch:s.observedAuthorizationEpoch,projectionId:identity.worktreeId,generation:version.generation},state:status.needsRepair?"stale":incomplete?"incomplete":"ready",pathPlatform:s.pathSemantics==="portable_windows"?"windows":"posix",caseMode:s.pathSemantics==="portable_windows"?"insensitive":"sensitive",namespaceComplete:true,entries,reservedPaths:[]};
  if(result.state==="ready"&&!nativeProjectionReady(result))throw new Error("Registre de chemins incohérent ou ambigu.");
  return result;
}

export function projectionCaptureMatches(status:NativeProjectionStatus,patch:NativeCapturedProjectionPatch):boolean{
  if(!patch||patch.schemaVersion!=="layrs.projection-capture.v1"||!hash(patch.digest)||!Number.isSafeInteger(patch.sequence)||patch.sequence<1||patch.projectionGeneration!==status.projection.generation||patch.projectionDigest!==status.projection.digest||!patch.identity||!["storeId","worktreeId","spaceId","origin","principalId"].every(key=>patch.identity[key as keyof NativeProjectionIdentity]===status.identity[key as keyof NativeProjectionIdentity])||!Array.isArray(patch.changes)||patch.changes.length>100000)return false;
  let previous="";
  for(const change of patch.changes){
    if(!change||!text(change.path)||pathCompare(previous,change.path)>=0||change.expected===null&&change.replacement===null)return false;previous=change.path;
    for(const file of [change.expected,change.replacement])if(file!==null&&(!file||!hash(file.fileId)||typeof file.executable!=="boolean"))return false;
    if(change.replacement!==null&&(!Number.isSafeInteger(change.replacement.bytes)||change.replacement.bytes<0))return false;
  }
  return true;
}
