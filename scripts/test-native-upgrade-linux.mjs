// Fresh, networkless container running only verified release archives. Never
// builds an image, pulls from a registry, installs a service or mounts user data.
import assert from "node:assert/strict";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { readDistribution, validateUpgradePair } from "./native-upgrade-input.mjs";

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
const invocation = [...docker, "run", "--rm", "--pull", "never", "--network", "none", "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--pids-limit", "64", "--memory", "1g", "--tmpfs", "/tmp:rw,nosuid,nodev,noexec,size=64m",
  "--mount", `type=bind,source=${scripts},target=/scripts,readonly`,
  "--mount", `type=bind,source=${oldPath},target=/old.tar.gz,readonly`,
  "--mount", `type=bind,source=${newPath},target=/new.tar.gz,readonly`,
  "--mount", `type=bind,source=${output},target=/out`,
  "--entrypoint", "node", imageId, "/scripts/test-native-upgrade.mjs",
  "--old", "/old.tar.gz", "--old-sha256", options["--old-sha256"], "--new", "/new.tar.gz", "--new-sha256", options["--new-sha256"], "--output", "/out/recipe"];
const run = spawnSync("docker", invocation, { encoding: "utf8", windowsHide: true, maxBuffer: 8 * 1024 * 1024, timeout: 10 * 60_000 });
process.stdout.write(run.stdout ?? ""); process.stderr.write(run.stderr ?? "");
assert.equal(run.status, 0, run.error?.message ?? "Linux archive upgrade failed; fixture retained");
const report = JSON.parse(readFileSync(join(output, "recipe", "report.json"), "utf8"));
assert.equal(report.passed, true);
writeFileSync(join(output, "container.json"), JSON.stringify({ schemaVersion: "layrs.validation.archive-upgrade-container.v1", image, imageId, network: "none", freshContainer: true, readonlySourcesAndArchives: true, fixture: "recipe/report.json" }, null, 2) + "\n", { flag: "wx" });
