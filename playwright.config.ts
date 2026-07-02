import { defineConfig, devices } from "@playwright/test";

const desktopPort = 15740;
const studioWebPort = 15750;

export default defineConfig({
  testDir: "./tests/e2e",
  fullyParallel: true,
  reporter: [["list"], ["html", { open: "never" }]],
  retries: process.env.CI ? 2 : 0,
  use: {
    trace: "on-first-retry",
    screenshot: "only-on-failure"
  },
  webServer: [
    {
      command: `node_modules\\.bin\\vite.CMD --host 127.0.0.1 --port ${desktopPort} --strictPort`,
      cwd: "apps/studio-desktop",
      url: `http://127.0.0.1:${desktopPort}`,
      reuseExistingServer: !process.env.CI,
      timeout: 120_000
    },
    {
      command: `node_modules\\.bin\\vite.CMD --host 127.0.0.1 --port ${studioWebPort} --strictPort`,
      cwd: "apps/studio-web",
      env: {
        ...process.env,
        VITE_LAYRS_STUDIO_MODE: "mock"
      },
      url: `http://127.0.0.1:${studioWebPort}`,
      reuseExistingServer: !process.env.CI,
      timeout: 120_000
    }
  ],
  projects: [
    {
      name: "Desktop renderer - Chromium",
      testMatch: /desktop\.spec\.ts/,
      use: {
        ...devices["Desktop Chrome"],
        baseURL: `http://127.0.0.1:${desktopPort}`
      }
    },
    {
      name: "Studio Web mock - Chromium",
      testMatch: /studio-web\.spec\.ts/,
      use: {
        ...devices["Desktop Chrome"],
        baseURL: `http://127.0.0.1:${studioWebPort}`
      }
    }
  ]
});
