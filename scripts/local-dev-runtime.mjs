import {spawn,spawnSync} from 'node:child_process';
import {closeSync,existsSync,openSync,readFileSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import https from 'node:https';
import {assertLocalEndpoint,dockerEnvironment,parsePrivateJson,regularFile} from './local-dev-state.mjs';

export function localDocker(source){
  const env=dockerEnvironment(source);
  const run=args=>spawnSync('docker',args,{env,encoding:'utf8',windowsHide:true,timeout:15000});
  let prefix=[],endpoint,builder;
  if(source.DOCKER_HOST&&!source.DOCKER_CONTEXT){assertLocalEndpoint(source.DOCKER_HOST);endpoint=source.DOCKER_HOST;builder='default';prefix=['--host',source.DOCKER_HOST];}
  else{
    const selected=source.DOCKER_CONTEXT?{status:0,stdout:source.DOCKER_CONTEXT}:run(['context','show']);
    if(selected.status!==0)throw new Error('Docker is unavailable; start Docker Desktop in Linux containers mode');
    const context=selected.stdout.trim();
    const inspected=run(['context','inspect',context]);
    if(inspected.status!==0)throw new Error('Cannot inspect the selected Docker context');
    endpoint=parsePrivateJson(inspected.stdout)[0]?.Endpoints?.docker?.Host;
    assertLocalEndpoint(endpoint);builder=context;
    prefix=['--context',context];
  }
  const info=run([...prefix,'info','--format','{{json .}}']);
  if(info.status!==0)throw new Error('Start Docker Desktop with Linux containers, then retry npm run dev');
  const engine=parsePrivateJson(info.stdout);
  if(engine.OSType!=='linux'||typeof engine.ID!=='string'||!engine.ID)throw new Error('Local dev requires a Linux Docker engine with a persistent identity');
  const inspection=run([...prefix,'buildx','inspect',builder]);
  if(inspection.status!==0)throw new Error('Cannot inspect the local context builder');
  assertLocalBuilder(inspection.stdout,builder,endpoint);
  return {prefix,engineId:engine.ID,endpoint,builder};
}
export function assertLocalBuilder(text,builder,endpoint){
  const nodes=[...text.matchAll(/^\s*Endpoint:\s*(.+)$/gm)].map(match=>match[1].trim());
  if(!/^Driver:\s+docker\s*$/m.test(text)||nodes.length!==1||![builder,endpoint].includes(nodes[0]))throw new Error('Builds must use the single local Docker context builder; remote/custom builders are refused');
}
export function composeArguments(root,directory,state,...args){
  return ['compose','--env-file',join(directory,'compose.env'),'-p',state.project,'-f',join(root,'deploy/selfhost/compose.yml'),'-f',join(directory,'override.yml'),...args];
}
export function dockerCommand(root,directory,source,state,prefix,args,{capture=false}={}){
  return new Promise((resolve,reject)=>{
    const log=join(directory,'docker.log');
    if(existsSync(log))regularFile(log);
    const fd=openSync(log,'a',0o600);
    const env=dockerEnvironment(source,state);
    // Compose's Buildx subprocess must inherit the same explicitly selected daemon.
    if(prefix[0]==='--context'){env.DOCKER_CONTEXT=prefix[1];delete env.DOCKER_HOST;}
    if(prefix[0]==='--host'){env.DOCKER_HOST=prefix[1];delete env.DOCKER_CONTEXT;}
    const child=spawn('docker',[...prefix,...args],{cwd:root,env,windowsHide:true,stdio:['ignore',capture?'pipe':fd,fd]});
    let output='',settled=false;
    const finish=(error)=>{if(settled)return;settled=true;closeSync(fd);if(error)reject(error);else resolve(output.trim());};
    if(capture)child.stdout.on('data',chunk=>{output+=chunk.toString();if(output.length>1024*1024){child.kill();finish(new Error('Docker response exceeds local dev limit'));}});
    child.once('error',()=>finish(new Error('Cannot start Docker; check that Docker Desktop is running')));
    child.once('close',code=>finish(code===0?null:new Error(`Docker command failed (${code}); private diagnostic: ${log}. Data and volumes are preserved.`)));
  });
}
export function localRequest(origin,ca,method,route,body,token){
  return new Promise((resolve,reject)=>{
    const bytes=body===undefined?undefined:Buffer.from(JSON.stringify(body));
    const headers={Origin:origin};
    if(bytes){headers['Content-Type']='application/json';headers['Content-Length']=bytes.length;}
    if(token)headers.Authorization=`Bearer ${token}`;
    const req=https.request(origin+route,{method,ca,rejectUnauthorized:true,headers,timeout:15000},res=>{
      const chunks=[];let count=0;
      res.on('data',chunk=>{count+=chunk.length;if(count>1024*1024)req.destroy(new Error('Local dev response too large'));else chunks.push(chunk);});
      res.on('end',()=>resolve({status:res.statusCode,text:Buffer.concat(chunks).toString('utf8')}));
    });
    req.on('error',()=>reject(new Error('Local HTTPS connection failed; check private Docker diagnostics and the saved CA')));
    req.on('timeout',()=>req.destroy());req.end(bytes);
  });
}
export async function bootstrapAdmin(directory,state,ca){
  const origin=`https://localhost:${state.port}`,marker=join(directory,'bootstrapped.json');
  if(existsSync(marker)){
    if(regularFile(marker).size>1024||parsePrivateJson(readFileSync(marker,'utf8')).project!==state.project)throw new Error('Invalid local bootstrap marker');
    return;
  }
  const credentials={login:state.login,password:state.secrets.admin_password};
  let response=await localRequest(origin,ca,'POST','/v3/auth/login',credentials);
  if(response.status!==200)response=await localRequest(origin,ca,'POST','/v3/auth/bootstrap',{...credentials,secret:state.secrets.bootstrap_secret,display_name:'Administrateur local'});
  if(response.status!==200)throw new Error('Cannot establish the saved local administrator; state and volumes were preserved');
  const token=parsePrivateJson(response.text).access_token;
  if(typeof token!=='string'||!token)throw new Error('Invalid local bootstrap response');
  const logout=await localRequest(origin,ca,'POST','/v3/auth/logout',undefined,token);
  if(logout.status!==204)throw new Error('Cannot revoke the temporary setup session; retry startup');
  writeFileSync(marker,JSON.stringify({project:state.project})+'\n',{flag:'wx',mode:0o600});
}
