import { spawn, spawnSync } from "node:child_process";
import { createServer } from "node:net";
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
const isIsolated = process.env.LAYRS_E2E_ISOLATED !== "0";
const command = process.platform === "win32" ? "cmd.exe" : "playwright";
const playwrightArgs = ["test", "-c", config];
const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
let cleanupE2eServices = () => {};

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
  LAYRS_E2E_ISOLATED: isIsolated ? "1" : "0",
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
  const serverPort = await e2ePort("LAYRS_E2E_SERVER_PORT", 18787);
  const studioWebPort = await e2ePort("LAYRS_E2E_STUDIO_WEB_PORT", 15173);
  env.LAYRS_E2E_SERVER_URL = process.env.LAYRS_E2E_SERVER_URL ?? `http://127.0.0.1:${serverPort}`;
  env.LAYRS_E2E_STUDIO_WEB_PORT = String(studioWebPort);
  cleanupE2eServices = () => cleanupComposeProject("layrs-e2e", Number.parseInt(new URL(env.LAYRS_E2E_SERVER_URL).port, 10));
}

if (target === "web") {
  const serverPort = await e2ePort("LAYRS_E2E_SERVER_PORT", 18887);
  const studioWebPort = await e2ePort("LAYRS_E2E_STUDIO_WEB_PORT", 15174);
  env.LAYRS_E2E_SERVER_URL = process.env.LAYRS_E2E_SERVER_URL ?? `http://127.0.0.1:${serverPort}`;
  env.LAYRS_E2E_STUDIO_WEB_PORT = String(studioWebPort);
  cleanupE2eServices = () =>
    cleanupComposeProject("layrs-web-e2e", Number.parseInt(new URL(env.LAYRS_E2E_SERVER_URL).port, 10));
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
  cleanupE2eServices();
  if (signal) {
    console.error(`Playwright exited with signal ${signal}`);
    process.exit(1);
  }
  process.exit(code ?? 0);
});

process.on("SIGINT", () => {
  child.kill("SIGINT");
  cleanupE2eServices();
  process.exit(130);
});

process.on("SIGTERM", () => {
  child.kill("SIGTERM");
  cleanupE2eServices();
  process.exit(143);
});

async function e2ePort(envName, fallback) {
  const configured = Number.parseInt(process.env[envName] ?? "", 10);
  if (Number.isFinite(configured) && configured > 0) {
    return configured;
  }
  return isIsolated ? freePort() : fallback;
}

async function freePort() {
  return new Promise((resolvePort, rejectPort) => {
    const server = createServer();
    server.once("error", rejectPort);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (!address || typeof address === "string") {
        server.close(() => rejectPort(new Error("Could not allocate a TCP port.")));
        return;
      }
      const port = address.port;
      server.close(() => resolvePort(port));
    });
  });
}

function cleanupComposeProject(prefix, port) {
  if (!isIsolated || !Number.isInteger(port) || port <= 0) {
    return;
  }

  const composeEnv = {
    ...process.env,
    LAYRS_COMPOSE_PROJECT_NAME: `${prefix}-${port}`,
    LAYRS_POSTGRES_CONTAINER_NAME: `${prefix}-postgres-${port}`,
    LAYRS_MINIO_CONTAINER_NAME: `${prefix}-minio-${port}`
  };

  spawnSync("docker", ["compose", "down", "--volumes", "--remove-orphans"], {
    cwd: rootDir,
    env: composeEnv,
    stdio: "inherit",
    windowsHide: true
  });
}
