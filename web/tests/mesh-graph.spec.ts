import { expect, test, type Page } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import { meshScene } from "./mesh-scene.mjs";

async function setup(page: Page, subject = false) {
  const scene = meshScene();
  let data = scene.data;
  if (subject)
    data.access = { kind: "subject", tenant: "acme", subject: "ryota" };
  const errors: string[] = [];
  const warnings: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (message) => {
    if (
      message.type() === "warn" &&
      /cytoscape|parent|style.*invalid/i.test(message.text())
    )
      warnings.push(message.text());
  });
  await installBearerDashboard(
    page,
    "synthetic-mesh-test",
    subject ? { tenant: "acme", name: "ryota" } : undefined,
  );
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    const json =
      path === "/api/session"
        ? { access: data.access, node_id: data.node.id }
        : path === "/api/state"
          ? data
          : path === "/api/discover"
            ? scene.discovery
            : path === "/api/mesh"
              ? { nodes: [], errors: [] }
              : path === "/api/workspaces/product-lab"
                ? {
                    workspace: data.workspaces[0],
                    tasks: data.tasks,
                    artifacts: data.artifacts,
                    events: data.events,
                    messages: [],
                  }
                : [];
    return route.fulfill({ json });
  });
  await page.goto("/graph?channel=product-lab");
  await expect(page.locator(".mesh-canvas canvas").first()).toBeVisible();
  await expect(page.locator(".mesh-node-label").first()).toBeVisible();
  return {
    errors,
    warnings,
    revoke() {
      data = {
        ...data,
        registry: [],
        workspaces: [],
        tasks: [],
        runs: [],
        artifacts: [],
        conversations: [],
        events: [],
        peers: [],
      };
    },
  };
}

test("renders mesh groups and navigates node details, tasks and the existing relationship dialog", async ({
  page,
}) => {
  const { errors, warnings } = await setup(page);
  const inspector = page.getByRole("complementary", { name: "Node details" });
  await expect(
    inspector.getByRole("heading", { name: "Planner Agent", exact: true }),
  ).toBeVisible();
  await inspector.getByRole("button", { name: "Tasks 3", exact: true }).click();
  await inspector
    .getByRole("button", { name: "Build Prototype Running", exact: true })
    .click();
  await expect(
    inspector.getByRole("heading", { name: "Build Prototype", exact: true }),
  ).toBeVisible();
  await inspector
    .getByRole("button", { name: "Connections", exact: true })
    .click();
  await expect(inspector.locator(".mesh-relations-list")).toContainText(
    "prerequisite for",
  );
  await page
    .locator(".mesh-node-label")
    .filter({ hasText: "Planner Agent" })
    .click();
  await inspector
    .getByRole("button", { name: "Open details", exact: true })
    .click();
  await expect(
    page.getByRole("region", { name: "Agent relationship graph" }),
  ).toBeVisible();
  expect(errors).toEqual([]);
  expect(warnings).toEqual([]);
});
test("search, type and relation filters, neighborhood focus, list view and reset work", async ({
  page,
}) => {
  const { errors } = await setup(page);
  const search = page.getByRole("searchbox");
  await search.fill("nothing-matches");
  await expect(
    page.getByRole("heading", { name: "No matching nodes" }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Reset filters", exact: true })
    .first()
    .click();
  await search.fill("Prototype");
  await expect(
    page.locator(".mesh-node-label").filter({ hasText: "Build Prototype" }),
  ).toBeVisible();
  await expect(
    page.locator(".mesh-node-label").filter({ hasText: "Data Analyst" }),
  ).toHaveCount(0);
  await search.fill("");
  await page.getByRole("checkbox", { name: "Tools", exact: true }).uncheck();
  await expect(
    page.locator(".mesh-node-label").filter({ hasText: "Web Search" }),
  ).toHaveCount(0);
  await page.getByRole("checkbox", { name: "Clusters", exact: true }).uncheck();
  await page.getByRole("checkbox", { name: "Clusters", exact: true }).check();
  await expect(
    page.locator(".mesh-node-label").filter({ hasText: "Planner Agent" }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Focus neighborhood", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Show full graph" }),
  ).toHaveAttribute("aria-pressed", "true");
  await page
    .getByRole("button", { name: "Relationship list", exact: true })
    .click();
  const table = page.getByRole("table", { name: "Relationship list" });
  await expect(table).toBeVisible();
  await page.getByText("Relation types", { exact: true }).first().click();
  await page.getByRole("checkbox", { name: "executes", exact: true }).uncheck();
  await expect(table).not.toContainText("→ executes →");
  expect(errors).toEqual([]);
});
test("all perspectives and layout engines render and execution events select tasks", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const { errors, warnings } = await setup(page);
  for (const mode of [
    "mesh",
    "collaboration",
    "knowledge",
    "execution",
    "topology",
  ]) {
    await page
      .getByLabel("Graph perspective", { exact: true })
      .selectOption(mode);
    await expect(page.locator(".mesh-canvas canvas").first()).toBeVisible();
    await expect(page.locator(".mesh-node-label").first()).toBeVisible();
    const defaults: Record<string, string> = {
      mesh: "Planner Agent",
      collaboration: "Product Lab",
      knowledge: "PRD",
      execution: "Research & Plan",
      topology: "Local Agent Cluster",
    };
    await expect(page.locator(".mesh-inspector h2")).toHaveText(defaults[mode]);
    if (mode === "execution") {
      await expect(page.locator(".mesh-statistics")).toContainText(
        "Blocked tasks",
      );
      await page.locator(".mesh-timeline-track button").last().click();
      await expect(page.locator(".mesh-inspector h2")).toHaveText(
        "Research & Plan",
      );
    }
    // Labels follow Cytoscape pan/zoom through an animation-frame projection.
    await page.evaluate(
      () =>
        new Promise<void>((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
        ),
    );
    const clipped = await page
      .locator(".mesh-node-label")
      .evaluateAll((labels) => {
        const stage = document
          .querySelector(".mesh-stage")!
          .getBoundingClientRect();
        return labels
          .filter((label) => {
            const r = label.getBoundingClientRect();
            return (
              r.bottom > stage.bottom + 1 ||
              r.top < stage.top - 1 ||
              r.left < stage.left - 1 ||
              r.right > stage.right + 1
            );
          })
          .map((label) => label.textContent);
      });
    expect(
      clipped,
      `${mode} fits its own canvas after a perspective change`,
    ).toEqual([]);
    await page.screenshot({ path: `test-results/mesh-${mode}.png` });
  }
  for (const layout of ["force", "circle", "structured"]) {
    await page.getByLabel("Layout", { exact: true }).selectOption(layout);
    await expect(page.locator(".mesh-node-label").first()).toBeVisible();
    await page.getByRole("button", { name: "Zoom in", exact: true }).click();
    await page.getByRole("button", { name: "Zoom out", exact: true }).click();
    await page.getByRole("button", { name: "Fit graph", exact: true }).click();
  }
  expect(errors).toEqual([]);
  expect(warnings).toEqual([]);
});
test("zoom and node positions survive snapshot refresh and revoked details disappear", async ({
  page,
}) => {
  const { errors, revoke } = await setup(page);
  const before = await page.locator(".mesh-label-layer").getAttribute("style");
  await page.getByRole("button", { name: "Zoom in", exact: true }).click();
  await expect(page.locator(".mesh-label-layer")).not.toHaveAttribute(
    "style",
    before!,
  );
  const camera = await page.locator(".mesh-label-layer").getAttribute("style");
  await page.waitForResponse((r) => r.url().includes("/api/state"));
  await expect(page.locator(".mesh-label-layer")).toHaveAttribute(
    "style",
    camera!,
  );
  revoke();
  await expect(page.locator(".mesh-inspector")).toHaveCount(0, {
    timeout: 8000,
  });
  await expect(page.locator(".mesh-graph")).not.toContainText("Planner Agent");
  expect(errors).toEqual([]);
});
test("subject graph excludes remote discovery and topology", async ({
  page,
}) => {
  const { errors } = await setup(page, true);
  await expect(
    page
      .getByLabel("Graph perspective", { exact: true })
      .locator("option[value='topology']"),
  ).toHaveCount(0);
  await expect(
    page.locator(".mesh-node-label").filter({ hasText: "Data Analyst" }),
  ).toHaveCount(0);
  expect(errors).toEqual([]);
});
for (const viewport of [
  { width: 390, height: 844 },
  { width: 900, height: 600 },
]) {
  test(`graph controls, inspector and navigation fit ${viewport.width}x${viewport.height}`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    const { errors } = await setup(page);
    await page
      .getByRole("button", { name: "Close details", exact: true })
      .click();
    await page.getByRole("button", { name: "Filters", exact: true }).click();
    await page.getByRole("checkbox", { name: "Tools", exact: true }).uncheck();
    await page.getByRole("button", { name: "Filters", exact: true }).click();
    await page
      .getByRole("button", { name: "Relationship list", exact: true })
      .click();
    await page
      .getByRole("table", { name: "Relationship list" })
      .getByRole("button", { name: "Build Prototype", exact: true })
      .click();
    await expect(page.locator(".mesh-inspector h2")).toHaveText(
      "Build Prototype",
    );
    await expect(
      page.getByRole("button", { name: "Close details", exact: true }),
    ).toBeInViewport();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
    await page.screenshot({ path: `test-results/mesh-${viewport.width}.png` });
    expect(errors).toEqual([]);
  });
}
