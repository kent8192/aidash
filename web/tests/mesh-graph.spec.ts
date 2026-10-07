import { expect, test, type Page } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import { meshScene } from "./mesh-scene.mjs";
import type { Core } from "cytoscape";
import { cytoscapeCamera, expectCytoscapeFitted } from "./graph-fit-assertions";

async function setup(
  page: Page,
  subject = false,
  extraWorkspace = false,
  locale = "en-US",
) {
  const scene = meshScene();
  if (extraWorkspace) {
    scene.data.workspaces.push({
      ...scene.data.workspaces[0],
      id: "other-workspace",
      title: "Other Workspace",
    });
    scene.data.tasks.push({
      ...scene.data.tasks[0],
      id: "other-task",
      workspace_id: "other-workspace",
      title: "Other Task",
    });
    scene.data.events.push({
      ...scene.data.events[0],
      id: "other-workspace-event",
      workspace_id: "other-workspace",
      kind: "task.created",
      data: { task_id: "other-task" },
    });
  }
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
  await page.addInitScript(
    (locale) => localStorage.setItem("aidash-locale", locale),
    locale,
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
  // Fit samples current geometry; later font arrival must not trigger a refit.
  await page.evaluate(() => document.fonts.ready.then(() => undefined));
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

test("inspector follows the selected graph workspace", async ({ page }) => {
  const { errors } = await setup(page, false, true);
  await page.getByLabel("Graph workspace").selectOption("other-workspace");
  await page
    .locator(".mesh-node-label")
    .filter({ hasText: "Other Task" })
    .click();
  const inspector = page.getByRole("complementary", { name: "Node details" });
  await inspector.getByRole("button", { name: "Events", exact: true }).click();
  await expect(inspector).toContainText("task.created");
  expect(errors).toEqual([]);
});

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
}, testInfo) => {
  const { errors } = await setup(page);
  const search = page.getByRole("banner").getByRole("searchbox");
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
  await page.locator(".mesh-filters").screenshot({
    path: testInfo.outputPath("filters-light.png"),
    animations: "disabled",
  });
  await page.screenshot({
    path: testInfo.outputPath("graph-light.png"),
    animations: "disabled",
  });
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
  await page.locator(".account-popover > summary").click();
  await page.getByRole("button", { name: "Dark theme", exact: true }).click();
  await page.locator(".account-popover > summary").click();
  await expect(page.locator(".intent-app")).toHaveAttribute(
    "data-theme",
    "dark",
  );
  await expect(
    page.getByRole("checkbox", { name: "Tools", exact: true }),
  ).not.toBeChecked();
  await page.locator(".mesh-filters").screenshot({
    path: testInfo.outputPath("filters-dark.png"),
    animations: "disabled",
  });
  expect(errors).toEqual([]);
});
test("connected local node opens topology and relationship controls only show active options", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.getByLabel("Graph perspective").selectOption("topology");
  await page
    .locator(".mesh-region-execution-label")
    .filter({ hasText: "aidash://product-lab" })
    .click();
  await expect(page.getByLabel("Graph perspective")).toHaveValue("topology");
  await expect(page.locator(".mesh-inspector h2")).toHaveText(
    "aidash://product-lab",
  );
  await page.getByLabel("Graph perspective").selectOption("neighborhood");
  await expect(page.getByRole("banner").getByRole("searchbox")).toHaveCount(0);
  await expect(page.getByLabel("Activity window")).toHaveCount(0);
  await expect(page.getByLabel("Layout", { exact: true })).toHaveCount(0);
  await page
    .getByRole("button", {
      name: "Node topology and communication",
      exact: true,
    })
    .click();
  await expect(page.locator(".mesh-canvas svg > g")).toHaveCount(2);
  await expect(page.locator(".collab-run-grid button")).not.toHaveCount(0);
  expect(errors).toEqual([]);
});
test("leaving a focused agent relationship view clears its URL without changing the chosen perspective", async ({
  page,
}) => {
  const { errors } = await setup(page);
  const focus = JSON.stringify([
    "entity",
    "aidash://product-lab",
    "agent",
    "planner",
    "1.0.0",
  ]);
  await page.goto(
    `/graph?channel=product-lab&focus=${encodeURIComponent(focus)}`,
  );
  await expect(page.getByLabel("Graph perspective")).toHaveValue(
    "neighborhood",
  );
  await page.getByLabel("Graph perspective").selectOption("knowledge");
  await expect(page.getByLabel("Graph perspective")).toHaveValue("knowledge");
  await expect
    .poll(() => new URL(page.url()).searchParams.has("focus"))
    .toBe(false);
  await page.goBack();
  await expect(page.getByLabel("Graph perspective")).toHaveValue(
    "neighborhood",
  );
  await expect
    .poll(() => new URL(page.url()).searchParams.get("focus"))
    .toBe(focus);
  await page.goForward();
  await expect
    .poll(() => new URL(page.url()).searchParams.has("focus"))
    .toBe(false);
  await expect(page.getByLabel("Graph perspective")).toHaveValue("knowledge");
  await page.reload();
  await expect(page.getByLabel("Graph perspective")).toHaveValue("mesh");
  expect(errors).toEqual([]);
});
test("all perspectives and layout engines render and execution events select tasks", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const { errors, warnings } = await setup(page);
  await page
    .getByRole("button", { name: "Fit entire graph", exact: true })
    .click();
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
    await expectCytoscapeFitted(page.locator(".mesh-canvas"));
    await page.locator(".mesh-canvas").evaluate((element) => {
      const cy = (element as HTMLElement & { _cyreg: { cy: Core } })._cyreg.cy;
      cy.zoom(2);
      cy.pan({ x: 50000, y: -50000 });
    });
    await page
      .getByRole("button", { name: "Fit entire graph", exact: true })
      .click();
    await expectCytoscapeFitted(page.locator(".mesh-canvas"));
    await page.screenshot({ path: `test-results/mesh-${mode}.png` });
    for (const layout of ["force", "circle", "structured"]) {
      await page.getByLabel("Layout", { exact: true }).selectOption(layout);
      await expect(page.locator(".mesh-node-label").first()).toBeVisible();
      await page.getByRole("button", { name: "Zoom in", exact: true }).click();
      await page.getByRole("button", { name: "Zoom out", exact: true }).click();
      await page
        .getByRole("button", { name: "Fit entire graph", exact: true })
        .click();
      await expectCytoscapeFitted(page.locator(".mesh-canvas"));
    }
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
test("subject graph offers topology without joining remote discovery", async ({
  page,
}) => {
  const { errors } = await setup(page, true);
  await expect(
    page
      .getByLabel("Graph perspective", { exact: true })
      .locator("option[value='topology']"),
  ).toHaveCount(1);
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
  }, testInfo) => {
    await page.setViewportSize(viewport);
    const { errors } = await setup(page);
    await page
      .getByRole("button", { name: "Close details", exact: true })
      .click();
    await page.getByRole("button", { name: "Filters", exact: true }).click();
    await page.getByRole("checkbox", { name: "Tools", exact: true }).uncheck();
    await page.screenshot({
      path: testInfo.outputPath("filters-mobile.png"),
      animations: "disabled",
    });
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

test("whole-graph fit recovers all labels and groups below the former zoom floor", async ({
  page,
}) => {
  await setup(page);
  const state = await page.locator(".mesh-canvas").evaluate((element) => {
    const cy = (element as HTMLElement & { _cyreg: { cy: Core } })._cyreg.cy;
    cy.nodes()
      .not(":parent")
      .forEach((node, index) => {
        node.position({ x: index * 12000, y: (index % 3) * 10000 });
      });
    cy.zoom(2);
    cy.pan({ x: 50000, y: -50000 });
    return cy
      .nodes()
      .map((node) => ({ id: node.id(), position: node.position() }));
  });
  const fit = page.getByRole("button", { name: /^Fit (entire )?graph$/ });
  await fit.click();
  await expectCytoscapeFitted(page.locator(".mesh-canvas"));
  await expect(fit).toHaveText("Fit entire graph");
  const after = await page.locator(".mesh-canvas").evaluate((element) => {
    const cy = (element as HTMLElement & { _cyreg: { cy: Core } })._cyreg.cy;
    return {
      zoom: cy.zoom(),
      nodes: cy
        .nodes()
        .map((node) => ({ id: node.id(), position: node.position() })),
    };
  });
  expect(after.zoom).toBeGreaterThan(0);
  expect(after.zoom).toBeLessThan(0.12);
  expect(after.nodes).toEqual(state);
  await expect(page.locator(".mesh-inspector h2")).toHaveText("Planner Agent");
  await page.getByRole("button", { name: "Zoom out", exact: true }).click();
  const zoomedOut = await cytoscapeCamera(page.locator(".mesh-canvas"));
  expect(zoomedOut.zoom).toBeLessThan(after.zoom);
  await page.getByRole("button", { name: "Zoom in", exact: true }).click();
  expect(
    (await cytoscapeCamera(page.locator(".mesh-canvas"))).zoom,
  ).toBeCloseTo(after.zoom, 8);
});

for (const locale of ["en-US", "ja-JP"]) {
  test(`whole-graph fit keeps its label and keyboard operation above a compact inspector in ${locale}`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await setup(page, false, false, locale);
    const canvas = page.locator(".mesh-canvas");
    const fit = page.getByRole("button", {
      name: locale === "ja-JP" ? "全体表示" : "Fit entire graph",
      exact: true,
    });
    await expect(fit).toBeEnabled();
    await expect(fit).toBeInViewport();
    await expect(fit).toHaveText(
      locale === "ja-JP" ? "全体表示" : "Fit entire graph",
    );
    const before = await cytoscapeCamera(canvas);
    for (const key of ["Enter", "Space"]) {
      await canvas.evaluate((element) => {
        const cy = (element as HTMLElement & { _cyreg: { cy: Core } })._cyreg
          .cy;
        cy.zoom(2);
        cy.pan({ x: -50000, y: 50000 });
      });
      await fit.focus();
      await fit.press(key);
      await expectCytoscapeFitted(canvas);
      await expect(fit).toBeFocused();
    }
    const after = await cytoscapeCamera(canvas);
    expect(after.nodes).toEqual(before.nodes);
    await expect(page.locator(".mesh-inspector")).toBeVisible();
    await fit.click();
    await expectCytoscapeFitted(canvas);
    const repeated = await cytoscapeCamera(canvas);
    expect(repeated.zoom).toBeCloseTo(after.zoom, 8);
    expect(repeated.pan.x).toBeCloseTo(after.pan.x, 6);
    expect(repeated.pan.y).toBeCloseTo(after.pan.y, 6);
    await page.screenshot({
      path: `test-results/whole-graph-fit-${locale}-390.png`,
    });
  });
}

test("empty filters and viewport changes preserve the camera until explicit fit", async ({
  page,
}) => {
  await setup(page);
  const canvas = page.locator(".mesh-canvas");
  const fit = page.getByRole("button", {
    name: "Fit entire graph",
    exact: true,
  });
  await page.getByRole("button", { name: "Zoom in", exact: true }).click();
  const before = await cytoscapeCamera(canvas);
  await page.getByRole("banner").getByRole("searchbox").fill("nothing-matches");
  await expect(
    page.getByRole("heading", { name: "No matching nodes" }),
  ).toBeVisible();
  await expect(fit).toBeDisabled();
  await expect
    .poll(() =>
      page.locator(".mesh-minimap canvas").evaluate((element) => {
        const canvas = element as HTMLCanvasElement;
        return canvas
          .getContext("2d")!
          .getImageData(0, 0, canvas.width, canvas.height)
          .data.every((value) => value === 0);
      }),
    )
    .toBe(true);
  expect((await cytoscapeCamera(canvas)).zoom).toEqual(before.zoom);
  await page.getByRole("banner").getByRole("searchbox").fill("");
  await expect(fit).toBeEnabled();
  expect((await cytoscapeCamera(canvas)).pan).toEqual(before.pan);
  await page
    .getByRole("button", { name: "Close details", exact: true })
    .click();
  await page.setViewportSize({ width: 900, height: 600 });
  expect((await cytoscapeCamera(canvas)).zoom).toEqual(before.zoom);
  expect((await cytoscapeCamera(canvas)).pan).toEqual(before.pan);
  await fit.click();
  await expectCytoscapeFitted(canvas);
  const fitted = await cytoscapeCamera(canvas);
  await canvas.evaluate((element) => {
    (element as HTMLElement).style.display = "none";
  });
  await expect(fit).toBeDisabled();
  await canvas.evaluate((element) => {
    (element as HTMLElement).style.display = "";
  });
  await expect(fit).toBeEnabled();
  expect((await cytoscapeCamera(canvas)).pan).toEqual(fitted.pan);
  expect((await cytoscapeCamera(canvas)).zoom).toEqual(fitted.zoom);
  await page.locator(".mesh-camera").evaluate((element) => {
    const buttons = [...element.querySelectorAll<HTMLButtonElement>("button")];
    buttons
      .find((button) => button.textContent?.includes("Fit entire graph"))!
      .click();
    buttons[0].click();
  });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  expect((await cytoscapeCamera(canvas)).zoom).toBeCloseTo(
    fitted.zoom * 1.25,
    8,
  );
});

test("agent neighborhood fits its current nodes and labels without resetting exploration", async ({
  page,
}) => {
  await setup(page);
  const focus = JSON.stringify([
    "entity",
    "aidash://product-lab",
    "agent",
    "planner",
    "1.0.0",
  ]);
  await page.goto(
    `/graph?channel=product-lab&focus=${encodeURIComponent(focus)}`,
  );
  const canvas = page.locator(".collab-cytoscape");
  await expect(canvas.locator("canvas").first()).toBeVisible();
  const fit = page.getByRole("button", {
    name: "Fit entire graph",
    exact: true,
  });
  await expect(fit).toBeEnabled();
  const before = await canvas.evaluate((element) => {
    const cy = (element as HTMLElement & { _cyreg: { cy: Core } })._cyreg.cy;
    cy.nodes().forEach((node, index) =>
      node.position({ x: index * 15000, y: (index % 2) * 40000 }),
    );
    cy.zoom(3);
    cy.pan({ x: 50000, y: 50000 });
    return cy
      .nodes()
      .map((node) => ({ id: node.id(), position: node.position() }));
  });
  await fit.focus();
  await fit.press("Enter");
  await expectCytoscapeFitted(canvas);
  const after = await cytoscapeCamera(canvas);
  expect(after.nodes).toEqual(before);
  expect(after.zoom).toBeGreaterThan(0);
  expect(after.zoom).toBeLessThan(0.15);
  await page.getByRole("button", { name: "Zoom in", exact: true }).click();
  expect((await cytoscapeCamera(canvas)).zoom).toBeCloseTo(
    after.zoom * 1.25,
    8,
  );
  await expect(page.getByLabel("Graph perspective")).toHaveValue(
    "neighborhood",
  );
});

test("a filtered single resource is centered without automatic magnification", async ({
  page,
}) => {
  await setup(page);
  await page.getByLabel("Graph perspective").selectOption("collaboration");
  for (const checkbox of await page
    .locator(".mesh-filters")
    .getByRole("checkbox")
    .all()) {
    if ((await checkbox.locator("..").innerText()).trim() !== "Conversations")
      await checkbox.uncheck();
  }
  const canvas = page.locator(".mesh-canvas");
  await expect(page.locator(".mesh-node-label")).toHaveCount(1);
  await canvas.evaluate((element) => {
    const cy = (element as HTMLElement & { _cyreg: { cy: Core } })._cyreg.cy;
    cy.zoom(2.5);
    cy.pan({ x: -10000, y: 10000 });
  });
  await page
    .getByRole("button", { name: "Fit entire graph", exact: true })
    .click();
  await expectCytoscapeFitted(canvas);
  expect((await cytoscapeCamera(canvas)).zoom).toBe(1);
});
