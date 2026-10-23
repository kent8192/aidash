import { expect, test, type Page } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import { setup } from "./collaboration-fixture";
import {
  builtInApprovalDecider,
  defaultNode,
  registryVersion,
  routeRegistryVersions,
} from "./registry-fixture";

// UI contract fixtures; the order of evaluation and the Tenant's ceiling are
// verified by the Rust approval_preferences and execution_authorization suites.
type Mode = "manual" | "auto" | "dont_ask" | "bypass";
type Reason = "policy" | "not_accepted" | "decider_unapproved" | "no_decider";
type Rule = {
  id: string;
  kind: string;
  target: "every" | { exact: string };
  effect: string;
  effective: boolean;
  created_at: string;
};
const modes: Mode[] = ["manual", "auto", "dont_ask", "bypass"];
const BUILT_IN = { id: "aidash.approval-decider", version: "1.0.0" };
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
    /** Why Auto is held back although the Tenant's policy allows it. */
    auto?: "decider_unapproved" | "no_decider";
    /** The versions (`id@version`) that hold a Catalog Approval. */
    approvedDeciders?: string[];
    /** The stored Answer Deadline in seconds; `null` waits without limit. */
    answerDeadline?: number | null;
    /** Nodes where the chosen approval Decider holds a Catalog Approval. */
    approvedNodes?: string[];
  } = {},
) {
  const allowed = options.allowed ?? ["manual", "dont_ask"];
  const state = {
    default_mode: "manual" as Mode,
    answer_deadline_seconds:
      options.answerDeadline === undefined ? 300 : options.answerDeadline,
    bypass_accepted_at: options.bypassAcceptedAt ?? (null as string | null),
    revision: 3,
    rules: options.rules ?? [],
    thread: null as Mode | null,
    decider: {
      decider: { id: BUILT_IN.id, version: BUILT_IN.version },
      built_in: true,
      registered: options.auto !== "no_decider",
      approved: options.auto === undefined,
      revision: 0,
    },
  };
  const calls = {
    deciders: [] as Record<string, unknown>[],
    preferences: [] as Record<string, unknown>[],
    rules: [] as Record<string, unknown>[],
    deleted: [] as string[],
    thread: [] as Record<string, unknown>[],
  };
  // The reason the server reports for a mode: the policy first, then what the
  // mode itself still needs.
  const reasonOf = (mode: Mode): Reason | null =>
    !allowed.includes(mode)
      ? "policy"
      : mode === "auto"
        ? (options.auto ?? null)
        : mode === "bypass" && state.bypass_accepted_at === null
          ? "not_accepted"
          : null;
  const choices = (node: string = defaultNode.name) =>
    modes.map((mode) => {
      const reason =
        reasonOf(mode) ??
        (mode === "auto" &&
        options.approvedNodes &&
        !options.approvedNodes.includes(node)
          ? "decider_unapproved"
          : null);
      return {
        mode,
        allowed: reason === null || reason === "not_accepted",
        reason,
      };
    });
  const preferences = (node: string) => ({
    default_mode: state.default_mode,
    answer_deadline_seconds: state.answer_deadline_seconds,
    bypass_accepted_at: state.bypass_accepted_at,
    revision: state.revision,
    modes: choices(node),
  });
  await page.route(
    (url) => url.pathname === "/api/tenants/acme/approval-preferences/mine",
    (route) => {
      const request = route.request();
      const node =
        new URL(request.url()).searchParams.get("node") ?? defaultNode.name;
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
      return route.fulfill({ json: preferences(node) });
    },
  );
  await page.route(
    (url) => url.pathname === "/api/tenants/acme/approval-decider",
    (route) => {
      const request = route.request();
      const node =
        new URL(request.url()).searchParams.get("node") ?? defaultNode.name;
      if (request.method() === "PUT") {
        const body = request.postDataJSON();
        calls.deciders.push(body);
        if (body.expected_revision !== state.decider.revision)
          return route.fulfill({ status: 409, json: { error: "stale" } });
        const named = body.decider as { id: string; version: string } | null;
        state.decider = {
          decider: named ?? { id: BUILT_IN.id, version: BUILT_IN.version },
          built_in: named === null,
          registered: true,
          approved:
            named === null
              ? true
              : (options.approvedDeciders ?? []).includes(
                  `${named.id}@${named.version}`,
                ),
          revision: state.decider.revision + 1,
        };
      }
      return route.fulfill({
        json: {
          ...state.decider,
          approved:
            state.decider.approved &&
            (options.approvedNodes?.includes(node) ?? true),
        },
      });
    },
  );
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

const deadlinePanel = (page: Page) =>
  page.locator("section.panel").filter({
    has: page.getByRole("heading", { name: "Answer Deadline" }),
  });

test("the Answer Deadline is edited as a duration and saved with the preferences", async ({
  page,
}) => {
  const { errors } = await setup(page, { subject: true });
  const { calls } = await approvals(page, { allowed: ["manual", "dont_ask"] });
  await page.goto("/settings?view=approvals");

  const panel = deadlinePanel(page);
  await expect(panel).toContainText(
    "When the Agent asks you a question and you have not answered by then, the Run continues on its own judgment. It never applies to Completion Reviews or other requests.",
  );
  await expect(panel.getByRole("radio", { name: "After" })).toBeChecked();
  const amount = panel.getByRole("spinbutton", { name: "Duration" });
  const unit = panel.getByRole("combobox", { name: "Unit" });
  await expect(amount).toHaveValue("5");
  await expect(unit).toHaveValue("minutes");
  const save = panel.getByRole("button", { name: "Save Answer Deadline" });
  await expect(save).toBeDisabled();

  await amount.fill("2");
  await unit.selectOption("hours");
  await save.click();
  await expect(panel.getByRole("status")).toHaveText("Saved");
  expect(calls.preferences).toEqual([
    {
      expected_revision: 3,
      default_mode: "manual",
      answer_deadline_seconds: 7200,
      accept_bypass: false,
    },
  ]);
  await expect(save).toBeDisabled();

  await panel.getByRole("radio", { name: "None" }).click();
  await expect(amount).toHaveCount(0);
  await save.click();
  await expect(panel.getByRole("status")).toHaveText("Saved");
  expect(calls.preferences.at(-1)).toEqual({
    expected_revision: 4,
    default_mode: "manual",
    answer_deadline_seconds: null,
    accept_bypass: false,
  });

  await radio(page, "Don't ask").click();
  await expect(radio(page, "Don't ask")).toBeChecked();
  expect(calls.preferences.at(-1)).toEqual({
    expected_revision: 5,
    default_mode: "dont_ask",
    answer_deadline_seconds: null,
    accept_bypass: false,
  });
  expect(errors).toEqual([]);
});

test("no Answer Deadline is the default, and choosing one starts from five minutes", async ({
  page,
}) => {
  await setup(page, { subject: true });
  const { calls } = await approvals(page, { answerDeadline: null });
  await page.goto("/settings?view=approvals");

  const panel = deadlinePanel(page);
  await expect(panel.getByRole("radio", { name: "None" })).toBeChecked();
  await expect(panel.getByRole("spinbutton")).toHaveCount(0);
  await expect(
    panel.getByRole("button", { name: "Save Answer Deadline" }),
  ).toBeDisabled();

  await panel.getByRole("radio", { name: "After" }).click();
  await expect(panel.getByRole("spinbutton", { name: "Duration" })).toHaveValue(
    "5",
  );
  await panel.getByRole("button", { name: "Save Answer Deadline" }).click();
  await expect(panel.getByRole("status")).toHaveText("Saved");
  expect(calls.preferences).toEqual([
    {
      expected_revision: 3,
      default_mode: "manual",
      answer_deadline_seconds: 300,
      accept_bypass: false,
    },
  ]);
});

test("an Answer Deadline outside a minute to 30 days is explained and not sent", async ({
  page,
}) => {
  await setup(page, { subject: true });
  const { calls } = await approvals(page);
  await page.goto("/settings?view=approvals");

  const panel = deadlinePanel(page);
  const amount = panel.getByRole("spinbutton", { name: "Duration" });
  const unit = panel.getByRole("combobox", { name: "Unit" });
  const save = panel.getByRole("button", { name: "Save Answer Deadline" });
  const problem = panel.getByRole("alert");

  await amount.fill("0");
  await save.click();
  await expect(problem).toHaveText("Enter a number greater than zero.");
  await amount.fill("0.5");
  await save.click();
  await expect(problem).toHaveText("An Answer Deadline is at least 1 minute.");
  await unit.selectOption("days");
  await amount.fill("31");
  await save.click();
  await expect(problem).toHaveText("An Answer Deadline is at most 30 days.");
  await amount.fill("30");
  await expect(problem).toHaveCount(0);
  await save.click();
  await expect(panel.getByRole("status")).toHaveText("Saved");
  expect(calls.preferences).toEqual([
    {
      expected_revision: 3,
      default_mode: "manual",
      answer_deadline_seconds: 30 * 24 * 60 * 60,
      accept_bypass: false,
    },
  ]);
});

test("the Answer Deadline settings read in Japanese", async ({ page }) => {
  await setup(page, { subject: true, locale: "ja-JP" });
  const { calls } = await approvals(page, { answerDeadline: 7200 });
  await page.goto("/settings?view=approvals");

  const panel = page.locator("section.panel").filter({
    has: page.getByRole("heading", { name: "Answer Deadline" }),
  });
  await expect(panel).toContainText(
    "Run は自身の判断で続行します。Completion Review やその他のリクエストには適用されません。",
  );
  await expect(panel.getByRole("radio", { name: "経過後" })).toBeChecked();
  await expect(panel.getByRole("spinbutton", { name: "期間" })).toHaveValue(
    "2",
  );
  await expect(panel.getByRole("combobox", { name: "単位" })).toHaveValue(
    "hours",
  );
  await panel.getByRole("radio", { name: "なし" }).click();
  await panel.getByRole("button", { name: "Answer Deadline を保存" }).click();
  await expect(panel.getByRole("status")).toHaveText("保存しました");
  expect(calls.preferences.at(-1)?.answer_deadline_seconds).toBeNull();
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

async function openThread(page: Page, ja = false) {
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", {
      name: ja ? "スレッドで返信" : "Reply in thread",
      exact: true,
    })
    .first()
    .click();
  await expect(
    page.getByRole("heading", {
      name: ja ? "スレッド" : "Thread",
      exact: true,
    }),
  ).toBeVisible();
  return page.getByRole("combobox", {
    name: ja ? "この Thread の Approval Mode" : "Approval Mode for this Thread",
  });
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
  // Bypass cannot be chosen here before its risk is accepted as the default.
  await expect(mode.locator("option[value=bypass]")).toBeDisabled();
  await expect(mode.locator("option[value=bypass]")).toHaveText(
    "Bypass (choose it as default first)",
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

test("Sampling approval shows the server messages and grants only the pinned server", async ({
  page,
}) => {
  const { errors } = await setup(page, { subject: true });
  const digest = `sha256:${"ab".repeat(32)}`;
  const target = `sampling:${JSON.stringify(["researcher", "reports", "writer", digest, "https://mcp.example.com"])}`;
  const decisions = await inbox(page, [
    pending({
      kind: "sampling",
      tool: null,
      version: null,
      action: "sampling.request_unapproved",
      targets: [target],
      sampling_target: {
        agent: "researcher",
        plugin: "reports",
        server: "writer",
        digest,
        origin: "https://mcp.example.com",
      },
      sampling: {
        system_prompt: "Summarize only this server request",
        messages: [{ role: "user", text: "Server report draft" }],
        max_tokens: 64,
      },
    }),
  ]);
  await page.goto("/collaboration?channel=workspace-one");
  await page.getByRole("button", { name: /Decisions/ }).click();
  const card = page.getByRole("article", {
    name: "Sampling approval",
    exact: true,
  });
  await expect(card).toContainText("Summarize only this server request");
  await expect(card).toContainText("Server report draft");
  await expect(card).toContainText("includeContext: none");
  await expect(card).toContainText("Run's model and budget");
  await expect(card).toContainText(digest);
  await card.getByRole("checkbox").check();
  await card
    .getByRole("button", { name: "Allow for this Run", exact: true })
    .click();
  await expect.poll(() => decisions.length).toBe(1);
  expect(decisions[0]).toMatchObject({
    choice: "allow_run",
    targets: [target],
  });
  expect(errors).toEqual([]);
});

/** A Thread whose Run shows `items` among its approvals and grants. */
async function runApprovals(page: Page, items: unknown[], ja = false) {
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
    return route.fulfill({ json: { items, next_cursor: null } });
  });
  await openThread(page, ja);
  await expect(
    page.getByRole("heading", { name: ja ? "Agent の作業" : "Agent work" }),
  ).toBeVisible();
  // Nothing is pending, so the list starts closed.
  await page
    .getByText(
      `${ja ? "承認と接続許可" : "Approvals and grants"} (${items.length})`,
    )
    .click();
  return page.getByRole("article", {
    name: ja ? "ツール呼び出しの承認" : "Tool call approval",
  });
}

const given = (id: string, by: unknown) =>
  pending({
    id,
    state: "approved",
    allow_always: false,
    given_by: by,
    decided_by: "alice",
    decided_at: "2026-10-20T00:00:00Z",
  });

/** A call refused before anyone was asked: nobody approves it. */
const refused = (id: string, by: unknown, person = "alice") =>
  pending({
    id,
    state: "denied",
    allow_always: false,
    approver: null,
    viewer_is_approver: false,
    refused_by: by,
    decided_by: person,
    decided_at: "2026-10-20T00:00:00Z",
  });

test("an approval given in advance says which rule or mode gave it", async ({
  page,
}) => {
  await setup(page, { subject: true });
  const cards = await runApprovals(page, [
    given("by-rule", { rule: "2f1c0b1e" }),
    given("by-bypass", { mode: "bypass" }),
    given("by-auto", { mode: "auto", decision: "4d1e9b27" }),
    pending({ id: "by-person", state: "approved", allow_always: false }),
  ]);
  await expect(cards).toHaveCount(4);
  await expect(cards.nth(0)).toContainText("Allowed by your rule");
  await expect(cards.nth(1)).toContainText("Allowed by Bypass");
  await expect(cards.nth(2)).toContainText("Allowed by Auto");
  await expect(cards.nth(3)).not.toContainText("Allowed by");
});

test("a refused approval says what refused it, and why Auto blocked it", async ({
  page,
}) => {
  await setup(page, { subject: true });
  const block = (reason: string) => ({ mode: "auto", reason });
  const cards = await runApprovals(page, [
    refused("by-rule", { rule: "9a0b" }),
    refused("by-dont-ask", { mode: "dont_ask" }),
    refused("by-others-rule", { rule: "9a0c" }, "bob"),
    refused("exfiltration", block("data_exfiltration")),
    refused("destructive", block("destructive")),
    refused("beyond", block("beyond_request")),
    refused("injected", block("injected")),
    refused("boundary", block("boundary")),
    refused("unavailable", block("unavailable")),
  ]);
  const texts = [
    "Refused by your rule",
    "Refused: Don't ask",
    "Refused by bob's rule",
    "Blocked by Auto: data sent beyond your request",
    "Blocked by Auto: destroys or irreversibly changes things",
    "Blocked by Auto: goes beyond your request",
    "Blocked by Auto: driven by content the Run read",
    "Blocked by Auto: contradicts a boundary you stated",
    "Blocked by Auto: the Decider was unavailable",
  ];
  await expect(cards).toHaveCount(texts.length);
  for (const [index, text] of texts.entries())
    await expect(cards.nth(index)).toContainText(text);
});

test("what gave or refused an approval reads in Japanese", async ({ page }) => {
  await setup(page, { subject: true, locale: "ja-JP" });
  const cards = await runApprovals(
    page,
    [
      given("by-auto", { mode: "auto", decision: "4d1e9b27" }),
      refused("blocked", { mode: "auto", reason: "injected" }),
      refused("by-rule", { rule: "9a0b" }),
    ],
    true,
  );
  await expect(cards).toHaveCount(3);
  await expect(cards.nth(0)).toContainText("Auto により許可");
  await expect(cards.nth(1)).toContainText(
    "Auto がブロック: Run が読んだ内容に動かされている",
  );
  await expect(cards.nth(2)).toContainText("あなたのルールにより拒否");
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
          deployment: {
            id: "aidash://test",
            endpoint: "http://localhost",
            protocol_version: "0.1",
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
    if (path === "/api/tenants/alpha/nodes")
      return route.fulfill({
        json: [
          { ...defaultNode, address: `aidash://test/${defaultNode.name}` },
        ],
      });
    if (path === "/api/tenants/alpha/approval-decider")
      return route.fulfill({ json: builtInApprovalDecider });
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
          answer_deadline_seconds: 7200,
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
  await expect(member).toContainText("2 Hours");
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

const everyMode: Mode[] = ["manual", "auto", "dont_ask", "bypass"];
const BUILT_IN_KEY = `${BUILT_IN.id}@${BUILT_IN.version}`;

const decider = (id: string, version = "1.0.0", hook = "approval") => ({
  id,
  version,
  kind: "decider",
  name: { en: id === BUILT_IN.id ? "Approval Decider" : "Judge" },
  description: { en: "Judges approvals" },
  capabilities: [],
  tags: [],
  languages: [],
  skills: [],
  schema: {},
  config: {
    hook,
    routes: [
      { provider_contract: "typesafe.jev/1", model: "typesafe/jev-1.14" },
      {
        provider_contract: "openrouter.decisions/1",
        model: "cloudflare/clef",
        hosts: ["cloudflare", "deepinfra"],
      },
    ],
  },
});

test("a mode held back by the approval Decider says so and links to its Catalog Approval", async ({
  page,
}) => {
  await setup(page, { subject: true });
  await approvals(page, { allowed: everyMode, auto: "decider_unapproved" });
  await page.goto("/settings?view=approvals");

  await expect(radio(page, "Auto")).toBeDisabled();
  await expect(radio(page, "Auto")).toHaveAccessibleDescription(
    new RegExp(
      `\\(${BUILT_IN_KEY.replaceAll(".", "\\.")}\\) has no Catalog Approval on this Node`,
    ),
  );
  await expect(
    page.getByRole("link", { name: "Review its Catalog Approval" }),
  ).toHaveAttribute(
    "href",
    `/settings?view=registry&focus=${encodeURIComponent(BUILT_IN_KEY)}`,
  );
  // The policy's own wording is kept for the policy.
  await expect(radio(page, "Manual")).toBeEnabled();
});

test("Auto without a registered approval Decider says it is not registered", async ({
  page,
}) => {
  await setup(page, { subject: true });
  await approvals(page, { allowed: everyMode, auto: "no_decider" });
  await page.goto("/settings?view=approvals");

  await expect(radio(page, "Auto")).toBeDisabled();
  await expect(radio(page, "Auto")).toHaveAccessibleDescription(
    /is not registered as an approval Decider/,
  );
  await expect(
    page.getByRole("link", { name: "Review its Catalog Approval" }),
  ).toHaveCount(0);
});

test("Bypass explains that its risk is accepted when first chosen as the default", async ({
  page,
}) => {
  await setup(page, { subject: true });
  await approvals(page, { allowed: everyMode });
  await page.goto("/settings?view=approvals");

  await expect(radio(page, "Bypass")).toBeEnabled();
  await expect(radio(page, "Bypass")).toHaveAccessibleDescription(
    /You accept the risk of Bypass when you first choose it as your default\./,
  );
  await expect(radio(page, "Auto")).toBeEnabled();
  await expect(radio(page, "Auto")).not.toHaveAccessibleDescription(
    /Catalog Approval/,
  );
});

test("the Settings reasons read in Japanese", async ({ page }) => {
  await setup(page, { subject: true, locale: "ja-JP" });
  await approvals(page, { allowed: everyMode, auto: "decider_unapproved" });
  await page.goto("/settings?view=approvals");

  await expect(
    page.getByRole("radio", { name: "Auto", exact: true }),
  ).toHaveAccessibleDescription(/Catalog Approval を受けていません/);
  await expect(
    page.getByRole("radio", { name: "Bypass", exact: true }),
  ).toHaveAccessibleDescription(/初めて既定のモードとして選ぶとき/);
  await expect(
    page.getByRole("link", { name: "Catalog Approval を確認する" }),
  ).toBeVisible();
});

test("the Thread switcher names why Auto and Bypass cannot be chosen", async ({
  page,
}) => {
  await setup(page, { subject: true });
  await approvals(page, { allowed: everyMode, auto: "decider_unapproved" });
  const mode = await openThread(page);

  await expect(mode.locator("option[value=auto]")).toBeDisabled();
  await expect(mode.locator("option[value=auto]")).toHaveText(
    "Auto (Decider not approved)",
  );
  await expect(mode.locator("option[value=bypass]")).toHaveText(
    "Bypass (choose it as default first)",
  );
});

test("the Thread switcher says when no approval Decider is registered", async ({
  page,
}) => {
  await setup(page, { subject: true });
  await approvals(page, { allowed: everyMode, auto: "no_decider" });
  const mode = await openThread(page);
  await expect(mode.locator("option[value=auto]")).toHaveText(
    "Auto (no approval Decider)",
  );
});

test("the link opens the Catalog Approval of the Tenant's approval Decider", async ({
  page,
}) => {
  await setup(page, { subject: true });
  await routeRegistryVersions(page, () => [
    registryVersion(decider(BUILT_IN.id), { approval: null, history: [] }),
    registryVersion(decider("tenant.judge")),
  ]);
  await approvals(page, { allowed: everyMode, auto: "decider_unapproved" });
  await page.goto("/settings?view=approvals");

  await page.getByRole("link", { name: "Review its Catalog Approval" }).click();
  await expect(page).toHaveURL(/\/settings\?view=registry&focus=/);
  const section = page.getByRole("region", { name: "Catalog Approval" });
  await expect(section).toBeVisible();
  await expect(section).toContainText("Not approved");
  await expect(section).toContainText("typesafe/jev-1.14");
  await expect(section).toContainText("cloudflare, deepinfra");
});

test("a focus that names no version opens nothing", async ({ page }) => {
  await setup(page, { subject: true });
  await routeRegistryVersions(page, () => [
    registryVersion(decider("tenant.judge")),
  ]);
  await page.goto("/settings?view=registry&focus=missing%401.0.0");
  await expect(page.getByRole("heading", { name: "Judge" })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

const panelOf = (page: Page, title: string) =>
  page.locator("section.panel").filter({
    has: page.getByRole("heading", { name: title, exact: true }),
  });

async function deciderPanel(
  page: Page,
  actions: string[],
  options: Parameters<typeof approvals>[1] = {},
) {
  const errors = (await setup(page, { subject: true, tenantActions: actions }))
    .errors;
  await routeRegistryVersions(page, () => [
    registryVersion(decider(BUILT_IN.id), { approval: null, history: [] }),
    registryVersion(decider("tenant.judge")),
    registryVersion(decider("tenant.compaction", "1.0.0", "compaction")),
  ]);
  const { calls } = await approvals(page, options);
  await page.goto("/trust?view=authorization");
  const panel = panelOf(page, "Approval Decider");
  return { panel, calls, errors };
}

test("Tenant administration shows the chosen Decider, its approval and where it sends Run state", async ({
  page,
}) => {
  const { panel, errors } = await deciderPanel(page, ["registry.create"], {
    auto: "decider_unapproved",
  });
  await expect(panel).toContainText(BUILT_IN_KEY);
  await expect(panel).toContainText(
    "Built-in, used until the Tenant names another",
  );
  await expect(panel).toContainText("Not approved, so Auto is unavailable");
  await expect(panel).toContainText("typesafe");
  await expect(panel).toContainText("cloudflare, deepinfra");
  await expect(
    panel.getByRole("link", { name: "Review its Catalog Approval" }),
  ).toBeVisible();
  // Without tenant.update the choice is read only.
  await expect(panel.getByRole("combobox")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("a holder of tenant.update chooses a Decider of hook approval or resets to the built-in", async ({
  page,
}) => {
  const { panel, calls } = await deciderPanel(page, ["tenant.update"], {
    approvedDeciders: ["tenant.judge@1.0.0"],
  });
  const select = panel.getByRole("combobox", {
    name: "Approval Decider for Auto",
  });
  await expect(select).toHaveValue("");
  // Only the Tenant's own Deciders of hook approval are offered.
  await expect(select.locator("option")).toHaveText([
    "Built-in",
    "Judge · tenant.judge@1.0.0",
  ]);

  await select.selectOption("tenant.judge@1.0.0");
  await expect(panel).toContainText("Chosen by this Tenant");
  await expect(panel).toContainText("tenant.judge@1.0.0");
  await expect(panel).toContainText("Approved");
  await expect(select).toHaveValue("tenant.judge@1.0.0");
  expect(calls.deciders).toEqual([
    { expected_revision: 0, decider: { id: "tenant.judge", version: "1.0.0" } },
  ]);

  await select.selectOption("");
  await expect(panel).toContainText(
    "Built-in, used until the Tenant names another",
  );
  expect(calls.deciders.at(-1)).toEqual({
    expected_revision: 1,
    decider: null,
  });
});

test("a Decider the Registry does not hold as an approval Decider says Auto is unavailable", async ({
  page,
}) => {
  const { panel } = await deciderPanel(page, ["registry.create"], {
    auto: "no_decider",
  });
  await expect(panel).toContainText(
    "The Registry holds no Decider of hook approval under this name",
  );
  await expect(panel.getByRole("link")).toHaveCount(0);
});

test("the Approval Decider panel reads in Japanese", async ({ page }) => {
  await setup(page, {
    subject: true,
    locale: "ja-JP",
    tenantActions: ["tenant.update"],
  });
  await approvals(page, { auto: "decider_unapproved" });
  await page.goto("/trust?view=authorization");
  const panel = panelOf(page, "承認 Decider");
  await expect(panel).toContainText("未承認のため、Auto は利用できません");
  await expect(
    panel.getByRole("combobox", { name: "Auto の承認 Decider" }),
  ).toBeVisible();
});

test("Approval Mode and Decider availability follow the selected Node", async ({
  page,
}) => {
  const { errors } = await setup(page, {
    subject: true,
    tenantActions: ["tenant.update"],
  });
  const sibling = "sibling-node";
  await page.route("**/api/tenants/acme/nodes", (route) =>
    route.fulfill({
      json: [
        { ...defaultNode, address: `aidash://home/${defaultNode.name}` },
        {
          ...defaultNode,
          name: sibling,
          label: "Lab",
          default: false,
          address: `aidash://home/${sibling}`,
        },
      ],
    }),
  );
  await page.route(
    (url) => url.pathname === "/api/registry/versions",
    (route) => {
      const approved =
        new URL(route.request().url()).searchParams.get("node") ===
        defaultNode.name;
      return route.fulfill({
        json: {
          versions: [BUILT_IN.id, "tenant.judge"].map((id) =>
            registryVersion(
              decider(id),
              approved ? {} : { approval: null, history: [] },
            ),
          ),
          next_offset: null,
        },
      });
    },
  );
  const { calls } = await approvals(page, {
    allowed: everyMode,
    approvedNodes: [defaultNode.name],
    approvedDeciders: ["tenant.judge@1.0.0"],
  });
  const requests: string[] = [];
  let release!: () => void;
  const siblingRead = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route(
    (url) =>
      url.pathname === "/api/tenants/acme/approval-preferences/mine" ||
      url.pathname === "/api/tenants/acme/approval-decider",
    async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      const node = url.searchParams.get("node");
      const kind = url.pathname.endsWith("/approval-decider")
        ? "decider"
        : "preferences";
      requests.push(`${request.method()}:${kind}:${node}`);
      if (
        request.method() === "GET" &&
        kind === "preferences" &&
        node === sibling
      )
        await siblingRead;
      await route.fallback();
    },
  );
  const selectNode = async (
    name: string,
    view: "approvals" | "trust" = "approvals",
  ) => {
    await page
      .getByRole("button", { name: "Account settings", exact: true })
      .click();
    await page.getByTestId("node-selector").selectOption(name);
    await page.keyboard.press("Escape");
    // Leaving a Workspace returns to Request. Reopen the view within the SPA
    // so its previous Node's query cache remains part of this regression.
    if (view === "approvals") {
      await page.getByRole("button", { name: "Settings", exact: true }).click();
      await page
        .getByRole("button", { name: "Approvals", exact: true })
        .click();
    } else {
      await page.getByRole("button", { name: "Trust", exact: true }).click();
      const access = page.getByRole("button", {
        name: "Access and accounts",
        exact: true,
      });
      if ((await access.getAttribute("aria-expanded")) === "false")
        await access.click();
    }
  };

  await page.goto("/settings?view=approvals");
  await expect(radio(page, "Auto")).toBeEnabled();
  await selectNode(sibling);
  try {
    await expect.poll(() => requests).toContain(`GET:preferences:${sibling}`);
    // Until this Node answers, another Node's cached approval must not enable Auto.
    await expect(radio(page, "Auto")).toHaveCount(0);
  } finally {
    release();
  }
  await expect(radio(page, "Auto")).toBeDisabled();
  await expect(radio(page, "Auto")).toHaveAccessibleDescription(
    /has no Catalog Approval/,
  );
  await radio(page, "Don't ask").click();
  await expect(radio(page, "Don't ask")).toBeChecked();
  expect(requests).toContain(`PUT:preferences:${sibling}`);
  expect(requests).toContain(`GET:decider:${sibling}`);
  expect(calls.preferences).toHaveLength(1);
  const deadline = deadlinePanel(page);
  await deadline.getByRole("spinbutton", { name: "Duration" }).fill("10");
  await deadline.getByRole("button", { name: "Save Answer Deadline" }).click();
  await expect(deadline.getByRole("status")).toHaveText("Saved");
  await expect(radio(page, "Auto")).toBeDisabled();
  expect(
    requests.filter((value) => value === `PUT:preferences:${sibling}`),
  ).toHaveLength(2);

  await page.goto("/trust?view=authorization");
  const panel = panelOf(page, "Approval Decider");
  await expect(panel).toContainText("Not approved, so Auto is unavailable");
  await panel
    .getByRole("combobox", { name: "Approval Decider for Auto" })
    .selectOption("tenant.judge@1.0.0");
  await expect(panel).toContainText("Chosen by this Tenant");
  await expect(panel).toContainText("Not approved, so Auto is unavailable");
  expect(requests).toContain(`PUT:decider:${sibling}`);
  await selectNode(defaultNode.name, "trust");
  await expect(panel).not.toContainText("Not approved, so Auto is unavailable");
  await expect(panel).toContainText("Chosen by this Tenant");
  await expect
    .poll(() => requests)
    .toContain(`GET:decider:${defaultNode.name}`);
  await selectNode(sibling, "trust");
  await expect(panel).toContainText("Not approved, so Auto is unavailable");
  expect(errors).toEqual([]);
});
