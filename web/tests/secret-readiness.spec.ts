import { expect, test, type Page } from "@playwright/test";
import { setup } from "./collaboration-fixture";

const facts = (
  repair: "manage_secret" | "ask_administrator" = "manage_secret",
) => [
  {
    item: "crm@1.0.0",
    kind: "tool",
    tenant_secret_name: "crm-key",
    status: "ready",
    repair: "none",
  },
  {
    item: "images@1.0.0",
    kind: "external_service",
    tenant_secret_name: "image-key",
    status: "secret_missing",
    repair,
  },
  {
    item: "sync@1.0.0",
    kind: "tool",
    tenant_secret_name: "sync-key",
    status: "secret_hosts_mismatch",
    repair,
  },
];

function readinessPanel(page: Page) {
  return page.getByRole("region", {
    name: "Tenant Secret readiness",
    exact: true,
  });
}

async function routeAgents(page: Page) {
  const agent = (version: string) => ({
    id: "researcher",
    version,
    kind: "agent",
    name: { en: "Researcher" },
    description: { en: "Readiness fixture" },
    config: {
      schema_version: 1,
      bindings: [],
      remove_default: [],
      model: { id: "model", version: "1.0.0" },
    },
  });
  await page.route("**/api/registry**", (route) => {
    const url = new URL(route.request().url());
    if (
      url.pathname === "/api/registry" &&
      url.searchParams.get("kind") === "agent"
    )
      return route.fulfill({ json: [agent("1.0.0"), agent("1.1.0")] });
    return route.fallback();
  });
}

test("the Agent page shows ready, missing and administrator-repair readiness", async ({
  page,
}) => {
  const { errors } = await setup(page, {
    subject: true,
    coreCapabilities: true,
    coreVersion: true,
  });
  await routeAgents(page);
  await page.route(
    "**/api/registry/researcher/1.0.0/secret-readiness",
    (route) => route.fulfill({ json: facts() }),
  );
  await page.goto("/settings?view=workingFiles");
  const config = page.locator("details", {
    has: page.locator("summary", {
      hasText: "Agent capabilities, Skills and original references",
    }),
  });
  await config.locator("summary").click();
  await config
    .getByLabel("Source Agent version")
    .selectOption("researcher@1.0.0");
  const panel = readinessPanel(page);
  await expect(panel).toBeVisible();
  await expect(panel.getByText("crm@1.0.0", { exact: true })).toBeVisible();
  await expect(panel.getByText("Ready", { exact: true })).toBeVisible();
  await expect(
    panel.getByText("Tenant Secret missing", { exact: true }),
  ).toBeVisible();
  await expect(
    panel.getByText("Hosts do not permit this destination", { exact: true }),
  ).toBeVisible();
  await expect(
    panel.getByText("Tenant Secret · image-key", { exact: false }),
  ).toBeVisible();
  const manage = panel.getByRole("link", {
    name: "Manage Tenant Secrets",
    exact: true,
  });
  await expect(manage).toHaveCount(2);
  for (const link of await manage.all())
    await expect(link).toHaveAttribute("href", "/trust?view=authorization");
  expect(errors).toEqual([]);
});

test("readiness without secret.manage asks an administrator to repair it", async ({
  page,
}) => {
  const { errors } = await setup(page, {
    subject: true,
    coreCapabilities: true,
    coreVersion: true,
  });
  await routeAgents(page);
  await page.route(
    "**/api/registry/researcher/1.0.0/secret-readiness",
    (route) => route.fulfill({ json: facts("ask_administrator") }),
  );
  await page.goto("/settings?view=workingFiles");
  const config = page.locator("details", {
    has: page.locator("summary", {
      hasText: "Agent capabilities, Skills and original references",
    }),
  });
  await config.locator("summary").click();
  await config
    .getByLabel("Source Agent version")
    .selectOption("researcher@1.0.0");
  const panel = readinessPanel(page);
  await expect(
    panel.getByText(
      "Ask an administrator who can manage this Tenant's secrets to fix it.",
      { exact: true },
    ),
  ).toHaveCount(2);
  await expect(
    panel.getByRole("link", { name: "Manage Tenant Secrets", exact: true }),
  ).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("the Marketplace install plan shows Secret slot readiness", async ({
  page,
}) => {
  await setup(page, { subject: true });
  const entry = {
    id: "source",
    version: "1.0.0",
    kind: "tool",
    name: { en: "Image sync" },
    description: { en: "Calls an image service" },
    capabilities: [],
    tags: [],
    languages: ["en"],
    skills: [],
    schema: {},
    config: {
      transport: "http",
      endpoint: "https://api.example.test/v1/",
      secret: "image-key",
    },
  };
  const summary = {
    key: "package-a",
    repository: "aidash://home",
    owner_tenant: "acme",
    package_id: "source",
    version: "1.0.0",
    kind: "tool",
    name: entry.name,
    description: entry.description,
    author: "Author",
    capabilities: [],
    permissions: [],
    languages: ["en"],
    digest: "sha256:source",
    actions: ["read", "install"],
  };
  await page.route("**/api/marketplace/packages**", (route) => {
    const url = new URL(route.request().url());
    if (url.pathname === "/api/marketplace/packages")
      return route.fulfill({ json: [summary] });
    return route.fallback();
  });
  await page.route("**/api/marketplace/packages/package-a", (route) =>
    route.fulfill({
      json: {
        summary,
        manifest: {
          entity: entry,
          author: "Author",
          permissions: [],
          dependencies: [],
          secrets: ["image-key"],
        },
        audience: { revision: 1, tenants: [] },
      },
    }),
  );
  await page.route(
    "**/api/marketplace/packages/package-a/secret-readiness",
    (route) =>
      route.fulfill({
        json: [
          {
            item: "image-key",
            kind: "slot",
            status: "secret_missing",
            repair: "manage_secret",
          },
          {
            item: "search-key",
            kind: "slot",
            status: "secret_hosts_mismatch",
            repair: "ask_administrator",
          },
        ],
      }),
  );
  await page.route("**/api/tenants/acme/secrets", (route) =>
    route.fulfill({
      json: [
        {
          name: "image-key",
          hosts: ["api.example.test"],
          fingerprint: "f".repeat(16),
          last4: "1234",
          revision: 2,
        },
      ],
    }),
  );
  await page.goto("/settings?view=marketplace");
  await page.getByRole("button", { name: "Details", exact: true }).click();
  await page
    .getByRole("button", { name: "Review and install", exact: true })
    .click();
  const dialog = page.getByRole("dialog", {
    name: "Install for this tenant",
    exact: true,
  });
  const panel = dialog.getByRole("region", {
    name: "Tenant Secret readiness",
    exact: true,
  });
  await expect(panel).toBeVisible();
  await expect(panel.getByText("image-key", { exact: true })).toBeVisible();
  await expect(
    panel.getByText("Tenant Secret missing", { exact: true }),
  ).toBeVisible();
  await expect(
    panel.getByText("Hosts do not permit this destination", { exact: true }),
  ).toBeVisible();
  await expect(
    panel.getByRole("link", { name: "Manage Tenant Secrets", exact: true }),
  ).toHaveAttribute("href", "/trust?view=authorization");
  await expect(
    panel.getByText(
      "Ask an administrator who can manage this Tenant's secrets to fix it.",
      { exact: true },
    ),
  ).toBeVisible();
});
