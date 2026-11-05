// Fixed public repository qualification; no repository code, hooks or filters run.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';

const root = path.resolve(import.meta.dirname, '..');
const project = process.argv.find(value=>value.startsWith('--project='))?.slice(10) ?? 'pyproject-hooks';
const projects = {
  itsdangerous: { repository:'pallets/itsdangerous',pin:'672971d66a2ef9f85151e53283113f33d642dabd',readme:'README.md',license:'LICENSE.txt' },
  'pyproject-hooks': { repository:'pypa/pyproject-hooks',pin:'184c9f56a87ede1276a5b33f5024204adb792cd2',readme:'README.rst',license:'LICENSE' },
};
const chosen=projects[project];assert(chosen,'unknown fixed pilot project');
const job = path.join(root, `tmp/t7-${project}-20260920`);
const source = path.join(job, 'source.git');
const native = path.join(job, 'native');
const destination = path.join(job, 'export.git');
const cli = path.join(root, 'target/debug', process.platform === 'win32' ? 'layrs.exe' : 'layrs');
const pin = chosen.pin;
const resume = process.argv.includes('--resume');
const reportPath = path.join(root, `docs/plans/pme/measurements/T7-public-${project}-2026-09-20.json`);
const progressPath = path.join(job, 'pilot-progress.json');
let report = {
  schemaVersion: 'layrs.git-bridge.public-pilot.v1', status: 'running',
  repository: `https://github.com/${chosen.repository}`, head: pin,
  readme: `https://github.com/${chosen.repository}/blob/${pin}/${chosen.readme}`,
  license: `https://github.com/${chosen.repository}/blob/${pin}/${chosen.license}`,
  platform: process.platform, architecture: process.arch, binary: 'debug',
  timingsMs: {}, assertions: {},
  scope: 'Complete advertised local branches and commit tags, including all ancestors. Public open-source repository qualification; no private repository or human PME pilot.',
};
if (resume && fs.existsSync(progressPath)) report = JSON.parse(fs.readFileSync(progressPath, 'utf8'));
const save = () => fs.writeFileSync(progressPath, JSON.stringify(report, null, 2));
function execute(exe, args, options = {}) {
  const result = spawnSync(exe, args, {
    cwd: native, timeout: 600_000, maxBuffer: 64 * 1024 * 1024,
    env: { ...process.env, GIT_TERMINAL_PROMPT: '0', GIT_OPTIONAL_LOCKS: '0' }, ...options,
  });
  if (result.error || result.status !== 0) {
    throw new Error(`${path.basename(exe)} ${args.slice(0, 4).join(' ')}: ${result.error?.message ?? result.status}\n${result.stderr?.toString().slice(-8000)}\n${result.stdout?.toString().slice(-1000)}`);
  }
  return result.stdout;
}
const git = (repository, args, input) => execute('git', ['--no-optional-locks', '-C', repository, ...args], { cwd: job, input });
const text = buffer => buffer.toString('utf8').trim();
function fingerprint(directory) {
  const hash = crypto.createHash('sha256'); let bytes = 0; let files = 0;
  function visit(relative) {
    for (const entry of fs.readdirSync(path.join(directory, relative), { withFileTypes: true }).sort((a,b)=>a.name.localeCompare(b.name))) {
      const name = path.join(relative, entry.name); assert(!entry.isSymbolicLink());
      if (entry.isDirectory()) visit(name);
      else {const value = fs.readFileSync(path.join(directory,name)); hash.update(name.replaceAll('\\','/'));hash.update('\0');hash.update(value);bytes += value.length;files++;}
    }
  }
  visit(''); return { sha256: hash.digest('hex'), bytes, files };
}
function stage(name, args) {
  const file = path.join(job, `${name}.json`);
  if (resume && fs.existsSync(file)) return JSON.parse(fs.readFileSync(file, 'utf8')).data;
  console.log(`Git pilot: ${name}`); save();
  const started = performance.now(); const bytes = execute(cli, ['--json', ...args]);
  const value = JSON.parse(bytes); assert.equal(value.ok, true);
  report.timingsMs[name] = Math.round(performance.now() - started);
  fs.writeFileSync(file, bytes); save(); return value.data;
}
function batch(repository, ids) {
  const bytes = git(repository, ['cat-file', '--batch'], `${ids.join('\n')}\n`);
  const result = new Map(); let position = 0;
  for (const expected of ids) {
    const end = bytes.indexOf(10, position); assert(end >= position);
    const [id, kind, length] = bytes.subarray(position,end).toString().split(' ');
    assert.equal(id, expected); const size = Number(length); assert(Number.isSafeInteger(size) && size >= 0);
    position = end + 1; result.set(id, {kind, bytes:bytes.subarray(position, position + size)});
    position += size; assert.equal(bytes[position++], 10);
  }
  assert.equal(position, bytes.length); return result;
}
try {
  assert(fs.existsSync(source), 'clone the documented source mirror first');
  assert.equal(text(git(source, ['rev-parse','HEAD'])), pin);
  assert.equal(text(git(source, ['rev-parse','--is-shallow-repository'])), 'false');
  assert(!fs.existsSync(path.join(source,'objects/info/alternates')));
  const initial = fingerprint(source);
  if (resume) assert.deepEqual(initial, report.sourceFingerprintBefore);
  else {assert(!fs.existsSync(native) && !fs.existsSync(destination)); report.sourceFingerprintBefore = initial;}
  fs.mkdirSync(native, { recursive: true });
  report.gitVersion = text(git(source,['--version']));
  report.sourceReferences = text(git(source,['for-each-ref','--format=%(refname) %(objectname)','refs/heads','refs/tags'])).split('\n').map(line=>{const [name,id]=line.split(' ');return {name,id};});
  report.selectedCommitCount=Number(text(git(source,['rev-list','--count','--branches','--tags'])));
  report.advertisedReferenceCount=text(git(source,['for-each-ref','--format=%(refname)'])).split('\n').length;
  report.licenseSha256 = crypto.createHash('sha256').update(git(source,['show',`${pin}:${chosen.license}`])).digest('hex');
  const dry = stage('dry-run',['git','import',source,'--dry-run','--timeout-seconds','600']);
  assert.equal(dry.dryRun,true); assert(!fs.existsSync(path.join(native,'.layrs')) || resume);
  stage('initialize',['init',`${project} public qualification`]);
  const imported = stage('import',['git','import',source,'--timeout-seconds','600']);
  assert.equal(imported.report.importId,dry.report.importId);
  assert.equal(imported.report.commits.length,report.selectedCommitCount);
  assert.deepEqual(imported.report.references.map(ref=>ref.gitRef).toSorted(),report.sourceReferences.map(ref=>ref.name).toSorted());
  const repeated = stage('repeat-import',['git','import',source,'--timeout-seconds','600']);
  assert.equal(repeated.import.alreadyImported,true);
  assert.equal(repeated.import.operationId,imported.import.operationId);
  stage('materialize-main',['layer','use','git/main']);
  let checkedFiles=0;
  for(const entry of git(source,['ls-tree','-r','-z',pin]).toString().split('\0').filter(Boolean)) {
    const split=entry.indexOf('\t');const [mode,kind,id]=entry.slice(0,split).split(' ');const name=entry.slice(split+1);
    assert.equal(kind,'blob');assert(!name.split('/').some(part=>part==='..'||part===''));
    const actual=mode==='120000'?Buffer.from(fs.readlinkSync(path.join(native,name))):fs.readFileSync(path.join(native,name));
    const hash=crypto.createHash('sha1').update(Buffer.from(`blob ${actual.length}\0`)).update(actual).digest('hex');assert.equal(hash,id);checkedFiles++;
  }
  const exportArgs = ['git','export',destination,'--timeout-seconds','600'];
  if (resume && fs.existsSync(path.join(destination,'layrs-export.incomplete'))) exportArgs.push('--resume');
  const exported = stage('export',exportArgs).export;
  const started = performance.now(); git(destination,['fsck','--full','--strict']); report.timingsMs.fsck=Math.round(performance.now()-started);
  const mappings = new Map(imported.report.commits.map(commit=>[commit.gitId,commit]));
  const sourceObjects = batch(source,[...new Set([...mappings.keys(),...report.sourceReferences.map(ref=>ref.id)])]);
  const exportedObjects = batch(destination,[...new Set(Object.values(exported.commits))]);
  let parentOrderChanges=0;
  const parents = bytes => [...bytes.toString('utf8').split('\n\n',1)[0].matchAll(/^parent ([0-9a-f]+)$/gm)].map(match=>match[1]);
  for (const commit of imported.report.commits) {
    const original=sourceObjects.get(commit.gitId);assert.equal(original.kind,'commit');
    assert(original.bytes.equals(Buffer.from(commit.rawCommitBase64,'base64')));
    const result=exportedObjects.get(exported.commits[commit.stepId]);assert.equal(result.kind,'commit');
    assert.equal(result.bytes.toString().match(/^tree ([0-9a-f]+)$/m)[1],original.bytes.toString().match(/^tree ([0-9a-f]+)$/m)[1]);
    const expected=[...new Set(commit.parentGitIds.map(id=>exported.commits[mappings.get(id).stepId]))];
    const actual=parents(result.bytes);if(JSON.stringify(expected)!==JSON.stringify(actual))parentOrderChanges++;
    assert.deepEqual(actual.toSorted(),expected.toSorted());
  }
  for (const reference of imported.report.references) {
    assert.equal(text(git(destination,['rev-parse',`${reference.gitRef}^{commit}`])),exported.commits[reference.stepId]);
    if(reference.annotatedTagBase64){const original=report.sourceReferences.find(ref=>ref.name===reference.gitRef);assert(sourceObjects.get(original.id).bytes.equals(Buffer.from(reference.annotatedTagBase64,'base64')));}
  }
  report.sourceFingerprintAfter = fingerprint(source); assert.deepEqual(report.sourceFingerprintAfter,initial);
  report.counts={sourceCommits:imported.report.commits.length,uniqueImportedSteps:new Set(imported.report.commits.map(commit=>commit.stepId)).size,exportedCommits:Object.keys(exported.commits).length,sourceBranches:report.sourceReferences.filter(ref=>ref.name.startsWith('refs/heads/')).length,sourceTags:report.sourceReferences.filter(ref=>ref.name.startsWith('refs/tags/')).length,annotatedTags:imported.report.references.filter(ref=>ref.annotatedTagBase64).length,exportedReferences:Object.keys(exported.references).length,sourceObjectBytes:imported.report.sourceBytes,parentOrderChanges,materializedHeadFiles:checkedFiles};
  report.warnings={import:imported.report.warnings,export:exported.warnings};
  report.assertions={sourceEntireMirrorUnchanged:true,selectedBranchesAndTagsHaveCompleteHistory:true,importRepeatSameOperation:true,everySourceCommitProvenanceExact:true,everyImportedCommitTreeExact:true,everyImportedParentSetPreserved:true,allSelectedBranchAndTagReferencesPreserved:true,annotatedTagProvenanceExact:true,materializedMainBytesExact:true,gitFsckStrict:true,noSourceCodeExecuted:true};
  report.nativeStorage=fingerprint(path.join(native,'.layrs'));report.exportStorage=fingerprint(destination);
  report.status='passed';save();fs.writeFileSync(reportPath,JSON.stringify(report,null,2));
  console.log(JSON.stringify({status:report.status,counts:report.counts,timingsMs:report.timingsMs,report:reportPath}));
} catch(error) {
  report.status=error.message.includes('Unsupported(')?'refused':'failed';report.error=error.message;
  report.sourceFingerprintAfter=fingerprint(source);
  report.assertions.sourceEntireMirrorUnchanged=JSON.stringify(report.sourceFingerprintAfter)===JSON.stringify(report.sourceFingerprintBefore);
  report.assertions.nativeStoreCreated=fs.existsSync(path.join(native,'.layrs'));
  save();fs.writeFileSync(reportPath,JSON.stringify(report,null,2));console.error(error.message);process.exitCode=report.status==='refused'?2:1;
}
