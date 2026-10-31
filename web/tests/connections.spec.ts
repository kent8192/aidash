import { expect, test, type Page } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import {
  builtInApprovalDecider,
  routeRegistryVersions,
  routeTenantActions,
} from "./registry-fixture";

const access = { kind: "subject", tenant: "alpha", subject: "alice" };
const own = {
  id: "own",
  resource: "https://mcp.example/mcp",
  scopes: ["read"],
  state: "needs_reconnection",
};
const panel = (page: Page, title: string) =>
  page
    .locator("section.panel")
    .filter({ has: page.getByRole("heading", { name: title, exact: true }) });
async function setup(page: Page, actions: string[]) {
  await installBearerDashboard(page, "fixture", {
    tenant: "alpha",
    name: "alice",
  });
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({ json: { access, node_id: "aidash://test" } });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access,
          deployment: {
            id: "aidash://test",
            endpoint: "http://localhost",
            protocol_version: "0.4",
            key: null,
          },
          registry: [],
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
    if (path === "/api/tenants/alpha/approval-decider")
      return route.fulfill({ json: builtInApprovalDecider });
    if (path === "/api/tenants/alpha/connections")
      return route.fulfill({ json: [own] });
    if (path === "/api/tenants/alpha/connections/probe")
      return route.fulfill({
        json: {
          oauth: true,
          scopes_supported: ["read", "write", "admin"],
          scope_ceiling: ["read", "write"],
        },
      });
    if (path === "/api/tenants/alpha/secrets")
      return route.fulfill({
        json: ["tenant", "me"].map((owner) => ({
          name: "key",
          owner,
          hosts: ["mcp.example"],
          fingerprint: "fingerprint",
          last4: "1234",
          revision: 1,
        })),
      });
    if (path === "/api/tenants/alpha/oauth-clients")
      return route.fulfill({ json: [] });
    return route.fulfill({ json: [] });
  });
  await routeTenantActions(page, actions, {
    tenant: "alpha",
    deployment: "aidash://test",
  });
  await routeRegistryVersions(page, () => []);
}

test("a member reconnects their own server, sees both secret owners, and can write only personal secrets", async ({
  page,
}) => {
  await setup(page, ["secret.use"]);
  await page.route("**/connections/authorizations", (route) =>
    route.fulfill({
      json: {
        authorization_url: `${new URL(route.request().url()).origin}/oauth-fixture`,
      },
    }),
  );
  await page.context().route("**/oauth-fixture", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: '<script>window.opener.postMessage({type:"aidash.connection",outcome:"reconnected"},location.origin);window.close();</script>',
    }),
  );
  await page.goto("/trust?view=authorization&connection=connected");
  await expect(
    page.getByRole("status").filter({ hasText: "Connected" }),
  ).toBeVisible();
  expect(new URL(page.url()).searchParams.has("connection")).toBe(false);
  const connections = panel(page, "Connections");
  await expect(
    connections.getByText("Needs reconnection", { exact: true }),
  ).toBeVisible();
  await expect(connections.getByLabel("Member subject")).toHaveCount(0);
  await connections
    .getByRole("button", { name: "Reconnect", exact: true })
    .click();
  await expect(
    connections.getByText("Reconnected", { exact: true }),
  ).toBeVisible();
  const secrets = panel(page, "Tenant Secrets");
  await expect(secrets.getByRole("listitem")).toHaveCount(2);
  await expect(
    secrets.getByRole("listitem").getByText("Personal", { exact: true }),
  ).toBeVisible();
  await expect(
    secrets.getByRole("listitem").getByText("Shared", { exact: true }),
  ).toBeVisible();
  await expect(secrets.getByLabel("Owner").locator("option")).toHaveText([
    "Personal",
  ]);
  await expect(
    secrets.getByRole("button", { name: "Edit", exact: true }),
  ).toHaveCount(1);
  await expect(
    panel(page, "OAuth Clients").getByLabel("Owner").locator("option"),
  ).toHaveText(["Mine"]);
  await expect(
    page.getByRole("heading", { name: "MCP server scope ceilings" }),
  ).toHaveCount(0);
});

test("administrators force disconnect metadata and edit immutable-issuer clients and scope ceilings", async ({
  page,
}) => {
  await setup(page, [
    "secret.use",
    "secret.manage",
    "connection.administer",
    "connection.scopes.update",
  ]);
  const writes: { path: string; method: string; body: unknown }[] = [];
  let memberConnected = true;
  await page.route("**/members/bob/connections**", (route) => {
    if (route.request().method() === "DELETE") {
      memberConnected = false;
      writes.push({
        path: new URL(route.request().url()).pathname,
        method: "DELETE",
        body: null,
      });
    }
    return route.fulfill({
      json:
        route.request().method() === "GET"
          ? memberConnected
            ? [{ ...own, id: "bob-connection", subject: "bob" }]
            : []
          : { revoked: true },
    });
  });
  const client = {
    id: "client",
    owner: "tenant",
    issuer: "https://issuer.example",
    client_id: "registered-client",
    has_secret: true,
    revision: 4,
  };
  await page.route("**/oauth-clients**", (route) => {
    if (route.request().method() === "PUT")
      writes.push({
        path: new URL(route.request().url()).pathname,
        method: "PUT",
        body: route.request().postDataJSON(),
      });
    return route.fulfill({
      json: route.request().method() === "GET" ? [client] : client,
    });
  });
  await page.route("**/connection-scope-ceilings/*", (route) => {
    if (route.request().method() === "PUT")
      writes.push({
        path: new URL(route.request().url()).pathname,
        method: "PUT",
        body: route.request().postDataJSON(),
      });
    return route.fulfill({
      json: { resource: own.resource, scopes: ["read"], revision: 2 },
    });
  });
  await page.goto("/trust?view=authorization");
  const connections = panel(page, "Connections");
  await connections.getByLabel("Member subject").fill("bob");
  await connections.getByRole("button", { name: "View connections" }).click();
  await connections.getByRole("button", { name: "Force disconnect" }).click();
  await expect
    .poll(() => writes[0]?.path)
    .toBe("/api/tenants/alpha/members/bob/connections/bob-connection");
  await expect(
    connections.getByRole("button", { name: "Force disconnect" }),
  ).toHaveCount(0);
  const clients = panel(page, "OAuth Clients");
  await clients.getByRole("button", { name: "Edit", exact: true }).click();
  await expect(clients.getByLabel("Issuer", { exact: true })).toHaveAttribute(
    "readonly",
    "",
  );
  await clients
    .getByLabel("Client secret", { exact: false })
    .fill("write-only-canary");
  await clients.getByRole("button", { name: "Save", exact: true }).click();
  await expect(clients.locator('input[type="password"]')).toHaveValue("");
  await expect.poll(() => writes.length).toBe(2);
  expect(writes[1].body).toEqual({
    owner: "tenant",
    issuer: "https://issuer.example",
    client_id: "registered-client",
    client_secret: "write-only-canary",
    expected_revision: 4,
  });
  const ceiling = panel(page, "MCP server scope ceilings");
  await ceiling
    .getByLabel("MCP server", { exact: true })
    .fill("https://MCP.example:443/mcp/");
  await ceiling.getByRole("button", { name: "Edit ceiling" }).click();
  await ceiling.getByLabel("Allowed scopes (space separated)").fill("");
  await ceiling.getByRole("button", { name: "Save", exact: true }).click();
  await expect.poll(() => writes.length).toBe(3);
  expect(writes[2].body).toEqual({ scopes: [], expected_revision: 2 });
  expect(
    Buffer.from(writes[2].path.split("/").at(-1)!, "base64url").toString(),
  ).toBe(own.resource);
  expect(
    await page.evaluate(() =>
      JSON.stringify([
        Object.entries(localStorage),
        Object.entries(sessionStorage),
      ]),
    ),
  ).not.toContain("write-only-canary");
});

test("Registry MCP registration bounds scope choices and remains available when Connect is skipped", async ({
  page,
}) => {
  await setup(page, ["registry.create", "secret.use"]);
  let registered: Record<string, unknown> | null = null;
  await page.route("**/api/registry", (route) => {
    registered = route.request().postDataJSON();
    return route.fulfill({ json: registered });
  });
  await page.goto("/registry");
  await page
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("tool");
  await dialog.getByLabel("Description", { exact: true }).fill("MCP tool");
  await dialog
    .getByLabel("Connection type", { exact: true })
    .selectOption("mcp");
  await dialog
    .getByLabel("Endpoint", { exact: true })
    .fill("https://mcp.example/mcp");
  await dialog.getByLabel("MCP tool name", { exact: true }).fill("search");
  await dialog
    .getByLabel("Declared OAuth scopes (space separated)")
    .fill("read admin");
  await expect(
    dialog.getByRole("checkbox", { name: "read", exact: true }),
  ).toBeChecked();
  await expect(
    dialog.getByRole("checkbox", { name: "admin", exact: true }),
  ).toHaveCount(0);
  await expect(
    dialog.getByRole("checkbox", { name: "write", exact: true }),
  ).toHaveCount(0);
  await dialog.getByRole("button", { name: "Skip", exact: true }).click();
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  await expect.poll(() => registered).not.toBeNull();
  expect(registered).toMatchObject({
    config: { transport: { transport: "mcp", scopes: ["read", "admin"] } },
  });
});
