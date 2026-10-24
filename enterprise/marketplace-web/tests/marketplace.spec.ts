import { expect, test, type Page } from "@playwright/test";

// The HTTP boundary is the frontend's contract owner. These tests deliberately
// do not claim to exercise backend authentication or entitlement enforcement.
const plugin = {
  id: "plugin-1",
  slug: "source-reader",
  name: "Source Reader",
  description:
    "Read source material with explicit access to selected repositories.",
  publisher_id: "publisher-1",
  publisher_name: "Research Tools",
  license: "Apache-2.0",
  origin: "https://example.org/source-reader",
  price_cents: 0,
  currency: "USD",
  declared_permissions: ["network:example.org", "repository:read"],
  latest_version: "1.0.0",
};
const version = {
  version: "1.0.0",
  digest: "a".repeat(64),
  size_bytes: 1024,
  recalled: false,
  recall_reason: null,
};

async function api(page: Page, authenticated = false) {
  await page.route("**/api/marketplace/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const body = path.endsWith("/session")
      ? {
          authenticated,
          ...(authenticated
            ? { account_id: "account-1", csrf_token: "csrf-proof" }
            : {}),
        }
      : path.endsWith("/catalog")
        ? { plugins: [plugin] }
        : path.endsWith("/catalog/source-reader")
          ? { plugin, versions: [version] }
          : path.endsWith("/library")
            ? {
                entries: [
                  { ...plugin, plugin_id: plugin.id, versions: [version] },
                ],
              }
            : path.endsWith("/publishers")
              ? { publishers: [{ id: "publisher-1", name: "Research Tools" }] }
              : {};
    await route.fulfill({ json: body });
  });
}

test("anonymous visitors can inspect provenance and declared permissions without signing in", async ({
  page,
}) => {
  await api(page);
  await page.goto("/");
  await page.getByRole("link", { name: "Source Reader", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Source Reader", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("Research Tools", { exact: true })).toBeVisible();
  await expect(page.getByText("Apache-2.0", { exact: true })).toBeVisible();
  await expect(
    page.getByText("repository:read", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("a".repeat(64), { exact: true })).toBeVisible();
  await expect(
    page.getByRole("link", { name: "Sign in to add to Library" }),
  ).toHaveAttribute("href", /^\/auth\/login/);
});

test("a free acquisition uses the session CSRF proof before opening the Library", async ({
  page,
}) => {
  await api(page, true);
  let submitted = false;
  await page.route(
    "**/api/marketplace/plugins/plugin-1/acquire",
    async (route) => {
      expect(route.request().method()).toBe("POST");
      expect(route.request().headers()["x-marketplace-csrf"]).toBe(
        "csrf-proof",
      );
      submitted = true;
      await route.fulfill({ json: { acquired: true } });
    },
  );
  await page.goto("/#/plugins/source-reader");
  await page.getByRole("button", { name: "Add to Library" }).click();
  await expect(
    page.getByRole("heading", { name: "Your Library" }),
  ).toBeVisible();
  expect(submitted).toBe(true);
  await expect(page.getByText("1.0.0", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("link", { name: "Download version" }),
  ).toHaveAttribute(
    "href",
    "/api/marketplace/plugins/plugin-1/versions/1.0.0/download",
  );
});

test("authentication outages cannot render a protected Library from stale UI state", async ({
  page,
}) => {
  await api(page, true);
  await page.goto("/#/library");
  await expect(
    page.getByRole("heading", { name: "Your Library" }),
  ).toBeVisible();
  await page.route("**/api/marketplace/library", (route) =>
    route.fulfill({
      status: 503,
      json: { error: "Authentication service unavailable" },
    }),
  );
  await page.getByRole("link", { name: "Explore", exact: true }).click();
  await page.getByRole("link", { name: "Library", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Authentication service unavailable",
  );
  await expect(
    page.getByRole("link", { name: "Source Reader", exact: true }),
  ).toHaveCount(0);
});

test("publisher registration sends an authenticated write and shows the server result", async ({
  page,
}) => {
  await api(page, true);
  await page.route("**/api/marketplace/publishers", async (route) => {
    if (route.request().method() === "POST") {
      expect(route.request().postDataJSON()).toEqual({
        name: "Independent Tools",
      });
      expect(route.request().headers()["x-marketplace-csrf"]).toBe(
        "csrf-proof",
      );
      await route.fulfill({
        json: { publisher: { id: "publisher-2", name: "Independent Tools" } },
      });
    } else {
      await route.fulfill({ json: { publishers: [] } });
    }
  });
  await page.goto("/#/publishers");
  await page.getByLabel("Publisher name").fill("Independent Tools");
  await page.getByRole("button", { name: "Create Publisher" }).click();
  await expect(
    page.getByText("Independent Tools", { exact: true }),
  ).toBeVisible();
});

test("catalog failure offers recovery while an empty catalog explains the next step", async ({
  page,
}) => {
  await api(page);
  let available = false;
  await page.route("**/api/marketplace/catalog", (route) =>
    route.fulfill(
      available
        ? { json: { plugins: [] } }
        : { status: 503, json: { error: "Catalog temporarily unavailable" } },
    ),
  );
  await page.goto("/");
  await expect(page.getByRole("alert")).toContainText(
    "Catalog temporarily unavailable",
  );
  available = true;
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(
    page.getByRole("heading", {
      name: "The catalog is ready for its first plugin",
    }),
  ).toBeVisible();
});

test("Publisher uploads the file map and displays the backend's canonical Agent Plugin digest", async ({
  page,
}) => {
  await api(page, true);
  await page.route(
    "**/api/marketplace/publishers/publisher-1/plugins",
    (route) => route.fulfill({ json: { plugins: [plugin] } }),
  );
  const manifest = Buffer.from(
    '{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"source-reader"}',
  ).toString("base64");
  const files = {
    "plugin.json": manifest,
    "skills/reader/SKILL.md": "UmVhZCBzb3VyY2VzLg==",
  };
  await page.route(
    "**/api/marketplace/plugins/plugin-1/versions",
    async (route) => {
      expect(route.request().headers()["x-marketplace-csrf"]).toBe(
        "csrf-proof",
      );
      expect(route.request().postDataJSON()).toEqual({
        version: "1.1.0",
        files,
      });
      await route.fulfill({
        json: {
          version: {
            ...version,
            version: "1.1.0",
            digest: `sha256:${"b".repeat(64)}`,
          },
        },
      });
    },
  );
  await page.goto("/#/publishers");
  await page
    .getByRole("combobox", { name: "Plugin listing", exact: true })
    .selectOption("plugin-1");
  await page.getByLabel("Version", { exact: true }).fill("1.1.0");
  await page.getByLabel(/^Agent Plugin bundle/).setInputFiles({
    name: "plugin.json",
    mimeType: "application/json",
    buffer: Buffer.from(JSON.stringify(files)),
  });
  await page
    .getByRole("button", { name: "Publish version", exact: true })
    .click();
  await expect(
    page.getByRole("status", { name: "Publication result" }),
  ).toContainText("Version 1.1.0 published.");
  await expect(
    page.getByRole("status", { name: "Publication result" }),
  ).toContainText(`sha256:${"b".repeat(64)}`);
});

test("skip navigation focuses the catalog instead of changing the application route", async ({
  page,
}) => {
  await api(page);
  await page.goto("/");
  const skip = page.getByRole("link", { name: "Skip to content", exact: true });
  // WASM initialization can finish after the document's load event. Begin
  // keyboard navigation once the application's first focus target exists.
  await expect(skip).toBeAttached();
  await page.keyboard.press("Tab");
  await expect(skip).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.locator("main")).toBeFocused();
  await expect(
    page.getByRole("link", { name: "Source Reader", exact: true }),
  ).toBeVisible();
});

test("catalog search updates the visible plugins and can recover from an empty match", async ({
  page,
}) => {
  await api(page);
  await page.goto("/");
  await expect(
    page.getByRole("link", { name: "Source Reader", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("searchbox", { name: "Search plugins" })
    .fill("unlisted tool");
  await expect(
    page.getByRole("heading", { name: "No matching plugins" }),
  ).toBeVisible();
  await page
    .getByRole("searchbox", { name: "Search plugins" })
    .fill("research tools");
  await expect(
    page.getByRole("link", { name: "Source Reader", exact: true }),
  ).toBeVisible();
});

test("paid listings have no acquisition action and recalled Library versions have no download", async ({
  page,
}) => {
  await api(page, true);
  await page.route("**/api/marketplace/catalog/source-reader", (route) =>
    route.fulfill({
      json: { plugin: { ...plugin, price_cents: 1250 }, versions: [version] },
    }),
  );
  await page.route("**/api/marketplace/library", (route) =>
    route.fulfill({
      json: {
        entries: [
          {
            ...plugin,
            plugin_id: plugin.id,
            versions: [
              {
                ...version,
                recalled: true,
                recall_reason: "Unsafe repository access",
              },
            ],
          },
        ],
      },
    }),
  );
  await page.goto("/#/plugins/source-reader");
  await expect(
    page.getByText("Paid acquisition is not available yet."),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Add to Library" }),
  ).toHaveCount(0);
  await page.getByRole("link", { name: "Library", exact: true }).click();
  await expect(page.getByText("Recalled", { exact: true })).toBeVisible();
  await expect(page.getByText("Unsafe repository access")).toBeVisible();
  await expect(
    page.getByRole("link", { name: "Download version" }),
  ).toHaveCount(0);
});

test("a new listing preserves its metadata and opens publication for the created plugin", async ({
  page,
}) => {
  await api(page, true);
  await page.route(
    "**/api/marketplace/publishers/publisher-1/plugins",
    (route) => route.fulfill({ json: { plugins: [] } }),
  );
  await page.route("**/api/marketplace/plugins", async (route) => {
    expect(route.request().headers()["x-marketplace-csrf"]).toBe("csrf-proof");
    expect(route.request().postDataJSON()).toEqual({
      publisher_id: "publisher-1",
      slug: "source-reader",
      name: "Source Reader",
      description: "Read selected repositories.",
      license: "Apache-2.0",
      origin: "https://example.org/source-reader",
      price_cents: 0,
      currency: "USD",
      declared_permissions: ["repository:read", "network:example.org"],
    });
    await route.fulfill({ json: { plugin } });
  });
  await page.goto("/#/publishers");
  await page
    .getByRole("button", { name: "New plugin listing", exact: true })
    .click();
  await page.getByLabel("Plugin name", { exact: true }).fill("Source Reader");
  await page.getByLabel("URL slug", { exact: true }).fill("source-reader");
  await page
    .getByLabel("Description", { exact: true })
    .fill("Read selected repositories.");
  await page.getByLabel("License", { exact: true }).fill("Apache-2.0");
  await page
    .getByLabel("Origin URL", { exact: true })
    .fill("https://example.org/source-reader");
  await page
    .getByLabel("Declared Permissions", { exact: true })
    .fill("repository:read\n\n network:example.org ");
  await page
    .getByRole("button", { name: "Create listing", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Source Reader", exact: true }),
  ).toBeVisible();
  await expect(page.getByLabel("Version", { exact: true })).toBeVisible();
});

test("invalid upload file maps fail before sending a publication request", async ({
  page,
}) => {
  await api(page, true);
  await page.route(
    "**/api/marketplace/publishers/publisher-1/plugins",
    (route) => route.fulfill({ json: { plugins: [plugin] } }),
  );
  let publicationRequests = 0;
  await page.route("**/api/marketplace/plugins/plugin-1/versions", (route) => {
    publicationRequests += 1;
    return route.fulfill({
      status: 500,
      json: { error: "Unexpected publication" },
    });
  });
  await page.goto("/#/publishers");
  await page
    .getByRole("combobox", { name: "Plugin listing", exact: true })
    .selectOption("plugin-1");
  await page.getByLabel("Version", { exact: true }).fill("1.1.0");
  for (const bundle of [
    "not JSON",
    "[]",
    '{"plugin.json":42}',
    '{"\u00e9.md":"YQ=="}',
  ]) {
    await page.getByLabel(/^Agent Plugin bundle/).setInputFiles({
      name: "bundle.json",
      mimeType: "application/json",
      buffer: Buffer.from(bundle),
    });
    await page
      .getByRole("button", { name: "Publish version", exact: true })
      .click();
    await expect(page.getByRole("alert")).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Publish version", exact: true }),
    ).toBeEnabled();
  }
  expect(publicationRequests).toBe(0);
});

test("a delayed detail response cannot update a disposed route", async ({
  page,
}) => {
  await api(page, true);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  let release!: () => void;
  let started!: () => void;
  const pending = new Promise<void>((resolve) => {
    release = resolve;
  });
  const requested = new Promise<void>((resolve) => {
    started = resolve;
  });
  await page.route(
    "**/api/marketplace/catalog/source-reader",
    async (route) => {
      started();
      await pending;
      await route.fulfill({ json: { plugin, versions: [version] } });
    },
  );
  await page.goto("/#/plugins/source-reader");
  await requested;
  await page.getByRole("link", { name: "Explore", exact: true }).click();
  await expect(
    page.getByRole("link", { name: "Source Reader", exact: true }),
  ).toBeVisible();
  const received = page.waitForResponse(
    "**/api/marketplace/catalog/source-reader",
  );
  release();
  await (await received).finished();
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  expect(errors).toEqual([]);
  await expect(page).toHaveURL(/\/#\/$/);
  await expect(
    page.getByRole("heading", { name: "Source Reader", exact: true, level: 1 }),
  ).toHaveCount(0);
  await page.getByRole("link", { name: "Library", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Your Library" }),
  ).toBeVisible();
});

test("a delayed publication completion cannot refetch or restore a departed Publisher view", async ({
  page,
}) => {
  await api(page, true);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  let pluginReads = 0;
  await page.route(
    "**/api/marketplace/publishers/publisher-1/plugins",
    (route) => {
      pluginReads += 1;
      return route.fulfill({ json: { plugins: [plugin] } });
    },
  );
  let release!: () => void;
  let started!: () => void;
  const pending = new Promise<void>((resolve) => {
    release = resolve;
  });
  const requested = new Promise<void>((resolve) => {
    started = resolve;
  });
  await page.route(
    "**/api/marketplace/plugins/plugin-1/versions",
    async (route) => {
      started();
      await pending;
      await route.fulfill({
        json: { version: { ...version, version: "1.1.0" } },
      });
    },
  );
  await page.goto("/#/publishers");
  await page
    .getByRole("combobox", { name: "Plugin listing", exact: true })
    .selectOption("plugin-1");
  await page.getByLabel("Version", { exact: true }).fill("1.1.0");
  await page.getByLabel(/^Agent Plugin bundle/).setInputFiles({
    name: "bundle.json",
    mimeType: "application/json",
    buffer: Buffer.from('{"plugin.json":"e30="}'),
  });
  await page
    .getByRole("button", { name: "Publish version", exact: true })
    .click();
  await requested;
  await page.getByRole("link", { name: "Explore", exact: true }).click();
  await expect(
    page.getByRole("link", { name: "Source Reader", exact: true }),
  ).toBeVisible();
  const received = page.waitForResponse(
    "**/api/marketplace/plugins/plugin-1/versions",
  );
  release();
  await (await received).finished();
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  expect(errors).toEqual([]);
  expect(pluginReads).toBe(1);
  await expect(
    page.getByRole("status", { name: "Publication result" }),
  ).toHaveCount(0);
  await page.getByRole("link", { name: "Publishers", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Publisher tools", exact: true }),
  ).toBeVisible();
});

test("browser Back and Forward retain working hash routes without orphan listeners", async ({
  page,
}) => {
  await api(page, true);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await page.getByRole("link", { name: "Source Reader", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Source Reader", exact: true }),
  ).toBeVisible();
  await page.getByRole("link", { name: "Library", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Your Library" }),
  ).toBeVisible();
  await page.goBack();
  await expect(
    page.getByRole("heading", { name: "Source Reader", exact: true }),
  ).toBeVisible();
  await page.goForward();
  await expect(
    page.getByRole("heading", { name: "Your Library" }),
  ).toBeVisible();
  await page.getByRole("link", { name: "Explore", exact: true }).click();
  await expect(
    page.getByRole("link", { name: "Source Reader", exact: true }),
  ).toBeVisible();
  await page.getByRole("link", { name: "Source Reader", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Source Reader", exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});
