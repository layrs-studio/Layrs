// Exercise only packaged binaries, with private disposable configuration.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const directory = resolve(process.argv[2] ?? ".");
const verify = spawnSync(process.execPath, [fileURLToPath(new URL("./verify-native-package.mjs", import.meta.url)), directory], { encoding: "utf8" });
assert.equal(verify.status, 0, verify.stderr);
const manifest = JSON.parse(readFileSync(join(directory, "manifest.json"), "utf8"));
const fixture = mkdtempSync(join(tmpdir(), "layrs-packaged-smoke-"));
const work = join(fixture, "workspace");
const privateConfig = join(fixture, "config");
mkdirSync(work); mkdirSync(privateConfig);
const env = { ...process.env, HOME: privateConfig, USERPROFILE: privateConfig, APPDATA: privateConfig, LOCALAPPDATA: privateConfig, XDG_CONFIG_HOME: privateConfig };
delete env.LAYRS_LENS_WORKER; delete env.LAYRS_LENS_WORKER_SHA256;
if (["win32","linux"].includes(process.platform)) {
  const worker = manifest.files.find((file) => file.path === (process.platform === "win32" ? "layrs-lens-worker.exe" : "layrs-lens-worker"));
  assert.ok(worker,"The qualified package must include its supervised Lens worker");
  env.LAYRS_LENS_WORKER = join(directory, worker.path);
  env.LAYRS_LENS_WORKER_SHA256 = `sha256:${worker.sha256}`;
}
const binary = join(directory, process.platform === "win32" ? "layrs.exe" : "layrs");
function cli(...args) {
  const result = spawnSync(binary, ["--json", ...args], { cwd: work, env, encoding: "utf8", timeout: 30_000 });
  assert.equal(result.status, 0, `${args.join(" ")}: ${result.stderr}\n${result.stdout}`);
  const body = JSON.parse(result.stdout);
  assert.equal(body.ok, true);
  return body.data;
}
try {
  cli("init", "Packaged smoke");
  const initial = Buffer.from([0, 1, 255, 13, 10]);
  const changed = Buffer.from([255, 0, 64, 65]);
  writeFileSync(join(work, "binary"), initial);
  const first = cli("step").stepId;
  assert.ok(first);
  writeFileSync(join(work, "binary"), changed);
  cli("step"); cli("restore", first);
  assert.deepEqual(readFileSync(join(work, "binary")), initial);
  cli("undo");
  assert.deepEqual(readFileSync(join(work, "binary")), changed);
  writeFileSync(join(work, "story.txt"), "alpha\nmiddle\nomega\n");
  cli("step");
  cli("layer", "create", "topic");
  cli("layer", "use", "topic");
  writeFileSync(join(work, "story.txt"), "changed alpha\nmiddle\nomega\n");
  cli("step");
  cli("layer", "use", "main");
  writeFileSync(join(work, "story.txt"), "alpha\nmiddle\nchanged omega\n");
  cli("step");
  const preview=cli("weave", "topic", "--target", "main", "--preview");
  assert.deepEqual(preview.conflicts,[]);
  cli("weave", "topic", "--target", "main");
  assert.equal(readFileSync(join(work,"story.txt"),"utf8"),"changed alpha\nmiddle\nchanged omega\n");
  assert.equal(cli("status").dirty, false);
  console.log(JSON.stringify({ target: manifest.target, profile: manifest.profile, packageManifestSha256: createHash("sha256").update(readFileSync(join(directory, "manifest.json"))).digest("hex"), passed: ["verify", "init", "capture-binary", "restore", "undo", "weave-preview", "supervised-weave", "clean-status"] }));
} finally {
  const target=resolve(fixture);
  assert.ok(dirname(target)===resolve(tmpdir()) && basename(target).startsWith("layrs-packaged-smoke-"),"Refuse cleanup outside the generated fixture");
  rmSync(target, { recursive: true, force: true });
}
