import { expect, type Page, test } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";

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
const registry = [
  entry("approved-keyless-model", "model", {
    provider: "openrouter",
    model_id: "fixture",
    endpoint: "http://ollama.lan:11434/v1",
  }),
  entry("pending-keyless-embedding", "embedding", {
    provider: "openai",
    endpoint: "https://embeddings.example/v1",
    model: "fixture",
    model_version: "1",
    dimensions: 3,
  }),
  entry("pending-catalog-model", "model", {
    provider: "openrouter",
    model_id: "fixture",
    endpoint: "https://openrouter.ai/api/v1",
    provider_credential: "openrouter",
  }),
];
const snapshot = {
  revision: 1,
  bundle: {
    tenant: "acme",
    subjects: { alice: { kind: "user" } },
    policies: [],
  },
};
const state = () => ({
  access: { kind: "operator" },
  node: {
    id: "aidash://node",
    endpoint: "http://localhost",
    protocol_version: "0.1",
    capabilities: [],
    clusters: [],
  },
  registry,
  workspaces: [],
  tasks: [],
  artifacts: [],
  events: [],
  runs: [],
  conversations: [],
  human_requests: [],
  installations: [],
  peers: [],
});
const catalogPanel = (page: Page) =>
  page.locator(".panel").filter({
    has: page.getByRole("heading", {
      name: "Approved components",
      exact: true,
    }),
  });

test("Catalog Approval says that a Keyless definition carries no data retention guarantee", async ({
  page,
}) => {
  await installBearerDashboard(page, "fixture");
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({
        json: { access: { kind: "operator" }, node_id: "aidash://node" },
      });
    if (path === "/api/mesh")
      return route.fulfill({ json: { nodes: [], errors: [] } });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access: { kind: "operator" },
          node: {
            id: "aidash://node",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            capabilities: [],
            clusters: [],
          },
          registry,
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
    if (path === "/api/authorization/acme")
      return route.fulfill({
        json: {
          revision: 1,
          bundle: {
            tenant: "acme",
            subjects: { alice: { kind: "user" } },
            policies: [],
          },
        },
      });
    if (path === "/api/authorization/acme/catalog")
      return route.fulfill({
        json: [
          {
            tenant: "acme",
            entry_id: "approved-keyless-model",
            entry_version: "1.0.0",
            enabled: true,
            revision: 1,
          },
        ],
      });
    return route.fulfill({ json: [] });
  });
  // `/authorization` would reach Vite's `/auth` proxy instead of the dashboard.
  await page.goto("/trust?view=authorization");
  await page.getByLabel("Tenant", { exact: true }).fill("acme");
  await page.getByRole("button", { name: "Open", exact: true }).click();
  const catalog = page.locator(".panel").filter({
    has: page.getByRole("heading", {
      name: "Approved components",
      exact: true,
    }),
  });

  // The approved Keyless model shows where it sends requests.
  await expect(catalog.locator(".auth-row .keyless-origin")).toHaveText(
    "No retention guarantee · http://ollama.lan:11434",
  );

  // Choosing a Keyless definition to approve warns before the decision.
  const version = page.getByLabel("Component version", { exact: true });
  await version.selectOption(
    JSON.stringify(["pending-keyless-embedding", "1.0.0"]),
  );
  await expect(catalog.locator(".keyless-notice")).toContainText(
    "no zero-data-retention guarantee",
  );
  await expect(catalog.locator(".keyless-notice")).toContainText(
    "https://embeddings.example",
  );

  // A Provider Catalog definition carries no such notice.
  await version.selectOption(
    JSON.stringify(["pending-catalog-model", "1.0.0"]),
  );
  await expect(catalog.locator(".keyless-notice")).toHaveCount(0);

  // The pre-approval warning is announced, not only drawn.
  await version.selectOption(
    JSON.stringify(["pending-keyless-embedding", "1.0.0"]),
  );
  await expect(catalog.getByRole("status")).toContainText(
    "no zero-data-retention guarantee",
  );
});

test("approving a Keyless definition resets the choice instead of showing another one", async ({
  page,
}) => {
  await installBearerDashboard(page, "fixture");
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  const approved: { entry_id: string; revision: number }[] = [];
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({
        json: { access: { kind: "operator" }, node_id: "aidash://node" },
      });
    if (path === "/api/mesh")
      return route.fulfill({ json: { nodes: [], errors: [] } });
    if (path === "/api/state") return route.fulfill({ json: state() });
    if (path === "/api/authorization/acme")
      return route.fulfill({ json: snapshot });
    if (path === "/api/authorization/acme/catalog") {
      if (route.request().method() === "POST") {
        const body = route.request().postDataJSON() as {
          entry: { id: string };
        };
        approved.push({ entry_id: body.entry.id, revision: 1 });
      }
      return route.fulfill({
        json: approved.map((binding) => ({
          tenant: "acme",
          entry_version: "1.0.0",
          enabled: true,
          ...binding,
        })),
      });
    }
    return route.fulfill({ json: [] });
  });
  await page.goto("/trust?view=authorization");
  await page.getByLabel("Tenant", { exact: true }).fill("acme");
  await page.getByRole("button", { name: "Open", exact: true }).click();
  const catalog = catalogPanel(page);
  const version = page.getByLabel("Component version", { exact: true });
  await version.selectOption(
    JSON.stringify(["pending-keyless-embedding", "1.0.0"]),
  );
  await expect(catalog.locator(".keyless-notice")).toHaveCount(1);

  await catalog.getByRole("button", { name: "Approve", exact: true }).click();

  // The approved option leaves the list; nothing else is silently chosen.
  await expect(catalog.locator(".auth-row .keyless-origin")).toContainText(
    "https://embeddings.example",
  );
  await expect(version).toHaveValue("");
  await expect(catalog.locator(".keyless-notice")).toHaveCount(0);
});

test("an approved Keyless row keeps its warning when the Registry state is unavailable", async ({
  page,
}) => {
  let registryReads = 0;
  await installBearerDashboard(page, "fixture");
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({
        json: { access: { kind: "operator" }, node_id: "aidash://node" },
      });
    if (path === "/api/mesh")
      return route.fulfill({ json: { nodes: [], errors: [] } });
    if (path === "/api/state")
      return route.fulfill({ status: 503, json: { error: "unavailable" } });
    // The definition read fails once during the same outage, then recovers.
    if (path === "/api/registry/approved-keyless-model/1.0.0")
      return (registryReads += 1) === 1
        ? route.fulfill({ status: 503, json: { error: "unavailable" } })
        : route.fulfill({ json: registry[0] });
    if (path === "/api/authorization/acme")
      return route.fulfill({ json: snapshot });
    if (path === "/api/authorization/acme/catalog")
      return route.fulfill({
        json: [
          {
            tenant: "acme",
            entry_id: "approved-keyless-model",
            entry_version: "1.0.0",
            enabled: true,
            revision: 1,
          },
        ],
      });
    return route.fulfill({ json: [] });
  });
  await page.goto("/trust?view=authorization");
  await page.getByLabel("Tenant", { exact: true }).fill("acme");
  await page.getByRole("button", { name: "Open", exact: true }).click();

  await expect(
    catalogPanel(page).locator(".auth-row .keyless-origin"),
  ).toHaveText("No retention guarantee · http://ollama.lan:11434", {
    timeout: 15000,
  });
  expect(registryReads).toBeGreaterThan(1);
});
