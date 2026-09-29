// Mandatory native service integration; an ordinary cargo test skips these tests.
import { spawnSync } from 'node:child_process';

if (!process.env.LAYRS_TEST_DATABASE_URL || process.env.LAYRS_TEST_DATABASE_ISOLATED !== '1') {
  throw new Error('Set an isolated PostgreSQL administrator test endpoint and LAYRS_TEST_DATABASE_ISOLATED=1');
}
const suites = [
  ['native_identity', 'native_bootstrap_sessions_revocation_and_recovery'],
  ['native_invitations', 'invitations_are_nominative_atomic_expiring_and_never_regrant_on_replay'],
  ['native_storage_budget', 'administrator_capacity_http_and_monitor_exit_codes'],
  ['native_retention', 'retention_observes_catalogue_missing_and_prepared_files_without_mutation'],
  ['native_sync_v3', 'real_service_provision_upload_advance_replay_concurrency_fetch_and_revoke'],
  ['native_clients_v3', 'two_native_clients_publish_clone_receive_divergence_and_local_operations'],
];
if (Object.keys(process.env).some(name => name.startsWith('LAYRS_TEST_') && name.endsWith('_ONLY'))) {
  throw new Error('Remove targeted LAYRS_TEST_*_ONLY modes before full native PostgreSQL qualification');
}
// The native TLS scenario executes the actual supervised WASM worker. A clean
// checkout must build it explicitly rather than rely on a previous local run.
const buildArgs = ['build', '--locked', '-p', 'layrs-weave', '--bin', 'layrs-lens-worker'];
if (process.argv.includes('--offline')) buildArgs.push('--offline');
const worker = spawnSync('cargo', buildArgs, { windowsHide: true, stdio: 'inherit' });
if (worker.error || worker.status !== 0) process.exit(worker.status ?? 1);
for (const [suite, test] of suites) {
  const args = ['test', '--locked'];
  if (process.argv.includes('--offline')) args.push('--offline');
  args.push('-p', 'layrs-server', '--test', suite, '--', '--exact', test,
    '--ignored', '--nocapture', '--test-threads=1');
  // Child entrypoints must be invoked only by their parent fixture. Selecting the
  // actual test by exact name also prevents a helper returning early from being
  // mistaken for PostgreSQL coverage. Credentials remain environment-only.
  const result = spawnSync('cargo', args, {
    encoding: 'utf8', windowsHide: true, maxBuffer: 32 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  process.stdout.write(result.stdout ?? '');
  if (result.error || result.status !== 0) process.exit(result.status ?? 1);
  const summaries = [...result.stdout.matchAll(/test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;/g)];
  if (summaries.length !== 1 || summaries.some(([, passed, failed, ignored]) =>
    Number(passed) !== 1 || Number(failed) !== 0 || Number(ignored) !== 0)) {
    throw new Error(`Mandatory PostgreSQL fixture ${suite}::${test} did not execute exactly once`);
  }
}
console.log(`${suites.length} native PostgreSQL integration suites executed; none skipped`);
