import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "./tests",
  testMatch: "gcip-sign-in.spec.ts",
  workers: 1,
  use: { baseURL: "http://127.0.0.1:18084", trace: "retain-on-failure" },
  webServer: {
    command:
      "node node_modules/vite/bin/vite.js --host 127.0.0.1 --port 18084 --strictPort",
    url: "http://127.0.0.1:18084",
    reuseExistingServer: false,
  },
});
