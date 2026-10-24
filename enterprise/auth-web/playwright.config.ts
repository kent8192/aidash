import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  use: {
    baseURL: "http://127.0.0.1:4191",
    trace: "retain-on-failure",
    ...devices["Desktop Chrome"],
  },
  webServer: {
    command: "npm exec -- vite --host 127.0.0.1 --port 4191 --strictPort",
    url: "http://127.0.0.1:4191",
    reuseExistingServer: !process.env.CI,
  },
});
