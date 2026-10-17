import { expect, type Page, test } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import {
  registryVersion,
  routeRegistryVersions,
  routeTenantActions,
  tenantSubject,
} from "./registry-fixture";

const entry = (id: string, kind: string, config: Record<string, unknown>) => ({
  id,
  version: "1.0.0",
  kind,
  name: { en: id },
  description: { en: "Approval fixture" },
  capabilities: [],
  languages: ["en"],
  skills: [],
  tags: [],
  schema: {},
  config,
});
const approvedKeyless = entry("approved-keyless-model", "model", {
  provider: "openrouter",
  model_id: "fixture",
  endpoint: "http://ollama.lan:11434/v1",
});
const pendingKeyless = entry("pending-keyless-embedding", "embedding", {
  provider: "openai",
  endpoint: "https://embeddings.example/v1",
  model: "fixture",
  model_version: "1",
  dimensions: 3,
});
const pendingCatalog = entry("pending-catalog-model", "model", {
  provider: "openrouter",
  model_id: "fixture",
  endpoint: "https://openrouter.ai/api/v1",
  provider_credential: "openrouter",
});
const pending = { approval: null, history: [] };

/** A Tenant subject's Registry; `/api/state` lists only approved versions. */
async function registry(page: Page, actions: string[]) {
  const approved = new Set([approvedKeyless.id]);
  const versions = () =>
    [approvedKeyless, pendingKeyless, pendingCatalog].map((value) =>
      registryVersion(value, approved.has(value.id) ? {} : pending),
    );
  const decisions: Record<string, unknown>[] = [];
  await installBearerDashboard(page, "fixture");
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({
        json: { access: tenantSubject, node_id: "aidash://node" },
      });
    if (path === "/api/mesh")
      return route.fulfill({ json: { nodes: [], errors: [] } });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access: tenantSubject,
          node: {
            id: "aidash://node",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            capabilities: [],
            clusters: [],
          },
          registry: versions()
            .filter((version) => version.approval?.enabled)
            .map((version) => version.entry),
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
    if (path === "/api/authorization/acme/catalog") {
      const body = route.request().postDataJSON() as {
        entry: { id: string };
      };
      decisions.push(body);
      approved.add(body.entry.id);
      return route.fulfill({ json: {} });
    }
    return route.fulfill({ json: [] });
  });
  await routeRegistryVersions(page, versions);
  await routeTenantActions(page, actions);
  await page.goto("/settings?view=registry");
  return decisions;
}

const open = (page: Page, name: string) =>
  page
    .locator(".entity-card")
    .filter({ has: page.getByRole("heading", { name, exact: true }) })
    .click();
const approval = (page: Page) =>
  page.getByRole("dialog").getByRole("region", { name: "Catalog Approval" });

test("Catalog Approval says that a Keyless definition carries no data retention guarantee", async ({
  page,
}) => {
  await registry(page, ["catalog.approve"]);

  // The Registry list shows where a Keyless definition sends requests.
  await expect(
    page
      .locator(".entity-card")
      .filter({ hasText: "approved-keyless-model" })
      .locator(".keyless-origin"),
  ).toHaveText("No retention guarantee · http://ollama.lan:11434");

  // Reviewing a Keyless definition to approve warns before the decision,
  // and the warning is announced, not only drawn.
  await open(page, "pending-keyless-embedding");
  const notice = approval(page).getByRole("status").filter({
    hasText: "no zero-data-retention guarantee",
  });
  await expect(notice).toHaveClass(/keyless-notice/);
  await expect(notice).toContainText("https://embeddings.example");
  await page.keyboard.press("Escape");

  // A Provider Catalog definition carries no such notice.
  await open(page, "pending-catalog-model");
  await expect(approval(page)).toBeVisible();
  await expect(approval(page).locator(".keyless-notice")).toHaveCount(0);
  await expect(approval(page).locator(".keyless-origin")).toHaveCount(0);
});

test("an approved Keyless definition keeps saying where it sends requests", async ({
  page,
}) => {
  const decisions = await registry(page, ["catalog.approve"]);
  await open(page, "pending-keyless-embedding");
  await expect(approval(page).locator(".keyless-notice")).toHaveCount(1);
  await approval(page)
    .getByRole("button", { name: "Approve", exact: true })
    .click();
  await expect(approval(page).locator(".keyless-origin")).toHaveText(
    "No retention guarantee · https://embeddings.example",
  );
  await expect(approval(page).locator(".keyless-notice")).toHaveCount(0);
  expect(decisions).toEqual([
    {
      entry: { id: "pending-keyless-embedding", version: "1.0.0" },
      enabled: true,
      expected_revision: 0,
    },
  ]);
});

test("a member who cannot approve still sees the Keyless destination", async ({
  page,
}) => {
  await registry(page, []);
  await open(page, "pending-keyless-embedding");
  await expect(approval(page).locator(".keyless-origin")).toHaveText(
    "No retention guarantee · https://embeddings.example",
  );
  await expect(approval(page).locator(".keyless-notice")).toHaveCount(0);
  await expect(approval(page).getByRole("button")).toHaveCount(0);
});
