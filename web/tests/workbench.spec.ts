import { createHash } from "node:crypto";
import { expect, test, type Page } from "@playwright/test";
import { setup } from "./collaboration-fixture";

const entry = {
  id: "managed-agent",
  version: "1.0.0",
  kind: "agent",
  name: { en: "Managed researcher", ja: "調査エージェント" },
  description: { en: "Summarize sources", ja: "資料を要約する" },
  capabilities: ["summarize"],
  tags: [],
  languages: ["en", "ja"],
  skills: [],
  schema: {},
  config: {
    model: { id: "model", version: "1.0.0" },
    instructions: "Summarize carefully",
    tools: [],
    skills: [],
    cluster: null,
    max_steps: 8,
    allow_task_creation: false,
    allow_task_delegation: false,
    allow_memory_write: false,
    allow_workspace_retrieval: false,
    allow_cross_conversation_memory: false,
  },
};

async function editableDrafts(
  page: Page,
  secondRevision = 7,
  paginated = false,
) {
  await setup(page, { locale: "en-US" });
  const state = { failRefresh: false };
  const saves: Record<string, unknown>[] = [];
  const cursors: (string | null)[] = [];
  const drafts = [1, secondRevision].map((revision, index) => ({
    id: `00000000-0000-7000-8000-00000000000${index + 1}`,
    tenant: "acme",
    owner: "alice",
    revision,
    entry: {
      ...entry,
      id: index ? "second-agent" : "managed-agent",
      config: {
        ...entry.config,
        instructions: index ? "Second draft" : "First draft",
      },
    },
    documents: [] as unknown[],
    release_notes: "",
    source_id: null,
    source_version: null,
    archived: false,
    updated_at: "2026-09-25T00:00:00Z",
  }));
  await page.route("**/api/workbench/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/workbench/drafts") {
      if (state.failRefresh)
        return route.fulfill({
          status: 503,
          json: { error: "refresh failed" },
        });
      const cursor = new URL(route.request().url()).searchParams.get(
        "before_id",
      );
      cursors.push(cursor);
      if (paginated && !cursor)
        return route.fulfill({
          json: Array.from({ length: 100 }, (_, index) => ({
            ...drafts[1],
            id: `00000000-0000-7000-8000-${String(index + 100).padStart(12, "0")}`,
            entry: { ...drafts[1].entry, id: `newer-agent-${index}` },
          })),
        });
      return route.fulfill({ json: drafts });
    }
    const index = drafts.findIndex(
      (draft) => path === `/api/workbench/drafts/${draft.id}`,
    );
    if (index >= 0 && route.request().method() === "PUT") {
      const body = route.request().postDataJSON();
      saves.push({ id: drafts[index].id, ...body });
      drafts[index] = {
        ...drafts[index],
        ...body,
        revision: drafts[index].revision + 1,
      };
      return route.fulfill({ json: drafts[index] });
    }
    if (path.endsWith("/test-limits"))
      return route.fulfill({
        json: {
          max_input_bytes: 20000,
          max_output_tokens: 2048,
          max_total_tokens: 16384,
          max_steps: 12,
          max_duration_secs: 60,
          max_concurrent: 2,
          payload_days: 30,
        },
      });
    return route.fulfill({ json: [] });
  });
  await page.goto("/creator?focus=managed-agent%401.0.0");
  await expect(page.getByLabel("Additional instructions")).toHaveValue(
    "First draft",
  );
  return { state, saves, cursors, drafts };
}

test("Creator opens and edits a focused draft beyond the first page", async ({
  page,
}) => {
  const { saves, cursors } = await editableDrafts(page, 7, true);
  expect(cursors.slice(0, 2)).toEqual([
    null,
    "00000000-0000-7000-8000-000000000199",
  ]);
  await expect(page.locator(".wb-picker select").first()).toHaveValue(
    "managed-agent@1.0.0",
  );
  await page.getByLabel("Additional instructions").fill("Edit an older draft");
  await page
    .locator(".wb-actions")
    .getByRole("button", { name: "Save draft", exact: true })
    .click();
  await expect.poll(() => saves.length).toBe(1);
  expect(saves[0].id).toBe("00000000-0000-7000-8000-000000000001");
});

test("Creator retains dirty edits after a background refresh error", async ({
  page,
}) => {
  await page.clock.install();
  const { state } = await editableDrafts(page);
  const instructions = page.getByLabel("Additional instructions");
  await instructions.fill("Keep these unsaved edits");
  state.failRefresh = true;
  await page.clock.runFor(10001);
  await expect(page.getByRole("alert")).toContainText("refresh failed");
  await expect(instructions).toHaveValue("Keep these unsaved edits");
  await expect(
    page
      .locator(".wb-actions")
      .getByRole("button", { name: "Save draft", exact: true }),
  ).toBeEnabled();
});

test("Creator discards edits and hydrates a selected draft with another revision", async ({
  page,
}) => {
  const { saves } = await editableDrafts(page);
  const instructions = page.getByLabel("Additional instructions");
  await instructions.fill("Discard these edits");
  page.on("dialog", (dialog) => dialog.accept());
  await page
    .locator(".wb-picker select")
    .first()
    .selectOption("second-agent@1.0.0");
  await expect(instructions).toHaveValue("Second draft");
  await instructions.fill("Saved second draft");
  await page
    .locator(".wb-actions")
    .getByRole("button", { name: "Save draft", exact: true })
    .click();
  await expect.poll(() => saves.length).toBe(1);
  expect(saves[0]).toMatchObject({
    id: "00000000-0000-7000-8000-000000000002",
    expected_revision: 7,
  });
});

for (const revision of [1, 7]) {
  test(`Creator confirms query-only Back navigation at revision ${revision}`, async ({
    page,
  }) => {
    await editableDrafts(page, revision);
    const picker = page.locator(".wb-picker select").first();
    const instructions = page.getByLabel("Additional instructions");
    await picker.selectOption("second-agent@1.0.0");
    await expect(instructions).toHaveValue("Second draft");
    await instructions.fill("Keep the second draft edits");
    let accept = false;
    let confirmations = 0;
    page.on("dialog", async (dialog) => {
      confirmations += 1;
      if (accept) await dialog.accept();
      else await dialog.dismiss();
    });
    await page.goBack();
    await expect(picker).toHaveValue("second-agent@1.0.0");
    await expect(instructions).toHaveValue("Keep the second draft edits");
    expect(confirmations).toBe(1);
    accept = true;
    await page.goBack();
    await expect(picker).toHaveValue("managed-agent@1.0.0");
    await expect(instructions).toHaveValue("First draft");
    expect(confirmations).toBe(2);
    await instructions.fill("Keep the first draft edits");
    accept = false;
    await page.goForward();
    await expect(picker).toHaveValue("managed-agent@1.0.0");
    await expect(instructions).toHaveValue("Keep the first draft edits");
    expect(confirmations).toBe(3);
    accept = true;
    await page.goForward();
    await expect(picker).toHaveValue("second-agent@1.0.0");
    await expect(instructions).toHaveValue("Second draft");
    expect(confirmations).toBe(4);
  });
}

for (const [viewport, locale] of [
  [{ width: 1280, height: 960 }, "en-US"],
  [{ width: 900, height: 700 }, "en-US"],
  [{ width: 390, height: 844 }, "ja-JP"],
] as const) {
  test(`Creator layout and save at ${viewport.width}x${viewport.height} ${locale}`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    const { errors } = await setup(page, { locale });
    let draft = {
      id: "00000000-0000-7000-8000-000000000001",
      tenant: "acme",
      owner: "alice",
      revision: 1,
      entry,
      documents: [],
      release_notes: "",
      source_id: null,
      source_version: null,
      archived: false,
      updated_at: "2026-09-25T00:00:00Z",
    };
    const postedTests: Record<string, unknown>[] = [];
    const sessions: Record<string, unknown>[] = [];
    await page.route("**/api/workbench/**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (
        path === "/api/workbench/drafts" &&
        route.request().method() === "GET"
      )
        return route.fulfill({ json: [draft] });
      if (path === `/api/workbench/drafts/${draft.id}/tests`) {
        if (route.request().method() === "GET")
          return route.fulfill({ json: sessions });
        const body = route.request().postDataJSON() as Record<string, unknown>;
        postedTests.push(body);
        const session = {
          id: `00000000-0000-7000-8000-${String(postedTests.length).padStart(12, "0")}`,
          draft_id: draft.id,
          revision: draft.revision,
          status: "completed",
          scenario: { mode: body.mode, profile_id: body.profile_id },
          conversation: [{ role: "user", content: body.message }],
          tool_calls: [],
          usage: {},
          error: null,
          created_at: "2026-09-25T00:00:00Z",
          expires_at: "2026-10-25T00:00:00Z",
          expired_at: null,
        };
        sessions.unshift(session);
        return route.fulfill({ json: session });
      }
      if (path === `/api/workbench/drafts/${draft.id}/versions`)
        return route.fulfill({ json: [] });
      if (path === `/api/workbench/drafts/${draft.id}/test-limits`)
        return route.fulfill({
          json: {
            max_input_bytes: 20000,
            max_output_tokens: 2048,
            max_total_tokens: 16384,
            max_steps: 12,
            max_duration_secs: 60,
            max_concurrent: 2,
            payload_days: 30,
          },
        });
      if (path === "/api/workbench/test-profiles")
        return route.fulfill({
          json: [{ id: "sandbox", revision: 1, enabled: true }],
        });
      if (
        path === `/api/workbench/drafts/${draft.id}` &&
        route.request().method() === "PUT"
      ) {
        const body = route.request().postDataJSON();
        draft = {
          ...draft,
          revision: draft.revision + 1,
          entry: body.entry,
          documents: body.documents,
          release_notes: body.release_notes,
        };
        return route.fulfill({ json: draft });
      }
      return route.fulfill({
        status: 404,
        json: { error: "fixture route unavailable" },
      });
    });
    await page.goto("/creator?focus=managed-agent%401.0.0");
    await expect(page.locator(".wb-page")).toBeVisible();
    await expect(page.locator(".wb-layout.overview")).toBeVisible();
    const instructions = page.getByLabel(
      locale === "ja-JP" ? "追加の指示" : "Additional instructions",
    );
    await instructions.fill("Summarize with citations");
    await page
      .locator(".wb-actions")
      .getByRole("button", {
        name: locale === "ja-JP" ? "下書きを保存" : "Save draft",
        exact: true,
      })
      .click();
    await expect(instructions).toHaveValue("Summarize with citations");
    expect(draft.revision).toBe(2);
    if (viewport.width === 1280) {
      await page
        .locator(".wb-tabs")
        .getByRole("button", { name: "Test", exact: true })
        .click();
      await page.getByLabel("Tool mode").selectOption("real");
      await page.getByLabel("Test connection profile").selectOption("sandbox");
      await page.getByPlaceholder("Test message…").fill("First turn");
      await page
        .locator(".wb-test-compose")
        .getByRole("button", { name: "Test", exact: true })
        .click();
      await expect.poll(() => postedTests.length).toBe(1);
      expect(postedTests[0]).toMatchObject({
        mode: "real",
        profile_id: "sandbox",
        continue_from: null,
      });
      await page.getByPlaceholder("Test message…").fill("Second turn");
      await page
        .locator(".wb-test-compose")
        .getByRole("button", { name: "Test", exact: true })
        .click();
      await expect.poll(() => postedTests.length).toBe(2);
      expect(postedTests[1].continue_from).toBe(sessions[1].id);
      await page.getByRole("button", { name: "Reset conversation" }).click();
      await page.getByPlaceholder("Test message…").fill("Fresh turn");
      await page
        .locator(".wb-test-compose")
        .getByRole("button", { name: "Test", exact: true })
        .click();
      await expect.poll(() => postedTests.length).toBe(3);
      expect(postedTests[2].continue_from).toBeNull();
      await page
        .locator(".wb-tabs")
        .getByRole("button", { name: "Overview", exact: true })
        .click();
      await expect(page.locator(".wb-layout.overview")).toBeVisible();
    }
    const widths = await page.evaluate(() => ({
      content: document.documentElement.scrollWidth,
      viewport: window.innerWidth,
    }));
    expect(widths.content).toBeLessThanOrEqual(widths.viewport);
    const positions = await page
      .locator(".wb-layout.overview > *")
      .evaluateAll((elements) =>
        elements.map((element) => {
          const box = element.getBoundingClientRect();
          return { x: box.x, y: box.y };
        }),
      );
    if (viewport.width === 1280)
      expect(positions[0].x).toBeLessThan(positions[1].x);
    if (viewport.width === 390)
      expect(positions[0].y).toBeLessThan(positions[1].y);
    expect(errors).toEqual([]);
  });
}

for (const [viewport, locale] of [
  [{ width: 1280, height: 960 }, "en-US"],
  [{ width: 390, height: 844 }, "ja-JP"],
] as const) {
  test(`Trust shows factual context without a verdict at ${viewport.width} ${locale}`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    const { errors } = await setup(page, { locale });
    await page.route("**/api/workbench/**", (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path === "/api/workbench/drafts") return route.fulfill({ json: [] });
      if (path === "/api/workbench/versions/managed-agent/1.0.0")
        return route.fulfill({
          json: {
            entry,
            source_node: "aidash://home",
            observed_at: "2026-09-25T00:00:00Z",
            workspaces: [
              {
                workspace_id: "workspace-one",
                title: "Research",
                current: true,
                latest_run_at: "2026-09-25T00:00:00Z",
              },
            ],
            usage_truncated: false,
            external_assessment_available: false,
          },
        });
      if (path === "/api/workbench/versions/managed-agent/1.0.0/incidents")
        return route.fulfill({ json: [] });
      if (path === "/api/workbench/versions/managed-agent/1.0.0/permissions")
        return route.fulfill({
          json: {
            tenant: "acme",
            subject: "alice",
            workspace_id: null,
            policy_revision: 1,
            observed_at: "2026-09-25T00:00:00Z",
            requested_capabilities: [],
            rows: [
              {
                reference: { id: "model", version: "1.0.0" },
                kind: "model",
                action: "model.infer",
                catalog_enabled: true,
                policy_allowed: true,
                registry_read_allowed: false,
                effective_for_component: false,
              },
            ],
            workspace_read: null,
            note: "Execution permission context",
          },
        });
      return route.fulfill({
        status: 404,
        json: { error: "fixture route unavailable" },
      });
    });
    await page.goto("/trust?focus=managed-agent%401.0.0");
    await expect(
      page.getByRole("heading", {
        name: locale === "ja-JP" ? "調査エージェント" : "Managed researcher",
      }),
    ).toBeVisible();
    await expect(page.locator(".wb-trust-layout")).toBeVisible();
    await expect(page.getByText("Research ·", { exact: false })).toBeVisible();
    await page
      .locator(".wb-tabs")
      .getByRole("button", {
        name: locale === "ja-JP" ? "認証" : "Certifications",
      })
      .click();
    await expect(
      page
        .getByText(
          locale === "ja-JP"
            ? /Trust評価・認証は行いません/
            : /Trust assessments and certification are unavailable/,
        )
        .first(),
    ).toBeVisible();
    await page
      .locator(".wb-tabs")
      .getByRole("button", {
        name: locale === "ja-JP" ? "ポリシー" : "Policies",
      })
      .click();
    await page.locator(".wb-policy input").nth(0).fill("acme");
    await page.locator(".wb-policy input").nth(1).fill("alice");
    await page
      .getByRole("button", {
        name:
          locale === "ja-JP" ? "この条件で権限を確認" : "Check this context",
      })
      .click();
    await expect(
      page.getByRole("columnheader", {
        name: locale === "ja-JP" ? "Registry参照" : "Registry read",
      }),
    ).toBeVisible();
    await expect(
      page.locator(".wb-policy tbody tr").getByRole("cell"),
    ).toHaveText(["model · model@1.0.0", "model.infer", "✓", "✓", "—", "—"]);
    const widths = await page.evaluate(() => ({
      content: document.documentElement.scrollWidth,
      viewport: window.innerWidth,
    }));
    expect(widths.content).toBeLessThanOrEqual(widths.viewport);
    expect(errors).toEqual([]);
  });
}

function behavioralSession(
  draft: { id: string; revision: number },
  status: string,
) {
  return {
    id: "00000000-0000-7000-8000-000000000099",
    draft_id: draft.id,
    revision: draft.revision,
    status,
    scenario: { mode: "simulated", profile_id: null },
    conversation: [{ role: "user", content: "First test" }],
    tool_calls: [],
    usage: {},
    error: null,
    created_at: "2026-09-25T00:00:00Z",
    expires_at: "2026-10-25T00:00:00Z",
    expired_at: null,
  };
}

for (const status of [
  "blocked",
  "failed",
  "timed_out",
  "outcome_unknown",
  "completed",
]) {
  test(`Creator retries a ${status} behavioral session with the correct continuation`, async ({
    page,
  }) => {
    await page.clock.install();
    const { drafts } = await editableDrafts(page);
    const posts: Record<string, unknown>[] = [];
    const sessions: ReturnType<typeof behavioralSession>[] = [];
    await page.route(
      `**/api/workbench/drafts/${drafts[0].id}/tests`,
      async (route) => {
        if (route.request().method() === "GET")
          return route.fulfill({ json: sessions });
        posts.push(route.request().postDataJSON());
        const session = behavioralSession(drafts[0], "running");
        sessions.unshift(session);
        return route.fulfill({ json: session });
      },
    );
    await page
      .locator(".wb-tabs")
      .getByRole("button", { name: "Test", exact: true })
      .click();
    const input = page.getByPlaceholder("Test message…");
    const send = page
      .locator(".wb-test-compose")
      .getByRole("button", { name: "Test", exact: true });
    await input.fill("First test");
    await send.click();
    await expect.poll(() => posts.length).toBe(1);
    await input.fill("Retry test");
    await expect(send).toBeDisabled();
    await expect(
      page.getByRole("button", { name: "Reset conversation" }),
    ).toBeDisabled();
    await expect(page.getByLabel("Tool mode")).toBeDisabled();
    sessions[0].status = status;
    await page.clock.runFor(2600);
    await expect(send).toBeEnabled();
    await send.click();
    await expect.poll(() => posts.length).toBe(2);
    expect(posts[1].continue_from).toBe(
      status === "completed" ? sessions[1].id : null,
    );
  });
}

test("Creator renders object-valued Tool conversation content", async ({
  page,
}) => {
  const { drafts } = await editableDrafts(page);
  const session = {
    ...behavioralSession(drafts[0], "completed"),
    conversation: [
      { role: "tool", content: { rows: [{ title: "Tool result" }], count: 1 } },
    ],
  };
  await page.route(`**/api/workbench/drafts/${drafts[0].id}/tests`, (route) =>
    route.fulfill({ json: [session] }),
  );
  await page.reload();
  await page
    .locator(".wb-tabs")
    .getByRole("button", { name: "Test", exact: true })
    .click();
  await expect(page.locator(".wb-chat .tool")).toContainText(
    '"title": "Tool result"',
  );
  await expect(page.getByPlaceholder("Test message…")).toBeVisible();
});

for (const change of ["add", "replace", "remove", "unchanged", "cluster"]) {
  test(`Creator version differences include ${change} private reference or cluster state`, async ({
    page,
  }) => {
    const { drafts } = await editableDrafts(page);
    const previous = [
      { name: "notes.txt", media_type: "text/plain", text: "元の参考資料" },
    ];
    const documents = change === "add" ? [] : previous;
    const digest = documents.length
      ? createHash("sha256")
          .update(
            JSON.stringify(
              documents.map(({ media_type, name, text }) => ({
                media_type,
                name,
                text,
              })),
            ),
          )
          .digest("hex")
      : undefined;
    const registered = {
      ...drafts[0].entry,
      config: { ...drafts[0].entry.config, knowledge_digest: digest },
    };
    drafts[0].documents =
      change === "remove"
        ? []
        : change === "replace"
          ? [{ ...previous[0], text: "Updated references" }]
          : previous;
    Object.assign(drafts[0].entry.config, {
      knowledge_digest: digest,
      cluster:
        change === "cluster"
          ? { id: "execution-cluster", version: "1.0.0" }
          : null,
    });
    await page.route(
      `**/api/workbench/drafts/${drafts[0].id}/versions`,
      (route) =>
        route.fulfill({
          json: [
            {
              entry: registered,
              draft_knowledge_digest: drafts[0].documents.length
                ? createHash("sha256")
                    .update(
                      JSON.stringify(
                        (
                          drafts[0].documents as {
                            media_type: string;
                            name: string;
                            text: string;
                          }[]
                        ).map(({ media_type, name, text }) => ({
                          media_type,
                          name,
                          text,
                        })),
                      ),
                    )
                    .digest("hex")
                : null,
              registered_at: "2026-09-25T00:00:00Z",
              registered_by: "alice",
              behavioral_tested: false,
              release_notes: "",
              source_id: null,
              source_version: null,
            },
          ],
        }),
    );
    await page.reload();
    await page
      .locator(".wb-tabs")
      .getByRole("button", { name: "Versions", exact: true })
      .click();
    const differences = page
      .locator(".wb-version-detail p")
      .filter({ hasText: "Differences from saved draft" });
    await expect(differences).toContainText(
      change === "cluster"
        ? "Cluster"
        : change === "unchanged"
          ? "None"
          : "Private references",
    );
    if (change === "cluster")
      await expect(differences).not.toContainText("Private references");
  });
}

test("Trust audit uses its returned cursor and resets it for a different tenant", async ({
  page,
}) => {
  await page.clock.install();
  const { errors } = await setup(page, { locale: "en-US" });
  const reads: URL[] = [];
  await page.route("**/api/workbench/**", (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    if (path === "/api/workbench/drafts" || path.endsWith("/incidents"))
      return route.fulfill({ json: [] });
    if (path === "/api/workbench/versions/managed-agent/1.0.0")
      return route.fulfill({
        json: {
          entry,
          source_node: "aidash://home",
          observed_at: "2026-09-25T00:00:00Z",
          workspaces: [],
          usage_truncated: false,
          external_assessment_available: false,
        },
      });
    if (path.endsWith("/audit")) {
      reads.push(url);
      const older = url.searchParams.get("cursor") === "fixed-history-cursor";
      return route.fulfill({
        json: {
          observed_at: "2026-09-25T00:00:00Z",
          items: [
            {
              id: older ? "incident:old" : "incident:new",
              source: "incident",
              kind: "incident_changed",
              at: "2026-09-24T00:00:00Z",
              actor: "alice",
              details: {
                label: older ? "Older audit record" : "Latest audit record",
              },
            },
          ],
          next_cursor: older ? null : "fixed-history-cursor",
          source_boundary: "Observed history",
        },
      });
    }
    return route.fulfill({
      status: 404,
      json: { error: "fixture route unavailable" },
    });
  });
  await page.goto("/trust?focus=managed-agent%401.0.0");
  await page
    .locator(".wb-tabs")
    .getByRole("button", { name: "Audit", exact: true })
    .click();
  await expect(
    page.getByText('"Latest audit record"', { exact: false }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Next 50" }).click();
  await expect(
    page.getByText('"Older audit record"', { exact: false }),
  ).toBeVisible();
  expect(reads.at(-1)?.searchParams.get("cursor")).toBe("fixed-history-cursor");
  expect(reads.every((url) => !url.searchParams.has("offset"))).toBe(true);
  await page.clock.runFor(10100);
  expect(reads.at(-1)?.searchParams.get("cursor")).toBe("fixed-history-cursor");
  await page.getByRole("button", { name: "Latest history" }).click();
  await expect(
    page.getByText('"Latest audit record"', { exact: false }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Next 50" }).click();
  await expect(
    page.getByText('"Older audit record"', { exact: false }),
  ).toBeVisible();
  await page.getByLabel("Tenant").fill("other");
  await expect
    .poll(() => reads.at(-1)?.searchParams.get("tenant"))
    .toBe("other");
  expect(reads.at(-1)?.searchParams.get("cursor")).toBeNull();
  expect(errors).toEqual([]);
});
