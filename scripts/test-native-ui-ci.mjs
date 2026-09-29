// Disposable native TLS/PostgreSQL UI recipe. Credentials/logs remain private
// and are never uploaded as test artifacts. No V1 service or mocked V3 routes.
import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, existsSync, readFileSync, rmSync, openSync, closeSync } from 'node:fs';
import { join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { testPostgres } from './native-ui-postgres.mjs';
import { nativeUiProxy } from './native-ui-proxy.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const target = process.argv[2];
if (!['web', 'desktop', 'desktop-team', 'desktop-partial', 'cli', 'lifecycle'].includes(target)) throw new Error('Expected web, desktop, desktop-team, desktop-partial, cli or lifecycle');
const temporary = resolve(root, 'tmp');
mkdirSync(temporary, { recursive: true, mode: 0o700 });
const directory = mkdtempSync(join(temporary, 'native-ui-ci-'));
const fixtureFile = join(directory, 'fixture.json');
const resourceFile = join(directory, 'resources.json');
const log = openSync(join(directory, 'fixture.log'), 'wx', 0o600);
const suffix = process.platform === 'win32' ? '.exe' : '';
const binary = join(root, 'target/debug/examples', `native_ui_fixture${suffix}`);
let database, fixtureProcess, proxy;
let failed = false;

function run(command, args, environment, privateOutput = false, cwd = root) {
  const result = spawnSync(command, args, { cwd, env: environment, windowsHide: true, stdio: privateOutput ? ['ignore', log, log] : 'inherit', timeout: 1_800_000 });
  if (result.status !== 0) throw new Error(`Native UI ${target} prerequisite or test failed (exit ${result.status})`);
}
async function runBrowser(args, env) {
  const child = spawn(process.execPath, args, { cwd: root, env, windowsHide: true, stdio: 'inherit' });
  const signal = () => child.kill();
  process.once('SIGINT', signal); process.once('SIGTERM', signal);
  try {
    await new Promise((resolve, reject) => {
      child.once('error', () => reject(new Error('Browser harness could not start')));
      child.once('exit', code => code === 0 ? resolve() : reject(new Error(`Native browser recipe failed (exit ${code})`)));
    });
  } finally { process.removeListener('SIGINT', signal); process.removeListener('SIGTERM', signal); }
}
async function stopFixture() {
  if (!fixtureProcess || fixtureProcess.exitCode !== null) return;
  const exited = new Promise(resolve => fixtureProcess.once('exit', resolve));
  fixtureProcess.kill();
  await Promise.race([exited, new Promise(resolve => setTimeout(resolve, 5000))]);
  if (fixtureProcess.exitCode === null) { fixtureProcess.kill('SIGKILL'); await exited; }
}
try {
  database = await testPostgres(directory);
  const env = { ...database.environment, LAYRS_TEST_UI_FIXTURE_FILE: fixtureFile, LAYRS_TEST_UI_RESOURCE_FILE: resourceFile };
  run('cargo', ['build', '--locked', '--target-dir', join(root, 'target'), '-p', 'layrs-server', '--example', 'native_ui_fixture'], env);
  if (target === 'cli') run('cargo', ['build', '--locked', '--target-dir', join(root, 'target'), '-p', 'layrs-cli'], env);
  if (target === 'desktop' || target === 'desktop-team' || target === 'desktop-partial') {
    run('cargo', ['build', '--locked', '--target-dir', join(root, 'target'), '-p', 'layrs-weave', '--bin', 'layrs-lens-worker'], env);
    env.LAYRS_LENS_WORKER = join(root, 'target/debug', `layrs-lens-worker${suffix}`);
    env.LAYRS_LENS_WORKER_SHA256 = `sha256:${createHash('sha256').update(readFileSync(env.LAYRS_LENS_WORKER)).digest('hex')}`;
  }
  fixtureProcess = spawn(binary, [], { cwd: root, env, windowsHide: true, stdio: ['ignore', log, log] });
  fixtureProcess.on('error', () => { failed = true; });
  const deadline = Date.now() + 120_000;
  while (!existsSync(fixtureFile)) {
    if (failed || fixtureProcess.exitCode !== null || Date.now() > deadline) throw new Error('Native TLS fixture startup failed (private log retained until cleanup)');
    await new Promise(resolve => setTimeout(resolve, 200));
  }
  const fixture = JSON.parse(readFileSync(fixtureFile, 'utf8'));
  if (target === 'web') {
    run(process.execPath, ['node_modules/vite/bin/vite.js', 'build'], { ...env }, false, join(root, 'apps/studio-web'));
  }
  let extra = {};
  if (target === 'web') {
    proxy = await nativeUiProxy(fixture, join(root, 'apps/studio-web/dist'));
    extra = { LAYRS_NATIVE_WEB_URL: proxy.origin, LAYRS_NATIVE_WEB_CERT_PIN: proxy.pin };
  }
  if (target === 'cli') {
    run(process.execPath, ['scripts/test-native-cli-remote.mjs'], { ...env, LAYRS_NATIVE_UI_FIXTURE: fixtureFile });
  } else if (target !== 'lifecycle') {
    const config = target === 'web' ? 'playwright.native-web.config.ts' : 'playwright.native.config.ts';
    await runBrowser(['node_modules/@playwright/test/cli.js', 'test', '-c', config,...(target === 'desktop-team' ? ['tests/e2e/desktop-native-team.spec.ts'] : target === 'desktop-partial' ? ['tests/e2e/desktop-native-partial.spec.ts'] : [])], {
      ...env, ...extra, CI: 'true', LAYRS_NATIVE_UI_FIXTURE: fixtureFile, LAYRS_NATIVE_UI_REQUIRED: '1',
      LAYRS_NATIVE_UI_TARGET: target,
      LAYRS_E2E_VISUAL: '0', LAYRS_E2E_HEADLESS: '1',
    });
  }
  console.log(`Native UI ${target} recipe passed`);
} catch (error) {
  failed = true;
  console.error(error.message);
} finally {
  try { await proxy?.close(); } catch { failed = true; }
  await stopFixture();
  if (database && existsSync(resourceFile)) {
    try { run(binary, ['--cleanup-fixture', resourceFile], database.environment, true); }
    catch { failed = true; console.error('Native fixture cleanup failed; private resource ledger retained'); }
  }
  try { database?.close(); } catch { failed = true; }
  closeSync(log);
  const path = resolve(directory);
  if (!failed && path.startsWith(temporary + sep) && path !== temporary) rmSync(path, { recursive: true });
  // Failed runs retain only a private local directory, excluded from uploads.
}
process.exitCode = failed ? 1 : 0;
