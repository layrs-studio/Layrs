// Branch-switch and per-agent worktree measurements (L0). Fresh retained
// fixtures only; never touches an existing checkout. One run per case.
//
// usage: node scripts/measure-switch.mjs [BINARY] [REPORT.json] [FIXTURE_BASE] [COUNTS]
//   COUNTS: comma list, multiples of 1000 in 1000..100000 (default 1000,10000)
import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { cpus, platform, release, totalmem } from "node:os";
import { performance } from "node:perf_hooks";

const binary = resolve(process.argv[2] || `target/release/layrs${process.platform === "win32" ? ".exe" : ""}`);
const reportPath = resolve(process.argv[3] || "docs/plans/pme/measurements/switch.json");
const base = resolve(process.argv[4] || `../tmp/layrs-switch-${randomUUID()}`);
const counts = (process.argv[5] || "1000,10000").split(",").map(Number);
if (counts.some(n => !Number.isSafeInteger(n) || n < 1000 || n > 100000 || n % 1000)) {
  throw new Error("Counts must be multiples of 1000, between 1000 and 100000");
}
await mkdir(base); // An existing path is deliberately rejected.
const report = {
  schemaVersion: "layrs.switch-measurement.v1", startedAt: new Date().toISOString(),
  platform: platform(), os: release(), cpu: cpus()[0]?.model, totalMemoryBytes: totalmem(),
  binary, binarySha256: createHash("sha256").update(await readFile(binary)).digest("hex"), fixtureRoot: base,
  limitations: [
    "Synthetic 256-byte text files, all distinct contents, 1000 files per directory.",
    "One run per case, warm OS caches; durations include CLI startup and the full command.",
    "Concurrent cases start both commands together; they measure contention, not throughput.",
    "Fixtures retained; no pruning, no remote.",
  ],
  cases: [],
};
const checkpoint = () => writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`);
await checkpoint();

async function command(root, args, label, expectedSuccess = true) {
  const start = performance.now();
  const result = await new Promise((done, fail) => {
    const child = spawn(binary, ["--json", ...args], { cwd: root, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "", stderr = "", truncated = false;
    const timer = setTimeout(() => { truncated = true; child.kill(); }, 30 * 60 * 1000);
    child.stdout.on("data", b => { if (stdout.length < 32 * 1024 * 1024) stdout += b; else { truncated = true; child.kill(); } });
    child.stderr.on("data", b => { if (stderr.length < 4096) stderr += b; });
    child.on("error", e => { clearTimeout(timer); fail(e); });
    child.on("exit", code => { clearTimeout(timer); done({ code, stdout, stderr, truncated }); });
  });
  let envelope = null;
  try { envelope = JSON.parse(result.stdout); } catch { /* reported below */ }
  const ok = result.code === 0 && envelope?.ok === true;
  return {
    label, command: args.join(" "), cwd: root, elapsedMs: Math.round(performance.now() - start), exitCode: result.code,
    expectedSuccess, matchedExpectation: expectedSuccess ? ok : !ok, truncated: result.truncated,
    error: envelope?.error ?? (result.code !== 0 ? result.stderr.slice(0, 2000) : undefined),
  };
}

async function writeFiles(root, count, salt) {
  for (let dir = 0; dir < count / 1000; dir++) {
    const directory = join(root, `d${dir}`);
    await mkdir(directory, { recursive: true });
    await Promise.all(Array.from({ length: 1000 }, (_, i) =>
      writeFile(join(directory, `f${i}.txt`), `${salt}:${dir}:${i}:`.padEnd(255, "x") + "\n")));
  }
}
async function touch(root, k, salt) {
  for (let i = 0; i < k; i++) await writeFile(join(root, `d${Math.floor(i / 1000)}`, `f${i % 1000}.txt`), `${salt}:${i}\n`);
}

for (const count of counts) {
  const root = join(base, `files-${count}`);
  await mkdir(root);
  const scenario = { files: count, root, measurements: [] };
  report.cases.push(scenario);
  await checkpoint();
  const step = async (cwd, args, label, expected = true) => {
    const measured = await command(cwd, args, label, expected);
    scenario.measurements.push(measured);
    await checkpoint();
    if (!measured.matchedExpectation) throw new Error(`${label} failed: ${measured.error}`);
    return measured;
  };
  try {
    await writeFiles(root, count, "base");
    await step(root, ["init", `Switch ${count}`], "init");
    await step(root, ["status"], "status clean");
    await step(root, ["step"], "step initial");
    await step(root, ["layer", "create", "baseline"], "layer create");
    // K=1: the T8 scenario (comparable with T8-local-volumes 2026-09-20).
    await touch(root, 1, "k1");
    await step(root, ["diff", "--stat"], "diff K=1");
    await step(root, ["step"], "step K=1");
    await step(root, ["layer", "use", "baseline"], "switch out K=1");
    await step(root, ["layer", "use", "main"], "switch back K=1");
    await step(root, ["layer", "use", "main"], "switch no-op");
    // K=100 changed files between the two Layers.
    await touch(root, Math.min(100, count), "k100");
    await step(root, ["step"], "step K=100");
    await step(root, ["layer", "use", "baseline"], "switch out K=100");
    await step(root, ["layer", "use", "main"], "switch back K=100");
    // A second worktree for another agent, on the other branch.
    const agent = join(base, `files-${count}-agent`);
    await mkdir(agent);
    await step(root, ["worktree", "create", agent, "--layer", "baseline", "--step", "@layer:baseline"], "worktree create");
    await step(agent, ["status"], "agent status");
    // Two agents at once: concurrent status, then concurrent capture.
    const pair = async (label, a, b) => {
      const [x, y] = await Promise.all([command(root, a, `${label} primary`), command(agent, b, `${label} agent`)]);
      scenario.measurements.push(x, y);
      await checkpoint();
    };
    await pair("concurrent status", ["status"], ["status"]);
    await writeFile(join(root, "d0/f1.txt"), "primary concurrent edit\n");
    await writeFile(join(agent, "d0/f2.txt"), "agent concurrent edit\n");
    await pair("concurrent step", ["step"], ["step"]);
  } catch (error) {
    scenario.aborted = String(error);
    await checkpoint();
  }
}
report.completedAt = new Date().toISOString();
await checkpoint();
console.log(`Measurements saved: ${reportPath}; fixtures: ${base}`);
if (report.cases.some(c => c.aborted || c.measurements.some(m => !m.matchedExpectation))) process.exitCode = 1;
