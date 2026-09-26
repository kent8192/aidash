import { expect, test } from "@playwright/test";
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
