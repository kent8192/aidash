import { test, expect } from "@playwright/test";

test("registers an agent with selected versioned skills", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() => {
    sessionStorage.setItem("aidash-token", "fixture-token");
    localStorage.setItem("aidash-locale", "ja-JP");
  });
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") {
      await route.abort();
    } else if (path === "/api/session") {
      await route.fulfill({
        json: { access: { kind: "operator" }, node_id: "aidash://test" },
      });
    } else if (path === "/api/state") {
      await route.fulfill({
        json: {
          access: { kind: "operator" },
          node: {
            id: "aidash://test",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            capabilities: [],
            clusters: [],
          },
          registry: [
            {
              id: "model",
              version: "1.0.0",
              kind: "model",
              name: { en: "Model" },
              description: { en: "Model" },
              capabilities: [],
              languages: [],
              tags: [],
              config: {},
            },
            ...["1.0.0", "2.0.0"].map((version) => ({
              id: "research-skill",
              version,
              kind: "skill",
              name: { en: "Research skill" },
              description: { en: "Instructions" },
              capabilities: [],
              languages: [],
              tags: [],
              config: { instructions: "Research carefully" },
            })),
          ],
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
    } else if (path === "/api/mesh") {
      await route.fulfill({ json: { nodes: [], errors: [] } });
    } else if (path === "/api/discover") {
      await route.fulfill({ json: { agents: [], errors: [] } });
    } else if (
      path === "/api/registry" &&
      route.request().method() === "POST"
    ) {
      await route.fulfill({ json: route.request().postDataJSON() });
    } else {
      await route.fulfill({ json: [] });
    }
  });
  await page.goto("/registry");
  await page
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("エンティティの種類").selectOption("agent");
  await expect(dialog.getByLabel("エンティティID")).toHaveCount(0);
  await dialog.getByLabel("名前").fill("Skilled agent");
  await dialog.getByLabel("説明").fill("Uses selected skill versions");
  await dialog.locator('[name="model"]').selectOption("model@1.0.0");
  await dialog.locator('[name="instructions"]').fill("Use the selected skill");
  await dialog.locator('[name="skills"][value="research-skill@2.0.0"]').check();
  await expect(
    dialog.locator('[name="skills"][value="research-skill@1.0.0"]'),
  ).not.toBeChecked();
  const submitted = page.waitForRequest(
    (request) =>
      new URL(request.url()).pathname === "/api/registry" &&
      request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  expect((await submitted).postDataJSON().config).toMatchObject({
    model: { id: "model", version: "1.0.0" },
    skills: [{ id: "research-skill", version: "2.0.0" }],
  });
  await expect(dialog).not.toBeVisible();
  expect(errors).toEqual([]);
});
