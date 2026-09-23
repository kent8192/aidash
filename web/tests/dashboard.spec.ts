import { test, expect } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";

test.beforeEach(async ({ page }) => {
  await installBearerDashboard(page, "acceptance-access-token");
  await page.goto("/");
  await expect(page.locator(".collab-app")).toBeVisible();
});

test("observes the two-node execution and all management screens", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await expect(page.locator(".stream-status")).toHaveText("接続中");
  await expect(
    page.locator(".collab-rail nav").first().getByRole("link"),
  ).toHaveCount(2);
  await page.goto("/graph");
  await page
    .getByRole("button", { name: "ノード構成と通信", exact: true })
    .click();
  await expect(page.locator(".mesh-canvas svg > g")).toHaveCount(2);
  await expect(page.locator(".collab-run-grid button")).not.toHaveCount(0);
  for (const view of [
    "agents",
    "generation",
    "clusters",
    "registry",
    "marketplace",
    "node",
  ]) {
    await page.goto(`/settings?view=${view}`);
    await expect(page.locator(".collab-settings-select select")).toHaveValue(
      view,
    );
    await expect(page.locator(".collab-settings h1")).toHaveText("設定");
  }
  await page.goto("/collaboration");
  await page.getByTestId("language-selector").selectOption("en-US");
  await expect(
    page
      .locator(".collab-rail nav")
      .getByRole("link", { name: "Collaboration", exact: true }),
  ).toBeVisible();
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
  await page.goto("/collaboration");
  await page.getByRole("button", { name: "準備用チャンネル" }).click();
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
    .getByRole("button", { name: "タスクと成果物", exact: true })
    .click();
  await page.getByRole("button", { name: "タスクを追加", exact: true }).click();
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
  await page.locator(".collab-task").filter({ hasText: taskName }).click();
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
  const selectedLabel = await select.locator("option:checked").textContent();
  let reordered = false;
  await page.route("**/api/discover", async (route) => {
    const response = await route.fetch({
      headers: { ...route.request().headers(), authorization: "Bearer acceptance-access-token" },
    });
    const discovery = await response.json();
    discovery.agents.reverse();
    reordered = true;
    await route.fulfill({ response, json: discovery });
  });
  await expect.poll(() => reordered, { timeout: 15000 }).toBe(true);
  await expect(select.locator("option:checked")).toHaveText(selectedLabel!);

  await page
    .getByRole("dialog")
    .getByRole("button", { name: "担当を割り当て" })
    .click();
  const row = page.locator(".collab-task").filter({ hasText: taskName });
  await expect(row).toContainText("完了", { timeout: 30000 });
});

test("publishes and installs a skill through the marketplace", async ({
  page,
}) => {
  const id = `browser-skill-${Date.now()}`;
  await page.goto("/settings?view=registry");
  await page
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("エンティティの種類").selectOption("skill");
  await expect(dialog.getByLabel("エンティティID")).toHaveCount(0);
  await dialog.getByLabel("名前").fill(`Research checklist ${id}`);
  await dialog.getByLabel("説明").fill("Cite every factual claim.");
  await dialog
    .getByLabel("指示", { exact: true })
    .fill("Cite every factual claim.");
  const registration = page.waitForResponse(
    (response) =>
      response.url().endsWith("/api/registry") &&
      response.request().method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const registered = await (await registration).json();
  await expect(dialog).not.toBeVisible();
  await page.goto("/settings?view=marketplace");
  await page
    .getByRole("button", { name: "パッケージを公開", exact: true })
    .click();
  await dialog
    .getByLabel("ローカルのエンティティ")
    .selectOption({ label: `${registered.id}@1.0.0` });
  await dialog.getByLabel("作成者").fill("Acceptance fixture");
  await dialog
    .getByRole("button", { name: "パッケージを公開", exact: true })
    .click();
  await expect(dialog).not.toBeVisible();
  await page
    .getByRole("button")
    .filter({
      has: page.getByRole("heading", {
        name: `Research checklist ${id}`,
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

test("preserves a draft after a failed message request and clears it after success", async ({
  page,
}) => {
  await page.goto("/collaboration");
  const input = page
    .getByRole("textbox", { name: "メッセージ", exact: true })
    .first();
  await input.fill("Keep this unsent draft");
  await page.route("**/api/workspaces/*/thread-messages", async (route) => {
    await route.fulfill({
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({ error: "Temporary message failure" }),
    });
  });
  const failed = page.waitForResponse(
    (response) =>
      response.url().includes("/thread-messages") && response.status() === 503,
  );
  await page.getByRole("button", { name: "送信", exact: true }).first().click();
  await failed;
  await expect(input).toHaveValue("Keep this unsent draft");
  await page.unroute("**/api/workspaces/*/thread-messages");
  await page.getByRole("button", { name: "送信", exact: true }).first().click();
  await expect(input).toHaveValue("");
});

test("creates task dependencies and a parent through the dashboard", async ({
  page,
}) => {
  const headers = { authorization: "Bearer acceptance-access-token" };
  const name = `Relationships ${Date.now()}`;
  const workspace = await (
    await page.request.post("/api/workspaces", {
      headers,
      data: { title: name, goal: "Task relationships" },
    })
  ).json();
  const tasks = [];
  for (const title of ["Parent", "Prerequisite"]) {
    const response = await page.request.post(
      `/api/workspaces/${workspace.id}/tasks`,
      {
        headers,
        data: {
          title,
          description: title,
          requirements: {},
          dependencies: [],
          parent_id: null,
        },
      },
    );
    expect(response.status()).toBe(200);
    tasks.push(await response.json());
  }
  await page.goto(`/collaboration?channel=${encodeURIComponent(workspace.id)}`);
  await page
    .getByRole("button", { name: "タスクと成果物", exact: true })
    .click();
  await page.getByRole("button", { name: "タスクを追加", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.locator(`select[name="workspace"] option[value="${workspace.id}"]`),
  ).toHaveCount(1);
  await dialog
    .getByLabel("ワークスペース", { exact: true })
    .selectOption(workspace.id);
  await dialog
    .getByLabel("タイトル", { exact: true })
    .fill("Child with prerequisite");
  await dialog
    .getByLabel("説明", { exact: true })
    .fill("Created with immutable relationships");
  await dialog.locator('textarea[name="requirements"]').fill("{}");
  await dialog
    .getByLabel("親タスク", { exact: true })
    .selectOption(tasks[0].id);
  await expect(
    dialog.locator(
      `select[name="dependencies"] option[value="${tasks[0].id}"]`,
    ),
  ).toHaveCount(0);
  await dialog
    .getByLabel("依存関係", { exact: true })
    .selectOption([tasks[1].id]);
  const response = page.waitForResponse(
    (r) =>
      r.url().endsWith(`/api/workspaces/${workspace.id}/tasks`) &&
      r.request().method() === "POST",
  );
  await dialog.getByRole("button", { name: "作成", exact: true }).click();
  const created = await response;
  expect(created.status()).toBe(200);
  expect(await created.json()).toMatchObject({
    parent_id: tasks[0].id,
    dependencies: [tasks[1].id],
  });
});
