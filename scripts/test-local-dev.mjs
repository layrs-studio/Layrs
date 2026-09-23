import assert from 'node:assert/strict';
import {mkdtempSync,mkdirSync,readFileSync,writeFileSync,linkSync,unlinkSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {spawnSync} from 'node:child_process';
import {test} from 'node:test';
import {assertLocalEndpoint,assertSameEngine,composeOverride,createState,dockerEnvironment,ensureExactFile,lockState,parsePrivateJson,privateDirectory,readState,regularFile,stateDirectory,writeNewState} from './local-dev-state.mjs';
import {assertLocalBuilder,composeArguments} from './local-dev-runtime.mjs';

const docker={engineId:'fixture-engine',endpoint:'unix:///var/run/docker.sock'};
function fixture(run){
  const directory=mkdtempSync(join(tmpdir(),'layrs-dev-test-'));
  try{return run(directory);}finally{rmSync(directory,{recursive:true,force:true});}
}
test('manifest and derived secrets survive restart without replacement',()=>fixture(directory=>{
  const state=createState(19443,docker);writeNewState(directory,state);
  assert.deepEqual(readState(directory),state);
  assert.throws(()=>writeNewState(directory,createState(19443,docker)));
  assert.deepEqual(readState(directory),state);
  const secret=join(directory,'secret');ensureExactFile(secret,'original');ensureExactFile(secret,'original');
  assert.throws(()=>ensureExactFile(secret,'replacement'));
  assert.equal(readFileSync(secret,'utf8'),'original');
}));
test('corrupt private JSON does not disclose its contents',()=>fixture(directory=>{
  const sentinel='credential-must-never-appear';
  writeFileSync(join(directory,'state.json'),`{"secret":"${sentinel}`);
  for(const run of [()=>readState(directory),()=>parsePrivateJson(sentinel)])
    assert.throws(run,error=>!error.message.includes(sentinel)&&error.message.includes('withheld'));
}));
test('instance traversal, remote daemons and switched engines are rejected',()=>{
  for(const name of ['../other','a/b','A',''])assert.throws(()=>stateDirectory('/repo',name));
  for(const endpoint of ['ssh://host','tcp://localhost:2375','https://host',undefined])assert.throws(()=>assertLocalEndpoint(endpoint));
  assertLocalEndpoint(docker.endpoint);assertLocalEndpoint('npipe:////./pipe/dockerDesktopLinuxEngine');
  const state=createState(19443,docker);assertSameEngine(state,docker);
  assert.throws(()=>assertSameEngine(state,{...docker,engineId:'other'}));
  assert.throws(()=>assertSameEngine(state,{...docker,endpoint:'unix:///other.sock'}));
});
test('ambient compose and builder overrides cannot select another project or builder',()=>{
  const env=dockerEnvironment({PATH:'retained',COMPOSE_FILE:'evil',COMPOSE_PROJECT_NAME:'production',LAYRS_HOST:'remote',BUILDX_BUILDER:'remote',BUILDKIT_HOST:'tcp://remote',COMPOSE_BAKE:'true'},createState(19443,docker));
  assert.equal(env.PATH,'retained');assert.equal(env.COMPOSE_FILE,undefined);
  assert.equal(env.COMPOSE_PROJECT_NAME,undefined);assert.equal(env.BUILDX_BUILDER,undefined);
  assert.equal(env.BUILDKIT_HOST,undefined);assert.equal(env.COMPOSE_BAKE,'false');
  assert.equal(env.LAYRS_HOST,'localhost:19443');
  assertLocalBuilder('Driver: docker\nEndpoint: local\n','local',docker.endpoint);
  for(const text of ['Driver: docker-container\nEndpoint: local','Driver: docker\nEndpoint: remote','Driver: docker\nEndpoint: local\nEndpoint: remote'])
    assert.throws(()=>assertLocalBuilder(text,'local',docker.endpoint));
});
test('routing is loopback-only and shutdown retains volumes',()=>{
  const state=createState(19443,docker),args=composeArguments('/repo','/state',state,'down');
  assert.equal(args.at(-1),'down');assert(!args.includes('--volumes'));assert(!args.includes('--remove-orphans'));
  assert(args.includes(state.project));assert(args.includes('--env-file'));
  assert.match(composeOverride('/state',state,'/state/Caddyfile'),/ports: !override \["127\.0\.0\.1:19443:443"\]/);
});
test('lock excludes concurrent operations and hard links are refused',()=>fixture(directory=>{
  const release=lockState(directory);assert.throws(()=>lockState(directory));release();lockState(directory)();
  const original=join(directory,'original'),linked=join(directory,'linked');writeFileSync(original,'data');
  linkSync(original,linked);assert.throws(()=>regularFile(linked));assert.throws(()=>privateDirectory(directory));unlinkSync(linked);
  assert.equal(readFileSync(original,'utf8'),'data');
}));
test('private directory removes pre-existing broad Windows read grants',()=>fixture(parent=>{
  const directory=join(parent,'private');mkdirSync(directory);const file=join(directory,'state.json');writeFileSync(file,'{}');
  if(process.platform==='win32'){
    for(const path of [directory,file])assert.equal(spawnSync('icacls',[path,'/grant','*S-1-5-32-545:(R)'],{windowsHide:true}).status,0);
  }
  privateDirectory(directory);privateDirectory(directory);
  assert.equal(readFileSync(file,'utf8'),'{}');
  // privateDirectory itself reads back every ACL and rejects any other allowed SID.
}));
