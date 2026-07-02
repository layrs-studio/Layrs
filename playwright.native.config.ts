import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/e2e",
  testMatch: /desktop-native\.spec\.ts/,
  fullyParallel: false,
  workers: 1,
  reporter: [["list"], ["html", { open: "never" }]],
  timeout: 180_000,
  retries: process.env.CI ? 1 : 0,
  use: {
    trace: "on-first-retry",
    screenshot: "only-on-failure"
  }
});
