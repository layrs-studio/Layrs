// Fresh, networkless container running only verified release archives. Never
// builds an image, pulls from a registry, installs a service or mounts user data.
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { readDistribution, validateUpgradePair } from "./native-upgrade-input.mjs";
import { ensureUpgradeContainerStopped } from "./native-upgrade-lifecycle.mjs";

const args = process.argv.slice(2), options = {};
for (let i = 0; i < args.length; i++) {
  assert.ok(["--old", "--new", "--old-sha256", "--new-sha256", "--output"].includes(args[i]) && args[i + 1]);
  assert.ok(!options[args[i]], "duplicate argument"); options[args[i]] = args[++i];
}
assert.equal(Object.keys(options).length, 5, "explicit archives, hashes and new output required");
const oldPath = resolve(options["--old"]), newPath = resolve(options["--new"]);
validateUpgradePair(readDistribution(oldPath, options["--old-sha256"]), readDistribution(newPath, options["--new-sha256"]), "linux");
const output = resolve(options["--output"]); assert.ok(!existsSync(output), "output must be new");
const docker = [];
if (process.env.LAYRS_TEST_DOCKER_CONFIG) docker.push("--config", process.env.LAYRS_TEST_DOCKER_CONFIG);
if (process.env.LAYRS_TEST_DOCKER_HOST) docker.push("-H", process.env.LAYRS_TEST_DOCKER_HOST);
const image = process.env.LAYRS_UPGRADE_LINUX_IMAGE ?? "layrs-native-packaging:validation";
const inspect = spawnSync("docker", [...docker, "image", "inspect", "--format", "{{.Id}}", image], { encoding: "utf8", windowsHide: true });
assert.equal(inspect.status, 0, "validation image must already exist locally");
const imageId = inspect.stdout.trim(); assert.match(imageId, /^sha256:[a-f0-9]{64}$/);
mkdirSync(output);
const scripts = dirname(fileURLToPath(import.meta.url));
for (const path of [scripts, oldPath, newPath, output]) assert.ok(!path.includes(","), "Docker bind path cannot contain comma");
// Native Unix filesystem semantics are required by the checkout. A Windows
// bind mount can reject exclusive rename/durability operations with EINVAL.
// Keep both the stopped container and its dedicated volume for inspection;
// exporting the fixture does not preserve its original filesystem identity.
const container = `layrs-upgrade-${randomUUID()}`, volume = `${container}-data`;
const provenance = { schemaVersion: "layrs.validation.archive-upgrade-container.v1", image, imageId, network: "none", freshContainer: true, readonlySourcesAndArchives: true, filesystem: "dedicated-native-linux-volume", retainedContainer: container, retainedVolume: volume, fixture: "recipe/report.json", fixtureExported: false, recipeExitCode: null, phase: "prepared", exportedCopyIsNotARelocatedWorktree: true };
const provenancePath = join(output, "container.json");
writeFileSync(provenancePath, JSON.stringify(provenance, null, 2) + "\n", { flag: "wx" });
const invocation = [...docker, "run", "--name", container, "--pull", "never", "--network", "none", "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--pids-limit", "64", "--memory", "1g", "--tmpfs", "/tmp:rw,nosuid,nodev,noexec,size=64m",
  "--mount", `type=bind,source=${scripts},target=/scripts,readonly`,
  "--mount", `type=bind,source=${oldPath},target=/old.tar.gz,readonly`,
  "--mount", `type=bind,source=${newPath},target=/new.tar.gz,readonly`,
  "--mount", `type=volume,source=${volume},target=/out`,
  "--entrypoint", "node", imageId, "/scripts/test-native-upgrade.mjs",
  "--old", "/old.tar.gz", "--old-sha256", options["--old-sha256"], "--new", "/new.tar.gz", "--new-sha256", options["--new-sha256"], "--output", "/out/recipe"];
const run = spawnSync("docker", invocation, { encoding: "utf8", windowsHide: true, maxBuffer: 8 * 1024 * 1024, timeout: 10 * 60_000 });
process.stdout.write(run.stdout ?? ""); process.stderr.write(run.stderr ?? "");
const termination = ensureUpgradeContainerStopped(docker, container, Boolean(run.error) || run.status === null);
Object.assign(provenance, { phase: "termination-checked", recipeExitCode: run.status, executionError: run.error?.message ?? null, termination });
writeFileSync(provenancePath, JSON.stringify(provenance, null, 2) + "\n");
assert.equal(termination.stopped, true, "Container termination unverified; fixture retained, export refused");
const exported = spawnSync("docker", [...docker, "cp", `${container}:/out/recipe`, join(output, "recipe")], { encoding: "utf8", windowsHide: true, maxBuffer: 8 * 1024 * 1024, timeout: 120_000 });
Object.assign(provenance, { phase: run.status === 0 && exported.status === 0 ? "completed" : "failed", fixtureExported: exported.status === 0 });
writeFileSync(provenancePath, JSON.stringify(provenance, null, 2) + "\n");
assert.equal(run.status, 0, run.error?.message ?? "Linux archive upgrade failed; fixture retained");
assert.equal(exported.status, 0, exported.error?.message ?? "Linux fixture export failed; native volume retained");
const report = JSON.parse(readFileSync(join(output, "recipe", "report.json"), "utf8"));
assert.equal(report.passed, true);
