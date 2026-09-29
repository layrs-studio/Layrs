// Build a local MSI candidate. This script never installs or publishes it.
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { digestFiles, sourceTreeChanges, sourceTreeFiles } from "./package-source.mjs";

if (process.platform !== "win32" || process.arch !== "x64" || process.argv.length !== 4 || process.argv[2] !== "--output") throw new Error("Windows x64 only: node scripts/package-desktop-windows.mjs --output NEW_DIRECTORY");
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const app = join(root, "apps/studio-desktop");
const output = resolve(process.argv[3]);
if (existsSync(output)) throw new Error("Refuse to mix installers in an existing output directory");
mkdirSync(dirname(output), { recursive: true }); mkdirSync(output);
const run = (command, args, cwd = root, capture = true) => {
  const result = spawnSync(command, args, { cwd, encoding: "utf8", stdio: capture ? ["ignore", "pipe", "inherit"] : "inherit", maxBuffer: 32 * 1024 * 1024 });
  if (result.error || result.status !== 0) throw new Error(`${command} failed: ${result.error?.message ?? result.status}`);
  return result.stdout?.trim() ?? "";
};
const config = join(output, "build-config.json");
// Use the locked, installed workspace tooling; no global pnpm cache is required.
writeFileSync(config, JSON.stringify({ build: { beforeBuildCommand: "node node_modules/vite/bin/vite.js build" }, bundle: { icon: ["icons/icon.ico"] } }));
const metadata = JSON.parse(run("cargo", ["metadata", "--locked", "--offline", "--no-deps", "--format-version", "1", "--manifest-path", join(app, "src-tauri/Cargo.toml")]));
const sourceFiles = () => sourceTreeFiles(root, ["crates", "packages", "apps/studio-desktop", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "package.json", "pnpm-lock.yaml"]);
const beforeFiles = sourceFiles();
const beforeBuild = digestFiles(beforeFiles);
run(process.execPath, ["scripts/check-packaged-wasm.mjs", "--offline"], root, false);
const started = Date.now();
run(process.execPath, [join(app, "node_modules/@tauri-apps/cli/tauri.js"), "build", "--bundles", "msi", "--config", config, "--ci", "--no-sign", "--", "--locked"], app, false);
const changes = sourceTreeChanges(beforeFiles, sourceFiles());
if (changes.length > 0) throw new Error(`Sources changed during compilation; retry from a stable checkout in a new output directory: ${changes.join(", ")}`);
const bundle = join(metadata.target_directory, "release/bundle/msi");
const paths = readdirSync(bundle).filter((name) => name.endsWith(".msi")).map((name) => join(bundle, name)).filter((path) => statSync(path).mtimeMs >= started - 2000);
if (paths.length === 0) throw new Error("Tauri produced no fresh MSI; incomplete output must not be distributed");
const artifacts = paths.map((path) => {
  const bytes = readFileSync(path);
  const name = basename(path);
  copyFileSync(path, join(output, name));
  return { path: name, bytes: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") };
});
writeFileSync(join(output, "SHA256SUMS"), artifacts.map((artifact) => `${artifact.sha256}  ${artifact.path}\n`).join(""));
writeFileSync(join(output, "manifest.json"), JSON.stringify({ schema: "layrs.desktop-distribution.v1", createdAt: new Date().toISOString(), commit: run("git", ["rev-parse", "HEAD"]), dirty: Boolean(run("git", ["status", "--porcelain", "--untracked-files=all"])), treeSha256: beforeBuild, signature: "unsigned", installedByRecipe: false, artifacts }, null, 2) + "\n");
console.log(`Built ${artifacts.length} MSI candidate(s) in ${output}; installation and publisher signing remain separate validation steps`);
