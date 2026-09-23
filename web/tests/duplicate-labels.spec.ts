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
