import { test, expect } from "@playwright/test";
import { createTenant, issueSubjectCredential } from "./tenant-fixture";
import {
  installBearerDashboard,
  selectDashboardLanguage,
} from "./auth-fixture";
import { randomUUID } from "node:crypto";

test("the Deployment Administrator replaces whole policies and holds no credential, simulation or decision authority", async ({
  page,
  request,
  browser,
}) => {
  test.setTimeout(90000);
  const tenant = await createTenant(
    request,
    "acceptance-access-token",
    `policy-${randomUUID()}`,
  );
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
  await installBearerDashboard(page, "acceptance-access-token");
  await page.addInitScript(() => {
    localStorage.setItem("aidash-locale", "ja-JP");
  });
  await page.goto("/authorization");
  await expect(page).toHaveURL(/\/trust\?view=authorization$/);
  await expect(
    page.getByRole("heading", { name: "Trust Workbench", exact: true }),
  ).toBeVisible();
  await page.getByLabel("テナント", { exact: true }).fill(tenant);
  await page.getByRole("button", { name: "開く", exact: true }).click();
  await page
    .getByRole("button", { name: "権限ポリシーを編集", exact: true })
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
  // A Deployment Administrator holds no Tenant authority beyond the policy as
  // a whole (#484, ADR 0102): the page offers no simulation, decision history
  // or credential controls, and the server refuses those routes.
  await expect(
    page.getByRole("heading", { name: "権限判定を確認", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "権限判定の監査履歴", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "認証情報を発行", exact: true }),
  ).toHaveCount(0);
  for (const path of [
    `${base}/decisions`,
    `${base}/revisions`,
    `${base}/credentials`,
    `${base}/peer-mappings`,
  ])
    expect((await request.get(path, { headers })).status(), path).toBe(403);
  for (const path of [`${base}/simulate`, `${base}/credentials`])
    expect(
      (
        await request.post(path, {
          headers,
          data: { subject: "alice", action: "workspace.read" },
        })
      ).status(),
      path,
    ).toBe(403);

  // A concurrent Deployment Administrator update must not overwrite the editor's pinned revision.
  await page
    .getByRole("button", { name: "権限ポリシーを編集", exact: true })
    .click();
  await api(base, { expected_revision: 2, bundle: policy });
  await dialog
    .getByRole("button", { name: "ポリシーを確認", exact: true })
    .click();
  await dialog
    .getByRole("button", { name: "ポリシーを適用", exact: true })
    .click();
  await expect(dialog.getByRole("alert")).toContainText(
    "リビジョンが更新されています",
  );
  expect((await api(base)).revision).toBe(3);
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
  expect((await api(base)).revision).toBe(4);

  // The host mints alice's first credential (`manage issue-subject-credential`).
  const token = issueSubjectCredential(tenant, "alice", 120).token;
  expect(token).toBeTruthy();
  expect(
    (
      await request.get("/api/session", {
        headers: { authorization: `Bearer ${token}` },
      })
    ).status(),
  ).toBe(200);
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
    await installBearerDashboard(subjectPage, token, { tenant, name: "alice" });
    await subjectPage.addInitScript(() => {
      localStorage.setItem("aidash-locale", "en-US");
    });
    await subjectPage.goto("/authorization");
    // A member who administers nothing may only leave the Tenant (#239).
    await expect(
      subjectPage.getByRole("button", { name: "Leave Tenant", exact: true }),
    ).toBeVisible();
    await expect(
      subjectPage.getByRole("button", {
        name: "Edit access policy",
        exact: true,
      }),
    ).toHaveCount(0);
    await expect(
      subjectPage.locator(
        '.collab-settings-select option[value="authorization"]',
      ),
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
  // Policy evaluation stays a Deployment-wide administered route.
  const evaluated = await api(`${base}/evaluate`, {
    subject: "alice",
    action: "workspace.read",
    resource: {
      tenant,
      kind: "workspace",
      id: "audit-0",
      attributes: { team: "research" },
    },
  });
  expect(evaluated.allowed).toBe(false);
  await selectDashboardLanguage(page, "en-US");
  await expect(
    page.getByRole("heading", { name: "Policy bundle", exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: "../private/dashboard-authorization-desktop.png",
    fullPage: true,
  });
  await selectDashboardLanguage(page, "ja-JP");
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({
    path: "../private/dashboard-authorization-mobile.png",
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
    page.getByText("この名前のテナントはありません。", { exact: false }),
  ).toBeVisible();
  await expect(page.locator(".auth-decision")).toHaveCount(0);
  await expect(page.locator(".auth-history")).toHaveCount(0);
  expect(errors).toEqual([]);
});

// Peer Mappings and Node Registrations belong to a Tenant Administrator, which
// only a browser Mapping can be (#484, ADR 0102). The Deployment Administrator
// neither sees nor reaches them; the Mapping flow with its revisions and
// bilingual copy is covered by tenant-administration.spec.ts.
test(
  "the Deployment Administrator is offered no peer mappings or node registrations",
  { tag: "@golden-path" },
  async ({ page, request }) => {
    const tenant = await createTenant(
      request,
      "acceptance-access-token",
      `peer-${randomUUID()}`,
    );
    const base = `/api/authorization/${tenant}`;
    const headers = { authorization: "Bearer acceptance-access-token" };
    const refused = async (method: "get" | "post", path: string) =>
      (
        await request[method](path, {
          headers,
          ...(method === "post" ? { data: {} } : {}),
        })
      ).status();
    expect(await refused("get", `${base}/peer-mappings`)).toBe(403);
    expect(await refused("post", `${base}/peer-mappings`)).toBe(403);
    expect(await refused("get", `${base}/peer-mapping-history`)).toBe(403);
    expect(await refused("get", `/api/tenants/${tenant}/nodes`)).toBe(403);
    await installBearerDashboard(page, "acceptance-access-token");
    await page.addInitScript(() => {
      localStorage.setItem("aidash-locale", "en-US");
    });
    await page.goto("/authorization");
    await expect(page).toHaveURL(/\/trust\?view=authorization$/);
    await page.getByLabel("Tenant", { exact: true }).fill(tenant);
    await page.getByRole("button", { name: "Open", exact: true }).click();
    await expect(
      page.getByRole("button", { name: "Edit access policy", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("heading", {
        name: "Peer identity mappings",
        exact: true,
      }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: "Add peer mapping" }),
    ).toHaveCount(0);
    await selectDashboardLanguage(page, "ja-JP");
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(
      page.getByRole("heading", {
        name: "接続ノードの主体対応付け",
        exact: true,
      }),
    ).toHaveCount(0);
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
  },
);
