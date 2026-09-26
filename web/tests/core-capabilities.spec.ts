import { test, expect, type Page } from "@playwright/test";
import { setup } from "./collaboration-fixture";

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
    workspace_id: "workspace-one",
    thread_id: "thread-message-one",
    agent_id: "researcher",
    owner: "alice",
    state: "active",
    generation: 1,
    revision: 7,
    manifest: [],
  };
  const managed = {
    area_id: area.id,
    ...area,
    files: 2,
    bytes: 128,
    snapshot_id: null as string | null,
    recovery_expires_at: null as string | null,
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
      approver: "alice",
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
      approver: "alice",
      targets: ["https://expired.example.org"],
      expires_at: "2020-01-01T00:00:00Z",
    },
    {
      id: "blocked",
      area_id: area.id,
      run_id: "run-0",
      revision: 1,
      kind: "approval",
      state: "blocked_no_approver",
      requester: "alice",
      approver: null,
      targets: ["https://blocked.example.org"],
      expires_at: "2099-01-01T00:00:00Z",
    },
  ];
  let receipt = false;
  let pythonCalls = 0;
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const body =
      request.method() === "POST" ? (request.postDataJSON() ?? {}) : {};
    const send = (json: unknown) => route.fulfill({ json });
    if (path === "/api/working-areas")
      return send({ items: [area], next_cursor: null });
    if (path === "/api/working-files")
      return send({ items: [managed], next_cursor: null });
    if (path === "/api/registry")
      return send([
        {
          id: "researcher",
          version: "1.0.0",
          name: { en: "Researcher" },
          config: {},
        },
      ]);
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
        queue: [
          {
            run_id: "run-0",
            sequence: 1,
            phase: "THINKING",
            control: "ACTIVE",
          },
        ],
      });
    if (path.endsWith("/core-operations"))
      return send({ items: [], next_cursor: null });
    if (path === "/api/capabilities/approvals")
      return send({ items: approvals, next_cursor: null });
    if (path.endsWith("/transfers"))
      return send({
        items: [
          {
            operation_id: "transfer-one",
            status: receipt ? "completed" : "uncertain",
            recipient: {
              node_id: "aidash://peer",
              agent_id: "reviewer",
              agent_version: "1.0.0",
              thread_id: "remote-thread",
            },
            manifest_digest: "a".repeat(64),
            effects_may_have_occurred: true,
            receipt: receipt ? { accepted: true } : null,
          },
        ],
        next_cursor: null,
      });
    if (
      request.method() === "POST" &&
      (path.startsWith("/api/working-areas/") ||
        path.startsWith("/api/runs/") ||
        path.includes("/restore/new-thread") ||
        path.startsWith("/api/capabilities/") ||
        path.startsWith("/api/file-transfers/") ||
        path.endsWith("/delete") ||
        path.includes("/capability-configuration"))
    ) {
      calls.push({ path, body });
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
      if (path.endsWith("/files/search"))
        return send({
          matches: [
            { path: "data.csv", location: { line: 2 }, snippet: "東京,42" },
          ],
          next_cursor: null,
        });
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
      if (path.endsWith("/reconcile")) receipt = true;
      if (path.endsWith("/deletion-confirmation"))
        return send({ confirmation_id: "confirm-seven", revision: 7 });
      if (path.endsWith("/restore/new-thread"))
        return send({ thread: { id: "restored-thread" }, area: managed });
      if (path.endsWith("/cleanup")) {
        managed.state =
          body.choice === "recoverable"
            ? "recoverable"
            : body.choice === "irreversible"
              ? "deleted"
              : managed.state;
        managed.snapshot_id =
          body.choice === "recoverable" ? "snapshot-seven" : null;
        managed.recovery_expires_at =
          body.choice === "recoverable" ? "2099-01-08T00:00:00Z" : null;
      }
      return send({ status: "completed", accepted_run_id: "run-0" });
    }
    return route.fallback();
  });
  return { ...fixture, calls, managed };
}

test("capability setup attaches a direct Skill and failed-extraction original to a new immutable version", async ({
  page,
}, info) => {
  const { errors } = await core(page);
  const writes: { path: string; body: Record<string, unknown> }[] = [];
  let uploaded = false;
  let saved: Record<string, unknown> | undefined;
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (
      route.request().method() === "POST" &&
      (path.startsWith("/api/references/") || path.endsWith("/capabilities"))
    ) {
      const body = route.request().postDataJSON() ?? {};
      writes.push({ path, body });
      if (path.endsWith("/capabilities")) {
        saved = body;
        return route.fulfill({ json: { catalog_approval_required: true } });
      }
      if (path.endsWith("/commit")) uploaded = true;
      return route.fulfill({
        json: {
          reference_id: "uploaded-original",
          state: "uploading",
          uploaded_bytes: 0,
        },
      });
    }
    if (path === "/api/references")
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
    return route.fallback();
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
  for (const name of [
    "Search and read files",
    "Shell",
    "Python",
    "Apply patches",
    "Direct Skills",
    "Share files",
  ])
    await expect(
      config.getByRole("checkbox", { name, exact: true }),
    ).not.toBeChecked();
  await config
    .getByRole("checkbox", { name: "Search and read files", exact: true })
    .check();
  await config
    .getByRole("checkbox", { name: "Direct Skills", exact: true })
    .check();
  await config.getByLabel("SKILL.md", { exact: true }).setInputFiles({
    name: "SKILL.md",
    mimeType: "text/markdown",
    buffer: Buffer.from(
      "---\nname: direct-analysis\ndescription: Analyze authorized files\n---\nRead only the selected files.\n",
    ),
  });
  await config
    .getByRole("button", { name: "Attach this Skill", exact: true })
    .click();
  await config
    .getByLabel("Add PDF, Excel or text (up to 10 MiB)")
    .setInputFiles({
      name: "broken.pdf",
      mimeType: "application/pdf",
      buffer: Buffer.from("%PDF-broken"),
    });
  await expect(
    config.getByText("ready · malformed", { exact: true }),
  ).toBeVisible();
  await expect(
    config.getByRole("button", { name: "Download original", exact: true }),
  ).toBeEnabled();
  await config
    .getByRole("button", { name: "Attach to this Agent", exact: true })
    .click();
  await config.getByLabel("New immutable version").fill("1.1.0");
  await config
    .getByRole("button", { name: "Save a new version", exact: true })
    .focus();
  await page.keyboard.press("Enter");
  await expect(
    config.getByText(
      "Saved 1.1.0. It becomes available after catalog approval.",
      { exact: true },
    ),
  ).toBeVisible();
  expect(saved).toMatchObject({
    source_version: "1.0.0",
    new_version: "1.1.0",
    core_capabilities: {
      files: true,
      skills: true,
      shell: false,
      python: false,
      patch: false,
      sharing: false,
    },
    reference_attachments: [
      { reference_id: "uploaded-original", digest: "b".repeat(64) },
    ],
  });
  expect(saved?.skill_attachments as { digest: string }[]).toHaveLength(1);
  expect((saved?.skill_attachments as { digest: string }[])[0].digest).toMatch(
    /^sha256:[a-f0-9]{64}$/,
  );
  expect(writes.map((w) => w.path)).toEqual([
    "/api/references/uploads",
    "/api/references/uploaded-original/chunks",
    "/api/references/uploaded-original/commit",
    "/api/agents/researcher/capabilities",
  ]);
  expect(writes[1].body).toEqual({
    offset: 0,
    data: Buffer.from("%PDF-broken").toString("base64"),
  });
  await config
    .getByRole("checkbox", { name: "Direct Skills", exact: true })
    .uncheck();
  await config
    .getByRole("checkbox", { name: "Search and read files", exact: true })
    .uncheck();
  await config
    .getByRole("checkbox", { name: "Direct Skills", exact: true })
    .check();
  await config
    .getByRole("checkbox", { name: "Search and read files", exact: true })
    .check();
  await config.getByLabel("New immutable version").fill("1.2.0");
  await config
    .getByRole("button", { name: "Save a new version", exact: true })
    .focus();
  await page.keyboard.press("Enter");
  await expect(
    config.getByText(
      "Saved 1.2.0. It becomes available after catalog approval.",
      { exact: true },
    ),
  ).toBeVisible();
  expect(saved).toMatchObject({
    new_version: "1.2.0",
    core_capabilities: { files: true, skills: true },
    skill_attachments: [],
    skill_roots: [],
    reference_attachments: [],
  });
  await page.screenshot({
    path: info.outputPath("capability-setup.png"),
    fullPage: true,
  });
  expect(errors).toEqual([]);
});

async function openThread(page: Page) {
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Reply in thread", exact: true })
    .first()
    .click();
  await expect(page.getByRole("heading", { name: "Agent work" })).toBeVisible();
}

test("queue, steer and stop remain usable; stale Python requires explicit acknowledgement", async ({
  page,
}, info) => {
  const { calls, errors } = await core(page);
  await openThread(page);
  await expect(page.getByLabel("Agent to run")).toHaveValue("researcher@1.0.0");
  await page.getByLabel("Agent to run").selectOption("researcher@1.1.0");
  await page.getByLabel("Next instruction").fill("Check the next result");
  await page.getByRole("button", { name: "Queue next Run" }).click();
  await expect
    .poll(() => calls.filter((c) => c.path.endsWith("/queue")).length)
    .toBe(1);
  expect(calls.find((c) => c.path.endsWith("/queue"))?.body.agent_version).toBe(
    "1.1.0",
  );
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

test("located search, progressive Skills, denied approval and uncertain transfer show actionable state", async ({
  page,
}, info) => {
  const { calls, errors } = await core(page);
  await openThread(page);
  await page.getByLabel("Search query").fill("東京");
  await page.getByRole("button", { name: "Search", exact: true }).click();
  await expect(page.getByText("data.csv:2 東京,42")).toBeVisible();
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
  await page
    .locator("summary", { hasText: "Transfer history and receipts" })
    .click();
  await expect(
    page.getByText("Delivery may have occurred.", { exact: false }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Reconcile this transfer", exact: true })
    .click();
  await expect(
    page.getByText("Receipt confirmed.", { exact: false }),
  ).toBeVisible();
  await page.screenshot({
    path: info.outputPath("scoped-thread.png"),
    fullPage: true,
  });
  expect(errors).toEqual([]);
});

test("narrow keyboard cleanup keeps default, separates confirmation and restores retained files", async ({
  page,
}, info) => {
  const { calls, managed, errors } = await core(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/settings?view=workingFiles");
  const retention = page.getByLabel("Retention choice");
  await expect(retention).toHaveValue("keep");
  expect(calls).toHaveLength(0);
  await retention.selectOption("recoverable");
  await page.getByRole("button", { name: "Apply retention choice" }).focus();
  await page.keyboard.press("Enter");
  await expect(
    page.getByText("Recover until:", { exact: false }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Restore into a new thread" }).click();
  await expect
    .poll(
      () => calls.filter((c) => c.path.endsWith("/restore/new-thread")).length,
    )
    .toBe(1);
  const restore = calls.find((c) => c.path.endsWith("/restore/new-thread"));
  expect(restore?.body.thread_id).toBeUndefined();
  expect(restore?.body.content).toBe("Restore retained working files");
  await retention.selectOption("irreversible");
  const review = page.getByRole("button", {
    name: "Review deletion",
    exact: true,
  });
  await expect(review).toBeEnabled();
  await expect(
    page.getByRole("button", { name: "Confirm irreversible deletion" }),
  ).toHaveCount(0);
  await review.focus();
  await page.keyboard.press("Enter");
  await expect(
    page.getByText("Permanently delete 2 files for researcher.", {
      exact: false,
    }),
  ).toBeVisible();
  expect(calls.filter((c) => c.path.endsWith("/cleanup"))).toHaveLength(1);
  await page.screenshot({
    path: info.outputPath("deletion-confirmation-mobile.png"),
    fullPage: true,
  });
  await page
    .getByRole("button", { name: "Confirm irreversible deletion" })
    .focus();
  await page.keyboard.press("Enter");
  await expect
    .poll(() => calls.filter((c) => c.path.endsWith("/cleanup")).length)
    .toBe(2);
  expect(
    calls.filter((c) => c.path.endsWith("/cleanup"))[1].body.confirmation_id,
  ).toBe("confirm-seven");
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await expect(
    page.getByText("ready · malformed", { exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("deleting a thread requires its file retention choice before controls disappear", async ({
  page,
}) => {
  const { calls, errors } = await core(page);
  await openThread(page);
  await page.locator("summary", { hasText: "Delete this thread" }).click();
  await expect(page.getByLabel("File retention")).toHaveValue("keep");
  await page
    .getByRole("button", {
      name: "Delete thread with these choices",
      exact: true,
    })
    .click();
  await expect
    .poll(() => calls.filter((c) => c.path.endsWith("/delete")).length)
    .toBe(1);
  expect(calls.find((c) => c.path.endsWith("/delete"))?.body.files).toEqual([
    {
      area_id: "area-one",
      expected_revision: 7,
      choice: "keep",
      confirmation_id: null,
    },
  ]);
  await expect(page.getByRole("heading", { name: "Agent work" })).toHaveCount(
    0,
  );
  expect(errors).toEqual([]);
});

test("thread deletion waits for every working-area page before submitting choices", async ({
  page,
}) => {
  const { calls, errors } = await core(page);
  const areas = Array.from({ length: 51 }, (_, i) => ({
    id: `area-${i}`,
    workspace_id: "workspace-one",
    thread_id: "thread-message-one",
    agent_id: `agent-${i}`,
    owner: "alice",
    state: "active",
    generation: 1,
    revision: 7,
    manifest: [],
  }));
  let releaseNext!: () => void;
  const nextPage = new Promise<void>((resolve) => {
    releaseNext = resolve;
  });
  let requestedNext = false;
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname !== "/api/working-areas") return route.fallback();
    if (url.searchParams.get("cursor") === "after-50") {
      requestedNext = true;
      await nextPage;
      return route.fulfill({
        json: { items: areas.slice(50), next_cursor: null },
      });
    }
    return route.fulfill({
      json: { items: areas.slice(0, 50), next_cursor: "after-50" },
    });
  });
  await openThread(page);
  await page.locator("summary", { hasText: "Delete this thread" }).click();
  const remove = page.getByRole("button", {
    name: "Delete thread with these choices",
    exact: true,
  });
  await expect.poll(() => requestedNext).toBe(true);
  await expect(remove).toBeDisabled();
  releaseNext();
  await expect(remove).toBeEnabled();
  await expect(page.getByText("agent-50 · 0 files · 0 bytes")).toBeVisible();
  await remove.click();
  await expect
    .poll(() => calls.filter((c) => c.path.endsWith("/delete")).length)
    .toBe(1);
  const choices = calls.find((c) => c.path.endsWith("/delete"))?.body.files;
  expect(choices).toEqual(
    areas.map((a) => ({
      area_id: a.id,
      expected_revision: a.revision,
      choice: "keep",
      confirmation_id: null,
    })),
  );
  expect(errors).toEqual([]);
});

test("outbound retries reuse an ambiguous request key and a new fetch uses a fresh key", async ({
  page,
}) => {
  const { errors } = await core(page);
  const requests: { url: string; idempotency_key: string }[] = [];
  await page.route("**/api/runs/run-0/outbound", async (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    requests.push(route.request().postDataJSON());
    if (requests.length === 1) return route.abort("failed");
    return route.fulfill({
      json: {
        operation_id: `fetch-${requests.length}`,
        status: "completed",
        url: requests.at(-1)!.url,
      },
    });
  });
  await openThread(page);
  await page
    .locator("summary", { hasText: "External files and Python packages" })
    .click();
  await page.getByLabel("HTTPS URL").fill("https://example.org/current.csv");
  const fetch = page.getByRole("button", {
    name: "Request fetch",
    exact: true,
  });
  await fetch.click();
  await expect(page.getByRole("alert")).toContainText("fetch");
  await fetch.click();
  await expect.poll(() => requests.length).toBe(2);
  await expect(fetch).toBeEnabled();
  await fetch.click();
  await expect.poll(() => requests.length).toBe(3);
  expect(requests[0].url).toBe(requests[2].url);
  expect(requests[0].idempotency_key).toBe(requests[1].idempotency_key);
  expect(requests[2].idempotency_key).not.toBe(requests[1].idempotency_key);
  expect(errors).toEqual([]);
});

test("thread run retries reuse an ambiguous idempotency key and changed prompts get a new key", async ({
  page,
}) => {
  const { errors } = await core(page);
  const requests: Record<string, unknown>[] = [];
  await page.route("**/api/working-areas*", (route) =>
    route.fulfill({ json: { items: [], next_cursor: null } }),
  );
  await page.route(
    "**/api/workspaces/*/threads/*/agents/*/runs",
    async (route) => {
      if (route.request().method() !== "POST") return route.fallback();
      requests.push(route.request().postDataJSON());
      if (requests.length === 1) return route.abort("failed");
      return route.fulfill({ json: { id: "new-area" } });
    },
  );
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
  await page.route("**/api/working-areas/area-one/steer", async (route) => {
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

test("generated image and outbound downloads use the authenticated file endpoint", async ({
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
    if (path.endsWith("/outbound"))
      return route.fulfill({
        json: {
          items: [
            {
              operation_id: "outbound-one",
              status: "completed",
              url: "https://example.org/plot.png",
              http_status: 200,
              output_file: { ...file, file_id: "outbound-one" },
            },
          ],
          next_cursor: null,
        },
      });
    if (path.match(/\/files\/(display-one|outbound-one)\/download$/)) {
      downloads.push(path);
      return route.fulfill({
        json: {
          file: {
            ...file,
            file_id: path.includes("outbound-one")
              ? "outbound-one"
              : "display-one",
          },
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
  await page
    .locator("summary", { hasText: "External files and Python packages" })
    .click();
  const outboundDownload = page.waitForEvent("download");
  await page.getByRole("button", { name: "Download", exact: true }).click();
  expect((await outboundDownload).suggestedFilename()).toBe("plot.png");
  expect(downloads).toContain(
    "/api/working-areas/area-one/files/outbound-one/download",
  );
  expect(downloads.filter((path) => path.includes("display-one"))).toHaveLength(
    2,
  );
  expect(errors).toEqual([]);
});
