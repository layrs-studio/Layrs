// CI-only local PostgreSQL lifecycle. Existing explicit test endpoints stay external.
import { randomBytes } from 'node:crypto';
import { existsSync, mkdirSync, readdirSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

export async function freePort() {
  const server = createServer();
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  return port;
}

export async function testPostgres(directory) {
  if (process.env.LAYRS_TEST_DATABASE_URL) {
    if (process.env.LAYRS_TEST_DATABASE_ISOLATED !== '1') throw new Error('Explicit disposable PostgreSQL consent is required');
    return { environment: process.env, close() {} };
  }
  if (process.platform !== 'win32') throw new Error('Set the isolated PostgreSQL test endpoint');
  // Hosted Windows runners supply PostgreSQL. Fail explicitly if the runner
  // image changes; never download or silently omit the team test.
  const parent = join(process.env.ProgramFiles || 'C:/Program Files', 'PostgreSQL');
  const candidates = process.env.PGBIN ? [process.env.PGBIN] : existsSync(parent)
    ? readdirSync(parent).sort((a, b) => b.localeCompare(a, undefined, { numeric: true })).map(version => join(parent, version, 'bin')) : [];
  const bin = candidates.find(path => existsSync(join(path, 'initdb.exe')) && existsSync(join(path, 'pg_ctl.exe')));
  if (!bin) throw new Error('PostgreSQL binaries missing: install the CI prerequisite or set PGBIN');
  const password = randomBytes(32).toString('hex');
  const passwordFile = join(directory, 'postgres-password');
  writeFileSync(passwordFile, password, { flag: 'wx', mode: 0o600 });
  const data = join(directory, 'postgres');
  mkdirSync(data);
  const run = (program, args) => {
    const result = spawnSync(join(bin, `${program}.exe`), args, { encoding: 'utf8', windowsHide: true, timeout: 120_000 });
    if (result.status !== 0) throw new Error(`Disposable PostgreSQL ${program} failed (details withheld)`);
  };
  const port = await freePort();
  run('initdb', ['-D', data, '-U', 'layrs_ci', '--auth-host=scram-sha-256', '--auth-local=trust', '--pwfile', passwordFile]);
  run('pg_ctl', ['-D', data, '-l', join(directory, 'postgres.log'), '-o', `-h 127.0.0.1 -p ${port}`, '-w', 'start']);
  return {
    environment: { ...process.env, LAYRS_TEST_DATABASE_URL: `postgres://layrs_ci:${password}@127.0.0.1:${port}/postgres`, LAYRS_TEST_DATABASE_ISOLATED: '1' },
    close() { run('pg_ctl', ['-D', data, '-m', 'fast', '-w', 'stop']); },
  };
}
