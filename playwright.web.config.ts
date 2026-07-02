import { defineConfig, devices } from "@playwright/test";

const serverUrl = (process.env.LAYRS_E2E_SERVER_URL ?? "http://127.0.0.1:18887").replace(/\/$/, "");
const studioWebPort = Number.parseInt(process.env.LAYRS_E2E_STUDIO_WEB_PORT ?? "15174", 10);
const serverPort = Number.parseInt(new URL(serverUrl).port || "18887", 10);
const viteCommand =
  process.platform === "win32"
    ? `node_modules\\.bin\\vite.CMD --host 127.0.0.1 --port ${studioWebPort} --strictPort`
    : `node_modules/.bin/vite --host 127.0.0.1 --port ${studioWebPort} --strictPort`;

export default defineConfig({
  testDir: "./tests/e2e",
  testMatch: /studio-web-real\.spec\.ts/,
  fullyParallel: false,
  workers: 1,
  reporter: [["list"], ["html", { open: "never" }]],
  timeout: 180_000,
  retries: process.env.CI ? 1 : 0,
  use: {
    ...devices["Desktop Chrome"],
    baseURL: `http://127.0.0.1:${studioWebPort}`,
    headless: process.env.LAYRS_E2E_HEADLESS === "1",
    trace: "on-first-retry",
    screenshot: "only-on-failure",
    video: process.env.LAYRS_E2E_VISUAL === "1" ? "retain-on-failure" : "off"
  },
  webServer: [
    {
      command: "node scripts/dev.mjs",
      cwd: ".",
      env: {
        ...process.env,
        LAYRS_DEV_SKIP_STUDIO: "1",
        LAYRS_SERVER_PORT: String(serverPort),
        LAYRS_STUDIO_WEB_PORT: String(studioWebPort)
      },
      url: `${serverUrl}/healthz`,
      reuseExistingServer: !process.env.CI,
      timeout: 180_000
    },
    {
      command: viteCommand,
      cwd: "apps/studio-web",
      env: {
        ...process.env,
        VITE_LAYRS_API_URL: serverUrl,
        VITE_LAYRS_SERVER_URL: serverUrl
      },
      url: `http://127.0.0.1:${studioWebPort}`,
      reuseExistingServer: !process.env.CI,
      timeout: 120_000
    }
  ]
});
