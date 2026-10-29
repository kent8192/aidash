import { expect, test, type Page } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
const id = "019c1234-5678-7123-8123-123456789abc";
const metadata = {
  id,
  tenant: "alpha",
  provider: "openrouter",
  base_url: "https://openrouter.ai/api/v1",
  fingerprint: "aabbccdd11223344",
  last4: "9876",
  state: "active",
  created_at: "2026-10-08T12:00:00Z",
  rotated_at: null,
  revoked_at: null,
  revision: 2,
};
async function setup(
  page: Page,
  options: {
    subject?: boolean;
    locale?: string;
    rejectRevoke?: boolean;
    disabled?: boolean;
    boundOutsidePage?: boolean;
    bound?: boolean;
    keySettingsUrl?: string;
    authorizations?: unknown[];
    path?: string;
    /** The Tenant the callback's authorization was started for. */
    startedTenant?: string;
    /** Holds the credential list until this settles. */
    holdRows?: Promise<void>;
  } = {},
) {
  // Provider Credentials belong to a Tenant: a Mapping by default, since a
  // Deployment Administrator has no Tenant content authority (ADR 0102).
  const subject = options.subject ?? true;
  await installBearerDashboard(
    page,
    "fixture",
    subject ? { tenant: "alpha", name: "alice" } : undefined,
  );
  await page.addInitScript(
    (locale) => localStorage.setItem("aidash-locale", locale),
    options.locale ?? "en-US",
  );
  // The dashboard records the Tenant before it leaves for the consent page.
  if (options.path?.includes("provider_authorization="))
    await page.addInitScript(
      (tenant) =>
        sessionStorage.setItem("aidash-provider-authorization-tenant", tenant),
      options.startedTenant ?? "alpha",
    );
  const access = subject
    ? { kind: "subject", tenant: "alpha", subject: "alice" }
    : { kind: "deployment_administrator" };
  let records: (typeof metadata & { key_settings_url?: string | null })[] = [
    { ...metadata, key_settings_url: options.keySettingsUrl ?? null },
  ];
  let binding: {
    tenant: string;
    provider: string;
    provider_credential_id: string | null;
    revision: number;
  } | null = null;
  if (options.bound)
    binding = {
      tenant: "alpha",
      provider: "openrouter",
      provider_credential_id: id,
      revision: 1,
    };
  if (options.boundOutsidePage) {
    records = [{ ...metadata, id: "019c1234-5678-7123-8123-123456789def" }];
    binding = {
      tenant: "alpha",
      provider: "openrouter",
      provider_credential_id: id,
      revision: 1,
    };
  }
  await page.route("**/api/**", async (route) => {
    const request = route.request(),
      path = new URL(request.url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({ json: { access, node_id: "aidash://test" } });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access,
          deployment: {
            id: "aidash://test",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            key: null,
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
    if (path === "/api/tenants/alpha/provider-credentials") {
      expect(request.method()).toBe("GET");
      if (options.disabled)
        return route.fulfill({
          status: 404,
          json: { error: "Secret Store is not configured" },
        });
      await options.holdRows;
      return route.fulfill({ json: records });
    }
    if (path === "/api/tenants/alpha/provider-authorizations/openrouter") {
      expect(request.method()).toBe("POST");
      options.authorizations?.push(request.postDataJSON());
      return route.fulfill({
        json: {
          authorization_url:
            "https://openrouter.ai/auth?callback_url=fixture&state=fixture",
        },
      });
    }
    if (path.endsWith("/rotate"))
      throw new Error("Public rotation must not be called");
    if (path === `/api/tenants/alpha/provider-credentials/${id}/revoke`) {
      expect(request.method()).toBe("POST");
      expect(request.postDataJSON()).toEqual({
        expected_revision: records[0].revision,
      });
      if (options.rejectRevoke)
        return route.fulfill({
          status: 409,
          json: { error: "stale Provider Credential revision" },
        });
      records = [
        { ...records[0], state: "revoked", revision: records[0].revision + 1 },
      ];
      return route.fulfill({ json: records[0] });
    }
    if (path === "/api/tenants/alpha/provider-credential-bindings")
      return route.fulfill({ json: binding ? [binding] : [] });
    if (path === `/api/tenants/alpha/provider-credentials/${id}`) {
      if (request.method() === "DELETE") {
        expect(request.postDataJSON()).toEqual({
          expected_revision: records[0].revision,
        });
        records = [
          {
            ...records[0],
            state: "deleted",
            revision: records[0].revision + 1,
          },
        ];
        return route.fulfill({ json: records[0] });
      }
      expect(request.method()).toBe("GET");
      return route.fulfill({ json: metadata });
    }
    if (
      path === "/api/tenants/alpha/provider-credential-bindings/openrouter" &&
      request.method() === "PUT"
    ) {
      const body = request.postDataJSON();
      expect(body.expected_revision).toBe(binding?.revision ?? 0);
      binding = {
        tenant: "alpha",
        provider: "openrouter",
        provider_credential_id: body.provider_credential_id,
        revision: (binding?.revision ?? 0) + 1,
      };
      return route.fulfill({ json: binding });
    }
    return route.fulfill({ json: [] });
  });
  // The provider's consent page is outside the dashboard; never contact it.
  await page.route("https://openrouter.ai/**", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: "OpenRouter consent fixture",
    }),
  );
  await page.goto(options.path ?? "/settings?view=providerCredentials");
}
async function cachedBrowserData(page: Page) {
  return page.evaluate(() => {
    const root = document.querySelector("#root") as HTMLElement &
      Record<string, unknown>;
    const container = Object.keys(root).find((key) =>
      key.startsWith("__reactContainer$"),
    );
    if (!container) throw new Error("React root is unavailable");
    const start = root[container] as { stateNode?: { current?: unknown } };
    type Fiber = {
      child?: Fiber;
      sibling?: Fiber;
      memoizedProps?: {
        client?: {
          getQueryCache?: () => { getAll: () => { state: unknown }[] };
          getMutationCache?: () => { getAll: () => { state: unknown }[] };
        };
      };
    };
    const queue = [(start.stateNode?.current ?? start) as Fiber];
    while (queue.length) {
      const fiber = queue.pop()!;
      const client = fiber.memoizedProps?.client;
      if (client?.getQueryCache && client?.getMutationCache)
        return JSON.stringify({
          queries: client
            .getQueryCache()
            .getAll()
            .map((query) => query.state),
          mutations: client
            .getMutationCache()
            .getAll()
            .map((mutation) => mutation.state),
          local: { ...localStorage },
          session: { ...sessionStorage },
        });
      if (fiber.child) queue.push(fiber.child);
      if (fiber.sibling) queue.push(fiber.sibling);
    }
    throw new Error("Query client is unavailable");
  });
}
test("metadata management supports binding and unbinding without key-entry controls", async ({
  page,
}) => {
  await setup(page);
  await expect(
    page.getByText(metadata.fingerprint, { exact: true }),
  ).toBeVisible();
  await expect(page.getByText(metadata.last4, { exact: true })).toBeVisible();
  await expect(
    page.locator('input[type="password"], input[name="key_material"]'),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", {
      name: /Add Provider Credential|Rotate Provider Credential/,
    }),
  ).toHaveCount(0);
  const selector = page.getByRole("combobox", {
    name: "Provider Credential bindings",
    exact: true,
  });
  await selector.selectOption(id);
  await expect(selector).toHaveValue(id);
  await expect(
    page.getByRole("button", { name: "Delete", exact: true }),
  ).toBeDisabled();
  await selector.selectOption("");
  await expect(selector).toHaveValue("");
  const cache = await cachedBrowserData(page);
  expect(cache).toContain(metadata.fingerprint);
  expect(cache).not.toContain("key_material");
  expect(cache).not.toContain("secret_resource");
  if (process.env.AIDASH_PROVIDER_SCREENSHOT)
    await page.screenshot({
      path: process.env.AIDASH_PROVIDER_SCREENSHOT,
      fullPage: true,
    });
});
test("binding selection retains its metadata when the bound record is on another page", async ({
  page,
}) => {
  await setup(page, { subject: true, boundOutsidePage: true });
  const selector = page.getByRole("combobox", {
    name: "Provider Credential bindings",
    exact: true,
  });
  await expect(selector).toHaveValue(id);
  await expect(selector.locator(`option[value="${id}"]`)).toContainText(
    metadata.fingerprint,
  );
  expect(await cachedBrowserData(page)).not.toContain("key_material");
});
test("revoke and delete use metadata revisions without accepting Key Material", async ({
  page,
}) => {
  await setup(page, { subject: true });
  await page.getByRole("button", { name: "Revoke", exact: true }).click();
  await expect(page.getByText("Revoked", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Revoke", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(page.getByText("Deleted", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Delete", exact: true }),
  ).toHaveCount(0);
});
test("stale revocation reports an error and keeps current metadata", async ({
  page,
}) => {
  await setup(page, { subject: true, rejectRevoke: true });
  await page.getByRole("button", { name: "Revoke", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "stale Provider Credential revision",
  );
  await expect(page.getByText("Active", { exact: true })).toBeVisible();
  await expect(page.locator('input[name="key_material"]')).toHaveCount(0);
});
test("missing Store disables management without offering a paste path", async ({
  page,
}) => {
  await setup(page, { subject: true, disabled: true });
  await expect(page.getByRole("status")).toContainText(
    "Secret Store is not configured",
  );
  await expect(
    page.getByRole("combobox", {
      name: "Provider Credential bindings",
      exact: true,
    }),
  ).toHaveCount(0);
  await expect(page.locator('input[name="key_material"]')).toHaveCount(0);
});
test("mapped session opens the Japanese Provider Credentials page without a tenant picker", async ({
  page,
}) => {
  await setup(page, { subject: true, locale: "ja-JP" });
  await expect(
    page.getByRole("heading", { name: "プロバイダ認証情報", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("Fingerprint", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: /追加|ローテーション/ }),
  ).toHaveCount(0);
  await expect(page.locator("input[name=tenant]")).toHaveCount(0);
});
test("connect starts Provider Authorization and leaves for the provider consent page", async ({
  page,
}) => {
  const authorizations: unknown[] = [];
  await setup(page, { subject: true, authorizations });
  await expect(page.getByText("Not connected", { exact: true })).toBeVisible();
  await page
    .getByRole("button", { name: "Connect OpenRouter", exact: true })
    .click();
  await page.waitForURL(/^https:\/\/openrouter\.ai\/auth/);
  expect(authorizations).toEqual([
    {
      provider_credential_id: null,
      expected_revision: null,
      return_to: "/settings?view=providerCredentials",
    },
  ]);
});
test("reconnect rotates the bound Provider Credential and links to its key settings", async ({
  page,
}) => {
  const authorizations: unknown[] = [];
  const keySettingsUrl = "https://openrouter.ai/keys/" + "ab".repeat(32);
  await setup(page, {
    subject: true,
    bound: true,
    keySettingsUrl,
    authorizations,
  });
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("link", { name: "Manage key at OpenRouter" }).first(),
  ).toHaveAttribute("href", keySettingsUrl);
  await page
    .getByRole("button", { name: "Reconnect", exact: true })
    .first()
    .click();
  await page.waitForURL(/^https:\/\/openrouter\.ai\/auth/);
  expect(authorizations).toEqual([
    {
      provider_credential_id: id,
      expected_revision: metadata.revision,
      return_to: "/settings?view=providerCredentials",
    },
  ]);
});
test("the callback outcome is reported once and removed from the address", async ({
  page,
}) => {
  await setup(page, {
    subject: true,
    path: "/settings?view=providerCredentials&provider_authorization=failed",
  });
  await expect(page.getByRole("status")).toContainText(
    "OpenRouter authorization failed. Nothing was stored",
  );
  expect(new URL(page.url()).searchParams.get("provider_authorization")).toBe(
    null,
  );
  await page.reload();
  await expect(
    page.getByRole("button", { name: "Connect OpenRouter", exact: true }),
  ).toBeVisible();
  await expect(
    page
      .getByRole("status")
      .filter({ hasText: "OpenRouter authorization failed" }),
  ).toHaveCount(0);
});
test("a connection left unbound is not reported as connected", async ({
  page,
}) => {
  await setup(page, {
    subject: true,
    path: "/settings?view=providerCredentials&provider_authorization=unbound",
  });
  const notice = page
    .getByRole("status")
    .filter({ hasText: "it is not bound to this Tenant" });
  await expect(notice).toBeVisible();
  await expect(notice).not.toContainText("OpenRouter is connected.");
});
test("a Deployment Administrator sees no Provider Credentials or Tenant picker", async ({
  page,
}) => {
  await setup(page, { subject: false });
  await expect(
    page.getByText(
      "Tenant content is not available to a Deployment Administrator.",
    ),
  ).toBeVisible();
  await expect(page.getByLabel("Tenant", { exact: true })).toHaveCount(0);
});
test("an outcome is never shown for another Tenant", async ({ page }) => {
  await setup(page, {
    subject: true,
    startedTenant: "beta",
    path: "/settings?view=providerCredentials&provider_authorization=failed",
  });
  await expect(
    page.getByRole("button", { name: "Connect OpenRouter" }),
  ).toBeVisible();
  await expect(
    page.getByRole("status").filter({ hasText: "authorization failed" }),
  ).toHaveCount(0);
});
test("connect waits for the bound record before choosing connect or reconnect", async ({
  page,
}) => {
  let release = () => {};
  const holdRows = new Promise<void>((resolve) => (release = resolve));
  await setup(page, { subject: true, bound: true, holdRows });
  // The binding is known, the credential list is not: no action yet.
  await expect(
    page.getByRole("button", { name: "Connect OpenRouter" }),
  ).toBeDisabled();
  release();
  await expect(
    page.getByRole("button", { name: "Reconnect", exact: true }).first(),
  ).toBeEnabled();
  await expect(
    page.getByRole("button", { name: "Connect OpenRouter" }),
  ).toHaveCount(0);
});
test("revocation asks the user to delete the key at OpenRouter", async ({
  page,
}) => {
  const keySettingsUrl = "https://openrouter.ai/keys/" + "cd".repeat(32);
  await setup(page, { subject: true, keySettingsUrl });
  await page.getByRole("button", { name: "Revoke", exact: true }).click();
  const notice = page.getByRole("status");
  await expect(notice).toContainText("The key stays valid at OpenRouter");
  await expect(
    notice.getByRole("link", { name: "Manage key at OpenRouter" }),
  ).toHaveAttribute("href", keySettingsUrl);
});
test("deletion keeps the link to delete the key at OpenRouter", async ({
  page,
}) => {
  const keySettingsUrl = "https://openrouter.ai/keys/" + "ef".repeat(32);
  await setup(page, { subject: true, keySettingsUrl });
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  const notice = page.getByRole("status");
  await expect(notice).toContainText("The key stays valid at OpenRouter");
  await expect(
    notice.getByRole("link", { name: "Manage key at OpenRouter" }),
  ).toHaveAttribute("href", keySettingsUrl);
  // The deleted tombstone still links to the provider's settings page.
  await expect(page.getByText("Deleted", { exact: true })).toBeVisible();
  await expect(
    page
      .getByRole("row")
      .filter({ hasText: "Deleted" })
      .getByRole("link", { name: "Manage key at OpenRouter" }),
  ).toHaveAttribute("href", keySettingsUrl);
});
test("a failure after OpenRouter issued a key links to that key", async ({
  page,
}) => {
  const keySettingsUrl = "https://openrouter.ai/keys/" + "01".repeat(32);
  await setup(page, {
    subject: true,
    path:
      "/settings?view=providerCredentials&provider_authorization=failed&provider_key_settings_url=" +
      encodeURIComponent(keySettingsUrl),
  });
  const notice = page
    .getByRole("status")
    .filter({ hasText: "OpenRouter issued a key" });
  await expect(
    notice.getByRole("link", { name: "Manage key at OpenRouter" }),
  ).toHaveAttribute("href", keySettingsUrl);
  const search = new URL(page.url()).searchParams;
  expect(search.get("provider_authorization")).toBe(null);
  expect(search.get("provider_key_settings_url")).toBe(null);
});
test("a reconnect links to the replaced key and reports key warnings", async ({
  page,
}) => {
  const previous = "https://openrouter.ai/keys/" + "02".repeat(32);
  await setup(page, {
    subject: true,
    path:
      "/settings?view=providerCredentials&provider_authorization=reconnected&provider_key_settings_url=" +
      encodeURIComponent(previous) +
      "&provider_key_warnings=1",
  });
  const notice = page
    .getByRole("status")
    .filter({ hasText: "The previous key stays valid at OpenRouter" });
  await expect(
    notice.getByRole("link", { name: "Manage key at OpenRouter" }),
  ).toHaveAttribute("href", previous);
  await expect(notice).toContainText("missing spending limit");
  const search = new URL(page.url()).searchParams;
  expect(search.get("provider_key_settings_url")).toBe(null);
  expect(search.get("provider_key_warnings")).toBe(null);
});
test("a failure never links outside OpenRouter's key settings", async ({
  page,
}) => {
  await setup(page, {
    subject: true,
    path:
      "/settings?view=providerCredentials&provider_authorization=failed&provider_key_settings_url=" +
      encodeURIComponent("https://attacker.example/keys/" + "01".repeat(32)),
  });
  const notice = page
    .getByRole("status")
    .filter({ hasText: "OpenRouter authorization failed" });
  await expect(notice).toBeVisible();
  await expect(notice.getByRole("link")).toHaveCount(0);
});
