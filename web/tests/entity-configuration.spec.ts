import { test, expect } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import {
  routeRegistryVersions,
  routeTenantActions,
  tenantSubject,
} from "./registry-fixture";

test.beforeEach(async ({ page }) => {
  await installBearerDashboard(page, "fixture-token");
  await page.addInitScript(() => {
    localStorage.setItem("aidash-locale", "en-US");
  });
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({
        json: {
          access: tenantSubject,
          node_id: "aidash://test",
        },
      });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access: tenantSubject,
          deployment: {
            id: "aidash://test",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            key: null,
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
            ...["1.0.0", "2.0.0"].map((version) => ({
              id: "shared-tool",
              version,
              kind: "tool",
              name: { en: "Shared tool" },
              description: { en: "Fixture" },
              config: {},
              schema: {},
              capabilities: [],
              languages: [],
              tags: [],
              skills: [],
            })),
          ],
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
  await routeTenantActions(page, ["registry.create"], {
    deployment: "aidash://test",
  });
  await routeRegistryVersions(page, () => []);
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
  await dialog.getByLabel("Entity type").selectOption("skill");
  const original = await dialog
    .getByLabel("Name", { exact: true })
    .inputValue();
  expect(original).toMatch(/^[a-z]+-[a-z]+$/);
  await dialog.getByLabel("Entity type").selectOption("skill");
  await expect(dialog.getByLabel("Name", { exact: true })).toHaveValue(
    original,
  );
  await expect(dialog.getByLabel("Entity ID")).toHaveCount(0);
  await expect(dialog.getByText(/UUID/i)).toHaveCount(0);
  await expect(
    dialog.locator(
      '[name="name_ja"], [name="description_ja"], textarea[name="config"], textarea[name="schema"]',
    ),
  ).toHaveCount(0);
  await expect(dialog.getByLabel("Supported languages")).toHaveValue("ja, en");
  await dialog.getByLabel("Instructions").fill("Research reliable sources.");
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  const body = (await posted).postDataJSON();
  expect(body).toMatchObject({
    name: { en: original },
    description: { en: "Fixture configuration" },
    languages: ["ja", "en"],
    config: { instructions: "Research reliable sources." },
  });
  expect(body.id).toBe("");
  expect(Object.keys(body.name)).toEqual(["en"]);
});

test("delegates ID generation to the server on registration and retry", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("skill");
  await dialog.getByLabel("Instructions").fill("Retry safely.");
  let fail = true;
  await page.route("**/api/registry", async (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    if (fail) {
      fail = false;
      return route.fulfill({
        status: 503,
        contentType: "application/json",
        body: JSON.stringify({ error: "temporary failure" }),
      });
    }
    return route.fallback();
  });
  const first = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  const firstRequest = await first;
  const firstBody = firstRequest.postDataJSON();
  const key = firstRequest.headers()["idempotency-key"];
  expect(key).toMatch(/^[0-9a-f-]{36}$/);
  expect(firstBody.id).toBe("");
  await expect(dialog.getByLabel("Entity ID")).toHaveCount(0);
  await expect(dialog.getByText(/UUID/i)).toHaveCount(0);
  await expect(page.getByRole("alert")).toContainText("temporary failure");
  const second = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  const secondRequest = await second;
  expect(secondRequest.postDataJSON().id).toBe(firstBody.id);
  expect(secondRequest.headers()["idempotency-key"]).toBe(key);
});

test("edited registration uses a new request key", async ({ page }) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("skill");
  await dialog.getByLabel("Instructions").fill("Research.");
  await page.route("**/api/registry", (route) =>
    route.fulfill({ status: 503, json: { error: "temporary failure" } }),
  );
  const keys: string[] = [];
  for (const name of ["First skill", "Edited skill"]) {
    await dialog.getByLabel("Name", { exact: true }).fill(name);
    const posted = page.waitForRequest(
      (request) =>
        request.url().endsWith("/api/registry") && request.method() === "POST",
    );
    await dialog
      .getByRole("button", { name: "Register entity", exact: true })
      .click();
    keys.push((await posted).headers()["idempotency-key"]);
    await expect(page.getByRole("alert")).toContainText("temporary failure");
  }
  expect(keys[0]).toBeTruthy();
  expect(keys[1]).toBeTruthy();
  expect(keys[0]).not.toBe(keys[1]);
});

test("agent tool uses fixed task arguments", async ({ page }) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("tool");
  await dialog
    .getByRole("button", { name: "Add argument", exact: true })
    .click();
  await dialog.getByLabel("Argument name", { exact: true }).fill("unrelated");
  await dialog.getByLabel("Connection type").selectOption("agent");
  await expect(
    dialog.getByRole("button", { name: "Add argument", exact: true }),
  ).toHaveCount(0);
  await dialog.getByLabel("Executor agent").selectOption("coordinator@2.0.0");
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON().schema).toEqual({
    type: "object",
    properties: {
      title: { type: "string" },
      description: { type: "string" },
      requirements: { type: "object", additionalProperties: true },
      dependencies: {
        type: "array",
        items: { type: "string", format: "uuid" },
      },
      parent_id: { type: ["string", "null"], format: "uuid" },
    },
    required: ["title", "description"],
    additionalProperties: false,
  });
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

test("context sources are explicit immutable definitions", async ({ page }) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("memory");
  await dialog.getByLabel("Context source").selectOption("conversation_memory");
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON().config).toEqual({
    schema_version: 1,
    source: { adapter: "conversation_memory" },
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
  await dialog.getByLabel("What a call does").selectOption("unsafe");
  await expect(dialog.getByLabel("Request key argument")).toHaveCount(0);
  await expect(
    dialog.getByLabel("What a call does").locator("option"),
  ).toHaveText([
    "Reads only (needs no approval)",
    "May change things (asks for approval)",
  ]);
  // Tool transports never name a Deployment secret (#296).
  await expect(dialog.getByLabel("Use configured credentials")).toHaveCount(0);
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
      registry_deployment: "aidash://test",
      provider: "integration.mcp@1",
      operation: "invoke",
      default_alias: "integration_invoke",
      tier: "integration",
      narrow: {},
      transport: {
        transport: "mcp",
        endpoint: "https://example.com/mcp",
        tool_name: "search",
        replay: "unsafe",
        idempotency_argument: null,
      },
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

test("a replay choice made while an MCP hint is pending wins over a late answer", async ({
  page,
}) => {
  let release!: () => void;
  const answered = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/api/registry/mcp-tool-hint", async (route) => {
    await answered;
    return route.fulfill({ json: { read_only_hint: true } });
  });
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("tool");
  await dialog.getByLabel("Connection type").selectOption("mcp");
  await dialog
    .getByLabel("Endpoint", { exact: true })
    .fill("https://example.com/mcp");
  await dialog.getByLabel("MCP tool name").fill("search");
  const replay = dialog.getByLabel("What a call does");
  await replay.selectOption("read_only");
  await dialog
    .getByRole("button", { name: "Check the server's hint", exact: true })
    .click();
  // The person's explicit unsafe choice arrives while the request is pending.
  await replay.selectOption("unsafe");
  release();
  await expect(replay).toHaveValue("unsafe");
  await expect(
    dialog.getByText(/The server hints that this tool only reads/),
  ).toHaveCount(0);
});

test("an MCP server's read-only hint prefills the choice and never decides it", async ({
  page,
}) => {
  const asked: unknown[] = [];
  let answer: { status: number; json: unknown } = {
    status: 200,
    json: { read_only_hint: true },
  };
  await page.route("**/api/registry/mcp-tool-hint", (route) => {
    asked.push(route.request().postDataJSON());
    return route.fulfill(answer);
  });
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("tool");
  await dialog.getByLabel("Connection type").selectOption("mcp");
  const check = dialog.getByRole("button", {
    name: "Check the server's hint",
    exact: true,
  });
  const replay = dialog.getByLabel("What a call does");
  await expect(replay).toHaveValue("unsafe");
  await expect(check).toBeDisabled();
  await dialog
    .getByLabel("Endpoint", { exact: true })
    .fill("https://example.com/mcp");
  await dialog.getByLabel("MCP tool name").fill("search");
  await check.click();
  await expect(replay).toHaveValue("read_only");
  await expect(
    dialog.getByText(/The server hints that this tool only reads/),
  ).toBeVisible();
  expect(asked).toEqual([
    { endpoint: "https://example.com/mcp", tool_name: "search" },
  ]);
  // Changing Transport also invalidates the automatic prefill: an HTTP
  // definition must not inherit an MCP server's read-only hint.
  await dialog.getByLabel("Connection type").selectOption("http");
  await expect(replay).toHaveValue("unsafe");
  await dialog.getByLabel("Connection type").selectOption("mcp");
  await expect(replay).toHaveValue("unsafe");
  // Changing the queried tool undoes the automatic prefill and hides the old
  // answer, so submitting another tool never inherits "read_only".
  await dialog.getByLabel("MCP tool name").fill("search-more");
  await expect(
    dialog.getByText(/The server hints that this tool only reads/),
  ).toHaveCount(0);
  await expect(replay).toHaveValue("unsafe");
  answer = { status: 200, json: { read_only_hint: false } };
  await check.click();
  await expect(
    dialog.getByText("The server hints that this tool may change things."),
  ).toBeVisible();
  await expect(replay).toHaveValue("unsafe");
  answer = { status: 200, json: { read_only_hint: null } };
  await replay.selectOption("read_only");
  await check.click();
  await expect(
    dialog.getByText("The server gives no hint for this tool."),
  ).toBeVisible();
  await expect(replay).toHaveValue("read_only");
  answer = {
    status: 400,
    json: { error: "the MCP server lists no tool with that name" },
  };
  await check.click();
  await expect(
    dialog.getByText("the MCP server lists no tool with that name"),
  ).toBeVisible();
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON().config.transport).toMatchObject({
    transport: "mcp",
    tool_name: "search-more",
    replay: "read_only",
  });
});

test("an HTTP tool declares reads only itself", async ({ page }) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("tool");
  await dialog.getByLabel("Connection type").selectOption("http");
  await expect(
    dialog.getByRole("button", { name: "Check the server's hint" }),
  ).toHaveCount(0);
  await dialog
    .getByLabel("Endpoint", { exact: true })
    .fill("https://example.com/tool");
  await dialog.getByLabel("What a call does").selectOption("read_only");
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON().config.transport).toEqual({
    transport: "http",
    endpoint: "https://example.com/tool",
    replay: "read_only",
  });
});

test("changing tool transport excludes hidden settings", async ({ page }) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("tool");
  await dialog.getByLabel("Connection type").selectOption("mcp");
  await dialog.getByLabel("What a call does").selectOption("unsafe");
  await dialog.getByLabel("Connection type").selectOption("agent");
  await dialog.getByLabel("Executor agent").selectOption("coordinator@2.0.0");
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON().config.transport).toEqual({
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
      registry_deployment: "aidash://test",
      provider: "integration.http@1",
      operation: "invoke",
      transport: {
        transport: "http",
        endpoint: "https://example.com/tool",
        replay: "unsafe",
      },
    },
    schema: {
      properties: { tags: { type: "array", items: { type: "string" } } },
      required: ["tags"],
    },
  });
});

test("HTTP arguments preserve JSON Schema constraints", async ({ page }) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("tool");
  await dialog.getByLabel("Connection type").selectOption("http");
  await dialog
    .getByLabel("Endpoint", { exact: true })
    .fill("https://example.com/tool");
  await dialog
    .getByRole("button", { name: "Add argument", exact: true })
    .click();
  await dialog.getByLabel("Argument name", { exact: true }).fill("status");
  await dialog
    .getByLabel("Allowed values", { exact: true })
    .fill("ready, done");
  await dialog.getByLabel("Pattern", { exact: true }).fill("^[a-z]+$");
  await dialog
    .getByRole("button", { name: "Add argument", exact: true })
    .click();
  await dialog
    .getByLabel("Argument name", { exact: true })
    .nth(1)
    .fill("limit");
  await dialog.getByLabel("Value type").nth(1).selectOption("integer");
  await dialog.getByLabel("Minimum", { exact: true }).fill("1");
  await dialog.getByLabel("Maximum", { exact: true }).fill("100");
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON().schema).toMatchObject({
    properties: {
      status: {
        type: "string",
        enum: ["ready", "done"],
        pattern: "^[a-z]+$",
      },
      limit: { type: "integer", minimum: 1, maximum: 100 },
    },
  });
});

test("bundle members allow one version per ID and reenable siblings on removal", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  await dialog
    .getByLabel("Description", { exact: true })
    .fill("Bundle fixture");
  await dialog.getByLabel("Entity type").selectOption("bundle");
  const first = dialog.getByRole("checkbox", { name: "Shared tool · 1.0.0" });
  const second = dialog.getByRole("checkbox", { name: "Shared tool · 2.0.0" });
  await first.check();
  await expect(second).toBeDisabled();
  await first.uncheck();
  await expect(second).toBeEnabled();
  await second.check();
  await expect(first).toBeDisabled();
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON().config.members).toEqual([
    {
      registry_deployment: "aidash://test",
      id: "shared-tool",
      version: "2.0.0",
    },
  ]);
});

test("changing Source adapter clears unsupported settings and prior validation", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("source");
  await dialog
    .getByLabel("Context source")
    .selectOption("reference_attachments");
  await dialog.getByLabel("Source settings (JSON)").fill('{"references":[]}');
  await dialog.getByLabel("Context source").selectOption("workspace_retrieval");
  await expect(dialog.getByLabel("Source settings (JSON)")).toHaveValue("{}");
  await dialog.getByLabel("Source settings (JSON)").fill("{");
  await dialog
    .getByLabel("Context source")
    .selectOption("reference_attachments");
  await dialog.getByLabel("Context source").selectOption("workspace_retrieval");
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON().config).toEqual({
    schema_version: 1,
    source: { adapter: "workspace_retrieval" },
  });
});

test("an External service form submits its destination, methods and Secret attachment", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("external_service");
  await dialog
    .getByLabel("Destination HTTPS URL")
    .fill("https://images.example.test/v1/");
  await dialog.getByRole("checkbox", { name: "POST", exact: true }).check();
  await dialog
    .getByLabel("Tenant Secret (optional)", { exact: true })
    .fill("image-key");
  await dialog
    .getByLabel("Authentication", { exact: true })
    .selectOption("header");
  await dialog.getByLabel("Header name", { exact: true }).fill("X-API-Key");
  await expect(dialog.locator('textarea[name="config"]')).toHaveCount(0);
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON()).toMatchObject({
    kind: "external_service",
    config: {
      base_url: "https://images.example.test/v1/",
      methods: ["GET", "POST"],
      secret: "image-key",
      authentication: { type: "header", name: "X-API-Key" },
    },
  });
});

test("source declarations preserve exact tools and prevent mandatory Home filtering", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("source");
  await dialog
    .getByLabel("Context source", { exact: true })
    .selectOption("workspace_retrieval");
  await dialog.getByLabel("Configure an explicit source declaration").check();
  await dialog
    .getByLabel("Provider operation: provider", { exact: true })
    .fill("workspace");
  await dialog
    .getByLabel("Provider operation", { exact: true })
    .fill("retrieve");
  await dialog.getByLabel("Pin policy").selectOption("on_demand");
  await dialog.getByLabel("Required-Home contribution").check();
  await expect(dialog.getByLabel("Pin policy")).toHaveValue("always");
  await expect(
    dialog.getByLabel("Pin policy").locator('option[value="on_demand"]'),
  ).toBeDisabled();
  await dialog.getByLabel("Pin policy").selectOption("scored");
  await expect(
    dialog.getByLabel("Selection mode").locator('option[value="enforce"]'),
  ).toBeDisabled();
  await expect(dialog.getByLabel("Selection mode")).toHaveValue("shadow");
  await dialog.getByLabel("Pin policy").selectOption("always");
  await dialog
    .getByLabel("Query policy")
    .selectOption("accepted-inputs-first@1");
  const read = dialog.getByLabel("Exact read tools");
  const version = JSON.stringify({
    registry_deployment: "aidash://test",
    id: "shared-tool",
    version: "2.0.0",
  });
  await read.selectOption(version);
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON().config.context_source).toMatchObject({
    provider: "workspace",
    operation: "retrieve",
    pin: { policy: "always" },
    tools: {
      read: [
        {
          registry_deployment: "aidash://test",
          id: "shared-tool",
          version: "2.0.0",
        },
      ],
    },
    query: "accepted-inputs-first@1",
    volatility: "step",
    required_home: true,
  });
});

test("new observation Deciders pin their own contract and remain Shadow", async ({
  page,
}) => {
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("decider");
  await dialog.getByLabel("Decision hook").selectOption("approval_observation");
  await expect(dialog.locator('[name="decider_mode"]')).toHaveValue("shadow");
  await expect(dialog.locator('[name="decider_mode"]')).toBeDisabled();
  await dialog.getByLabel("Jev Route threshold").fill("0.2");
  const posted = page.waitForRequest(
    (request) =>
      request.url().endsWith("/api/registry") && request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  expect((await posted).postDataJSON().config).toMatchObject({
    hook: "approval_observation",
    mode: "shadow",
    builder: "aidash.approval-observation/1",
    option_source: "pending_action/1",
    rule: "approval.observe/1",
    state_retention: { enabled: false },
    routes: [{ threshold: "3fc999999999999a" }],
  });
});
