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
  await expect(page.getByRole("status")).toContainText(
    "Version 1.1.0 published.",
  );
  await expect(page.getByRole("status")).toContainText(
    `sha256:${"b".repeat(64)}`,
  );
});

test("skip navigation focuses the catalog instead of changing the application route", async ({
  page,
}) => {
  await api(page);
  await page.goto("/");
  await page.keyboard.press("Tab");
  await page.keyboard.press("Enter");
  await expect(page.locator("main")).toBeFocused();
  await expect(
    page.getByRole("link", { name: "Source Reader", exact: true }),
  ).toBeVisible();
});
