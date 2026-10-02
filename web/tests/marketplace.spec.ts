import { test, expect } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";

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
          operator: false,
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
        "mapping:second-mapping";
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
            peers: [],
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
        name: ja ? "このテナントに導入" : "Install for this tenant",
      })
      .click();
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
    await page.locator(".account-popover summary").click();
    await page
      .locator(".account-popover select")
      .first()
      .selectOption("mapping:second-mapping");
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
  test(`operator Marketplace pages retained revisions and clears revoked results (${locale})`, async ({
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
          json: { access: { kind: "operator" }, node_id: "aidash://node" },
        });
      if (url.pathname === "/api/mesh")
        return route.fulfill({ json: { nodes: [], errors: [] } });
      if (url.pathname === "/api/state")
        return route.fulfill({
          json: {
            access: { kind: "operator" },
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
            peers: [],
          },
        });
      if (url.pathname === "/api/marketplace/compatibility")
        return route.fulfill({ json: { enabled: true, revision: 2 } });
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
          json: { access: { kind: "operator" }, node_id: "aidash://node" },
        });
      return route.fulfill({ json: [] });
    });
    await page.goto("/settings?view=marketplace");
    const ja = locale === "ja-JP";
    const panel = page.locator(".panel").filter({
      has: page.getByRole("heading", {
        name: ja ? "テナント導入の承認" : "Tenant installation approval",
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
