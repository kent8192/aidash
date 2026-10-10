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
  } = {},
) {
  await installBearerDashboard(
    page,
    "fixture",
    options.subject ? { tenant: "alpha", name: "alice" } : undefined,
  );
  await page.addInitScript(
    (locale) => localStorage.setItem("aidash-locale", locale),
    options.locale ?? "en-US",
  );
  const access = options.subject
    ? { kind: "subject", tenant: "alpha", subject: "alice" }
    : { kind: "operator" };
  let records: (typeof metadata)[] = [metadata];
  let binding: {
    tenant: string;
    provider: string;
    provider_credential_id: string | null;
    revision: number;
  } | null = null;
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
          node: {
            id: "aidash://test",
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
    if (path === "/api/mesh")
      return route.fulfill({ json: { nodes: [], errors: [] } });
    if (path === "/api/tenants/alpha/provider-credentials") {
      expect(request.method()).toBe("GET");
      if (options.disabled)
        return route.fulfill({
          status: 404,
          json: { error: "Provider Credential Store is not configured" },
        });
      return route.fulfill({ json: records });
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
  await page.goto("/settings?view=providerCredentials");
  if (!options.subject) {
    await page.getByLabel("Tenant", { exact: true }).fill("alpha");
    await page.getByRole("button", { name: "Open", exact: true }).click();
  }
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
    "Provider Credential Store is not configured",
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
