import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "./tests",
  // The GCIP SDK fixture intercepts source modules served by its own Vite server.
  // Hosted CI runs that suite with playwright.gcip.config.ts.
  testIgnore: "gcip-sign-in.spec.ts",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 45000,
  use: {
    baseURL: process.env.AIDASH_E2E_URL ?? "http://127.0.0.1:18080",
    viewport: { width: 1440, height: 1000 },
    trace: "retain-on-failure",
  },
  reporter: "list",
});
