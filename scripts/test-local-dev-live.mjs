// Explicit, opt-in qualification of an already started disposable launcher instance.
import assert from 'node:assert/strict';
import {readFileSync,unlinkSync} from 'node:fs';
import {spawn} from 'node:child_process';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {readState,stateDirectory} from './local-dev-state.mjs';
import {composeArguments,dockerCommand,localDocker,localRequest} from './local-dev-runtime.mjs';

const root=fileURLToPath(new URL('../',import.meta.url));
const instance=process.env.LAYRS_DEV_INSTANCE;
assert(/^launcher-(test|final)-/.test(instance??''),'Use an explicitly disposable launcher-test-* instance');
const directory=stateDirectory(root,instance),state=readState(directory),engine=localDocker(process.env);
const ca=readFileSync(join(directory,'root.crt'),'utf8'),origin=`https://localhost:${state.port}`;
const saved=readFileSync(join(directory,'state.json'),'utf8');
const docker=args=>dockerCommand(root,directory,process.env,state,engine.prefix,args,{capture:true});
const compose=(...args)=>docker(composeArguments(root,directory,state,...args));
const request=(method,route,body,token)=>localRequest(origin,ca,method,route,body,token);
const login=async()=>{
  const result=await request('POST','/v3/auth/login',{login:state.login,password:state.secrets.admin_password});
  assert.equal(result.status,200);return JSON.parse(result.text).access_token;
};
const checks=[];
function record(message){checks.push(message);console.log(`PASS: ${message}`);}
async function command(...args){
  const output=await new Promise((done,fail)=>{
    const child=spawn(process.execPath,[join(root,'scripts/dev.mjs'),...args],{cwd:root,env:process.env,windowsHide:true});
    let output='';child.stdout.on('data',chunk=>output+=chunk);child.stderr.on('data',chunk=>output+=chunk);
    child.once('error',fail);child.once('close',code=>code===0?done(output):fail(new Error(`Launcher exited ${code}; inspect private instance diagnostics`)));
  });
  for(const value of Object.values(state.secrets))assert(!output.includes(value),'credential exposed');
}
let token;
try{
  assert.equal((await request('GET','/readyz')).status,200);
  const html=await request('GET','/');assert.equal(html.status,200);assert.match(html.text,/<html/i);
  const asset=html.text.match(/src="([^" ]+\.js)"/);assert(asset);
  assert.equal((await request('GET',asset[1])).status,200);
  record('HTTPS readiness, Web HTML and JavaScript bundle');
  const old=process.env.NODE_TLS_REJECT_UNAUTHORIZED;
  try{process.env.NODE_TLS_REJECT_UNAUTHORIZED='0';await assert.rejects(localRequest(origin,[],'GET','/readyz'));}
  finally{if(old===undefined)delete process.env.NODE_TLS_REJECT_UNAUTHORIZED;else process.env.NODE_TLS_REJECT_UNAUTHORIZED=old;}
  record('wrong CA refused even when environment disables default TLS checks');
  const proxy=await compose('ps','-q','proxy');
  const bindings=JSON.parse(await docker(['inspect',proxy,'--format','{{json .HostConfig.PortBindings}}']));
  assert.deepEqual(bindings,{'443/tcp':[{HostIp:'127.0.0.1',HostPort:String(state.port)}]});
  record('only loopback HTTPS port published');
  token=await login();
  const slug=`check-${Date.now()}`;
  const provision=await request('POST','/v3/workspaces/provision',{workspaceSlug:slug,spaceSlug:'project',name:'Launcher persistence qualification'},token);
  assert.equal(provision.status,201);const scope=JSON.parse(provision.text);assert(scope.spaceId&&scope.layerId);
  assert.equal((await request('POST','/v3/auth/logout',undefined,token)).status,204);token=null;
  await command('--down');
  await docker(['volume','inspect',...['postgres','cas','caddy_data','caddy_config'].map(name=>`${state.project}_${name}`)]);
  await command('--no-build');
  assert.equal(readFileSync(join(directory,'state.json'),'utf8'),saved);
  assert.equal(readFileSync(join(directory,'root.crt'),'utf8'),ca);
  token=await login();const restored=await request('GET',`/v3/spaces/${encodeURIComponent(scope.spaceId)}/bootstrap`,undefined,token);
  assert.equal(restored.status,200);assert.equal(JSON.parse(restored.text).spaceId,scope.spaceId);
  assert.equal((await request('POST','/v3/auth/logout',undefined,token)).status,204);token=null;
  record('down/restart preserves four volumes, CA, credentials and provisioned project');
  const marker=join(directory,'bootstrapped.json');
  unlinkSync(marker);await command('--no-build');
  assert.equal(JSON.parse(readFileSync(marker,'utf8')).project,state.project);
  record('lost bootstrap acknowledgement recovered through existing login');
  await command('--info');record('launcher output contains no generated credentials');
}finally{
  if(token)await request('POST','/v3/auth/logout',undefined,token);
  await command('--down');
}
console.log(JSON.stringify({instance,project:state.project,origin,checks,volumes:'retained',containers:'stopped',date:new Date().toISOString()},null,2));
