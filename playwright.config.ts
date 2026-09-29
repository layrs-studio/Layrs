import { defineConfig, devices } from "@playwright/test";

const desktopPort = 15740;
const studioWebPort = 15750;
// Invoke Vite through node so the command is identical on Windows and Linux.
const vite = (port: number) => `node node_modules/vite/bin/vite.js --host 127.0.0.1 --port ${port} --strictPort`;

export default defineConfig({
  testDir: "./tests/e2e",
  fullyParallel: true,
  outputDir: "test-results/renderer",
  reporter: [["list"], ["html", { open: "never", outputFolder: "playwright-report/renderer" }]],
  retries: 0,
  use: {
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure"
  },
  webServer: [
    {
      command: vite(desktopPort),
      cwd: "apps/studio-desktop",
      url: `http://127.0.0.1:${desktopPort}`,
      reuseExistingServer: !process.env.CI,
      timeout: 120_000
    },
    {
      command: vite(studioWebPort),
      cwd: "apps/studio-web",
      url: `http://127.0.0.1:${studioWebPort}`,
      reuseExistingServer: !process.env.CI,
      timeout: 120_000
    }
  ],
  projects: [
    {
      name: "Desktop renderer - Chromium",
      testMatch: /desktop-renderer\.spec\.ts/,
      use: {
        ...devices["Desktop Chrome"],
        baseURL: `http://127.0.0.1:${desktopPort}`
      }
    },
    {
      name: "Studio Web renderer - Chromium",
      testMatch: /studio-web-renderer\.spec\.ts/,
      use: {
        ...devices["Desktop Chrome"],
        baseURL: `http://127.0.0.1:${studioWebPort}`
      }
    }
  ]
});
