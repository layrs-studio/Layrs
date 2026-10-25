// Local CLI measurements on fresh, retained fixtures. No builds, cleanup, or VFS simulation.
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { cp, lstat, mkdir, mkdtemp, readFile, readdir, realpath, rename, writeFile } from "node:fs/promises";
import { arch, cpus, hostname, platform, release, tmpdir, totalmem } from "node:os";
import { join, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { parseArgs } from "node:util";
import { fileURLToPath } from "node:url";

const HELP = `Usage: node scripts/measure-worktrees.mjs --binary PATH --profile LABEL --mode MODE --audit-binary PATH --audit-profile LABEL --audit-sha256 HEX [options]
  --audit-binary PATH                           Current read-only format-2 auditor, required
  --audit-profile LABEL                         Auditor build profile, required
  --audit-sha256 HEX                            Required common auditor hash, pinned across cohorts
  --mode independent-copies | shared-worktrees   Required; never inferred from the binary
  --counts 1000,10000,99000                      Logical files per checkout (1..99000)
  --agents 1,2                                  Separate processes/checkouts
  --payloads code,assets,mixed                  Unique code / assets / alternating 50:50 mix
  --repeats 1                                   Additional rounds after first invocation (1..5)
  --cas-retries 2                               Additional CLI capture attempts (0..5)
  --timeout-ms 900000                           Command including launcher (1000..900000)
  --run-timeout-ms 14400000                     Whole run budget (1000..43200000)
  --output-parent PATH                          Existing directory; default OS temp directory
  --run-note TEXT                               Build revision / quiet-slot conditions
  --help                                       No files or CLI processes created
Reports and fixtures remain in a new layrs-worktrees-* directory under output-parent.
All cases require the same complete auditor; actual rows/history must fit its fixed budgets.
Caches are NEVER purged: first-invocation/repeat is not physical cold/warm isolation.
Native VFS is unqualified and unsupported by this runner. Coordinate a quiet slot first.`;

const SHARD_SIZE = 1000;
const PAYLOAD_BYTES = { code: 512, assets: 4096 };
const WINDOWS_HELPER = fileURLToPath(new URL("./measure-worktree-process.ps1", import.meta.url));
const OUTPUT_LIMIT = { stdout: 16 * 1024 * 1024, stderr: 1024 * 1024 };
export const AUDIT_TABLES = [
  "objects", "object_edges", "file_manifests", "file_manifest_chunks", "space_object_admissions",
  "steps", "step_parents", "repository_views", "repository_view_layer_heads", "repository_view_markers",
  "repository_view_releases", "repository_view_worktrees", "layers", "layer_parents", "step_observations",
  "operations", "operation_parents", "operation_rewrites", "operation_rewrite_targets", "worktrees",
  "markers", "marker_proofs", "releases", "release_proofs", "release_publications", "current_repository_views",
  "operation_heads", "object_pins", "gc_roots", "recovery_operations", "recovery_paths",
];
const now = () => new Date().toISOString();
const elapsed = start => Math.round((performance.now() - start) * 100) / 100;
const sha = bytes => createHash("sha256").update(bytes).digest("hex");
const requireThat = (condition, message) => { if (!condition) throw new Error(message); };

function integer(value, name, min, max) {
  const number = Number(value);
  requireThat(/^\d+$/.test(value) && Number.isSafeInteger(number) && number >= min && number <= max,
    `${name} must be an integer in ${min}..${max}`);
  return number;
}

function list(value, name, allowed) {
  const entries = value.split(",");
  requireThat(entries.length > 0 && new Set(entries).size === entries.length && entries.every(allowed),
    `Invalid or repeated ${name}: ${value}`);
  return entries;
}

function fileKind(shape, index) { return shape === "mixed" ? (index % 2 === 0 ? "code" : "assets") : shape; }

// Called only after a failed command; keep the capture retry policy testable without launching a CLI.
export function canRetryCapture(record, { retry, attempt, casRetries, stopped }) {
  const message = typeof record.error === "string" ? record.error : JSON.stringify(record.error);
  const cas = /compare-and-swap mismatch|Worktree or repository changed during capture; read a new base and recapture/.test(message);
  return retry && cas && record.exitCode !== null && !record.timedOut &&
    !record.outputLimitExceeded && !record.invalidJson && !record.helperError &&
    !record.cancelled && attempt < casRetries && !stopped;
}

export function logicalBytes(shape, count) {
  return shape === "mixed" ? Math.ceil(count / 2) * PAYLOAD_BYTES.code + Math.floor(count / 2) * PAYLOAD_BYTES.assets
    : count * PAYLOAD_BYTES[shape];
}

export function relativeFile(shape, index) {
  return `${shape}/d${String(Math.floor(index / SHARD_SIZE)).padStart(3, "0")}/f${String(index).padStart(6, "0")}.${fileKind(shape, index) === "code" ? "mjs" : "bin"}`;
}

export function payload(shape, index, variant = "base") {
  shape = fileKind(shape, index);
  const seed = `layrs-worktrees-v1:${shape}:${index}:${variant}`;
  if (shape === "assets") {
    const bytes = createHash("shake256", { outputLength: PAYLOAD_BYTES.assets }).update(seed).digest();
    bytes.writeUInt32LE(index); // Distinct file identity, including in the high-entropy payload.
    return bytes;
  }
  const source = `// ${seed}\nexport const fileId = ${index};\nexport const revision = ${JSON.stringify(variant)};\n// ${sha(seed)} `;
  return Buffer.from(source.padEnd(PAYLOAD_BYTES.code - 1, ".") + "\n");
}

export function validateDiff(data, expected) {
  requireThat(data && Object.keys(data).length === 1 && Array.isArray(data.changes),
    "Diff must contain only the CLI changes array; missing/fixture-only responses are invalid");
  requireThat(data.changes.length === expected.length, "Diff change count differs from expected edits");
  const changes = [...data.changes].sort((left, right) => String(left?.path).localeCompare(String(right?.path)));
  const wanted = [...expected].sort((left, right) => left.path.localeCompare(right.path));
  changes.forEach((change, index) => {
    const target = wanted[index];
    requireThat(change && !Object.hasOwn(change, "fixtureOnly") && change.path === target.path &&
      change.status === "modified" && change.beforeBytes === target.bytes && change.afterBytes === target.bytes &&
      change.executableChanged === false && typeof change.beforeDigest === "string" && change.beforeDigest.length > 0 &&
      typeof change.afterDigest === "string" && change.afterDigest.length > 0 && change.beforeDigest !== change.afterDigest,
    `Diff must report the exact modified path and byte counts: ${target.path}`);
  });
  return changes.map(({ path, status, beforeBytes, afterBytes }) => ({ path, status, beforeBytes, afterBytes }));
}

export function validateHistory(data, expectedKinds, expectedPrefixIds, expectedRootId, spaceId) {
  requireThat(data && Object.keys(data).length === 1 && Array.isArray(data.operations),
    "History must contain only the CLI operations array; missing/fixture-only responses are invalid");
  const operations = data.operations;
  requireThat(operations.length === expectedKinds.length, "History cardinality differs from the natural fixture lifecycle");
  const ids = operations.map(operation => operation?.operationId);
  requireThat(new Set(ids).size === ids.length, "History contains repeated Operations");
  operations.forEach((operation, index) => {
    const expectedParents = index + 1 < operations.length ? [ids[index + 1]] : [];
    requireThat(operation && !Object.hasOwn(operation, "fixtureOnly") && operation.schemaVersion === "layrs.operation.v1" &&
      typeof operation.operationId === "string" && operation.operationId.startsWith("operation:") &&
      operation.spaceId === spaceId && typeof operation.resultingViewId === "string" &&
      /^blake3:[0-9a-f]{64}$/.test(operation.resultingViewId) && operation.kind === expectedKinds[index] &&
      Array.isArray(operation.parentOperationIds) && JSON.stringify(operation.parentOperationIds) === JSON.stringify(expectedParents),
    `History Operation ${index} has an unexpected identity, kind, or parent chain`);
    if (index < expectedPrefixIds.length) requireThat(ids[index] === expectedPrefixIds[index],
      "History does not retain the observed capture/setup Operations");
  });
  requireThat(ids.at(-1) === expectedRootId, "History does not terminate at the expected initialization Operation");
  return { operationCount: operations.length, expectedOperationCount: expectedKinds.length,
    operationIds: ids, kinds: operations.map(operation => operation.kind),
    parentOperationIds: operations.map(operation => operation.parentOperationIds), spaceId,
    equalWorkComparisonEligible: false,
    comparisonExclusion: "Natural lifecycle history; secondary creation adds two Operations. No equal-work history claim." };
}

export function auditExpectation(scenario, agents, storeRoot) {
  const states = agents.map(agent => scenario.initialStates[agent]);
  const histories = scenario.histories.filter(history => agents.includes(history.agent));
  const checks = scenario.privateByteChecks.filter(check => agents.includes(check.agent));
  const unique = values => [...new Set(values)].sort();
  return { storeRoot, storeId: states[0].storeId,
    checkoutRoots: agents.map(agent => scenario.roots[agent]).sort(),
    worktreeIds: unique(states.map(state => state.worktreeId)), spaceIds: unique(states.map(state => state.spaceId)),
    stepIds: unique(states.map(state => state.stepId).concat(checks.map(check => check.capturedStepId))),
    operationIds: unique(histories.flatMap(history => history.operationIds)),
    operationParents: unique(histories.flatMap(history => history.operationIds.flatMap((id, index) =>
      history.parentOperationIds[index].map(parent => JSON.stringify([id, parent]))))).map(pair => JSON.parse(pair)),
    fileManifests: scenario.logicalFiles + checks.length,
    fileBytes: scenario.logicalBytesPerCheckout + checks.reduce((bytes, check) =>
      bytes + PAYLOAD_BYTES[check.path.endsWith(".mjs") ? "code" : "assets"], 0),
  };
}

export function validateAudit(data, expected) {
  requireThat(data?.schemaVersion === "layrs.worktree-audit.v4" && data.readOnly === true &&
    data.formatVersion === 2 && data.storeId === expected.storeId && typeof data.storeRoot === "string" &&
    resolve(data.storeRoot) === resolve(expected.storeRoot) && !Object.hasOwn(data, "fixtureOnly"),
  "Audit must identify the expected read-only format-2 Store");
  requireThat(data.clean === true && data.complete === true && data.truncatedCount === 0 && !data.outputTruncated,
    "Audit must be clean and complete with zero truncation");
  const budgets = data.budgets, preflight = data.preflight;
  requireThat(budgets?.maxFindings === 15625 && budgets.workRows === 1000000 && budgets.historyRowsPerTable === 256 &&
    budgets.canonicalMetadataBytes === 256 * 1024 * 1024 && budgets.chunkBytes === 4 * 1024 ** 3 &&
    budgets.fileBytes === 4 * 1024 ** 3 && budgets.traversalWorkUnits === 4000000,
  "Auditor budgets differ from the approved fixed contract");
  requireThat(preflight?.complete === true && preflight.observedCountsWithinBudgets === true,
    "Audit preflight is incomplete or its observed counts exceed a budget");
  const counts = preflight.tableRows;
  requireThat(counts && Object.keys(counts).length === AUDIT_TABLES.length + 1 &&
    [...AUDIT_TABLES, "spaces"].every(table => Number.isSafeInteger(counts[table]) && counts[table] >= 0),
  "Audit preflight must report every actual work-table count and spaces");
  const workRows = AUDIT_TABLES.reduce((sum, table) => sum + counts[table], 0);
  requireThat(workRows === preflight.workRows && workRows <= budgets.workRows, "Audit actual row budget exceeded or inconsistent");
  for (const [field, table] of [["stepIds", "steps"], ["operationIds", "operations"], ["worktreeIds", "worktrees"], ["spaceIds", "spaces"]]) {
    const ids = preflight[field];
    requireThat(Array.isArray(ids) && ids.length === counts[table] && ids.length <= budgets.historyRowsPerTable &&
      ids.every(id => typeof id === "string" && id.length > 0 && id.length <= 160) &&
      JSON.stringify([...ids].sort()) === JSON.stringify([...expected[field]].sort()),
    `Audit ${field} must exactly match observed fixture/history identities`);
  }
  requireThat(Array.isArray(preflight.operationParents) && preflight.operationParents.length === counts.operation_parents &&
    counts.operation_parents <= budgets.historyRowsPerTable && counts.step_parents <= budgets.historyRowsPerTable &&
    JSON.stringify(preflight.operationParents.map(pair => JSON.stringify(pair)).sort()) ===
      JSON.stringify(expected.operationParents.map(pair => JSON.stringify(pair)).sort()),
  "Audit Operation parent edges must exactly match observed history");
  // Every fixture file is below the existing 1 MiB fixed chunk size; every version is unique.
  requireThat(counts.file_manifests === expected.fileManifests && counts.file_manifest_chunks === expected.fileManifests &&
    preflight.uniqueChunks === expected.fileManifests && preflight.fileBytes === expected.fileBytes &&
    preflight.chunkBytes === expected.fileBytes,
  "Audit manifests/unique chunks/retained bytes differ from the unique initial and edited payloads");
  requireThat(Array.isArray(data.checkoutBindings) && data.checkoutBindings.length === expected.checkoutRoots.length &&
    data.checkoutBindings.every(binding => binding?.validatedBeforeAndAfter === true &&
      typeof binding.checkoutRoot === "string" && typeof binding.storeRoot === "string" &&
      resolve(binding.storeRoot) === resolve(expected.storeRoot)) &&
    JSON.stringify(data.checkoutBindings.map(binding => resolve(binding.checkoutRoot)).sort()) ===
      JSON.stringify(expected.checkoutRoots.map(root => resolve(root)).sort()),
  "Audit must validate every expected checkout binding before and after fsck");
  for (const [field, budget] of [["canonicalObjectBytes", "canonicalMetadataBytes"], ["chunkBytes", "chunkBytes"], ["fileBytes", "fileBytes"]]) {
    requireThat(Number.isSafeInteger(preflight[field]) && preflight[field] >= 0 && preflight[field] <= budgets[budget],
      `Audit ${field} is unavailable or exceeds its fixed budget`);
  }
  for (const field of ["checkedObjects", "checkedRows", "retentionRoots", "reachableObjects", "unreachableObjects", "reconstructibleProjections"]) {
    requireThat(Number.isSafeInteger(data[field]) && data[field] >= 0, `Audit counter ${field} is missing or invalid`);
  }
  requireThat(data.checkedObjects > 0 && data.checkedRows > 0 && Array.isArray(data.findings) &&
    data.findings.length <= budgets.maxFindings && data.findings.every(finding => ["info", "warning"].includes(finding?.severity)),
  "Audit lacks checked objects/rows or contains error/invalid findings");
  requireThat(data.unreachableObjects === 0, "Fresh retained fixture must have zero unreachable objects");
  const physical = data.physicalStore;
  requireThat(physical?.complete === true && physical.reconciled === true && !physical.error &&
    physical.entryLimit === 2000000 && physical.depthLimit === 16 && Number.isSafeInteger(physical.visitedEntries) &&
    physical.visitedEntries > 0 && physical.visitedEntries <= physical.entryLimit,
  "Physical Store accounting must be complete, bounded and reconciled");
  for (const category of ["objects", "metadata", "repairBackups", "other", "staging", "total", "indexedExpected"]) {
    requireThat(["files", "bytes"].every(field => Number.isSafeInteger(physical[category]?.[field]) && physical[category][field] >= 0),
      `Physical Store ${category} counts are missing or invalid`);
  }
  requireThat(Number.isSafeInteger(physical.repairBackups.dirs) && physical.repairBackups.dirs >= 0,
    "Repair-backup directory count is missing or invalid");
  for (const category of ["repairBackups", "metadata", "other"]) {
    requireThat(Array.isArray(physical[category].samplePaths) && physical[category].samplePaths.length <= 8 &&
      physical[category].samplePaths.every(path => typeof path === "string" && path.length > 0),
    `Physical Store ${category} path samples must be present and bounded to eight`);
  }
  requireThat(physical.repairBackups.samplePaths.length === Math.min(8, physical.repairBackups.files + physical.repairBackups.dirs),
    "Repair-backup samples must cover the observed entries up to the eight-path limit");
  requireThat(physical.indexedExpected.files === counts.objects && physical.objects.files === counts.objects &&
    physical.indexedExpected.bytes === preflight.canonicalObjectBytes + preflight.chunkBytes &&
    physical.objects.bytes === physical.indexedExpected.bytes,
  "Physical CAS files/bytes must exactly equal the indexed object counts and stored bytes");
  requireThat(physical.staging.pendingEntries === 0 && physical.staging.leaseFiles === 1 && physical.staging.leaseBytes === 0 &&
    physical.staging.files === 1 && physical.staging.bytes === 0 && Array.isArray(physical.staging.samplePaths) &&
    physical.staging.samplePaths.length === 0,
  "Completed staging must contain only its empty .lease control file");
  for (const field of ["files", "bytes"]) requireThat(physical.total[field] ===
    ["objects", "metadata", "repairBackups", "other", "staging"].reduce((sum, category) => sum + physical[category][field], 0),
  `Physical Store total ${field} does not reconcile with its categories`);
}

async function hashFile(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

function ownState(data) {
  const worktree = data?.state?.worktree;
  const own = data?.state?.view?.worktrees?.find(item => item.worktreeId === worktree?.worktreeId);
  requireThat(own?.activeLayerId && own?.baseStepId && worktree?.storeId && worktree?.spaceId && worktree?.activeOperationId,
    "CLI status lacks explicit worktree, Store, Layer, or exact Step identity");
  return { worktreeId: worktree.worktreeId, storeId: worktree.storeId,
    layerId: own.activeLayerId, stepId: own.baseStepId, operationId: worktree.activeOperationId, spaceId: worktree.spaceId };
}

async function main() {
  const { values } = parseArgs({ options: Object.fromEntries([
    "binary", "profile", "mode", "audit-binary", "audit-profile", "audit-sha256", "counts", "agents", "payloads", "repeats", "cas-retries",
    "timeout-ms", "run-timeout-ms", "output-parent", "run-note",
  ].map(name => [name, { type: "string" }]).concat([["help", { type: "boolean" }]])) });
  if (values.help) { console.log(HELP); return; }
  const parent = await realpath(resolve(values["output-parent"] || tmpdir()));
  const base = await mkdtemp(join(parent, "layrs-worktrees-"));
  const reportPath = join(base, "report.json");
  const started = performance.now();
  const report = {
    schemaVersion: "layrs.worktrees-benchmark.v5", startedAt: now(), status: "running", success: false,
    invocation: process.argv.slice(2), fixtureRoot: base, reportPath,
    binary: { path: values.binary ? resolve(values.binary) : null, profileDeclared: values.profile ?? null },
    auditBinary: { path: values["audit-binary"] ? resolve(values["audit-binary"]) : null,
      profileDeclared: values["audit-profile"] ?? null, expectedSha256: values["audit-sha256"]?.toLowerCase() ?? null },
    machine: { hostname: hostname(), platform: platform(), release: release(), arch: arch(),
      cpu: cpus()[0]?.model ?? null, logicalCpus: cpus().length, totalMemoryBytes: totalmem(), node: process.version },
    cachePolicy: { name: "unpurged", physicalCachePurged: false,
      labels: "first-invocation / repeat within each phase; fresh CLI process every time",
      caveat: "Fixture generation, initialization, copying, and verification already touch OS caches." },
    resourceMetrics: { method: process.platform === "win32" ? "windows-owned-process-handle" : "unavailable",
      scope: "Invoked native process only; excludes launcher, descendants, Node, and filesystem cache",
      availability: "See per-command resourceMetrics; unsupported/failed counters are null with reasons." },
    operationCoverage: {
      creation: "init and, with two agents, worktree create or payload copy plus independent init",
      capture: "step", diff: "clean and dirty diff --stat; exact paths/status/byte counts validated",
      history: "timeline --limit 10; natural cardinality/kinds/ancestry validated, excluded from equal-work timing comparisons",
      integrity: "separate current read-only auditor per complete Store; clean and complete required",
      firstFileUsable: "not measured; command completion is not first-file latency",
      lockWait: "unavailable: CLI exposes no lock-wait counter; CAS retries are not lock-wait time",
      reception: "not measured: requires server fixtures", publication: "not measured: requires server fixtures",
      networkBytes: "not measured: requires server fixtures; process I/O is not a network-byte measurement",
      disk: "logical Store/payload/secondary-locator file sizes plus Windows process I/O; physical allocation/I/O not measured",
    },
    nativeVfs: { qualified: false, measured: false, reason: "Awaiting root integration and native platform qualification." },
    comparisonPolicy: { history: "All history timings are lifecycle diagnostics, excluded from equal-work comparisons.",
      cohorts: "Require matching binary.profileDeclared, auditBinary.profileDeclared and pinned auditor SHA-256. Root verifies baseline build provenance before real runs.",
      integrity: "Same auditor SHA-256 required across cohorts; separate audit-validation timings/counters excluded from CLI comparisons." },
    runNote: values["run-note"] || "Quiet slot and build provenance not supplied; timings are unqualified.",
    limitations: [
      "Independent copies each initialize a complete Store; shared worktrees use one complete Store. These are different storage models.",
      "CLI wall times include process startup; fixture creation, verification and reporting are outside command timings.",
      "Only external CLI CAS retries are observable. The CLI can retry internally; internal counts are unknown, not zero.",
      "Coordinated process launch does not force an internal CAS collision; overlap is reported, never assumed.",
      "File sizes are logical bytes, not allocated disk blocks, I/O counters, dedup savings, or memory private bytes.",
      "No network, publication, first-file latency, lock-wait instrumentation, or physical cache purging.",
      "Windows uses a hidden PowerShell launcher; native lifetime and launcher/collection overhead are reported separately.",
      "Windows process I/O includes file/device/pipe I/O, not physical disk or network bytes; descendants are excluded.",
      "Do not infer performance gains from debug builds, a single run, or runs overlapping compilation.",
    ], cases: [],
  };
  // Serialize checkpoints and atomically replace only this run's own report.
  let writes = Promise.resolve();
  let checkpointNumber = 0;
  function checkpoint() {
    writes = writes.catch(() => {}).then(async () => {
      report.updatedAt = now();
      const temporary = join(base, `report-${++checkpointNumber}.tmp`);
      await writeFile(temporary, JSON.stringify(report, null, 2) + "\n", { flag: "wx" });
      await rename(temporary, reportPath);
    });
    return writes;
  }
  await checkpoint();
  console.log(`Report: ${reportPath}`);
  let deadline = Infinity;
  let stopped = null;
  const children = new Set();
  function checkBudget() {
    requireThat(!stopped, stopped);
    requireThat(performance.now() < deadline, "Whole-run timeout exceeded");
  }
  function interrupt(signal) {
    stopped = `Interrupted by ${signal}`;
    for (const cancel of children) cancel();
  }
  const onInt = () => interrupt("SIGINT"), onTerm = () => interrupt("SIGTERM");
  process.on("SIGINT", onInt);
  process.on("SIGTERM", onTerm);
  let options;
  let commandNumber = 0;

  async function command(scenario, root, args, context, audit = false) {
    checkBudget();
    const id = ++commandNumber;
    const record = { id, attempt: 0, retryCount: 0, ...context, cwd: root,
      binaryPath: (audit ? report.auditBinary : report.binary).path, argv: ["--json", ...args], startedAt: now(),
      startOffsetMs: elapsed(started), exitCode: null, success: false, timedOut: false,
      outputLimitExceeded: false, internalRetryCount: null };
    (audit ? scenario.auditValidation.commands : scenario.commands).push(record);
    record.stdoutPath = join(base, "logs", `${id}.stdout.txt`);
    record.stderrPath = join(base, "logs", `${id}.stderr.txt`);
    const start = performance.now();
    let output;
    if (process.platform === "win32") {
      output = await windowsCommand(record, start);
    } else {
      record.resourceMetrics = { cpu: null, memory: null, io: null,
        unavailableReason: `No owned-process counter adapter implemented for ${process.platform}` };
      record.nativeElapsedWallMs = null;
      record.launcherOverheadMs = null;
      output = await new Promise(resolveRun => {
        const chunks = { stdout: [], stderr: [] }, sizes = { stdout: 0, stderr: 0 };
        const child = spawn(record.binaryPath, record.argv, { cwd: root, windowsHide: true,
          shell: false, stdio: ["ignore", "pipe", "pipe"] });
        const cancel = () => child.kill("SIGKILL");
        children.add(cancel);
        record.pid = child.pid ?? null;
        const timer = setTimeout(() => { record.timedOut = true; child.kill("SIGKILL"); },
          Math.max(1, Math.min(options.timeoutMs, deadline - performance.now())));
        for (const stream of ["stdout", "stderr"]) child[stream].on("data", bytes => {
          const available = OUTPUT_LIMIT[stream] - sizes[stream];
          if (available > 0) chunks[stream].push(bytes.subarray(0, available));
          sizes[stream] += bytes.length;
          if (sizes[stream] > OUTPUT_LIMIT[stream]) { record.outputLimitExceeded = true; child.kill("SIGKILL"); }
        });
        child.on("error", error => { record.spawnError = error.message; });
        // 'close' waits for both output streams; 'exit' can truncate the JSON envelope.
        child.on("close", (code, signal) => {
          clearTimeout(timer); children.delete(cancel);
          Object.assign(record, { exitCode: code, signal, elapsedWallMs: elapsed(start),
            endOffsetMs: elapsed(started), stdoutBytes: sizes.stdout, stderrBytes: sizes.stderr });
          resolveRun({ stdout: Buffer.concat(chunks.stdout).toString("utf8"),
            stderr: Buffer.concat(chunks.stderr).toString("utf8") });
        });
      });
      await writeFile(record.stdoutPath, output.stdout, { flag: "wx" });
      await writeFile(record.stderrPath, output.stderr, { flag: "wx" });
    }
    let envelope;
    try { envelope = JSON.parse(output.stdout); } catch { record.invalidJson = true; }
    record.success = record.exitCode === 0 && envelope?.ok === true && !record.timedOut &&
      !record.outputLimitExceeded && !record.spawnError && !record.helperError && !record.cancelled && !stopped;
    record.error = envelope?.error ?? record.spawnError ?? record.helperError ?? (record.success ? null : output.stderr.slice(0, 4096));
    if (audit) record.auditResult = envelope?.data ?? null; // Retain failed/incomplete preflight before throwing.
    await checkpoint();
    return { record, data: envelope?.data };
  }

  async function windowsCommand(record, start) {
    const timeoutMs = Math.max(1, Math.min(options.timeoutMs, deadline - performance.now()));
    const request = { binary: record.binaryPath, cwd: record.cwd, argv: record.argv,
      stdoutPath: record.stdoutPath, stderrPath: record.stderrPath,
      stdoutLimit: OUTPUT_LIMIT.stdout, stderrLimit: OUTPUT_LIMIT.stderr, deadlineUnixMs: Date.now() + timeoutMs };
    const summary = await new Promise(resolveRun => {
      const child = spawn(report.resourceMetrics.launcherPath,
        ["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", WINDOWS_HELPER],
        { windowsHide: true, shell: false, stdio: ["pipe", "pipe", "pipe"] });
      record.launcherPid = child.pid ?? null;
      // EOF cancels through the helper's retained handle, never by a possibly reused PID.
      // Before the explicit start grant, killing our launcher is safe: no CLI exists yet.
      const cancel = () => { child.stdin.end(); if (!record.launchGranted) child.kill("SIGKILL"); };
      children.add(cancel);
      const timer = setTimeout(() => { record.timedOut = true; cancel(); }, timeoutMs);
      const chunks = { stdout: [], stderr: [] }, sizes = { stdout: 0, stderr: 0 };
      let readiness = "", readinessRead = false;
      for (const stream of ["stdout", "stderr"]) child[stream].on("data", bytes => {
        const available = 64 * 1024 - sizes[stream];
        if (available > 0) chunks[stream].push(bytes.subarray(0, available));
        sizes[stream] += bytes.length;
        if (sizes[stream] > 64 * 1024) { record.helperError = "Launcher output cap exceeded"; cancel(); }
        if (stream === "stdout" && !readinessRead && sizes.stdout <= 64 * 1024) {
          readiness += bytes.toString("utf8");
          if (readiness.includes("\n")) {
            readinessRead = true;
            let message;
            try { message = JSON.parse(readiness.split("\n")[0]); } catch { /* Diagnosed below from the retained summary. */ }
            if (Date.now() >= request.deadlineUnixMs) record.timedOut = true;
            if (message?.ready === true && !record.timedOut && !record.helperError && !stopped) {
              record.launchGranted = true;
              child.stdin.write("start\n");
            } else { cancel(); }
          }
        }
      });
      child.stdin.on("error", error => { record.helperError = `Launcher input failed: ${error.code}`; });
      child.on("error", error => { record.spawnError = error.message; });
      child.on("close", (code, signal) => {
        clearTimeout(timer); children.delete(cancel);
        Object.assign(record, { launcherExitCode: code, signal, elapsedWallMs: elapsed(start), endOffsetMs: elapsed(started) });
        resolveRun({ stdout: Buffer.concat(chunks.stdout).toString("utf8"), stderr: Buffer.concat(chunks.stderr).toString("utf8") });
      });
      child.stdin.write(JSON.stringify(request) + "\n"); // Leave open as a parent-lifetime signal.
    });
    record.launcherStdoutPath = join(base, "logs", `${record.id}.launcher.stdout.txt`);
    record.launcherStderrPath = join(base, "logs", `${record.id}.launcher.stderr.txt`);
    await writeFile(record.launcherStdoutPath, summary.stdout, { flag: "wx" });
    await writeFile(record.launcherStderrPath, summary.stderr, { flag: "wx" });
    let measured;
    try { measured = JSON.parse(summary.stdout.trim().split(/\r?\n/).at(-1)); }
    catch { record.helperError ??= "Launcher returned invalid JSON"; }
    record.helperError ??= measured?.helperError ?? measured?.outputError ??
      (measured?.drainTimedOut ? "CLI output drain timed out" : null) ??
      (record.launcherExitCode !== 0 ? "Launcher failed" : null);
    record.pid = measured?.pid ?? null;
    record.exitCode = measured?.exitCode ?? null;
    record.timedOut ||= measured?.timedOut === true;
    record.cancelled = measured?.cancelled === true;
    record.outputLimitExceeded = measured?.outputLimitExceeded === true;
    record.stdoutBytes = measured?.stdoutBytes ?? null;
    record.stderrBytes = measured?.stderrBytes ?? null;
    record.nativeElapsedWallMs = measured?.nativeElapsedWallMs ?? null;
    record.nativeStartedUnixMs = measured?.nativeStartedUnixMs ?? null;
    record.nativeEndedUnixMs = measured?.nativeEndedUnixMs ?? null;
    record.launcherOverheadMs = record.nativeElapsedWallMs === null ? null
      : Math.round((record.elapsedWallMs - record.nativeElapsedWallMs) * 100) / 100;
    record.resourceMetrics = { cpu: measured?.cpu ?? null, memory: measured?.memory ?? null, io: measured?.io ?? null,
      unavailableReason: measured?.launched ? null : record.helperError || "Native process was not launched" };
    const output = {};
    for (const stream of ["stdout", "stderr"]) {
      try { output[stream] = await readFile(record[`${stream}Path`], "utf8"); }
      catch (error) { if (error.code !== "ENOENT") throw error; output[stream] = ""; }
    }
    return output;
  }

  async function checked(scenario, root, args, context) {
    const result = await command(scenario, root, args, context);
    requireThat(result.record.success, `CLI ${args.join(" ")} failed (command ${result.record.id})`);
    return result.data;
  }

  async function group(scenario, phase, invocation, args, retry = false) {
    const batch = { phase, invocation, cacheLabel: invocation === 0 ? "first-invocation" : "repeat",
      agents: scenario.agents, success: false, externalRetryCount: 0, internalRetryCount: null,
      comparisonExclusion: phase === "history" ? report.comparisonPolicy.history : null };
    scenario.batches.push(batch);
    await checkpoint();
    const start = performance.now();
    const first = Array(scenario.agents).fill(null);
    // One event-loop turn releases all processes. Every failed attempt stays in commands/logs.
    const results = await Promise.allSettled(scenario.roots.map(async (root, agent) => {
      for (let attempt = 0; ; attempt++) {
        const result = await command(scenario, root, args, { phase, invocation, agent,
          cacheLabel: batch.cacheLabel, attempt, retryCount: attempt, comparisonExclusion: batch.comparisonExclusion });
        if (attempt === 0) first[agent] = result.record;
        if (result.record.success) return result.data;
        const retryable = canRetryCapture(result.record, { retry, attempt, casRetries: options.casRetries, stopped });
        result.record.retryScheduled = retryable;
        await checkpoint();
        requireThat(retryable, `CLI ${args.join(" ")} failed (command ${result.record.id})`);
        batch.externalRetryCount++;
        // A fresh CLI process reads a fresh base. Never retry an unknown error or repair the fixture.
      }
    }));
    batch.elapsedWallMs = elapsed(start); // Includes retry/report overhead; individual CLI timings do not.
    batch.initialLaunchSkewMs = first.length === scenario.agents && first.every(Boolean)
      ? Math.max(...first.map(item => item.startOffsetMs)) - Math.min(...first.map(item => item.startOffsetMs)) : null;
    batch.observedLauncherOverlapMs = scenario.agents === 2 && first.length === 2 && first.every(Boolean)
      ? Math.max(0, Math.min(...first.map(item => item.endOffsetMs)) - Math.max(...first.map(item => item.startOffsetMs))) : null;
    batch.observedNativeOverlapMs = scenario.agents === 2 && first.every(item => item?.nativeStartedUnixMs != null && item?.nativeEndedUnixMs != null)
      ? Math.max(0, Math.min(...first.map(item => item.nativeEndedUnixMs)) - Math.max(...first.map(item => item.nativeStartedUnixMs))) : null;
    batch.success = results.every(result => result.status === "fulfilled");
    await checkpoint();
    const failed = results.find(result => result.status === "rejected");
    if (failed) throw failed.reason;
    return results.map(result => result.value);
  }

  async function measureFiles(root) {
    let files = 0, bytes = 0;
    for (const entry of await readdir(root, { withFileTypes: true })) {
      checkBudget();
      const path = join(root, entry.name), info = await lstat(path);
      requireThat(!info.isSymbolicLink(), `Unexpected link: ${path}`);
      if (info.isDirectory()) {
        const nested = await measureFiles(path); files += nested.files; bytes += nested.bytes;
      } else { requireThat(info.isFile(), `Unexpected file type: ${path}`); files++; bytes += info.size; }
    }
    return { files, bytes };
  }

  async function verifyPayload(root, shape, count, variant = "base") {
    const digest = createHash("sha256");
    for (let index = 0; index < count; index++) {
      checkBudget();
      const edited = index === 0 || (shape === "mixed" && index === 1);
      const path = relativeFile(shape, index), expected = payload(shape, index, edited ? variant : "base");
      const info = await lstat(join(root, path));
      requireThat(info.isFile() && !info.isSymbolicLink() && info.nlink === 1, `File is not private: ${path}`);
      const actual = await readFile(join(root, path));
      requireThat(actual.equals(expected), `Payload mismatch: ${root}/${path}`);
      digest.update(path).update("\0").update(actual);
    }
    const storage = await measureFiles(join(root, shape));
    requireThat(storage.files === count && storage.bytes === logicalBytes(shape, count), "Payload file count/bytes changed");
    return { ...storage, sha256: digest.digest("hex"), privateFiles: true };
  }

  async function runCase(shape, count, agents) {
    const caseStart = performance.now();
    const root = join(base, `${shape}-${count}-${agents}`);
    const scenario = { shape, logicalFiles: count, agents, storageModel: options.mode, root,
      logicalDirectories: 1 + Math.ceil(count / SHARD_SIZE), logicalBytesPerCheckout: logicalBytes(shape, count),
      codeFiles: shape === "mixed" ? Math.ceil(count / 2) : shape === "code" ? count : 0,
      assetFiles: shape === "mixed" ? Math.floor(count / 2) : shape === "assets" ? count : 0,
      roots: [], commands: [], batches: [], storeBytes: null, privateCheckoutBytes: null,
      auditValidation: { commands: [], success: false, comparisonExclusion: report.comparisonPolicy.integrity },
      success: false, status: "running" };
    scenario.projectionEntriesWithRoot = count + scenario.logicalDirectories + 1;
    requireThat(scenario.projectionEntriesWithRoot <= 100000, "Projection budget exceeded including directories");
    report.cases.push(scenario);
    await checkpoint();
    try {
      await mkdir(root);
      const primary = join(root, "agent-0"); await mkdir(primary); scenario.roots.push(primary);
      const generationStart = performance.now();
      await mkdir(join(primary, shape));
      for (let index = 0; index < count; index++) {
        checkBudget();
        if (index % SHARD_SIZE === 0) await mkdir(join(primary, shape, `d${String(index / SHARD_SIZE).padStart(3, "0")}`));
        await writeFile(join(primary, relativeFile(shape, index)), payload(shape, index), { flag: "wx" });
      }
      scenario.fixtureGenerationMs = elapsed(generationStart);
      await checked(scenario, primary, ["init", `Benchmark ${shape} ${count}`], { phase: "setup", agent: 0 });
      const initial = ownState(await checked(scenario, primary, ["status"], { phase: "setup", agent: 0 }));
      scenario.initialStates = [initial];
      if (agents === 2) {
        const secondary = join(root, "agent-1"); await mkdir(secondary); scenario.roots.push(secondary);
        if (options.mode === "shared-worktrees") {
          await checked(scenario, primary, ["worktree", "create", secondary, "--layer", initial.layerId,
            "--step", initial.stepId], { phase: "provision", agent: 1 });
        } else {
          const copyStart = performance.now();
          await cp(join(primary, shape), join(secondary, shape), { recursive: true, force: false, errorOnExist: true });
          scenario.payloadCopyMs = elapsed(copyStart);
          checkBudget();
          await checked(scenario, secondary, ["init", `Benchmark ${shape} ${count}`], { phase: "provision", agent: 1 });
        }
        const second = ownState(await checked(scenario, secondary, ["status"], { phase: "setup", agent: 1 }));
        requireThat((initial.storeId === second.storeId) === (options.mode === "shared-worktrees"), "Incorrect Store sharing");
        requireThat(initial.worktreeId !== second.worktreeId, "Worktree identities must differ");
        if (options.mode === "shared-worktrees") requireThat(initial.stepId === second.stepId && initial.layerId === second.layerId,
          "Shared worktree did not start from the exact requested Layer and Step");
        scenario.initialStates.push(second);
      }
      scenario.initialPayloads = [];
      for (const checkout of scenario.roots) scenario.initialPayloads.push(await verifyPayload(checkout, shape, count));
      scenario.diffChecks = [];
      for (let invocation = 0; invocation <= options.repeats; invocation++) {
        const clean = await group(scenario, "clean-status", invocation, ["status"]);
        requireThat(clean.every(data => data.dirty === false && data.needsRepair === false), "Initial checkout must be clean and healthy");
        const diffs = await group(scenario, "clean-diff", invocation, ["diff", "--stat"]);
        diffs.forEach((data, agent) => scenario.diffChecks.push({ phase: "clean", invocation, agent,
          changes: validateDiff(data, []), success: true }));
      }
      const editedIndices = shape === "mixed" ? [0, 1] : [0];
      scenario.editedPaths = editedIndices.map(index => relativeFile(shape, index));
      scenario.privateByteChecks = [];
      scenario.capturedOperationIds = scenario.roots.map(() => []);
      for (let invocation = 0; invocation <= options.repeats; invocation++) {
        // Same path, distinct deterministic bytes in each checkout; no shared writable inode.
        for (let agent = 0; agent < agents; agent++) for (const index of editedIndices) {
          checkBudget();
          const path = join(scenario.roots[agent], relativeFile(shape, index)), info = await lstat(path);
          requireThat(info.isFile() && !info.isSymbolicLink() && info.nlink === 1, "Edit target is not a private regular file");
          await writeFile(path, payload(shape, index, `agent-${agent}-round-${invocation}`), { flag: "r+" });
        }
        const dirty = await group(scenario, "dirty-status", invocation, ["status"]);
        requireThat(dirty.every(data => data.dirty === true && data.needsRepair === false), "Edited checkout must be dirty and healthy");
        const diffs = await group(scenario, "dirty-diff", invocation, ["diff", "--stat"]);
        const expectedChanges = editedIndices.map(index => ({ path: relativeFile(shape, index), bytes: PAYLOAD_BYTES[fileKind(shape, index)] }));
        diffs.forEach((data, agent) => scenario.diffChecks.push({ phase: "dirty", invocation, agent,
          changes: validateDiff(data, expectedChanges), success: true }));
        const captures = await group(scenario, "capture", invocation, ["step"], true);
        requireThat(captures.every(data => data.status === "captured" && typeof data.stepId === "string"), "Expected an accepted capture per agent");
        requireThat(new Set(captures.map(data => data.stepId)).size === agents, "Distinct edits must produce distinct Steps");
        const reopened = await group(scenario, "reopen-status", invocation, ["status"]);
        for (let agent = 0; agent < agents; agent++) for (const index of editedIndices) {
          const state = ownState(reopened[agent]);
          if (index === 0) scenario.capturedOperationIds[agent].push(state.operationId);
          const path = relativeFile(shape, index);
          const actual = await readFile(join(scenario.roots[agent], path));
          const expected = payload(shape, index, `agent-${agent}-round-${invocation}`);
          const check = { agent, invocation, path, expectedSha256: sha(expected), actualSha256: sha(actual),
            capturedStepId: captures[agent].stepId, reopenedState: state,
            success: actual.equals(expected) && state.stepId === captures[agent].stepId &&
              reopened[agent].dirty === false && reopened[agent].needsRepair === false };
          scenario.privateByteChecks.push(check);
          requireThat(check.success, `Agent ${agent} lost its private bytes or captured Step after reopening`);
        }
        await checkpoint();
      }
      scenario.histories = [];
      for (let invocation = 0; invocation <= options.repeats; invocation++) {
        const histories = await group(scenario, "history", invocation, ["timeline", "--limit", "10"]);
        histories.forEach((data, agent) => {
          const secondary = options.mode === "shared-worktrees" && agent === 1;
          const kinds = [...Array(options.repeats + 1).fill("capture_worktree"),
            ...Array(secondary ? 2 : 0).fill("restore"), "initialize"];
          const prefix = [...scenario.capturedOperationIds[agent]].reverse().concat(scenario.initialStates[agent].operationId);
          const rootId = scenario.initialStates[secondary ? 0 : agent].operationId;
          scenario.histories.push({ invocation, agent, ...validateHistory(data, kinds, prefix, rootId, scenario.initialStates[agent].spaceId) });
        });
      }
      scenario.finalPayloads = [];
      for (let agent = 0; agent < agents; agent++) scenario.finalPayloads.push(await verifyPayload(
        scenario.roots[agent], shape, count, `agent-${agent}-round-${options.repeats}`));
      const storeAgents = options.mode === "shared-worktrees" ? [scenario.roots.map((_, agent) => agent)]
        : scenario.roots.map((_, agent) => [agent]);
      scenario.stores = [];
      for (const agentIndices of storeAgents) {
        const checkout = scenario.roots[agentIndices[0]], storeRoot = join(checkout, ".layrs");
        const expected = auditExpectation(scenario, agentIndices, storeRoot);
        const result = await command(scenario, checkout, ["--store", storeRoot,
          ...expected.checkoutRoots.flatMap(root => ["--checkout", root])], { phase: "audit-validation",
          agents: agentIndices, expected, comparisonExclusion: report.comparisonPolicy.integrity }, true);
        try {
          requireThat(result.record.success, `Complete audit failed (command ${result.record.id})`);
          validateAudit(result.data, expected);
          result.record.contractValidated = true;
          const physical = result.data.physicalStore;
          scenario.stores.push({ path: storeRoot, ...physical.total, indexedCas: physical.objects,
            databaseAndMetadata: physical.metadata, repairBackups: physical.repairBackups,
            other: physical.other, staging: physical.staging,
            physicalEntryCount: physical.visitedEntries });
        } catch (error) {
          result.record.success = false; result.record.validationError = error.message;
          throw error;
        } finally { await checkpoint(); }
      }
      scenario.auditValidation.success = true;
      scenario.storeBytes = scenario.stores.reduce((sum, item) => sum + item.bytes, 0);
      scenario.storeByteBreakdown = Object.fromEntries(["indexedCas", "databaseAndMetadata", "repairBackups", "other", "staging"].map(category =>
        [category, scenario.stores.reduce((sum, item) => sum + item[category].bytes, 0)]));
      scenario.sharedLocators = [];
      if (options.mode === "shared-worktrees") for (const checkout of scenario.roots.slice(1)) {
        const path = join(checkout, ".layrs"), info = await lstat(path);
        requireThat(info.isFile() && !info.isSymbolicLink() && info.nlink === 1, "Expected a private shared-worktree locator file");
        scenario.sharedLocators.push({ path, bytes: info.size });
      }
      scenario.sharedLocatorBytes = scenario.sharedLocators.reduce((sum, item) => sum + item.bytes, 0);
      scenario.storeAndLocatorBytes = scenario.storeBytes + scenario.sharedLocatorBytes;
      scenario.privateCheckoutBytes = scenario.finalPayloads.reduce((sum, item) => sum + item.bytes, 0);
      checkBudget();
      scenario.success = true; scenario.status = "passed";
    } catch (error) {
      scenario.status = "failed"; scenario.error = error.message;
      throw error;
    } finally { scenario.elapsedWallMs = elapsed(caseStart); await checkpoint(); }
  }

  try {
    requireThat(values.binary && values.profile?.trim() && values.mode && values["audit-binary"] && values["audit-profile"]?.trim() && values["audit-sha256"],
      "--binary, --profile, --mode, --audit-binary, --audit-profile and --audit-sha256 are required (see --help)");
    requireThat(/^[0-9a-f]{64}$/i.test(values["audit-sha256"]), "--audit-sha256 must be exactly 64 hex digits");
    requireThat(["independent-copies", "shared-worktrees"].includes(values.mode), "Unsupported mode; native VFS is not qualified");
    options = {
      mode: values.mode,
      counts: list(values.counts || "1000,10000,99000", "counts", value => /^\d+$/.test(value)).map(value => integer(value, "counts", 1, 99000)),
      agents: list(values.agents || "1,2", "agents", value => ["1", "2"].includes(value)).map(Number),
      payloads: list(values.payloads || "code,assets,mixed", "payloads", value => ["code", "assets", "mixed"].includes(value)),
      repeats: integer(values.repeats || "1", "repeats", 1, 5),
      casRetries: integer(values["cas-retries"] ?? "2", "cas-retries", 0, 5),
      timeoutMs: integer(values["timeout-ms"] || "900000", "timeout-ms", 1000, 900000),
      runTimeoutMs: integer(values["run-timeout-ms"] || "14400000", "run-timeout-ms", 1000, 43200000),
    };
    requireThat(options.counts.length <= 8 && new Set(options.counts).size === options.counts.length, "At most eight distinct counts per run");
    requireThat(!options.payloads.includes("mixed") || options.counts.every(count => count >= 2), "Mixed payload needs at least two files");
    deadline = started + options.runTimeoutMs;
    report.options = options;
    report.integrityPreflight = { auditor: "explicit-current-read-only", rowLimit: 1000000, historyRowsPerTable: 256,
      scope: "Each retained Store must pass actual bounded row/history/byte preflight and a clean, complete native audit. No matrix clamping." };
    for (const binary of [report.binary, report.auditBinary]) {
      binary.path = await realpath(binary.path);
      const info = await lstat(binary.path);
      requireThat(info.isFile(), "Binary must be a regular file");
      binary.bytes = info.size; binary.modifiedAt = info.mtime.toISOString();
      binary.sha256 = await hashFile(binary.path);
    }
    requireThat(report.auditBinary.sha256 === report.auditBinary.expectedSha256,
      "Auditor SHA-256 differs from the pinned cohort hash");
    report.runnerSha256 = await hashFile(new URL(import.meta.url));
    if (process.platform === "win32") {
      report.resourceMetrics.helperPath = WINDOWS_HELPER;
      report.resourceMetrics.helperSha256 = await hashFile(WINDOWS_HELPER);
      report.resourceMetrics.launcherPath = await realpath(join(process.env.SystemRoot || "C:\\Windows", "System32", "WindowsPowerShell", "v1.0", "powershell.exe"));
    }
    await mkdir(join(base, "logs"));
    await checkpoint();
    for (const shape of options.payloads) for (const count of options.counts) for (const agents of options.agents) {
      checkBudget();
      await runCase(shape, count, agents);
    }
    checkBudget();
    report.success = true; report.status = "passed";
  } catch (error) {
    report.status = "failed"; report.success = false; report.error = error.message;
    process.exitCode = 1;
  } finally {
    for (const binary of [report.binary, report.auditBinary]) if (binary.sha256) {
      try {
        binary.sha256After = await hashFile(binary.path);
        requireThat(binary.sha256After === binary.sha256, "Binary changed during measurement; results invalid");
      } catch (error) {
        report.success = false; report.status = "failed"; binary.validationError = error.message; process.exitCode = 1;
      }
    }
    if (stopped) {
      report.success = false; report.status = "failed"; report.error = stopped; process.exitCode = 1;
    }
    report.completedAt = now(); report.elapsedWallMs = elapsed(started);
    await checkpoint();
    process.removeListener("SIGINT", onInt); process.removeListener("SIGTERM", onTerm);
  }
  console.log(`${report.status}: ${reportPath}; fixtures retained: ${base}`);
  if (report.error) console.error(report.error);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(`Benchmark failed: ${error.message}`); process.exitCode = 1; });
}
