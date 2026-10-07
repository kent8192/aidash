import { expect, test } from "@playwright/test";
import { setup } from "./collaboration-fixture";
import type { ChannelMessagePage } from "../src/generated/models";

for (const locale of ["en-US", "ja-JP"] as const) {
  test(`remote assignment pins the selected memory providers in ${locale}`, async ({
    page,
  }) => {
    const ja = locale === "ja-JP";
    const { errors } = await setup(page, {
      subject: true,
      locale,
      openTask: true,
      remoteAssignment: true,
    });
    const grants: Record<string, unknown>[] = [];
    await page.route("**/api/tasks/task-0/remote-grants**", async (route) => {
      if (new URL(route.request().url()).pathname.endsWith("/remote-grants")) {
        grants.push(route.request().postDataJSON());
        return route.fulfill({ json: { id: grants.at(-1)!.id } });
      }
      return route.fulfill({
        json: {
          admission_id: "remote-run",
          phase: "RECEIVED",
          control: "ACTIVE",
        },
      });
    });
    await page.goto("/collaboration?channel=workspace-one");
    await page
      .getByRole("button", {
        name: ja ? "タスクと成果物" : "Tasks and results",
        exact: true,
      })
      .click();
    await page
      .locator(".collab-channel .collab-task")
      .filter({ hasText: "Collect evidence" })
      .click();
    await page
      .getByRole("button", {
        name: ja ? "担当を割り当て" : "Assign agent",
        exact: true,
      })
      .click();
    const dialog = page.getByRole("dialog");
    await dialog
      .locator("select[name=agent]")
      .selectOption(JSON.stringify(["aidash://remote", "researcher", "1.0.0"]));
    const enabled = dialog.getByLabel(
      ja
        ? "各推論の前に Home の記憶を検索する"
        : "Require Home memory before each inference",
    );
    await expect(enabled).not.toBeChecked();
    await enabled.check();
    await dialog
      .locator("select[name=embedding]")
      .selectOption("home-embedding@1.0.0");
    await dialog
      .locator("input[name=compactor]")
      .fill("approved-compactor@2.0.0");
    await dialog
      .getByRole("button", {
        name: ja ? "担当を割り当て" : "Assign agent",
        exact: true,
      })
      .click();
    await expect(dialog).toHaveCount(0);
    expect(grants).toHaveLength(1);
    expect(grants[0]).toMatchObject({
      node_id: "aidash://remote",
      agent: { id: "researcher", version: "1.0.0" },
      semantic: {
        mode: "required_home",
        embedding: { id: "home-embedding", version: "1.0.0" },
        compactor: { id: "approved-compactor", version: "2.0.0" },
      },
    });
    expect(errors).toEqual([]);
  });

  test(`hidden run retains only stop controls in ${locale}`, async ({
    page,
  }) => {
    const ja = locale === "ja-JP";
    const { errors } = await setup(page, { subject: true, locale });
    let control = "PAUSED";
    const actions: unknown[] = [];
    await page.route("**/api/runs/run-0**", async (route) => {
      if (new URL(route.request().url()).pathname.endsWith("/management")) {
        if (route.request().method() === "POST") {
          actions.push(route.request().postDataJSON());
          control = "CANCELLED";
        }
        return route.fulfill({
          json: {
            id: "run-0",
            phase: "THINKING",
            control,
            semantic_reason: "invalidated",
          },
        });
      }
      return route.fulfill({
        status: 403,
        json: {
          error: { message: "Current reader cannot access dependent content" },
        },
      });
    });
    await page.goto("/collaboration?channel=workspace-one");
    await page
      .getByRole("button", {
        name: ja ? "実行履歴" : "Execution history",
        exact: true,
      })
      .click();
    await page.locator(".collab-channel .collab-task").click();
    const panel = page.getByRole("region", {
      name: ja ? "実行の管理" : "Execution management",
    });
    await expect(panel).toContainText(
      ja ? "参照済みのソース" : "A consumed source changed",
    );
    await expect(
      page
        .getByRole("dialog")
        .getByRole("button", { name: ja ? "再開" : "Resume", exact: true }),
    ).toHaveCount(0);
    await panel
      .getByRole("button", {
        name: ja ? "実行を中止" : "Cancel execution",
        exact: true,
      })
      .click();
    await expect(panel.getByRole("button")).toHaveCount(0);
    expect(actions).toEqual([{ action: "cancel" }]);
    expect(errors).toEqual([]);
  });

  test(`remote generation preserves approval and activation bindings in ${locale}`, async ({
    page,
  }) => {
    const ja = locale === "ja-JP";
    const { errors } = await setup(page, {
      subject: true,
      locale,
      openTask: true,
    });
    const intents: Record<string, unknown>[] = [];
    const grants: Record<string, unknown>[] = [];
    let approved = false;
    let unavailable = false;
    let activationAttempts = 0;
    let cancellations = 0;
    await page.route("**/api/generation/acme/policies", (route) =>
      route.fulfill({ json: [] }),
    );
    await page.route("**/api/tasks/task-0/remote-**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path.endsWith("/remote-generation")) {
        const body = route.request().postDataJSON();
        intents.push(body);
        if (unavailable)
          return route.fulfill({
            status: 403,
            json: { error: { message: "Current authority unavailable" } },
          });
        return route.fulfill({
          json: {
            intent_id: body.id,
            node_id: body.node_id,
            request_id: "receiver-request",
            agent: { id: "prepared-agent", version: "1.0.0" },
            prepared: approved,
            status: approved ? "QUEUED" : "PENDING_APPROVAL",
            expires_at: new Date(Date.now() + 3600000).toISOString(),
          },
        });
      }
      if (path.endsWith("/cancel")) {
        cancellations++;
        return route.fulfill({ json: true });
      }
      if (path.endsWith("/remote-grants")) {
        grants.push(route.request().postDataJSON());
        return route.fulfill({ json: { id: grants.at(-1)!.id } });
      }
      if (path.endsWith("/activate")) {
        activationAttempts++;
        if (activationAttempts === 1)
          return route.fulfill({
            status: 503,
            json: { error: { message: "Temporary response loss" } },
          });
        return route.fulfill({
          json: {
            run_id: "receiver-run",
            admission_id: "receiver-run",
            phase: "RECEIVED",
            control: "ACTIVE",
          },
        });
      }
      return route.fallback();
    });
    await page.goto("/collaboration?channel=workspace-one");
    await page
      .getByRole("button", {
        name: ja ? "タスクと成果物" : "Tasks and results",
        exact: true,
      })
      .click();
    await page
      .locator(".collab-channel .collab-task")
      .filter({ hasText: "Collect evidence" })
      .click();
    await page
      .getByRole("button", {
        name: ja ? "ポリシーで割り当て" : "Assign with policy",
        exact: true,
      })
      .click();
    const dialog = page.getByRole("dialog");
    await dialog
      .getByText(
        ja ? "別の Node で Agent を生成" : "Generate an agent at another node",
        { exact: true },
      )
      .click();
    const form = dialog
      .locator("details")
      .filter({ has: page.locator("input[name=node]") });
    await form
      .getByLabel(ja ? "実行 Node" : "Execution node", { exact: true })
      .fill("aidash://remote");
    await form
      .getByLabel(
        ja ? "実行 Node の生成ポリシー" : "Execution node generation policy",
        { exact: true },
      )
      .fill("specialist");
    await form
      .getByLabel(ja ? "生成を依頼する理由" : "Reason for generation", {
        exact: true,
      })
      .fill("Approved remote research");
    await form
      .getByRole("button", {
        name: ja ? "Agent を準備" : "Prepare agent",
        exact: true,
      })
      .click();
    await expect(
      form.getByRole("button", {
        name: ja ? "許可して実行" : "Authorize and execute",
        exact: true,
      }),
    ).toHaveCount(0);
    await form
      .getByRole("button", {
        name: ja ? "準備を中止" : "Cancel preparation",
        exact: true,
      })
      .click();
    await expect.poll(() => cancellations).toBe(1);
    await form
      .getByRole("button", {
        name: ja ? "Agent を準備" : "Prepare agent",
        exact: true,
      })
      .click();
    await expect.poll(() => intents.length).toBe(2);
    expect(intents[0].id).not.toBe(intents[1].id);
    approved = true;
    const refresh = form.getByRole("button", {
      name: ja
        ? "同じ準備・承認状態を再確認"
        : "Recheck the same preparation and approval",
      exact: true,
    });
    await refresh.click();
    await expect(
      form.getByText("prepared-agent@1.0.0", { exact: true }),
    ).toBeVisible();
    expect(intents[2]).toEqual(intents[1]);
    unavailable = true;
    await refresh.click();
    await expect(
      form.getByText("prepared-agent@1.0.0", { exact: true }),
    ).toHaveCount(0);
    unavailable = false;
    await refresh.click();
    await form
      .getByLabel(ja ? "Home の embedding 定義" : "Home embedding definition", {
        exact: true,
      })
      .fill("home-embedding@1.0.0");
    const activate = form.getByRole("button", {
      name: ja ? "許可して実行" : "Authorize and execute",
      exact: true,
    });
    await activate.click();
    await expect(dialog.getByRole("alert")).toContainText(
      "Temporary response loss",
    );
    await activate.click();
    await expect(dialog).toHaveCount(0);
    expect(activationAttempts).toBe(2);
    expect(grants).toHaveLength(2);
    expect(grants[1]).toEqual(grants[0]);
    expect(grants[0]).toMatchObject({
      node_id: "aidash://remote",
      agent: { id: "prepared-agent", version: "1.0.0" },
      semantic: {
        mode: "required_home",
        embedding: { id: "home-embedding", version: "1.0.0" },
      },
    });
    expect(errors).toEqual([]);
  });

  test(`remote memory shows provenance and invalidation controls in ${locale}`, async ({
    page,
  }) => {
    const ja = locale === "ja-JP";
    const { errors } = await setup(page, { subject: true, locale });
    const grant = {
      id: "4191ce62-a1a8-42d6-9926-bd32f5e0dc34",
      task_id: "task-0",
      node_id: "aidash://remote",
      agent: { id: "researcher", version: "1.0.0" },
      expires_at: new Date(Date.now() + 3600000).toISOString(),
      revoked: false,
    };
    const execution = {
      grant_id: grant.id,
      admission_id: "829cb342-d2c8-43e4-99c2-0dbe96bc4f70",
      run_id: "829cb342-d2c8-43e4-99c2-0dbe96bc4f70",
      phase: "THINKING",
      control: "ACTIVE",
      error: null,
    };
    const semantic = {
      state: "ready",
      reason: null as string | null,
      operation_id: "c8cf392a-f1d4-42e1-8261-a003d1422660",
      retry_count: 0,
      retry_at: null,
      result_count: 2,
      truncated: false,
      retrieved_at: new Date().toISOString(),
    };
    let denied = false;
    let followup: Record<string, unknown> | null = null;
    await page.route("**/api/tasks/task-0/remote-**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path.endsWith("/remote-executions"))
        return route.fulfill({
          json: [{ grant, execution, semantic, unavailable: false }],
        });
      if (path.endsWith("/semantic"))
        return denied
          ? route.fulfill({
              status: 403,
              json: { error: { message: "forbidden" } },
            })
          : route.fulfill({
              json: {
                home_node: "aidash://home",
                operation_id: semantic.operation_id,
                executor: "aidash://remote/agents/researcher@1.0.0",
                binding: { mode: "required_home" },
                model: "home-vector",
                model_version: "1",
                retrieved_at: semantic.retrieved_at,
                truncated: false,
                allowance_node: "aidash://home",
                allowances: [
                  {
                    request_id: "origin-allowance",
                    used_tokens: 1234,
                    token_limit: 800000,
                    embedding_calls: 2,
                    embedding_call_limit: 10,
                    compaction_calls: 1,
                    compaction_call_limit: 4,
                  },
                ],
                sources: [
                  {
                    entry_id: "verified-source",
                    revision: 1,
                    content_digest: "sha256:fixture",
                    agent: null,
                  },
                ],
              },
            });
      if (path.endsWith("/follow-up")) {
        followup = route.request().postDataJSON();
        return route.fulfill({ json: { id: "fresh-task" } });
      }
      if (path.endsWith("/control")) {
        execution.control = "CANCELLED";
        return route.fulfill({ json: execution });
      }
      return route.fallback();
    });
    await page.goto("/collaboration?channel=workspace-one");
    await page
      .getByRole("button", {
        name: ja ? "タスクと成果物" : "Tasks and results",
        exact: true,
      })
      .click();
    await page
      .locator(".collab-channel .collab-task")
      .filter({ hasText: "Collect evidence" })
      .click();
    const panel = page.getByRole("region", {
      name: ja ? "遠隔実行" : "Remote execution",
      exact: true,
    });
    await panel
      .getByRole("button", {
        name: ja ? "参照情報を確認" : "Inspect provenance",
        exact: true,
      })
      .click();
    await expect(
      panel.getByText("verified-source", { exact: true }),
    ).toBeVisible();
    await expect(
      panel.getByText("origin-allowance", { exact: true }),
    ).toBeVisible();
    await expect(panel).toContainText("1,234 / 800,000");
    denied = true;
    await expect(
      panel.getByText("verified-source", { exact: true }),
    ).toHaveCount(0, { timeout: 12000 });
    await expect(panel.getByRole("status")).toContainText(
      ja ? "現在の権限" : "current authority",
    );
    semantic.state = "invalidated";
    semantic.reason = "invalidated";
    execution.control = "PAUSED";
    await panel
      .getByRole("button", {
        name: ja ? "状態を再読み込み" : "Refresh status",
        exact: true,
      })
      .click();
    await expect(
      panel.getByRole("button", {
        name: ja ? "権限を再確認して再開" : "Recheck authority and resume",
        exact: true,
      }),
    ).toHaveCount(0);
    await panel
      .getByRole("button", {
        name: ja ? "Follow-up Task を作成" : "Create follow-up task",
        exact: true,
      })
      .click();
    await panel
      .getByLabel(ja ? "タイトル" : "Title", { exact: true })
      .fill("New independent intent");
    await panel
      .getByLabel(ja ? "新しい指示" : "New instructions", { exact: true })
      .fill("Use currently authorized material.");
    await panel
      .getByRole("button", { name: ja ? "作成" : "Create", exact: true })
      .click();
    await expect
      .poll(() => followup)
      .toMatchObject({
        title: "New independent intent",
        description: "Use currently authorized material.",
        requirements: {},
      });
    expect(followup).not.toHaveProperty("parent_id");
    await panel
      .getByRole("button", {
        name: ja ? "実行を中止" : "Cancel execution",
        exact: true,
      })
      .click();
    await expect(
      panel.getByRole("button", {
        name: ja ? "実行を中止" : "Cancel execution",
        exact: true,
      }),
    ).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

for (const viewport of [
  { width: 1440, height: 700 },
  { width: 390, height: 600 },
  { width: 900, height: 400 },
]) {
  test(`long registration dialog stays contained at ${viewport.width}x${viewport.height}`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    const { errors } = await setup(page);
    await page.goto("/settings?view=registry");
    await page
      .getByRole("button", { name: "Register entity", exact: true })
      .click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Entity type").selectOption("model");
    const heading = dialog.getByRole("heading", { name: "Register entity" });
    const headingBefore = await heading.boundingBox();
    const submit = dialog.getByRole("button", {
      name: "Register entity",
      exact: true,
    });
    await submit.scrollIntoViewIfNeeded();
    await expect(submit).toBeInViewport();
    await expect(heading).toBeInViewport();
    expect(await heading.boundingBox()).toEqual(headingBefore);
    const bounds = await dialog.boundingBox();
    const buttonBounds = await submit.boundingBox();
    expect(bounds!.y).toBeGreaterThanOrEqual(0);
    expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(viewport.height);
    expect(buttonBounds!.y + buttonBounds!.height).toBeLessThan(
      bounds!.y + bounds!.height,
    );
    expect(
      await dialog.evaluate(
        (element) => element.scrollWidth <= element.clientWidth,
      ),
    ).toBe(true);
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

test("conversation is the landing view, with contextual progress and account settings", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "# Research", exact: true }),
  ).toBeVisible();
  await expect(page.locator(".collab-rail")).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "New request", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "View in graph", exact: true }),
  ).toBeVisible();
  await page.getByLabel("Account settings", { exact: true }).click();
  await expect(
    page
      .locator(".intent-account-links")
      .getByRole("button", { name: "Settings", exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("new request preserves the Japanese conversation creation contract", async ({
  page,
}) => {
  const { errors } = await setup(page, { locale: "ja-JP" });
  let payload: unknown;
  await page.route("**/api/conversations", (route) => {
    payload = route.request().postDataJSON();
    return route.fulfill({ json: { workspace: { id: "workspace-two" } } });
  });
  await page.goto("/");
  await page.getByRole("button", { name: "新しい依頼", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("タイトル", { exact: true }).fill("競合調査");
  await dialog
    .getByLabel("ゴール", { exact: true })
    .fill("Rustフレームワークを比較して");
  await dialog.getByRole("combobox").selectOption("researcher@1.0.0");
  await dialog
    .getByRole("button", { name: "新しいゴール", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  await expect(page).toHaveURL(/channel=workspace-two/);
  expect(payload).toEqual({
    title: "競合調査",
    goal: "Rustフレームワークを比較して",
    target: { id: "researcher", version: "1.0.0" },
    target_kind: "agent",
  });
  expect(errors).toEqual([]);
});

test("failed message keeps its draft and idempotency key on retry", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { failFirstMessage: true });
  await page.goto("/collaboration?channel=workspace-one");
  const composer = page.getByRole("textbox", { name: "Message", exact: true });
  await composer.fill("Please check the source.");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Your text has been kept",
  );
  await expect(composer).toHaveValue("Please check the source.");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(composer).toHaveValue("");
  await expect(
    page.getByText("Please check the source.", { exact: true }),
  ).toBeVisible();
  const messages = submissions.filter((item) =>
    item.path.endsWith("/thread-messages"),
  );
  expect(messages).toHaveLength(2);
  expect(messages[0].body.idempotency_key).toBe(
    messages[1].body.idempotency_key,
  );
  expect(errors).toEqual([]);
});

test("message attachments show their names and download in the selected tab context", async ({
  page,
}) => {
  const { errors } = await setup(page, { messageAttachment: true });
  await page.goto("/collaboration?channel=workspace-one");
  const attachment = page.getByRole("button", {
    name: "Download attachment: evidence.txt",
  });
  await expect(attachment).toBeVisible();
  const [download, request] = await Promise.all([
    page.waitForEvent("download"),
    page.waitForRequest(
      (request) =>
        request.url().endsWith("/attachments/attachment-one") &&
        request.method() === "GET",
    ),
    attachment.click(),
  ]);
  expect(download.suggestedFilename()).toBe("evidence.txt");
  expect(request.headers()["x-aidash-context"]).toBe("operator");
  expect(request.headers()["authorization"]).toBeUndefined();
  expect(errors).toEqual([]);
});

test("preparing a channel does not invoke agent execution", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page);
  await page.goto("/");
  await page.getByLabel("Account settings", { exact: true }).click();
  await page
    .getByRole("button", { name: "Prepare a channel", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByRole("textbox", { name: "Title", exact: true })
    .fill("Draft research");
  await dialog
    .getByRole("textbox", { name: "Goal", exact: true })
    .fill("Discuss sources before starting");
  await dialog.getByRole("button", { name: "Create", exact: true }).click();
  await expect(page).toHaveURL(/channel=prepared/);
  await expect(
    page.getByRole("heading", { name: "# Draft research", exact: true }),
  ).toBeVisible();
  expect(submissions.map((item) => item.path)).toEqual(["/api/workspaces"]);
  expect(errors).toEqual([]);
});

test("graph renders with Cytoscape and offers current related channel first", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/collaboration?channel=workspace-two");
  await page
    .getByRole("button", { name: "View in graph", exact: true })
    .click();
  await expect(page).toHaveURL(/\/graph\?channel=workspace-two/);
  await expect(page.locator(".collab-cytoscape canvas").first()).toBeVisible();
  const related = page.locator(".collab-selection .collab-channel-link");
  await expect(related).toHaveCount(2);
  await expect(related.first()).toHaveText("# Planning");
  await related.filter({ hasText: "Research" }).click();
  await expect(page).toHaveURL(/\/collaboration\?channel=workspace-one/);
  await expect(
    page.getByText("Evidence is ready to review.", { exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("graph focus survives contextual selection, reload, and history", async ({
  page,
}) => {
  const { errors } = await setup(page, { extraGraphAgent: true });
  await page.goto("/collaboration?channel=workspace-one");
  await page.getByRole("button", { name: "Tasks and results" }).click();
  await page
    .locator(".collab-channel .collab-agent")
    .filter({ hasText: "Researcher" })
    .click();
  const selector = page.getByLabel("Focus agent");
  const original = JSON.stringify([
    "entity",
    "aidash://home",
    "agent",
    "researcher",
    "1.0.0",
  ]);
  const focused = JSON.stringify([
    "entity",
    "aidash://home",
    "agent",
    'review/"[special]:/agent',
    "1.0.0",
  ]);
  await expect(selector).toHaveValue(original);
  expect(new URL(page.url()).searchParams.get("focus")).toBe(original);

  await selector.selectOption(focused);
  await expect(selector).toHaveValue(focused);
  await expect(page.locator(".collab-cytoscape canvas").first()).toBeVisible();
  expect(new URL(page.url()).searchParams.get("focus")).toBe(focused);

  await page.reload();
  await expect(selector).toHaveValue(focused);
  await page.goBack();
  await expect(selector).toHaveValue(original);
  await page.goForward();
  await expect(selector).toHaveValue(focused);

  const unavailable = JSON.stringify([
    "entity",
    "aidash://home",
    "agent",
    "unavailable",
    "1.0.0",
  ]);
  await page.goto(`/graph?focus=${encodeURIComponent(unavailable)}`);
  await expect(selector).toHaveValue("");
  await expect(
    page.getByRole("status").filter({
      hasText: "This resource is unavailable under your current access.",
    }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("channel intervention answers a human request without settings navigation", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page);
  await page.goto("/");
  await page
    .getByRole("button")
    .filter({ hasText: "Which market should I examine?" })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByRole("textbox", { name: "Answer", exact: true })
    .fill("Japan");
  await dialog.getByRole("button", { name: "Send", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(
    submissions.some(
      (item) => item.path === "/api/human-requests/question-one/answer",
    ),
  ).toBe(true);
  expect(errors).toEqual([]);
});

test("revoked history is hidden rather than retaining a stale conversation", async ({
  page,
}) => {
  const { revokeHistory } = await setup(page);
  await page.goto("/collaboration?channel=workspace-one");
  await expect(
    page.getByText("Evidence is ready to review.", { exact: true }),
  ).toBeVisible();
  revokeHistory();
  await expect(
    page.getByText("Evidence is ready to review.", { exact: true }),
  ).toHaveCount(0);
  await expect(page.getByRole("alert")).toContainText("unavailable");
});

test("an inaccessible explicit channel is not silently replaced with another channel", async ({
  page,
}) => {
  await setup(page);
  await page.goto("/collaboration?channel=unavailable");
  await expect(
    page.getByRole("heading", { name: "# Research", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", {
      name: "This resource is unavailable under your current access.",
      exact: true,
    }),
  ).toBeVisible();
});

test("legacy management links are routed into secondary settings", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/agents");
  await expect(page).toHaveURL(/\/settings\?.*view=agents/);
  await expect(
    page.getByRole("heading", { name: "Researcher", exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("mobile conversation and channel switching do not require horizontal scrolling", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/collaboration?channel=workspace-one");
  await expect(
    page.getByRole("textbox", { name: "Message", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Channels", exact: true }).click();
  await page.getByRole("link", { name: "# Planning", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "# Planning", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  expect(errors).toEqual([]);
});

test("thread replies persist separately and channel drafts survive thread navigation", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page);
  await page.goto("/collaboration?channel=workspace-one");
  const composer = page.getByRole("textbox", { name: "Message", exact: true });
  await composer.fill("Unsent channel note");
  await page
    .getByRole("button", { name: "Reply in thread", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Thread", exact: true }),
  ).toBeVisible();
  await expect(composer).toHaveValue("Unsent channel note");
  const reply = page.getByRole("textbox", { name: "Reply", exact: true });
  await expect(reply).toHaveValue("");
  await reply.fill("Here is the supporting source.");
  await page.getByRole("button", { name: "Send reply", exact: true }).click();
  await expect(
    page.getByText("Here is the supporting source.", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Back to channel", exact: true })
    .click();
  await expect(composer).toHaveValue("Unsent channel note");
  await expect(
    page.getByText("Here is the supporting source.", { exact: true }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Open thread", exact: true }).click();
  await expect(
    page.getByText("Here is the supporting source.", { exact: true }),
  ).toBeVisible();
  const posts = submissions.filter((item) =>
    item.path.endsWith("/thread-messages"),
  );
  expect(posts).toHaveLength(1);
  expect(posts[0].body.thread_id).toBe("thread-message-one");
  expect(errors).toEqual([]);
});

test("older message pages are prepended without losing the latest messages", async ({
  page,
}) => {
  const { errors } = await setup(page, { olderMessages: 45 });
  await page.goto("/collaboration?channel=workspace-one");
  await expect(page.locator(".collab-message")).toHaveCount(40);
  await expect(
    page.getByText("Previous message 0", { exact: true }),
  ).toHaveCount(0);
  await page
    .getByRole("button", { name: "Load older messages", exact: true })
    .click();
  await expect(page.locator(".collab-message")).toHaveCount(46);
  await expect(
    page.getByText("Previous message 0", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("Evidence is ready to review.", { exact: true }),
  ).toHaveCount(1);
  await expect(
    page.getByRole("button", { name: "Load older messages", exact: true }),
  ).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("remote channel participants are displayed without linking to the local graph", async ({
  page,
}) => {
  const { errors } = await setup(page, { remoteParticipant: true });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Tasks and results", exact: true })
    .click();
  const local = page.locator("button.collab-agent");
  const remote = page.locator("div.collab-agent");
  await expect(local).toHaveCount(1);
  await expect(remote).toHaveCount(1);
  await expect(local).toContainText("Researcher · 1.0.0");
  await expect(remote).toContainText("Unavailable item · 1.0.0");
  await expect(local).toHaveJSProperty("tagName", "BUTTON");
  await expect(remote).toHaveJSProperty("tagName", "DIV");
  expect(errors).toEqual([]);
});

test("partial mesh responses tell operators which peer was omitted", async ({
  page,
}) => {
  const { errors } = await setup(page, { meshErrors: true });
  await page.goto("/graph");
  await expect(
    page.getByRole("status").filter({ hasText: "Unavailable item" }),
  ).toContainText("connection refused");
  expect(errors).toEqual([]);
});

test("remote human requests remain visible without a run in the capped snapshot", async ({
  page,
}) => {
  const { errors } = await setup(page, { unpairedRemoteRequest: true });
  await page.goto("/collaboration?channel=workspace-one");
  await expect(
    page
      .locator(".collab-attention .request-card")
      .filter({ hasText: "The run is outside the capped snapshot." }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("local run details retain execution memory", async ({ page }) => {
  const { errors } = await setup(page, { runMemory: true });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Execution history", exact: true })
    .click();
  await page.locator(".collab-channel .collab-task").click();
  await page.getByText("Memory", { exact: true }).click();
  await expect(
    page.getByRole("dialog").locator("details").filter({ hasText: "Memory" }),
  ).toContainText("Retained execution memory.");
  expect(errors).toEqual([]);
});

test("run message attachments stay fixed while an upload is pending", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page);
  let uploadStarted!: () => void;
  let finishUpload!: () => void;
  const started = new Promise<void>((resolve) => (uploadStarted = resolve));
  const uploadGate = new Promise<void>((resolve) => (finishUpload = resolve));
  await page.route(
    "**/api/workspaces/workspace-one/attachments?*",
    async (route) => {
      uploadStarted();
      await uploadGate;
      await route.fallback();
    },
  );
  await page.route("**/api/runs/run-0/message", (route) =>
    route.fulfill({ json: { id: "run-message-one" } }),
  );
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Execution history", exact: true })
    .click();
  await page.locator(".collab-channel .collab-task").click();
  const dialog = page.getByRole("dialog");
  const draft = dialog.getByRole("textbox", { name: "Message", exact: true });
  const picker = dialog.locator('input[type="file"]');
  await draft.fill("Inspect this image");
  await picker.setInputFiles({
    name: "sample.png",
    mimeType: "image/png",
    buffer: Buffer.from("\x89PNG\r\n\x1a\nfixture"),
  });
  const remove = dialog.getByRole("button", {
    name: "Remove attachment: sample.png",
  });
  await dialog.getByRole("button", { name: "Send", exact: true }).click();
  await started;
  await expect(draft).toBeDisabled();
  await expect(picker).toBeDisabled();
  await expect(remove).toBeDisabled();
  await expect(dialog.locator("button.primary")).toBeDisabled();
  finishUpload();
  await expect(dialog).toHaveCount(0);
  expect(
    submissions.filter((item) => item.path.endsWith("/attachments")),
  ).toHaveLength(1);
  expect(errors).toEqual([]);
});

test("run media picker follows the effective model route before uploads", async ({
  page,
}) => {
  const { submissions, errors, setRunMediaRoutes } = await setup(page, {
    runMediaRoutes: [],
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Execution history", exact: true })
    .click();
  await page.locator(".collab-channel .collab-task").click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.locator('input[type="file"]')).toHaveCount(0);
  await expect(
    dialog.getByRole("textbox", { name: "Message", exact: true }),
  ).toBeVisible();
  expect(
    submissions.filter((item) => item.path.endsWith("/attachments")),
  ).toHaveLength(0);
  setRunMediaRoutes([["image/png"]]);
  await expect(dialog.locator('input[type="file"]')).toBeVisible();
  const picker = dialog.locator('input[type="file"]');
  await picker.setInputFiles({
    name: "sample.png",
    mimeType: "image/png",
    buffer: Buffer.from("\x89PNG\r\n\x1a\nfixture"),
  });
  await expect(
    dialog.getByRole("button", { name: "Remove attachment: sample.png" }),
  ).toBeVisible();
  setRunMediaRoutes([]);
  await expect(picker).toHaveCount(0);
  await dialog.getByRole("button", { name: "Send", exact: true }).click();
  await expect(dialog.getByRole("alert")).toBeVisible();
  expect(
    submissions.filter((item) => item.path.endsWith("/attachments")),
  ).toHaveLength(0);
  expect(errors).toEqual([]);
});

test("mobile history remains reachable from graph and settings", async ({
  page,
}) => {
  await setup(page);
  await page.setViewportSize({ width: 390, height: 844 });
  for (const url of ["/graph", "/settings?view=agents"]) {
    await page.goto(url);
    await page.getByRole("button", { name: "Channels", exact: true }).click();
    await expect(
      page.getByRole("link", { name: "# Research", exact: true }),
    ).toBeVisible();
    await page
      .getByRole("button", { name: "Close history", exact: true })
      .click();
    await expect(page.locator(".intent-sidebar")).toBeHidden();
  }
});

test("a same-length refreshed page follows a new latest message", async ({
  page,
}) => {
  const { errors } = await setup(page, {
    latestMessageChangesOnPoll: true,
    olderMessages: 39,
  });
  await page.goto("/collaboration?channel=workspace-one");
  await expect(page.locator(".collab-message")).toHaveCount(40);
  await expect(
    page.getByText("New latest message.", { exact: true }),
  ).toBeVisible({
    timeout: 5000,
  });
  await expect(page.locator(".collab-message")).toHaveCount(40);
  await expect
    .poll(async () =>
      page.locator(".collab-messages").evaluate((element) => {
        const messages = element.querySelectorAll(".collab-message");
        const latest = messages[messages.length - 1];
        return (
          Math.abs(
            latest.getBoundingClientRect().bottom -
              element.getBoundingClientRect().bottom,
          ) < 5
        );
      }),
    )
    .toBe(true);
  expect(errors).toEqual([]);
});

test("message following keeps context after the latest message and respects reading older history", async ({
  page,
}) => {
  const { errors } = await setup(page, { olderMessages: 39 });
  await page.goto("/collaboration?channel=workspace-one");
  const feed = page.locator(".collab-messages");
  await expect(feed.locator(".collab-message")).toHaveCount(40);
  await expect
    .poll(() =>
      feed.evaluate((element) => {
        const latest = element.querySelectorAll(".collab-message")[39];
        const bottom = element.getBoundingClientRect().bottom;
        return (
          Math.abs(latest.getBoundingClientRect().bottom - bottom) < 5 &&
          element.scrollHeight - element.scrollTop - element.clientHeight > 80
        );
      }),
    )
    .toBe(true);
  const history: ChannelMessagePage = await page.evaluate(() =>
    fetch("/api/workspaces/workspace-one/message-history").then((response) =>
      response.json(),
    ),
  );
  await page.route(
    "**/api/workspaces/workspace-one/message-history**",
    (route) => route.fulfill({ json: history }),
  );
  await feed.evaluate((element) => {
    element.scrollTop = 0;
    element.dispatchEvent(new Event("scroll"));
  });
  const latest = history.messages.at(-1)!;
  latest.message = {
    ...latest.message,
    id: "manual-reading-update",
    content: "Update while reading older history.",
  };
  await expect(feed.locator("#message-manual-reading-update")).toBeAttached();
  expect(await feed.evaluate((element) => element.scrollTop)).toBeLessThan(10);
  await page
    .getByRole("button", { name: "Jump to latest messages", exact: true })
    .click();
  latest.message = {
    ...latest.message,
    id: "following-update",
    content: "Update after resuming following.",
  };
  await expect(feed.locator("#message-following-update")).toBeAttached();
  await expect
    .poll(() =>
      feed.evaluate((element) => {
        const latest = element.querySelector("#message-following-update")!;
        return (
          Math.abs(
            latest.getBoundingClientRect().bottom -
              element.getBoundingClientRect().bottom,
          ) < 5
        );
      }),
    )
    .toBe(true);
  await expect(
    page.getByRole("button", { name: "Jump to latest messages", exact: true }),
  ).toHaveCount(0);
  expect(errors).toEqual([]);
});

for (const locale of ["en-US", "ja-JP"] as const) {
  test(`progress overview remains reachable after every detail tab in ${locale}`, async ({
    page,
  }) => {
    const ja = locale === "ja-JP";
    if (ja) await page.setViewportSize({ width: 390, height: 1000 });
    const { errors } = await setup(page, { locale });
    await page.goto("/collaboration?channel=workspace-one");
    const open = page.getByRole("button", {
      name: ja ? "状況を表示" : "Show channel status",
      exact: true,
    });
    await open.click();
    const sheet = page.locator(".intent-progress-sheet");
    const nav = sheet.locator(".intent-progress-nav");
    const pause = sheet.getByRole("button", {
      name: ja
        ? "このチャンネルの実行を一時停止"
        : "Pause runs in this channel",
      exact: true,
    });
    await expect(pause).toBeVisible();
    for (const name of ja
      ? ["タスクと成果物", "成果物", "実行履歴"]
      : ["Tasks and results", "Artifacts", "Execution history"]) {
      await nav.getByRole("button", { name, exact: true }).click();
      await expect(pause).toHaveCount(0);
      await nav
        .getByRole("button", { name: ja ? "概要" : "Overview", exact: true })
        .click();
      await expect(pause).toBeVisible();
    }
    await nav
      .getByRole("button", {
        name: ja ? "タスクと成果物" : "Tasks and results",
        exact: true,
      })
      .click();
    await sheet
      .getByRole("button", { name: ja ? "閉じる" : "Close", exact: true })
      .click();
    await expect(sheet).toHaveCount(0);
    await open.click();
    await expect(pause).toBeVisible();
    expect(errors).toEqual([]);
  });
}

test("revoking a thread read hides cached replies without exposing another conversation", async ({
  page,
}) => {
  const { revokeThreads } = await setup(page);
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Reply in thread", exact: true })
    .click();
  const threadPanel = page.locator(".workspace-thread-panel");
  await expect(
    threadPanel.getByText("Evidence is ready to review.", { exact: true }),
  ).toBeVisible();
  revokeThreads();
  await expect(page.getByRole("alert")).toContainText("unavailable");
  await expect(threadPanel.locator(".collab-message")).toHaveCount(0);
  await expect(
    threadPanel.getByRole("textbox", { name: "Reply", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("textbox", { name: "Message", exact: true }),
  ).toBeVisible();
});

test("attachment retries preserve uploads, files and message identity", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { failFirstMessage: true });
  await page.goto("/");
  await page
    .getByRole("textbox", { name: "Message", exact: true })
    .fill("Please review the source.");
  await page.locator('input[type="file"]').setInputFiles({
    name: "source.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("evidence"),
  });
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Your text has been kept",
  );
  await expect(
    page.getByRole("button", { name: "Remove attachment: source.txt" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Download attachment: source.txt" }),
  ).toBeVisible();
  await expect(page.locator(".workspace-draft-files")).toHaveCount(0);
  const uploads = submissions.filter((item) =>
    item.path.endsWith("/attachments"),
  );
  const messages = submissions.filter((item) =>
    item.path.endsWith("/thread-messages"),
  );
  expect(uploads).toHaveLength(1);
  expect(uploads[0].body.size).toBe(8);
  expect(messages).toHaveLength(2);
  expect(messages[0].body).toEqual(messages[1].body);
  expect(messages[0].body.attachment_ids).toHaveLength(1);
  expect(errors).toEqual([]);
});

test("upload failures retain their key and file limits apply before network use", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { failFirstUpload: true });
  await page.goto("/");
  const files = page.locator('input[type="file"]');
  await files.setInputFiles({
    name: "large.txt",
    mimeType: "text/plain",
    buffer: Buffer.alloc(1024 * 1024 + 1),
  });
  await expect(page.getByRole("alert")).toContainText("1 MiB");
  expect(submissions).toHaveLength(0);
  await files.setInputFiles({
    name: "source.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("evidence"),
  });
  await page
    .getByRole("textbox", { name: "Message", exact: true })
    .fill("Source");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("upload unavailable");
  expect(
    submissions.filter((item) => item.path.endsWith("/thread-messages")),
  ).toHaveLength(0);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Download attachment: source.txt" }),
  ).toBeVisible();
  const uploads = submissions.filter((item) =>
    item.path.endsWith("/attachments"),
  );
  expect(uploads).toHaveLength(2);
  expect(uploads[0].body.idempotency_key).toBe(uploads[1].body.idempotency_key);
  expect(errors).toEqual([]);
});

test("channel controls scope run changes and persist approval responses", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { approval: true });
  await page.goto("/");
  await page
    .getByRole("button", { name: "Show channel status", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Pause runs in this channel", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Resume runs in this channel" }),
  ).toBeEnabled();
  expect(
    submissions
      .filter((item) => item.path.endsWith("/control"))
      .map((item) => item.path),
  ).toEqual(["/api/runs/run-0/control"]);
  await page
    .getByRole("button", { name: "Resume runs in this channel" })
    .click();
  await expect(
    page.getByRole("button", { name: "Pause runs in this channel" }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "Close", exact: true }).click();
  await page.getByRole("button", { name: "Approve", exact: true }).click();
  await expect(page.locator(".workspace-approval")).toHaveCount(0);
  expect(
    submissions.find((item) => item.path.endsWith("/answer"))?.body,
  ).toEqual({ approved: true });
  expect(errors).toEqual([]);
});

test("search, notifications, thread navigation and persisted theme work", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/");
  await page
    .getByRole("searchbox", { name: "Search history" })
    .fill("next release");
  await expect(
    page.locator(".intent-sidebar").getByRole("link", { name: "# Research" }),
  ).toHaveCount(0);
  await expect(page.getByRole("link", { name: "# Planning" })).toBeVisible();
  await page.getByRole("searchbox", { name: "Search history" }).fill("");
  await page
    .getByRole("button", { name: "Notifications", exact: true })
    .click();
  await page
    .locator(".intent-notifications")
    .getByRole("button", { name: /Which market should I examine\?/ })
    .click();
  await expect(page.locator(".intent-notifications")).toHaveCount(0);
  await expect(
    page.getByRole("dialog", { name: "Human input", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Close", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByLabel("Account settings", { exact: true }).click();
  await page.getByRole("button", { name: "Dark theme", exact: true }).click();
  await expect(page.locator(".collab-app")).toHaveAttribute(
    "data-theme",
    "dark",
  );
  await page.reload();
  await expect(page.locator(".collab-app")).toHaveAttribute(
    "data-theme",
    "dark",
  );
  await page
    .getByRole("button", { name: "Request actions", exact: true })
    .click();
  await page
    .getByRole("menuitem", { name: "Threads in this channel", exact: true })
    .click();
  await expect(
    page.getByText("No threads in the loaded history."),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Show channel status", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Tasks and results", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Add task", exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("composer preserves IME input and safely renders inline formatting", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page);
  await page.goto("/");
  const composer = page.getByRole("textbox", { name: "Message", exact: true });
  await composer.fill("draft");
  await composer.press("Shift+Enter");
  await expect(composer).toHaveValue("draft\n");
  await composer.dispatchEvent("keydown", {
    key: "Enter",
    code: "Enter",
    isComposing: true,
  });
  expect(submissions).toHaveLength(0);
  await composer.fill("**Important** <script>alert('xss')</script>");
  await composer.press("Enter");
  await expect(composer).toHaveValue("");
  await expect(page.locator(".workspace-message-body p strong")).toHaveText(
    "Important",
  );
  await expect(page.locator(".workspace-message-body p").last()).toContainText(
    "<script>",
  );
  expect(errors).toEqual([]);
});

for (const viewport of [
  { width: 390, height: 844 },
  { width: 900, height: 400 },
]) {
  test(`composer and status remain in view at ${viewport.width}x${viewport.height}`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    const { errors } = await setup(page, { olderMessages: 45 });
    await page.goto("/");
    await expect(
      page.getByRole("textbox", { name: "Message", exact: true }),
    ).toBeInViewport();
    await expect(
      page.getByRole("button", { name: "Send", exact: true }),
    ).toBeInViewport();
    await page.getByRole("button", { name: "Show channel status" }).click();
    await expect(
      page.getByRole("button", { name: "Pause runs in this channel" }),
    ).toBeInViewport();
    await page.getByRole("button", { name: "Close", exact: true }).click();
    await expect(
      page.getByRole("textbox", { name: "Message", exact: true }),
    ).toBeInViewport();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    expect(errors).toEqual([]);
  });
}

test("workspace reference renders light, dark and thread layouts", async ({
  page,
}, testInfo) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const { errors } = await setup(page, {
    referenceLayout: true,
    locale: "ja-JP",
  });
  await page.goto("/");
  await expect(page.locator(".workspace-approval")).toBeVisible();
  await page.getByRole("button", { name: "状況を表示", exact: true }).click();
  await expect(
    page.locator(".workspace-mini-graph canvas").first(),
  ).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("workspace-light.png") });
  await page.locator(".workspace-status").screenshot({
    path: testInfo.outputPath("workspace-status.png"),
  });
  await page.keyboard.press("Escape");
  await page.locator(".collab-composer").screenshot({
    path: testInfo.outputPath("workspace-composer.png"),
  });
  await page.getByLabel("アカウント設定", { exact: true }).click();
  await page.getByRole("button", { name: "ダークテーマ", exact: true }).click();
  await page.getByLabel("アカウント設定", { exact: true }).click();
  await page.screenshot({
    path: testInfo.outputPath("workspace-dark.png"),
    animations: "disabled",
  });
  await page
    .locator("#message-message-one")
    .getByRole("button", { name: "スレッドで返信" })
    .click();
  await expect(
    page.getByRole("textbox", { name: "返信", exact: true }),
  ).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("workspace-thread.png") });
  await page.setViewportSize({ width: 1280, height: 960 });
  await page.screenshot({
    path: testInfo.outputPath("workspace-thread-tall.png"),
  });
  expect(errors).toEqual([]);
});

test("shared files can be inspected without leaving the workspace", async ({
  page,
}) => {
  const { errors } = await setup(page, { messageAttachment: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Files", exact: true }).click();
  const files = page.locator(".workspace-file-gallery");
  await expect(
    files.getByRole("button", { name: "Download attachment: evidence.txt" }),
  ).toBeVisible();
  await files.getByRole("button", { name: "Preview: evidence.txt" }).click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("heading", { name: "evidence.txt" }),
  ).toBeVisible();
  await expect(dialog.locator("pre")).toHaveText("Source evidence");
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(1);
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  expect(errors).toEqual([]);
});

for (const width of [1440, 390]) {
  test(`remote execution reconciles controls and retries one instruction at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 800 });
    const { errors } = await setup(page, { subject: true });
    const grant = {
      id: "4191ce62-a1a8-42d6-9926-bd32f5e0dc34",
      task_id: "task-0",
      node_id: "aidash://remote",
      agent: { id: "researcher", version: "1.0.0" },
      expires_at: new Date(Date.now() + 3600000).toISOString(),
      revoked: false,
    };
    const execution = {
      grant_id: grant.id,
      admission_id: "829cb342-d2c8-43e4-99c2-0dbe96bc4f70",
      run_id: "829cb342-d2c8-43e4-99c2-0dbe96bc4f70",
      phase: "THINKING",
      control: "ACTIVE",
      error: null,
    };
    let unavailable = false;
    const messages: { id: string; content: string }[] = [];
    await page.route("**/api/tasks/task-0/remote-**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path.endsWith("/remote-executions"))
        return route.fulfill({
          json: [
            { grant, execution: unavailable ? null : execution, unavailable },
          ],
        });
      if (path.endsWith("/control")) {
        const action = route.request().postDataJSON().action;
        execution.control = {
          pause: "PAUSED",
          resume: "ACTIVE",
          cancel: "CANCELLED",
        }[action as "pause"];
        return route.fulfill({ json: execution });
      }
      if (path.endsWith("/messages")) {
        messages.push(route.request().postDataJSON());
        return messages.length === 1
          ? route.fulfill({
              status: 503,
              json: {
                error: { message: "Receipt unavailable. Retry to reconcile." },
              },
            })
          : route.fulfill({
              json: {
                id: messages[0].id,
                run_id: execution.run_id,
                accepted: true,
              },
            });
      }
      return route.fallback();
    });
    await page.goto("/collaboration?channel=workspace-one");
    await page.getByRole("button", { name: "Tasks and results" }).click();
    await page
      .locator(".collab-channel .collab-task")
      .filter({ hasText: "Collect evidence" })
      .click();
    const panel = page.getByRole("region", { name: "Remote execution" });
    await expect(
      panel.getByText("aidash://remote", { exact: true }),
    ).toBeVisible();
    await panel.getByRole("button", { name: "Pause", exact: true }).click();
    const resume = panel.getByRole("button", {
      name: "Recheck authority and resume",
    });
    await expect(resume).toBeVisible();
    await resume.focus();
    await page.keyboard.press("Enter");
    await expect(
      panel.getByRole("button", { name: "Pause", exact: true }),
    ).toBeVisible();
    const input = panel.getByLabel("Instruction for the remote Agent");
    await input.fill("Keep the original source and report the uncertainty.");
    await panel.getByRole("button", { name: "Send instruction" }).click();
    await expect(input).toBeDisabled();
    await expect(panel.getByRole("alert")).toContainText("Receipt unavailable");
    await panel
      .getByRole("button", { name: "Retry the same instruction" })
      .click();
    await expect(panel.getByRole("status")).toContainText(
      "Instruction acceptance confirmed",
    );
    expect(messages).toHaveLength(2);
    expect(messages[1]).toEqual(messages[0]);
    await expect(input).toHaveValue("");
    unavailable = true;
    await panel.getByRole("button", { name: "Refresh status" }).click();
    await expect(
      panel.getByText("The destination is unavailable.", { exact: false }),
    ).toBeVisible();
    unavailable = false;
    grant.expires_at = "2000-01-01T00:00:00Z";
    execution.control = "PAUSED";
    await panel.getByRole("button", { name: "Refresh status" }).click();
    await expect(panel.getByText("Expired", { exact: false })).toBeVisible();
    await expect(
      panel.getByRole("button", { name: "Recheck authority and resume" }),
    ).toHaveCount(0);
    await expect(
      panel.getByRole("button", { name: "Cancel execution" }),
    ).toBeEnabled();
    const dialog = page.getByRole("dialog");
    expect(
      await dialog.evaluate(
        (element) => element.scrollWidth <= element.clientWidth,
      ),
    ).toBe(true);
    await panel.getByRole("button", { name: "Cancel execution" }).click();
    await expect(
      panel.getByRole("button", { name: "Cancel execution" }),
    ).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

test("invalid execution remains inspectable and cancellable with resume disabled", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { invalidRun: true });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Execution history", exact: true })
    .click();
  await page.locator(".collab-channel .collab-task").click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("alert")).toContainText(
    "invalid execution context",
  );
  await expect(
    dialog.getByRole("button", { name: "Resume", exact: true }),
  ).toBeDisabled();
  page.once("dialog", (dialog) => dialog.accept());
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect
    .poll(
      () =>
        submissions.filter(
          (item) =>
            item.path.endsWith("/control") && item.body.action === "cancel",
        ).length,
    )
    .toBe(1);
  expect(errors).toEqual([]);
});

test("paused failure delivery can resume while retaining its invalid Context diagnostic", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, {
    failureDeliveryRun: true,
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Execution history", exact: true })
    .click();
  await page.locator(".collab-channel .collab-task").click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("alert")).toContainText(
    "invalid execution context",
  );
  const resume = dialog.getByRole("button", { name: "Resume", exact: true });
  await expect(resume).toBeEnabled();
  await resume.click();
  await expect
    .poll(
      () =>
        submissions.filter(
          (item) =>
            item.path.endsWith("/control") && item.body.action === "resume",
        ).length,
    )
    .toBe(1);
  expect(errors).toEqual([]);
});

test("channel controls resume a validated failure delivery with an invalid Context", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, {
    failureDeliveryRun: true,
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Show channel status", exact: true })
    .click();
  const resume = page.getByRole("button", {
    name: "Resume runs in this channel",
    exact: true,
  });
  await expect(resume).toBeEnabled();
  await resume.click();
  await expect
    .poll(
      () =>
        submissions.filter(
          (item) =>
            item.path === "/api/runs/run-0/control" &&
            item.body.action === "resume",
        ).length,
    )
    .toBe(1);
  expect(errors).toEqual([]);
});
