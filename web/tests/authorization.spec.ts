import { test, expect } from "@playwright/test";
import { randomUUID } from "node:crypto";

test("authorization dashboard manages revisions, RBAC/ABAC decisions, catalog and credential revocation", async ({
  page,
  request,
  browser,
}) => {
  test.setTimeout(90000);
  const tenant = `policy-${randomUUID()}`;
  const base = `/api/authorization/${tenant}`;
  const headers = { authorization: "Bearer acceptance-access-token" };
  const api = async (path: string, data?: unknown) => {
    const response =
      data === undefined
        ? await request.get(path, { headers })
        : await request.post(path, { headers, data });
    expect(response.status(), await response.text()).toBe(200);
    return response.json();
  };
  const model = `model-${tenant}`;
  await api("/api/registry", {
    id: model,
    version: "1.0.0",
    kind: "model",
    name: { en: "Policy fixture model", ja: "権限確認モデル" },
    description: { en: "Local authorization fixture" },
    config: {
      provider: "openrouter",
      model_id: "fixture",
      endpoint: "http://127.0.0.1:1/v1",
      context_window: 128000,
      max_output_tokens: 4096,
      modalities: ["text"],
      cost: {},
    },
  });
  const policy = {
    tenant,
    subjects: {
      alice: {
        kind: "user",
        groups: ["research"],
        attributes: { team: "research" },
      },
    },
    groups: { research: { roles: ["researcher"] } },
    roles: { researcher: { inherits: ["reader"] }, reader: {} },
    policies: [
      {
        id: "team-read",
        effect: "allow",
        subjects: { roles: ["reader"] },
        actions: ["workspace.read"],
        resources: { kinds: ["workspace"] },
        condition: {
          op: "eq",
          left: { source: "subject", path: "/team" },
          right: { source: "resource", path: "/team" },
        },
      },
    ],
  };
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() => {
    sessionStorage.setItem("aidash-token", "acceptance-access-token");
    localStorage.setItem("aidash-locale", "ja-JP");
  });
  await page.goto("/authorization");
  await page.getByLabel("テナント", { exact: true }).fill(tenant);
  await page.getByRole("button", { name: "開く", exact: true }).click();
  await page
    .getByRole("button", { name: "権限ポリシーを作成", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("ポリシー定義（JSON）").fill(JSON.stringify(policy));
  await dialog
    .getByRole("button", { name: "ポリシーを確認", exact: true })
    .click();
  await expect(dialog).toContainText("置き換え内容を確認");
  await dialog
    .getByRole("button", { name: "ポリシーを適用", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  const testPanel = page.locator(".panel").filter({
    has: page.getByRole("heading", { name: "権限判定を確認", exact: true }),
  });
  await testPanel
    .getByLabel("リソース属性（JSON）")
    .fill('{"team":"research"}');
  await testPanel
    .getByRole("button", { name: "ドライラン", exact: true })
    .click();
  await expect(testPanel.locator(".auth-decision")).toContainText("許可");
  await expect(testPanel.locator(".auth-decision")).toContainText(
    "reader, researcher",
  );
  expect(await api(`${base}/decisions`)).toEqual([]);
  await testPanel.getByLabel("リソース属性（JSON）").fill('{"team":"sales"}');
  await expect(testPanel.locator(".auth-decision")).toHaveCount(0);
  await testPanel
    .getByRole("button", { name: "ドライラン", exact: true })
    .click();
  await expect(testPanel.locator(".auth-decision")).toContainText(
    "一致する許可ルールがありません",
  );
  await testPanel.getByLabel("この判定を監査履歴に保存する").check();
  await testPanel
    .getByRole("button", { name: "判定して記録", exact: true })
    .click();
  await expect
    .poll(async () => (await api(`${base}/decisions`)).length)
    .toBe(1);

  // A concurrent operator update must not overwrite the editor's pinned revision.
  await page
    .getByRole("button", { name: "権限ポリシーを編集", exact: true })
    .click();
  await api(base, { expected_revision: 1, bundle: policy });
  await dialog
    .getByRole("button", { name: "ポリシーを確認", exact: true })
    .click();
  await dialog
    .getByRole("button", { name: "ポリシーを適用", exact: true })
    .click();
  await expect(dialog.getByRole("alert")).toContainText(
    "リビジョンが更新されています",
  );
  expect((await api(base)).revision).toBe(2);
  await dialog.getByRole("button", { name: "編集に戻る", exact: true }).click();
  await expect(dialog.getByLabel("ポリシー定義（JSON）")).toHaveValue(
    /team-read/,
  );
  await dialog.getByRole("button", { name: "閉じる", exact: true }).click();

  // Explicit deny overrides the inherited role allow.
  await page
    .getByRole("button", { name: "権限ポリシーを編集", exact: true })
    .click();
  const latest = JSON.parse(
    await dialog.getByLabel("ポリシー定義（JSON）").inputValue(),
  );
  latest.policies.push({
    id: "hold-access",
    effect: "deny",
    subjects: { ids: ["alice"] },
    actions: ["workspace.read"],
    resources: { kinds: ["workspace"] },
  });
  await dialog.getByLabel("ポリシー定義（JSON）").fill(JSON.stringify(latest));
  await dialog
    .getByRole("button", { name: "ポリシーを確認", exact: true })
    .click();
  await dialog
    .getByRole("button", { name: "ポリシーを適用", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  await testPanel
    .getByLabel("リソース属性（JSON）")
    .fill('{"team":"research"}');
  await testPanel
    .getByRole("button", { name: "ドライラン", exact: true })
    .click();
  await expect(testPanel.locator(".auth-decision")).toContainText(
    "明示的な拒否",
  );

  await page
    .getByRole("button", { name: "認証情報を発行", exact: true })
    .click();
  await dialog.getByLabel("有効期間（秒）").fill("120");
  await dialog
    .getByRole("button", { name: "認証情報を発行", exact: true })
    .click();
  const token = await dialog.getByLabel("発行したトークン").inputValue();
  expect(token).toBeTruthy();
  await expect(dialog).toContainText("この画面でのみ表示");
  expect(
    (
      await request.get("/api/session", {
        headers: { authorization: `Bearer ${token}` },
      })
    ).status(),
  ).toBe(200);
  await dialog
    .getByRole("button", { name: "保存したので閉じる", exact: true })
    .click();
  await expect(page.getByLabel("発行したトークン")).toHaveCount(0);
  expect(
    await page.evaluate(() =>
      JSON.stringify({ ...localStorage, ...sessionStorage }),
    ),
  ).not.toContain(token);
  const subjectContext = await browser.newContext({
    baseURL: process.env.AIDASH_E2E_URL ?? "http://127.0.0.1:18080",
  });
  try {
    const subjectPage = await subjectContext.newPage();
    await subjectPage.addInitScript((value) => {
      sessionStorage.setItem("aidash-token", value);
      localStorage.setItem("aidash-locale", "en-US");
    }, token);
    await subjectPage.goto("/authorization");
    await expect(
      subjectPage.getByText("This page is available to administrators."),
    ).toBeVisible();
    await expect(
      subjectPage
        .locator(".sidebar")
        .getByRole("link", { name: "Access policies", exact: true }),
    ).toHaveCount(0);
    expect(
      (
        await request.get(base, {
          headers: { authorization: `Bearer ${token}` },
        })
      ).status(),
    ).toBe(403);
  } finally {
    await subjectContext.close();
  }
  await page.getByRole("button", { name: "失効", exact: true }).click();
  await dialog.getByRole("button", { name: "失効を確定", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(
    page.locator(".auth-row").filter({ hasText: "alice" }),
  ).toContainText("失効済み");
  expect(
    (
      await request.get("/api/session", {
        headers: { authorization: `Bearer ${token}` },
      })
    ).status(),
  ).toBe(401);

  const state = await api("/api/state");
  const entry = state.registry.find(
    (value: { id: string }) => value.id === model,
  );
  expect(entry).toBeTruthy();
  await page
    .getByLabel("コンポーネントのバージョン", { exact: true })
    .selectOption(JSON.stringify([entry.id, entry.version]));
  const catalogPanel = page.locator(".panel").filter({
    has: page.getByRole("heading", {
      name: "承認済みコンポーネント",
      exact: true,
    }),
  });
  await catalogPanel.getByRole("button", { name: "承認", exact: true }).click();
  await expect(catalogPanel.locator(".auth-row")).toContainText("承認済み");
  await catalogPanel
    .getByRole("button", { name: "承認を無効化", exact: true })
    .click();
  await expect(catalogPanel.locator(".auth-row .badge")).toHaveText("無効");
  expect((await api(`${base}/catalog`))[0]).toMatchObject({
    enabled: false,
    revision: 2,
  });

  for (let i = 0; i < 26; i++)
    await api(`${base}/evaluate`, {
      subject: "alice",
      action: "workspace.read",
      resource: {
        tenant,
        kind: "workspace",
        id: `audit-${i}`,
        attributes: { team: "research" },
      },
    });
  const auditRecords = await api(`${base}/decisions?limit=200`);
  const history = page.locator(".panel").filter({
    has: page.getByRole("heading", {
      name: "権限判定の監査履歴",
      exact: true,
    }),
  });
  await expect(
    history.getByRole("button", { name: "次のページ", exact: true }),
  ).toBeEnabled({ timeout: 10000 });
  await history
    .getByRole("button", { name: "次のページ", exact: true })
    .click();
  await expect(history.locator(".auth-history")).toHaveCount(
    auditRecords.length - 25,
  );
  await history
    .getByRole("button", { name: "前のページ", exact: true })
    .click();
  await expect(history.locator(".auth-history")).toHaveCount(25);
  await page.getByLabel("言語", { exact: true }).selectOption("en-US");
  await expect(
    page.getByRole("heading", { name: "Access policies." }),
  ).toBeVisible();
  await page.screenshot({
    path: "../.ignore/dashboard-authorization-desktop.png",
    fullPage: true,
  });
  await page.getByLabel("Language", { exact: true }).selectOption("ja-JP");
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({
    path: "../.ignore/dashboard-authorization-mobile.png",
    fullPage: true,
  });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.getByLabel("テナント", { exact: true }).fill(`${tenant}-other`);
  await page.getByRole("button", { name: "開く", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "権限ポリシーを作成", exact: true }),
  ).toBeVisible();
  await expect(page.locator(".auth-decision")).toHaveCount(0);
  await expect(page.locator(".auth-history")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("peer identity mappings preserve revisions, credential rotation and bilingual administration", async ({
  page,
  request,
}) => {
  const tenant = `peer-${randomUUID()}`;
  const base = `/api/authorization/${tenant}`;
  const headers = { authorization: "Bearer acceptance-access-token" };
  const api = async (path: string, data?: unknown) => {
    const response =
      data === undefined
        ? await request.get(path, { headers })
        : await request.post(path, { headers, data });
    expect(response.status(), await response.text()).toBe(200);
    return response.json();
  };
  const state = await api("/api/state");
  const peer = state.peers.find((value: { enabled: boolean }) => value.enabled);
  expect(peer, "fixture requires a configured peer").toBeTruthy();
  await api(base, {
    expected_revision: 0,
    bundle: { tenant, subjects: { bridge: { kind: "user" } }, policies: [] },
  });
  const first = await api(`${base}/credentials`, { subject: "bridge" });
  const second = await api(`${base}/credentials`, { subject: "bridge" });
  await page.addInitScript(() => {
    sessionStorage.setItem("aidash-token", "acceptance-access-token");
    localStorage.setItem("aidash-locale", "en-US");
  });
  await page.goto("/authorization");
  await page.getByLabel("Tenant", { exact: true }).fill(tenant);
  await page.getByRole("button", { name: "Open", exact: true }).click();
  const panel = page.locator(".panel").filter({
    has: page.getByRole("heading", {
      name: "Peer identity mappings",
      exact: true,
    }),
  });
  await panel.getByRole("button", { name: "Add peer mapping" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Source node", { exact: true }).fill(peer.node_id);
  await dialog
    .getByLabel("Source tenant", { exact: true })
    .fill("remote-tenant");
  await dialog
    .getByLabel("Source subject", { exact: true })
    .fill("remote-user");
  await dialog
    .getByLabel("Local credential", { exact: true })
    .selectOption(first.credential.id);
  await dialog.getByRole("button", { name: "Save and enable mapping" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(panel).toContainText(first.credential.id);
  expect((await api(`${base}/peer-mappings`))[0].revision).toBe(1);
  await panel.getByRole("button", { name: "Edit peer mapping" }).click();
  await expect(
    dialog.getByLabel("Source node", { exact: true }),
  ).toHaveAttribute("readonly", "");
  // An operator on another session advances the revision while this form is open.
  const current = (await api(`${base}/peer-mappings`))[0];
  const input = {
    source_node: current.source_node,
    source_tenant: current.source_tenant,
    source_subject: current.source_subject,
    credential_id: current.credential_id,
    expected_revision: 1,
    enabled: false,
  };
  await api(`${base}/peer-mappings`, input);
  await dialog.getByRole("button", { name: "Save and enable mapping" }).click();
  await expect(dialog.getByRole("alert")).toBeVisible();
  expect((await api(`${base}/peer-mappings`))[0].enabled).toBe(false);
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await panel.getByRole("button", { name: "Edit peer mapping" }).click();
  await dialog
    .getByLabel("Local credential", { exact: true })
    .selectOption(second.credential.id);
  await dialog.getByRole("button", { name: "Save and enable mapping" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(panel).toContainText(second.credential.id);
  await api(`${base}/credentials/${second.credential.id}/revoke`, {});
  await panel.getByRole("button", { name: "Disable approval" }).click();
  await expect(
    panel.getByRole("button", { name: "Approve", exact: true }),
  ).toBeVisible();
  await panel.getByRole("button", { name: "Approve", exact: true }).click();
  await expect(panel.getByRole("alert")).toBeVisible();
  expect((await api(`${base}/peer-mappings`))[0].enabled).toBe(false);
  const history = await api(`${base}/peer-mapping-history`);
  expect(history.map((value: { revision: number }) => value.revision)).toEqual([
    1, 2, 3, 4,
  ]);
  expect(JSON.stringify(history)).not.toContain(first.token);
  await panel.scrollIntoViewIfNeeded();
  await page.screenshot({
    path: "../.ignore/dashboard-peer-mappings-en.png",
    fullPage: true,
  });
  await page.getByLabel("Language", { exact: true }).selectOption("ja-JP");
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(
    page.getByRole("heading", {
      name: "接続ノードの主体対応付け",
      exact: true,
    }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: "../.ignore/dashboard-peer-mappings-ja.png",
    fullPage: true,
  });
});
