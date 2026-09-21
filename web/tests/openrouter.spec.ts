import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() => {
    sessionStorage.setItem("aidash-token", "fixture-token");
    localStorage.setItem("aidash-locale", "ja-JP");
  });
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") {
      await route.abort();
    } else if (path === "/api/providers/openrouter/models") {
      await route.fulfill({
        json: [
          {
            id: "vendor/fixture-model",
            name: "Fixture Chat",
            context_length: 65536,
            pricing: { prompt: "0.000001", completion: "0.000003" },
            architecture: {
              input_modalities: ["text"],
              output_modalities: ["text"],
            },
            supported_parameters: ["tools", "reasoning"],
            reasoning: {
              supported_efforts: ["high", "low", "none"],
              mandatory: true,
              default_effort: "low",
            },
          },
          {
            id: "other/model",
            name: "Other Model",
            context_length: 8192,
            pricing: { prompt: "0", completion: "0" },
            architecture: {
              input_modalities: ["text"],
              output_modalities: ["text"],
            },
            supported_parameters: ["tools"],
          },
        ],
      });
    } else if (path === "/api/session") {
      await route.fulfill({
        json: { access: { kind: "operator" }, node_id: "aidash://test" },
      });
    } else if (path === "/api/state") {
      await route.fulfill({
        json: {
          access: { kind: "operator" },
          node: {
            id: "aidash://test",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            capabilities: [],
            clusters: [],
          },
          registry: [],
          workspaces: [],
          tasks: [],
          artifacts: [],
          events: [],
          runs: [],
          conversations: [],
          human_requests: [],
          installations: [],
          peers: [],
        },
      });
    } else if (path === "/api/mesh") {
      await route.fulfill({ json: { nodes: [], errors: [] } });
    } else if (path === "/api/discover") {
      await route.fulfill({ json: { agents: [], errors: [] } });
    } else if (
      path === "/api/registry" &&
      route.request().method() === "POST"
    ) {
      await route.fulfill({ json: route.request().postDataJSON() });
    } else {
      await route.fulfill({ json: [] });
    }
  });
  await page.goto("/registry");
  await page
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByLabel("エンティティの種類")
    .selectOption("model");
  expect(errors).toEqual([]);
});

test("searches and selects a model without credential input", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByLabel("プロバイダー", { exact: true })).toHaveCount(
    0,
  );
  await expect(dialog.getByLabel("資格情報の参照名")).toHaveCount(0);
  await dialog.getByLabel("エンティティID").fill("openrouter-model");
  await dialog.getByLabel("名前 · English").fill("OpenRouter test model");
  await dialog.getByLabel("説明 · English").fill("A model for agent execution");
  const picker = dialog.getByRole("combobox", {
    name: "プロバイダーのモデルID",
  });
  await picker.fill("not-found");
  await expect(dialog.getByText("一致するモデルがありません。")).toBeVisible();
  await picker.fill("FIXTURE");
  await expect(dialog.getByRole("option", { name: /Other Model/ })).toHaveCount(
    0,
  );
  await dialog.getByRole("option", { name: /Fixture Chat/ }).click();
  await expect(dialog.getByLabel("コンテキスト上限")).toHaveValue("65536");
  await expect(dialog.getByText(/1 \/ 3$/)).toBeVisible();
  await expect(dialog.getByText(/Zero Data Retention：常時有効/)).toBeVisible();
  const effort = dialog.getByLabel("Reasoning Effort");
  await expect(effort.locator("option")).toHaveText([
    "モデルの既定値",
    "high",
    "low",
  ]);
  await effort.selectOption("high");
  const submitted = page.waitForRequest(
    (r) =>
      new URL(r.url()).pathname === "/api/registry" && r.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  expect((await submitted).postDataJSON().config).toMatchObject({
    provider: "openrouter",
    model_id: "vendor/fixture-model",
    endpoint: "https://openrouter.ai/api/v1",
    credential_env: "AIDASH_SECRET_OPENROUTER",
    reasoning_effort: "high",
    context_window: 65536,
    cost: { input_per_million: 1, output_per_million: 3, currency: "USD" },
  });
  await expect(dialog).not.toBeVisible();
});

test("requires a catalog selection and supports keyboard selection", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("エンティティID").fill("openrouter-model");
  await dialog.getByLabel("名前 · English").fill("Model");
  await dialog.getByLabel("説明 · English").fill("Model description");
  const picker = dialog.getByRole("combobox", {
    name: "プロバイダーのモデルID",
  });
  await picker.fill("vendor/");
  await expect(
    dialog.getByRole("option", { name: /Fixture Chat/ }),
  ).toBeVisible();
  await picker.press("Enter");
  await expect(dialog.locator('[name="model_id"]')).toHaveValue(
    "vendor/fixture-model",
  );
  await picker.fill("unregistered/model");
  await dialog
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  await expect(
    dialog.getByText("一覧からモデルを選択してください。", { exact: false }),
  ).toBeVisible();
  await expect(dialog.locator('[name="model_id"]')).toHaveValue("");
});

test("retries a failed model catalog request", async ({ page }) => {
  await page.route("**/api/providers/openrouter/models", async (route) => {
    await route.fulfill({
      status: 502,
      json: { error: "upstream unavailable" },
    });
  });
  await page.reload();
  await page
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("エンティティの種類").selectOption("model");
  await expect(dialog.getByRole("alert")).toContainText(
    "モデル一覧を取得できませんでした。",
  );
  await page.unroute("**/api/providers/openrouter/models");
  await dialog.getByRole("button", { name: "再試行", exact: true }).click();
  const picker = dialog.getByRole("combobox", {
    name: "プロバイダーのモデルID",
  });
  await picker.fill("other/model");
  await expect(
    dialog.getByRole("option", { name: /Other Model/ }),
  ).toBeVisible();
  await expect(dialog.getByRole("alert")).toHaveCount(0);
});

test("clears reasoning settings when selecting an unsupported model", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  const picker = dialog.getByRole("combobox", {
    name: "プロバイダーのモデルID",
  });
  await picker.fill("fixture");
  await dialog.getByRole("option", { name: /Fixture Chat/ }).click();
  await dialog.getByLabel("Reasoning Effort").selectOption("high");
  await picker.fill("other/model");
  await dialog.getByRole("option", { name: /Other Model/ }).click();
  await expect(dialog.getByLabel("Reasoning Effort")).toBeDisabled();
  await expect(dialog.getByLabel("Reasoning Effort")).toHaveValue("");
  await expect(dialog.getByLabel("コンテキスト上限")).toHaveValue("8192");
});
