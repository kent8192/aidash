import { defineConfig } from "@playwright/test";
const port = Number(process.env.AIDASH_UI_PORT ?? 18082);
if (!Number.isInteger(port) || port < 1 || port > 65535)
  throw new Error("AIDASH_UI_PORT must be a TCP port");
const baseURL = `http://127.0.0.1:${port}`;
export default defineConfig({
  testDir: "./tests",
  testMatch: [
    "collaboration.spec.ts",
    "mesh-graph.spec.ts",
    "agent-graph.spec.ts",
    "agent-skills.spec.ts",
    "entity-configuration.spec.ts",
    "registry-labels.spec.ts",
    "duplicate-labels.spec.ts",
    "openrouter.spec.ts",
    "generation-validation.spec.ts",
  ],
  workers: 1,
  retries: 0,
  timeout: 30000,
  use: {
    baseURL,
    viewport: { width: 1440, height: 1000 },
    launchOptions: { executablePath: process.env.AIDASH_CHROMIUM_EXECUTABLE },
    trace: "retain-on-failure",
  },
  webServer: {
    command: `node node_modules/vite/bin/vite.js preview --host 127.0.0.1 --port ${port} --strictPort`,
    url: baseURL,
    reuseExistingServer: false,
  },
  reporter: "list",
});
