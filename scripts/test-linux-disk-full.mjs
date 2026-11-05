// Actual ENOSPC in an isolated, bounded tmpfs; source is read-only and Cargo
// outputs live in Linux volumes. No host filesystem is filled or cleaned.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const docker = [];
if (process.env.LAYRS_TEST_DOCKER_CONFIG) docker.push('--config', process.env.LAYRS_TEST_DOCKER_CONFIG);
if (process.env.LAYRS_TEST_DOCKER_HOST) docker.push('-H', process.env.LAYRS_TEST_DOCKER_HOST);
const image = process.env.LAYRS_LINUX_TEST_IMAGE ?? 'layrs-linux-tests:validation';
const buildVolume = process.env.LAYRS_LINUX_BUILD_VOLUME ?? 'layrs-disk-full-build';
const cargoVolume = process.env.LAYRS_LINUX_CARGO_VOLUME ?? 'layrs-disk-full-cargo';
for (const name of [buildVolume, cargoVolume]) assert(/^[a-zA-Z0-9][a-zA-Z0-9_.-]+$/.test(name));
const registry = process.env.LAYRS_LINUX_CARGO_REGISTRY
  ? `type=bind,source=${path.resolve(process.env.LAYRS_LINUX_CARGO_REGISTRY)},target=/usr/local/cargo/registry`
  : `type=volume,source=${cargoVolume},target=/usr/local/cargo/registry`;
const args = [...docker, 'run', '--rm', '--network', 'none',
  '--mount', `type=bind,source=${root},target=/src,readonly`,
  '--mount', `type=volume,source=${buildVolume},target=/build`,
  '--mount', registry,
  '--tmpfs', '/layrs-full:rw,nosuid,nodev,noexec,size=64m,mode=0700',
  '-e', 'CARGO_TARGET_DIR=/build', '-e', 'LAYRS_TEST_TMPFS_ROOT=/layrs-full',
  '-w', '/src'];
if (process.env.LAYRS_LINUX_CARGO_CACHE) args.push('--mount',
  `type=bind,source=${path.resolve(process.env.LAYRS_LINUX_CARGO_CACHE)},target=/usr/local/cargo/registry/cache,readonly`);
// The validation image contains 1.90.0. Override the checkout's MSRV toolchain
// rather than cause rustup to attempt a download in the networkless container.
args.push('-e', `RUSTUP_TOOLCHAIN=${process.env.LAYRS_LINUX_RUST_TOOLCHAIN ?? '1.90.0'}`);
for (const [crate, test] of [['layrs-client-core', 'native_disk_full'], ['layrs-store-server', 'v3_disk_full']]) {
  const invocation = [...args, image, 'cargo', 'test', '--offline', '--locked', '-p', crate,
    '--test', test, '--', '--ignored', '--nocapture', '--test-threads=1'];
  const result = spawnSync('docker', invocation, {
    encoding: 'utf8', windowsHide: true, maxBuffer: 16 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  process.stdout.write(result.stdout ?? '');
  if (result.error || result.status !== 0) process.exit(result.status ?? 1);
  assert.match(result.stdout, /test result: ok\. 1 passed; 0 failed; 0 ignored;/,
    'the real ENOSPC integration test must execute; a cfg-skipped test is not success');
}
console.log('Two Linux actual disk-full gates passed; disposable tmpfs removed with containers');
