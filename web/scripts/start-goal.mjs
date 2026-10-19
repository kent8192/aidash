import { chromium, expect } from "@playwright/test";
// Only a Tenant subject submits goals (#371); golden_path.py passes its credential.
const token = process.env.AIDASH_E2E_TOKEN;
const tenant = process.env.AIDASH_E2E_TENANT;
const subject = process.env.AIDASH_E2E_SUBJECT;
if (!token || !tenant || !subject)
  throw new Error(
    "AIDASH_E2E_TOKEN, AIDASH_E2E_TENANT and AIDASH_E2E_SUBJECT are required",
  );
const mapping = "acceptance-mapping";
const browser = await chromium.launch();
try {
  const page = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
  });
  await page.addInitScript((context) => {
    sessionStorage.setItem("aidash-session-id", "acceptance-browser-session");
    sessionStorage.setItem("aidash-context", context);
    localStorage.setItem("aidash-locale", "ja-JP");
  }, `mapping:${tenant}:${mapping}`);
  await page.route("**/auth/config", (route) =>
    route.fulfill({ json: { enabled: true } }),
  );
  await page.route("**/auth/session", (route) =>
    route.fulfill({
      json: {
        id: "acceptance-browser-session",
        deployment_administrator: false,
        mappings: [{ id: mapping, tenant, subject }],
      },
    }),
  );
  await page.route("**/auth/activity", (route) =>
    route.fulfill({ status: 204 }),
  );
  let payload;
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const headers = {
      ...request.headers(),
      authorization: `Bearer ${token}`,
    };
    if (
      request.url().endsWith("/api/conversations") &&
      request.method() === "POST"
    ) {
      // Capture the real response before the UI can navigate away and Chromium
      // discards its body. Forward the same response to the submitting UI.
      const response = await route.fetch({ headers });
      payload = await response.json();
      await route.fulfill({ response });
    } else {
      await route.continue({ headers });
    }
  });
  await page.goto(process.env.AIDASH_E2E_URL ?? "http://127.0.0.1:18080");
  await page.getByRole("button", { name: "新しい依頼", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByLabel("タイトル", { exact: true })
    .fill("Rustフレームワークの競合調査");
  await dialog
    .getByLabel("ゴール", { exact: true })
    .fill("Rust製Webフレームワークについて競合調査して");
  await dialog.getByRole("combobox").selectOption("research-cluster@1.0.0");
  const created = page.waitForResponse(
    (response) =>
      response.url().endsWith("/api/conversations") &&
      response.request().method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "新しいゴール", exact: true })
    .click();
  const response = await created;
  expect(response.status()).toBe(200);
  await expect(dialog).not.toBeVisible({ timeout: 30000 });
  await page.screenshot({
    path: "private/acceptance/dashboard-goal.png",
    fullPage: true,
  });
  process.stdout.write(JSON.stringify(payload));
} finally {
  await browser.close();
}
