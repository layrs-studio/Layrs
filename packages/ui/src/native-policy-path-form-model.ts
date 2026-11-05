export type NativePathRule = {effect:"allow"|"deny";selector:{kind:"exact"|"subtree";path:string}};
export type NativePathPermission = {default:"allow"|"deny";rules:NativePathRule[]};
export type NativePathPolicy = {principalIds:string[];read:NativePathPermission;write:NativePathPermission};
export type NativePathPolicyRules = {schemaVersion:"layrs.space-policy-rules.v3";rules:unknown[];pathPolicies:NativePathPolicy[]};
export type NativePolicyRuleEntry = {policyId:string;policyVersionId:string;policyProductObjectId:string;rules:unknown};
export type NativePolicyRulesRead = {principalId:string;spaceId:string;policySetId:string;authorizationEpoch:number;policies:NativePolicyRuleEntry[]};
const object=(v:unknown):v is Record<string,unknown>=>!!v&&typeof v==="object"&&!Array.isArray(v);
const closed=(v:Record<string,unknown>,keys:string[])=>Object.keys(v).length===keys.length&&keys.every(k=>k in v);
const effect=(v:unknown)=>v==="allow"||v==="deny";
const repoPath=(v:unknown):v is string=>typeof v==="string"&&!!v&&new TextEncoder().encode(v).length<=4096&&!/[\\\x00-\x1f\x7f]/.test(v)&&v.split("/").every(p=>p!==""&&p!=="."&&p!=="..");
export function pathRules(value:unknown):NativePathPolicyRules {
  if(!object(value)||!closed(value,["schemaVersion","rules","pathPolicies"])||value.schemaVersion!=="layrs.space-policy-rules.v3"||!Array.isArray(value.rules)||!Array.isArray(value.pathPolicies)||value.pathPolicies.length<1||value.pathPolicies.length>64)throw new Error("Cette politique nécessite l’éditeur JSON ; le formulaire ne convertit pas les anciens schémas.");
  let total=0;
  for(const policy of value.pathPolicies){
    if(!object(policy)||!closed(policy,["principalIds","read","write"])||!Array.isArray(policy.principalIds)||!policy.principalIds.length||policy.principalIds.length>256||!policy.principalIds.every(p=>typeof p==="string"&&p.startsWith("principal:")))throw new Error("Sujets de politique invalides.");
    for(const permission of [policy.read,policy.write]){
      if(!object(permission)||!closed(permission,["default","rules"])||!effect(permission.default)||!Array.isArray(permission.rules)||permission.rules.length>256)throw new Error("Permissions invalides.");
      total+=permission.rules.length;if(total>1024)throw new Error("Trop de règles.");
      for(const rule of permission.rules)if(!object(rule)||!closed(rule,["effect","selector"])||!effect(rule.effect)||!object(rule.selector)||!closed(rule.selector,["kind","path"])||!["exact","subtree"].includes(String(rule.selector.kind))||!repoPath(rule.selector.path))throw new Error("Règle de chemin invalide.");
    }
  }
  if(new TextEncoder().encode(JSON.stringify(value)).length>64*1024)throw new Error("Règles limitées à 64 Kio.");
  return value as NativePathPolicyRules;
}

/** Changes this member at this exact selector; retains every other subject/default/rule. */
export function changeNativeMemberPathRules(value:unknown|null,principalId:string,path:string,kind:"exact"|"subtree",access:"none"|"read"|"write"):NativePathPolicyRules{
  if(!principalId.startsWith("principal:")||principalId.length<=10||!repoPath(path)||!["exact","subtree"].includes(kind)||!["none","read","write"].includes(access))throw new Error("Sélectionnez un membre et un chemin valides.");
  const input:NativePathPolicyRules=value===null?{schemaVersion:"layrs.space-policy-rules.v3",rules:[],pathPolicies:[]}:JSON.parse(JSON.stringify(pathRules(value)));
  const read:NativePathRule={effect:access==="none"?"deny":"allow",selector:{kind,path}};
  const write:NativePathRule={effect:access==="write"?"allow":"deny",selector:{kind,path}};
  const update=(permission:NativePathPermission,rule:NativePathRule):NativePathPermission=>({...permission,rules:[...permission.rules.filter(r=>r.selector.kind!==kind||r.selector.path!==path),rule]});
  let matched=false;
  input.pathPolicies=input.pathPolicies.flatMap(policy=>{
    if(!policy.principalIds.includes(principalId))return [policy];
    matched=true;
    const others=policy.principalIds.filter(p=>p!==principalId);
    return [...(others.length?[{...policy,principalIds:others}]:[]),{principalIds:[principalId],read:update(policy.read,read),write:update(policy.write,write)}];
  });
  if(!matched)input.pathPolicies.push({principalIds:[principalId],read:{default:"deny",rules:[read]},write:{default:"deny",rules:[write]}});
  return pathRules(input);
}
