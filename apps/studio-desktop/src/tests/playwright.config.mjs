import { defineConfig } from "@playwright/test";
import { fileURLToPath } from "node:url";

export default defineConfig({
  testDir: ".",
  testMatch: "desktop-worktrees-selection.spec.mjs",
  outputDir: "../../../../.layrs-local/desktop-selection-results",
  reporter: "list",
  workers: 1,
  projects: [
    { name: "Worktrees and selection" },
    { name: "Git bridge", testMatch: "desktop-git-bridge.spec.mjs" },
    { name: "Offline projection", testMatch: "desktop-offline.spec.mjs" },
    { name: "Existing Desktop renderer", testDir: "../../../../tests/e2e", testMatch: "desktop-renderer.spec.ts" },
  ],
  use: {
    baseURL: "http://127.0.0.1:15742",
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    launchOptions: process.env.LAYRS_DESKTOP_TEST_BROWSER ? { executablePath: process.env.LAYRS_DESKTOP_TEST_BROWSER } : {},
  },
  webServer: {
    command: "node node_modules/vite/bin/vite.js --host 127.0.0.1 --port 15742 --strictPort",
    cwd: fileURLToPath(new URL("../../", import.meta.url)),
    url: "http://127.0.0.1:15742",
    reuseExistingServer: true,
  },
});
