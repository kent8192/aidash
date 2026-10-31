import {
  test,
  expect,
  type APIRequestContext,
  type BrowserContext,
  type Page,
} from "@playwright/test";
import {
  createTenant,
  defaultNode,
  issueSubjectCredential,
} from "./tenant-fixture";
import {
  installBearerDashboard,
  selectDashboardLanguage,
} from "./auth-fixture";
import { createServer } from "node:http";
import { KeylessFixture, serveKeyless } from "./keyless-ports";
import { randomUUID } from "node:crypto";

async function createGenerationScenario(
  page: Page,
  request: APIRequestContext,
) {
  let embeddingCalls = 0;
  const semanticContexts: unknown[] = [];
  let semanticRoot = "";
  // alice's credential, once issued; the semantic index is her Tenant's.
  const cleanupHeaders = { authorization: "" };
  let semanticIndex:
    | { collection: string; spec: { vector: { endpoint: string } } }
    | undefined;
  const provider = createServer(async (req, res) => {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(Buffer.from(chunk));
    const input = JSON.parse(Buffer.concat(chunks).toString());
    res.writeHead(200, { "content-type": "application/json" });
    if (req.url === "/v1/embeddings") {
      embeddingCalls++;
      res.end(
        JSON.stringify({
          model: input.model,
          data: [{ index: 0, embedding: [1, 0, 0] }],
          usage: { prompt_tokens: 2, total_tokens: 2 },
        }),
      );
      return;
    }
    semanticContexts.push(
      JSON.parse(input.messages[1].content).current.semantic_memory.workspace,
    );
    res.end(
      JSON.stringify({
        choices: [
          {
            index: 0,
            finish_reason: "stop",
            message: {
              role: "assistant",
              content: "Generated specialist completed the task",
            },
          },
        ],
        usage: { prompt_tokens: 18, completion_tokens: 6 },
      }),
    );
  });
  const providerOrigin = await serveKeyless(
    provider,
    KeylessFixture.generation,
  );
  const dispose = async () => {
    provider.closeAllConnections();
    await new Promise<void>((resolve, reject) =>
      provider.close((error) => (error ? reject(error) : resolve())),
    );
    if (semanticIndex) {
      const current = await request.get(`${semanticRoot}/index`, {
        headers: cleanupHeaders,
      });
      if (current.ok()) {
        const index = await current.json();
        const response = await request.post(`${semanticRoot}/index`, {
          headers: cleanupHeaders,
          data: {
            expected_revision: index.revision,
            spec: { ...index.spec, enabled: false },
          },
        });
        expect(response.ok()).toBe(true);
      }
    }
  };
  try {
    const id = `generation-${randomUUID()}`;
    const tenant = await createTenant(
      request,
      "acceptance-access-token",
      `tenant-${id}`,
    );
    const api = async (
      path: string,
      body?: unknown,
      token = "acceptance-access-token",
    ) => {
      const options = { headers: { authorization: `Bearer ${token}` } };
      const response =
        body === undefined
          ? await request.get(path, options)
          : await request.post(path, { ...options, data: body });
      expect(response.status(), await response.text()).toBe(200);
      return response.json();
    };
    await api(`/api/authorization/${tenant}`, {
      expected_revision: 1,
      bundle: {
        tenant,
        subjects: { alice: { kind: "user" } },
        policies: [
          {
            id: "approved-work",
            effect: "allow",
            subjects: { any: true },
            actions: ["*"],
            resources: { kinds: ["*"] },
          },
        ],
      },
    });
    // A Deployment Administrator holds no Tenant authority (#484): the host issues the credential.
    const credential = issueSubjectCredential(tenant, "alice");
    cleanupHeaders.authorization = `Bearer ${credential.token}`;
    const node = await defaultNode(request, credential.token, tenant);
    await api(
      "/api/registry",
      {
        id,
        version: "1.0.0",
        kind: "model",
        name: { en: "Generation fixture model", ja: "生成確認モデル" },
        description: { en: "Local generation fixture" },
        config: {
          provider: "openrouter",
          model_id: "fixture",
          endpoint: `${providerOrigin}/v1`,
          context_window: 128000,
          max_output_tokens: 4096,
          modalities: ["text"],
          cost: {},
        },
      },
      credential.token,
    );
    await api(
      `/api/authorization/${tenant}/catalog`,
      {
        entry: { id, version: "1.0.0" },
        expected_revision: 0,
        enabled: true,
        node: node.name,
      },
      credential.token,
    );
    const decider = `${id}-jev`;
    await api(
      "/api/registry",
      {
        id: decider,
        version: "1.0.0",
        kind: "decider",
        name: { en: "Approved compaction", ja: "承認済み圧縮" },
        description: {
          en: "Approved Jev release (never called: contexts fit)",
        },
        config: {
          hook: "compaction",
          answer_type: "noul",
          description: "Keep the history the task still needs",
          provider_credential: "openrouter",
          routes: [
            {
              provider_contract: "typesafe.jev/1",
              model: "typesafe/jev-1.14",
              threshold: "3fe0000000000000",
            },
          ],
          builder: "aidash.compaction/2",
          option_source: "history_event/1",
          rule: "compaction.keep/1",
          mode: "enforce",
          max_calls_per_run: 20,
        },
      },
      credential.token,
    );
    await api(
      `/api/authorization/${tenant}/catalog`,
      {
        entry: { id: decider, version: "1.0.0" },
        expected_revision: 0,
        enabled: true,
        node: node.name,
      },
      credential.token,
    );
    const memoryDescriptor = `${id}-memory`;
    await api(
      "/api/registry",
      {
        id: memoryDescriptor,
        version: "1.0.0",
        kind: "memory",
        name: { en: "Approved semantic memory", ja: "承認済みメモリ" },
        description: {
          en: "Explicit semantic retrieval for the generated Agent",
        },
        config: { schema_version: 1, source: { adapter: "semantic_memory" } },
      },
      credential.token,
    );
    await api(
      `/api/authorization/${tenant}/catalog`,
      {
        entry: { id: memoryDescriptor, version: "1.0.0" },
        expected_revision: 0,
        enabled: true,
        node: node.name,
      },
      credential.token,
    );
    const embeddingConfig = {
      provider: "openai",
      endpoint: `${providerOrigin}/v1`,
      model: "fixture-embedding",
      model_version: "v1",
      dimensions: 3,
      inputs: ["text"],
    };
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await installBearerDashboard(page, credential.token, {
      tenant,
      name: "alice",
    });
    // The Tenant overview answers only a dashboard Mapping, never a Bearer
    // credential, so it is the one read this fixture supplies. The registry
    // still checks alice's policy on every write.
    await page.route(`**/api/tenants/${tenant}/administration`, (route) =>
      route.fulfill({
        json: {
          tenant,
          label: null,
          actions: ["registry.create"],
          assignable_groups: [],
          policy_revision: null,
        },
      }),
    );
    await page.goto("/");
    await expect(page.locator("#main")).toBeVisible();
    const navigate = async (label: string) => {
      if (label === "タスク") {
        await page.goto(
          `/collaboration?channel=${encodeURIComponent(workspace.id)}`,
        );
        await page
          .getByRole("button", { name: "タスクと成果物", exact: true })
          .click();
      } else if (label === "ワークスペース") {
        await page.goto("/collaboration");
      } else {
        await page.goto(
          `/settings?view=${label === "レジストリ" ? "registry" : "generation"}`,
        );
      }
    };
    await navigate("レジストリ");
    await page
      .getByRole("button", { name: "エンティティを登録", exact: true })
      .click();
    const registryDialog = page.getByRole("dialog");
    await registryDialog
      .getByLabel("エンティティの種類")
      .selectOption("embedding");
    await registryDialog
      .locator('[name="embedding_provider"]')
      .selectOption("openai");
    await expect(registryDialog.getByLabel("エンティティID")).toHaveCount(0);
    await registryDialog.getByLabel("名前").fill("Approved embedding");
    await registryDialog.getByLabel("説明").fill("Local semantic provider");
    await registryDialog
      .locator('[name="endpoint"]')
      .fill(embeddingConfig.endpoint);
    await registryDialog
      .locator('[name="model_id"]')
      .fill(embeddingConfig.model);
    await registryDialog
      .getByLabel("埋め込みモデルのバージョン", { exact: true })
      .fill("v1");
    await registryDialog
      .getByLabel("埋め込みの次元数", { exact: true })
      .fill("3");
    // Buffer the body before releasing it to the UI, whose success handler navigates.
    // Chromium can discard a page response body as soon as that navigation starts.
    let embedder = "";
    await page.route("**/api/registry", async (route) => {
      if (route.request().method() !== "POST") return route.fallback();
      const response = await route.fetch({
        headers: {
          ...route.request().headers(),
          authorization: `Bearer ${credential.token}`,
        },
      });
      expect(response.status(), await response.text()).toBe(200);
      embedder = (await response.json()).id as string;
      await route.fulfill({ response });
    });
    await registryDialog
      .getByRole("button", { name: "エンティティを登録", exact: true })
      .click();
    await expect(registryDialog).toHaveCount(0);
    await page.unroute("**/api/registry");
    expect(embedder).not.toBe("");
    // A Tenant subject reads only versions its Tenant approved.
    await api(
      `/api/authorization/${tenant}/catalog`,
      {
        entry: { id: embedder, version: "1.0.0" },
        expected_revision: 0,
        enabled: true,
        node: node.name,
      },
      credential.token,
    );
    expect(
      (
        await api(
          `/api/registry/${embedder}/1.0.0`,
          undefined,
          credential.token,
        )
      ).config,
    ).toEqual(embeddingConfig);
    await page.reload();
    await navigate("Agent生成");
    await page
      .getByRole("button", { name: "生成ポリシーを作成", exact: true })
      .click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("名前 · English").fill("Research specialist");
    await dialog.getByLabel("名前 · 日本語").fill("調査スペシャリスト");
    await dialog
      .getByLabel("説明 · English")
      .fill("Complete a specialist task");
    await dialog
      .getByLabel("説明 · 日本語")
      .fill("専門的な調査タスクを完了する");
    await dialog.getByLabel("能力", { exact: true }).fill(id);
    await dialog
      .getByLabel("モデル", { exact: true })
      .selectOption(`${id}@1.0.0`);
    await dialog
      .getByLabel("追加するもの", { exact: true })
      .selectOption(`memory:${memoryDescriptor}@1.0.0`);
    await dialog
      .getByRole("button", { name: "このエージェントに追加", exact: true })
      .click();
    await dialog
      .getByLabel("追加の指示（任意）", { exact: true })
      .fill("Complete the task using the approved model.");
    await dialog
      .getByLabel("権限属性（JSON）", { exact: true })
      .fill('{"team":"research"}');
    await dialog
      .getByLabel("圧縮 Decider", { exact: true })
      .selectOption(`${decider}@1.0.0`);
    await dialog
      .getByLabel("Agentごとの圧縮呼び出し上限", { exact: true })
      .fill("2");
    await dialog.getByLabel("圧縮呼び出しの総予算", { exact: true }).fill("8");
    await dialog
      .getByLabel("埋め込みプロバイダー", { exact: true })
      .selectOption(`${embedder}@1.0.0`);
    await dialog
      .getByLabel("Agentごとの埋め込み呼び出し上限", { exact: true })
      .fill("2");
    await dialog
      .getByLabel("埋め込み呼び出しの総予算", { exact: true })
      .fill("8");
    await expect(dialog.getByLabel("実行前に承認を必要とする")).toBeChecked();
    await dialog.getByRole("button", { name: "保存", exact: true }).click();
    await expect(dialog).toHaveCount(0);
    await expect(page.locator(".generation-policy")).toContainText(
      "調査スペシャリスト",
    );
    await page
      .getByRole("button", { name: "ポリシーを編集", exact: true })
      .click();
    await expect(
      dialog.getByLabel("圧縮 Decider", { exact: true }),
    ).toHaveValue(`${decider}@1.0.0`);
    await expect(
      dialog.getByLabel("Agentごとの圧縮呼び出し上限", { exact: true }),
    ).toHaveValue("2");
    await dialog.getByLabel("最大同時Agent数", { exact: true }).fill("2");
    await expect(
      dialog.getByLabel("埋め込みプロバイダー", { exact: true }),
    ).toHaveValue(`${embedder}@1.0.0`);
    await expect(
      dialog.getByLabel("Agentごとの埋め込み呼び出し上限", { exact: true }),
    ).toHaveValue("2");
    await dialog.getByRole("button", { name: "保存", exact: true }).click();
    await expect(dialog).toHaveCount(0);
    await navigate("ワークスペース");
    await page.getByRole("button", { name: "設定", exact: true }).click();
    await page
      .getByRole("button", { name: "準備用チャンネル", exact: true })
      .click();
    await dialog.getByLabel("タイトル", { exact: true }).fill(id);
    await dialog
      .getByLabel("ゴール", { exact: true })
      .fill("Complete work with a missing specialist");
    await dialog.getByRole("button", { name: "作成", exact: true }).click();
    // The dialog closes once the refreshed state lists the new channel (#404).
    await expect(dialog).toHaveCount(0, { timeout: 30000 });
    const { workspaces } = await api("/api/state", undefined, credential.token);
    const workspace = workspaces.find(
      (item: { title: string }) => item.title === id,
    );
    semanticRoot = `/api/workspaces/${workspace.id}/semantic`;
    semanticIndex = await api(
      `${semanticRoot}/index`,
      {
        expected_revision: 0,
        spec: {
          embedding: embeddingConfig,
          vector: {
            provider: "postgres",
            endpoint: "local",
            credential_env: null,
          },
          enabled: true,
          auto_context: true,
          max_sources: 64,
          max_results: 10,
          max_result_tokens: 4096,
          max_input_bytes: 8192,
        },
      },
      credential.token,
    );
    const memory = await api(
      `${semanticRoot}/entries`,
      {
        key: "vehicle",
        expected_revision: 0,
        source: { kind: "memory", text: "A car carries passengers." },
        metadata: {},
      },
      credential.token,
    );
    await expect
      .poll(
        async () =>
          (await api(`${semanticRoot}/entries`, undefined, credential.token))[0]
            .state,
      )
      .toBe("READY");
    const assign = async (title: string) => {
      await navigate("タスク");
      await page
        .getByRole("button", { name: "タスクを追加", exact: true })
        .click();
      await dialog
        .getByLabel("ワークスペース", { exact: true })
        .selectOption({ label: id });
      await dialog.getByLabel("タイトル", { exact: true }).fill(title);
      await dialog
        .getByLabel("説明", { exact: true })
        .fill("Research with a generated specialist");
      await dialog
        .getByLabel("タスクの要件", { exact: true })
        .fill(JSON.stringify({ capability: id }));
      await dialog.getByRole("button", { name: "作成", exact: true }).click();
      await expect(dialog).toHaveCount(0, { timeout: 15000 });
      // Tasks are listed in the channel's "Tasks and results" sheet.
      await page
        .getByRole("button", { name: "タスクと成果物", exact: true })
        .click();
      await dialog.getByRole("button").filter({ hasText: title }).click();
      await dialog
        .getByRole("button", { name: "ポリシーで割り当て", exact: true })
        .click();
      await dialog
        .getByLabel("生成ポリシー", { exact: true })
        .selectOption({ index: 1 });
      await dialog
        .getByLabel("生成する理由", { exact: true })
        .fill(`Missing specialist: ${title}`);
      await dialog
        .getByRole("button", { name: "ポリシーで割り当て", exact: true })
        .click();
      // Assignment reloads the real server state before closing the dialog.
      await expect(dialog).toHaveCount(0, { timeout: 15000 });
      await navigate("Agent生成");
      const row = page
        .locator(".generation-request")
        .filter({ hasText: title });
      await expect(row).toContainText("承認待ち");
      return row;
    };
    const approveAndComplete = async () => {
      const completed = await assign("Generated research");
      await page
        .getByRole("button", { name: "ポリシーを編集", exact: true })
        .click();
      await dialog
        .getByLabel("権限属性（JSON）", { exact: true })
        .fill('{"team":"later"}');
      await dialog.getByRole("button", { name: "保存", exact: true }).click();
      await expect(dialog).toHaveCount(0);
      await completed.click();
      await expect(dialog).toContainText("調査スペシャリスト");
      await expect(dialog).toContainText("生成確認モデル · 1.0.0");
      await expect(dialog.locator(".generation-permissions")).toContainText(
        '"team": "research"',
      );
      await expect(dialog.locator(".generation-permissions")).not.toContainText(
        '"team": "later"',
      );
      await dialog
        .getByLabel("判断の理由", { exact: true })
        .fill("定義と上限を確認済み");
      await expect(dialog).toContainText("承認済み圧縮 · 1.0.0");
      await expect(dialog).toContainText("圧縮呼び出し消費数 / 上限");
      await expect(dialog).toContainText("Approved embedding · 1.0.0");
      await expect(dialog).toContainText("埋め込み試行数");
      await page.screenshot({
        path: "../private/dashboard-generation-approval-ja.png",
        fullPage: true,
      });
      await dialog
        .getByRole("button", { name: "生成を承認", exact: true })
        .click();
      await expect(dialog).toHaveCount(0);
      // A deferred activation reaches the Run through recovery, ~10 s later.
      await expect(completed).toContainText("完了", { timeout: 60000 });
      await completed.click();
      await expect(dialog).toContainText("26 / 200,000");
      expect(semanticContexts).toEqual([
        expect.objectContaining({
          matches: [
            expect.objectContaining({
              entry_id: memory.id,
              text: "A car carries passengers.",
            }),
          ],
        }),
      ]);
      expect(embeddingCalls).toBe(2);
      await expect(dialog.locator(".generation-history")).toContainText(
        "定義と上限を確認済み",
      );
      await dialog.getByRole("button", { name: "閉じる", exact: true }).click();
      await selectDashboardLanguage(page, "en-US");
      await expect(page).toHaveURL(/\/creator\?.*view=generation/);
      await expect(page.locator(".generation-page")).toBeVisible();
      await page
        .locator(".generation-request")
        .filter({ hasText: "Generated research" })
        .click();
      await expect(
        dialog.getByText("Lifecycle history", { exact: true }),
      ).toBeVisible();
      await expect(
        dialog.getByText("Charged tokens / limit", { exact: true }),
      ).toBeVisible();
      await expect(
        dialog.getByText("Embedding attempts", { exact: true }),
      ).toBeVisible();
      await page.screenshot({
        path: "../private/dashboard-generation-complete-en.png",
        fullPage: true,
      });
      await dialog.getByRole("button", { name: "Close", exact: true }).click();
      await selectDashboardLanguage(page, "ja-JP");
    };
    const rejectAndArchive = async () => {
      const denied = await assign("Rejected specialist");
      await denied.click();
      await dialog
        .getByLabel("判断の理由", { exact: true })
        .fill("今回は生成しない");
      await dialog
        .getByRole("button", { name: "生成を拒否", exact: true })
        .click();
      await expect(dialog).toHaveCount(0);
      await expect(denied).toContainText("拒否済み");
      const stopped = await assign("Stopped specialist");
      await stopped.click();
      await dialog
        .getByLabel("判断の理由", { exact: true })
        .fill("要求を停止する");
      await dialog
        .getByRole("button", { name: "Agentを停止", exact: true })
        .click();
      await expect(dialog).toHaveCount(0);
      await expect(stopped).toContainText("停止済み");
      await stopped.click();
      await dialog
        .getByLabel("判断の理由", { exact: true })
        .fill("履歴を保持して整理する");
      await dialog
        .getByRole("button", { name: "要求をアーカイブ", exact: true })
        .click();
      await expect(dialog).toHaveCount(0);
      await expect(stopped).toContainText("アーカイブ済み");
      await page
        .getByRole("button", { name: "生成を無効化", exact: true })
        .click();
      await expect(page.locator(".generation-policy .badge")).toHaveText(
        "無効",
      );
      await page
        .getByRole("button", { name: "生成を有効化", exact: true })
        .click();
      await expect(page.locator(".generation-policy .badge")).toHaveText(
        "有効",
      );
      await page.screenshot({
        path: "../private/dashboard-generation-page-ja.png",
        fullPage: true,
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await page.screenshot({
        path: "../private/dashboard-generation-mobile-ja.png",
        fullPage: true,
      });
      const policies = await api(
        `/api/generation/${tenant}/policies`,
        undefined,
        credential.token,
      );
      expect(policies[0].generated_count).toBe(3);
      expect(policies[0].allocated_tokens).toBe(26);
      expect(policies[0].allocated_embedding_calls).toBe(1);
      const jobs = await api(
        `/api/generation/${tenant}/requests`,
        undefined,
        credential.token,
      );
      expect(jobs.map((job: { status: string }) => job.status).sort()).toEqual([
        "COMPLETED",
        "DELETED",
        "DENIED",
      ]);
      expect(errors).toEqual([]);
    };
    return { approveAndComplete, rejectAndArchive, dispose };
  } catch (error) {
    await dispose();
    throw error;
  }
}

// The journey shares one browser and Tenant deliberately. Each serial phase
// has its own deadline, so slow approval/execution cannot spend the history
// and policy-management phase's budget. A failed phase skips its dependants.
test.describe.serial("generation dashboard", () => {
  test.describe.configure({ timeout: 90000 });
  let context: BrowserContext;
  let scenario: Awaited<ReturnType<typeof createGenerationScenario>>;

  test.beforeAll(async ({ browser, baseURL }) => {
    context = await browser.newContext({
      baseURL,
      viewport: { width: 1440, height: 1000 },
    });
  });
  test.afterAll(async () => {
    try {
      await scenario?.dispose();
    } finally {
      await context?.close();
    }
  });

  test("configures the embedding model and generation policy", async () => {
    scenario = await createGenerationScenario(
      await context.newPage(),
      context.request,
    );
  });
  test("approves a pinned definition and completes semantic work in both languages", async () => {
    await scenario.approveAndComplete();
  });
  test("rejects and archives requests while retaining history and policy controls", async () => {
    await scenario.rejectAndArchive();
  });
});
