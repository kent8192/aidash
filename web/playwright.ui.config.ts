import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "./tests",
  testMatch: [
    "collaboration.spec.ts",
    "agent-graph.spec.ts",
    "agent-skills.spec.ts",
    "entity-configuration.spec.ts",
    "openrouter.spec.ts",
    "generation-validation.spec.ts",
  ],
  workers: 1,
  retries: 0,
  timeout: 30000,
  use: {
    baseURL: "http://127.0.0.1:18082",
    viewport: { width: 1440, height: 1000 },
    launchOptions: { executablePath: process.env.AIDASH_CHROMIUM_EXECUTABLE },
    trace: "retain-on-failure",
  },
  webServer: {
    command:
      "node node_modules/vite/bin/vite.js preview --host 127.0.0.1 --port 18082 --strictPort",
    url: "http://127.0.0.1:18082",
    reuseExistingServer: false,
  },
  reporter: "list",
});
