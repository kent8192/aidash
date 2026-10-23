import { expect, type Page, test } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import {
  registryVersion,
  routeRegistryVersions,
  routeTenantActions,
  tenantSubject,
} from "./registry-fixture";

const entry = (
  id: string,
  kind: string,
  config: Record<string, unknown>,
  name = id,
) => ({
  id,
  version: "1.0.0",
  kind,
  name: { en: name },
  description: { en: "Agent Plugin fixture" },
  capabilities: [],
  languages: ["en"],
  skills: [],
  tags: [],
  schema: {},
  config,
});
const model = entry("fixture-model", "model", {}, "Fixture model");

const digest = (character: string) => `sha256:${character.repeat(64)}`;
const reference = {
  digest: digest("a"),
  name: "review-kit",
  source: "https://github.com/acme/kits/tree/0123456789abcdef/plugins/review",
  version: "1.4.0",
  license: "MIT",
  skills: ["code-review"],
  servers: {
    index: {
      stdio: {
        command: { plugin_relative: "bin/index" },
        args: ["--root", "${PLUGIN_DATA}"],
        env: { LEVEL: "debug" },
        cwd: { data: "cache" },
      },
    },
    search: {
      remote: {
        url: "https://mcp.example.com/search",
        headers: [{ name: "Authorization", value: "Bearer sk-live-secret" }],
      },
    },
  },
  findings: [
    {
      component: { server: "legacy" },
      outcome: "skipped",
      reason: "unsupported_transport",
      detail: "sse is not supported",
    },
    {
      component: "manifest",
      outcome: "ignored",
      reason: "unknown_field",
      detail: "homepage",
    },
  ],
};
const otherReference = {
  ...reference,
  digest: digest("b"),
  name: "docs-kit",
  skills: [],
  servers: {},
  findings: [],
};

async function dashboard(page: Page, registry: unknown[]) {
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
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access: tenantSubject,
          deployment: {
            id: "aidash://node",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            key: null,
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
        },
      });
    if (path === "/api/registry" && route.request().method() === "POST")
      return route.fulfill({ json: route.request().postDataJSON() });
    return route.fulfill({ json: [] });
  });
}

test.describe("Agent form", () => {
  const imports: Record<string, unknown>[] = [];
  test.beforeEach(async ({ page }) => {
    imports.length = 0;
    await dashboard(page, [model]);
    await page.route("**/api/agent-plugins/import", (route) => {
      const body = route.request().postDataJSON() as Record<string, unknown>;
      imports.push(body);
      const roots = ["plugins/docs", "plugins/review"];
      if (!body.plugin_path)
        return route.fulfill({ json: { plugins: roots, selected: null } });
      const chosen = body.plugin_path === "plugins/review";
      return route.fulfill({
        json: {
          plugins: [body.plugin_path],
          selected: {
            reference: chosen ? reference : otherReference,
            denied: chosen ? ["links/outside"] : [],
          },
        },
      });
    });
    await routeTenantActions(page, ["registry.create"]);
    await routeRegistryVersions(page, () => []);
    await page.goto("/registry");
    await page
      .getByRole("button", { name: "Register entity", exact: true })
      .click();
    const dialog = page.getByRole("dialog");
    await dialog
      .getByLabel("Description", { exact: true })
      .fill("Agent with plugins");
    await dialog.getByLabel("Model", { exact: true }).selectOption({
      label: "Fixture model · 1.0.0",
    });
  });

  const register = async (page: Page) => {
    const posted = page.waitForRequest(
      (request) =>
        request.url().endsWith("/api/registry") && request.method() === "POST",
    );
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "Register entity", exact: true })
      .click();
    return (await posted).postDataJSON();
  };

  test("an Agent without plugins submits no agent_plugins key", async ({
    page,
  }) => {
    await page
      .getByRole("dialog")
      .getByLabel("Additional instructions")
      .fill("Answer briefly.");
    const body = await register(page);
    expect(body.config).not.toHaveProperty("agent_plugins");
  });

  test("an imported plugin is reviewed, added and registered with the Agent", async ({
    page,
  }) => {
    const dialog = page.getByRole("dialog");
    const importer = dialog.getByRole("group", {
      name: "Import an Agent Plugin",
    });
    await expect(importer).toContainText("mcp_stdio");
    await expect(importer).toContainText("Tool call approval");

    await importer
      .getByLabel("Plugin source URL")
      .fill("https://github.com/acme/kits");
    await importer.getByRole("button", { name: "Find plugin at URL" }).click();
    await importer
      .getByLabel("Plugin in repository")
      .selectOption("plugins/review");
    await importer
      .getByRole("button", { name: "Import selected plugin" })
      .click();
    expect(imports).toEqual([
      { url: "https://github.com/acme/kits" },
      { url: "https://github.com/acme/kits", plugin_path: "plugins/review" },
    ]);

    await expect(importer).toContainText("review-kit");
    await expect(importer).toContainText("MIT");
    await expect(importer).toContainText("code-review");
    await expect(importer).toContainText("./bin/index");
    await expect(importer).toContainText("${PLUGIN_DATA}/cache");
    await expect(importer).toContainText("LEVEL=debug");
    await expect(importer).toContainText("https://mcp.example.com/search");
    await expect(importer).toContainText("Authorization");
    await expect(importer).not.toContainText("sk-live-secret");
    await expect(importer).toContainText("Skipped (1)");
    await expect(importer).toContainText("unsupported_transport");
    await expect(importer).toContainText("Ignored (1)");
    await expect(importer).toContainText("links/outside");
    await expect(
      importer.getByRole("link", { name: /github\.com\/acme\/kits/ }),
    ).toHaveAttribute("href", reference.source);

    await importer.getByRole("button", { name: "Add to Agent" }).click();
    await expect(
      importer.getByText("Plugins in this Agent (1/8)"),
    ).toBeVisible();
    await expect(
      importer.getByRole("button", { name: "Add to Agent" }),
    ).toHaveCount(0);

    const body = await register(page);
    expect(body.config.agent_plugins).toEqual([reference]);
  });

  test("a removed plugin leaves no agent_plugins key and the Agent needs instructions again", async ({
    page,
  }) => {
    const dialog = page.getByRole("dialog");
    const importer = dialog.getByRole("group", {
      name: "Import an Agent Plugin",
    });
    await importer
      .getByLabel("Plugin source URL")
      .fill("https://github.com/acme/kits");
    await importer.getByRole("button", { name: "Find plugin at URL" }).click();
    await importer
      .getByLabel("Plugin in repository")
      .selectOption("plugins/review");
    await importer
      .getByRole("button", { name: "Import selected plugin" })
      .click();
    await importer.getByRole("button", { name: "Add to Agent" }).click();
    await importer.getByRole("button", { name: "Remove plugin" }).click();
    await expect(importer.getByText(/Plugins in this Agent/)).toHaveCount(0);

    await dialog
      .getByRole("button", { name: "Register entity", exact: true })
      .click();
    await expect(
      page.getByText(
        "Select at least one Skill or enter additional instructions.",
      ),
    ).toBeVisible();
    await dialog.getByLabel("Additional instructions").fill("Answer briefly.");
    const body = await register(page);
    expect(body.config).not.toHaveProperty("agent_plugins");
  });

  test("a plugin without Skills does not make the Agent instructional", async ({
    page,
  }) => {
    const dialog = page.getByRole("dialog");
    const importer = dialog.getByRole("group", {
      name: "Import an Agent Plugin",
    });
    await importer
      .getByLabel("Plugin source URL")
      .fill("https://github.com/acme/kits");
    await importer.getByRole("button", { name: "Find plugin at URL" }).click();
    await importer
      .getByLabel("Plugin in repository")
      .selectOption("plugins/docs");
    await importer
      .getByRole("button", { name: "Import selected plugin" })
      .click();
    await importer.getByRole("button", { name: "Add to Agent" }).click();
    await dialog
      .getByRole("button", { name: "Register entity", exact: true })
      .click();
    await expect(
      page.getByText(
        "Select at least one Skill or enter additional instructions.",
      ),
    ).toBeVisible();
  });
});

test("Catalog Approval shows the servers an Agent version's plugins admit", async ({
  page,
}) => {
  const withPlugins = entry(
    "plugin-agent",
    "agent",
    { agent_plugins: [reference, otherReference] },
    "Plugin agent",
  );
  const plain = entry("plain-agent", "agent", {}, "Plain agent");
  await dashboard(page, []);
  await routeTenantActions(page, ["catalog.approve"]);
  await routeRegistryVersions(page, () =>
    [withPlugins, plain].map((value) =>
      registryVersion(value, { approval: null, history: [] }),
    ),
  );
  await page.goto("/settings?view=registry");

  await page
    .locator(".entity-card")
    .filter({
      has: page.getByRole("heading", { name: "Plain agent", exact: true }),
    })
    .click();
  await expect(
    page.getByRole("dialog").getByRole("region", { name: "Catalog Approval" }),
  ).toBeVisible();
  await expect(
    page.getByRole("dialog").getByRole("region", { name: "Agent Plugins" }),
  ).toHaveCount(0);
  await page.keyboard.press("Escape");

  await page
    .locator(".entity-card")
    .filter({
      has: page.getByRole("heading", { name: "Plugin agent", exact: true }),
    })
    .click();
  const plugins = page
    .getByRole("dialog")
    .getByRole("region", { name: "Agent Plugins" });
  await expect(plugins).toContainText("Agent Plugins (2)");
  await expect(plugins.locator(".agent-plugins-notice")).toContainText(
    "Approving this version admits the servers its plugins declare.",
  );
  await expect(plugins.locator(".agent-plugins-notice")).toContainText(
    "mcp_stdio",
  );
  await expect(plugins).toContainText("review-kit");
  await expect(plugins).toContainText("docs-kit");
  await expect(plugins).toContainText("./bin/index");
  await expect(plugins).toContainText("--root ${PLUGIN_DATA}");
  await expect(plugins).toContainText("LEVEL=debug");
  await expect(plugins).toContainText("https://mcp.example.com/search");
  await expect(plugins).toContainText("Authorization");
  await expect(plugins).not.toContainText("sk-live-secret");
  await expect(plugins).toContainText("sse is not supported");
});
