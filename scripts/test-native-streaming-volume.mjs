// Runs an already-built native_clients_v3 test binary; never builds or prints secrets.
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { open, stat, readFile, appendFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { once } from 'node:events';

const args = process.argv.slice(2);
const options = new Map();
for (let i = 0; i < args.length; i++) {
  const key = args[i];
  if (key === '--restore') options.set(key, true);
  else if (['--binary', '--report', '--sizes', '--timeout-minutes'].includes(key) && args[i + 1]) {
    if (options.has(key)) throw new Error('Duplicate argument.');
    options.set(key, args[++i]);
  } else throw new Error('Usage: --binary TEST_EXECUTABLE --report NEW_JSONL [--sizes 100,1024] [--restore] [--timeout-minutes 90]');
}
if (!options.has('--binary') || !options.has('--report')) throw new Error('Binary and new report paths are required.');
if (!process.env.LAYRS_TEST_DATABASE_URL) throw new Error('Set the private isolated-test PostgreSQL URL in LAYRS_TEST_DATABASE_URL.');
const binary = resolve(options.get('--binary'));
const report = resolve(options.get('--report'));
const sizes = options.get('--sizes') ?? '100,1024';
if (!['100', '1024', '100,1024'].includes(sizes)) throw new Error('Supported sizes: 100, 1024 or 100,1024 MiB.');
const minutes = Number(options.get('--timeout-minutes') ?? 90);
if (!Number.isInteger(minutes) || minutes < 1 || minutes > 180) throw new Error('Timeout must be 1–180 minutes.');
if (!(await stat(binary)).isFile()) throw new Error('Expected a compiled test executable.');
try { await stat(report); throw new Error('The report already exists; choose a new path.'); }
catch (error) { if (error.code !== 'ENOENT') throw error; }
const log = await open(`${report}.log`, 'wx');
const hash = createHash('sha256');
for await (const chunk of createReadStream(binary)) hash.update(chunk);
const binarySha256 = hash.digest('hex');
const started = Date.now();
const environment = { ...process.env };
delete environment.LAYRS_TEST_STREAMING_VOLUME_RESTORE;
Object.assign(environment, {
  LAYRS_TEST_STREAMING_VOLUME_ONLY: '1',
  LAYRS_TEST_STREAMING_VOLUME_REPORT: report,
  LAYRS_TEST_STREAMING_VOLUME_MIB: sizes,
  ...(options.get('--restore') ? { LAYRS_TEST_STREAMING_VOLUME_RESTORE: '1' } : {}),
});
const child = spawn(binary, [
  'two_native_clients_publish_clone_receive_divergence_and_local_operations', '--exact', '--ignored', '--nocapture',
], {
  windowsHide: true,
  stdio: ['ignore', 'pipe', 'pipe'],
  env: environment,
});
let timedOut = false;
let logBytes = 0;
const maxLogBytes = 16 * 1024 * 1024;
async function drain(stream) {
  for await (const chunk of stream) {
    const permitted = Math.max(0, Math.min(chunk.length, maxLogBytes - logBytes));
    logBytes += permitted;
    if (permitted) await log.write(chunk.subarray(0, permitted));
  }
}
const drains = Promise.all([drain(child.stdout), drain(child.stderr)]);
const timer = setTimeout(() => { timedOut = true; child.kill('SIGKILL'); }, minutes * 60_000);
const closed = once(child, 'close');
let code = null;
let spawnFailed = false;
try { [code] = await closed; }
catch { spawnFailed = true; }
finally { clearTimeout(timer); await drains; await log.sync(); await log.close(); }
let result = null;
let separator = '';
try {
  const metadata = await stat(report);
  if (metadata.size > 1024 * 1024) throw new Error('Measurement report exceeded its bound.');
  const contents = await readFile(report, 'utf8');
  separator = contents.length && !contents.endsWith('\n') ? '\n' : '';
  const lines = contents.trim().split('\n').filter(Boolean);
  for (const line of lines) {
    const event = JSON.parse(line);
    if (event.event === 'result') result = event;
  }
} catch (error) {
  result = null;
  if (error.code === 'ENOENT') { const missing = await open(report, 'wx'); await missing.close(); }
  // A partial last line after abrupt death remains evidence; wrapper status follows.
}
const passed = code === 0 && result?.state === 'passed' && !timedOut && !spawnFailed;
await appendFile(report, `${separator}${JSON.stringify({
  event: 'runner-result', state: passed ? 'passed' : 'failed', code, timedOut, spawnFailed,
  elapsedMs: Date.now() - started, binarySha256, logBytes,
  limitation: 'Only this child is terminated on timeout; report phases distinguish completed and interrupted work.',
})}\n`);
const durable = await open(report, 'r+'); await durable.sync(); await durable.close();
console.log(`HTTPS streaming volume: ${passed ? 'PASS' : 'FAILED'}; report ${report}`);
process.exitCode = passed ? 0 : 1;
