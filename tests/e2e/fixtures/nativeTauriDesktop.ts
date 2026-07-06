import { chromium, expect, type Browser, type Page, type TestInfo } from "@playwright/test";
import { spawn, spawnSync, type ChildProcessWithoutNullStreams } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { dirname, resolve } from "node:path";

type JsonRecord = Record<string, unknown>;

export interface NativeTauriDesktop {
  browser: Browser;
  page: Page;
  desktopUrl: string;
  instanceId: string;
  invoke<T = unknown>(command: string, args?: JsonRecord): Promise<T>;
  pause(ms?: number): Promise<void>;
  dispose(): Promise<void>;
}

export interface NativeTauriDesktopOptions {
  selectedFolder?: string;
  visual?: boolean;
}

declare global {
  interface Window {
    __TAURI__?: {
      core?: {
        invoke<T>(command: string, args?: JsonRecord): Promise<T>;
      };
    };
  }
}

const repoRoot = process.cwd();
const desktopAppDir = resolve(repoRoot, "apps/studio-desktop");
const desktopE2eCargoTargetDir = resolve(repoRoot, "target", "studio-desktop-e2e");
const nativeWindowSize = { width: 1920, height: 1080 };

export async function launchNativeTauriDesktop(
  testInfo: TestInfo,
  options: NativeTauriDesktopOptions = {}
): Promise<NativeTauriDesktop> {
  const debugPort = await freePort();
  const desktopPort = await freePort();
  const stateRoot = testInfo.outputPath("native-tauri-state");
  await mkdir(stateRoot, { recursive: true });

  const logs: string[] = [];
  const visual = options.visual ?? process.env.LAYRS_E2E_VISUAL !== "0";
  const instanceId = makeInstanceId(testInfo);
  const desktopUrl = `http://127.0.0.1:${desktopPort}`;
  const configPath = await writeE2eTauriConfig(stateRoot, desktopPort, instanceId);
  await stopStaleE2eDesktopProcesses();
  const child = spawnTauriDev(debugPort, desktopPort, instanceId, configPath, stateRoot, logs, options, visual);

  let browser: Browser | undefined;
  try {
    await waitForCdp(debugPort, logs, 120_000);
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${debugPort}`, {
      slowMo: visual ? visualSlowMoMs() : 0
    });
    const page = await waitForTauriPage(browser, desktopUrl, logs, 60_000);
    await page.bringToFront();
    await expect.poll(() => hasRealTauriInvoke(page), { timeout: 30_000 }).toBe(true);

    return {
      browser,
      page,
      desktopUrl,
      instanceId,
      invoke: (command, args) =>
        page.evaluate(
          ([commandName, commandArgs]) => window.__TAURI__!.core!.invoke(commandName, commandArgs),
          [command, args ?? {}] as [string, JsonRecord]
        ),
      pause: (ms) => delay(ms ?? visualStepDelayMs()),
      dispose: async () => {
        if (visual) {
          await delay(visualHoldMs());
        }
        await attachLogs(testInfo, logs);
        await browser?.close().catch(() => undefined);
        stopProcessTree(child);
        await stopStaleE2eDesktopProcesses();
      }
    };
  } catch (error) {
    await attachLogs(testInfo, logs);
    await browser?.close().catch(() => undefined);
    stopProcessTree(child);
    await stopStaleE2eDesktopProcesses();
    throw error;
  }
}

export async function makeNativeSpaceFolder(
  testInfo: TestInfo,
  files: Array<{ path: string; body: string }>
): Promise<string> {
  const root = testInfo.outputPath("native-space");
  for (const file of files) {
    const target = resolve(root, file.path);
    await mkdir(dirname(target), { recursive: true });
    await writeFile(target, file.body, "utf8");
  }
  return root;
}

async function hasRealTauriInvoke(page: Page): Promise<boolean> {
  try {
    return await page.evaluate(() => Boolean(window.__TAURI__?.core?.invoke));
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (message.includes("Execution context was destroyed") || message.includes("Cannot find context")) {
      return false;
    }
    throw error;
  }
}

function spawnTauriDev(
  debugPort: number,
  desktopPort: number,
  instanceId: string,
  configPath: string,
  stateRoot: string,
  logs: string[],
  options: NativeTauriDesktopOptions,
  visual: boolean
): ChildProcessWithoutNullStreams {
  const isolatedAppEnv =
    process.platform === "win32"
      ? {
          APPDATA: resolve(stateRoot, "AppData", "Roaming"),
          LOCALAPPDATA: resolve(stateRoot, "AppData", "Local")
        }
      : {
          HOME: resolve(stateRoot, "Home"),
          XDG_CONFIG_HOME: resolve(stateRoot, "xdg-config")
        };
  const env = {
    ...process.env,
    ...isolatedAppEnv,
    ...(options.selectedFolder ? { LAYRS_E2E_SELECTED_FOLDER: options.selectedFolder } : {}),
    CARGO_TARGET_DIR: desktopE2eCargoTargetDir,
    LAYRS_E2E_DESKTOP_PORT: String(desktopPort),
    LAYRS_E2E_INSTANCE_ID: instanceId,
    LAYRS_E2E_WINDOW_SIZE: `${nativeWindowSize.width}x${nativeWindowSize.height}`,
    WEBVIEW2_USER_DATA_FOLDER: resolve(stateRoot, "WebView2"),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${debugPort} --remote-allow-origins=*`
  };
  if (!visual) {
    env.CI = "true";
  } else if (!process.env.CI) {
    delete env.CI;
  }

  const child = spawn(
    process.platform === "win32" ? "cmd.exe" : "pnpm",
    process.platform === "win32"
      ? ["/d", "/s", "/c", "node_modules\\.bin\\tauri.CMD", "dev", "--config", configPath]
      : ["tauri", "dev", "--config", configPath],
    {
      cwd: desktopAppDir,
      env,
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: !visual
    }
  );

  child.stdout.on("data", (chunk) => pushLog(logs, chunk));
  child.stderr.on("data", (chunk) => pushLog(logs, chunk));
  child.on("exit", (code, signal) => pushLog(logs, `tauri dev exited with ${signal ?? code}`));
  pushLog(logs, `native e2e instance ${instanceId} frontend=${desktopPort} cdp=${debugPort}\n`);

  return child;
}

async function waitForCdp(debugPort: number, logs: string[], timeoutMs: number) {
  const deadline = Date.now() + timeoutMs;
  const url = `http://127.0.0.1:${debugPort}/json/version`;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) {
        return;
      }
    } catch {
      // WebView2 has not opened the debug port yet.
    }
    await delay(500);
  }

  throw new Error(`Timed out waiting for WebView2 CDP on ${url}.\n${logs.slice(-80).join("")}`);
}

async function waitForTauriPage(
  browser: Browser,
  desktopUrl: string,
  logs: string[],
  timeoutMs: number
): Promise<Page> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const pages = browser.contexts().flatMap((context) => context.pages());
    const page = pages.find((candidate) => candidate.url().startsWith(desktopUrl));
    if (page) {
      await page.waitForLoadState("domcontentloaded", { timeout: 30_000 }).catch(() => undefined);
      return page;
    }
    await delay(250);
  }

  throw new Error(`Timed out waiting for a Tauri WebView page at ${desktopUrl}.\n${logs.slice(-80).join("")}`);
}

async function writeE2eTauriConfig(stateRoot: string, desktopPort: number, instanceId: string): Promise<string> {
  const configPath = resolve(stateRoot, "tauri.e2e.conf.json");
  const title = `Layrs Studio E2E ${instanceId}`;
  const config = {
    build: {
      beforeDevCommand: `pnpm exec vite --host 127.0.0.1 --port ${desktopPort} --strictPort`,
      devUrl: `http://127.0.0.1:${desktopPort}`
    },
    app: {
      windows: [
        {
          label: "main",
          title,
          width: nativeWindowSize.width,
          height: nativeWindowSize.height,
          minWidth: 960,
          minHeight: 640
        }
      ]
    }
  };
  await writeFile(configPath, `${JSON.stringify(config, null, 2)}\n`, "utf8");
  return configPath;
}

function makeInstanceId(testInfo: TestInfo): string {
  const safeTitle = testInfo.titlePath.join("-").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
  const entropy = Math.random().toString(36).slice(2, 8);
  return `${process.pid}-${testInfo.workerIndex}-${testInfo.retry}-${safeTitle.slice(0, 48)}-${entropy}`;
}

async function freePort(): Promise<number> {
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

function stopProcessTree(child: ChildProcessWithoutNullStreams) {
  if (child.killed) {
    return;
  }

  if (process.platform === "win32" && child.pid) {
    spawnSync("taskkill.exe", ["/PID", String(child.pid), "/T", "/F"], {
      stdio: "ignore",
      windowsHide: true
    });
    return;
  }

  child.kill("SIGTERM");
}

async function stopStaleE2eDesktopProcesses() {
  if (process.platform !== "win32") {
    return;
  }

  spawnSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-Command",
      "Get-Process -Name layrs-studio-desktop -ErrorAction SilentlyContinue | Where-Object { $_.Path -like '*target*studio-desktop-e2e*' } | Stop-Process -Force"
    ],
    {
      stdio: "ignore",
      windowsHide: true
    }
  );
  await delay(800);
}

async function attachLogs(testInfo: TestInfo, logs: string[]) {
  if (logs.length === 0) {
    return;
  }
  await testInfo.attach("tauri-dev.log", {
    body: logs.join(""),
    contentType: "text/plain"
  });
}

function pushLog(logs: string[], chunk: unknown) {
  logs.push(Buffer.isBuffer(chunk) ? chunk.toString("utf8") : String(chunk));
  if (logs.length > 500) {
    logs.splice(0, logs.length - 500);
  }
}

function delay(ms: number) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, ms));
}

function visualSlowMoMs() {
  return numberEnv("LAYRS_E2E_SLOW_MO_MS", 250);
}

function visualStepDelayMs() {
  return numberEnv("LAYRS_E2E_STEP_DELAY_MS", 650);
}

function visualHoldMs() {
  return numberEnv("LAYRS_E2E_HOLD_MS", 2_000);
}

function numberEnv(name: string, fallback: number) {
  const value = Number.parseInt(process.env[name] ?? "", 10);
  return Number.isFinite(value) && value >= 0 ? value : fallback;
}
