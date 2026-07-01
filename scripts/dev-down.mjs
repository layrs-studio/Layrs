import { spawn } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const child = spawn("docker", ["compose", "down", "--remove-orphans"], {
  cwd: rootDir,
  shell: false,
  stdio: "inherit"
});

child.on("error", (error) => {
  console.error(`Layrs dev: failed to stop Docker services: ${error.message}`);
  process.exit(1);
});

child.on("exit", (code, signal) => {
  process.exitCode = code ?? (signal ? 1 : 0);
});
