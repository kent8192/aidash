import { test, expect, type Page } from "@playwright/test";
import { setup } from "./collaboration-fixture";

// UI contract fixtures for Web research in the dashboard. The real path —
// URL intent rendering, recorded approval cards, the Run research API and
// disclosure decisions against the fixture provider — is verified by the Rust
// web_research integration suite against PostgreSQL.

const NODE = "aidash://home";
const tool = (id: string, provider: string, operation: string) => ({
  id,
  version: "1.0.0",
  kind: "tool",
  name: { en: id },
  description: { en: id },
  capabilities: [],
  tags: [],
  languages: [],
  skills: [],
  schema: {},
  config: { registry_node: NODE, provider, operation },
});

type Card = Record<string, unknown> & { id: string; state: string };

const searchContent = JSON.stringify({ q: "rust release", count: 3 });
const pending: Card[] = [
  {
    id: "disclose-search",
    kind: "disclosure",
    run_id: "run-0",
    area_id: "area-one",
    state: "pending",
    revision: 1,
    requester: "alice",
    approver: "alice",
    viewer_is_approver: true,
    targets: ["api.search.brave.com"],
    action: "research.disclose",
    expires_at: "2099-01-01T00:00:00Z",
    grant_id: null,
    disclosure: searchContent,
    agent_id: "researcher",
    agent_version: "1.0.0",
    operation: "search",
    provider: "research.brave@1",
    estimated_micro_usd: 5000,
    query: { query: "rust release", language: "en", count: 3 },
    decided_by: null,
    decided_at: null,
  },
  {
    id: "disclose-page",
    kind: "disclosure",
    run_id: "run-0",
    area_id: "area-one",
    state: "pending",
    revision: 1,
    requester: "alice",
    approver: "alice",
    viewer_is_approver: true,
    targets: ["example.com"],
    action: "research.disclose",
    expires_at: "2099-01-01T00:00:00Z",
    grant_id: null,
    disclosure: "https://example.com/notes",
    agent_id: "researcher",
    agent_version: "1.0.0",
    operation: "fetch",
    provider: "research.page@1",
    decided_by: null,
    decided_at: null,
  },
  {
    id: "disclose-other-page",
    kind: "disclosure",
    run_id: "run-0",
    area_id: "area-one",
    state: "pending",
    revision: 1,
    requester: "alice",
    approver: "alice",
    viewer_is_approver: true,
    targets: ["example.org"],
    action: "research.disclose",
    expires_at: "2099-01-01T00:00:00Z",
    grant_id: null,
    disclosure: "https://example.org/page",
    agent_id: "researcher",
    agent_version: "1.0.0",
    operation: "fetch",
    provider: "research.page@1",
    decided_by: null,
    decided_at: null,
  },
];
const grants: Card[] = [
  {
    id: "grant-search",
    kind: "grant",
    run_id: "run-0",
    area_id: "area-one",
    state: "active",
    revision: 1,
    requester: "alice",
    approver: "alice",
    viewer_is_approver: true,
    targets: ["api.search.brave.com"],
    action: "research.disclose",
    expires_at: "2099-01-01T00:00:00Z",
    grant_id: null,
    agent_id: "researcher",
    agent_version: "1.0.0",
    operation: "search",
    provider: "research.brave@1",
    decided_by: null,
    decided_at: null,
  },
];
/**
 * What the requester's Thread lists: one waiting, one denied, one expired,
 * one closed by an Evidence Revocation, one nobody may approve, and the Run
 * grant the requester may revoke.
 */
const thread: Card[] = [
  {
    ...pending[1],
    id: "waiting",
    approver: "bob",
    viewer_is_approver: false,
  },
  {
    ...pending[2],
    id: "denied",
    state: "denied",
    approver: "bob",
    viewer_is_approver: false,
    decided_by: "bob",
    decided_at: "2026-10-14T09:00:00Z",
  },
  {
    ...pending[0],
    id: "expired",
    state: "expired",
    approver: "bob",
    viewer_is_approver: false,
    expires_at: "2026-10-01T00:00:00Z",
  },
  {
    ...pending[2],
    id: "closed",
    state: "closed",
    approver: "bob",
    viewer_is_approver: false,
    closed_reason: "evidence_revoked",
  },
  {
    ...pending[1],
    id: "blocked",
    state: "blocked",
    approver: null,
    viewer_is_approver: false,
  },
  {
    ...grants[0],
    id: "thread-grant",
    approver: "bob",
    viewer_is_approver: false,
  },
];

async function research(
  page: Page,
  { admittedPageReading = true, nextVersionWithoutResearch = false } = {},
) {
  const fixture = await setup(page, { subject: true, coreCapabilities: true });
  await page.goto("/collaboration?channel=workspace-one");
  const data = await page.evaluate(() =>
    fetch("/api/state").then((response) => response.json()),
  );
  // The Agent binds the fixture provider's bundle and Page reading directly.
  data.registry.push(
    tool("aidash.research.fixture.search", "research.fixture@1", "search"),
    tool("aidash.research.fixture.fetch", "research.fixture@1", "fetch"),
    tool("aidash.research.fixture.find", "research.fixture@1", "find"),
    tool("aidash.research.page.fetch", "research.page@1", "fetch"),
    {
      ...tool("research-fixture", "", ""),
      kind: "bundle",
      config: {
        members: ["search", "fetch", "find"].map((operation) => ({
          registry_node: NODE,
          id: `aidash.research.fixture.${operation}`,
          version: "1.0.0",
        })),
      },
    },
  );
  data.registry[0].config.bindings = [
    {
      kind: "bundle",
      target: { registry_node: NODE, id: "research-fixture", version: "1.0.0" },
      narrow: {},
    },
    {
      kind: "tool",
      target: {
        registry_node: NODE,
        id: "aidash.research.page.fetch",
        version: "1.0.0",
      },
      narrow: {},
    },
  ];
  // A later version of the same Agent that binds no research.
  if (nextVersionWithoutResearch)
    data.registry.push({
      ...data.registry[0],
      version: "1.1.0",
      config: { ...data.registry[0].config, bindings: [] },
    });
  const area = {
    id: "area-one",
    workspace_id: "workspace-one",
    thread_id: "thread-message-one",
    agent_id: "researcher",
    owner: "alice",
    state: "active",
    generation: 1,
    revision: 7,
    manifest: [],
  };
  const calls: { path: string; body: Record<string, unknown> }[] = [];
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const body =
      request.method() === "POST" ? (request.postDataJSON() ?? {}) : {};
    const send = (json: unknown) => route.fulfill({ json });
    if (path === "/api/state")
      return send({
        ...data,
        access: { kind: "subject", tenant: "acme", subject: "alice" },
      });
    if (path === "/api/working-areas")
      return send({ items: [area], next_cursor: null });
    if (path === "/api/working-areas/area-one/session")
      return send({
        area_id: area.id,
        active_run_id: "run-0",
        last_run_id: "run-0",
        last_agent_version: "1.0.0",
        queue: [
          { run_id: "run-0", sequence: 1, phase: "WAITING", control: "ACTIVE" },
        ],
      });
    if (
      path.endsWith("/core-operations") ||
      path.endsWith("/outbound") ||
      path.endsWith("/transfers")
    )
      return send({ items: [], next_cursor: null });
    if (path === "/api/research/providers") {
      // The Tenant approved the fixture's search and page reading copies,
      // not its find; Brave and public page reading are not set up.
      const operation = (provider: string, name: string, approved = true) => ({
        operation: name,
        approved_definitions: approved
          ? [
              {
                id: `aidash.${provider.replace("@1", "")}.${name}`,
                version: "1.0.0",
              },
            ]
          : [],
      });
      return send({
        providers: [
          {
            provider: "research.brave@1",
            set_up: false,
            operations: [operation("research.brave@1", "search", false)],
          },
          {
            provider: "research.page@1",
            set_up: false,
            operations: [
              operation("research.page@1", "fetch"),
              operation("research.page@1", "find"),
            ],
          },
          {
            provider: "research.fixture@1",
            set_up: true,
            operations: [
              operation("research.fixture@1", "search"),
              operation("research.fixture@1", "fetch"),
              operation("research.fixture@1", "find", false),
            ],
          },
        ],
        limits: {
          search_attempts: 20,
          fetch_attempts: 50,
          sources: 100,
          snapshots: 50,
          snapshot_bytes: 8388608,
          page_views: 30,
        },
      });
    }
    // The active Run's details carry the binding snapshot it was admitted
    // with; steering reads pages only through a fetch Binding it includes.
    if (path === "/api/runs/run-0")
      return send({
        run: {
          ...data.runs.find((run: { id: string }) => run.id === "run-0"),
          context: {
            binding_snapshot: {
              remote: false,
              bindings: [
                {
                  definition: tool(
                    "aidash.research.fixture.fetch",
                    "research.fixture@1",
                    "fetch",
                  ),
                  excluded_reason: admittedPageReading
                    ? null
                    : "catalog_approval",
                },
              ],
            },
          },
        },
        invocations: [],
        media_input_routes: [],
        memory: null,
      });
    if (path === "/api/runs/run-0/research")
      return send({
        limits: {
          search_attempts: 20,
          fetch_attempts: 50,
          sources: 100,
          snapshots: 50,
          snapshot_bytes: 8388608,
          page_views: 30,
        },
        usage: {
          search_attempts: 3,
          fetch_attempts: 1,
          sources: 4,
          snapshots: 1,
          snapshot_bytes: 2048,
          page_views: 2,
        },
        estimated_micro_usd: 15000,
        uncertain_attempts: 1,
        in_flight_attempts: 0,
        url_intents: [
          {
            input: "task",
            pages: ["https://example.com/notes"],
            requested_by: "alice",
            requested_at: "2026-10-14T08:00:00Z",
          },
        ],
      });
    if (path === "/api/capabilities/approvals") {
      const state = url.searchParams.get("state");
      const role = url.searchParams.get("role");
      const items =
        role === "approver"
          ? state === "pending"
            ? pending.filter((item) => item.state === "pending")
            : grants.filter((item) => item.state === "active")
          : state === "pending"
            ? thread.filter((item) => item.state === "pending")
            : thread;
      return send({ items, next_cursor: null });
    }
    if (request.method() === "POST") {
      calls.push({ path, body });
      const decided = /^\/api\/capabilities\/approvals\/([^/]+)\/decide$/.exec(
        path,
      );
      if (decided) {
        const card = pending.find((item) => item.id === decided[1])!;
        card.state = body.choice === "deny" ? "denied" : "approved";
        return send({
          approval_id: card.id,
          state: card.state,
          revision: 2,
          grant_id: body.choice === "allow_run" ? "grant-new" : null,
        });
      }
      const revoked = /^\/api\/capabilities\/grants\/([^/]+)\/revoke$/.exec(
        path,
      );
      if (revoked) {
        grants.find((item) => item.id === revoked[1])!.state = "revoked";
        return send({
          id: revoked[1],
          state: "revoked",
          revision: 2,
          effects_may_have_occurred: true,
        });
      }
      if (path.endsWith("/queue"))
        return send({
          area_id: area.id,
          active_run_id: "run-0",
          last_run_id: "run-0",
          last_agent_version: "1.0.0",
          queue: [],
        });
      if (path.endsWith("/steer")) return send({ accepted_run_id: "run-0" });
    }
    return route.fallback();
  });
  return { ...fixture, calls };
}

async function openThread(page: Page) {
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Reply in thread", exact: true })
    .first()
    .click();
  await expect(page.getByRole("heading", { name: "Agent work" })).toBeVisible();
}

test("an Agent's bound research operations show their availability, the Run's usage and the disclosure it waits on", async ({
  page,
}, info) => {
  const { errors } = await research(page);
  await openThread(page);
  const panel = page.getByRole("region", { name: "Web research", exact: true });
  await expect(panel).toBeVisible();
  const operations = panel.getByRole("listitem");
  await expect(operations).toHaveText([
    "Web search · research.fixture@1 · Ready",
    "Page reading · research.fixture@1 · Ready",
    "Page reading · research.page@1 · Not set up in this Deployment",
    "Find in a read page · research.fixture@1 · Not approved for use here",
  ]);
  // Nothing names an environment variable or who must act.
  await expect(panel).not.toContainText("AIDASH_");
  await expect(panel).not.toContainText("Deployment Administrator");
  await expect(
    panel.getByText(
      "This Run: searches 3/20 · page readings 1/50 · sources 4/100 · snapshots 1/50 · page views 2/30 · estimated research cost $0.0150 (an estimate, not an invoice; the Research account holder pays)",
    ),
  ).toBeVisible();
  // The URL intent comes from its record, not from the Task's text.
  await expect(
    panel.getByText("Asked to read (alice):", { exact: false }),
  ).toContainText("https://example.com/notes");
  await expect(
    panel.getByText(
      "1 attempts have an unknown outcome. They are never sent again automatically.",
    ),
  ).toBeVisible();
  await expect(
    panel.getByText("Waiting for Web disclosure approval by bob", {
      exact: false,
    }),
  ).toBeVisible();
  await page.screenshot({ path: info.outputPath("research-panel.png") });
  expect(errors).toEqual([]);
});

test("the active Run's research stays with its pinned Agent version when another is selected for the next Run", async ({
  page,
}) => {
  const { errors } = await research(page, { nextVersionWithoutResearch: true });
  await openThread(page);
  await page.getByLabel("Agent to run").selectOption("researcher@1.1.0");
  const panel = page.getByRole("region", { name: "Web research", exact: true });
  await expect(panel.getByRole("listitem")).toHaveCount(4);
  await expect(
    panel.getByText("This Run: searches 3/20", { exact: false }),
  ).toBeVisible();
  await expect(
    panel.getByText("Waiting for Web disclosure approval by bob", {
      exact: false,
    }),
  ).toBeVisible();
  // The selected version binds no research, so it has no readiness to show.
  await expect(
    page.getByRole("region", {
      name: "Web research for the next Run",
      exact: true,
    }),
  ).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("a pasted URL can be offered for reading without approving its disclosure", async ({
  page,
}) => {
  const { calls, errors } = await research(page);
  await openThread(page);
  const prompt =
    "Summarize https://example.com/notes and compare https://example.org/page.";
  await page.getByLabel("Next instruction").fill(prompt);
  const chips = page.getByRole("group", { name: "Read this page" });
  await expect(chips.getByRole("button")).toHaveText([
    "Read this page: https://example.com/notes",
    "Read this page: https://example.org/page",
  ]);
  await expect(
    chips.getByText("each page still needs its own Web disclosure approval", {
      exact: false,
    }),
  ).toBeVisible();
  // Without a selected chip nothing is requested for reading.
  await page.getByRole("button", { name: "Queue next Run" }).click();
  await expect
    .poll(() => calls.filter((call) => call.path.endsWith("/queue")).length)
    .toBe(1);
  const queued = calls.find((call) => call.path.endsWith("/queue"))!;
  expect(queued.body.read_pages).toBeUndefined();
  expect(queued.body.description).toBe(prompt);

  await page.getByLabel("Next instruction").fill(prompt);
  const first = chips.getByRole("button", {
    name: "Read this page: https://example.com/notes",
  });
  await first.click();
  await expect(first).toHaveAttribute("aria-pressed", "true");
  await page.getByRole("button", { name: "Steer active Run" }).click();
  await expect
    .poll(() => calls.find((call) => call.path.endsWith("/steer"))?.body)
    .toMatchObject({
      content: prompt,
      read_pages: ["https://example.com/notes"],
    });
  await expect(
    page.getByText("Asked the Run to read: https://example.com/notes"),
  ).toBeVisible();
  // Asking to read approved nothing: no decision was sent.
  expect(calls.some((call) => call.path.endsWith("/decide"))).toBe(false);
  expect(errors).toEqual([]);
});

test("a Run admitted without Page reading is not steered with pages, whatever is approved now", async ({
  page,
}) => {
  const { calls, errors } = await research(page, {
    admittedPageReading: false,
  });
  await openThread(page);
  await page
    .getByLabel("Next instruction")
    .fill("Summarize https://example.com/notes.");
  const chip = page
    .getByRole("group", { name: "Read this page" })
    .getByRole("button", { name: "Read this page: https://example.com/notes" });
  await chip.click();
  await expect(chip).toHaveAttribute("aria-pressed", "true");
  // The queued Run's version can read it, the active Run cannot.
  await expect(
    page.getByRole("button", { name: "Steer active Run" }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Queue next Run" }),
  ).toBeEnabled();
  expect(calls.some((call) => call.path.endsWith("/steer"))).toBe(false);
  expect(errors).toEqual([]);
});

test("an approver decides Web disclosures and revokes Run grants from the global inbox; denied and expired ones never send", async ({
  page,
}, info) => {
  const { calls, errors } = await research(page);
  await openThread(page);
  await page.getByRole("button", { name: /Decisions/ }).click();
  const inbox = page.getByRole("region", {
    name: "Approvals awaiting you",
    exact: true,
  });
  const search = inbox.getByRole("article", { name: "Web search disclosure" });
  for (const text of [
    "api.search.brave.com",
    "research.brave@1",
    "This Deployment's research.brave@1 account",
    "$0.0050 (estimate, not an invoice)",
    "researcher@1.0.0",
    "rust release",
  ])
    await expect(
      search.getByText(text, { exact: false }).first(),
    ).toBeVisible();
  await search.getByText("Exact content sent").click();
  await expect(search.getByText(searchContent)).toBeVisible();
  await page.screenshot({ path: info.outputPath("disclosure-card.png") });

  // Allow for this Run, with its scope stated.
  await search.getByRole("checkbox").check();
  await expect(
    search.getByText(
      "Every later research.brave@1 search in this Run, including queries you have not seen, until new input or expiry",
      { exact: false },
    ),
  ).toBeVisible();
  await search.getByRole("button", { name: "Allow for this Run" }).click();
  await expect
    .poll(
      () =>
        calls.find((call) => call.path.includes("disclose-search/decide"))
          ?.body,
    )
    .toMatchObject({ choice: "allow_run", targets: ["api.search.brave.com"] });

  const pages = inbox.getByRole("article", { name: "Page reading disclosure" });
  await expect(pages).toHaveCount(2);
  await pages.first().getByRole("button", { name: "Deny" }).click();
  await expect
    .poll(
      () =>
        calls.find((call) => call.path.includes("disclose-page/decide"))?.body,
    )
    .toMatchObject({ choice: "deny" });
  await expect(pages).toHaveCount(1);
  await pages.getByRole("button", { name: "Allow once" }).click();
  await expect
    .poll(
      () =>
        calls.find((call) => call.path.includes("disclose-other-page/decide"))
          ?.body,
    )
    .toMatchObject({ choice: "allow_once", targets: null, expires_at: null });

  const given = page.getByRole("region", {
    name: "Run grants you gave",
    exact: true,
  });
  const grant = given.getByRole("article", {
    name: "Web disclosure Run grant",
  });
  await expect(
    grant.getByText("Every later research.brave@1 search in this Run", {
      exact: false,
    }),
  ).toBeVisible();
  await grant.getByRole("button", { name: "Revoke" }).click();
  await expect
    .poll(() =>
      calls.some(
        (call) => call.path === "/api/capabilities/grants/grant-search/revoke",
      ),
    )
    .toBe(true);
  await page.keyboard.press("Escape");

  // The requester's Thread shows denied and expired disclosures as such; its
  // list is open because one disclosure is still waiting.
  await expect(
    page.getByText("Denied by bob at", { exact: false }),
  ).toBeVisible();
  await expect(
    page.getByText("Expired. Nothing is sent.", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText(
      "Closed because this Run's research evidence was revoked. Nothing is sent.",
      { exact: true },
    ),
  ).toBeVisible();
  // Nobody may approve: shown apart from a denial, and never in an inbox.
  await expect(
    page.getByText(
      "Nobody in this Tenant may approve Web disclosures to example.com. Nothing is sent.",
      { exact: true },
    ),
  ).toBeVisible();
  // The Thread lists the Run's grant with its scope; its requester may revoke it.
  const threadGrant = page
    .getByRole("article", { name: "Web disclosure Run grant" })
    .filter({ hasText: "Approver: bob" });
  await expect(
    threadGrant.getByRole("button", { name: "Revoke" }),
  ).toBeVisible();
  // A disclosure approval itself is never revoked.
  for (const card of await page
    .getByRole("article", { name: /disclosure$/ })
    .all())
    await expect(card.getByRole("button", { name: "Revoke" })).toHaveCount(0);
  expect(errors).toEqual([]);
});
