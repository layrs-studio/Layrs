// Mandatory native service integration; an ordinary cargo test skips these tests.
import { spawnSync } from 'node:child_process';

if (!process.env.LAYRS_TEST_DATABASE_URL || process.env.LAYRS_TEST_DATABASE_ISOLATED !== '1') {
  throw new Error('Set an isolated PostgreSQL administrator test endpoint and LAYRS_TEST_DATABASE_ISOLATED=1');
}
const suites = [
  'native_identity',
  'native_invitations',
  'native_storage_budget',
  'native_retention',
  'native_sync_v3',
  'native_clients_v3',
];
// The native TLS scenario executes the actual supervised WASM worker. A clean
// checkout must build it explicitly rather than rely on a previous local run.
const buildArgs = ['build', '--locked', '-p', 'layrs-weave', '--bin', 'layrs-lens-worker'];
if (process.argv.includes('--offline')) buildArgs.push('--offline');
const worker = spawnSync('cargo', buildArgs, { windowsHide: true, stdio: 'inherit' });
if (worker.error || worker.status !== 0) process.exit(worker.status ?? 1);
const args = ['test', '--locked'];
if (process.argv.includes('--offline')) args.push('--offline');
args.push('-p', 'layrs-server', ...suites.flatMap(name => ['--test', name]),
  '--', '--ignored', '--nocapture', '--test-threads=1');
// The endpoint and credentials are inherited only through the environment.
// Capture stdout to reject accidental zero-test/ignored-only successful runs.
const result = spawnSync('cargo', args, {
  encoding: 'utf8', windowsHide: true, maxBuffer: 32 * 1024 * 1024,
  stdio: ['ignore', 'pipe', 'inherit'],
});
process.stdout.write(result.stdout ?? '');
if (result.error || result.status !== 0) process.exit(result.status ?? 1);
const summaries = [...result.stdout.matchAll(/test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;/g)];
if (summaries.length !== suites.length || summaries.some(([, passed, failed, ignored]) =>
  Number(passed) !== 1 || Number(failed) !== 0 || Number(ignored) !== 0)) {
  throw new Error('Every mandatory native PostgreSQL suite must execute exactly one integration test');
}
console.log(`${suites.length} native PostgreSQL integration suites executed; none skipped`);
