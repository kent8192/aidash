import { test, expect, type Page } from "@playwright/test";
import { setup, fixture } from "./collaboration-fixture";

// UI contract fixtures; real execution/authorization is verified by the Rust
// core_capabilities integration suite against PostgreSQL and gVisor.
async function core(page: Page) {
  const fixture = await setup(page, {
    subject: true,
    coreCapabilities: true,
    coreVersion: true,
  });
  const calls: { path: string; body: Record<string, unknown> }[] = [];
  const area = {
    id: "area-one",
    thread_id: "thread-one",
    agent_id: "researcher",
    owner: "alice",
    state: "active",
    generation: 1,
    revision: 7,
  };
  const approvals = [
    {
      id: "pending",
      area_id: area.id,
      run_id: "run-0",
      revision: 1,
      kind: "approval",
      state: "pending",
      requester: "alice",
      viewer_is_approver: true,
      targets: ["https://example.org"],
      expires_at: "2099-01-01T00:00:00Z",
    },
    {
      id: "expired",
      area_id: area.id,
      run_id: "run-0",
      revision: 1,
      kind: "approval",
      state: "expired",
      requester: "alice",
      targets: ["https://expired.example.org"],
      expires_at: "2020-01-01T00:00:00Z",
    },
    {
      id: "blocked",
      area_id: area.id,
      run_id: "run-0",
      revision: 1,
      kind: "approval",
      state: "blocked",
      requester: "alice",
      targets: ["https://blocked.example.org"],
      expires_at: "2099-01-01T00:00:00Z",
    },
  ];
  let pythonCalls = 0;
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const body =
      request.method() === "POST" ? (request.postDataJSON() ?? {}) : {};
    const send = (json: unknown) => route.fulfill({ json });
    if (path === "/api/execution-scopes")
      return send({ items: [area], next_cursor: null });
    if (path === "/api/registry") {
      const entries = [
        {
          id: "researcher",
          version: "1.0.0",
          kind: "agent",
          name: { en: "Researcher" },
          config: { schema_version: 1, bindings: [], remove_default: [] },
        },
        {
          id: "skill-source",
          version: "1.0.0",
          kind: "source",
          name: { en: "Reviewed Skills" },
          config: {},
        },
        {
          id: "conversation-memory",
          version: "1.0.0",
          kind: "memory",
          name: { en: "Conversation memory" },
          config: {},
        },
        {
          id: "approved-shell",
          version: "1.0.0",
          kind: "bundle",
          name: { en: "Approved Shell package" },
          config: {},
        },
      ];
      return send(
        new URL(request.url()).searchParams.get("kind") === "agent"
          ? entries.filter((e) => e.kind === "agent")
          : entries,
      );
    }
    if (path === "/api/references")
      return send({
        items: [
          {
            reference_id: "ref-one",
            revision: 1,
            name: "broken.pdf",
            state: "ready",
            extraction_state: "malformed",
            digest: "b".repeat(64),
          },
        ],
        next_cursor: null,
      });
    if (path.endsWith("/session") && path !== "/api/session")
      return send({
        active_run_id: "run-0",
        last_run_id: "run-0",
        last_agent_version: "1.0.0",
      });
    if (path.endsWith("/core-operations"))
      return send({ items: [], next_cursor: null });
    if (path === "/api/capabilities/approvals")
      return send({ items: approvals, next_cursor: null });
    if (
      request.method() === "POST" &&
      (path.startsWith("/api/execution-scopes/") ||
        path.startsWith("/api/executions/") ||
        path.startsWith("/api/capabilities/") ||
        /^\/api\/threads\/[^/]+\/(?:scratch\/clear|agents\/[^/]+\/executions)$/.test(
          path,
        ) ||
        path.endsWith("/delete") ||
        path.includes("/capability-configuration"))
    ) {
      calls.push({ path, body });
      // A deleted Thread leaves the State, as the server tombstones it.
      const deleted = /^\/api\/threads\/([^/]+)\/delete$/.exec(path)?.[1];
      if (deleted) {
        fixture.deleteThread(deleted);
        return send({
          thread_id: deleted,
          state: "deleted",
        });
      }
      if (path.endsWith("/python")) {
        pythonCalls++;
        return send(
          pythonCalls === 1
            ? {
                status: "blocked",
                session_reset: true,
                session_id: "new-session",
                reset_reason: "idle_timeout",
                error: { code: "SESSION_RESET", message: "Acknowledge reset" },
              }
            : {
                status: "prepared",
                session_id: "new-session",
                operation_id: "python-one",
              },
        );
      }
      if (path.endsWith("/skills/list"))
        return send({
          skills: [
            {
              skill_id: "direct-one",
              name: "Analyze",
              description: "Read the CSV",
              origin: "upload:analysis/SKILL.md",
              digest: "skill-digest",
            },
          ],
          next_cursor: null,
        });
      if (path.endsWith("/skills/load"))
        return send({
          path: "SKILL.md",
          content: "Analyze the selected CSV.",
          files: [{ path: "scripts/check.py", size: 24 }],
        });
      if (path.endsWith("/skills/read"))
        return send({
          path: "scripts/check.py",
          content: "print('must not execute')",
        });
      if (path.endsWith("/decide"))
        approvals[0].state = String(
          body.choice === "deny" ? "denied" : "approved",
        );
      return send({ status: "completed", accepted_run_id: "run-0" });
    }
    return route.fallback();
  });
  return { ...fixture, calls };
}

test("Binding setup saves explicit Sources, Memory and Host packages in a new immutable version", async ({
  page,
}, info) => {
  const { errors } = await core(page);
  let saved: Record<string, unknown> | undefined;
  await page.route("**/api/agents/researcher/capabilities", (route) => {
    saved = route.request().postDataJSON();
    return route.fulfill({ json: { catalog_approval_required: true } });
  });
  await page.goto("/settings?view=workingFiles");
  const config = page.locator("details", {
    has: page.locator("summary", {
      hasText: "Agent capabilities, Skills and original references",
    }),
  });
  await config.locator("summary").click();
  await config
    .getByLabel("Source Agent version")
    .selectOption("researcher@1.0.0");
  await expect(config.getByLabel("Registering Node")).toHaveValue(
    "aidash://home",
  );
  await expect(config.getByLabel("Registering Node")).not.toBeEditable();
  await expect(
    config.getByText("workspace_read and human_request are always included.", {
      exact: false,
    }),
  ).toBeVisible();
  await expect(
    config.getByRole("checkbox", { name: "workspace_read", exact: true }),
  ).toHaveCount(0);
  await expect(
    config.getByRole("checkbox", { name: "human_request", exact: true }),
  ).toHaveCount(0);
  for (const operation of [
    "memory_recall",
    "memory_reflect",
    "memory_mutate",
  ]) {
    await expect(
      config.getByRole("checkbox", { name: operation, exact: true }),
    ).toBeVisible();
  }
  await expect(
    config.getByText("bundle: aidash://home/approved-shell@1.0.0", {
      exact: true,
    }),
  ).toHaveCount(0);
  for (const target of [
    "source:skill-source@1.0.0",
    "memory:conversation-memory@1.0.0",
    "bundle:approved-shell@1.0.0",
  ]) {
    await config.getByLabel("Item to add").selectOption(target);
    await config
      .getByRole("button", { name: "Add to this Agent", exact: true })
      .click();
  }
  await config
    .getByLabel("Member IDs (empty selects all)")
    .fill("shell_exec, shell_poll, shell_cancel");
  await config
    .getByRole("checkbox", { name: "task_delegate", exact: true })
    .uncheck();
  await config.getByLabel("New immutable version").fill("1.1.0");
  await config
    .getByRole("button", { name: "Save a new version", exact: true })
    .click();
  await expect(config.getByRole("status")).toHaveText(
    "Saved 1.1.0. It becomes available after catalog approval.",
  );
  expect(saved).toMatchObject({
    source_version: "1.0.0",
    new_version: "1.1.0",
    remove_default: ["task_delegate"],
    bindings: [
      {
        kind: "source",
        target: {
          registry_deployment: "aidash://home",
          id: "skill-source",
          version: "1.0.0",
        },
        narrow: {},
      },
      {
        kind: "memory",
        target: {
          registry_deployment: "aidash://home",
          id: "conversation-memory",
          version: "1.0.0",
        },
        narrow: {},
      },
      {
        kind: "bundle",
        target: {
          registry_deployment: "aidash://home",
          id: "approved-shell",
          version: "1.0.0",
        },
        narrow: {},
        members: ["shell_exec", "shell_poll", "shell_cancel"],
      },
    ],
  });
  for (const field of [
    "core_capabilities",
    "tools",
    "skills",
    "allow_memory_write",
    "skill_attachments",
    "reference_attachments",
  ])
    expect(saved).not.toHaveProperty(field);
  await config
    .getByRole("button", { name: "Remove from this Agent", exact: true })
    .nth(0)
    .click();
  await config.getByLabel("New immutable version").fill("1.2.0");
  await config
    .getByRole("button", { name: "Save a new version", exact: true })
    .click();
  await expect(config.getByRole("status")).toHaveText(
    "Saved 1.2.0. It becomes available after catalog approval.",
  );
  expect(saved?.bindings).toHaveLength(2);
  await page.screenshot({
    path: info.outputPath("binding-setup.png"),
    fullPage: true,
  });
  expect(errors).toEqual([]);
});

test("failed-extraction originals stay private and downloadable", async ({
  page,
}) => {
  const { errors } = await core(page);
  const writes: { path: string; body: unknown }[] = [];
  let uploaded = false;
  await page.route("**/api/references**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (route.request().method() === "POST") {
      writes.push({ path, body: route.request().postDataJSON() });
      if (path.endsWith("/commit")) uploaded = true;
      return route.fulfill({
        json: {
          reference_id: "uploaded-original",
          state: "uploading",
          uploaded_bytes: 0,
        },
      });
    }
    return route.fulfill({
      json: {
        items: uploaded
          ? [
              {
                reference_id: "uploaded-original",
                revision: 2,
                state: "ready",
                name: "broken.pdf",
                extraction_state: "malformed",
                digest: "b".repeat(64),
              },
            ]
          : [],
        next_cursor: null,
      },
    });
  });
  await page.goto("/settings?view=workingFiles");
  await page.getByLabel("Add PDF, Excel or text (up to 10 MiB)").setInputFiles({
    name: "broken.pdf",
    mimeType: "application/pdf",
    buffer: Buffer.from("%PDF-broken"),
  });
  await expect(
    page.getByText("ready · malformed", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Download original", exact: true }),
  ).toBeEnabled();
  expect(writes.map((w) => w.path)).toEqual([
    "/api/references/uploads",
    "/api/references/uploaded-original/chunks",
    "/api/references/uploaded-original/commit",
  ]);
  expect(writes[1].body).toEqual({
    offset: 0,
    data: Buffer.from("%PDF-broken").toString("base64"),
  });
  expect(errors).toEqual([]);
});

/** Opens the Agent work of the Thread `thread-one`, which #1008 moved from
 *  the reply-thread pane to the Thread's Results tab. */
async function openThread(page: Page) {
  await page.goto("/collaboration?channel=thread-one");
  await page.getByRole("tab", { name: /^Results/ }).click();
  await expect(page.getByRole("heading", { name: "Agent work" })).toBeVisible();
}

test("Live preview background status is quiet when absent or unavailable", async ({
  page,
}) => {
  const { errors } = await core(page);
  let status = 404;
  let requests = 0;
  await page.route("**/api/executions/run-0/live-preview", (route) => {
    requests++;
    return route.fulfill({
      status,
      json: { error: status === 404 ? "not_found" : "unavailable" },
    });
  });
  await openThread(page);
  await expect.poll(() => requests).toBeGreaterThan(0);
  await expect(page.getByText("Live preview", { exact: true })).toHaveCount(0);

  status = 503;
  await expect(
    page.getByText("Live preview status is temporarily unavailable. Retrying."),
  ).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("starting work, steering and stopping remain usable; stale Python requires explicit acknowledgement", async ({
  page,
}, info) => {
  const { calls, errors } = await core(page);
  await openThread(page);
  await expect(page.getByLabel("Agent to run")).toHaveValue("researcher@1.0.0");
  await page.getByLabel("Agent to run").selectOption("researcher@1.1.0");
  await page.getByLabel("Next instruction").fill("Check the next result");
  await page.getByRole("button", { name: "Start work" }).click();
  await expect
    .poll(
      () =>
        calls.filter((c) => c.path.includes("/agents/researcher/executions"))
          .length,
    )
    .toBe(1);
  expect(
    calls.find((c) => c.path.includes("/agents/researcher/executions"))?.body
      .agent_version,
  ).toBe("1.1.0");
  await page.getByLabel("Next instruction").fill("Use the corrected value");
  await page.getByRole("button", { name: "Steer active Run" }).click();
  await expect
    .poll(
      () => calls.find((c) => c.path.endsWith("/steer"))?.body.expected_run_id,
    )
    .toBe("run-0");
  await page.getByRole("button", { name: "Stop Run", exact: true }).click();
  await page.locator("summary", { hasText: "Shell · Python" }).click();
  await page
    .getByRole("textbox", { name: "Code", exact: true })
    .fill("print(counter)");
  await page.getByRole("button", { name: "Execute", exact: true }).click();
  await expect(
    page.getByText("Python memory was reset.", { exact: false }),
  ).toBeVisible();
  expect(calls.filter((c) => c.path.endsWith("/python"))).toHaveLength(1);
  await page.screenshot({
    path: info.outputPath("python-reset.png"),
    fullPage: true,
  });
  const acknowledge = page.getByRole("button", {
    name: "Acknowledge reset and execute this code",
  });
  await acknowledge.focus();
  await page.keyboard.press("Enter");
  await expect
    .poll(() => calls.filter((c) => c.path.endsWith("/python")).length)
    .toBe(2);
  expect(
    calls.filter((c) => c.path.endsWith("/python"))[1].body.expected_session_id,
  ).toBe("new-session");
  expect(errors).toEqual([]);
});

test("progressive Skills and denied approval show actionable state", async ({
  page,
}, info) => {
  const { calls, errors } = await core(page);
  await openThread(page);
  await page.locator("summary", { hasText: /^Skills$/ }).click();
  await page.getByRole("button", { name: "List available Skills" }).click();
  await expect(
    page.getByText("upload:analysis/SKILL.md", { exact: false }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Load instructions" }).click();
  await page.getByRole("button", { name: "scripts/check.py" }).click();
  await expect(
    page.getByText("print('must not execute')", { exact: true }),
  ).toBeVisible();
  expect(calls.filter((c) => /\/(shell|python)$/.test(c.path))).toHaveLength(0);
  await expect(
    page.getByText("Blocked: no eligible approver", { exact: false }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Allow once", exact: true }),
  ).toHaveCount(1);
  await page.getByRole("button", { name: "Deny", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Allow once", exact: true }),
  ).toHaveCount(0);
  await page.screenshot({
    path: info.outputPath("scoped-thread.png"),
    fullPage: true,
  });
  expect(errors).toEqual([]);
});

const toolArguments = { account: "acme", fields: ["owner", "stage"] };

function toolApproval(id: string, patch: Record<string, unknown> = {}) {
  return {
    id,
    area_id: "area-one",
    run_id: "run-0",
    revision: 1,
    kind: "tool_call",
    state: "pending",
    requester: "alice",
    viewer_is_approver: true,
    viewer_can_decide_inline: true,
    targets: ["tool:crm@1.0.0#invoke"],
    action: "tool.call_unapproved",
    expires_at: "2099-01-01T00:00:00Z",
    agent_id: "researcher",
    agent_version: "1.0.0",
    tool: "crm",
    version: "1.0.0",
    operation: "invoke",
    call: { id: "call-0", name: "lookup_account", arguments: toolArguments },
    ...patch,
  };
}

test("a Tool call approval shows the exact call and is allowed once, for this Run or denied", async ({
  page,
}) => {
  const { calls, errors } = await core(page);
  const longArguments = Object.fromEntries(
    Array.from({ length: 80 }, (_, i) => [`field_${i}`, `value ${i}`]),
  );
  const cards = [
    toolApproval("tool-once"),
    toolApproval("tool-run", {
      call: { id: "call-1", name: "lookup_account", arguments: { id: "run" } },
    }),
    toolApproval("tool-deny", {
      call: { id: "call-2", name: "lookup_account", arguments: { id: "deny" } },
    }),
    toolApproval("tool-long", {
      call: { id: "call-3", name: "export", arguments: longArguments },
    }),
    toolApproval("tool-denied", {
      state: "denied",
      decided_by: "bob",
      decided_at: "2026-10-19T00:00:00Z",
      call: { id: "call-4", name: "lookup_account", arguments: { id: "no" } },
    }),
    toolApproval("tool-blocked", {
      state: "blocked",
      viewer_is_approver: false,
      call: { id: "call-5", name: "lookup_account", arguments: { id: "nb" } },
    }),
    {
      ...toolApproval("tool-grant", {
        kind: "grant",
        state: "active",
        call: null,
        decided_by: "alice",
      }),
      revision: 3,
    },
  ];
  await page.route("**/api/capabilities/approvals**", (route) => {
    const url = new URL(route.request().url());
    if (
      route.request().method() !== "GET" ||
      url.pathname !== "/api/capabilities/approvals"
    )
      return route.fallback();
    return route.fulfill({ json: { items: cards, next_cursor: null } });
  });
  await openThread(page);
  const inline = page.getByRole("region", {
    name: "Approvals for this Thread",
  });

  const once = inline
    .getByRole("article", { name: "Tool call approval" })
    .filter({
      has: page
        .getByRole("region", { name: /^Arguments/ })
        .filter({ hasText: JSON.stringify(toolArguments, null, 2) }),
    });
  for (const text of ["crm@1.0.0", "researcher@1.0.0", "lookup_account"])
    await expect(once.getByText(text, { exact: true })).toBeVisible();
  await expect(
    once
      .getByRole("term")
      .filter({ hasText: /^Operation$/ })
      .locator("+ dd"),
  ).toHaveText("invoke");
  const shown = once.getByRole("region", { name: /^Arguments/ });
  await expect(shown).toHaveText(JSON.stringify(toolArguments, null, 2));
  await expect(
    once.getByText("once allowed, this exact call runs", { exact: false }),
  ).toBeVisible();

  // A long argument list stays in a bounded block that scrolls.
  const long = inline
    .getByRole("article", { name: "Tool call approval" })
    .filter({ hasText: "export" })
    .getByRole("region", { name: /^Arguments/ });
  const box = await long.evaluate((el) => ({
    scrolls: el.scrollHeight > el.clientHeight,
    height: el.clientHeight,
    overflow: getComputedStyle(el).overflowY,
  }));
  expect(box).toMatchObject({ scrolls: true, overflow: "auto" });
  expect(box.height).toBeLessThanOrEqual(220);

  await once.getByRole("button", { name: "Allow once", exact: true }).click();
  await expect
    .poll(() => calls.find((c) => c.path.endsWith("/tool-once/decide"))?.body)
    .toMatchObject({
      choice: "allow_once",
      expected_revision: 1,
      targets: null,
      expires_at: null,
    });

  const run = inline
    .getByRole("article", { name: "Tool call approval" })
    .filter({ hasText: '"id": "run"' });
  await run.getByRole("checkbox").check();
  await expect(
    run.getByText(
      "Every later invoke call of crm@1.0.0 in this Run, whatever its arguments, until expiry. New user input does not end it",
      { exact: false },
    ),
  ).toBeVisible();
  await run.getByLabel("Duration (minutes, up to 60)").fill("30");
  await run.getByRole("button", { name: "Allow for this Run" }).click();
  await expect
    .poll(() => calls.find((c) => c.path.endsWith("/tool-run/decide"))?.body)
    .toMatchObject({
      choice: "allow_run",
      targets: ["tool:crm@1.0.0#invoke"],
    });
  const granted = calls.find((c) => c.path.endsWith("/tool-run/decide"))!;
  const minutes =
    (Date.parse(String(granted.body.expires_at)) - Date.now()) / 60000;
  expect(minutes).toBeGreaterThan(29);
  expect(minutes).toBeLessThanOrEqual(30);

  await inline
    .getByRole("article", { name: "Tool call approval" })
    .filter({ hasText: '"id": "deny"' })
    .getByRole("button", { name: "Deny", exact: true })
    .click();
  await expect
    .poll(() => calls.find((c) => c.path.endsWith("/tool-deny/decide"))?.body)
    .toMatchObject({ choice: "deny", targets: null, expires_at: null });

  // Decided and blocked approvals say the call is refused and the Run goes on.
  await expect(
    page.getByText(
      /^Denied by bob at .+\. The call is refused and the Run continues\.$/,
    ),
  ).toBeVisible();
  await expect(
    page.getByText(
      "Nobody in this Tenant may approve calls of crm@1.0.0. The call is refused and the Run continues.",
      { exact: true },
    ),
  ).toBeVisible();
  const grant = page.getByRole("article", { name: "Tool Run grant" });
  await expect(
    grant.getByText("Every later invoke call of crm@1.0.0", { exact: false }),
  ).toBeVisible();
  await grant.getByRole("button", { name: "Revoke" }).click();
  await expect
    .poll(
      () =>
        calls.find(
          (c) => c.path === "/api/capabilities/grants/tool-grant/revoke",
        )?.body,
    )
    .toMatchObject({ expected_revision: 3 });

  // The approver's inbox lists the same pending approvals and the grant given.
  await page.getByRole("button", { name: /Decisions/ }).click();
  const inbox = page.getByRole("region", {
    name: "Approvals awaiting you",
    exact: true,
  });
  await expect(
    inbox.getByRole("article", { name: "Tool call approval" }),
  ).toHaveCount(4);
  await expect(
    page
      .getByRole("region", { name: "Run grants you gave", exact: true })
      .getByRole("article", { name: "Tool Run grant" }),
  ).toHaveCount(1);
  expect(errors).toEqual([]);
});

test("clearing and deleting a Thread uses its Scratch folder workflow", async ({
  page,
}) => {
  const { calls, errors } = await core(page);
  await openThread(page);
  await page.locator("summary", { hasText: "Manage Thread" }).click();

  page.once("dialog", (dialog) => dialog.dismiss());
  await page
    .getByRole("button", { name: "Clear Scratch folder", exact: true })
    .click();
  expect(
    calls.filter((call) => call.path.endsWith("/scratch/clear")),
  ).toHaveLength(0);

  page.once("dialog", (dialog) => dialog.accept());
  await page
    .getByRole("button", { name: "Clear Scratch folder", exact: true })
    .click();
  await expect
    .poll(() =>
      calls.some(
        (call) => call.path === "/api/threads/thread-one/scratch/clear",
      ),
    )
    .toBe(true);

  page.once("dialog", (dialog) => dialog.accept());
  await page
    .getByRole("button", { name: "Delete Thread", exact: true })
    .click();
  await expect
    .poll(
      () =>
        calls.find((call) => call.path === "/api/threads/thread-one/delete")
          ?.body,
    )
    .toEqual({
      idempotency_key: expect.any(String),
    });
  expect(errors).toEqual([]);
});

test("thread run retries reuse an ambiguous idempotency key and changed prompts get a new key", async ({
  page,
}) => {
  const { errors } = await core(page);
  const requests: Record<string, unknown>[] = [];
  await page.route("**/api/execution-scopes*", (route) =>
    route.fulfill({ json: { items: [], next_cursor: null } }),
  );
  // An Agent runs in the Thread itself (#1008).
  await page.route("**/api/threads/*/agents/*/executions", async (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    requests.push(route.request().postDataJSON());
    if (requests.length === 1) return route.abort("failed");
    return route.fulfill({ json: { id: "new-area" } });
  });
  await openThread(page);
  await page.getByLabel("Agent to run").selectOption("researcher@1.0.0");
  const prompt = page.getByLabel("Next instruction");
  const start = page.getByRole("button", { name: "Start work", exact: true });
  await prompt.fill("First instruction");
  await start.click();
  await expect(page.getByRole("alert")).toBeVisible();
  await start.click();
  await expect.poll(() => requests.length).toBe(2);
  await prompt.fill("Changed instruction");
  await expect(start).toBeEnabled();
  await start.click();
  await expect.poll(() => requests.length).toBe(3);
  expect(requests[0].idempotency_key).toBe(requests[1].idempotency_key);
  expect(requests[2].idempotency_key).not.toBe(requests[1].idempotency_key);
  expect(errors).toEqual([]);
});

test("steering retries reuse an ambiguous key and changed prompts get a new key", async ({
  page,
}) => {
  const { errors } = await core(page);
  const requests: Record<string, unknown>[] = [];
  await page.route("**/api/execution-scopes/area-one/steer", async (route) => {
    requests.push(route.request().postDataJSON());
    if (requests.length === 1) return route.abort("failed");
    return route.fulfill({ json: { status: "queued" } });
  });
  await openThread(page);
  const prompt = page.getByLabel("Next instruction");
  const steer = page.getByRole("button", {
    name: "Steer active Run",
    exact: true,
  });
  await prompt.fill("Keep the original request");
  await steer.click();
  await expect(page.getByRole("alert")).toBeVisible();
  await steer.click();
  await expect.poll(() => requests.length).toBe(2);
  await prompt.fill("Use a changed request");
  await steer.click();
  await expect.poll(() => requests.length).toBe(3);
  expect(requests[0].expected_run_id).toBe("run-0");
  expect(requests[0].idempotency_key).toBe(requests[1].idempotency_key);
  expect(requests[2].idempotency_key).not.toBe(requests[1].idempotency_key);
  expect(errors).toEqual([]);
});

test("a generated image uses its Run and command download endpoint", async ({
  page,
}) => {
  const { createHash } = await import("node:crypto");
  const { errors } = await core(page);
  const imageData =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
  const bytes = Buffer.from(imageData, "base64");
  const file = {
    file_id: "display-one",
    path: "plot.png",
    digest: createHash("sha256").update(bytes).digest("hex"),
    size: bytes.length,
    media_type: "image/png",
    scope: "working",
  };
  const downloads: string[] = [];
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/python/poll"))
      return route.fulfill({
        json: {
          operation_id: "display-operation",
          area_id: "area-one",
          kind: "code_interpreter",
          status: "completed",
          displays: [file],
        },
      });
    if (path.endsWith("/core-operations"))
      return route.fulfill({
        json: {
          items: [
            {
              operation_id: "display-operation",
              area_id: "area-one",
              kind: "code_interpreter",
              status: "completed",
              displays: [file],
            },
          ],
          next_cursor: null,
        },
      });
    if (path.endsWith("/files/display-one/download")) {
      downloads.push(path);
      return route.fulfill({
        json: {
          file,
          data: imageData,
          next_offset: null,
        },
      });
    }
    return route.fallback();
  });
  await openThread(page);
  await page.locator("summary", { hasText: "Shell · Python" }).click();
  await page.getByRole("button", { name: "Show image", exact: true }).click();
  const image = page.getByRole("img", { name: "Graph generated by Python" });
  await expect(image).toBeVisible();
  await expect
    .poll(() =>
      image.evaluate((element) => (element as HTMLImageElement).naturalWidth),
    )
    .toBe(1);
  const imageDownload = page.waitForEvent("download");
  await page
    .getByRole("button", { name: "Download output", exact: true })
    .click();
  expect((await imageDownload).suggestedFilename()).toBe("plot.png");
  expect(downloads).toEqual([
    "/api/executions/run-0/core-operations/display-operation/files/display-one/download",
    "/api/executions/run-0/core-operations/display-operation/files/display-one/download",
  ]);
  expect(errors).toEqual([]);
});
