import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    sessionStorage.setItem("aidash-token", "fixture-token");
    localStorage.setItem("aidash-locale", "en-US");
  });
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({
        json: { access: { kind: "operator" }, node_id: "aidash://test" },
      });
    if (path === "/api/state")
      return route.fulfill({
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
            {
              id: "coordinator",
              version: "2.0.0",
              kind: "agent",
              name: { en: "Coordinator" },
              description: { en: "Coordinator" },
              config: {},
              schema: {},
              capabilities: [],
              languages: [],
              tags: [],
              skills: [],
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
          peers: [
            {
              node_id: "aidash://remote",
              endpoint: "http://remote",
              enabled: true,
            },
          ],
        },
      });
    if (path === "/api/registry" && route.request().method() === "POST")
      return route.fulfill({ json: route.request().postDataJSON() });
    if (path === "/api/mesh")
      return route.fulfill({ json: { nodes: [], errors: [] } });
    if (path === "/api/discover")
      return route.fulfill({ json: { agents: [], errors: [] } });
    return route.fulfill({ json: [] });
  });
  await page.goto("/registry");
  await page
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByLabel("Description", { exact: true })
    .fill("Fixture configuration");
});

test("skill defaults are editable and need no JSON or localized metadata", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  const original = await dialog
    .getByLabel("Name", { exact: true })
    .inputValue();
  expect(original).toMatch(/^[a-z]+-[a-z]+$/);
  await dialog.getByLabel("Entity type").selectOption("skill");
  await expect(dialog.getByLabel("Name", { exact: true })).toHaveValue(
    original,
  );
  await expect(dialog.getByLabel("Entity ID")).toHaveValue("");
  await expect(
    dialog.locator(
      '[name="name_ja"], [name="description_ja"], [name="languages"], textarea[name="config"], textarea[name="schema"]',
    ),
  ).toHaveCount(0);
  await dialog.getByLabel("Instructions").fill("Research reliable sources.");
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON()).toMatchObject({
    id: "",
    name: { en: original },
    description: { en: "Fixture configuration" },
    config: { instructions: "Research reliable sources." },
  });
  expect(Object.keys((await posted).postDataJSON().name)).toEqual(["en"]);
});

test("cluster chooses an exact agent version", async ({ page }) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("cluster");
  await dialog
    .getByLabel("Coordinator agent")
    .selectOption("coordinator@2.0.0");
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON().config).toEqual({
    coordinator: { id: "coordinator", version: "2.0.0" },
  });
});

test("native web tool uses host fields and a URL argument", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("tool");
  await dialog
    .getByLabel("Operation", { exact: true })
    .selectOption("http_get");
  await dialog
    .getByLabel("Allowed hostnames")
    .fill("example.com, docs.example.com");
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON()).toMatchObject({
    config: {
      transport: "native",
      operation: "http_get",
      allowed_hosts: ["example.com", "docs.example.com"],
    },
    schema: {
      properties: { url: { type: "string", format: "uri" } },
      required: ["url"],
    },
  });
});

test("MCP settings and nested arguments serialize without JSON input", async ({
  page,
}, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("tool");
  await dialog.getByLabel("Connection type").selectOption("mcp");
  await dialog
    .getByLabel("Endpoint", { exact: true })
    .fill("https://example.com/mcp");
  await dialog.getByLabel("MCP tool name").fill("search");
  await dialog.getByLabel("Retry behavior").selectOption("idempotent");
  await dialog.getByLabel("Request key argument").fill("request_id");
  await dialog.getByLabel("Use configured credentials").check();
  await dialog
    .getByRole("button", { name: "Add argument", exact: true })
    .click();
  await dialog.getByLabel("Argument name", { exact: true }).fill("filters");
  await dialog.getByLabel("Value type").selectOption("object");
  await dialog
    .getByRole("button", { name: "Add argument", exact: true })
    .first()
    .click();
  await dialog
    .getByLabel("Argument name", { exact: true })
    .nth(1)
    .fill("limit");
  await dialog.getByLabel("Value type").nth(1).selectOption("integer");
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: testInfo.outputPath("configuration-mobile.png"),
    fullPage: true,
  });
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON()).toMatchObject({
    config: {
      transport: "mcp",
      endpoint: "https://example.com/mcp",
      tool_name: "search",
      credential_env: "AIDASH_SECRET_TOOL",
      replay: "idempotent",
      idempotency_argument: "request_id",
    },
    schema: {
      properties: {
        filters: {
          type: "object",
          properties: { limit: { type: "integer" } },
          required: ["limit"],
        },
      },
      required: ["filters"],
    },
  });
});

test("changing tool transport excludes hidden settings", async ({ page }) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("tool");
  await dialog.getByLabel("Connection type").selectOption("mcp");
  await dialog.getByLabel("Retry behavior").selectOption("idempotent");
  await dialog.getByLabel("Connection type").selectOption("agent");
  await dialog.getByLabel("Executor agent").selectOption("coordinator@2.0.0");
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON().config).toEqual({
    transport: "agent",
    node_id: "aidash://test",
    agent: { id: "coordinator", version: "2.0.0" },
  });
});

test("HTTP tools validate duplicate argument names and support typed lists", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("tool");
  await dialog.getByLabel("Connection type").selectOption("http");
  await dialog
    .getByLabel("Endpoint", { exact: true })
    .fill("https://example.com/tool");
  await dialog
    .getByRole("button", { name: "Add argument", exact: true })
    .click();
  await dialog.getByLabel("Argument name", { exact: true }).fill("tags");
  await dialog.getByLabel("Value type").selectOption("array");
  await dialog.getByLabel("List item type").selectOption("string");
  await dialog
    .getByRole("button", { name: "Add argument", exact: true })
    .click();
  await dialog.getByLabel("Argument name", { exact: true }).nth(1).fill("tags");
  await expect(dialog.locator("input:invalid")).toHaveCount(2);
  await dialog
    .getByRole("button", { name: "Remove argument", exact: true })
    .nth(1)
    .click();
  await expect(dialog.locator("input:invalid")).toHaveCount(0);
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON()).toMatchObject({
    config: {
      transport: "http",
      endpoint: "https://example.com/tool",
      credential_env: null,
      replay: "unsafe",
    },
    schema: {
      properties: { tags: { type: "array", items: { type: "string" } } },
      required: ["tags"],
    },
  });
});

test("remote agent tools identify the peer and exact executor", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("tool");
  await dialog.getByLabel("Connection type").selectOption("agent");
  await dialog
    .getByLabel("Node", { exact: true })
    .selectOption("aidash://remote");
  await dialog
    .getByLabel("Remote agent ID", { exact: true })
    .fill("researcher");
  await dialog
    .getByLabel("Remote agent version", { exact: true })
    .fill("3.0.0");
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON().config).toEqual({
    transport: "agent",
    node_id: "aidash://remote",
    agent: { id: "researcher", version: "3.0.0" },
  });
});
