import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/e2e",
  testMatch: process.env.LAYRS_NATIVE_UI_TARGET === "desktop-partial"
    ? /desktop-native-partial\.spec\.ts/
    : /desktop-native-(?:scm|team)\.spec\.ts/,
  fullyParallel: false,
  workers: 1,
  outputDir: "test-results/native",
  reporter: [["list"], ["html", { open: "never", outputFolder: "playwright-report/native" }]],
  timeout: 180_000,
  retries: 0,
  use: {
    trace: "retain-on-failure",
    screenshot: "only-on-failure"
  }
});
