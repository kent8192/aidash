import { resolve } from "node:path";

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
        driverProvider: "embedded",
      },
    },
  ],
  services: [
    [
      "@wdio/tauri-service",
      {
        driverProvider: "embedded",
        appBinaryPath: process.env.AIDASH_E2E_BINARY,
        embeddedPort: Number(process.env.TAURI_WEBDRIVER_PORT),
        startTimeout: 60000,
        autoInstallTauriDriver: false,
        autoDownloadEdgeDriver: false,
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
