import { chromium, expect } from "@playwright/test";
const browser = await chromium.launch();
try {
  const page = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
  });
  await page.goto(process.env.AIDASH_E2E_URL ?? "http://127.0.0.1:18080");
  await page.getByLabel("アクセストークン").fill("acceptance-access-token");
  await page.getByRole("button", { name: "接続", exact: true }).click();
  await page
    .getByRole("button", { name: "ゴールを作成して実行", exact: true })
    .click();
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
  const payload = await response.json();
  await expect(dialog).not.toBeVisible();
  await page.screenshot({
    path: ".ignore/acceptance/dashboard-goal.png",
    fullPage: true,
  });
  process.stdout.write(JSON.stringify(payload));
} finally {
  await browser.close();
}
