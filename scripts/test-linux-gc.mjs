// Real PostgreSQL + pg_dump/restore + Linux GC, only disposable containers.
import {spawnSync} from 'node:child_process';
import {randomBytes,randomUUID} from 'node:crypto';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import assert from 'node:assert/strict';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const docker=[];
if(process.env.LAYRS_TEST_DOCKER_CONFIG)docker.push('--config',process.env.LAYRS_TEST_DOCKER_CONFIG);
if(process.env.LAYRS_TEST_DOCKER_HOST)docker.push('-H',process.env.LAYRS_TEST_DOCKER_HOST);
function run(args,{capture=false,env={}}={}){
  const result=spawnSync('docker',[...docker,...args],{encoding:'utf8',windowsHide:true,maxBuffer:64*1024*1024,env:{...process.env,...env},stdio:['ignore',capture?'pipe':'inherit','inherit']});
  if(result.error||result.status!==0)throw new Error(`Docker GC recipe failed (${result.status??'spawn'})`);
  return result.stdout??'';
}
const build=process.env.LAYRS_LINUX_BUILD_VOLUME??'layrs-linux-gc-build';
const registry=process.env.LAYRS_LINUX_CARGO_VOLUME??'layrs-linux-gc-cargo';
for(const name of [build,registry])assert(/^[a-zA-Z0-9][a-zA-Z0-9_.-]+$/.test(name));
const mounts=['--mount',`type=bind,source=${root},target=/src,readonly`,'--mount',`type=volume,source=${build},target=/build`,
  '--mount',process.env.LAYRS_LINUX_CARGO_REGISTRY?`type=bind,source=${path.resolve(process.env.LAYRS_LINUX_CARGO_REGISTRY)},target=/usr/local/cargo/registry`:`type=volume,source=${registry},target=/usr/local/cargo/registry`];
if(process.env.LAYRS_LINUX_CARGO_CACHE)mounts.push('--mount',`type=bind,source=${path.resolve(process.env.LAYRS_LINUX_CARGO_CACHE)},target=/usr/local/cargo/registry/cache,readonly`);
if(process.env.LAYRS_LINUX_CARGO_INDEX)mounts.push('--mount',`type=bind,source=${path.resolve(process.env.LAYRS_LINUX_CARGO_INDEX)},target=/usr/local/cargo/registry/index,readonly`);
const compiled=run(['run','--rm','--pull=never','--network','none',...mounts,'-e','CARGO_TARGET_DIR=/build','-e','RUSTUP_TOOLCHAIN=1.90.0','-w','/src',process.env.LAYRS_LINUX_TEST_IMAGE??'layrs-linux-tests:validation',
  'cargo','test','--offline','--locked','-p','layrs-server','--test','native_gc','--no-run','--message-format=json'],{capture:true});
const artifact=compiled.split('\n').filter(Boolean).map(line=>JSON.parse(line)).find(value=>value.reason==='compiler-artifact'&&value.target.name==='native_gc'&&value.executable);
assert(artifact&&/^\/build\/debug\/deps\/native_gc-[a-f0-9]+$/.test(artifact.executable));
const name=`layrs-gc-${randomUUID()}`;
const password=randomBytes(32).toString('hex');
const pgImage='postgres:17.6-bookworm';
let container;
try{
  container=run(['run','-d','--pull=never','--network','none','--name',name,'--label','layrs.validation=offline-gc','--tmpfs','/var/lib/postgresql/data:rw,nosuid,nodev,size=256m','-e','POSTGRES_PASSWORD',pgImage],{capture:true,env:{POSTGRES_PASSWORD:password}}).trim();
  assert(/^[a-f0-9]{64}$/.test(container));
  let ready=false;
  for(let attempt=0;attempt<60;attempt++){
    const probe=spawnSync('docker',[...docker,'exec',container,'pg_isready','-U','postgres'],{stdio:'ignore',windowsHide:true});
    if(probe.status===0){ready=true;break;}
    await new Promise(resolve=>setTimeout(resolve,250));
  }
  assert(ready,'isolated PostgreSQL must become ready');
  const output=run(['run','--rm','--pull=never','--network',`container:${container}`,'--mount',`type=volume,source=${build},target=/build,readonly`,'--mount',`type=bind,source=${root},target=/src,readonly`,
    '-e','LAYRS_TEST_DATABASE_URL','-e','LAYRS_TEST_DATABASE_ISOLATED=1','--entrypoint',artifact.executable,pgImage,'--ignored','--nocapture','--test-threads=1'],
    {capture:true,env:{LAYRS_TEST_DATABASE_URL:`postgresql://postgres:${password}@127.0.0.1:5432/postgres`}});
  process.stdout.write(output);
  assert.match(output,/test result: ok\. 1 passed; 0 failed; 0 ignored;/);
}finally{
  if(container){
    const label=run(['inspect','--format','{{ index .Config.Labels "layrs.validation" }}',container],{capture:true}).trim();
    assert.equal(label,'offline-gc');
    run(['rm','-f',container],{capture:true});
  }
}
console.log('Linux offline GC recipe passed; isolated PostgreSQL and fixture files removed.');
