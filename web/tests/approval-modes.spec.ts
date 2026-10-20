import { expect, test, type Page } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import { setup } from "./collaboration-fixture";

// UI contract fixtures; the order of evaluation and the Tenant's ceiling are
// verified by the Rust approval_preferences and execution_authorization suites.
type Mode = "manual" | "auto" | "dont_ask" | "bypass";
type Rule = {
  id: string;
  kind: string;
  target: "every" | { exact: string };
  effect: string;
  effective: boolean;
  created_at: string;
};
const modes: Mode[] = ["manual", "auto", "dont_ask", "bypass"];
const rule = (id: string, patch: Partial<Rule> = {}): Rule => ({
  id,
  kind: "tool_call",
  target: { exact: "tool:crm#invoke" },
  effect: "allow",
  effective: true,
  created_at: "2026-10-19T10:00:00Z",
  ...patch,
});

/** A person's Approval Mode and Rules in the Tenant `acme`, as the server serves them. */
async function approvals(
  page: Page,
  options: {
    allowed?: Mode[];
    bypassAcceptedAt?: string | null;
    rules?: Rule[];
    refuseMode?: Mode;
    failThreadWrite?: boolean;
  } = {},
) {
  const allowed = options.allowed ?? ["manual", "dont_ask"];
  const state = {
    default_mode: "manual" as Mode,
    answer_deadline_seconds: 300 as number | null,
    bypass_accepted_at: options.bypassAcceptedAt ?? (null as string | null),
    revision: 3,
    rules: options.rules ?? [],
    thread: null as Mode | null,
  };
  const calls = {
    preferences: [] as Record<string, unknown>[],
    rules: [] as Record<string, unknown>[],
    deleted: [] as string[],
    thread: [] as Record<string, unknown>[],
  };
  const choices = () =>
    modes.map((mode) => ({ mode, allowed: allowed.includes(mode) }));
  const preferences = () => ({
    default_mode: state.default_mode,
    answer_deadline_seconds: state.answer_deadline_seconds,
    bypass_accepted_at: state.bypass_accepted_at,
    revision: state.revision,
    modes: choices(),
  });
  await page.route("**/api/tenants/acme/approval-preferences/mine", (route) => {
    const request = route.request();
    if (request.method() === "PUT") {
      const body = request.postDataJSON();
      calls.preferences.push(body);
      if (body.default_mode === options.refuseMode)
        return route.fulfill({
          status: 400,
          json: { error: "Auto needs the Tenant's approval Decider" },
        });
      state.default_mode = body.default_mode;
      state.answer_deadline_seconds = body.answer_deadline_seconds ?? null;
      if (body.accept_bypass && body.default_mode === "bypass")
        state.bypass_accepted_at ??= "2026-10-20T00:00:00Z";
      state.revision += 1;
    }
    return route.fulfill({ json: preferences() });
  });
  await page.route("**/api/tenants/acme/approval-rules/mine", (route) => {
    const request = route.request();
    if (request.method() === "POST") {
      const body = request.postDataJSON();
      calls.rules.push(body);
      const exact = body.target.exact as string | undefined;
      if (body.kind === "tool_call" && exact && !exact.startsWith("tool:"))
        return route.fulfill({
          status: 400,
          json: { error: "the target is not a valid ToolCall target" },
        });
      const created = rule(`rule-${calls.rules.length}`, body);
      state.rules.push({ ...created, effective: true });
      return route.fulfill({ status: 201, json: created });
    }
    return route.fulfill({ json: { rules: state.rules } });
  });
  await page.route("**/api/tenants/acme/approval-rules/mine/*", (route) => {
    const id = new URL(route.request().url()).pathname.split("/").at(-1)!;
    calls.deleted.push(id);
    state.rules = state.rules.filter((value) => value.id !== id);
    return route.fulfill({ status: 204 });
  });
  await page.route(
    "**/api/workspaces/workspace-one/threads/*/approval-mode",
    (route) => {
      const request = route.request();
      if (request.method() === "PUT") {
        const body = request.postDataJSON();
        calls.thread.push(body);
        if (options.failThreadWrite)
          return route.fulfill({
            status: 400,
            json: {
              error: "accept the risk of Bypass in your preferences first",
            },
          });
        state.thread = body.mode;
      }
      return route.fulfill({
        json: {
          mode: state.thread,
          default_mode: state.default_mode,
          modes: choices(),
        },
      });
    },
  );
  return { state, calls };
}

const radio = (page: Page, name: string) =>
  page.getByRole("radio", { name, exact: true });

test("Approvals settings offer the modes the Tenant's policy allows and keep the Answer Deadline", async ({
  page,
}) => {
  const { errors } = await setup(page, { subject: true });
  const { calls } = await approvals(page);
  await page.goto("/settings?view=approvals");

  await expect(
    page.getByRole("heading", { name: "Default Approval Mode" }),
  ).toBeVisible();
  await expect(radio(page, "Manual")).toBeChecked();
  await expect(radio(page, "Manual")).toHaveAccessibleDescription(
    /asked to decide/,
  );
  await expect(radio(page, "Don't ask")).toHaveAccessibleDescription(
    /never waits/,
  );
  for (const withheld of ["Auto", "Bypass"]) {
    await expect(radio(page, withheld)).toBeDisabled();
    await expect(radio(page, withheld)).toHaveAccessibleDescription(
      /Your Tenant's policy does not allow this mode\./,
    );
  }

  await radio(page, "Don't ask").click();
  await expect(radio(page, "Don't ask")).toBeChecked();
  await expect(
    page.getByRole("status").filter({ hasText: "Saved" }),
  ).toBeVisible();
  expect(calls.preferences).toEqual([
    {
      expected_revision: 3,
      default_mode: "dont_ask",
      answer_deadline_seconds: 300,
      accept_bypass: false,
    },
  ]);
  expect(errors).toEqual([]);
});

test("the server's reason shows when a mode cannot be saved", async ({
  page,
}) => {
  await setup(page, { subject: true });
  await approvals(page, {
    allowed: ["manual", "auto", "dont_ask"],
    refuseMode: "auto",
  });
  await page.goto("/settings?view=approvals");
  await radio(page, "Auto").click();
  await expect(page.getByRole("alert")).toHaveText(
    "Auto needs the Tenant's approval Decider",
  );
  await expect(radio(page, "Manual")).toBeChecked();
});

test("the first choice of Bypass explains it and records the person's acceptance", async ({
  page,
}) => {
  await setup(page, { subject: true });
  const { calls } = await approvals(page, {
    allowed: ["manual", "dont_ask", "bypass"],
  });
  await page.goto("/settings?view=approvals");

  await radio(page, "Bypass").click();
  const dialog = page.getByRole("dialog", { name: "Use Bypass?" });
  await expect(dialog).toContainText("without asking you");
  await expect(dialog).toContainText("Deny and ask rules still apply");
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(radio(page, "Manual")).toBeChecked();
  expect(calls.preferences).toEqual([]);

  await radio(page, "Bypass").click();
  await dialog
    .getByRole("button", { name: "I accept the risk, use Bypass" })
    .click();
  await expect(radio(page, "Bypass")).toBeChecked();
  expect(calls.preferences).toEqual([
    {
      expected_revision: 3,
      default_mode: "bypass",
      answer_deadline_seconds: 300,
      accept_bypass: true,
    },
  ]);

  // The risk is accepted once: choosing Bypass again does not ask again.
  await radio(page, "Manual").click();
  await radio(page, "Bypass").click();
  await expect(dialog).toHaveCount(0);
  await expect(radio(page, "Bypass")).toBeChecked();
  expect(calls.preferences.at(-1)).toMatchObject({
    default_mode: "bypass",
    accept_bypass: false,
  });
});

test("Approval Rules are listed, added with inline server errors, and deleted", async ({
  page,
}) => {
  await setup(page, { subject: true });
  const { calls } = await approvals(page, {
    rules: [
      rule("allow-crm"),
      rule("beyond", {
        kind: "outbound",
        target: { exact: "https://example.com" },
        effective: false,
      }),
      rule("deny-all", {
        kind: "page_reading",
        target: "every",
        effect: "deny",
      }),
    ],
  });
  await page.goto("/settings?view=approvals");

  const row = (text: string) =>
    page.getByRole("listitem").filter({ hasText: text });
  await expect(row("tool:crm#invoke")).toContainText("Tool call");
  await expect(row("tool:crm#invoke")).toContainText("Allow");
  await expect(row("tool:crm#invoke")).not.toContainText("Disabled");
  await expect(row("https://example.com")).toContainText(
    "Disabled by your Tenant's policy",
  );
  await expect(row("Every target")).toContainText("Page reading");
  await expect(row("Every target")).toContainText("Deny");

  // The shape a kind expects is explained before it is entered.
  await page.getByLabel("Kind", { exact: true }).selectOption("outbound");
  await expect(page.getByText("An origin, as https://host")).toBeVisible();
  await page.getByLabel("Kind", { exact: true }).selectOption("tool_call");
  await expect(page.getByLabel("Target", { exact: true })).toHaveAttribute(
    "placeholder",
    "tool:crm#invoke",
  );

  await page.getByRole("button", { name: "Add rule" }).click();
  await expect(page.getByRole("alert")).toHaveText(
    "Enter a target, or choose every target of this kind.",
  );
  expect(calls.rules).toEqual([]);

  await page.getByLabel("Target", { exact: true }).fill("crm");
  await page.getByLabel("Effect", { exact: true }).selectOption("ask");
  await page.getByRole("button", { name: "Add rule" }).click();
  await expect(page.getByRole("alert")).toHaveText(
    "the target is not a valid ToolCall target",
  );

  await page.getByLabel("Target", { exact: true }).fill("tool:wiki#read");
  await page.getByRole("button", { name: "Add rule" }).click();
  await expect(row("tool:wiki#read")).toContainText("Ask");
  expect(calls.rules.at(-1)).toEqual({
    kind: "tool_call",
    effect: "ask",
    target: { exact: "tool:wiki#read" },
  });

  await page.getByLabel("Kind", { exact: true }).selectOption("web_search");
  await page.getByLabel("Effect", { exact: true }).selectOption("deny");
  await page
    .getByRole("checkbox", { name: /Every target of this kind/ })
    .check();
  await expect(page.getByLabel("Target", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Add rule" }).click();
  await expect(
    row("Every target").filter({ hasText: "Web search" }),
  ).toBeVisible();
  expect(calls.rules.at(-1)).toEqual({
    kind: "web_search",
    effect: "deny",
    target: "every",
  });

  await page
    .getByRole("button", { name: "Delete rule: Tool call, tool:crm#invoke" })
    .click();
  await expect(row("tool:crm#invoke")).toHaveCount(0);
  expect(calls.deleted).toEqual(["allow-crm"]);
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("Approvals settings read in Japanese and are absent for a Deployment Administrator", async ({
  page,
}) => {
  await setup(page, { subject: true, locale: "ja-JP" });
  await approvals(page);
  await page.goto("/settings?view=approvals");
  await expect(
    page.getByRole("heading", { name: "既定の Approval Mode" }),
  ).toBeVisible();
  await expect(
    page.getByRole("radio", { name: "Don't ask", exact: true }),
  ).toHaveAccessibleDescription(/Run は待機しません/);
  await expect(
    page.getByRole("radio", { name: "Bypass", exact: true }),
  ).toHaveAccessibleDescription(/許可されていません/);
  await expect(
    page.getByRole("heading", { name: "Approval Rule" }),
  ).toBeVisible();
});

test("a Deployment Administrator has no Approvals section", async ({
  page,
}) => {
  await setup(page);
  await page.goto("/settings?view=approvals");
  await expect(
    page
      .getByRole("status")
      .filter({ hasText: "Approvals belong to a person" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Approvals", exact: true }),
  ).toHaveCount(0);
});

async function openThread(page: Page) {
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Reply in thread", exact: true })
    .first()
    .click();
  await expect(
    page.getByRole("heading", { name: "Thread", exact: true }),
  ).toBeVisible();
  return page.getByRole("combobox", { name: "Approval Mode for this Thread" });
}

test("a Thread's composer switches the Approval Mode and returns to the default", async ({
  page,
}) => {
  const { errors } = await setup(page, { subject: true });
  const { calls } = await approvals(page, {
    allowed: ["manual", "auto", "dont_ask", "bypass"],
  });
  const mode = await openThread(page);

  await expect(mode).toHaveValue("default");
  await expect(mode.locator("option:checked")).toHaveText("Default: Manual");
  // Bypass cannot be chosen here before its risk is accepted in Settings.
  await expect(mode.locator("option[value=bypass]")).toBeDisabled();
  await expect(mode.locator("option[value=bypass]")).toHaveText(
    "Bypass (accept in Settings first)",
  );

  await mode.selectOption("dont_ask");
  await expect(mode).toHaveValue("dont_ask");
  await expect(mode.locator("option:checked")).toHaveText("Don't ask");
  expect(calls.thread).toEqual([{ mode: "dont_ask" }]);

  await mode.selectOption("default");
  await expect(mode.locator("option:checked")).toHaveText("Default: Manual");
  expect(calls.thread.at(-1)).toEqual({ mode: null });
  expect(errors).toEqual([]);
});

test("a Thread's switcher lists modes the policy withholds as unavailable and reports a refusal", async ({
  page,
}) => {
  await setup(page, { subject: true });
  await approvals(page, {
    allowed: ["manual", "dont_ask"],
    bypassAcceptedAt: "2026-10-19T00:00:00Z",
    failThreadWrite: true,
  });
  const mode = await openThread(page);

  await expect(mode.locator("option[value=auto]")).toBeDisabled();
  await expect(mode.locator("option[value=auto]")).toHaveText(
    "Auto (not allowed by policy)",
  );
  await expect(mode.locator("option[value=dont_ask]")).toBeEnabled();

  await mode.selectOption("dont_ask");
  await expect(page.getByRole("alert")).toContainText(
    "accept the risk of Bypass in your preferences first",
  );
  await expect(mode).toHaveValue("default");
});

test("a Deployment Administrator's Thread has no Approval Mode switcher", async ({
  page,
}) => {
  await setup(page);
  await approvals(page);
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Reply in thread", exact: true })
    .first()
    .click();
  await expect(
    page.getByRole("heading", { name: "Thread", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("combobox", { name: "Approval Mode for this Thread" }),
  ).toHaveCount(0);
});

test("the Thread switcher keeps a narrow composer free of horizontal scrolling", async ({
  page,
}) => {
  await setup(page, { subject: true });
  await approvals(page);
  await page.setViewportSize({ width: 390, height: 844 });
  const mode = await openThread(page);
  await expect(mode).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  const send = page.getByRole("button", { name: "Send reply", exact: true });
  const [select, button] = await Promise.all([
    mode.boundingBox(),
    send.boundingBox(),
  ]);
  expect(select!.x + select!.width).toBeLessThanOrEqual(button!.x);
});

const pending = (patch: Record<string, unknown> = {}) => ({
  id: "tool-approval",
  area_id: "area-one",
  run_id: "run-0",
  revision: 1,
  kind: "tool_call",
  state: "pending",
  requester: "alice",
  approver: "alice",
  viewer_is_approver: true,
  targets: ["tool:crm@1.0.0#invoke"],
  action: "tool.call_unapproved",
  expires_at: "2099-01-01T00:00:00Z",
  agent_id: "researcher",
  agent_version: "1.0.0",
  tool: "crm",
  version: "1.0.0",
  operation: "invoke",
  call: { id: "call-0", name: "lookup_account", arguments: { id: "acme" } },
  allow_always: true,
  ...patch,
});

async function inbox(page: Page, cards: unknown[]) {
  const decisions: Record<string, unknown>[] = [];
  await page.route("**/api/capabilities/approvals**", (route) => {
    const url = new URL(route.request().url());
    if (url.pathname !== "/api/capabilities/approvals") return route.fallback();
    const pendingOnly = url.searchParams.get("state") === "pending";
    return route.fulfill({
      json: { items: pendingOnly ? cards : [], next_cursor: null },
    });
  });
  await page.route("**/api/capabilities/approvals/*/decide", (route) => {
    decisions.push(route.request().postDataJSON());
    return route.fulfill({ json: {} });
  });
  return decisions;
}

test("an approval the person's own Run waits for offers Always allow, with no scope", async ({
  page,
}) => {
  const { errors } = await setup(page, { subject: true });
  const decisions = await inbox(page, [
    pending(),
    pending({ id: "not-yours", allow_always: false }),
  ]);
  await page.goto("/collaboration?channel=workspace-one");
  await page.getByRole("button", { name: /Decisions/ }).click();
  const cards = page
    .getByRole("region", { name: "Approvals awaiting you" })
    .getByRole("article", { name: "Tool call approval" });
  await expect(cards).toHaveCount(2);
  await expect(
    cards.nth(1).getByRole("button", { name: "Always allow" }),
  ).toHaveCount(0);

  const card = cards.first();
  await expect(card).toContainText(
    "adds an allow rule for it to your Approval Rules",
  );
  // Ticking "Allow for this Run" must not give Always allow a scope.
  await card.getByRole("checkbox").check();
  await card.getByRole("button", { name: "Always allow" }).click();
  await expect.poll(() => decisions.length).toBe(1);
  expect(decisions[0]).toMatchObject({
    choice: "allow_always",
    expected_revision: 1,
    targets: null,
    expires_at: null,
  });
  expect(errors).toEqual([]);
});

test("Allow for this Run still carries its targets and expiry", async ({
  page,
}) => {
  await setup(page, { subject: true });
  const decisions = await inbox(page, [pending()]);
  await page.goto("/collaboration?channel=workspace-one");
  await page.getByRole("button", { name: /Decisions/ }).click();
  const card = page.getByRole("article", { name: "Tool call approval" });
  await card.getByRole("checkbox").check();
  await card.getByRole("button", { name: "Allow for this Run" }).click();
  await expect.poll(() => decisions.length).toBe(1);
  expect(decisions[0]).toMatchObject({
    choice: "allow_run",
    targets: ["tool:crm@1.0.0#invoke"],
  });
  expect(typeof decisions[0].expires_at).toBe("string");
});

test("an approval given in advance says which rule or mode gave it", async ({
  page,
}) => {
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
  const given = (id: string, by: unknown) =>
    pending({
      id,
      state: "approved",
      allow_always: false,
      given_by: by,
      decided_by: "alice",
      decided_at: "2026-10-20T00:00:00Z",
    });
  await setup(page, { subject: true });
  await page.route("**/api/working-areas**", (route) =>
    new URL(route.request().url()).pathname === "/api/working-areas"
      ? route.fulfill({ json: { items: [area], next_cursor: null } })
      : route.fallback(),
  );
  await page.route("**/api/working-areas/*/session", (route) =>
    route.fulfill({
      json: {
        active_run_id: "run-0",
        last_run_id: "run-0",
        last_agent_version: "1.0.0",
        queue: [
          {
            run_id: "run-0",
            sequence: 1,
            phase: "THINKING",
            control: "ACTIVE",
          },
        ],
      },
    }),
  );
  await page.route("**/api/capabilities/approvals**", (route) => {
    if (
      new URL(route.request().url()).pathname !== "/api/capabilities/approvals"
    )
      return route.fallback();
    return route.fulfill({
      json: {
        items: [
          given("by-rule", { rule: "2f1c0b1e" }),
          given("by-bypass", { mode: "bypass" }),
          pending({ id: "by-person", state: "approved", allow_always: false }),
        ],
        next_cursor: null,
      },
    });
  });
  await openThread(page);
  await expect(page.getByRole("heading", { name: "Agent work" })).toBeVisible();
  // Nothing is pending, so the list starts closed.
  await page.getByText("Approvals and grants (3)").click();
  const cards = page.getByRole("article", { name: "Tool call approval" });
  await expect(cards).toHaveCount(3);
  await expect(cards.nth(0)).toContainText("Allowed by your rule");
  await expect(cards.nth(1)).toContainText("Allowed by Bypass");
  await expect(cards.nth(2)).not.toContainText("Allowed by your");
  await expect(cards.nth(2)).not.toContainText("Allowed by Bypass");
});

test("a Tenant Administrator with approval.read sees a member's mode and rules, read only", async ({
  page,
}) => {
  await installBearerDashboard(page, "fixture", {
    tenant: "alpha",
    name: "alice",
  });
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  const access = { kind: "subject", tenant: "alpha", subject: "alice" };
  let actions = ["mapping.read", "approval.read"];
  const requested: string[] = [];
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({ json: { access, node_id: "aidash://test" } });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access,
          node: {
            id: "aidash://test",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            capabilities: [],
            clusters: [],
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
          peers: [],
        },
      });
    if (path === "/api/mesh")
      return route.fulfill({ json: { nodes: [], errors: [] } });
    if (path === "/api/tenants/alpha/administration")
      return route.fulfill({
        json: {
          tenant: "alpha",
          actions,
          assignable_groups: [],
          policy_revision: 3,
        },
      });
    if (path === "/api/tenants/alpha/mappings")
      return route.fulfill({
        json: [
          {
            id: "carol-mapping",
            identity: {
              id: "identity-carol",
              verified_email: "carol@example.test",
              display_name: "Carol",
            },
            subject: "carol",
            enabled: true,
            revision: 1,
            groups: [],
            own_identity: false,
            own_subject: false,
          },
        ],
      });
    if (path === "/api/tenants/alpha/subjects/carol/approval-settings") {
      requested.push(path);
      return route.fulfill({
        json: {
          subject: "carol",
          default_mode: "dont_ask",
          answer_deadline_seconds: null,
          bypass_accepted_at: null,
          revision: 2,
          rules: [
            rule("carol-rule", {
              kind: "outbound",
              target: { exact: "https://example.com" },
              effect: "deny",
              effective: false,
            }),
          ],
        },
      });
    }
    return route.fulfill({ json: [] });
  });
  await page.goto("/trust?view=authorization");
  const member = page.getByRole("listitem").filter({ hasText: "carol" });
  await expect(member).toBeVisible();
  // Rules are fetched only for a member whose settings are opened.
  expect(requested).toEqual([]);
  await member.getByText("Approval settings", { exact: true }).click();
  await expect(member).toContainText("Default Approval Mode");
  await expect(member).toContainText("Don't ask");
  await expect(member).toContainText("Has not accepted Bypass");
  await expect(member).toContainText("https://example.com");
  await expect(member).toContainText("Disabled by your Tenant's policy");
  await expect(member).toContainText("Read only");
  await expect(member.getByRole("button", { name: /Delete rule/ })).toHaveCount(
    0,
  );
  expect(requested).toEqual([
    "/api/tenants/alpha/subjects/carol/approval-settings",
  ]);

  // Without approval.read the member offers no such view.
  actions = ["mapping.read"];
  await page.reload();
  await expect(
    page.getByRole("listitem").filter({ hasText: "carol" }),
  ).toBeVisible();
  await expect(
    page.getByText("Approval settings", { exact: true }),
  ).toHaveCount(0);
});
