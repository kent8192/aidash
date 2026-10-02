import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "./desktop-tests",
  outputDir: "./desktop-test-results",
  workers: 1,
  use: { baseURL: "http://127.0.0.1:18083", trace: "retain-on-failure" },
  webServer: {
    command:
      "node node_modules/vite/bin/vite.js --host 127.0.0.1 --port 18083 --strictPort",
    url: "http://127.0.0.1:18083",
    reuseExistingServer: false,
  },
});
