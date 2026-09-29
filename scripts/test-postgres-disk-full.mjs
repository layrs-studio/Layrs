// Real PostgreSQL ENOSPC against unchanged qualified release binaries.
// Only the newly labelled container's tmpfs is filled; never an existing service.
import assert from 'node:assert/strict';
import {spawn, spawnSync} from 'node:child_process';
import {randomBytes, randomUUID, createHash} from 'node:crypto';
import {readFileSync, writeFileSync, mkdirSync, mkdtempSync} from 'node:fs';
import https from 'node:https';
import http from 'node:http';
import net from 'node:net';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const started=Date.now(), deadline=started+15*60_000;
const packageRoot = path.resolve(process.env.LAYRS_TEST_NATIVE_PACKAGE ?? '');
assert(process.env.LAYRS_TEST_NATIVE_PACKAGE, 'Set the already qualified native package directory');
const manifest = JSON.parse(readFileSync(path.join(packageRoot, 'manifest.json'), 'utf8'));
const executable = name => {
  const file = name + (process.platform === 'win32' ? '.exe' : '');
  const record = manifest.files.find(entry => entry.path === file);
  assert(record, 'Package executable must be recorded');
  const filename = path.join(packageRoot, file), bytes = readFileSync(filename);
  assert.equal(createHash('sha256').update(bytes).digest('hex'), record.sha256, 'Package executable digest');
  return filename;
};
const cliBinary = executable('layrs'), serverBinary = executable('layrs-server');
const name = `layrs-pg-full-${randomUUID()}`;
assert.match(name, /^layrs-pg-full-[a-f0-9-]{36}$/);
const directory = mkdtempSync(path.join(root, 'tmp', 'pg-disk-full-'));
const workspace = path.join(directory, 'client');
mkdirSync(workspace, {mode: 0o700});
const cas = path.join(directory, 'cas');
const reportFile = path.join(directory, 'report.json');
const pressure = '/var/lib/postgresql/data/layrs-fixture-pressure.bin';
const secret = () => randomBytes(32).toString('hex');
const adminPassword = secret(), runtimePassword = secret(), bootstrapSecret = secret(), loginPassword = secret();
const signingKey = secret();
const dockerPrefix = [];
if (process.env.LAYRS_TEST_DOCKER_CONFIG) dockerPrefix.push('--config', process.env.LAYRS_TEST_DOCKER_CONFIG);
if (process.env.LAYRS_TEST_DOCKER_HOST) dockerPrefix.push('-H', process.env.LAYRS_TEST_DOCKER_HOST);
let container, server, proxy, authenticated = false, origin, ca, adminToken;
const sockets = new Set();
const report = {schemaVersion:'layrs.pg-disk-full.v1', status:'running', packageCommit:manifest.source.commit,
  packageTreeSha256:manifest.source.treeSha256, fixture:name, tmpfsBytes:256 * 1024 * 1024,
  memoryLimitBytes:512 * 1024 * 1024, checks:[], limitations:[
    'Disposable Linux PostgreSQL tmpfs, Windows/native client and server binaries when run on Windows.',
    'No existing service/volume, host disk, power failure, kernel crash, or PostgreSQL WAL-device-only claim.',
    'No container restart: tmpfs must remain alive throughout PostgreSQL recovery.'
  ]};
const save = () => writeFileSync(reportFile, JSON.stringify(report, null, 2) + '\n', {mode:0o600});
const check = label => { report.checks.push(label); save(); console.log(`PASS ${label}`); };
save();
function docker(args, {input, allowFailure=false, env={}}={}) {
  const result = spawnSync('docker', [...dockerPrefix, ...args], {input, encoding:'utf8', windowsHide:true,
    env:{...process.env, ...env}, timeout:90_000, maxBuffer:8*1024*1024});
  if (!allowFailure) assert.equal(result.status, 0, `Docker ${args[0]} failed; output withheld`);
  return result;
}
function sql(query, allowFailure=false) {
  return docker(['exec','-i',container,'psql','-X','-U','postgres','-d','postgres','-At','-v','ON_ERROR_STOP=1'],
    {input:query, allowFailure});
}
function owned() {
  assert.match(container, /^[a-f0-9]{64}$/);
  const label=docker(['inspect','--format','{{ index .Config.Labels "layrs.validation" }}',container]).stdout.trim();
  assert.equal(label, name, 'Cleanup requires the exact private fixture label');
}
async function stop(child) {
  if(child.exitCode!==null||child.signalCode!==null)return true;
  return new Promise(resolve=>{
    const timer=setTimeout(()=>resolve(false),10_000);
    child.once('close',()=>{clearTimeout(timer);resolve(true);});child.kill();
  });
}
function run(binary, args, {env={}, timeout=90_000, stderrHook}={}) {
  timeout=Math.min(timeout,Math.max(1,deadline-Date.now()));
  return new Promise((resolve,reject) => {
    const child=spawn(binary,args,{env:{...process.env,...env},windowsHide:true,stdio:['ignore','pipe','pipe']});
    let stdout='',stderr='',length=0,done=false;
    const fail=message=>{if(done)return;done=true;clearTimeout(timer);stop(child).then(stopped=>reject(new Error(stopped?message:'Child termination failed; fixture retained')));};
    const timer=setTimeout(()=>fail('Fixture child timeout; output withheld'),timeout);
    const collect=(chunk,error)=>{if(done)return;length+=chunk.length;if(length>4*1024*1024)return fail('Fixture output limit');
      if(error){stderr+=chunk;stderrHook?.(stderr);}else stdout+=chunk;};
    child.stdout.on('data',chunk=>collect(chunk,false));child.stderr.on('data',chunk=>collect(chunk,true));
    child.on('error',()=>fail('Fixture child spawn failed'));
    child.on('close',code=>{if(done)return;done=true;clearTimeout(timer);resolve({code,stdout,stderr});});
  });
}
async function cli(args, {remote=false, failure=false, stderrHook}={}) {
  const result=await run(cliBinary,['--json','--space',workspace,...(remote?['remote',...args,'--endpoint',origin,'--ca',path.join(directory,'ca.pem')]:args)],{stderrHook});
  let value;try{value=JSON.parse(result.stdout);}catch{throw new Error('CLI returned no valid JSON; output withheld');}
  const safeDiagnostic=String(value.error?.message??'').match(/remote HTTP status \d{3}|Windows Credential Manager [^.]+\(error \d+\)|OS credential store unavailable|remote_[a-z_]+|invalid_[a-z_]+/)?.[0]??'output withheld';
  assert.equal(result.code === 0, !failure, `CLI ${args[0]} unexpected outcome (${safeDiagnostic})`);
  assert.equal(value.ok,!failure,'CLI structured outcome');
  return value.data;
}
async function freePort() {
  const listener=net.createServer();await new Promise(resolve=>listener.listen(0,'127.0.0.1',resolve));
  const port=listener.address().port;await new Promise(resolve=>listener.close(resolve));return port;
}
async function request(method, route, body, token) {
  return new Promise((resolve,reject)=>{
    const bytes=body===undefined?undefined:Buffer.from(JSON.stringify(body));
    const req=https.request(origin+route,{method,ca,timeout:10_000,headers:{...(token?{Authorization:`Bearer ${token}`} :{}),...(bytes?{'Content-Type':'application/json','Content-Length':bytes.length}:{})}},res=>{
      const chunks=[];let count=0;res.on('data',chunk=>{count+=chunk.length;if(count>4*1024*1024)req.destroy(new Error('HTTP bound'));else chunks.push(chunk);});
      res.on('end',()=>{let value;try{value=JSON.parse(Buffer.concat(chunks));}catch{value=null;}resolve({status:res.statusCode,value});});
    });req.on('error',reject);req.on('timeout',()=>req.destroy(new Error('HTTP timeout')));req.end(bytes);
  });
}
const tables=['steps','step_parents','space_steps','step_observations','repository_views','repository_view_layer_heads',
  'repository_view_markers','repository_view_releases','current_layer_heads_projection','spaces',
  'repository_view_history','space_events','sync_transactions','authorization_snapshots'];
function businessState() {
  return Object.fromEntries(tables.map(table=>[table,sql(`SELECT count(*) || ':' || md5(COALESCE(string_agg(row_to_json(t)::text,E'\\n' ORDER BY row_to_json(t)::text),'')) FROM ${table} t;`).stdout.trim()]));
}
const pending = () => JSON.parse(readFileSync(path.join(workspace,'.layrs','native-remote.json'),'utf8'));
const digest = value=>createHash('sha256').update(JSON.stringify(value)).digest('hex');
let arm=false, capturedRequest, beforeFailure, failureHttpStatus, proxyError;

try {
  const image=docker(['image','inspect','--format','{{.Id}}','postgres:17.6-bookworm']).stdout.trim();
  assert.match(image,/^sha256:[a-f0-9]{64}$/);report.postgresImage=image;save();
  container=docker(['run','-d','--pull=never','--name',name,'--label',`layrs.validation=${name}`,
    '--memory','512m','--memory-swap','512m','--pids-limit','96',
    '--tmpfs','/var/lib/postgresql/data:rw,nosuid,nodev,size=256m,mode=0700',
    '-p','127.0.0.1::5432','-e','POSTGRES_PASSWORD',image,
    '-c','shared_buffers=16MB','-c','max_connections=20','-c','min_wal_size=32MB','-c','max_wal_size=64MB'],
    {env:{POSTGRES_PASSWORD:adminPassword}}).stdout.trim();
  owned();
  let ready=false;for(let attempt=0;attempt<120;attempt++){
    if(docker(['exec',container,'pg_isready','-U','postgres'],{allowFailure:true}).status===0){ready=true;break;}
    await new Promise(resolve=>setTimeout(resolve,250));
  }assert(ready,'Disposable PostgreSQL readiness');
  const portMap=JSON.parse(docker(['inspect','--format','{{json .NetworkSettings.Ports}}',container]).stdout);
  const mapped=portMap['5432/tcp'];assert.equal(mapped.length,1);assert.equal(mapped[0].HostIp,'127.0.0.1');
  const pgPort=Number(mapped[0].HostPort);assert(pgPort>0&&pgPort<65536);
  sql(`CREATE ROLE layrs_runtime LOGIN PASSWORD '${runtimePassword}' NOSUPERUSER NOCREATEDB NOCREATEROLE;`);
  const pem=docker(['exec',container,'sh','-c',
    'umask 077; openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj /CN=127.0.0.1 -addext subjectAltName=IP:127.0.0.1 -addext basicConstraints=critical,CA:FALSE -addext extendedKeyUsage=serverAuth -keyout /tmp/layrs-fixture.key -out /tmp/layrs-fixture.crt 2>/dev/null && cat /tmp/layrs-fixture.key /tmp/layrs-fixture.crt && rm /tmp/layrs-fixture.key /tmp/layrs-fixture.crt']).stdout;
  const key=pem.match(/-----BEGIN PRIVATE KEY-----[\s\S]*?-----END PRIVATE KEY-----/)[0];
  ca=pem.match(/-----BEGIN CERTIFICATE-----[\s\S]*?-----END CERTIFICATE-----/)[0]+'\n';
  writeFileSync(path.join(directory,'ca.pem'),ca,{flag:'wx',mode:0o600});
  const backendPort=await freePort();
  proxy=https.createServer({key,cert:ca},async(req,res)=>{
    try{
      const chunks=[];let length=0;for await(const chunk of req){length+=chunk.length;assert(length<=8*1024*1024,'Proxy fixture request limit');chunks.push(chunk);}
      const body=Buffer.concat(chunks);
      const intercept=arm&&req.method==='POST'&&req.url.endsWith('/transactions');
      if(intercept){
        arm=false;capturedRequest=JSON.parse(body);assert(capturedRequest.steps.length>=128,'Real multi-Step admission required');
        beforeFailure=businessState();
        assert.deepEqual(pending().pending,capturedRequest,'Client journal exact before network transaction');
        sql('CHECKPOINT;');owned();
        const filled=docker(['exec',container,'sh','-c',`dd if=/dev/zero of=${pressure} bs=1048576 status=none`],{allowFailure:true});
        assert.notEqual(filled.status,0,'Filler must exhaust only fixture tmpfs');
        assert(/No space left on device/.test(filled.stderr),'Actual ENOSPC, not permission or quota refusal; output withheld');
        const free=docker(['exec',container,'df','-B1','--output=avail',pressure]).stdout.trim().split(/\s+/).at(-1);
        assert.equal(Number(free),0,'PGDATA tmpfs fully allocated');check('PGDATA tmpfs truly full before actual Layrs transaction');
      }
      const upstream=http.request({host:'127.0.0.1',port:backendPort,path:req.url,method:req.method,headers:req.headers,timeout:35_000},answer=>{
        if(intercept)failureHttpStatus=answer.statusCode;
        res.writeHead(answer.statusCode,answer.headers);answer.pipe(res);
      });upstream.on('timeout',()=>upstream.destroy(new Error('backend timeout')));
      upstream.on('error',()=>{if(!res.headersSent)res.writeHead(502);res.end();});upstream.end(body);
    }catch(error){proxyError=error;res.writeHead(500);res.end();}
  });
  proxy.on('connection',socket=>{sockets.add(socket);socket.on('close',()=>sockets.delete(socket));});
  await new Promise(resolve=>proxy.listen(0,'127.0.0.1',resolve));origin=`https://127.0.0.1:${proxy.address().port}`;
  const baseEnv={LAYRS_PUBLIC_URL:origin,LAYRS_SERVER_ADDR:`127.0.0.1:${backendPort}`,LAYRS_CAS_ROOT:cas,
    LAYRS_DEPLOYMENT_ID:`deployment:${name}`,LAYRS_STORE_ID:`store:${name}`,LAYRS_SIGNING_KEY:signingKey};
  const init=await run(serverBinary,['initialize'],{env:{...baseEnv,LAYRS_DATABASE_URL:`postgres://postgres:${adminPassword}@127.0.0.1:${pgPort}/postgres`,
    LAYRS_BOOTSTRAP_SECRET:bootstrapSecret,LAYRS_RUNTIME_ROLE:'layrs_runtime'}});
  assert.equal(init.code,0,'Qualified server initializes fresh database; output withheld');
  server=spawn(serverBinary,['serve'],{env:{...process.env,...baseEnv,LAYRS_DATABASE_URL:`postgres://layrs_runtime:${runtimePassword}@127.0.0.1:${pgPort}/postgres`},
    windowsHide:true,stdio:['ignore','ignore','ignore']});
  let healthy=false;for(let attempt=0;attempt<100;attempt++){
    assert(server.exitCode===null&&server.signalCode===null,'Owned server must remain alive during readiness');
    try{const health=await request('GET','/readyz');if(health.status===200&&health.value?.deploymentId===baseEnv.LAYRS_DEPLOYMENT_ID){healthy=true;break;}}catch{}
    await new Promise(resolve=>setTimeout(resolve,100));
  }assert(healthy,'Qualified native server ready');
  assert.equal(sql("SELECT rolsuper::int || ',' || rolcreatedb::int || ',' || rolcreaterole::int FROM pg_roles WHERE rolname='layrs_runtime';").stdout.trim(),'0,0,0');
  const boot=await request('POST','/v3/auth/bootstrap',{secret:bootstrapSecret,login:'pg-full@example.invalid',password:loginPassword,display_name:'Disposable PostgreSQL test'});
  assert.equal(boot.status,200);adminToken=boot.value.access_token;assert(adminToken);
  writeFileSync(path.join(workspace,'content'),'baseline');await cli(['init','PG disk full']);
  let approval;
  const login=await cli(['login'],{remote:true,stderrHook(text){const match=/and approve code ([A-Z0-9-]+)(?:\r?\n|$)/.exec(text);
    if(match&&!approval)approval=request('POST','/v3/auth/devices/approve',{userCode:match[1]},adminToken);}});
  assert.equal(login.status,'authorized');assert.equal((await approval).status,204);authenticated=true;
  await cli(['provision','disk-full','project'],{remote:true});
  const baseline=await cli(['publish'],{remote:true});assert.equal(baseline.receipt.status,'applied');
  check('Qualified CLI/server real baseline publication under nonprivileged PostgreSQL role');
  for(let index=0;index<160;index++){writeFileSync(path.join(workspace,'content'),`captured-${index}`);await cli(['step']);}
  const localBefore=await cli(['status']), bytesBefore=readFileSync(path.join(workspace,'content'));
  arm=true;await cli(['publish'],{remote:true,failure:true});
  if(proxyError)throw proxyError;
  assert(capturedRequest&&beforeFailure,'Transaction interception occurred');
  assert(failureHttpStatus>=500,'Backend must report failed admission');
  const capturedLogs=docker(['logs',container]);const logs=capturedLogs.stdout+capturedLogs.stderr;
  assert(/No space left on device/.test(logs),'PostgreSQL itself must report ENOSPC; logs withheld');
  const lines=logs.split('\n');
  const admissionEnospc=lines.some((line,index)=>{
    const pid=/\[(\d+)\]\s+ERROR:.*No space left on device/.exec(line)?.[1];
    return pid&&lines.slice(index+1,index+17).some(next=>next.includes(`[${pid}]`)
      && /STATEMENT:\s+INSERT INTO (?:steps|step_parents|space_steps|step_observations|authorization_snapshots|sync_transactions)\b/.test(next));
  });
  assert(admissionEnospc,'ENOSPC must belong to actual Layrs admission SQL, not only a background checkpoint; logs withheld');
  report.postgresAdmissionEnospc=true;
  report.failureHttpStatus=failureHttpStatus;report.attemptedSteps=capturedRequest.steps.length;
  assert.deepEqual(pending().pending,capturedRequest);
  assert.deepEqual(readFileSync(path.join(workspace,'content')),bytesBefore);
  assert.deepEqual(await cli(['status']),localBefore);
  check('Real PostgreSQL ENOSPC rejects Layrs admission and preserves exact client pending/data');
  owned();docker(['exec',container,'rm','--',pressure]);
  let recovered=false;for(let attempt=0;attempt<120;attempt++){
    if(sql('SELECT 1;',true).status===0){recovered=true;break;}await new Promise(resolve=>setTimeout(resolve,250));
  }assert(recovered,'PostgreSQL recovered in the SAME live tmpfs container');
  assert.deepEqual(businessState(),beforeFailure,'No partial Steps, View, observations, events, authorization snapshot or receipt');
  check('No partial business rows or receipt after freeing only fixture pressure file');
  const accepted=(await cli(['publish'],{remote:true})).receipt;assert.equal(accepted.status,'applied');
  assert.equal(accepted.idempotencyKey,capturedRequest.idempotencyKey);
  const knownSteps=new Set(baseline.receipt.acceptedStepIds);
  const allRequested=capturedRequest.steps.map(step=>step.stepId).sort();
  assert.deepEqual([...accepted.acceptedStepIds].sort(),allRequested.filter(step=>!knownSteps.has(step)),
    'Receipt admits exactly the new Steps, preserving already admitted ancestors');
  const admitted=JSON.parse(sql('SELECT json_agg(step_id ORDER BY step_id) FROM space_steps;').stdout);
  assert.deepEqual(admitted,allRequested,'Every requested Step exists in admitted closure after retry');
  assert.equal(pending().pending,null);assert.deepEqual(pending().lastReceipt,accepted);
  const after=businessState();
  const replay=await request('POST',`/v3/spaces/${capturedRequest.spaceId}/transactions`,capturedRequest,adminToken);
  assert.equal(replay.status,200);assert.deepEqual(replay.value,accepted);assert.deepEqual(businessState(),after);
  assert.deepEqual(await cli(['status']),localBefore);assert.deepEqual(readFileSync(path.join(workspace,'content')),bytesBefore);
  report.pendingSha256=digest(capturedRequest);report.receiptSha256=digest(accepted);
  check('Exact pending retry commits all Steps once; canonical receipt replay has no new effect');
  report.status='passed';save();
}catch(error){report.status='failed';report.error=error instanceof Error?error.message:'Fixture failure';save();
  console.error('PostgreSQL disk-full qualification failed; private diagnostics withheld; inspect report');process.exitCode=1;
}finally{
  if(authenticated){try{await cli(['logout'],{remote:true});}catch{report.credentialCleanup='failed';process.exitCode=1;}}
  if(server&&!await stop(server)){report.serverCleanup='failed';process.exitCode=1;}
  for(const socket of sockets)socket.destroy();if(proxy)await new Promise(resolve=>proxy.close(resolve));
  if(!container){const found=docker(['inspect','--format','{{.Id}}',name],{allowFailure:true});if(found.status===0)container=found.stdout.trim();}
  if(container){try{owned();docker(['rm','-f',container]);report.fixtureRemoved=true;}catch{report.fixtureRemoved=false;process.exitCode=1;}}
  if(process.exitCode&&report.status==='passed'){report.status='failed';report.error='Fixture cleanup did not complete';}
  report.elapsedMs=Date.now()-started;save();console.log(`Report ${reportFile}`);
}
