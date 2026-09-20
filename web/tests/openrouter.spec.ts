import { test, expect } from "@playwright/test";

test("registers an OpenRouter model with an editable endpoint and secret reference", async ({
  page,
}) => {
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
    } else if (path === "/api/state") {
      await route.fulfill({
        json: {
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
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("エンティティの種類").selectOption("model");
  await dialog.locator('[name="provider"]').selectOption("openrouter");
  await expect(dialog.locator('[name="endpoint"]')).toHaveValue(
    "https://openrouter.ai/api/v1",
  );
  await expect(dialog.locator('[name="credential_env"]')).toHaveAttribute(
    "placeholder",
    "AIDASH_SECRET_OPENROUTER",
  );
  await dialog.getByLabel("エンティティID").fill("openrouter-model");
  await dialog.getByLabel("名前 · English").fill("OpenRouter test model");
  await dialog.getByLabel("説明 · English").fill("A model for agent execution");
  await dialog.locator('[name="model_id"]').fill("vendor/fixture-model");
  await dialog
    .locator('[name="credential_env"]')
    .fill("AIDASH_SECRET_OPENROUTER");
  await dialog
    .locator('[name="endpoint"]')
    .fill("https://gateway.example/api/v1");
  const submitted = page.waitForRequest(
    (request) =>
      new URL(request.url()).pathname === "/api/registry" &&
      request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  expect((await submitted).postDataJSON().config).toMatchObject({
    provider: "openrouter",
    model_id: "vendor/fixture-model",
    endpoint: "https://gateway.example/api/v1",
    credential_env: "AIDASH_SECRET_OPENROUTER",
  });
  await expect(dialog).not.toBeVisible();
  expect(errors).toEqual([]);
});
