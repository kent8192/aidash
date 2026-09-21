import { test, expect } from "@playwright/test";

const skill = {
  id: "research",
  version: "1.0.0",
  kind: "skill",
  name: { en: "Research" },
  description: { en: "Read sources" },
  config: { instructions: "Check the sources." },
};
const markdown =
  "---\r\nname: research\r\ndescription: >\r\n  Read sources\r\n  carefully\r\n---\r\n# Research\r\nCheck the sources.";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    sessionStorage.setItem("aidash-token", "fixture-token");
    localStorage.setItem("aidash-locale", "en-US");
  });
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({
        json: { access: { kind: "operator" }, node_id: "aidash://test" },
      });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access: { kind: "operator" },
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
    if (path === "/api/registry/import")
      return route.fulfill({
        json: {
          imported: route.request().postDataJSON().entries.length,
          unchanged: 0,
        },
      });
    if (path === "/api/mesh")
      return route.fulfill({ json: { nodes: [], errors: [] } });
    if (path === "/api/discover")
      return route.fulfill({ json: { agents: [], errors: [] } });
    return route.fulfill({ json: [] });
  });
  await page.goto("/registry");
});

test("previews a SKILL.md file and imports its instructions with an explicit version", async ({
  page,
}) => {
  await page
    .getByRole("button", { name: "Import entities", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Import files").setInputFiles({
    name: "SKILL.md",
    mimeType: "text/markdown",
    buffer: Buffer.from(markdown),
  });
  await dialog.getByLabel("Version for imported skills").fill("2.1.0");
  await dialog
    .getByRole("button", { name: "Preview import", exact: true })
    .click();
  await expect(dialog.getByRole("listitem")).toContainText("research@2.1.0");
  await page.screenshot({
    path: test.info().outputPath("registry-import-preview.png"),
  });
  const request = page.waitForRequest("**/api/registry/import");
  await dialog.getByRole("button", { name: "Import all", exact: true }).click();
  expect((await request).postDataJSON()).toMatchObject({
    entries: [
      {
        id: "research",
        version: "2.1.0",
        kind: "skill",
        description: { en: "Read sources carefully" },
        config: { instructions: "# Research\r\nCheck the sources." },
      },
    ],
  });
  await expect(dialog.getByRole("status")).toHaveText(
    "Imported: 1 / Unchanged: 0",
  );
  await expect(
    dialog.getByRole("button", { name: "Import all", exact: true }),
  ).toBeDisabled();
});

test("imports multiple files in one request and refreshes the registry", async ({
  page,
}) => {
  let saved = false;
  await page.route("**/api/registry/import", async (route) => {
    saved = true;
    expect(route.request().postDataJSON().entries).toHaveLength(2);
    return route.fulfill({ json: { imported: 1, unchanged: 1 } });
  });
  await page.route("**/api/state", async (route) =>
    route.fulfill({
      json: {
        access: { kind: "operator" },
        node: {
          id: "aidash://test",
          endpoint: "http://localhost",
          protocol_version: "0.1",
          capabilities: [],
          clusters: [],
        },
        registry: saved
          ? [
              {
                ...skill,
                capabilities: [],
                tags: [],
                languages: [],
                skills: [],
                schema: {},
              },
            ]
          : [],
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
    }),
  );
  await page
    .getByRole("button", { name: "Import entities", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Import files").setInputFiles([
    {
      name: "one.json",
      mimeType: "application/json",
      buffer: Buffer.from(JSON.stringify(skill)),
    },
    {
      name: "two.json",
      mimeType: "application/json",
      buffer: Buffer.from(
        JSON.stringify({ entries: [{ ...skill, id: "other" }] }),
      ),
    },
  ]);
  await dialog
    .getByRole("button", { name: "Preview import", exact: true })
    .click();
  await expect(dialog.getByRole("listitem")).toHaveCount(2);
  await dialog.getByRole("button", { name: "Import all", exact: true }).click();
  await expect(dialog.getByRole("status")).toHaveText(
    "Imported: 1 / Unchanged: 1",
  );
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Research", exact: true }),
  ).toBeVisible();
});

test("changing pasted JSON invalidates the preview and reports duplicate entries", async ({
  page,
}) => {
  await page
    .getByRole("button", { name: "Import entities", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  const input = dialog.getByLabel("Paste SKILL.md or JSON");
  await dialog.getByLabel("Import files").setInputFiles({
    name: "SKILL.md",
    mimeType: "text/markdown",
    buffer: Buffer.from(markdown),
  });
  await expect(input).toHaveCount(0);
  await dialog
    .getByRole("button", { name: "Preview import", exact: true })
    .click();
  await dialog
    .getByRole("button", { name: "Clear files", exact: true })
    .click();
  await expect(
    dialog.getByRole("button", { name: "Import all", exact: true }),
  ).toHaveCount(0);
  await expect(dialog.getByLabel("Import files")).toHaveValue("");
  await input.fill(JSON.stringify([skill]));
  await dialog
    .getByRole("button", { name: "Preview import", exact: true })
    .click();
  await expect(
    dialog.getByRole("button", { name: "Import all", exact: true }),
  ).toBeVisible();
  await input.fill(JSON.stringify([skill, skill]));
  await expect(
    dialog.getByRole("button", { name: "Import all", exact: true }),
  ).toHaveCount(0);
  await dialog
    .getByRole("button", { name: "Preview import", exact: true })
    .click();
  await expect(dialog.getByRole("alert")).toContainText(
    "duplicate IDs and versions",
  );
});

test("invalid input and oversized files cannot be submitted", async ({
  page,
}) => {
  await page
    .getByRole("button", { name: "Import entities", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  for (const text of [
    "not json",
    "---\nname: invalid\n---\n",
    JSON.stringify({ ...skill, name: { en: {} } }),
    "[]",
  ]) {
    await dialog.getByLabel("Paste SKILL.md or JSON").fill(text);
    await dialog
      .getByRole("button", { name: "Preview import", exact: true })
      .click();
    await expect(dialog.getByRole("alert")).toBeVisible();
    await expect(
      dialog.getByRole("button", { name: "Import all", exact: true }),
    ).toHaveCount(0);
  }
  await dialog.getByLabel("Import files").setInputFiles({
    name: "large.json",
    mimeType: "application/json",
    buffer: Buffer.alloc(512_001, "a"),
  });
  await expect(dialog.getByRole("alert")).toContainText("512 KB");
  await expect(
    dialog.getByRole("button", { name: "Preview import", exact: true }),
  ).toBeDisabled();
});

test("server conflicts retain the preview for correction and retry", async ({
  page,
}) => {
  await page.route("**/api/registry/import", async (route) =>
    route.fulfill({
      status: 409,
      json: {
        error:
          "research@1.0.0: published versions are immutable; choose a new version",
      },
    }),
  );
  await page
    .getByRole("button", { name: "Import entities", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Paste SKILL.md or JSON").fill(markdown);
  await dialog
    .getByRole("button", { name: "Preview import", exact: true })
    .click();
  await dialog.getByRole("button", { name: "Import all", exact: true }).click();
  await expect(dialog.getByRole("alert")).toContainText(
    "published versions are immutable",
  );
  await expect(
    dialog.getByRole("button", { name: "Import all", exact: true }),
  ).toBeEnabled();
  await dialog.getByLabel("Version for imported skills").fill("1.0.1");
  await expect(
    dialog.getByRole("button", { name: "Import all", exact: true }),
  ).toHaveCount(0);
});

test("subject sessions cannot see import controls", async ({ page }) => {
  await page.route("**/api/session", async (route) =>
    route.fulfill({
      json: {
        access: { kind: "subject", tenant: "acme", subject: "alice" },
        node_id: "aidash://test",
      },
    }),
  );
  await page.reload();
  await expect(
    page.getByRole("button", { name: "Register entity", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Import entities", exact: true }),
  ).toHaveCount(0);
});
