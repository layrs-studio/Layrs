import { spawn } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const target = process.argv[2] ?? "real";
const mode = process.argv[3] ?? "visual";
const extraArgs = process.argv.slice(4);

const configs = {
  native: "playwright.native.config.ts",
  real: "playwright.real.config.ts",
  web: "playwright.web.config.ts",
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

if (target === "renderer") {
  playwrightArgs.push("--project", "Studio Web mock - Chromium");
}

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

if (target === "real") {
  env.LAYRS_E2E_SERVER_URL = process.env.LAYRS_E2E_SERVER_URL ?? "http://127.0.0.1:18787";
  env.LAYRS_E2E_STUDIO_WEB_PORT = process.env.LAYRS_E2E_STUDIO_WEB_PORT ?? "15173";
}

if (target === "web") {
  env.LAYRS_E2E_SERVER_URL = process.env.LAYRS_E2E_SERVER_URL ?? "http://127.0.0.1:18887";
  env.LAYRS_E2E_STUDIO_WEB_PORT = process.env.LAYRS_E2E_STUDIO_WEB_PORT ?? "15174";
}

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");

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
