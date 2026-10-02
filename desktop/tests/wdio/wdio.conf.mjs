import { resolve } from "node:path";

// The embedded driver's WebView2 bindings currently conflict with Tauri's.
// Both providers run the same native acceptance suite.
const external = process.platform === "win32";
const driverProvider = external ? "external" : "embedded";

export const config = {
  runner: "local",
  specs: ["./native.spec.mjs"],
  maxInstances: 1,
  logLevel: process.env.AIDASH_E2E_DEBUG ? "info" : "warn",
  outputDir: resolve(
    "artifacts/wdio",
    process.platform,
    process.env.AIDASH_E2E_PHASE,
  ),
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { timeout: 120000, bail: true },
  waitforTimeout: 30000,
  connectionRetryCount: 0,
  connectionRetryTimeout: 45000,
  specFileRetries: 0,
  capabilities: [
    {
      browserName: "tauri",
      "wdio:tauriServiceOptions": {
        appBinaryPath: process.env.AIDASH_E2E_BINARY,
        driverProvider,
      },
    },
  ],
  services: [
    [
      "@wdio/tauri-service",
      {
        driverProvider,
        appBinaryPath: process.env.AIDASH_E2E_BINARY,
        embeddedPort: Number(process.env.TAURI_WEBDRIVER_PORT),
        tauriDriverPort: Number(process.env.TAURI_WEBDRIVER_PORT),
        startTimeout: 60000,
        autoInstallTauriDriver: external,
        autoDownloadEdgeDriver: external,
        captureBackendLogs: false,
        captureFrontendLogs: false,
      },
    ],
  ],
  async afterTest(_test, _context, { passed }) {
    if (!passed) {
      const { browser } = await import("@wdio/globals");
      await browser.saveScreenshot(
        resolve(
          "artifacts/wdio",
          process.platform,
          `${process.env.AIDASH_E2E_PHASE}-failure.png`,
        ),
      );
    }
  },
};
