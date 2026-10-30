import { expect, test, type Page } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import {
  defaultNode,
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
  await installBearerDashboard(
    page,
    "fixture",
    "tenant" in access && "subject" in access
      ? { tenant: String(access.tenant), name: String(access.subject) }
      : undefined,
  );
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
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access,
          deployment: {
            id: "aidash://node",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            key: null,
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
        },
      });
    // Withdrawal first shows what it stops (#540).
    if (path.endsWith("/catalog/impact"))
      return route.fulfill({ json: { agents: [], running_runs: 0 } });
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
              ? "Deployment Administrator はテナントの内容を表示できません。"
              : "Tenant content is not available to a Deployment Administrator.",
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
  await routeTenantActions(page, []);
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
  await expect(
    approval(page).getByRole("button", {
      name: "Approval Review",
      exact: true,
    }),
  ).toHaveCount(1);
  await page.keyboard.press("Escape");
  await page.locator('[data-registry-kind="model"]').click();
  await row(page, "model").click();
  await expect(approval(page)).toContainText("No approval decisions yet.");
  await expect(
    approval(page).getByRole("button", {
      name: "Approval Review",
      exact: true,
    }),
  ).toHaveCount(1);
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
      node: defaultNode.name,
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
    node: defaultNode.name,
  });
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
  await expect(approval(page)).toContainText(
    "No approved agent on this Node uses it.",
  );
  expect(decisions).toHaveLength(0);
  await approval(page)
    .getByRole("button", { name: "Withdraw anyway", exact: true })
    .click();
  await expect(approval(page).getByRole("alert")).toHaveText(
    "The approval changed. Review the current state and try again.",
  );
  expect(decisions[0].body).toEqual({
    entry: ref("search"),
    enabled: false,
    expected_revision: 3,
    node: defaultNode.name,
  });
});

test("a tool version shows what a call does, and approving it covers a read-only declaration", async ({
  page,
}) => {
  const tool = (id: string, transport: Record<string, unknown>) =>
    entry(id, "tool", {
      registry_deployment: "aidash://node",
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
    node_id: `aidash://node/${defaultNode.name}`,
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

test("warning tool approval requires acknowledgment once and hides the warning after approval", async ({
  page,
}) => {
  let approved = false;
  const warning =
    "Containers are not deleted by Aidash; files persist at OpenRouter up to 30 days after last use; no Aidash sandbox protections.";
  const tool = () => ({
    ...registryVersion(
      entry("provider.openrouter.shell", "tool", { tier: "provider" }),
      approved ? { approval: { enabled: true, revision: 1 } } : unapproved,
    ),
    provider_warning: warning,
  });
  const { decisions } = await dashboard(
    page,
    tenantSubject,
    () => [tool()],
    () => {
      approved = true;
      return { json: { enabled: true, revision: 1 } };
    },
  );
  await routeTenantActions(page, ["catalog.approve"]);
  await page.goto("/settings?view=registry");
  await row(page, "provider.openrouter.shell").click();
  const section = approval(page);
  await expect(section.locator(".provider-tool-warning")).toContainText(
    warning,
  );
  const approve = section.getByRole("button", { name: "Approve", exact: true });
  await expect(approve).toBeDisabled();
  await section.locator(".provider-tool-warning").getByRole("checkbox").check();
  await approve.click();
  await expect.poll(() => decisions.length).toBe(1);
  expect(decisions[0].body).toEqual({
    entry: ref("provider.openrouter.shell"),
    enabled: true,
    expected_revision: 0,
    provider_warning_acknowledged: true,
    node: defaultNode.name,
  });
  await expect(
    section.getByRole("button", { name: "Withdraw approval", exact: true }),
  ).toBeVisible();
  await expect(section.locator(".provider-tool-warning")).toHaveCount(0);
});

test("dependency review requires acknowledgment for each warning tool", async ({
  page,
}) => {
  const current = reviewBody([
    reviewItem("writer", "agent", "approved"),
    reviewItem("provider.openrouter.shell", "tool", "approvable", {
      provider_warning: "shell disclosure",
    }),
    reviewItem("provider.openrouter.advisor", "tool", "approvable", {
      provider_warning: "advisor disclosure",
    }),
  ]);
  const { decisions } = await reviewDashboard(page, {
    actions: ["catalog.approve"],
    review: () => current,
    answer: () => ({ json: { bindings: [], installations: [] } }),
  });
  const review = await openReview(page);
  const submit = review.getByRole("button", { name: /Approve selected/ });
  await expect(submit).toBeDisabled();
  const disclosures = review.locator(".provider-review-warning");
  await expect(disclosures).toHaveCount(2);
  await disclosures.nth(0).getByRole("checkbox").check();
  await expect(submit).toBeDisabled();
  await disclosures.nth(1).getByRole("checkbox").check();
  await submit.click();
  await expect.poll(() => decisions.length).toBe(1);
  expect(
    (
      decisions[0].body.items as { provider_warning_acknowledged?: boolean }[]
    ).map((item) => item.provider_warning_acknowledged),
  ).toEqual([true, true]);
});

const reviewItem = (
  id: string,
  kind: string,
  status: string,
  extra: Record<string, unknown> = {},
) => ({
  reference: ref(id),
  kind,
  name: { en: id, ja: id },
  description: { en: `${id} definition`, ja: `${id} 定義` },
  digest: `sha256:${id}`,
  status,
  approved: status === "approved",
  can_approve: true,
  expected_revision: 2,
  restriction: null,
  proposed_restriction: {
    subjects: ["acme/agents/writer@1.0.0"],
    groups: [],
  },
  widen: false,
  installation: null,
  ...extra,
});

const reviewBody = (
  items: ReturnType<typeof reviewItem>[],
  overrides: Record<string, unknown> = {},
) => ({
  node: defaultNode.name,
  root: ref("writer"),
  items,
  hidden: 0,
  closure_digest: "sha256:closure",
  ready: false,
  ...overrides,
});

const requestBody = (id: string, overrides: Record<string, unknown> = {}) => ({
  id,
  node: defaultNode.name,
  root: ref("writer"),
  requester: "bob",
  reason: "Please review the writer",
  state: "open",
  decline_reason: null,
  closure_digest: "sha256:closure",
  created_at: new Date().toISOString(),
  updated_at: new Date().toISOString(),
  revision: 1,
  next_action: "waiting_for_approver",
  review: { ready: false, hidden: 0, total: 2, approvable: 1 },
  ...overrides,
});

async function reviewDashboard(
  page: Page,
  options: {
    access?: object;
    actions?: string[];
    review: () => Record<string, unknown>;
    requests?: () => Record<string, unknown>[];
    pending?: () => Record<string, unknown>[];
    answer: (decision: Decision) => { status?: number; json: unknown };
  },
) {
  const harness = await dashboard(
    page,
    options.access ?? tenantSubject,
    () => [
      registryVersion(entry("writer", "agent", { model: ref("model") }), {
        approval: { enabled: false, revision: 1 },
        history: [],
      }),
      registryVersion(entry("model", "model")),
    ],
    options.answer,
  );
  await routeTenantActions(page, options.actions ?? []);
  await page.route(
    (url) =>
      url.pathname === "/api/authorization/acme/catalog/review" &&
      url.searchParams.get("entry") === "writer@1.0.0",
    (route) =>
      route.request().method() === "GET"
        ? route.fulfill({ json: options.review() })
        : route.fallback(),
  );
  await page.route(
    (url) => url.pathname === "/api/authorization/acme/catalog/requests/mine",
    (route) =>
      route.fulfill({
        json: {
          requests: options.requests?.() ?? [],
          next_offset: null,
        },
      }),
  );
  await page.route(
    (url) =>
      url.pathname === "/api/authorization/acme/catalog/requests/pending",
    (route) =>
      route.fulfill({
        json: {
          requests: options.pending?.() ?? [],
          next_offset: null,
        },
      }),
  );
  return harness;
}

async function openReview(page: Page) {
  await page.goto("/settings?view=agents");
  await row(page, "writer").click();
  await approval(page)
    .getByRole("button", { name: "Approval Review", exact: true })
    .click();
  return page.getByRole("region", { name: "Approval Review" });
}

test("the Review shows the whole closure and approves local items and activations in one submission", async ({
  page,
}) => {
  let current = reviewBody(
    [
      reviewItem("writer", "agent", "approved"),
      reviewItem("model", "model", "approvable"),
      reviewItem("copy", "agent", "needs_marketplace_activation", {
        installation: {
          installation: "acme/copy",
          revision: 3,
          digest: "sha256:revision",
          expected_activation_revision: 1,
          active_revision: null,
        },
      }),
      reviewItem("widened", "tool", "approved", { widen: true }),
    ],
    { hidden: 2 },
  );
  const { decisions } = await reviewDashboard(page, {
    actions: ["catalog.approve"],
    review: () => current,
    answer: ({ path }) => {
      if (path === "/api/authorization/acme/catalog/review") {
        current = reviewBody(
          current.items.map((item) =>
            "installation" in item && item.installation
              ? { ...item, status: "approved", approved: true, widen: false }
              : { ...item, status: "approved", approved: true, widen: false },
          ),
          { ready: true, hidden: 2 },
        );
        return { json: { bindings: [], installations: [] } };
      }
      return { json: [] };
    },
  });
  const review = await openReview(page);
  await expect(review).toContainText("2 items you cannot see");
  await expect(review).toContainText("Widens an existing approval");
  await expect(review).toContainText("You can approve it");
  await expect(review).toContainText("Needs Marketplace activation");
  await review.getByRole("button", { name: /Approve selected \(3\)/ }).click();
  await expect.poll(() => decisions.length).toBe(1);
  const body = decisions[0].body as {
    items: { reference: { id: string } }[];
    installations: { installation: string }[];
    request?: string;
  };
  expect(body.items.map((item) => item.reference.id).sort()).toEqual([
    "model",
    "widened",
  ]);
  expect(body.installations).toEqual([
    expect.objectContaining({ installation: "acme/copy", revision: 3 }),
  ]);
  expect(body.request).toBeUndefined();
  await expect(review).toContainText("Ready to use on this Node.");
});

test("a dependency review retains the chosen groups for a new root agent", async ({
  page,
}) => {
  const current = reviewBody([
    reviewItem("writer", "agent", "approvable"),
    reviewItem("model", "model", "approvable"),
  ]);
  const { decisions } = await reviewDashboard(page, {
    actions: ["catalog.approve", "subject.group.update"],
    review: () => current,
    answer: ({ path }) =>
      path.endsWith("/catalog")
        ? { status: 400, json: { error: "dependency unapproved" } }
        : { json: { bindings: [], installations: [] } },
  });
  await routeTenantActions(page, ["catalog.approve", "subject.group.update"], {
    assignable: ["members", "reviewers"],
  });
  await page.goto("/settings?view=agents");
  await row(page, "writer").click();
  const groups = approval(page).getByRole("group", {
    name: "Groups for the agent subject",
  });
  await groups.getByLabel("members", { exact: true }).uncheck();
  await groups.getByLabel("reviewers", { exact: true }).check();
  await approval(page)
    .getByRole("button", { name: "Approve", exact: true })
    .click();
  const review = page.getByRole("region", { name: "Approval Review" });
  await review.getByRole("button", { name: /Approve selected/ }).click();
  await expect.poll(() => decisions.length).toBe(2);
  expect(decisions[1].body.root_groups).toEqual(["reviewers"]);
});

test("a dependency review retains the chosen restriction for a new root agent", async ({
  page,
}) => {
  const current = reviewBody([
    reviewItem("writer", "agent", "approvable", { proposed_restriction: null }),
    reviewItem("model", "model", "approvable"),
  ]);
  const { decisions } = await reviewDashboard(page, {
    actions: ["catalog.approve"],
    review: () => current,
    answer: ({ path }) =>
      path.endsWith("/catalog")
        ? { status: 400, json: { error: "dependency unapproved" } }
        : { json: { bindings: [], installations: [] } },
  });
  await page.goto("/settings?view=agents");
  await row(page, "writer").click();
  await approval(page)
    .getByRole("checkbox", { name: "Who may use it" })
    .check();
  await approval(page)
    .getByLabel("Subjects (comma separated)")
    .fill("alice, bob");
  await approval(page)
    .getByRole("button", { name: "Approve", exact: true })
    .click();
  const review = page.getByRole("region", { name: "Approval Review" });
  await review.getByRole("button", { name: /Approve selected/ }).click();
  await expect.poll(() => decisions.length).toBe(2);
  const items = (
    decisions[1].body as {
      items: { reference: { id: string }; restriction: unknown }[];
    }
  ).items;
  expect(
    items.find((item) => item.reference.id === "writer")?.restriction,
  ).toEqual({ subjects: ["alice", "bob"], groups: [] });
});

test("a partial submission keeps the pending root restriction for the next one", async ({
  page,
}) => {
  let current = reviewBody([
    reviewItem("writer", "agent", "approvable", { proposed_restriction: null }),
    reviewItem("model", "model", "approvable"),
  ]);
  const { decisions } = await reviewDashboard(page, {
    actions: ["catalog.approve"],
    review: () => current,
    answer: ({ path }) => {
      if (path.endsWith("/catalog"))
        return { status: 400, json: { error: "dependency unapproved" } };
      current = reviewBody(
        [
          reviewItem("writer", "agent", "approvable", {
            proposed_restriction: null,
          }),
          reviewItem("model", "model", "approved"),
        ],
        { closure_digest: "sha256:closure-2" },
      );
      return { json: { bindings: [], installations: [] } };
    },
  });
  await page.goto("/settings?view=agents");
  await row(page, "writer").click();
  await approval(page)
    .getByRole("checkbox", { name: "Who may use it" })
    .check();
  await approval(page).getByLabel("Subjects (comma separated)").fill("alice");
  await approval(page)
    .getByRole("button", { name: "Approve", exact: true })
    .click();
  const review = page.getByRole("region", { name: "Approval Review" });
  await review.getByRole("checkbox", { name: "writer" }).uncheck();
  await review.getByRole("button", { name: /Approve selected \(1\)/ }).click();
  await expect.poll(() => decisions.length).toBe(2);
  await review.getByRole("button", { name: /Approve selected \(1\)/ }).click();
  await expect.poll(() => decisions.length).toBe(3);
  const items = (
    decisions[2].body as {
      items: { reference: { id: string }; restriction: unknown }[];
    }
  ).items;
  expect(items.map((item) => item.reference.id)).toEqual(["writer"]);
  expect(items[0].restriction).toEqual({ subjects: ["alice"], groups: [] });
});

test("canceling a restriction edit leaves the approved item out of the submission", async ({
  page,
}) => {
  const current = reviewBody([
    reviewItem("writer", "agent", "approved", {
      restriction: { subjects: ["alice"], groups: [] },
      proposed_restriction: { subjects: ["alice"], groups: [] },
    }),
    reviewItem("model", "model", "approvable"),
  ]);
  const { decisions } = await reviewDashboard(page, {
    actions: ["catalog.approve"],
    review: () => current,
    answer: () => ({ json: { bindings: [], installations: [] } }),
  });
  const review = await openReview(page);
  const item = review
    .locator("li")
    .filter({ has: page.getByText("writer definition", { exact: true }) });
  await item.getByRole("button", { name: "Edit restriction" }).click();
  await item.getByRole("button", { name: "Cancel" }).click();
  await review.getByRole("button", { name: /Approve selected \(1\)/ }).click();
  await expect.poll(() => decisions.length).toBe(1);
  expect(
    (decisions[0].body as { items: { reference: { id: string } }[] }).items.map(
      (value) => value.reference.id,
    ),
  ).toEqual(["model"]);
});

test("partial authority submits only the items the principal may approve and stays not ready", async ({
  page,
}) => {
  const current = reviewBody([
    reviewItem("writer", "agent", "approved"),
    reviewItem("model", "model", "approvable"),
    reviewItem("tool", "tool", "needs_other_approver", {
      can_approve: false,
    }),
  ]);
  const { decisions } = await reviewDashboard(page, {
    actions: ["catalog.approve"],
    review: () => current,
    answer: () => ({ json: { bindings: [], installations: [] } }),
  });
  const review = await openReview(page);
  await expect(review).toContainText("Needs another approver");
  await review.getByRole("button", { name: /Approve selected \(1\)/ }).click();
  await expect.poll(() => decisions.length).toBe(1);
  expect((decisions[0].body as { items: unknown[] }).items).toHaveLength(1);
  await expect(review).not.toContainText("Ready to use on this Node.");
});

test("a stale review reports the change, reloads, and never resubmits", async ({
  page,
}) => {
  let current = reviewBody([
    reviewItem("writer", "agent", "approved"),
    reviewItem("model", "model", "approvable"),
  ]);
  const { decisions } = await reviewDashboard(page, {
    actions: ["catalog.approve"],
    review: () => current,
    answer: ({ path }) => {
      if (path === "/api/authorization/acme/catalog/review") {
        current = reviewBody(
          [
            reviewItem("writer", "agent", "approved"),
            reviewItem("model", "model", "approvable"),
          ],
          { closure_digest: "sha256:changed" },
        );
        return {
          status: 409,
          json: { error: "APPROVAL_REVIEW_STALE" },
        };
      }
      return { json: [] };
    },
  });
  const review = await openReview(page);
  await review.getByRole("button", { name: /Approve selected/ }).click();
  await expect(review.getByRole("alert")).toContainText(
    "This changed since you reviewed it",
  );
  await expect.poll(() => decisions.length).toBe(1);
  // The reload served the changed closure; no second POST was sent.
  await expect(review).toContainText("You can approve it");
  expect(decisions).toHaveLength(1);
});

test("a consumer requests approval, sees the waiting state, and can withdraw", async ({
  page,
}) => {
  let requests: Record<string, unknown>[] = [];
  const { decisions } = await reviewDashboard(page, {
    access: tenantSubject,
    actions: [],
    review: () =>
      reviewBody([
        reviewItem("writer", "agent", "approved"),
        reviewItem("model", "model", "needs_other_approver", {
          can_approve: false,
        }),
      ]),
    requests: () => requests,
    answer: ({ path }) => {
      if (path === "/api/authorization/acme/catalog/requests") {
        requests = [requestBody("request-1", { requester: "alice" })];
        return { json: requests[0] };
      }
      if (path.endsWith("/catalog/requests/request-1/withdraw")) {
        requests = [
          requestBody("request-1", {
            requester: "alice",
            state: "withdrawn",
            next_action: "withdrawn",
          }),
        ];
        return { json: requests[0] };
      }
      return { json: [] };
    },
  });
  const review = await openReview(page);
  await expect(
    review.getByRole("button", { name: /Approve selected/ }),
  ).toHaveCount(0);
  await review
    .getByPlaceholder("Why this definition is needed")
    .fill("Please let the writer use the model");
  await review.getByRole("button", { name: "Send request" }).click();
  await expect.poll(() => decisions.length).toBe(1);
  expect(decisions[0].body).toMatchObject({
    node: defaultNode.name,
    root: ref("writer"),
    reason: "Please let the writer use the model",
  });
  await expect(review).toContainText("Waiting for an approver.");
  await review.getByRole("button", { name: "Withdraw request" }).click();
  await expect.poll(() => decisions.length).toBe(2);
  await expect(review).toContainText("Request withdrawn.");
});

test("a consumer sees a declined reason and a changed closure asks them to request again", async ({
  page,
}) => {
  let requests = [
    requestBody("request-declined", {
      state: "declined",
      next_action: "declined",
      decline_reason: "The model is not approved yet",
    }),
  ];
  await reviewDashboard(page, {
    access: tenantSubject,
    actions: [],
    review: () =>
      reviewBody([
        reviewItem("writer", "agent", "approved"),
        reviewItem("model", "model", "needs_other_approver", {
          can_approve: false,
        }),
      ]),
    requests: () => requests,
    answer: () => ({ json: [] }),
  });
  const review = await openReview(page);
  await expect(review).toContainText("Declined by an approver.");
  await expect(review).toContainText("The model is not approved yet");

  requests = [
    requestBody("request-changed", {
      closure_digest: "sha256:old",
      state: "open",
    }),
  ];
  await page.reload();
  await openReview(page);
  await expect(review).toContainText(
    "The reviewed closure changed. Request again.",
  );
  await expect(
    review.getByRole("button", { name: "Send request" }),
  ).toBeVisible();
});

test("an approver opens a pending request with its id and declines it with a reason", async ({
  page,
}) => {
  const pending = requestBody("request-pending");
  const review = reviewBody([
    reviewItem("writer", "agent", "approved"),
    reviewItem("model", "model", "approvable"),
  ]);
  const { decisions } = await reviewDashboard(page, {
    actions: ["catalog.approve"],
    review: () => review,
    pending: () => [pending],
    answer: ({ path }) => {
      if (path.endsWith("/catalog/requests/request-pending/decline"))
        return {
          json: { ...pending, state: "declined", next_action: "declined" },
        };
      return { json: { bindings: [], installations: [] } };
    },
  });
  await page.goto("/settings?view=registry");
  await page.getByRole("button", { name: "Approval requests" }).click();
  const queue = page.getByRole("region", { name: "Approval requests" });
  await expect(queue.getByText("bob", { exact: true })).toBeVisible();
  await queue.getByRole("button", { name: "Open review" }).click();
  const panel = page.getByRole("region", { name: "Approval Review" });
  await expect(panel).toContainText("Answering a Catalog Approval Request.");
  await panel.getByRole("button", { name: /Approve selected/ }).click();
  await expect.poll(() => decisions.length).toBe(1);
  expect(decisions[0].body).toMatchObject({ request: "request-pending" });
  await panel.getByRole("button", { name: "Close" }).click();
  await queue.getByRole("button", { name: "Decline" }).click();
  await page
    .getByPlaceholder("Why this request is declined")
    .fill("The dependency is not ready");
  await page.getByRole("button", { name: "Decline request" }).click();
  await expect.poll(() => decisions.length).toBe(2);
  expect(decisions[1].body).toMatchObject({
    reason: "The dependency is not ready",
  });
});

test("a consumer's request is fulfilled through the approver queue and then reads ready", async ({
  browser,
}) => {
  let consumerRequests: Record<string, unknown>[] = [];
  let consumerReview = reviewBody([
    reviewItem("writer", "agent", "approved"),
    reviewItem("model", "model", "needs_other_approver", {
      can_approve: false,
    }),
  ]);
  const consumer = await browser.newPage();
  await reviewDashboard(consumer, {
    access: tenantSubject,
    actions: [],
    review: () => consumerReview,
    requests: () => consumerRequests,
    answer: ({ path }) => {
      if (path === "/api/authorization/acme/catalog/requests") {
        consumerRequests = [
          requestBody("request-flow", { requester: "alice" }),
        ];
        return { json: consumerRequests[0] };
      }
      return { json: [] };
    },
  });
  const initial = await openReview(consumer);
  await initial
    .getByPlaceholder("Why this definition is needed")
    .fill("The model is ready for the writer");
  await initial.getByRole("button", { name: "Send request" }).click();
  await expect(initial).toContainText("Waiting for an approver.");

  let pending = [requestBody("request-flow", { requester: "alice" })];
  const approver = await browser.newPage();
  const { decisions } = await reviewDashboard(approver, {
    actions: ["catalog.approve"],
    review: () =>
      reviewBody([
        reviewItem("writer", "agent", "approved"),
        reviewItem("model", "model", "approvable"),
      ]),
    pending: () => pending,
    answer: ({ path }) => {
      if (path === "/api/authorization/acme/catalog/review") {
        pending = [];
        consumerRequests = [
          requestBody("request-flow", {
            requester: "alice",
            state: "fulfilled",
            next_action: "fulfilled",
          }),
        ];
        consumerReview = reviewBody(
          [
            reviewItem("writer", "agent", "approved"),
            reviewItem("model", "model", "approved"),
          ],
          { ready: true },
        );
        return { json: { bindings: [], installations: [] } };
      }
      return { json: [] };
    },
  });
  await approver.goto("/settings?view=registry");
  await approver.getByRole("button", { name: "Approval requests" }).click();
  await approver.getByRole("button", { name: "Open review" }).click();
  await approver
    .getByRole("region", { name: "Approval Review" })
    .getByRole("button", { name: /Approve selected/ })
    .click();
  await expect.poll(() => decisions.length).toBe(1);
  expect(decisions[0].body).toMatchObject({ request: "request-flow" });

  await consumer.reload();
  const fulfilled = await openReview(consumer);
  await expect(fulfilled).toContainText("Ready to use on this Node.");
  await expect(fulfilled).toContainText(
    "Fulfilled: every reviewed item is approved.",
  );
});
