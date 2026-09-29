// Explicit, fail-closed comparison of existing TLS volume JSON reports.
import { openSync, closeSync, fstatSync, readSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const REPORT_SCHEMA = 'layrs.tls-volume-measurement.v1';
export const PHASES = Object.freeze([
  'generate', 'initialize', 'provision-publish', 'clone', 'verify-all-clone-bytes',
  'receive-unchanged', 'capture-publish-one-file', 'receive-one-file',
  'verify-all-updated-bytes',
]);
const MAX_INPUT_BYTES = 1024 * 1024;
function requireThat(condition, message) {
  if (!condition) throw new Error(message);
}
function object(value, label) {
  requireThat(value !== null && typeof value === 'object' && !Array.isArray(value), `${label}: object required`);
}
function closed(value, keys, label) {
  object(value, label);
  requireThat(Object.keys(value).every(key => keys.includes(key)), `${label}: unsupported field`);
}
function integer(value, label, minimum = 0) {
  requireThat(Number.isSafeInteger(value) && value >= minimum, `${label}: finite safe integer >= ${minimum} required`);
}
function text(value, label) {
  requireThat(typeof value === 'string' && value.trim().length > 0 && value.length <= 2048, `${label}: nonempty bounded string required`);
}
function validateThreshold(value, label) {
  closed(value, ['relativePercent', 'absoluteFloorMs'], label);
  requireThat(typeof value.relativePercent === 'number' && Number.isFinite(value.relativePercent)
    && value.relativePercent >= 0 && value.relativePercent <= 1000,
  `${label}.relativePercent: finite number between 0 and 1000 required`);
  integer(value.absoluteFloorMs, `${label}.absoluteFloorMs`);
}
function validateConfiguration(config) {
  closed(config, ['schemaVersion', 'reportSchema', 'relativePercent', 'absoluteFloorMs', 'phaseOverrides'], 'thresholds');
  requireThat(config.schemaVersion === 'layrs.benchmark-thresholds.v1', 'unsupported thresholds schema');
  requireThat(config.reportSchema === REPORT_SCHEMA, 'thresholds target an unsupported report schema');
  validateThreshold({ relativePercent: config.relativePercent, absoluteFloorMs: config.absoluteFloorMs }, 'thresholds');
  if (config.phaseOverrides !== undefined) {
    closed(config.phaseOverrides, PHASES, 'thresholds.phaseOverrides');
    for (const [phase, value] of Object.entries(config.phaseOverrides)) validateThreshold(value, `thresholds.${phase}`);
  }
}
function validateReport(report, label) {
  closed(report, ['schemaVersion', 'platform', 'profile', 'result', 'cases', 'limitations', 'note', 'totalTestElapsedMs'], label);
  requireThat(report.schemaVersion === REPORT_SCHEMA, `${label}: unsupported report schema`);
  requireThat(report.result === 'passed', `${label}: report must be passed, not incomplete/failed/paused`);
  text(report.platform, `${label}.platform`);
  text(report.profile, `${label}.profile`);
  if (report.note !== undefined) text(report.note, `${label}.note`);
  if (report.totalTestElapsedMs !== undefined) integer(report.totalTestElapsedMs, `${label}.totalTestElapsedMs`);
  requireThat(Array.isArray(report.limitations) && report.limitations.length > 0 && report.limitations.length <= 32,
    `${label}: workload/limitations metadata required`);
  report.limitations.forEach(value => text(value, `${label}.limitations`));
  requireThat(Array.isArray(report.cases) && report.cases.length > 0 && report.cases.length <= 2,
    `${label}: one or two complete volume cases required`);
  const seen = new Set();
  for (const scenario of report.cases) {
    closed(scenario, ['files', 'directories', 'payloadBytes', 'result', 'measurements'], `${label}.case`);
    requireThat([10_000, 99_000].includes(scenario.files), `${label}: unsupported files dimension`);
    requireThat(!seen.has(scenario.files), `${label}: duplicate case`);
    seen.add(scenario.files);
    integer(scenario.directories, `${label}.directories`, 1);
    integer(scenario.payloadBytes, `${label}.payloadBytes`, 1);
    requireThat(scenario.directories === Math.ceil(scenario.files / 1000)
      && scenario.payloadBytes === scenario.files * 256, `${label}: inconsistent v1 workload dimensions`);
    requireThat(scenario.result === 'passed', `${label}: every case must be passed`);
    requireThat(Array.isArray(scenario.measurements) && scenario.measurements.length === PHASES.length,
      `${label}: all nine phases are required`);
    for (const [index, measured] of scenario.measurements.entries()) {
      closed(measured, ['phase', 'elapsedMs'], `${label}.measurement`);
      requireThat(measured.phase === PHASES[index], `${label}: missing, duplicate or reordered phase`);
      integer(measured.elapsedMs, `${label}.${measured.phase}.elapsedMs`);
    }
  }
}

/** Inputs are never changed. No automatic hardware/profile/schema conversion. */
export function compareReports(baseline, candidate, config) {
  validateConfiguration(config);
  validateReport(baseline, 'baseline');
  validateReport(candidate, 'candidate');
  for (const key of ['schemaVersion', 'platform', 'profile']) {
    requireThat(baseline[key] === candidate[key], `incompatible ${key}`);
  }
  requireThat(JSON.stringify(baseline.limitations) === JSON.stringify(candidate.limitations),
    'incompatible workload/limitations metadata');
  const sorted = report => [...report.cases].sort((a, b) => a.files - b.files);
  const left = sorted(baseline);
  const right = sorted(candidate);
  requireThat(left.length === right.length, 'incompatible case set');
  const measurements = [];
  for (let index = 0; index < left.length; index++) {
    const before = left[index];
    const after = right[index];
    for (const key of ['files', 'directories', 'payloadBytes']) {
      requireThat(before[key] === after[key], `incompatible case ${key}`);
    }
    for (let phaseIndex = 0; phaseIndex < PHASES.length; phaseIndex++) {
      const phase = PHASES[phaseIndex];
      const threshold = config.phaseOverrides?.[phase] ?? config;
      const baselineMs = before.measurements[phaseIndex].elapsedMs;
      const candidateMs = after.measurements[phaseIndex].elapsedMs;
      const allowedIncreaseMs = Math.max(Math.ceil(baselineMs * (threshold.relativePercent / 100)), threshold.absoluteFloorMs);
      integer(allowedIncreaseMs, 'computed allowance');
      const deltaMs = candidateMs - baselineMs;
      measurements.push({ files: before.files, phase, baselineMs, candidateMs, deltaMs,
        relativePercent: threshold.relativePercent, absoluteFloorMs: threshold.absoluteFloorMs,
        allowedIncreaseMs, regressed: deltaMs > allowedIncreaseMs });
    }
  }
  return {
    schemaVersion: 'layrs.benchmark-comparison.v1', reportSchema: REPORT_SCHEMA,
    status: measurements.some(value => value.regressed) ? 'regression' : 'passed',
    platform: baseline.platform, profile: baseline.profile,
    cases: left.map(({ files, directories, payloadBytes }) => ({ files, directories, payloadBytes })),
    measurements,
    limitations: ['Report declarations are checked; hardware, load, build provenance and statistical significance require external qualification.'],
  };
}

function readJson(path, label) {
  const file = openSync(path, 'r');
  try {
    const info = fstatSync(file);
    requireThat(info.isFile() && info.size > 0 && info.size <= MAX_INPUT_BYTES, `${label}: expected nonempty JSON file <= 1 MiB`);
    const buffer = Buffer.alloc(MAX_INPUT_BYTES + 1);
    let length = 0;
    while (length < buffer.length) {
      const count = readSync(file, buffer, length, buffer.length - length, null);
      if (!count) break;
      length += count;
    }
    requireThat(length <= MAX_INPUT_BYTES, `${label}: file exceeds 1 MiB`);
    const bytes = buffer.subarray(0, length);
    let value;
    try { value = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes)); }
    catch { throw new Error(`${label}: invalid UTF-8 JSON`); }
    return { value, sha256: createHash('sha256').update(bytes).digest('hex') };
  } finally { closeSync(file); }
}
function main(args) {
  const flags = new Map();
  for (let index = 0; index < args.length; index += 2) {
    const key = args[index];
    requireThat(['--baseline', '--candidate', '--thresholds'].includes(key)
      && typeof args[index + 1] === 'string' && !args[index + 1].startsWith('--') && !flags.has(key),
    'usage: --baseline FILE --candidate FILE --thresholds FILE (each exactly once)');
    flags.set(key, args[index + 1]);
  }
  requireThat(flags.size === 3, 'baseline, candidate and explicit thresholds are required');
  const baseline = readJson(flags.get('--baseline'), 'baseline');
  const candidate = readJson(flags.get('--candidate'), 'candidate');
  const config = readJson(flags.get('--thresholds'), 'thresholds');
  const result = compareReports(baseline.value, candidate.value, config.value);
  result.inputs = { baselineSha256: baseline.sha256, candidateSha256: candidate.sha256, thresholdsSha256: config.sha256 };
  console.log(JSON.stringify(result, null, 2));
  return result.status === 'passed' ? 0 : 1;
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { process.exitCode = main(process.argv.slice(2)); }
  catch (error) {
    console.error(`Benchmark comparison refused: ${error.message}`);
    process.exitCode = 2;
  }
}
