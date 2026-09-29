// Disposable local Compose qualification. Never uses deployment secret files.
import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { chmodSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import https from 'node:https';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const project = `layrs-pilot-${randomBytes(6).toString('hex')}`;
const directory = path.join(root, 'tmp', project);
const port = Number(process.env.LAYRS_TEST_HTTPS_PORT ?? 19443);
assert(Number.isSafeInteger(port) && port >= 1024 && port <= 65535);
const origin = `https://localhost:${port}`;
mkdirSync(directory, { recursive: true, mode: 0o700 });
const backupDirectory = path.join(directory, 'backups');
mkdirSync(backupDirectory, { mode: 0o777 });
// Parent is private; this child is writable by the container's distinct UID.
chmodSync(backupDirectory, 0o777);
const secret = () => randomBytes(32).toString('hex');
const maintenance = secret(), runtime = secret(), bootstrap = secret(), password = secret();
const values = {
  postgres_password: maintenance, runtime_password: runtime,
  maintenance_database_url: `postgres://layrs_maintenance:${maintenance}@postgres:5432/layrs`,
  runtime_database_url: `postgres://layrs_runtime:${runtime}@postgres:5432/layrs`,
  bootstrap_secret: bootstrap, signing_key: secret(),
};
// Each direct Compose secret mount must be readable by UID 10001. The private
// enclosing directory prevents host users from traversing to these files.
for (const [name, value] of Object.entries(values)) writeFileSync(path.join(directory, name), `${value}\n`, { flag: 'wx', mode: 0o644 });
const caddy = readFileSync(path.join(root, 'deploy/selfhost/Caddyfile'), 'utf8').replace('{$LAYRS_HOST} {', 'https://localhost {\n    tls internal');
writeFileSync(path.join(directory, 'Caddyfile'), caddy, { flag: 'wx' });
const override = {
  services: { proxy: { volumes: [`${path.join(directory, 'Caddyfile').replaceAll('\\', '/')}:/etc/caddy/Caddyfile:ro`] } },
  secrets: Object.fromEntries(Object.keys(values).map(name => [name, { file: path.join(directory, name).replaceAll('\\', '/') }])),
};
// Compose merge tags reset production port publications, keeping the pilot local.
writeFileSync(path.join(directory, 'override.yml'), `services:\n  proxy:\n    ports: !override ["127.0.0.1:${port}:443"]\n    volumes:\n      - ${JSON.stringify(override.services.proxy.volumes[0])}\nsecrets:\n${Object.entries(override.secrets).map(([name, value]) => `  ${name}: {file: ${JSON.stringify(value.file)}}`).join('\n')}\n`, { flag: 'wx' });
const env = { ...process.env, LAYRS_HOST: `localhost:${port}`, LAYRS_DEPLOYMENT_ID: `deployment:${project}`, LAYRS_STORE_ID: `store:${project}`, LAYRS_BACKUP_PATH: backupDirectory.replaceAll('\\', '/'), LAYRS_IMAGE_TAG: process.env.LAYRS_IMAGE_TAG ?? 'validation' };
const prefix = [];
if (process.env.LAYRS_TEST_DOCKER_CONFIG) prefix.push('--config', process.env.LAYRS_TEST_DOCKER_CONFIG);
if (process.env.LAYRS_TEST_DOCKER_HOST) prefix.push('-H', process.env.LAYRS_TEST_DOCKER_HOST);
function docker(args, { allowFailure = false } = {}) {
  const result = spawnSync('docker', [...prefix, ...args], { cwd: root, env, encoding: 'utf8', timeout: 600_000, maxBuffer: 8 * 1024 * 1024, windowsHide: true });
  if (!allowFailure && result.status !== 0) {
    // Commands never put credentials in argv. Do not print captured Docker logs.
    let diagnostic = `${result.stdout}\n${result.stderr}`;
    for (const value of [...Object.values(values), password]) diagnostic = diagnostic.replaceAll(value, '[redacted]');
    writeFileSync(path.join(directory, 'docker-failure.txt'), diagnostic, { mode: 0o600 });
    throw new Error(`Docker ${args.slice(0, 2).join(' ')} failed (status ${result.status}); inspect only the named pilot containers locally`);
  }
  return result.stdout.trim();
}
const composeArgs = ['compose', '-p', project, '-f', path.join(root, 'deploy/selfhost/compose.yml'), '-f', path.join(directory, 'override.yml')];
const compose = (...args) => docker([...composeArgs, ...args]);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
let ca;
function request(method, route, body, token, trust = ca) {
  return new Promise((resolve, reject) => {
    const bytes = body === undefined ? undefined : Buffer.from(JSON.stringify(body));
    const headers = { Origin: origin };
    if (bytes) { headers['Content-Type'] = 'application/json'; headers['Content-Length'] = bytes.length; }
    if (token) headers.Authorization = `Bearer ${token}`;
    const req = https.request(`${origin}${route}`, { method, ca: trust, headers, timeout: 10_000 }, res => {
      const chunks = []; let length = 0;
      res.on('data', chunk => { length += chunk.length; if (length > 8 * 1024 * 1024) req.destroy(new Error('response too large')); else chunks.push(chunk); });
      res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, text: Buffer.concat(chunks).toString('utf8') }));
    });
    req.on('error', reject); req.on('timeout', () => req.destroy(new Error('request timeout')));
    req.end(bytes);
  });
}
const checks = [];
const record = name => { checks.push(name); console.log(`PASS ${name}`); };
try {
  console.log(`Starting isolated Compose pilot ${project}; state ${directory}`);
  compose('config', '--quiet');
  compose('up', '-d', '--no-build', '--wait', '--wait-timeout', '180');
  record('Compose starts PostgreSQL, migration job, runtime server and static proxy');
  const proxy = compose('ps', '-q', 'proxy');
  for (let attempt = 0; attempt < 30; attempt++) {
    docker(['cp', `${proxy}:/data/caddy/pki/authorities/local/root.crt`, path.join(directory, 'root.crt')], { allowFailure: true });
    try { ca = readFileSync(path.join(directory, 'root.crt')); break; } catch { await delay(500); }
  }
  assert(ca, 'Caddy internal root CA must exist');
  await assert.rejects(request('GET', '/healthz', undefined, undefined, []), undefined, 'untrusted private CA must be rejected');
  record('untrusted private CA refused; extracted root explicitly trusted');
  assert.equal((await request('GET', '/healthz')).status, 200);
  const ready = await request('GET', '/readyz'); assert.equal(ready.status, 200); assert.equal(JSON.parse(ready.text).ready, true);
  record('same-origin HTTPS liveness and database/CAS readiness');
  const html = await request('GET', '/'); assert.equal(html.status, 200); assert.match(html.text, /<html/i); assert.match(html.headers['content-security-policy'], /connect-src 'self'/);
  const asset = html.text.match(/src="([^" ]+\.js)"/); assert(asset, 'built JavaScript entry point');
  const js = await request('GET', asset[1]); assert.equal(js.status, 200); assert.match(js.headers['content-type'], /javascript/); assert(js.text.length > 1000);
  assert.equal((await request('GET', '/auth/device')).status, 200);
  record('Studio HTML, bundled JavaScript, CSP and device page share the HTTPS origin');
  const login = 'pilot-admin@example.test';
  const first = await request('POST', '/v3/auth/bootstrap', { secret: bootstrap, login, password, display_name: 'Local pilot administrator' });
  assert.equal(first.status, 200, 'bootstrap accepted'); const token = JSON.parse(first.text).access_token; assert(token);
  assert.equal((await request('GET', '/v3/auth/whoami', undefined, token)).status, 200);
  assert.equal((await request('GET', '/v3/auth/whoami')).status, 401);
  record('one-shot bootstrap, authenticated principal and anonymous refusal');
  const provision = await request('POST', '/v3/workspaces/provision', { workspaceSlug: 'pilot', spaceSlug: 'project', name: 'Disposable local pilot' }, token);
  assert.equal(provision.status, 201, 'real native provisioning'); const scope = JSON.parse(provision.text); assert(scope.spaceId && scope.layerId);
  record('native Workspace/Space/Layer persisted by nonprivileged runtime');
  const server = compose('ps', '-q', 'server');
  assert.equal(docker(['exec', server, 'id', '-u']), '10001');
  const postgres = compose('ps', '-q', 'postgres');
  const role = docker(['exec', postgres, 'psql', '-U', 'layrs_maintenance', '-d', 'layrs', '-Atc', "SELECT rolsuper::int || ',' || rolcreatedb::int || ',' || rolcreaterole::int FROM pg_roles WHERE rolname='layrs_runtime'"]);
  assert.equal(role, '0,0,0'); record('Linux UID 10001 and PostgreSQL runtime without superuser/database/role creation');
  compose('--profile', 'backup', 'run', '--rm', '--no-deps', 'backup');
  const status = JSON.parse(compose('--profile', 'backup', 'run', '--rm', '--no-deps', '--entrypoint', 'cat', 'backup', '/backups/last-backup.json'));
  assert.equal(status.state, 'verified'); assert(status.lastSuccessUnix > 0);
  assert.match(status.backup, /^backup-[0-9TZ]+-[0-9]+$/);
  const manifest = JSON.parse(compose('--profile', 'backup', 'run', '--rm', '--no-deps', '--entrypoint', 'cat', 'backup', `/backups/${status.backup}/manifest.json`));
  assert(manifest.objects.length > 0);
  docker([...composeArgs, '--profile', 'backup', 'run', '--rm', '--no-deps', '-e', 'LAYRS_SERVER_EXECUTABLE=/bin/false', 'backup'], { allowFailure: true });
  const failure = JSON.parse(compose('--profile', 'backup', 'run', '--rm', '--no-deps', '--entrypoint', 'cat', 'backup', '/backups/last-backup.json'));
  assert.equal(failure.state, 'failed'); assert.equal(failure.lastSuccessUnix, status.lastSuccessUnix);
  record('scheduled real PG/CAS backup verifies closure; failed attempt preserves last success and reports failure');
  compose('restart', 'server');
  for (let attempt = 0; attempt < 45; attempt++) { try { if ((await request('GET', '/readyz')).status === 200) break; } catch {} await delay(1000); }
  assert.equal((await request('GET', '/readyz')).status, 200);
  compose('run', '--rm', '--no-deps', 'initialize');
  const again = await request('POST', '/v3/auth/bootstrap', { secret: bootstrap, login, password, display_name: 'Second administrator' }); assert.notEqual(again.status, 200);
  const relogin = await request('POST', '/v3/auth/login', { login, password }); assert.equal(relogin.status, 200);
  const nextToken = JSON.parse(relogin.text).access_token;
  assert.equal((await request('GET', '/v3/auth/whoami', undefined, nextToken)).status, 200);
  assert.equal((await request('POST', '/v3/auth/logout', undefined, nextToken)).status, 204);
  assert.equal((await request('GET', '/v3/auth/whoami', undefined, nextToken)).status, 401);
  record('restart and reinitialize preserve consumed bootstrap; login/logout revocation survives');
  writeFileSync(path.join(directory, 'report.json'), JSON.stringify({ project, origin, checks, serverImage: docker(['image', 'inspect', `layrs-server:${env.LAYRS_IMAGE_TAG}`, '--format', '{{.Id}}']), studioImage: docker(['image', 'inspect', `layrs-studio:${env.LAYRS_IMAGE_TAG}`, '--format', '{{.Id}}']), date: new Date().toISOString() }, null, 2));
} finally {
  if (process.argv.includes('--keep')) console.log(`Pilot retained explicitly: ${project}; private state ${directory}`);
  else { compose('down', '--volumes', '--remove-orphans'); console.log(`Removed only isolated pilot ${project} containers and volumes; private files remain at ${directory}`); }
}
