import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("アクセストークン").fill("acceptance-access-token");
  await page.getByRole("button", { name: "接続", exact: true }).click();
  await expect(page.getByRole("heading", { name: "概要." })).toBeVisible();
});

test("observes the two-node execution and all eleven screens", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await expect(
    page.locator(".stats .stat").nth(2).locator("strong"),
  ).toHaveText(/^[4-9]$|^\d{2,}$/);
  await expect(
    page.locator(".stats .stat").nth(3).locator("strong"),
  ).toHaveText("2");
  for (const label of [
    "エージェント",
    "クラスター",
    "メッシュ",
    "タスク",
    "ワークスペース",
    "会話",
    "レジストリ",
    "マーケットプレイス",
    "イベント",
    "設定",
    "概要",
  ]) {
    await page
      .locator(".sidebar")
      .getByRole("link", { name: label, exact: true })
      .click();
    await expect(page.locator("main.content h1")).toHaveText(label + ".");
  }
  await page.getByLabel("言語").selectOption("en-US");
  await expect(page.locator("main.content h1")).toHaveText("Overview.");
  await page.screenshot({
    path: "../.ignore/dashboard-overview.png",
    fullPage: true,
  });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({
    path: "../.ignore/dashboard-mobile.png",
    fullPage: true,
  });
  expect(errors).toEqual([]);
});

test("creates a workspace and task and receives live assignment changes", async ({
  page,
}) => {
  const name = `Browser workspace ${Date.now()}`;
  const taskName = `Browser research ${Date.now()}`;
  await page
    .locator(".sidebar")
    .getByRole("link", { name: "ワークスペース", exact: true })
    .click();
  await page.getByRole("button", { name: "ワークスペースを作成" }).click();
  await page
    .getByRole("dialog")
    .getByLabel("タイトル", { exact: true })
    .fill(name);
  await page
    .getByRole("dialog")
    .getByLabel("ゴール", { exact: true })
    .fill("Verify a browser-created task");
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "作成", exact: true })
    .click();
  await expect(page.getByRole("dialog")).not.toBeVisible();
  await page
    .locator(".sidebar")
    .getByRole("link", { name: "タスク", exact: true })
    .click();
  await page.getByRole("button", { name: "タスクを作成", exact: true }).click();
  await page
    .getByRole("dialog")
    .getByLabel("ワークスペース", { exact: true })
    .selectOption({ label: name });
  await page
    .getByRole("dialog")
    .getByLabel("タイトル", { exact: true })
    .fill(taskName);
  await page
    .getByRole("dialog")
    .getByLabel("説明", { exact: true })
    .fill("Research the browser workflow");
  await page
    .getByRole("dialog")
    .getByLabel("タスクの要件", { exact: true })
    .fill("{}");
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "作成", exact: true })
    .click();
  await expect(page.getByRole("dialog")).not.toBeVisible();
  await page.getByRole("button", { name: taskName, exact: true }).click();
  await page
    .getByRole("button", { name: "担当を割り当て", exact: true })
    .click();
  const select = page
    .getByRole("dialog")
    .getByLabel("エージェント", { exact: true });
  const value = await select
    .locator("option")
    .filter({ hasText: "調査エージェント" })
    .first()
    .getAttribute("value");
  await select.selectOption(value!);
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "担当を割り当て" })
    .click();
  const row = page.getByRole("row").filter({
    has: page.getByRole("button", { name: taskName, exact: true }),
  });
  await expect(row).toContainText("完了", { timeout: 30000 });
});

test("publishes and installs a localized skill through the marketplace", async ({
  page,
}) => {
  const id = `browser-skill-${Date.now()}`;
  await page
    .locator(".sidebar")
    .getByRole("link", { name: "レジストリ", exact: true })
    .click();
  await page
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("エンティティの種類").selectOption("skill");
  await dialog.getByLabel("エンティティID").fill(id);
  await dialog.getByLabel("名前 · English").fill("Research checklist");
  await dialog.getByLabel("名前 · 日本語").fill(`調査チェックリスト ${id}`);
  await dialog.getByLabel("説明 · English").fill("Cite every factual claim.");
  await dialog
    .getByLabel("構成", { exact: true })
    .fill('{"instructions":"Cite every factual claim."}');
  await dialog
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  await page
    .locator(".sidebar")
    .getByRole("link", { name: "マーケットプレイス", exact: true })
    .click();
  await page
    .getByRole("button", { name: "パッケージを公開", exact: true })
    .click();
  await dialog
    .getByLabel("ローカルのエンティティ")
    .selectOption({ label: `${id}@1.0.0` });
  await dialog.getByLabel("作成者").fill("Acceptance fixture");
  await dialog
    .getByRole("button", { name: "パッケージを公開", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  await page
    .getByRole("button")
    .filter({
      has: page.getByRole("heading", {
        name: `調査チェックリスト ${id}`,
        exact: true,
      }),
    })
    .click();
  await dialog
    .getByRole("button", { name: "インストール", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  await expect(
    page.getByText("インストール済み", { exact: true }),
  ).toBeVisible();
});
