import { defineConfig } from "@playwright/test";

const server = (port: number, command: string) => ({
  command: `node node_modules/vite/bin/vite.js ${command} --host 127.0.0.1 --port ${port} --strictPort`,
  url: `http://127.0.0.1:${port}`,
  reuseExistingServer: false,
});

export default defineConfig({
  testDir: "./tests",
  outputDir: "test-results",
  workers: 1,
  use: { trace: "retain-on-failure" },
  projects: [
    {
      // The SDK fixture replaces source modules that only the dev server serves.
      name: "sources",
      testMatch: ["gcip-sign-in.spec.ts", "desktop-gcip-sign-in.spec.mjs"],
      use: { baseURL: "http://127.0.0.1:18084" },
    },
    {
      // Needs `npm run build` first.
      name: "bundle",
      testMatch: [
        "gcip-second-factor-menu.spec.ts",
        "plan-connections-unavailable.spec.ts",
      ],
      use: { baseURL: "http://127.0.0.1:18085" },
    },
  ],
  webServer: [
    server(18084, "--config vite.config.ts"),
    server(18085, "preview --config vite.config.ts"),
  ],
});
