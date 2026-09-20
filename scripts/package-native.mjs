// Build native artifacts locally. No signing, uploading or account credentials.
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { sourceTreeDigest } from "./package-source.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const options = { profile: "release", offline: false };
for (let i = 0; i < args.length; i++) {
  if (args[i] === "--offline") options.offline = true;
  else if (["--output", "--profile"].includes(args[i]) && args[i + 1]) options[args[i].slice(2)] = args[++i];
  else throw new Error("Usage: node scripts/package-native.mjs --output NEW_DIRECTORY [--offline] [--profile release|dev]");
}
if (!options.output || !["release", "dev"].includes(options.profile)) throw new Error("A new output directory and release/dev profile are required");
if (!["win32", "linux"].includes(process.platform)) throw new Error("Only Windows and Linux are qualified by this packaging recipe");
if (process.arch !== "x64") throw new Error("Only x64 artifacts are qualified by this packaging recipe");

function run(command, argv, capture = true) {
  const result = spawnSync(command, argv, { cwd: root, encoding: "utf8", stdio: capture ? ["ignore", "pipe", "inherit"] : "inherit", maxBuffer: 64 * 1024 * 1024 });
  if (result.error || result.status !== 0) throw new Error(`${command} failed: ${result.error?.message ?? result.status}`);
  return result.stdout?.trim() ?? "";
}
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const sourceDigest = () => sourceTreeDigest(root, ["crates", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml"]);
const sourcesBeforeBuild = sourceDigest();
run(process.execPath, ["scripts/check-packaged-wasm.mjs", ...(options.offline ? ["--offline"] : [])], false);
const rust = run("rustc", ["-vV"]);
const target = /^host: (.+)$/m.exec(rust)?.[1];
if (!target || !/^[a-z0-9_-]+$/.test(target)) throw new Error("Cannot identify native Rust host target");
const commit = run("git", ["rev-parse", "HEAD"]);
const dirty = Boolean(run("git", ["status", "--porcelain", "--untracked-files=all"]));
const output = resolve(options.output);
if (existsSync(output)) throw new Error("Output already exists; refuse to mix releases");
mkdirSync(dirname(output), { recursive: true });
mkdirSync(output);

const binaries = ["layrs", "layrs-server", "layrs-runner", "layrs-lens-worker"];
const cargo = ["build", "--locked", "--profile", options.profile, "--message-format=json-render-diagnostics", "-p", "layrs-cli", "-p", "layrs-server", "-p", "layrs-runner", "--bins"];
cargo.push("-p", "layrs-weave");
if (options.offline) cargo.push("--offline");
const built = new Map();
for (const line of run("cargo", cargo).split("\n")) {
  if (!line.trim()) continue;
  const event = JSON.parse(line);
  if (event.reason === "compiler-artifact" && event.executable && binaries.includes(event.target.name)) built.set(event.target.name, event.executable);
}
if (sourceDigest() !== sourcesBeforeBuild) throw new Error("Sources changed during compilation; retry from a stable checkout in a new output directory");
const name = `layrs-${commit.slice(0, 12)}-${target}-${options.profile}`;
const directory = join(output, name);
mkdirSync(directory);
const files = [];
function add(name, bytes, executable = false) {
  writeFileSync(join(directory, name), bytes, { flag: "wx", mode: executable ? 0o755 : 0o644 });
  if (executable) chmodSync(join(directory, name), 0o755);
  files.push({ path: name, bytes: Buffer.byteLength(bytes), sha256: sha256(bytes) });
}
for (const binary of binaries) {
  const source = built.get(binary);
  if (!source) throw new Error(`Cargo did not produce ${binary}; incomplete output must not be distributed`);
  add(basename(source), readFileSync(source), true);
}
add("LICENSE", readFileSync(join(root, "LICENSE")));
for (const document of ["README.md", "UPGRADING.md", "DISTRIBUTION.md", "BACKUPS.md", "RETENTION.md", "GC.md", "LOCAL-COMPACTION.md", "POLICIES.md", "OBSERVATIONS.md", "STORAGE-BUDGET.md", "OBSERVABILITY.md", "INVITATIONS.md", "NATIVE-CI.md", "NATIVE-CONFLICTS.md", "LOCAL-PILOT.md"]) {
  add(document, readFileSync(join(root, "deploy/selfhost", document)));
}
add("migrations.lock.json", readFileSync(join(root, "scripts/migrations.lock.json")));
add("native-lenses-v1.wasm", readFileSync(join(root, "crates/layrs-weave/wasm/native-lenses-v1.wasm")));
if (process.platform === "win32") {
  const worker = files.find((file) => file.path === "layrs-lens-worker.exe");
  add("layrs-with-lenses.ps1", `$ErrorActionPreference = 'Stop'\n$env:LAYRS_LENS_WORKER = Join-Path $PSScriptRoot 'layrs-lens-worker.exe'\n$env:LAYRS_LENS_WORKER_SHA256 = 'sha256:${worker.sha256}'\n& (Join-Path $PSScriptRoot 'layrs.exe') @args\nexit $LASTEXITCODE\n`);
} else {
  const worker = files.find((file) => file.path === "layrs-lens-worker");
  add("layrs-with-lenses", `#!/bin/sh\nset -eu\nscript_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)\nexport LAYRS_LENS_WORKER="$script_dir/layrs-lens-worker"\nexport LAYRS_LENS_WORKER_SHA256='sha256:${worker.sha256}'\nexec "$script_dir/layrs" "$@"\n`, true);
}
add("verify-native-package.mjs", readFileSync(join(root, "scripts/verify-native-package.mjs")));
const manifest = {
  schema: "layrs.native-distribution.v1", createdAt: new Date().toISOString(),
  source: { commit, dirty, treeSha256: sourcesBeforeBuild, cargoLockSha256: sha256(readFileSync(join(root, "Cargo.lock"))) },
  target, profile: options.profile, rust, signature: "unsigned", protocol: "V3", localStore: "native-format-2",
  semanticLensHost: process.platform === "win32" ? "windows-job-object-wasmi" : "linux-seccomp-rlimit-wasmi",
  files,
};
writeFileSync(join(directory, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n", { flag: "wx" });
// Verification after extraction needs no repository checkout.
run("node", [join(directory, "verify-native-package.mjs"), directory], false);
const archive = join(output, `${name}.tar.gz`);
run("tar", ["-czf", archive, "-C", output, name], false);
writeFileSync(join(output, "SHA256SUMS"), `${sha256(readFileSync(archive))}  ${basename(archive)}\n`, { flag: "wx" });
console.log(JSON.stringify({ archive, target, profile: options.profile, dirty, signature: "unsigned" }));
