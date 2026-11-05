// Synthetic local measurements. Fresh retained fixtures; never modifies an existing checkout.
import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { mkdir, open, readFile, readdir, stat, writeFile } from "node:fs/promises";
import { resolve, join } from "node:path";
import { cpus, platform, release, totalmem } from "node:os";
import { performance } from "node:perf_hooks";

const binary = resolve(process.argv[2] || `target/release/layrs${process.platform === "win32" ? ".exe" : ""}`);
const reportPath = resolve(process.argv[3] || "docs/plans/pme/measurements/T8-volumes.json");
const base = resolve(process.argv[4] || `../tmp/layrs-volumes-${randomUUID()}`);
const counts = (process.argv[5] || "10000,100000").split(",").map(Number);
if (counts.some(count => !Number.isSafeInteger(count) || count < 1000 || count > 100000 || count % 1000)) throw new Error("Counts must be multiples of 1000, between 1000 and 100000");
await mkdir(base); // An existing path is deliberately rejected.
const report = {
  schemaVersion: "layrs.local-volume-measurement.v1", startedAt: new Date().toISOString(),
  platform: platform(), os: release(), cpu: cpus()[0]?.model, totalMemoryBytes: totalmem(),
  binarySha256: createHash("sha256").update(await readFile(binary)).digest("hex"), fixtureRoot: base,
  limitations: ["Synthetic repeated content: 200 distinct payloads, 256 bytes each.", "Concurrent compilation may affect timings; one run, warm OS caches.", "Durations include CLI startup and full command; peak process memory and remote sync not measured.", "Fixtures retained for inspection; all current Operations retained, no pruning."],
  cases: [],
};
async function checkpoint() { await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`); }
await checkpoint();

async function command(root, args, expectedSuccess = true) {
  const start = performance.now();
  const result = await new Promise((resolveRun, reject) => {
    const child = spawn(binary, ["--json", ...args], { cwd: root, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "", stderr = "", truncated = false;
    const timer = setTimeout(() => { truncated = true; child.kill(); }, 15 * 60 * 1000);
    child.stdout.on("data", bytes => { if (stdout.length < 32 * 1024 * 1024) stdout += bytes; else { truncated = true; child.kill(); } });
    child.stderr.on("data", bytes => { if (stderr.length < 4096) stderr += bytes; });
    child.on("error", error => { clearTimeout(timer); reject(error); });
    child.on("exit", code => { clearTimeout(timer); resolveRun({ code, stdout, stderr, truncated }); });
  });
  let envelope;
  try { envelope = JSON.parse(result.stdout); } catch { envelope = null; }
  const measured = { command: args.join(" "), elapsedMs: Math.round(performance.now() - start), exitCode: result.code,
    expectedSuccess, matchedExpectation: expectedSuccess ? result.code === 0 && envelope?.ok === true : result.code !== 0 && envelope?.ok === false,
    truncated: result.truncated, error: envelope?.error ?? (result.code !== 0 ? result.stderr : undefined) };
  return { measured, data: envelope?.data };
}
async function bytesIn(root) {
  let bytes = 0, files = 0;
  for (const entry of await readdir(root, { withFileTypes: true })) {
    if (entry.isSymbolicLink()) throw new Error("Unexpected symlink in synthetic fixture");
    if (entry.isDirectory()) { const child = await bytesIn(join(root, entry.name)); bytes += child.bytes; files += child.files; }
    else { bytes += (await stat(join(root, entry.name))).size; files++; }
  }
  return { bytes, files };
}
for (const count of counts) {
  const root = join(base, `files-${count}`); await mkdir(root);
  const scenario = { files: count, root, measurements: [] }; report.cases.push(scenario); await checkpoint();
  for (let dir = 0; dir < count / 1000; dir++) {
    const directory = join(root, `d${dir}`); await mkdir(directory);
    await Promise.all(Array.from({ length: 1000 }, (_, index) => writeFile(join(directory, `f${index}.txt`), `${index % 200}`.padEnd(255, "x") + "\n")));
  }
  for (const args of [["init", `Volume ${count}`], ["status"], ["step"], ["layer", "create", "baseline"]]) {
    const { measured } = await command(root, args); scenario.measurements.push(measured); await checkpoint();
    if (!measured.matchedExpectation) break;
  }
  if (!scenario.measurements.every(m => m.matchedExpectation)) continue;
  await writeFile(join(root, "d0/f0.txt"), "single incremental change\n");
  for (const args of [["diff", "--stat"], ["step"], ["layer", "use", "baseline"], ["layer", "use", "main"]]) {
    const { measured } = await command(root, args); scenario.measurements.push(measured); await checkpoint();
    if (!measured.matchedExpectation) break;
  }
  scenario.storage = await bytesIn(join(root, ".layrs")); await checkpoint();
}
for (const size of process.argv.includes("--skip-binaries") ? [] : [100 * 1024 * 1024, 1024 * 1024 * 1024]) {
  const root = join(base, `binary-${size}`); await mkdir(root);
  const scenario = { binaryBytes: size, root, measurements: [] }; report.cases.push(scenario);
  const initialized = await command(root, ["init", "Binary limit"]);
  scenario.measurements.push(initialized.measured);
  const before = (await command(root, ["timeline", "--limit", "1"])).data;
  const file = await open(join(root, "large.bin"), "wx");
  try { await file.truncate(size); } finally { await file.close(); }
  scenario.measurements.push((await command(root, ["step"], false)).measured);
  const after = (await command(root, ["timeline", "--limit", "1"])).data;
  scenario.operationUnchanged = JSON.stringify(before) === JSON.stringify(after);
  scenario.fileBytesUnchanged = (await stat(join(root, "large.bin"))).size === size;
  await checkpoint();
}
report.completedAt = new Date().toISOString(); await checkpoint();
console.log(`Measurements saved: ${reportPath}; fixtures: ${base}`);
if (report.cases.some(c => c.measurements.some(m => !m.matchedExpectation) || c.operationUnchanged === false)) process.exitCode = 1;
