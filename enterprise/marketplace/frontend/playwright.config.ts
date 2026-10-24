import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  use: {
    baseURL: "http://127.0.0.1:4192",
    trace: "retain-on-failure",
    ...devices["Desktop Chrome"],
  },
  webServer: {
    command: "npm run preview -- --port 4192 --strictPort",
    url: "http://127.0.0.1:4192",
    reuseExistingServer: !process.env.CI,
  },
});
