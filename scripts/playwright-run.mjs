import { spawn } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const target = process.argv[2] ?? "renderer";
const mode = process.argv[3] ?? "visual";
const extraArgs = process.argv.slice(4);

const configs = {
  native: "playwright.native.config.ts",
  renderer: "playwright.config.ts"
};

const config = configs[target];
if (!config) {
  console.error(`Unknown Playwright target: ${target}`);
  console.error(`Expected one of: ${Object.keys(configs).join(", ")}`);
  process.exit(1);
}

const isCi = mode === "ci";
const isDebug = mode === "debug";
const isVisual = !isCi;
const command = process.platform === "win32" ? "cmd.exe" : "playwright";
const playwrightArgs = ["test", "-c", config];
const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");

if (isDebug) {
  playwrightArgs.push("--debug");
} else if (isVisual) {
  playwrightArgs.push("--headed", "--workers=1");
}

playwrightArgs.push(...extraArgs);

const env = {
  ...process.env,
  LAYRS_E2E_VISUAL: isVisual ? "1" : "0",
  LAYRS_E2E_HEADLESS: isCi ? "1" : "0",
  LAYRS_E2E_SLOW_MO_MS: process.env.LAYRS_E2E_SLOW_MO_MS ?? (isVisual ? "250" : "0"),
  LAYRS_E2E_STEP_DELAY_MS: process.env.LAYRS_E2E_STEP_DELAY_MS ?? (isVisual ? "650" : "0"),
  LAYRS_E2E_HOLD_MS: process.env.LAYRS_E2E_HOLD_MS ?? (isVisual ? "2000" : "0")
};

if (isCi) {
  env.CI = "true";
} else if (!process.env.CI) {
  delete env.CI;
}

const args =
  process.platform === "win32"
    ? ["/d", "/s", "/c", "node_modules\\.bin\\playwright.CMD", ...playwrightArgs]
    : playwrightArgs;

const child = spawn(command, args, {
  cwd: rootDir,
  env,
  shell: false,
  stdio: "inherit"
});

child.on("error", (error) => {
  console.error(error.message);
  process.exit(1);
});

child.on("exit", (code, signal) => {
  if (signal) {
    console.error(`Playwright exited with signal ${signal}`);
    process.exit(1);
  }
  process.exit(code ?? 0);
});

process.on("SIGINT", () => {
  child.kill("SIGINT");
  process.exit(130);
});

process.on("SIGTERM", () => {
  child.kill("SIGTERM");
  process.exit(143);
});
