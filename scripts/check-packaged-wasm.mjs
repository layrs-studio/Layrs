// Read-only guest gate: never rebuild or rewrite the embedded module/pins.
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
const args = process.argv.slice(2);
if (args.some(arg => arg !== "--offline")) throw new Error("Usage: node scripts/check-packaged-wasm.mjs [--offline]");
const result = spawnSync("cargo", [
  "test", "--locked", ...args, "-p", "layrs-weave", "--lib",
  "wasm_host::tests::real_compiled_guest_has_no_imports_and_executes_deterministically",
  "--", "--exact",
], { cwd: fileURLToPath(new URL("../", import.meta.url)), encoding: "utf8", maxBuffer: 8 * 1024 * 1024, windowsHide: true });
if (result.stdout) process.stdout.write(result.stdout);
if (result.stderr) process.stderr.write(result.stderr);
if (result.error || result.status !== 0 || !/test result: ok\. 1 passed; 0 failed; 0 ignored;/.test(result.stdout ?? "")) {
  throw new Error("Embedded WASM gate failed or did not execute exactly one test; rebuild/audit the guest explicitly before packaging");
}
