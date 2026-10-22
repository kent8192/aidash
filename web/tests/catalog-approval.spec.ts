import { expect, test, type Page } from "@playwright/test";
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
  config: Record<string, unknown> = {},
  version = "1.0.0",
  name = id,
) => ({
  id,
  version,
  kind,
  name: { en: name, ja: name },
  description: { en: "Catalog fixture" },
  capabilities: [],
  languages: ["en"],
  skills: [],
  tags: [],
  schema: {},
  config,
});
const ref = (id: string, version = "1.0.0") => ({ id, version });
const unapproved = { approval: null, history: [] };
type Version = ReturnType<typeof registryVersion>;
type Decision = { path: string; body: Record<string, unknown> };

async function dashboard(
  page: Page,
  access: object,
  versions: () => Version[],
  answer: (decision: Decision) => {
    status?: number;
    json: unknown;
  } = () => ({ json: [] }),
) {
  await installBearerDashboard(page, "fixture");
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  const requests: string[] = [];
  const decisions: Decision[] = [];
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    requests.push(path);
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({ json: { access, node_id: "aidash://node" } });
    if (path === "/api/mesh")
      return route.fulfill({ json: { nodes: [], errors: [] } });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access,
          node: {
            id: "aidash://node",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            capabilities: [],
            clusters: [],
          },
          registry: versions()
            .filter(
              (version) => !version.needs_approval || version.approval?.enabled,
            )
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
    if (
      path.startsWith("/api/authorization/") &&
      route.request().method() === "POST"
    ) {
      const decision = { path, body: route.request().postDataJSON() };
      decisions.push(decision);
      return route.fulfill(answer(decision));
    }
    return route.fulfill({ json: [] });
  });
  await routeRegistryVersions(page, versions);
  return { requests, decisions };
}

const row = (page: Page, name: string) =>
  page.locator(".entity-card").filter({
    has: page.getByRole("heading", { name, exact: true }),
  });
const approval = (page: Page) =>
  page.getByRole("dialog").getByRole("region", { name: "Catalog Approval" });

for (const locale of ["en-US", "ja-JP"] as const) {
  test(`a Deployment Administrator learns that definitions belong to Tenants (${locale})`, async ({
    page,
  }) => {
    const { requests } = await dashboard(
      page,
      { kind: "deployment_administrator" },
      () => [],
    );
    await page.addInitScript(
      (value) => localStorage.setItem("aidash-locale", value),
      locale,
    );
    for (const view of ["registry", "agents", "clusters"]) {
      await page.goto(`/settings?view=${view}`);
      await expect(
        page.getByRole("status").filter({
          hasText:
            locale === "ja-JP"
              ? "定義は各テナントに属します。登録と承認は、それぞれのテナントのレジストリで行います。"
              : "Definitions belong to Tenants. Each Tenant registers and approves its own in its Registry.",
        }),
      ).toBeVisible();
      await expect(
        page.getByRole("button", {
          name: locale === "ja-JP" ? "エンティティを登録" : "Register entity",
        }),
      ).toHaveCount(0);
    }
    expect(
      requests.filter((path) =>
        /^\/api\/(registry\/versions|tenants\/)/.test(path),
      ),
    ).toEqual([]);
  });
}

for (const actions of [[], ["registry.create"]]) {
  test(`Register follows registry.create (${actions.join(",") || "none"})`, async ({
    page,
  }) => {
    await dashboard(page, tenantSubject, () => [
      registryVersion(entry("researcher", "agent")),
    ]);
    await routeTenantActions(page, actions);
    await page.goto("/settings?view=agents");
    await expect(row(page, "researcher")).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Register entity", exact: true }),
    ).toHaveCount(actions.length);
  });
}

test("the Registry list follows next_offset through every page", async ({
  page,
}) => {
  await dashboard(page, tenantSubject, () => []);
  const offsets: string[] = [];
  await page.route(
    (url) => url.pathname === "/api/registry/versions",
    (route) => {
      const offset =
        new URL(route.request().url()).searchParams.get("offset") ?? "0";
      offsets.push(offset);
      return route.fulfill({
        json:
          offset === "0"
            ? {
                versions: [registryVersion(entry("first", "agent"))],
                next_offset: 100,
              }
            : {
                versions: [registryVersion(entry("second", "agent"))],
                next_offset: null,
              },
      });
    },
  );
  await page.goto("/settings?view=agents");
  await expect(row(page, "first")).toBeVisible();
  await expect(row(page, "second")).toBeVisible();
  expect([...new Set(offsets)].sort()).toEqual(["0", "100"]);
});

test("every version shows its approval state and newest-first history", async ({
  page,
}) => {
  const history = [
    {
      revision: 1,
      enabled: true,
      actor: "alice",
      created_at: "2026-10-01T09:00:00Z",
    },
    {
      revision: 2,
      enabled: false,
      actor: "bob",
      created_at: "2026-10-02T09:00:00Z",
    },
    {
      revision: 3,
      enabled: true,
      actor: "carol",
      created_at: "2026-10-03T09:00:00Z",
    },
  ];
  await dashboard(page, tenantSubject, () => [
    registryVersion(entry("search", "tool"), {
      approval: { enabled: true, revision: 3 },
      history,
    }),
    registryVersion(entry("model", "model"), unapproved),
    registryVersion(entry("notes", "skill"), {
      approval: { enabled: false, revision: 2 },
      history: history.slice(0, 2),
    }),
    registryVersion(entry("aidash.file_read", "tool"), {
      needs_approval: false,
      ...unapproved,
    }),
  ]);
  await routeTenantActions(page, []);
  await page.goto("/settings?view=registry");
  // The list opens on the first kind present and hides the others.
  await expect(page.locator('[data-registry-kind="model"]')).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(row(page, "model")).toHaveCount(1);
  await expect(row(page, "search")).toHaveCount(0);
  await page.locator('[data-registry-kind="all"]').click();
  await expect(page.locator(".entity-card")).toHaveCount(4);
  for (const [name, state, kind] of [
    ["search", "Approved", "tool"],
    ["model", "Not approved", "model"],
    ["notes", "Withdrawn", "skill"],
    ["aidash.file_read", "Built-in", "tool"],
  ]) {
    await page.locator(`[data-registry-kind="${kind}"]`).click();
    await expect(row(page, name).locator(".badge:visible")).toContainText([
      state,
    ]);
  }
  await row(page, "search").click();
  await expect(approval(page).locator(".catalog-history")).toHaveText([
    /Approved.*Revision 3 · carol/,
    /Withdrawn.*Revision 2 · bob/,
    /Approved.*Revision 1 · alice/,
  ]);
  await expect(approval(page).getByRole("button")).toHaveCount(0);
  await page.keyboard.press("Escape");
  await page.locator('[data-registry-kind="model"]').click();
  await row(page, "model").click();
  await expect(approval(page)).toContainText("No approval decisions yet.");
  await expect(approval(page).getByRole("button")).toHaveCount(0);
});

test("a new agent version starts from the groups of the latest approved earlier version", async ({
  page,
}) => {
  let approved = false;
  const writer = (version: string, name: string, groups?: string[]) =>
    registryVersion(
      entry("writer", "agent", { model: ref("model") }, version, name),
      {
        subject: groups && {
          id: `aidash://node/agents/writer@${version}`,
          groups,
          enabled: true,
        },
      },
    );
  const { decisions } = await dashboard(page, tenantSubject, () => [
    registryVersion(entry("model", "model")),
    writer("1.0.0", "Writer one", ["members"]),
    writer("1.5.0", "Writer one point five", ["reviewers"]),
    approved
      ? writer("2.0.0", "Writer two", ["reviewers"])
      : { ...writer("2.0.0", "Writer two"), ...unapproved },
  ]);
  await routeTenantActions(page, ["catalog.approve", "subject.group.update"], {
    assignable: ["members", "reviewers"],
  });
  await page.goto("/settings?view=agents");
  await row(page, "Writer two").click();
  const groups = approval(page).getByRole("group", {
    name: "Groups for the agent subject",
  });
  await expect(groups.getByLabel("reviewers", { exact: true })).toBeChecked();
  await expect(groups.getByLabel("members", { exact: true })).not.toBeChecked();
  approved = true;
  await approval(page)
    .getByRole("button", { name: "Approve", exact: true })
    .click();
  await expect.poll(() => decisions.length).toBe(1);
  expect(decisions[0]).toEqual({
    path: "/api/authorization/acme/catalog",
    body: {
      entry: ref("writer", "2.0.0"),
      enabled: true,
      expected_revision: 0,
      groups: ["reviewers"],
    },
  });
  await expect(
    approval(page).getByRole("button", {
      name: "Withdraw approval",
      exact: true,
    }),
  ).toBeVisible();
  await expect(approval(page)).toContainText(
    "aidash://node/agents/writer@2.0.0",
  );
});

test("without subject.group.update an agent approval keeps the default group", async ({
  page,
}) => {
  const { decisions } = await dashboard(page, tenantSubject, () => [
    registryVersion(entry("helper", "agent"), unapproved),
  ]);
  await routeTenantActions(page, ["catalog.approve"]);
  await page.goto("/settings?view=agents");
  await row(page, "helper").click();
  await expect(approval(page).getByRole("checkbox")).toHaveCount(0);
  await expect(approval(page)).toContainText(
    "Approval makes this agent a subject of this Tenant in the members group.",
  );
  await approval(page)
    .getByRole("button", { name: "Approve", exact: true })
    .click();
  await expect.poll(() => decisions.length).toBe(1);
  expect(decisions[0].body).toEqual({
    entry: ref("helper"),
    enabled: true,
    expected_revision: 0,
  });
});

test("an approval refused for a dependency approves the whole closure in one request", async ({
  page,
}) => {
  const versions = [
    registryVersion(
      entry("planner", "agent", {
        model: ref("model"),
        bindings: [
          { kind: "bundle", target: ref("aidash.bundle.research") },
          { kind: "skill", target: ref("notes") },
        ],
      }),
      unapproved,
    ),
    registryVersion(
      entry("model", "model", {
        provider: "openai",
        endpoint: "http://ollama.lan:11434/v1",
      }),
      unapproved,
    ),
    registryVersion(
      entry("aidash.bundle.research", "bundle", {
        members: [ref("aidash.web_search"), ref("aidash.page_read")],
      }),
      unapproved,
    ),
    registryVersion(entry("aidash.web_search", "tool"), {
      approval: { enabled: false, revision: 2 },
    }),
    registryVersion(entry("aidash.page_read", "tool")),
    registryVersion(entry("notes", "skill")),
  ];
  const { decisions } = await dashboard(
    page,
    tenantSubject,
    () => versions,
    ({ path }) =>
      path.endsWith("/catalog")
        ? {
            status: 400,
            json: { error: "CATALOG_DEPENDENCY_UNAPPROVED: model@1.0.0" },
          }
        : { json: [] },
  );
  await routeTenantActions(page, ["catalog.approve"]);
  await page.goto("/settings?view=agents");
  await row(page, "planner").click();
  await approval(page)
    .getByRole("button", { name: "Approve", exact: true })
    .click();
  const closure = approval(page).getByRole("group", {
    name: "Approve with its dependencies",
  });
  await expect(closure.locator("li")).toHaveText([
    /^model · 1\.0\.0.*No retention guarantee · http:\/\/ollama\.lan:11434/,
    /^aidash\.bundle\.research · 1\.0\.0/,
    /^aidash\.web_search · 1\.0\.0/,
    /^planner · 1\.0\.0/,
  ]);
  await closure.getByRole("button", { name: "Approve all (4)" }).click();
  await expect.poll(() => decisions.length).toBe(2);
  expect(decisions[1]).toEqual({
    path: "/api/authorization/acme/catalog/approvals",
    body: {
      approvals: [
        { entry: ref("model"), enabled: true, expected_revision: 0 },
        {
          entry: ref("aidash.bundle.research"),
          enabled: true,
          expected_revision: 0,
        },
        {
          entry: ref("aidash.web_search"),
          enabled: true,
          expected_revision: 2,
        },
        { entry: ref("planner"), enabled: true, expected_revision: 0 },
      ],
    },
  });
  await expect(closure).toHaveCount(0);
});

test("a closure waiting on a Marketplace installation cannot be approved here", async ({
  page,
}) => {
  const installed = entry("copied-model", "model");
  const { decisions } = await dashboard(
    page,
    tenantSubject,
    () => [
      registryVersion(
        entry("planner", "agent", { model: ref("copied-model") }),
        unapproved,
      ),
      registryVersion(
        { ...installed, installation: { installation: "copy", revision: 1 } },
        unapproved,
      ),
    ],
    () => ({
      status: 400,
      json: { error: "CATALOG_DEPENDENCY_UNAPPROVED: copied-model@1.0.0" },
    }),
  );
  await routeTenantActions(page, ["catalog.approve"]);
  await page.goto("/settings?view=agents");
  await row(page, "planner").click();
  await approval(page)
    .getByRole("button", { name: "Approve", exact: true })
    .click();
  const closure = approval(page).getByRole("group", {
    name: "Approve with its dependencies",
  });
  await expect(closure).toContainText(
    "copied-model · 1.0.0 · Activate its Marketplace installation first",
  );
  await expect(
    closure.getByRole("button", { name: "Approve all (1)" }),
  ).toBeDisabled();
  expect(decisions).toHaveLength(1);
});

test("withdrawing fences on the approval revision and reports a conflict", async ({
  page,
}) => {
  const { decisions } = await dashboard(
    page,
    tenantSubject,
    () => [
      registryVersion(entry("search", "tool"), {
        approval: { enabled: true, revision: 3 },
      }),
    ],
    () => ({ status: 409, json: { error: "catalog revision changed" } }),
  );
  await routeTenantActions(page, ["catalog.approve"]);
  await page.goto("/settings?view=registry");
  await row(page, "search").click();
  await approval(page)
    .getByRole("button", { name: "Withdraw approval", exact: true })
    .click();
  await expect(approval(page).getByRole("alert")).toHaveText(
    "The approval changed. Review the current state and try again.",
  );
  expect(decisions[0].body).toEqual({
    entry: ref("search"),
    enabled: false,
    expected_revision: 3,
  });
});

test("a tool version shows what a call does, and approving it covers a read-only declaration", async ({
  page,
}) => {
  const tool = (id: string, transport: Record<string, unknown>) =>
    entry(id, "tool", {
      registry_node: "aidash://node",
      provider: `integration.${transport.transport}@1`,
      operation: "invoke",
      default_alias: id,
      tier: "integration",
      narrow: {},
      transport,
    });
  const reads = tool("lookup", {
    transport: "http",
    endpoint: "https://example.com/lookup",
    replay: "read_only",
  });
  const writes = tool("update", {
    transport: "mcp",
    endpoint: "https://example.com/mcp",
    tool_name: "update",
    replay: "unsafe",
    idempotency_argument: null,
  });
  const delegate = tool("delegate", {
    transport: "agent",
    node_id: "aidash://node",
    agent: ref("worker"),
  });
  const approved = new Set<string>();
  const { decisions } = await dashboard(
    page,
    tenantSubject,
    () =>
      [reads, writes, delegate].map((value) =>
        registryVersion(value, approved.has(value.id) ? {} : unapproved),
      ),
    ({ body }) => {
      approved.add((body.entry as { id: string }).id);
      return { json: {} };
    },
  );
  await routeTenantActions(page, ["catalog.approve"]);
  await page.goto("/settings?view=registry");
  const dialog = page.getByRole("dialog");
  const effect = (value: string) =>
    dialog.locator("dl").filter({ hasText: "What a call does" }).filter({
      hasText: value,
    });

  await row(page, "lookup").click();
  await expect(effect("Reads only (needs no approval)")).toBeVisible();
  const notice = approval(page).locator(".read-only-notice");
  await expect(notice).toContainText(
    "Approving the version approves that declaration.",
  );
  await approval(page)
    .getByRole("button", { name: "Approve", exact: true })
    .click();
  await expect(notice).toHaveCount(0);
  await expect(effect("Reads only (needs no approval)")).toBeVisible();
  expect(decisions.map(({ body }) => body.entry)).toEqual([ref("lookup")]);
  await page.keyboard.press("Escape");

  await row(page, "update").click();
  await expect(effect("May change things (asks for approval)")).toBeVisible();
  await expect(approval(page).locator(".read-only-notice")).toHaveCount(0);
  await page.keyboard.press("Escape");

  await row(page, "delegate").click();
  await expect(dialog.getByText("What a call does")).toHaveCount(0);
});
