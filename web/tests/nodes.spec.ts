import { expect, test, type Page } from "@playwright/test";
import type { NodeRecord } from "../src/generated/models";
import { installBearerDashboard } from "./auth-fixture";

const access = { kind: "subject", tenant: "alpha", subject: "alice" };
const deployment = "aidash://home";
const created = "2026-10-21T00:00:00Z";

const node = (name: string, label: string, isDefault = false): NodeRecord => ({
  name,
  address: `${deployment}/${name}`,
  label,
  default: isDefault,
  created_at: created,
  state: "live",
  deletion: null,
  storage_retained_until_purge: false,
});
const deleted = (name: string, by = "tenant_administrator"): NodeRecord => ({
  ...node(name, name),
  state: "deleted",
  storage_retained_until_purge: true,
  deletion: {
    id: `deletion-${name}`,
    deleted_at: created,
    deleted_by_kind: by,
    purge_after: "2099-11-20T00:00:00Z",
    purged_at: null,
    purge_step: null,
    purge_failures: 0,
    purge_error: null,
    purge_failed_at: null,
  },
});
const workspace = (id: string, title: string, owner: string) => ({
  id,
  node: owner,
  title,
  goal: `${title} goal`,
  state: {},
  revision: 1,
  created_at: created,
});
const registration = (
  owner: string,
  source: string,
  overrides: Record<string, unknown> = {},
) => ({
  node: owner,
  source_node: source,
  endpoint: null,
  key_id: null,
  atomic_transactions: false,
  enabled: true,
  revision: 1,
  actor: "oidc:identity",
  created_at: created,
  updated_at: created,
  ...overrides,
});

/** One Tenant with two Nodes, a Workspace in each, and a Tenant Administrator
 *  who may manage Nodes and their registrations. */
async function setup(
  page: Page,
  options: {
    lab?: { request?: boolean };
    member?: boolean;
    updateOnly?: boolean;
    administrator?: boolean;
    retained?: NodeRecord[];
    restoreConflict?: boolean;
  } = {},
) {
  const nodes = [
    node("first", "Home", true),
    node("second", "Lab"),
    ...(options.retained ?? []),
  ];
  const queries: URL[] = [];
  const actor = options.administrator
    ? { kind: "deployment_administrator" }
    : access;
  const registrations: Record<string, ReturnType<typeof registration>[]> = {
    first: [registration("first", `${deployment}/sibling`)],
    second: [],
  };
  const posts: { path: string; body: Record<string, unknown> }[] = [];
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await installBearerDashboard(
    page,
    "fixture",
    options.administrator
      ? undefined
      : {
          tenant: "alpha",
          name: "alice",
        },
  );
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({ json: { access: actor, node_id: deployment } });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access: actor,
          deployment: {
            id: deployment,
            endpoint: "http://localhost",
            protocol_version: "0.4",
            key: null,
          },
          registry: [],
          workspaces: [
            workspace("w-home", "Home research", "first"),
            workspace("w-lab", "Lab notebook", "second"),
          ],
          tasks: [],
          artifacts: [],
          events: [],
          runs: [],
          conversations: [],
          human_requests: options.lab?.request
            ? [
                {
                  id: "ask-lab",
                  workspace_id: "w-lab",
                  run_id: "run-lab",
                  kind: "QUESTION",
                  prompt: "Which sample goes first?",
                  response: null,
                  answered_by: null,
                  created_at: created,
                },
              ]
            : [],
        },
      });
    if (path === "/api/tenants/alpha/administration")
      return route.fulfill({
        json: {
          tenant: "alpha",
          label: null,
          actions: options.member
            ? []
            : options.updateOnly
              ? ["node.update"]
              : [
                  "node.delete",
                  "node.create",
                  "node.update",
                  "node_registration.read",
                  "node_registration.update",
                ],
          assignable_groups: [],
          policy_revision: 1,
        },
      });
    if (path === "/api/tenants/alpha/nodes") {
      if (method === "POST") {
        const body = request.postDataJSON();
        posts.push({ path, body });
        const added = node("third", body.label);
        nodes.push(added);
        registrations.third = [];
        return route.fulfill({ status: 201, json: added });
      }
      const url = new URL(request.url());
      queries.push(url);
      const rows =
        url.searchParams.get("include_deleted") === "true"
          ? nodes.slice(
              Number(url.searchParams.get("offset") ?? 0),
              Number(url.searchParams.get("offset") ?? 0) +
                Number(url.searchParams.get("limit")),
            )
          : nodes.filter((value) => value.state === "live");
      return route.fulfill({ json: rows });
    }
    if (path === "/api/tenants")
      return route.fulfill({
        json: [
          {
            name: "alpha",
            label: "Alpha",
            personal: false,
            deletion: null,
            restorable: false,
            purgeable: false,
            purge: null,
          },
        ],
      });
    const lifecycle =
      /^\/api\/tenants\/alpha\/nodes\/([^/]+)\/(default|deletion|restore|purge)$/.exec(
        path,
      );
    if (lifecycle && method === "POST") {
      const target = nodes.find((value) => value.name === lifecycle[1])!;
      posts.push({
        path,
        body: request.postData() ? request.postDataJSON() : {},
      });
      if (lifecycle[2] === "restore" && options.restoreConflict)
        return route.fulfill({
          status: 409,
          json: { error: "Node capacity exceeded" },
        });
      if (lifecycle[2] === "default")
        nodes.forEach((value) => {
          value.default = value === target;
        });
      if (lifecycle[2] === "deletion")
        Object.assign(target, deleted(target.name), { label: target.label });
      if (lifecycle[2] === "restore")
        Object.assign(target, {
          state: "live",
          deletion: null,
          storage_retained_until_purge: false,
        });
      if (lifecycle[2] === "purge") {
        target.state = "deleting";
        Object.assign(target.deletion!, {
          purge_step: "capability_objects",
          purge_failures: 3,
          purge_error: "storage unavailable",
          purge_failed_at: created,
        });
      }
      return route.fulfill({ json: target });
    }
    const relabel = /^\/api\/tenants\/alpha\/nodes\/([^/]+)$/.exec(path);
    if (relabel && method === "PATCH") {
      const body = request.postDataJSON();
      posts.push({ path, body });
      const target = nodes.find((value) => value.name === relabel[1])!;
      target.label = body.label;
      return route.fulfill({ json: target });
    }
    const registered =
      /^\/api\/tenants\/alpha\/nodes\/([^/]+)\/registrations$/.exec(path);
    if (registered) {
      const owner = registered[1];
      if (method === "POST") {
        const body = request.postDataJSON();
        posts.push({ path, body });
        const rows = registrations[owner];
        const index = rows.findIndex(
          (row) => row.source_node === body.source_node,
        );
        const next = registration(owner, body.source_node, {
          endpoint: body.endpoint ?? null,
          key_id: body.key_id ?? null,
          atomic_transactions: body.atomic_transactions,
          enabled: body.enabled,
          revision: body.expected_revision + 1,
        });
        if (index < 0) rows.push(next);
        else rows[index] = next;
        return route.fulfill({ json: next });
      }
      return route.fulfill({ json: registrations[owner] });
    }
    if (/\/registration-history$/.test(path))
      return route.fulfill({ json: [] });
    if (/^\/api\/workspaces\/[^/]+\/message-history$/.test(path))
      return route.fulfill({ json: { messages: [], next_before: null } });
    const snapshot = /^\/api\/workspaces\/([^/]+)$/.exec(path);
    if (snapshot && method === "GET")
      return route.fulfill({
        json: {
          workspace: workspace(
            snapshot[1],
            snapshot[1] === "w-lab" ? "Lab notebook" : "Home research",
            snapshot[1] === "w-lab" ? "second" : "first",
          ),
          tasks: [],
          artifacts: [],
          events: [],
          messages: [],
        },
      });
    if (path === "/api/workspaces" && method === "POST") {
      const body = request.postDataJSON();
      posts.push({ path, body });
      return route.fulfill({
        json: workspace("w-new", body.title, body.node ?? "first"),
      });
    }
    return route.fulfill({ json: [] });
  });
  return { posts, errors, registrations, queries };
}

const openAccountMenu = (page: Page) =>
  page.getByRole("button", { name: "Account settings", exact: true }).click();
const switcher = (page: Page) =>
  page.getByRole("button", { name: /^Switch request/ });

test("the selected Node decides which Workspaces are listed, and is remembered", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/");
  await switcher(page).click();
  const list = page.getByRole("navigation", { name: "Workspaces" });
  await expect(
    list.getByRole("link", { name: "# Home research" }),
  ).toBeVisible();
  await expect(list.getByRole("link", { name: "# Lab notebook" })).toHaveCount(
    0,
  );
  await page.keyboard.press("Escape");

  await openAccountMenu(page);
  const selector = page.getByTestId("node-selector");
  await expect(selector).toHaveValue("first");
  await selector.selectOption("second");
  await expect(page.getByText("home / Lab", { exact: true })).toBeVisible();

  await switcher(page).click();
  await expect(
    page
      .getByRole("navigation", { name: "Workspaces" })
      .getByRole("link", { name: "# Lab notebook" }),
  ).toBeVisible();
  await expect(
    page
      .getByRole("navigation", { name: "Workspaces" })
      .getByRole("link", { name: "# Home research" }),
  ).toHaveCount(0);

  await page.reload();
  await openAccountMenu(page);
  await expect(page.getByTestId("node-selector")).toHaveValue("second");
  expect(
    await page.evaluate(() => localStorage.getItem("aidash-node:alpha")),
  ).toBe("second");
  expect(errors).toEqual([]);
});

test("a link to a Workspace of another Node opens in that Node", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/collaboration?channel=w-lab");
  await expect(
    page.getByRole("heading", { name: "# Lab notebook", exact: true }),
  ).toBeVisible();
  await openAccountMenu(page);
  await expect(page.getByTestId("node-selector")).toHaveValue("second");
  expect(errors).toEqual([]);
});

test("a new Workspace is created in the Node chosen in its form", async ({
  page,
}) => {
  const { posts, errors } = await setup(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page
    .getByRole("button", { name: "Prepare a channel", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByLabel("Node", { exact: true })).toHaveValue("first");
  await dialog
    .getByRole("textbox", { name: "Title", exact: true })
    .fill("Draft");
  await dialog
    .getByRole("textbox", { name: "Goal", exact: true })
    .fill("Notes");
  await dialog.getByLabel("Node", { exact: true }).selectOption("second");
  await dialog.getByRole("button", { name: "Create", exact: true }).click();
  await expect(page).toHaveURL(/channel=w-new/);
  expect(posts).toEqual([
    {
      path: "/api/workspaces",
      body: { title: "Draft", goal: "Notes", node: "second" },
    },
  ]);
  await openAccountMenu(page);
  await expect(page.getByTestId("node-selector")).toHaveValue("second");
  expect(errors).toEqual([]);
});

test("a Tenant Administrator adds and relabels Nodes", async ({ page }) => {
  const { posts, errors } = await setup(page);
  await page.goto("/settings?view=nodes");
  const rows = page.locator("[data-node]");
  await expect(rows).toHaveCount(2);
  await expect(rows.first()).toContainText("Home");
  await expect(rows.first()).toContainText("Default");
  await expect(rows.first()).toContainText(`${deployment}/first`);

  await page.getByRole("button", { name: "Add Node", exact: true }).click();
  await page.getByRole("dialog").getByLabel("Node label").fill("Staging");
  await page.getByRole("dialog").getByRole("button", { name: "Save" }).click();
  await expect(rows).toHaveCount(3);
  await expect(page.locator('[data-node="third"]')).toContainText("Staging");

  await page
    .locator('[data-node="second"]')
    .getByRole("button", { name: "Change label" })
    .click();
  await page.getByRole("dialog").getByLabel("Node label").fill("Laboratory");
  await page.getByRole("dialog").getByRole("button", { name: "Save" }).click();
  await expect(page.locator('[data-node="second"]')).toContainText(
    "Laboratory",
  );
  expect(posts).toEqual([
    { path: "/api/tenants/alpha/nodes", body: { label: "Staging" } },
    { path: "/api/tenants/alpha/nodes/second", body: { label: "Laboratory" } },
  ]);
  expect(errors).toEqual([]);
});

test("registering a Node of another Deployment pins its endpoint and key ID", async ({
  page,
}) => {
  const { posts, errors } = await setup(page);
  await page.goto("/settings?view=nodes");
  const registrations = page.locator("[data-registration]");
  await expect(registrations).toHaveCount(1);
  await expect(registrations).toContainText(`${deployment}/sibling`);

  await page.getByRole("button", { name: "Register a Node" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Node address").fill("aidash://away/other-node");
  await dialog.getByLabel("Endpoint").fill("https://aidash.example.com");
  await dialog.getByLabel("Key ID").fill("a".repeat(43));
  await dialog.getByRole("checkbox").check();
  await dialog.getByRole("button", { name: "Register", exact: true }).click();
  await expect(registrations).toHaveCount(2);
  const remote = page.locator('[data-registration="aidash://away/other-node"]');
  await expect(remote).toContainText("https://aidash.example.com");
  await expect(remote).toContainText("a".repeat(43));
  await expect(remote).toContainText("Atomic transactions");

  await remote.getByRole("button", { name: "Disable", exact: true }).click();
  await expect(remote).toContainText("Disabled");
  expect(posts.map((post) => post.body)).toEqual([
    {
      source_node: "aidash://away/other-node",
      atomic_transactions: true,
      enabled: true,
      expected_revision: 0,
      endpoint: "https://aidash.example.com",
      key_id: "a".repeat(43),
    },
    {
      source_node: "aidash://away/other-node",
      atomic_transactions: true,
      enabled: false,
      expected_revision: 1,
    },
  ]);
  expect(errors).toEqual([]);
});

test("a Node of this Deployment is registered with its address alone", async ({
  page,
}) => {
  const { posts, errors } = await setup(page);
  await page.goto("/settings?view=nodes");
  await page.getByRole("button", { name: "Register a Node" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Node address").fill(`${deployment}/neighbour`);
  await expect(dialog.getByLabel("Endpoint")).toHaveCount(0);
  await expect(dialog.getByLabel("Key ID")).toHaveCount(0);
  await dialog.getByRole("button", { name: "Register", exact: true }).click();
  await expect(page.locator("[data-registration]")).toHaveCount(2);
  expect(posts.map((post) => post.body)).toEqual([
    {
      source_node: `${deployment}/neighbour`,
      atomic_transactions: false,
      enabled: true,
      expected_revision: 0,
    },
  ]);
  expect(errors).toEqual([]);
});

test("registrations belong to the selected Node", async ({ page }) => {
  const { errors } = await setup(page);
  await page.goto("/settings?view=nodes");
  await expect(page.locator("[data-registration]")).toHaveCount(1);
  await page
    .locator('[data-node="second"]')
    .getByRole("button", { name: "Work here" })
    .click();
  await expect(
    page.getByText("No Node is registered at this Node."),
  ).toBeVisible();
  await expect(page.locator("[data-registration]")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("a question waiting in another Node is listed, and answering it goes to that Node", async ({
  page,
}) => {
  const { errors } = await setup(page, { lab: { request: true } });
  await page.goto("/");
  await page.getByRole("button", { name: /^Decisions/ }).click();
  const queue = page.getByRole("button", { name: /Which sample goes first\?/ });
  await expect(queue).toContainText("Lab / Lab notebook");
  await queue.click();
  const dialog = page.getByRole("dialog", { name: "Human input", exact: true });
  await expect(dialog.getByText("Which sample goes first?")).toBeVisible();
  await expect(page).toHaveURL(/channel=w-lab/);
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(dialog).toBeHidden();
  await openAccountMenu(page);
  await expect(page.getByTestId("node-selector")).toHaveValue("second");
  expect(errors).toEqual([]);
});

/** The browser owns deliberate independent confirmation and stale-form handling;
 * actual record authority/CAS persistence is owned by HTTP/PostgreSQL tests. */
test("receiving keys require independent confirmation and discard stale consent", async ({
  page,
}) => {
  const { registrations, errors } = await setup(page);
  const source = "aidash://remote/remote-node";
  registrations.first.push(
    registration("first", source, {
      endpoint: "https://remote.example",
      key_id: "a".repeat(43),
      enabled: false,
    }),
  );
  const writes: Record<string, unknown>[] = [];
  let status = {
    key_id: "a".repeat(43),
    quarantined: false,
    keys_revision: 1,
    authorization_revision: 2,
    enabled: false,
  };
  let conflict = false;
  await page.route(
    "**/api/tenants/alpha/nodes/first/registration-keys**",
    async (route) => {
      if (route.request().method() === "GET")
        return route.fulfill({ json: status });
      const body = route.request().postDataJSON();
      writes.push(body);
      if (conflict) {
        conflict = false;
        status = { ...status, keys_revision: 3 };
        return route.fulfill({
          status: 409,
          json: { error: "revision changed" },
        });
      }
      status = {
        ...status,
        quarantined: body.action === "quarantine",
        keys_revision: status.keys_revision + 1,
      };
      return route.fulfill({ json: status });
    },
  );
  await page.goto("/settings?view=nodes");
  const row = page.locator(`[data-registration="${source}"]`);
  await row.getByRole("button", { name: "Manage key", exact: true }).click();
  await page
    .getByRole("button", { name: "Quarantine key", exact: true })
    .click();
  await expect(
    page.getByText("Key quarantined", { exact: true }),
  ).toBeVisible();
  await page
    .getByLabel("Independently confirmed key ID", { exact: true })
    .fill("b".repeat(43));
  await page
    .getByLabel("Public key PEM", { exact: true })
    .fill("-----BEGIN PUBLIC KEY-----\nfixture\n-----END PUBLIC KEY-----");
  const confirm = page.getByRole("button", {
    name: "Confirm key",
    exact: true,
  });
  await expect(confirm).toBeDisabled();
  await page
    .getByLabel("I checked the key ID through a separate trusted channel", {
      exact: true,
    })
    .check();
  conflict = true;
  await confirm.click();
  await expect(
    page.getByText(
      "The record changed. Review its current key and confirm again.",
      { exact: true },
    ),
  ).toBeVisible();
  await expect(confirm).toBeDisabled();
  await page
    .getByLabel("I checked the key ID through a separate trusted channel", {
      exact: true,
    })
    .check();
  await confirm.click();
  expect(writes).toEqual([
    {
      source_node: source,
      action: "quarantine",
      expected_keys_revision: 1,
      expected_authorization_revision: 2,
    },
    {
      source_node: source,
      action: "confirm",
      expected_keys_revision: 2,
      expected_authorization_revision: 2,
      expected_key_id: "b".repeat(43),
      public_key_pem:
        "-----BEGIN PUBLIC KEY-----\nfixture\n-----END PUBLIC KEY-----",
    },
    {
      source_node: source,
      action: "confirm",
      expected_keys_revision: 3,
      expected_authorization_revision: 2,
      expected_key_id: "b".repeat(43),
      public_key_pem:
        "-----BEGIN PUBLIC KEY-----\nfixture\n-----END PUBLIC KEY-----",
    },
  ]);
  expect(errors).toEqual([]);
});

test("Node management switches the default, confirms deletion and falls back to the live default", async ({
  page,
}) => {
  const { posts, queries, registrations, errors } = await setup(page);
  const source = "aidash://remote/retained-source";
  registrations.first.push(
    registration("first", source, {
      endpoint: "https://remote.example",
      key_id: "a".repeat(43),
      enabled: false,
    }),
  );
  await page.goto("/settings?view=nodes");
  const first = page.locator('[data-lifecycle-node="first"]');
  const second = page.locator('[data-lifecycle-node="second"]');
  const keys = page.locator("section.panel").filter({
    has: page.getByRole("heading", { name: "Deleted Node keys", exact: true }),
  });
  await expect(keys.getByRole("button", { name: "Review keys" })).toHaveCount(
    0,
  );
  await expect(
    first.getByRole("button", { name: "Delete Node", exact: true }),
  ).toHaveCount(0);
  await second.getByRole("button", { name: "Make default" }).click();
  await expect(second).toContainText("Default");
  await page
    .locator('[data-node="first"]')
    .getByRole("button", { name: "Work here" })
    .click();
  await first.getByRole("button", { name: "Delete Node", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Delete Node", exact: true });
  await expect(dialog).toContainText("Storage charges depend on your Offering");
  await expect(
    dialog.getByRole("button", { name: "Delete Node", exact: true }),
  ).toBeDisabled();
  await dialog.getByLabel("Type the Node name to confirm").fill("first");
  await dialog
    .getByRole("button", { name: "Delete Node", exact: true })
    .click();
  await expect(dialog).toBeHidden();
  await expect(first).toContainText("Deleted");
  await expect(first).toContainText("2099");
  await expect(first).toContainText("Files remain stored until purge");
  await expect(
    second.getByRole("button", { name: "Delete Node", exact: true }),
  ).toHaveCount(0);
  await openAccountMenu(page);
  await expect(page.getByTestId("node-selector")).toHaveValue("second");
  await expect(page.getByTestId("node-selector").locator("option")).toHaveCount(
    1,
  );
  await page.keyboard.press("Escape");
  // The same mounted panel must refresh after deletion, without navigation,
  // refocus or reload. Selecting it opens the existing foreign record controls.
  await expect(keys.getByText("Home", { exact: true })).toBeVisible();
  await keys.getByRole("button", { name: "Review keys" }).click();
  await expect(keys.getByText(source, { exact: true })).toBeVisible();
  await expect(
    keys.getByRole("button", { name: "Manage key", exact: true }),
  ).toBeVisible();
  await first
    .getByRole("button", { name: "Restore Node", exact: true })
    .click();
  await expect(page.locator('[data-node="first"]')).toBeVisible();
  await expect(keys.getByRole("button", { name: "Review keys" })).toHaveCount(
    0,
  );
  await expect(keys.getByText(source, { exact: true })).toHaveCount(0);
  await expect(
    keys.getByRole("button", { name: "Manage key", exact: true }),
  ).toHaveCount(0);
  expect(posts.map((post) => post.path)).toEqual([
    "/api/tenants/alpha/nodes/second/default",
    "/api/tenants/alpha/nodes/first/deletion",
    "/api/tenants/alpha/nodes/first/restore",
  ]);
  expect(posts[1].body).toEqual({ confirmation: "first" });
  expect(queries.some((url) => url.search === "")).toBe(true);
  expect(
    queries.some(
      (url) =>
        url.searchParams.get("include_deleted") === "true" &&
        url.searchParams.get("limit") === "25",
    ),
  ).toBe(true);
  expect(errors).toEqual([]);
});

test("deleted Node management reports restore conflicts and restricts administrator deletions", async ({
  page,
}) => {
  await setup(page, {
    retained: [
      deleted("recoverable"),
      deleted("operator-deleted", "deployment_administrator"),
    ],
    restoreConflict: true,
  });
  await page.goto("/settings?view=nodes");
  const recoverable = page.locator('[data-lifecycle-node="recoverable"]');
  await recoverable
    .getByRole("button", { name: "Restore Node", exact: true })
    .click();
  await expect(page.getByRole("alert")).toContainText("current Node capacity");
  const restricted = page.locator('[data-lifecycle-node="operator-deleted"]');
  await expect(restricted).toContainText(
    "Only a Deployment Administrator can restore",
  );
  await expect(
    restricted.getByRole("button", { name: "Restore Node", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Purge Node now", exact: true }),
  ).toHaveCount(0);
});

test("a Deployment Administrator explicitly opens a Tenant's Node management and requests purge", async ({
  page,
}) => {
  const { posts, queries, errors } = await setup(page, {
    administrator: true,
    retained: [deleted("recoverable", "deployment_administrator")],
  });
  await page.goto("/trust?view=authorization");
  await expect(
    page.getByRole("heading", { name: "Node lifecycle", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Manage Nodes", exact: true }).click();
  const retained = page.locator('[data-lifecycle-node="recoverable"]');
  await expect(
    retained.getByRole("button", { name: "Restore Node", exact: true }),
  ).toBeVisible();
  await retained
    .getByRole("button", { name: "Purge Node now", exact: true })
    .click();
  const dialog = page.getByRole("dialog", {
    name: "Purge Node permanently",
    exact: true,
  });
  await dialog.getByLabel("Type the Node name to confirm").fill("recoverable");
  await dialog
    .getByRole("button", { name: "Purge Node permanently", exact: true })
    .click();
  await expect(retained).toContainText("Deleting");
  await expect(retained).toContainText("storage unavailable");
  await expect(retained).toContainText("3");
  await expect(
    retained.getByRole("button", { name: "Restore Node", exact: true }),
  ).toHaveCount(0);
  expect(posts.map((post) => post.path)).toEqual([
    "/api/tenants/alpha/nodes/recoverable/purge",
  ]);
  expect(
    queries.every((url) => url.pathname === "/api/tenants/alpha/nodes"),
  ).toBe(true);
  expect(errors).toEqual([]);
});

test("an ordinary member has live selection and no Node lifecycle controls or management reads", async ({
  page,
}) => {
  const { queries } = await setup(page, {
    member: true,
    retained: [deleted("hidden")],
  });
  await page.goto("/settings?view=nodes");
  await expect(page.locator("[data-node]")).toHaveCount(2);
  await expect(
    page.getByRole("heading", { name: "Node lifecycle", exact: true }),
  ).toHaveCount(0);
  expect(queries.every((url) => !url.searchParams.has("include_deleted"))).toBe(
    true,
  );
});

test("the node.update allowlist permits changing the default without deletion-management access", async ({
  page,
}) => {
  const { queries } = await setup(page, { updateOnly: true });
  await page.goto("/settings?view=nodes");
  const second = page.locator('[data-lifecycle-node="second"]');
  await second
    .getByRole("button", { name: "Make default", exact: true })
    .click();
  await expect(second).toContainText("Default");
  await expect(
    page.getByRole("button", { name: "Delete Node", exact: true }),
  ).toHaveCount(0);
  expect(queries.every((url) => !url.searchParams.has("include_deleted"))).toBe(
    true,
  );
});

test("Node management pages retained records and offers no recovery for purged or expired Nodes", async ({
  page,
}) => {
  const purged = deleted("permanent");
  purged.state = "purged";
  purged.storage_retained_until_purge = false;
  purged.deletion!.purged_at = created;
  const expired = deleted("expired");
  expired.deletion!.purge_after = "2020-11-20T00:00:00Z";
  const { queries } = await setup(page, {
    retained: [
      ...Array.from({ length: 23 }, (_, index) => deleted(`retained-${index}`)),
      purged,
      expired,
    ],
  });
  await page.goto("/settings?view=nodes");
  const panel = page
    .getByRole("heading", { name: "Node lifecycle", exact: true })
    .locator("xpath=ancestor::section[1]");
  await expect(page.locator("[data-lifecycle-node]")).toHaveCount(25);
  await panel.getByRole("button", { name: "Next page", exact: true }).click();
  await expect(page.locator("[data-lifecycle-node]")).toHaveCount(2);
  const permanent = page.locator('[data-lifecycle-node="permanent"]');
  await expect(permanent).toContainText("Purged");
  await expect(permanent.getByRole("button")).toHaveCount(0);
  await expect(page.locator('[data-lifecycle-node="expired"]')).toContainText(
    "The restore deadline has passed",
  );
  await expect(
    page.getByRole("button", { name: "Restore Node", exact: true }),
  ).toHaveCount(0);
  await expect(
    panel.getByRole("button", { name: "Next page", exact: true }),
  ).toBeDisabled();
  expect(
    queries.some(
      (url) =>
        url.searchParams.get("offset") === "25" &&
        url.searchParams.get("limit") === "25",
    ),
  ).toBe(true);
  await panel
    .getByRole("button", { name: "Previous page", exact: true })
    .click();
  await expect(page.locator("[data-lifecycle-node]")).toHaveCount(25);
});
