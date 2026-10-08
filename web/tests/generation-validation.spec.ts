import { test, expect } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";

for (const context of [
  { kind: "skill", adapter: "instructions", accepted: true },
  { kind: "source", adapter: "workspace_retrieval", accepted: false },
  { kind: "source", adapter: "reference_attachments", accepted: false },
  { kind: "source", adapter: "private_references", accepted: false },
  { kind: "source", adapter: "skill_attachments", accepted: true },
  { kind: "source", adapter: "skill_roots", accepted: true },
]) {
  test(`instructionless generation requires instructional context (${context.adapter})`, async ({
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
      kind: context.kind,
      name: { en: "Fixture skill", ja: "テストスキル" },
      description: { en: "Skill for generation form validation" },
      capabilities: [],
      languages: [],
      tags: [],
      config:
        context.kind === "skill"
          ? { instructions: "Use the fixture skill." }
          : { schema_version: 1, source: { adapter: context.adapter } },
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
            registry: [
              model,
              skill,
              {
                id: "fixture-cluster",
                version: "1.0.0",
                kind: "cluster",
                name: { en: "Fixture cluster" },
                config: {},
              },
            ],
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
          json: [model, skill, { id: "fixture-cluster", version: "1.0.0" }].map(
            (entry) => ({
              entry_id: entry.id,
              entry_version: entry.version,
              enabled: true,
            }),
          ),
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

    for (const name of ["skill_list", "skill_load", "skill_read"])
      await dialog.getByLabel(name, { exact: true }).uncheck();
    await dialog
      .getByLabel("追加する定義")
      .selectOption(`${context.kind}:fixture-skill@1.0.0`);
    await dialog
      .getByRole("button", { name: "Bindingを追加", exact: true })
      .click();
    for (const name of ["skill_list", "skill_load", "skill_read"]) {
      const checkbox = dialog.getByLabel(name, { exact: true });
      if (context.accepted) {
        await expect(checkbox).toBeChecked();
        await expect(checkbox).toBeDisabled();
      } else {
        await expect(checkbox).not.toBeChecked();
        await expect(checkbox).toBeEnabled();
      }
    }
    for (const name of ["task_create", "task_delegate", "agent_discover"])
      await dialog.getByLabel(name, { exact: true }).uncheck();
    await dialog
      .locator('[name="cluster"]')
      .selectOption("fixture-cluster@1.0.0");
    for (const name of ["task_create", "task_delegate", "agent_discover"]) {
      await expect(dialog.getByLabel(name, { exact: true })).toBeChecked();
      await expect(dialog.getByLabel(name, { exact: true })).toBeDisabled();
    }
    await dialog.locator('[name="cluster"]').selectOption("");
    await expect(
      dialog.getByLabel("task_delegate", { exact: true }),
    ).toBeEnabled();
    await dialog.getByLabel("追加の指示（任意）", { exact: true }).fill("");
    await dialog.getByRole("button", { name: "保存", exact: true }).click();
    if (!context.accepted) {
      await expect(dialog.getByRole("alert")).toHaveText(
        "Skillを1つ以上選択するか、追加の指示を入力してください。",
      );
      expect(savedPolicy).toBeUndefined();
      await dialog
        .getByLabel("追加の指示（任意）", { exact: true })
        .fill("Use this context.");
      await dialog.getByRole("button", { name: "保存", exact: true }).click();
    }
    await expect(dialog).toHaveCount(0);
    expect(savedPolicy).toMatchObject({
      spec: {
        template: {
          config: {
            instructions: context.accepted ? "" : "Use this context.",
            schema_version: 1,
            remove_default: context.accepted
              ? []
              : ["skill_list", "skill_load", "skill_read"],
            bindings: [
              {
                kind: context.kind,
                target: {
                  registry_node: "aidash://test",
                  id: "fixture-skill",
                  version: "1.0.0",
                },
                narrow: {},
              },
            ],
          },
        },
      },
    });
    expect(pageErrors).toEqual([]);
  });
}
