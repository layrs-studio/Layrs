// Offline compatibility/recovery recipe. Only archives explicitly checksum-pinned
// by the caller execute, and only in a newly generated disposable workspace.
import assert from "node:assert/strict";
import { cpSync, lstatSync, mkdirSync, readFileSync, readdirSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { extractDistribution, readDistribution, sha256, validateUpgradePair } from "./native-upgrade-input.mjs";

const args = process.argv.slice(2), options = {};
for (let i = 0; i < args.length; i++) {
  assert.ok(["--old", "--new", "--old-sha256", "--new-sha256", "--output"].includes(args[i]) && args[i + 1], "Usage: --old ARCHIVE --old-sha256 HASH --new ARCHIVE --new-sha256 HASH --output NEW_DIRECTORY");
  assert.ok(!options[args[i]], "duplicate option"); options[args[i]] = args[++i];
}
assert.equal(Object.keys(options).length, 5); assert.equal(process.arch, "x64");
const oldPackage = readDistribution(resolve(options["--old"]), options["--old-sha256"]);
const newPackage = readDistribution(resolve(options["--new"]), options["--new-sha256"]);
validateUpgradePair(oldPackage, newPackage);
const fixture = resolve(options["--output"]);
mkdirSync(fixture); // Parent must exist; an existing output is refused.
const oldDir = join(fixture, "old-package"), newDir = join(fixture, "new-package");
extractDistribution(oldPackage, oldDir); extractDistribution(newPackage, newDir);
const work = join(fixture, "project"), backup = join(fixture, "backup-before-upgrade"), failed = join(fixture, "incident-state-preserved");
mkdirSync(work); const config = join(fixture, "private-config"); mkdirSync(config);
const env = { ...process.env, HOME: config, USERPROFILE: config, APPDATA: config, LOCALAPPDATA: config, XDG_CONFIG_HOME: config, XDG_CACHE_HOME: config };
for (const key of Object.keys(env)) if (key.startsWith("LAYRS_")) delete env[key];
const cliName = process.platform === "win32" ? "layrs.exe" : "layrs";
const verifier = fileURLToPath(new URL("./verify-native-package.mjs", import.meta.url));
function verify(directory, expected = 0) {
  const result = spawnSync(process.execPath, [verifier, directory], { encoding: "utf8", timeout: 30_000 });
  if (expected === 0) assert.equal(result.status, 0, result.stderr);
  else assert.notEqual(result.status, 0, "incident must be detected before execution");
}
function cli(directory, ...command) {
  const result = spawnSync(join(directory, cliName), ["--json", ...command], { cwd: work, env, encoding: "utf8", timeout: 60_000, maxBuffer: 8 * 1024 * 1024, windowsHide: true });
  assert.equal(result.status, 0, `${command.join(" ")}: ${result.stderr}\n${result.stdout}`);
  const response = JSON.parse(result.stdout); assert.equal(response.ok, true); return response.data;
}
function inventory(directory) {
  const rows = []; let total = 0;
  const visit = (path, prefix, depth) => {
    assert.ok(depth <= 16);
    for (const name of readdirSync(path).sort()) {
      const current = join(path, name), stat = lstatSync(current), key = prefix + name;
      assert.ok(!stat.isSymbolicLink(), "links/reparse paths not accepted in upgrade fixture");
      if (stat.isDirectory()) visit(current, `${key}/`, depth + 1);
      else { assert.ok(stat.isFile()); total += stat.size; assert.ok(rows.length < 4096 && total <= 256 * 1024 * 1024); rows.push([key, stat.size, sha256(readFileSync(current))]); }
    }
  }; visit(directory, "", 0); return rows;
}
function snapshot(directory) {
  return { timeline: cli(directory, "timeline"), markers: cli(directory, "markers"), releases: cli(directory, "releases"), layers: cli(directory, "layers"), status: cli(directory, "status") };
}
verify(oldDir); verify(newDir);
const initial = Buffer.from([0, 255, 13, 10, 42]), changed = Buffer.from([255, 0, 44, 65]);
writeFileSync(join(work, "binary"), initial); cli(oldDir, "init", "Archive upgrade fixture");
const marker = cli(oldDir, "marker", "create", "before-upgrade").marker;
writeFileSync(join(work, "binary"), changed); const priorStep = cli(oldDir, "step").stepId;
cli(oldDir, "release", "create", "0.1.0-upgrade"); cli(oldDir, "layer", "create", "retained-layer");
const before = snapshot(oldDir); assert.equal(before.status.dirty, false);
const beforeFiles = inventory(work);
cpSync(work, backup, { recursive: true, errorOnExist: true, force: false });
assert.deepEqual(inventory(backup), beforeFiles, "closed local backup must be byte-identical");
assert.deepEqual(snapshot(newDir), before, "new CLI must preserve all exposed identities and records");
assert.deepEqual(readFileSync(join(work, "binary")), changed);
const newDoctor = cli(newDir, "doctor"); assert.equal(newDoctor.clean, true); assert.equal(newDoctor.complete, true);
cli(newDir, "restore", marker.targetStepId); assert.deepEqual(readFileSync(join(work, "binary")), initial);
cli(newDir, "undo"); assert.deepEqual(readFileSync(join(work, "binary")), changed);
writeFileSync(join(work, "binary"), Buffer.from([99, 0, 254]));
const newStep = cli(newDir, "step").stepId; assert.ok(newStep && newStep !== priorStep);
assert.deepEqual(cli(newDir, "markers"), before.markers); assert.deepEqual(cli(newDir, "releases"), before.releases);
const after = snapshot(newDir);
for (const operation of before.timeline.operations) assert.ok(after.timeline.operations.some((row) => row.operationId === operation.operationId), "upgrade lost historical Operation");
// An installation artifact becomes corrupt after writes. Do not run it again.
const binaryPath = join(newDir, cliName), damaged = readFileSync(binaryPath); damaged[damaged.length - 1] ^= 1; writeFileSync(binaryPath, damaged);
verify(newDir, 1);
const failedInventory = inventory(work);
assert.equal(dirname(work), fixture); assert.equal(dirname(failed), fixture);
// Keep the checkout root inode/identity: recreating the directory would require
// an explicit native Worktree relocation, which this recipe does not invent.
mkdirSync(failed);
const rootIdentity = lstatSync(work, { bigint: true });
for (const name of readdirSync(work)) {
  const from = join(work, name), to = join(failed, name);
  assert.equal(dirname(from), work); assert.equal(dirname(to), failed);
  renameSync(from, to); // Only generated fixture children, already link-checked.
}
assert.deepEqual(readdirSync(work), []);
for (const name of readdirSync(backup)) cpSync(join(backup, name), join(work, name), { recursive: true, errorOnExist: true, force: false });
const restoredIdentity = lstatSync(work, { bigint: true });
assert.equal(restoredIdentity.dev, rootIdentity.dev); assert.equal(restoredIdentity.ino, rootIdentity.ino);
assert.deepEqual(inventory(work), beforeFiles, "restore must target a new empty location");
assert.deepEqual(snapshot(oldDir), before, "old CLI runs only the restored pre-upgrade state");
assert.deepEqual(readFileSync(join(work, "binary")), changed);
const oldDoctor = cli(oldDir, "doctor"); assert.equal(oldDoctor.clean, true); assert.equal(oldDoctor.complete, true);
assert.deepEqual(inventory(backup), beforeFiles); assert.deepEqual(inventory(failed), failedInventory);
const report = { schemaVersion: "layrs.validation.archive-upgrade.v1", passed: true, target: newPackage.manifest.target, oldArchiveSha256: oldPackage.archiveSha256, newArchiveSha256: newPackage.archiveSha256, oldManifestSha256: oldPackage.manifestSha256, newManifestSha256: newPackage.manifestSha256, localStore: "native-format-2", checkoutRootIdentityPreserved: true, historicalOperationsRetained: before.timeline.operations.length, markerId: marker.markerId, priorStep, newStepPreservedInIncidentState: newStep, rollbackDropsPostBackupWritesFromActiveProject: true, incident: "extracted CLI checksum corruption detected before execution", fixtureRetained: fixture, scope: "local CLI same-format compatibility; no PostgreSQL migration, MSI installation or cross-format support qualified" };
writeFileSync(join(fixture, "report.json"), JSON.stringify(report, null, 2) + "\n", { flag: "wx" });
console.log(JSON.stringify(report));
