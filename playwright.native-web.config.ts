import { defineConfig } from "@playwright/test";

if (!process.env.LAYRS_NATIVE_UI_FIXTURE || !process.env.LAYRS_NATIVE_WEB_URL || !process.env.LAYRS_NATIVE_WEB_CERT_PIN) {
  throw new Error("Use node scripts/test-native-ui-ci.mjs web; a real disposable TLS fixture is required");
}
export default defineConfig({
  testDir: "./tests/e2e",
  testMatch: /studio-web-native\.spec\.ts/,
  workers: 1,
  fullyParallel: false,
  timeout: 120_000,
  retries: 0,
  outputDir: "test-results/native-web",
  reporter: [["list"], ["html", { open: "never", outputFolder: "playwright-report/native-web" }]],
  use: {
    baseURL: process.env.LAYRS_NATIVE_WEB_URL,
    headless: true,
    // Scope trust to the generated certificate's public key. The HTTPS proxy
    // independently validates the backend CA; no global TLS bypass is used.
    launchOptions: { args: [`--ignore-certificate-errors-spki-list=${process.env.LAYRS_NATIVE_WEB_CERT_PIN}`] },
    trace: "off", screenshot: "off", video: "off",
  },
});
