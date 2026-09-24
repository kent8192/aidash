import { chromium } from "@playwright/test";
import assert from "node:assert/strict";

const baseURL = process.env.AIDASH_E2E_URL;
assert(baseURL, "AIDASH_E2E_URL is required");
const browser = await chromium.launch({ headless: true });
try {
  const context = await browser.newContext({
    baseURL,
    viewport: { width: 1440, height: 1000 },
  });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() => {
    sessionStorage.setItem("aidash-session-id", "cluster-browser-session");
    sessionStorage.setItem("aidash-context", "operator");
    localStorage.setItem("aidash-locale", "en-US");
  });
  const configResponse = await context.request.get("/auth/config");
  assert.equal(configResponse.status(), 200);
  assert.equal(typeof (await configResponse.json()).enabled, "boolean");
  await page.route("**/auth/config", (route) =>
    route.fulfill({
      json: { enabled: true, login_url: "/auth/login" },
    }),
  );
  await page.route("**/auth/session", (route) =>
    route.fulfill({
      json: { id: "cluster-browser-session", operator: true, mappings: [] },
    }),
  );
  await page.route("**/auth/activity", (route) =>
    route.fulfill({ status: 204 }),
  );
  await page.route("**/api/**", (route) =>
    route.continue({
      headers: {
        ...route.request().headers(),
        authorization: "Bearer acceptance-access-token",
      },
    }),
  );
  const response = await context.request.get("/api/deployment", {
    headers: { authorization: "Bearer acceptance-access-token" },
  });
  assert.equal(response.status(), 200);
  const observed = await response.json();
  assert(observed.enabled);
  const worker = observed.deployments.find((d) => d.role === "worker");
  assert.equal(worker.desired, 3);
  assert.equal(worker.ready, 3);
  await page.goto("/deployment");
  await page
    .getByRole("heading", { name: "Replica status", exact: true })
    .waitFor();
  assert.equal(
    await page.getByRole("heading", { name: worker.name, exact: true }).count(),
    1,
  );
  await page.screenshot({
    path: ".ignore/dashboard-deployment-cluster-desktop.png",
    fullPage: true,
  });
  const accountMenu = page.locator(".account-popover > summary");
  await accountMenu.click();
  await page.getByTestId("language-selector").selectOption("ja-JP");
  await accountMenu.click();
  await page.setViewportSize({ width: 390, height: 844 });
  await page
    .getByRole("heading", { name: "レプリカの状態", exact: true })
    .waitFor();
  await page.screenshot({
    path: ".ignore/dashboard-deployment-cluster-mobile.png",
    fullPage: true,
  });
  assert(
    await page.evaluate(
      () =>
        globalThis.document.documentElement.scrollWidth <=
        globalThis.innerWidth,
    ),
  );
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify({
      namespace: observed.namespace,
      release: observed.release,
      ready_workers: worker.ready,
      locales: ["en-US", "ja-JP"],
      browser: "passed",
    }),
  );
} finally {
  await browser.close();
}
