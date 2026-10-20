import { test, expect, type Page } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import { routeTenantActions } from "./registry-fixture";

async function approvingTenant(page: Page, locale: "en-US" | "ja-JP") {
  await installBearerDashboard(page, "fixture");
  await page.addInitScript(
    (value) => localStorage.setItem("aidash-locale", value),
    locale,
  );
  const fences = { catalog: 7, activation: 3, denied: false };
  const row = (id: string) => ({
    installation: {
      id,
      tenant: "a",
      package_key: `package-${id}`,
      latest_revision: 2,
      active_revision: null,
      activation_revision: fences.activation,
    },
    revision: 2,
    digest: `digest-${id}`,
    entry: {
      id: `mkt-${id}`,
      version: "1.0.0",
      kind: id === "bundle" ? "bundle" : "tool",
      name: { en: id },
      description: { en: "Installed copy" },
      config:
        id === "bundle"
          ? { members: [{ id: "mkt-task", version: "1.0.0" }] }
          : { transport: "native", operation: "echo" },
      schema: {},
      tags: [],
      skills: [],
      languages: [],
      capabilities: [],
    },
    config: {},
    dependencies: [],
    bindings: [],
    approved: false,
    actions: [],
  });
  const decisions: { path: string; body: unknown }[] = [];
  const access = { kind: "subject", tenant: "a", subject: "alice" };
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({ json: { access, node_id: "aidash://node" } });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access,
          node: {
            id: "aidash://node",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            capabilities: [],
            clusters: [],
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
    if (path === "/api/marketplace/installations")
      return route.fulfill({ json: [row("bundle"), row("task")] });
    if (path === "/api/authorization/a/catalog")
      return fences.denied
        ? route.fulfill({ status: 403, json: { error: "forbidden" } })
        : route.fulfill({
            json: ["bundle", "task"].map((id) => ({
              entry_id: `mkt-${id}`,
              entry_version: "1.0.0",
              revision: fences.catalog,
              enabled: false,
              tenant: "a",
            })),
          });
    if (
      route.request().method() === "POST" &&
      path.startsWith("/api/marketplace/")
    ) {
      decisions.push({ path, body: route.request().postDataJSON() });
      if (path === "/api/marketplace/approval-sets") {
        fences.catalog = 8;
        fences.activation = 4;
        return route.fulfill({
          status: 409,
          json: { error: "stale selection" },
        });
      }
      return route.fulfill({ json: row("task").installation });
    }
    return route.fulfill({ json: [] });
  });
  return { fences, decisions };
}

for (const locale of ["en-US", "ja-JP"] as const) {
  test(`Catalog Approval reviews an exact pending Marketplace set (${locale})`, async ({
    page,
  }) => {
    const ja = locale === "ja-JP";
    const { fences, decisions } = await approvingTenant(page, locale);
    await routeTenantActions(page, ["catalog.approve"], { tenant: "a" });
    await page.goto("/settings?view=marketplace");
    const review = page.locator("section.panel").filter({
      has: page.getByRole("heading", {
        name: ja ? "まとめて承認" : "Approve a reviewed set",
        exact: true,
      }),
    });
    await expect(
      review.getByLabel("mkt-task@1.0.0", { exact: true }),
    ).toBeEnabled();
    await review.getByLabel("mkt-bundle@1.0.0", { exact: true }).check();
    await review.getByLabel("mkt-task@1.0.0", { exact: true }).check();
    const approve = review.getByRole("button", {
      name: ja
        ? "選択した対象を承認して有効化"
        : "Approve and activate selected set",
      exact: true,
    });
    await expect(review.locator(".approval-set-selection")).toContainText(
      '"id": "mkt-task"',
    );
    await approve.click();
    await expect.poll(() => decisions.length).toBe(1);
    expect(decisions[0]).toEqual({
      path: "/api/marketplace/approval-sets",
      body: {
        tenant: "a",
        installations: ["bundle", "task"].map((id) => ({
          installation: id,
          revision: 2,
          digest: `digest-${id}`,
          expected_activation_revision: 3,
        })),
        approvals: ["bundle", "task"].map((id) => ({
          reference: { id: `mkt-${id}`, version: "1.0.0" },
          expected_catalog_revision: 7,
        })),
      },
    });
    await expect(
      review.getByRole("status").filter({
        hasText: ja ? "承認対象が変更" : "reviewed selection changed",
      }),
    ).toBeVisible();
    // A repeated action cannot silently substitute refreshed approval fences.
    await approve.click();
    await expect.poll(() => decisions.length).toBe(2);
    expect(decisions[1]).toEqual(decisions[0]);
    const reload = review.getByRole("button", {
      name: ja ? "承認対象を再読み込み" : "Reload approval selection",
      exact: true,
    });
    await reload.click();
    await expect(approve).toBeDisabled();
    await review.getByLabel("mkt-task@1.0.0", { exact: true }).check();
    await approve.click();
    await expect.poll(() => decisions.length).toBe(3);
    expect(decisions[2]).toMatchObject({
      body: {
        installations: [
          { installation: "task", expected_activation_revision: 4 },
        ],
        approvals: [{ expected_catalog_revision: 8 }],
      },
    });
    fences.denied = true;
    await reload.click();
    await expect(review.getByRole("alert")).toBeVisible();
    await expect(
      review.getByText("mkt-task@1.0.0", { exact: true }),
    ).toHaveCount(0);
  });
}

test("Catalog Approval activates one installation with the current approval fence", async ({
  page,
}) => {
  const { decisions } = await approvingTenant(page, "en-US");
  await routeTenantActions(page, ["catalog.approve"], { tenant: "a" });
  await page.goto("/settings?view=marketplace");
  const task = page
    .locator("li")
    .filter({ has: page.getByRole("heading", { name: "task", exact: true }) });
  await task
    .getByRole("button", { name: "Approve and activate", exact: true })
    .click();
  await expect.poll(() => decisions.length).toBe(1);
  expect(decisions[0]).toEqual({
    path: "/api/marketplace/installations/task/activation",
    body: {
      tenant: "a",
      revision: 2,
      expected_activation_revision: 3,
      expected_catalog_revision: 7,
      enabled: true,
    },
  });
  await expect(
    task.getByRole("button", { name: "Revoke this revision", exact: true }),
  ).toHaveCount(0);
});

test("a member without Catalog Approval sees installations without approval controls", async ({
  page,
}) => {
  await approvingTenant(page, "en-US");
  await routeTenantActions(page, ["marketplace.install"], { tenant: "a" });
  await page.goto("/settings?view=marketplace");
  await expect(
    page.getByRole("heading", { name: "task", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /Approve and activate/ }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Approve a reviewed set", exact: true }),
  ).toHaveCount(0);
});

for (const locale of ["en-US", "ja-JP"] as const) {
  test(`scoped Marketplace stages and publishes exact definitions (${locale})`, async ({
    page,
  }, testInfo) => {
    await installBearerDashboard(page, "fixture", {
      tenant: "a",
      name: "alice",
    });
    await page.route("**/auth/session", (route) =>
      route.fulfill({
        json: {
          id: "fixture-browser-session",
          deployment_administrator: false,
          mappings: [
            { id: "fixture-mapping", tenant: "a", subject: "alice" },
            { id: "second-mapping", tenant: "b", subject: "bob" },
          ],
        },
      }),
    );
    await page.addInitScript(
      (locale) => localStorage.setItem("aidash-locale", locale),
      locale,
    );
    const entry = {
      id: "source",
      version: "1.0.0",
      kind: "tool",
      name: { en: "Review tool", ja: "レビューツール" },
      description: { en: "Package summary", ja: "パッケージの説明" },
      capabilities: [],
      tags: [],
      languages: ["en", "ja"],
      skills: [],
      schema: {},
      config: { transport: "native", operation: "echo" },
    };
    const summary = {
      key: "package-a",
      repository: "aidash://node",
      owner_tenant: "a",
      package_id: "tool",
      version: "1.0.0",
      kind: "tool",
      name: entry.name,
      description: entry.description,
      author: "Author",
      capabilities: [],
      permissions: [],
      languages: ["en", "ja"],
      digest: "sha256:source",
      actions: ["read", "install", "share"],
    };
    const installation = {
      installation: {
        id: "install-a",
        tenant: "a",
        package_key: "package-a",
        latest_revision: 1,
        active_revision: null as number | null,
        activation_revision: 0,
      },
      revision: 1,
      entry: { ...entry, id: "mkt-projection" },
      digest: "effective",
      config: {},
      dependencies: [],
      bindings: [],
      approved: false,
      actions: ["configure"],
    };
    let installed = false;
    let denied = false;
    let published: Record<string, unknown> | undefined;
    let shared: Record<string, unknown> | undefined;
    await page.route("**/api/**", async (route) => {
      const url = new URL(route.request().url());
      const path = url.pathname;
      const secondContext =
        route.request().headers()["x-aidash-context"] ===
        "mapping:b:second-mapping";
      const access = {
        kind: "subject",
        tenant: secondContext ? "b" : "a",
        subject: secondContext ? "bob" : "alice",
      };
      const body =
        route.request().method() === "GET"
          ? null
          : route.request().postDataJSON();
      if (path === "/api/events/stream") return route.abort();
      if (path === "/api/session")
        return route.fulfill({
          json: {
            access,
            node_id: "aidash://node",
          },
        });
      if (path === "/api/state")
        return route.fulfill({
          json: {
            access,
            node: {
              id: "aidash://node",
              endpoint: "http://localhost",
              protocol_version: "0.1",
              capabilities: [],
              clusters: [],
            },
            registry: [entry],
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
      if (secondContext && path.startsWith("/api/marketplace/"))
        return route.fulfill({ json: [] });
      if (denied && path.startsWith("/api/marketplace/"))
        return route.fulfill({ status: 403, json: { error: "forbidden" } });
      if (path === "/api/marketplace/packages" && body) {
        published = body;
        // Revoke before the successful mutation invalidates its queries. The
        // resulting refetch must clear protected content without another action.
        denied = true;
        return route.fulfill({
          json: { key: summary.key, digest: summary.digest },
        });
      }
      if (path === "/api/marketplace/packages")
        return route.fulfill({ json: [summary] });
      if (path === "/api/marketplace/packages/package-a/audience") {
        shared = body;
        return route.fulfill({ json: { revision: 2, tenants: ["a"] } });
      }
      if (path === "/api/marketplace/packages/package-a")
        return route.fulfill({
          json: {
            summary,
            manifest: {
              entity: entry,
              author: "Author",
              permissions: [],
              dependencies: [],
            },
            audience: { revision: 1, tenants: ["a"] },
          },
        });
      if (path === "/api/marketplace/packages/package-a/install") {
        expect(body.digest).toBe(summary.digest);
        installed = true;
        return route.fulfill({ json: installation });
      }
      if (path === "/api/marketplace/installations")
        return route.fulfill({ json: installed ? [installation] : [] });
      if (path === "/api/marketplace/publication-access")
        return route.fulfill({ json: { allowed: true, version: "1.0.0" } });
      if (path === "/api/marketplace/sources") {
        if (url.searchParams.get("offset") === "50")
          return route.fulfill({
            json: [
              {
                ...entry,
                id: "later-source",
                name: { en: "Later source", ja: "後の定義" },
              },
            ],
          });
        return route.fulfill({
          json: [entry],
          headers: { "x-aidash-next-offset": "50" },
        });
      }
      if (path === "/api/marketplace/installations/install-a" && !body)
        return route.fulfill({ json: installation });
      if (path === "/api/marketplace/installations/install-a" && body) {
        expect(body.expected_revision).toBe(1);
        return route.fulfill({
          status: 409,
          json: { error: "revision changed" },
        });
      }
      return route.fulfill({ json: [] });
    });
    await page.goto("/settings?view=marketplace");
    const ja = locale === "ja-JP";
    await expect(
      page.getByRole("heading", {
        name: ja ? "パッケージ" : "Packages",
        exact: true,
      }),
    ).toBeVisible();
    await page
      .getByRole("button", { name: ja ? "詳細" : "Details", exact: true })
      .click();
    await page
      .getByLabel(
        ja ? "共有先テナント（1行に1件）" : "Recipient tenants (one per line)",
      )
      .fill("");
    await page
      .getByRole("button", {
        name: ja ? "共有範囲を更新" : "Update sharing",
        exact: true,
      })
      .click();
    await expect.poll(() => shared).toMatchObject({ tenants: ["a"] });
    await page
      .getByRole("button", {
        name: ja ? "確認して導入" : "Review and install",
        exact: true,
      })
      .click();
    const review = page.getByRole("dialog", {
      name: ja ? "このテナントに導入" : "Install for this tenant",
    });
    await expect(review).toContainText("sha256:source");
    await review
      .getByRole("button", {
        name: ja ? "このテナントに導入" : "Install for this tenant",
      })
      .click();
    await expect(review).toHaveCount(0);
    await expect(
      page.getByText(
        ja ? "導入済み・利用承認待ち" : "Installed; awaiting approval",
        { exact: true },
      ),
    ).toBeVisible();
    await page
      .locator(".panel")
      .filter({
        has: page.getByRole("heading", {
          name: ja ? "テナントの導入一覧" : "Tenant installations",
          exact: true,
        }),
      })
      .screenshot({ path: testInfo.outputPath(`marketplace-${locale}.png`) });
    await page
      .getByRole("button", {
        name: ja ? "設定変更を申請" : "Stage configuration",
        exact: true,
      })
      .click();
    await page
      .getByLabel(ja ? "ローカル設定（JSON）" : "Local configuration (JSON)")
      .fill('{"operation":"echo"}');
    await page
      .getByRole("button", {
        name: ja ? "設定変更を申請" : "Stage configuration",
        exact: true,
      })
      .last()
      .click();
    await expect(page.getByRole("status")).toHaveText(
      ja
        ? "Revisionが更新されました。再読み込み後に変更してください。"
        : "The revision changed. Reload before applying another change.",
    );
    await page
      .getByRole("button", {
        name: ja ? "次のページ" : "Next page",
        exact: true,
      })
      .click();
    await expect(
      page.getByLabel(ja ? "登録済み定義" : "Registered source", {
        exact: true,
      }),
    ).toContainText(ja ? "後の定義" : "Later source");
    await page
      .getByRole("button", {
        name: ja ? "前のページ" : "Previous page",
        exact: true,
      })
      .click();
    await page
      .getByLabel(ja ? "登録済み定義" : "Registered source", { exact: true })
      .selectOption("source@1.0.0");
    await page
      .getByLabel(ja ? "表示用の作者名" : "Display author")
      .fill("Alice");
    await expect(
      page.getByText(
        `${ja ? "公開するバージョン" : "Publication version"}: 1.0.0`,
      ),
    ).toBeVisible();
    await page
      .getByRole("button", {
        name: ja ? "登録済み定義を公開" : "Publish registered source",
        exact: true,
      })
      .click();
    await expect
      .poll(() => published)
      .toMatchObject({
        source: { id: "source", version: "1.0.0" },
        package_id: "source",
        author: "Alice",
      });
    expect(published).not.toHaveProperty("manifest");
    await expect(page.getByRole("alert")).toHaveText(
      ja
        ? "現在の権限ではこの項目を利用できません"
        : "This item is unavailable with your current access",
    );
    await expect(
      page.getByRole("heading", {
        name: ja ? "レビューツール" : "Review tool",
        exact: true,
      }),
    ).toHaveCount(0);
    await page
      .getByRole("button", {
        name: ja ? "アカウント設定" : "Account settings",
        exact: true,
      })
      .click();
    await page
      .getByRole("combobox", {
        name: ja
          ? "このタブで使う権限を選択してください"
          : "Choose the authority for this tab",
        exact: true,
      })
      .selectOption("mapping:b:second-mapping");
    await expect(
      page.getByText(
        ja
          ? "この検索で表示できるパッケージはありません"
          : "No packages available for this search",
        { exact: true },
      ),
    ).toBeVisible();
    await expect(
      page.getByText(
        ja ? "導入済み・利用承認待ち" : "Installed; awaiting approval",
        { exact: true },
      ),
    ).toHaveCount(0);
    await expect(
      page.getByLabel(
        ja ? "ローカル設定（JSON）" : "Local configuration (JSON)",
      ),
    ).toHaveCount(0);
  });
}

for (const locale of ["en-US", "ja-JP"] as const) {
  test(`Deployment Administrator Marketplace lists retained revisions read-only and clears revoked results (${locale})`, async ({
    page,
  }) => {
    await installBearerDashboard(page, "fixture");
    await page.addInitScript(
      (locale) => localStorage.setItem("aidash-locale", locale),
      locale,
    );
    let denied = false;
    await page.route("**/api/**", async (route) => {
      const url = new URL(route.request().url());
      if (url.pathname === "/api/events/stream") return route.abort();
      if (url.pathname === "/api/session")
        return route.fulfill({
          json: {
            access: { kind: "deployment_administrator" },
            node_id: "aidash://node",
          },
        });
      if (url.pathname === "/api/state")
        return route.fulfill({
          json: {
            access: { kind: "deployment_administrator" },
            node: {
              id: "aidash://node",
              endpoint: "http://localhost",
              protocol_version: "0.1",
              capabilities: [],
              clusters: [],
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
      if (url.pathname === "/api/marketplace/administration") {
        if (denied)
          return route.fulfill({ status: 403, json: { error: "forbidden" } });
        const second = url.searchParams.get("offset") === "50";
        return route.fulfill({
          headers: second ? {} : { "x-aidash-next-offset": "50" },
          json: [
            {
              installation: {
                id: "installation",
                tenant: "a",
                package_key: "package",
                latest_revision: 2,
                active_revision: null,
                activation_revision: 0,
              },
              revision: second ? 2 : 1,
              entry: {
                id: "projected",
                version: "1.0.0",
                kind: "tool",
                name: {
                  en: second ? "Second revision" : "First revision",
                  ja: second ? "後のrevision" : "最初のrevision",
                },
                description: { en: "fixture" },
                config: { endpoint: "private-configuration" },
              },
              digest: "sha256:fixture",
              config: {},
              dependencies: [],
              bindings: [],
              approved: false,
              actions: [],
            },
          ],
        });
      }
      if (url.pathname === "/api/info")
        return route.fulfill({
          json: {
            access: { kind: "deployment_administrator" },
            node_id: "aidash://node",
          },
        });
      return route.fulfill({ json: [] });
    });
    await page.goto("/settings?view=marketplace");
    const ja = locale === "ja-JP";
    const panel = page.locator(".panel").filter({
      has: page.getByRole("heading", {
        name: ja ? "テナント別の導入一覧" : "Installations by Tenant",
        exact: true,
      }),
    });
    await panel
      .getByLabel(ja ? "テナント" : "Tenant", { exact: true })
      .fill("a");
    await panel
      .getByRole("button", { name: ja ? "読み込み" : "Load", exact: true })
      .click();
    await expect(
      panel.getByRole("heading", {
        name: ja ? /最初のrevision/ : /First revision/,
      }),
    ).toBeVisible();
    await expect(
      panel.getByRole("button", {
        name: ja ? "承認して有効化" : "Approve and activate",
      }),
    ).toHaveCount(0);
    await panel
      .getByRole("button", {
        name: ja ? "次のページ" : "Next page",
        exact: true,
      })
      .click();
    await expect(
      panel.getByRole("heading", {
        name: ja ? /後のrevision/ : /Second revision/,
      }),
    ).toBeVisible();
    denied = true;
    await panel
      .getByRole("button", { name: ja ? "読み込み" : "Load", exact: true })
      .click();
    await expect(panel.getByRole("alert")).toHaveText(
      ja
        ? "現在の権限ではこの項目を利用できません"
        : "This item is unavailable with your current access",
    );
    await expect(
      panel.getByRole("heading", {
        name: /First revision|Second revision|最初のrevision|後のrevision/,
      }),
    ).toHaveCount(0);
  });
}
