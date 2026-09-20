import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { compareReports, PHASES, REPORT_SCHEMA } from './compare-native-benchmarks.mjs';

const thresholds = () => ({ schemaVersion: 'layrs.benchmark-thresholds.v1', reportSchema: REPORT_SCHEMA,
  relativePercent: 20, absoluteFloorMs: 100 });
const scenario = files => ({ files, directories: files / 1000, payloadBytes: files * 256,
  result: 'passed', measurements: PHASES.map(phase => ({ phase, elapsedMs: 1000 })) });
const report = () => ({ schemaVersion: REPORT_SCHEMA, platform: 'windows', profile: 'debug',
  result: 'passed', limitations: ['Synthetic repeated 256-byte payloads; fixture v1.'],
  cases: [scenario(10_000), scenario(99_000)] });

test('compatible cases may be ordered differently; improvements and exact boundary pass', () => {
  const baseline = report();
  const candidate = report();
  candidate.cases.reverse();
  candidate.cases[0].measurements[0].elapsedMs = 800;
  candidate.cases[1].measurements[0].elapsedMs = 1200;
  const snapshot = JSON.stringify({ baseline, candidate });
  const result = compareReports(baseline, candidate, thresholds());
  assert.equal(result.status, 'passed');
  assert.equal(result.measurements.length, 18);
  assert.equal(JSON.stringify({ baseline, candidate }), snapshot, 'inputs remain unchanged');
});

test('relative threshold AND absolute floor must be exceeded; zero baseline is well-defined', () => {
  const baseline = report(); const candidate = report();
  baseline.cases[0].measurements[0].elapsedMs = 10;
  candidate.cases[0].measurements[0].elapsedMs = 80;
  baseline.cases[0].measurements[1].elapsedMs = 0;
  candidate.cases[0].measurements[1].elapsedMs = 100;
  assert.equal(compareReports(baseline, candidate, thresholds()).status, 'passed');
  candidate.cases[0].measurements[1].elapsedMs = 101;
  const failed = compareReports(baseline, candidate, thresholds());
  assert.equal(failed.status, 'regression');
  assert.deepEqual(failed.measurements.filter(value => value.regressed).map(value => [value.files, value.phase]), [[10_000, 'initialize']]);
});

test('per-phase settings are explicit and do not conceal a different phase regression', () => {
  const config = thresholds(); config.phaseOverrides = { clone: { relativePercent: 50, absoluteFloorMs: 0 } };
  const candidate = report(); candidate.cases[0].measurements[3].elapsedMs = 1400;
  candidate.cases[1].measurements[5].elapsedMs = 1201;
  const result = compareReports(report(), candidate, config);
  assert.equal(result.status, 'regression');
  assert.deepEqual(result.measurements.filter(value => value.regressed).map(value => value.phase), ['receive-unchanged']);
});

test('failed, paused, running or forged-complete reports cannot produce a comparison', () => {
  for (const state of ['failed', 'paused', 'running', undefined]) {
    for (const where of ['report', 'case']) {
      const candidate = report();
      if (where === 'report') candidate.result = state; else candidate.cases[0].result = state;
      assert.throws(() => compareReports(report(), candidate, thresholds()));
    }
  }
  const baseline = report(); const candidate = report();
  baseline.cases[0].measurements.pop(); candidate.cases[0].measurements.pop();
  assert.throws(() => compareReports(baseline, candidate, thresholds()), /nine phases/);
});

test('schema, platform, profile, case dimensions and workload declarations must match', () => {
  const mutations = [
    value => { value.schemaVersion = 'layrs.local-volume-measurement.v1'; },
    value => { value.platform = 'linux'; }, value => { value.profile = 'release'; },
    value => { delete value.profile; }, value => { delete value.platform; },
    value => { value.cases.pop(); }, value => { delete value.cases[0].directories; },
    value => { value.cases[0].payloadBytes++; }, value => { value.cases[0].files = 10001; },
    value => { value.limitations[0] = 'Unique file payloads'; },
    value => { value.cases[0].chunking = 'different'; },
  ];
  for (const mutate of mutations) {
    const candidate = report(); mutate(candidate);
    assert.throws(() => compareReports(report(), candidate, thresholds()));
  }
});

test('duplicate cases/phases, reordered phases and missing values fail closed', () => {
  const duplicate = report(); duplicate.cases[1] = structuredClone(duplicate.cases[0]);
  assert.throws(() => compareReports(report(), duplicate, thresholds()), /duplicate case/);
  for (const mutate of [
    value => { value.cases[0].measurements[1].phase = 'generate'; },
    value => { value.cases[0].measurements.reverse(); },
    value => { delete value.cases[0].measurements[0].elapsedMs; },
  ]) { const candidate = report(); mutate(candidate); assert.throws(() => compareReports(report(), candidate, thresholds())); }
});

test('nonfinite, negative, fractional, unsafe and coerced measurements/thresholds are refused', () => {
  for (const number of [NaN, Infinity, -Infinity, -1, 0.5, Number.MAX_SAFE_INTEGER + 1, '1000', null]) {
    const candidate = report(); candidate.cases[0].measurements[0].elapsedMs = number;
    assert.throws(() => compareReports(report(), candidate, thresholds()), /finite safe integer/);
  }
  for (const number of [NaN, Infinity, -1, 1001, '20', null]) {
    const config = thresholds(); config.relativePercent = number;
    assert.throws(() => compareReports(report(), report(), config));
  }
  const config = thresholds(); config.phaseOverrides = { typo: { relativePercent: 20, absoluteFloorMs: 100 } };
  assert.throws(() => compareReports(report(), report(), config), /unsupported field/);
  assert.throws(() => compareReports(report(), report(), undefined));
});

test('real CLI returns 0/1/2, bounded inputs and original reports remain intact', () => {
  const directory = mkdtempSync(join(tmpdir(), 'layrs-benchmark-comparison-'));
  try {
    const baseline = join(directory, 'baseline.json'); const candidate = join(directory, 'candidate.json');
    const config = join(directory, 'thresholds.json');
    const save = (path, value) => writeFileSync(path, JSON.stringify(value));
    save(baseline, report()); save(candidate, report()); save(config, thresholds());
    const original = readFileSync(baseline);
    const command = fileURLToPath(new URL('./compare-native-benchmarks.mjs', import.meta.url));
    const run = () => spawnSync(process.execPath, [command, '--baseline', baseline, '--candidate', candidate, '--thresholds', config], { encoding: 'utf8' });
    let result = run(); assert.equal(result.status, 0, result.stderr);
    assert.match(JSON.parse(result.stdout).inputs.baselineSha256, /^[a-f0-9]{64}$/);
    const slower = report(); slower.cases[0].measurements[0].elapsedMs = 1201; save(candidate, slower);
    result = run(); assert.equal(result.status, 1); assert.equal(JSON.parse(result.stdout).status, 'regression');
    writeFileSync(candidate, '{"elapsedMs":NaN}'); result = run(); assert.equal(result.status, 2); assert.equal(result.stdout, '');
    writeFileSync(candidate, ' '.repeat(1024 * 1024 + 1)); assert.equal(run().status, 2);
    assert.deepEqual(readFileSync(baseline), original);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test('existing paused TLS artifact is refused instead of being interpreted as a fast run', () => {
  const path = new URL('../docs/plans/pme/measurements/T8-tls-volumes-leases-2026-09-20.json', import.meta.url);
  const paused = JSON.parse(readFileSync(path, 'utf8'));
  assert.throws(() => compareReports(paused, paused, thresholds()));
});
