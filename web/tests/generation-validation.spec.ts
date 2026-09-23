import { test, expect } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";

test("generation policy requires instructions unless a Skill is selected", async ({
  page,
}) => {
  await installBearerDashboard(page, "fixture-token");
  await page.addInitScript(() => {
    localStorage.setItem("aidash-locale", "ja-JP");
  });

  let savedPolicy: Record<string, unknown> | undefined;
  const model = {
    id: "fixture-model",
    version: "1.0.0",
    kind: "model",
    name: { en: "Fixture model", ja: "テストモデル" },
    description: { en: "Model for generation form validation" },
    capabilities: [],
    languages: [],
    tags: [],
    config: {
      provider: "openrouter",
      model_id: "fixture",
      endpoint: "https://example.invalid/v1",
      context_window: 128000,
      modalities: ["text"],
      cost: {},
    },
  };
  const skill = {
    id: "fixture-skill",
    version: "1.0.0",
    kind: "skill",
    name: { en: "Fixture skill", ja: "テストスキル" },
    description: { en: "Skill for generation form validation" },
    capabilities: [],
    languages: [],
    tags: [],
    config: { instructions: "Use the fixture skill." },
  };

  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === "/api/events/stream") {
      await route.abort();
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
          registry: [model, skill],
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
    } else if (path === "/api/authorization/fixture-tenant/catalog") {
      await route.fulfill({
        json: [model, skill].map((entry) => ({
          entry_id: entry.id,
          entry_version: entry.version,
          enabled: true,
        })),
      });
    } else if (path === "/api/generation/fixture-tenant/policies") {
      await route.fulfill({ json: [] });
    } else if (path === "/api/generation/fixture-tenant/requests") {
      await route.fulfill({ json: [] });
    } else if (
      path.startsWith("/api/generation/fixture-tenant/policies/") &&
      request.method() !== "GET"
    ) {
      savedPolicy = request.postDataJSON();
      await route.fulfill({ json: {} });
    } else {
      await route.fulfill({ json: [] });
    }
  });

  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  await page.goto("/generation");
  const tenantForm = page.locator(".generation-tenant");
  await tenantForm.locator('[name="tenant"]').fill("fixture-tenant");
  await tenantForm.getByRole("button", { name: "開く", exact: true }).click();
  await page
    .getByRole("button", { name: "生成ポリシーを作成", exact: true })
    .click();

  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("名前 · English").fill("Validation policy");
  await dialog
    .getByLabel("説明 · English")
    .fill("Exercise Skill and instruction validation");
  await dialog
    .getByLabel("モデル", { exact: true })
    .selectOption("fixture-model@1.0.0");
  await dialog
    .getByLabel("追加の指示（任意）", { exact: true })
    .fill("   \t  ");
  await dialog.getByRole("button", { name: "保存", exact: true }).click();

  await expect(dialog.getByRole("alert")).toHaveText(
    "Skillを1つ以上選択するか、追加の指示を入力してください。",
  );
  expect(savedPolicy).toBeUndefined();

  await dialog.locator('[name="skills"]').selectOption("fixture-skill@1.0.0");
  await dialog.getByLabel("追加の指示（任意）", { exact: true }).fill("");
  await dialog.getByRole("button", { name: "保存", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(savedPolicy).toMatchObject({
    spec: {
      template: {
        config: {
          instructions: "",
          skills: [{ id: "fixture-skill", version: "1.0.0" }],
        },
      },
    },
  });
  expect(pageErrors).toEqual([]);
});
