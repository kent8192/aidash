import { expect, test } from "@playwright/test";
import { setup } from "./collaboration-fixture";

test("same-name generation policies stay distinguishable without showing IDs", async ({
  page,
}) => {
  const { errors } = await setup(page);
  const policy = (id: string) => ({
    id,
    tenant: "fixture-tenant",
    revision: 1,
    generated_count: 0,
    allocated_tokens: 0,
    allocated_compaction_calls: 0,
    allocated_embedding_calls: 0,
    spec: {
      enabled: true,
      template: { name: { en: "Shared policy" } },
      limits: { max_agents: 2, token_budget: 1000 },
    },
  });
  await page.route("**/api/generation/fixture-tenant/policies", (route) =>
    route.fulfill({ json: [policy("policy-b"), policy("policy-a")] }),
  );
  await page.goto("/generation");
  await page.locator('input[name="tenant"]').fill("fixture-tenant");
  await page.getByRole("button", { name: "Open", exact: true }).click();
  await expect(page.locator(".generation-policy h3")).toHaveText([
    "Shared policy (#2)",
    "Shared policy (#1)",
  ]);
  await expect(page.locator("body")).not.toContainText("policy-a");
  await expect(page.locator("body")).not.toContainText("policy-b");
  expect(errors).toEqual([]);
});

test("transactions with the same coordinator and deadline have distinct controls", async ({
  page,
}) => {
  const { errors } = await setup(page);
  const transaction = (id: string) => ({
    id,
    created_at: "2026-09-23T10:00:00Z",
    complete: false,
    decision: null,
    digest: "digest",
    last_error: null,
    visible: false,
    manifest: {
      id,
      coordinator: "aidash://home",
      deadline: "2026-09-23T12:00:00Z",
      isolation: "serializable",
      participants: [{ node_id: "aidash://home", mutations: [] }],
    },
  });
  await page.route("**/api/transactions", (route) =>
    route.fulfill({
      json: [transaction("transaction-b"), transaction("transaction-a")],
    }),
  );
  await page.goto("/transactions");
  const controls = page.locator("button.transaction-id");
  await expect(controls).toHaveCount(2);
  await expect(controls.nth(0)).toContainText("(#2)");
  await expect(controls.nth(1)).toContainText("(#1)");
  await expect(controls.nth(0)).not.toContainText("transaction-b");
  await expect(controls.nth(1)).not.toContainText("transaction-a");
  expect(errors).toEqual([]);
});

test("transaction composer keeps the coordinator and uses peer references separately", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.route("**/api/state", (route) =>
    route.fulfill({
      json: {
        access: { kind: "operator" },
        node: {
          id: "aidash://home",
          endpoint: "http://localhost",
          protocol_version: "0.1",
          capabilities: [],
          clusters: [],
        },
        peers: [
          { node_id: "aidash://peer", endpoint: "http://peer", enabled: true },
        ],
        registry: [],
        conversations: [],
        events: [],
        artifacts: [],
        human_requests: [],
        installations: [],
        workspaces: [{ id: "local-workspace", title: "Shared", revision: 3 }],
        tasks: [
          {
            id: "task-a",
            workspace_id: "local-workspace",
            title: "Review",
            revision: 1,
          },
          {
            id: "task-b",
            workspace_id: "local-workspace",
            title: "Review",
            revision: 2,
          },
        ],
        runs: [],
      },
    }),
  );
  await page.route("**/api/transactions", (route) =>
    route.fulfill({ json: [] }),
  );
  await page.goto("/transactions");
  await page.getByRole("button", { name: "Create transaction" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByLabel("Node", { exact: true })).toBeDisabled();
  await dialog
    .getByLabel("Operation", { exact: true })
    .selectOption("complete_task");
  await expect(
    dialog.getByLabel("Task", { exact: true }).locator("option"),
  ).toHaveText(["Choose…", "Shared / Review (#1)", "Shared / Review (#2)"]);
  await dialog
    .getByLabel("Operation", { exact: true })
    .selectOption("workspace_state");
  await dialog.getByRole("button", { name: "Add participant" }).click();
  await expect(
    dialog.getByRole("button", { name: "Delete", exact: true }).last(),
  ).toBeEnabled();
  await expect(
    dialog.getByLabel("Node", { exact: true }).first(),
  ).toBeDisabled();
  await dialog.getByRole("button", { name: "Add operation" }).last().click();
  await expect(
    dialog.getByLabel("Workspace", { exact: true }).first(),
  ).toHaveJSProperty("tagName", "SELECT");
  await expect(
    dialog.getByLabel("Workspace", { exact: true }).last(),
  ).toHaveJSProperty("tagName", "INPUT");
  await dialog
    .getByLabel("Workspace", { exact: true })
    .last()
    .fill("peer-workspace");
  await dialog
    .getByLabel("Operation", { exact: true })
    .last()
    .selectOption("complete_task");
  const choices = dialog.getByLabel("Task", { exact: true }).last();
  await expect(choices).toHaveJSProperty("tagName", "INPUT");
  await dialog
    .getByLabel("Operation", { exact: true })
    .last()
    .selectOption("finish_run");
  await expect(
    dialog.getByLabel("Execution", { exact: true }),
  ).toHaveJSProperty("tagName", "INPUT");
  expect(errors).toEqual([]);
});

test("semantic sources can page older messages and select remote agents", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.route("**/api/discover", (route) =>
    route.fulfill({
      json: {
        agents: [
          {
            node_id: "aidash://peer",
            entity: {
              id: "remote-analyst",
              version: "1.0.0",
              kind: "agent",
              name: { en: "Remote analyst" },
              description: {},
              config: {},
              schema: {},
              capabilities: [],
              languages: [],
              tags: [],
              skills: [],
            },
          },
        ],
        errors: [],
      },
    }),
  );
  await page.route("**/api/workspaces/workspace-one/semantic/index", (route) =>
    route.fulfill({
      json: {
        revision: 1,
        spec: {
          enabled: true,
          auto_context: false,
          embedding: { model: "Fixture model", model_version: "1.0.0" },
          max_results: 10,
          max_result_tokens: 100,
          max_sources: 10,
        },
      },
    }),
  );
  await page.route(
    "**/api/workspaces/workspace-one/semantic/entries",
    (route) => route.fulfill({ json: [] }),
  );
  await page.route(
    "**/api/workspaces/workspace-one/semantic/history",
    (route) => route.fulfill({ json: [] }),
  );
  await page.route(
    "**/api/workspaces/workspace-one/semantic/cleanup",
    (route) =>
      route.fulfill({
        json: {
          points: { pending: 0, failed: 0 },
          collections: { pending: 0, failed: 0 },
        },
      }),
  );
  await page.route(
    "**/api/workspaces/workspace-one/message-history**",
    (route) => {
      const older = new URL(route.request().url()).searchParams.has("before");
      return route.fulfill({
        json: {
          messages: [
            {
              message: {
                id: older ? "older-message" : "recent-message",
                content: older ? "Older evidence" : "Recent evidence",
                created_at: "2026-09-23T10:00:00Z",
              },
              attachments: [],
              thread_id: null,
              is_thread_root: false,
            },
          ],
          next_before: older ? null : "cursor",
        },
      });
    },
  );
  await page.goto("/semantic");
  await page.getByRole("button", { name: "Add or edit source" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Source type").selectOption("message");
  await dialog.getByRole("button", { name: "Load older messages" }).click();
  await dialog
    .getByLabel("Source", { exact: true })
    .selectOption("older-message");
  await expect(dialog.getByLabel("Source", { exact: true })).toHaveValue(
    "older-message",
  );
  await dialog
    .getByLabel("Agent scope (optional)")
    .selectOption("aidash://peer/agents/remote-analyst@1.0.0");
  await expect(dialog.getByLabel("Agent scope (optional)")).toHaveValue(
    "aidash://peer/agents/remote-analyst@1.0.0",
  );
  await expect(dialog).toContainText("Remote analyst · 1.0.0");
  expect(errors).toEqual([]);
});
