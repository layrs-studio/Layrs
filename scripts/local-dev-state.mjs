import {randomBytes} from 'node:crypto';
import {chmodSync,closeSync,existsSync,fsyncSync,linkSync,lstatSync,mkdirSync,openSync,readFileSync,readdirSync,unlinkSync,writeFileSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import {join,resolve} from 'node:path';
import {createServer} from 'node:net';
import {fileURLToPath} from 'node:url';

const schema='layrs.local-development.v1';
const secretNames=['postgres_password','runtime_password','signing_key','bootstrap_secret','admin_password'];
const mountedSecretNames=['postgres_password','runtime_password','signing_key','bootstrap_secret','maintenance_database_url','runtime_database_url'];
export function stateDirectory(root,instance='default'){
  if(!/^[a-z0-9][a-z0-9-]{0,39}$/.test(instance))throw new Error('LAYRS_DEV_INSTANCE must contain 1-40 lowercase letters, digits or hyphens');
  return join(root,'.layrs-local',`native-dev-${instance}`);
}
export function regularFile(path){
  const stat=lstatSync(path);
  if(stat.isSymbolicLink()||!stat.isFile()||stat.nlink!==1)throw new Error(`Expected an unlinked regular local file: ${path}`);
  return stat;
}
export function privateDirectory(directory){
  const parent=resolve(directory,'..');
  for(const path of [parent,directory]){
    if(!existsSync(path))mkdirSync(path,{mode:0o700});
    const stat=lstatSync(path);
    if(stat.isSymbolicLink()||!stat.isDirectory())throw new Error('Local development state must not use a link or junction');
  }
  for(const name of readdirSync(directory))regularFile(join(directory,name));
  if(process.platform==='win32'){
    const script=fileURLToPath(new URL('./local-dev-private.ps1',import.meta.url));
    const secured=spawnSync('powershell.exe',['-NoProfile','-NonInteractive','-File',script,'-Directory',directory],{encoding:'utf8',windowsHide:true});
    if(secured.status!==0)throw new Error('Cannot protect local development credentials with Windows ACLs');
  }else{
    chmodSync(directory,0o700);
    for(const name of readdirSync(directory))chmodSync(join(directory,name),mountedSecretNames.includes(name)?0o644:0o600);
  }
}
export function lockState(directory){
  const path=join(directory,'command.lock');
  let fd;
  try{fd=openSync(path,'wx',0o600);}catch(error){
    if(error.code==='EEXIST')throw new Error(`Another dev command owns ${path}. After an interrupted command, verify its recorded process has stopped before removing this lock.`);
    throw error;
  }
  writeFileSync(fd,JSON.stringify({pid:process.pid,createdAt:new Date().toISOString()})+'\n');
  closeSync(fd);
  return()=>unlinkSync(path);
}
export function createState(port,docker){
  const id=randomBytes(12).toString('hex');
  return {schema,project:`layrs-local-${id}`,deploymentId:`deployment:local-${id}`,storeId:`store:local-${id}`,docker,port,login:'admin@layrs.local',secrets:Object.fromEntries(secretNames.map(name=>[name,randomBytes(32).toString('hex')]))};
}
export function validateState(state){
  const id=typeof state?.project==='string'?state.project.match(/^layrs-local-([0-9a-f]{24})$/)?.[1]:null;
  if(state?.schema!==schema||!id||state.deploymentId!==`deployment:local-${id}`||state.storeId!==`store:local-${id}`||!Number.isSafeInteger(state.port)||state.port<1024||state.port>65535||state.login!=='admin@layrs.local'||secretNames.some(name=>!(/^[0-9a-f]{64}$/).test(state.secrets?.[name])))throw new Error('Invalid local development state; preserve it and its Docker volumes for recovery');
  if(typeof state.docker?.engineId!=='string'||!(/^[a-zA-Z0-9:_-]{1,128}$/).test(state.docker.engineId))throw new Error('Missing Docker engine binding in local development state');
  assertLocalEndpoint(state.docker.endpoint);
  return state;
}
export function readState(directory){
  const path=join(directory,'state.json');
  if(!existsSync(path))return null;
  if(regularFile(path).size>16384)throw new Error('Oversized local development state');
  return validateState(parsePrivateJson(readFileSync(path,'utf8')));
}
export function parsePrivateJson(value){try{return JSON.parse(value);}catch{throw new Error('Invalid private configuration or response JSON; contents are withheld');}}
export function assertSameEngine(state,docker){
  if(state.docker.engineId!==docker.engineId||state.docker.endpoint!==docker.endpoint)throw new Error('Docker engine differs from the saved instance. Restore its original local context; no volumes were changed.');
}
export function writeNewState(directory,state){
  validateState(state);
  const temporary=join(directory,`state-${randomBytes(8).toString('hex')}.tmp`);
  const fd=openSync(temporary,'wx',0o600);
  try{writeFileSync(fd,JSON.stringify(state,null,2)+'\n');fsyncSync(fd);}finally{closeSync(fd);}
  // Publish complete bytes without replacing another manifest.
  try{linkSync(temporary,join(directory,'state.json'));}finally{unlinkSync(temporary);}
}
export function ensureExactFile(path,value,mode=0o600){
  if(existsSync(path)){
    if(regularFile(path).size!==Buffer.byteLength(value)||readFileSync(path,'utf8')!==value)throw new Error(`Local configuration differs from its saved state: ${path}`);
  }else writeFileSync(path,value,{flag:'wx',mode});
}
export function secretFiles(state){
  const s=state.secrets;
  return {postgres_password:s.postgres_password,runtime_password:s.runtime_password,signing_key:s.signing_key,bootstrap_secret:s.bootstrap_secret,
    maintenance_database_url:`postgres://layrs_maintenance:${s.postgres_password}@postgres:5432/layrs`,runtime_database_url:`postgres://layrs_runtime:${s.runtime_password}@postgres:5432/layrs`};
}
export function composeOverride(directory,state,caddy){
  const file=path=>path.replaceAll('\\','/');
  const secrets=Object.keys(secretFiles(state)).map(name=>`  ${name}: {file: ${JSON.stringify(file(join(directory,name)))}}`).join('\n');
  return `services:\n  proxy:\n    ports: !override ["127.0.0.1:${state.port}:443"]\n    volumes:\n      - ${JSON.stringify(`${file(caddy)}:/etc/caddy/Caddyfile:ro`)}\nsecrets:\n${secrets}\n`;
}
export function dockerEnvironment(source,state){
  const env=Object.fromEntries(Object.entries(source).filter(([key])=>!(/^(COMPOSE_|LAYRS_|BUILDX_|BUILDKIT_|DOCKER_BUILDKIT$)/i).test(key)));
  env.COMPOSE_BAKE='false';env.DOCKER_BUILDKIT='1';
  if(state)Object.assign(env,{LAYRS_HOST:`localhost:${state.port}`,LAYRS_DEPLOYMENT_ID:state.deploymentId,LAYRS_STORE_ID:state.storeId,LAYRS_IMAGE_TAG:state.project});
  return env;
}
export function assertLocalEndpoint(endpoint){
  if(typeof endpoint!=='string'||!(/^(npipe:\/\/\/\/\.\/pipe\/[^\s]+|unix:\/\/\/[^\s]+)$/).test(endpoint))throw new Error('Local dev requires a local Docker socket (Docker Desktop or a Unix socket), not a remote/TCP context');
}
export async function choosePort(explicit){
  const ports=explicit===undefined?[19443,19444,19445,19446]:[Number(explicit)];
  for(const port of ports){
    if(!Number.isSafeInteger(port)||port<1024||port>65535)throw new Error('LAYRS_DEV_PORT must be an integer between 1024 and 65535');
    const free=await new Promise(resolve=>{const server=createServer();server.once('error',()=>resolve(false));server.listen(port,'127.0.0.1',()=>server.close(()=>resolve(true)));});
    if(free)return port;
  }
  throw new Error('No free local HTTPS port; set LAYRS_DEV_PORT for a new instance');
}
