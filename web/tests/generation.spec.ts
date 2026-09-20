import { test, expect } from "@playwright/test";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { randomUUID } from "node:crypto";

test("generation dashboard manages policy, approval, completion and retained history in both languages", async ({
  page,
  request,
}) => {
  test.setTimeout(90000);
  const provider = createServer(async (req, res) => {
    for await (const chunk of req) void chunk;
    res.writeHead(200, { "content-type": "application/json" });
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
  await new Promise<void>((resolve) =>
    provider.listen(0, "127.0.0.1", resolve),
  );
  try {
    const id = `generation-${randomUUID()}`;
    const tenant = `tenant-${id}`;
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
      expected_revision: 0,
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
    await api("/api/registry", {
      id,
      version: "1.0.0",
      kind: "model",
      name: { en: "Generation fixture model", ja: "生成確認モデル" },
      description: { en: "Local generation fixture" },
      config: {
        provider: "openai",
        model_id: "fixture",
        endpoint: `http://127.0.0.1:${(provider.address() as AddressInfo).port}/v1`,
        context_window: 128000,
        modalities: ["text"],
        cost: {},
      },
    });
    await api(`/api/authorization/${tenant}/catalog`, {
      entry: { id, version: "1.0.0" },
      expected_revision: 0,
      enabled: true,
    });
    const compactor = `${id}-jev`;
    await api("/api/registry", {
      id: compactor,
      version: "1.0.0",
      kind: "compactor",
      name: { en: "Approved compaction", ja: "承認済み圧縮" },
      description: { en: "Local System One fixture" },
      config: {
        provider: "typesafe-system-one",
        endpoint: `http://127.0.0.1:${(provider.address() as AddressInfo).port}/systemone`,
        model: "fixture-jev",
        credential_env: "AIDASH_SECRET_COMPACTION_FIXTURE",
        max_request_bytes: 200000,
        max_questions: 200,
        max_response_bytes: 16000,
      },
    });
    await api(`/api/authorization/${tenant}/catalog`, {
      entry: { id: compactor, version: "1.0.0" },
      expected_revision: 0,
      enabled: true,
    });
    const credential = await api(`/api/authorization/${tenant}/credentials`, {
      subject: "alice",
    });
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto("/");
    await page.getByLabel("アクセストークン").fill(credential.token);
    await page.getByRole("button", { name: "接続", exact: true }).click();
    await expect(page.getByRole("heading", { name: "概要." })).toBeVisible();
    const navigate = async (label: string) =>
      page
        .locator(".sidebar")
        .getByRole("link", { name: label, exact: true })
        .click();
    await navigate("Agent生成");
    await page
      .getByRole("button", { name: "生成ポリシーを作成", exact: true })
      .click();
    const dialog = page.getByRole("dialog");
    await dialog
      .getByLabel("生成ポリシーID", { exact: true })
      .fill("specialist");
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
      .getByLabel("指示", { exact: true })
      .fill("Complete the task using the approved model.");
    await dialog
      .getByLabel("権限属性（JSON）", { exact: true })
      .fill('{"team":"research"}');
    await dialog
      .getByLabel("圧縮プロバイダー", { exact: true })
      .selectOption(`${compactor}@1.0.0`);
    await dialog
      .getByLabel("Agentごとの圧縮呼び出し上限", { exact: true })
      .fill("2");
    await dialog.getByLabel("圧縮呼び出しの総予算", { exact: true }).fill("8");
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
      dialog.getByLabel("圧縮プロバイダー", { exact: true }),
    ).toHaveValue(`${compactor}@1.0.0`);
    await expect(
      dialog.getByLabel("Agentごとの圧縮呼び出し上限", { exact: true }),
    ).toHaveValue("2");
    await dialog.getByLabel("最大同時Agent数", { exact: true }).fill("2");
    await dialog.getByRole("button", { name: "保存", exact: true }).click();
    await expect(dialog).toHaveCount(0);
    await navigate("ワークスペース");
    await page
      .getByRole("button", { name: "ワークスペースを作成", exact: true })
      .click();
    await dialog.getByLabel("タイトル", { exact: true }).fill(id);
    await dialog
      .getByLabel("ゴール", { exact: true })
      .fill("Complete work with a missing specialist");
    await dialog.getByRole("button", { name: "作成", exact: true }).click();
    await expect(dialog).toHaveCount(0);
    const assign = async (title: string) => {
      await navigate("タスク");
      await page
        .getByRole("button", { name: "タスクを作成", exact: true })
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
      await expect(dialog).toHaveCount(0);
      await page.getByRole("button", { name: title, exact: true }).click();
      await dialog
        .getByRole("button", { name: "ポリシーで割り当て", exact: true })
        .click();
      await dialog
        .getByLabel("生成ポリシーID", { exact: true })
        .selectOption("specialist");
      await dialog
        .getByLabel("生成する理由", { exact: true })
        .fill(`Missing specialist: ${title}`);
      await dialog
        .getByRole("button", { name: "ポリシーで割り当て", exact: true })
        .click();
      await expect(dialog).toHaveCount(0);
      await navigate("Agent生成");
      const row = page
        .locator(".generation-request")
        .filter({ hasText: title });
      await expect(row).toContainText("承認待ち");
      return row;
    };
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
    await expect(dialog).toContainText(`${id}@1.0.0`);
    await expect(dialog.locator(".generation-permissions")).toContainText(
      '"team": "research"',
    );
    await expect(dialog.locator(".generation-permissions")).not.toContainText(
      '"team": "later"',
    );
    await dialog
      .getByLabel("判断の理由", { exact: true })
      .fill("定義と上限を確認済み");
    await expect(dialog).toContainText(`${compactor}@1.0.0`);
    await expect(dialog).toContainText("圧縮呼び出し消費数 / 上限");
    await page.screenshot({
      path: "../.ignore/dashboard-generation-approval-ja.png",
      fullPage: true,
    });
    await dialog
      .getByRole("button", { name: "生成を承認", exact: true })
      .click();
    await expect(dialog).toHaveCount(0);
    await expect(completed).toContainText("完了", { timeout: 20000 });
    await completed.click();
    await expect(dialog).toContainText("24 / 200,000");
    await expect(dialog.locator(".generation-history")).toContainText(
      "定義と上限を確認済み",
    );
    await dialog.getByRole("button", { name: "閉じる", exact: true }).click();
    await page.getByLabel("言語").selectOption("en-US");
    await expect(page.locator("main.content h1")).toHaveText(
      "Agent generation.",
    );
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
    await page.screenshot({
      path: "../.ignore/dashboard-generation-complete-en.png",
      fullPage: true,
    });
    await dialog.getByRole("button", { name: "Close", exact: true }).click();
    await page.getByLabel("Language").selectOption("ja-JP");
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
    await expect(page.locator(".generation-policy .badge")).toHaveText("無効");
    await page
      .getByRole("button", { name: "生成を有効化", exact: true })
      .click();
    await expect(page.locator(".generation-policy .badge")).toHaveText("有効");
    await page.screenshot({
      path: "../.ignore/dashboard-generation-page-ja.png",
      fullPage: true,
    });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.screenshot({
      path: "../.ignore/dashboard-generation-mobile-ja.png",
      fullPage: true,
    });
    const policies = await api(
      `/api/generation/${tenant}/policies`,
      undefined,
      credential.token,
    );
    expect(policies[0].generated_count).toBe(3);
    expect(policies[0].allocated_tokens).toBe(24);
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
  } finally {
    provider.closeAllConnections();
    await new Promise<void>((resolve, reject) =>
      provider.close((error) => (error ? reject(error) : resolve())),
    );
  }
});
