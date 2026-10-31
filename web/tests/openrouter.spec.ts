import { readFileSync } from "node:fs";
import { test, expect } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import {
  routeRegistryVersions,
  routeTenantActions,
  tenantSubject,
} from "./registry-fixture";

test.beforeEach(async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await installBearerDashboard(page, "fixture-token", {
    tenant: tenantSubject.tenant,
    name: tenantSubject.subject,
  });
  await page.addInitScript(() => {
    localStorage.setItem("aidash-locale", "ja-JP");
  });
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") {
      await route.abort();
    } else if (path === "/api/providers/openrouter/models") {
      await route.fulfill({
        json: {
          stale: false,
          fetched_at: "2026-10-18T00:00:00Z",
          models: [
            {
              id: "vendor/fixture-model",
              name: "Fixture Chat",
              context_length: 65536,
              top_provider: { max_completion_tokens: 65536 },
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
              top_provider: { max_completion_tokens: 4096 },
              pricing: { prompt: "0", completion: "0" },
              architecture: {
                input_modalities: ["text"],
                output_modalities: ["text"],
              },
              supported_parameters: ["tools"],
            },
          ],
        },
      });
    } else if (path === "/api/tenants/acme/plan-connections/chatgpt") {
      await route.fulfill({
        json: {
          enabled: true,
          connection: {
            id: "personal-plan",
            revision: 1,
            state: "active",
            refresh_expires_at: "2026-11-23T00:00:00Z",
          },
        },
      });
    } else if (path === "/api/tenants/acme/plan-connections/chatgpt/models") {
      await route.fulfill({
        json: {
          models: [
            {
              slug: "gpt-plan-fixture",
              display_name: "Plan Fixture",
              context_window: 128000,
              max_output_tokens: 16384,
              defaulted: true,
            },
          ],
        },
      });
    } else if (path === "/api/session") {
      await route.fulfill({
        json: {
          access: tenantSubject,
          node_id: "aidash://test",
        },
      });
    } else if (path === "/api/state") {
      await route.fulfill({
        json: {
          access: tenantSubject,
          deployment: {
            id: "aidash://test",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            key: null,
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
        },
      });
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
  await routeTenantActions(page, ["registry.create"], {
    deployment: "aidash://test",
  });
  await routeRegistryVersions(page, () => []);
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
  await expect(dialog.getByLabel("プロバイダー", { exact: true })).toHaveValue(
    "openrouter",
  );
  await expect(dialog.getByLabel("資格情報の参照名")).toHaveCount(0);
  await expect(dialog.getByLabel("エンティティID")).toHaveCount(0);
  await dialog.getByLabel("名前").fill("OpenRouter test model");
  await dialog.getByLabel("説明").fill("A model for agent execution");
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
  await expect(dialog.getByLabel("最大出力トークン数")).toHaveValue("65536");
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
    provider_credential: "openrouter",
    projection_versions: ["legacy", "ordered", "native"],
    reasoning_effort: "high",
    context_window: 65536,
    max_output_tokens: 65536,
    cost: { input_per_million: 1, output_per_million: 3, currency: "USD" },
  });
  await expect(dialog).not.toBeVisible();
});

test("requires a catalog selection and supports keyboard selection", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByLabel("エンティティID")).toHaveCount(0);
  await dialog.getByLabel("名前").fill("Model");
  await dialog.getByLabel("説明").fill("Model description");
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
  await expect(dialog.getByLabel("名前", { exact: true })).toHaveValue(
    "other-model-none",
  );
});

test("model names follow provider, model and effort until manually edited", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  const name = dialog.getByLabel("名前", { exact: true });
  await expect(name).toHaveValue("");
  const picker = dialog.getByRole("combobox", {
    name: "プロバイダーのモデルID",
  });
  await picker.fill("fixture");
  await dialog.getByRole("option", { name: /Fixture Chat/ }).click();
  await expect(name).toHaveValue("vendor-fixture-model-low");
  await dialog.getByLabel("Reasoning Effort").selectOption("high");
  await expect(name).toHaveValue("vendor-fixture-model-high");
  await dialog.getByLabel("説明").fill("Auto-named model");
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  expect((await posted).postDataJSON().name).toEqual({
    en: "vendor-fixture-model-high",
  });
});

test("manual model names survive model and effort changes", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  const name = dialog.getByLabel("名前", { exact: true });
  await name.fill("My research model");
  const picker = dialog.getByRole("combobox", {
    name: "プロバイダーのモデルID",
  });
  await picker.fill("fixture");
  await dialog.getByRole("option", { name: /Fixture Chat/ }).click();
  await dialog.getByLabel("Reasoning Effort").selectOption("high");
  await expect(name).toHaveValue("My research model");
  await name.fill("");
  await expect(name).toHaveValue("vendor-fixture-model-high");
  await dialog.getByLabel("Reasoning Effort").selectOption("");
  await expect(name).toHaveValue("vendor-fixture-model-low");
});

test("provider-first selection discovers personal plan models and submits an explicit Plan source", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("説明").fill("Personal plan model");
  await dialog
    .getByLabel("プロバイダー", { exact: true })
    .selectOption("chatgpt");
  await dialog
    .getByLabel("プロバイダーのモデルID", { exact: true })
    .selectOption("gpt-plan-fixture");
  await expect(
    dialog.getByText(
      "プロバイダが上限を返さなかったため、暫定値を表示しています。登録前に確認してください。",
    ),
  ).toBeVisible();
  await expect(dialog.getByLabel("コンテキスト上限")).toHaveValue("128000");
  await expect(dialog.getByLabel("最大出力トークン数")).toHaveValue("16384");
  const submitted = page.waitForRequest(
    (request) =>
      new URL(request.url()).pathname === "/api/registry" &&
      request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const config = (await submitted).postDataJSON().config;
  expect(config).toMatchObject({
    provider: "chatgpt",
    plan_connection: "chatgpt",
    streaming: true,
    endpoint: "https://api.openai.com/v1",
    model_id: "gpt-plan-fixture",
    context_window: 128000,
    max_output_tokens: 16384,
    projection_versions: ["legacy", "ordered", "native"],
    cost: {},
  });
  expect(config).not.toHaveProperty("provider_credential");
});

test("switching to a Keyless endpoint discards a provider catalog selection", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  await dialog
    .getByLabel("プロバイダー", { exact: true })
    .selectOption("chatgpt");
  await dialog
    .getByLabel("プロバイダーのモデルID", { exact: true })
    .selectOption("gpt-plan-fixture");
  await dialog
    .getByLabel("プロバイダー", { exact: true })
    .selectOption("keyless");
  await expect(
    dialog.getByLabel("プロバイダーのモデルID", { exact: true }),
  ).toHaveValue("");
  await dialog.getByLabel("説明").fill("Local Keyless model");
  await dialog
    .getByLabel("エンドポイント", { exact: true })
    .fill("http://127.0.0.1:11434/v1");
  await dialog
    .getByLabel("プロバイダーのモデルID", { exact: true })
    .fill("local-model");
  const submitted = page.waitForRequest(
    (request) =>
      new URL(request.url()).pathname === "/api/registry" &&
      request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const config = (await submitted).postDataJSON().config;
  const acceptedContract = JSON.parse(
    readFileSync(
      new URL(
        "../../crates/aidash-application/src/registry/tests/fixtures/keyless-dashboard-model.json",
        import.meta.url,
      ),
      "utf8",
    ),
  );
  expect(config).toMatchObject(acceptedContract);
  expect(config).not.toHaveProperty("provider_credential");
  expect(config).not.toHaveProperty("plan_connection");
});

test("Plan configuration disablement blocks model registration until enabled", async ({
  page,
}) => {
  let enabled = true;
  await page.route("**/api/tenants/acme/plan-connections/chatgpt", (route) =>
    route.fulfill({
      json: {
        enabled,
        connection: {
          id: "personal-plan",
          revision: 1,
          state: "active",
          refresh_expires_at: "2026-11-23T00:00:00Z",
        },
      },
    }),
  );
  const dialog = page.getByRole("dialog");
  await dialog
    .getByLabel("プロバイダー", { exact: true })
    .selectOption("chatgpt");
  const model = dialog.getByLabel("プロバイダーのモデルID", { exact: true });
  await model.selectOption("gpt-plan-fixture");
  enabled = false;
  await expect
    .poll(async () => {
      await page.evaluate(() =>
        window.dispatchEvent(new Event("visibilitychange")),
      );
      return model.isDisabled();
    })
    .toBe(true);
  await expect(model).toHaveValue("");
  await expect(
    dialog.getByText(
      "この Deployment では Plan モデルを登録できません。Plan 使用を有効にしてから登録してください。",
    ),
  ).toBeVisible();
  let submitted = false;
  page.on("request", (request) => {
    if (
      new URL(request.url()).pathname === "/api/registry" &&
      request.method() === "POST"
    )
      submitted = true;
  });
  await dialog.getByLabel("名前").fill("Disabled Plan");
  await dialog.getByLabel("説明").fill("Registration must remain blocked");
  await dialog.getByLabel("コンテキスト上限").fill("128000");
  await dialog.getByLabel("最大出力トークン数").fill("16384");
  await dialog
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  await expect(dialog.getByRole("alert")).toContainText(
    "一覧からモデルを選択してください。",
  );
  expect(submitted).toBe(false);
});

test("registers a manually entered Plan slug without a connection using disclosed code defaults", async ({
  page,
}) => {
  let discoveries = 0;
  await page.route("**/api/tenants/acme/plan-connections/chatgpt", (route) =>
    route.fulfill({ json: { enabled: true, connection: null } }),
  );
  await page.route(
    "**/api/tenants/acme/plan-connections/chatgpt/models",
    (route) => {
      discoveries++;
      return route.fulfill({
        status: 403,
        json: { error: "No personal connection" },
      });
    },
  );
  await page.goto("/registry");
  await page
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("エンティティの種類").selectOption("model");
  await dialog
    .getByLabel("プロバイダー", { exact: true })
    .selectOption("chatgpt");
  await dialog
    .getByLabel("プロバイダーのモデルID", { exact: true })
    .fill("gpt-manual-fixture");
  await expect(
    dialog.getByText(
      "コードで定義した既定値です。プロバイダが返した値・確認した値ではありません。登録前にモデルの上限を確認してください。",
    ),
  ).toBeVisible();
  await expect(dialog.getByLabel("コンテキスト上限")).toHaveValue("128000");
  await expect(dialog.getByLabel("最大出力トークン数")).toHaveValue("16384");
  await expect(dialog.getByText("未接続", { exact: true })).toBeVisible();
  await dialog
    .getByLabel("説明")
    .fill(
      "Metadata only; execution still requires an eligible personal connection",
    );
  const submitted = page.waitForRequest(
    (request) =>
      new URL(request.url()).pathname === "/api/registry" &&
      request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const config = (await submitted).postDataJSON().config;
  expect(config).toMatchObject({
    provider: "chatgpt",
    plan_connection: "chatgpt",
    model_id: "gpt-manual-fixture",
    context_window: 128000,
    max_output_tokens: 16384,
  });
  expect(config).not.toHaveProperty("provider_credential");
  expect(discoveries).toBe(0);
});
