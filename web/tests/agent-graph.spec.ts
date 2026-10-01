import { expect, test, type Page } from "@playwright/test";
import { expectSvgFitted } from "./graph-fit-assertions";
import { installBearerDashboard } from "./auth-fixture";

function entity(
  id: string,
  kind: string,
  name: string,
  config = {},
  version = "1.0.0",
) {
  return {
    id,
    kind,
    version,
    name: { en: name },
    description: { en: `${name} description` },
    capabilities: [],
    languages: ["en"],
    tags: [],
    skills: [],
    schema: {},
    config,
  };
}
function fixture() {
  return {
    access: { kind: "operator" },
    node: {
      id: "aidash://test",
      endpoint: "http://localhost",
      protocol_version: "0.1",
      capabilities: [],
      clusters: [],
    },
    registry: [
      entity("researcher", "agent", "Researcher", {
        model: { id: "model", version: "1.0.0" },
        tools: [
          { id: "search", version: "1.0.0" },
          { id: "missing", version: "1.0.0" },
        ],
        skills: [{ id: "evidence", version: "2.0.0" }],
        cluster: { id: "team", version: "1.0.0" },
      }),
      entity("other", "agent", "Other agent", {
        model: { id: "model", version: "1.0.0" },
      }),
      entity("model", "model", "Pinned model"),
      entity("model", "model", "New model", {}, "2.0.0"),
      entity("search", "tool", "Search tool"),
      entity("missing", "tool", "Wrong tool version", {}, "2.0.0"),
      entity("evidence", "skill", "Evidence skill", {}, "2.0.0"),
      entity("team", "cluster", "Research team", {
        coordinator: { id: "researcher", version: "1.0.0" },
      }),
    ],
    tasks: [
      {
        id: "task-1",
        title: "Collect evidence",
        description: "Verified task",
        status: "COMPLETED",
        owner: "aidash://test/agents/researcher@1.0.0",
        created_by: "human",
        workspace_id: "workspace-1",
        revision: 1,
        requirements: {},
      },
    ],
    runs: [
      {
        id: "run-1",
        workspace_id: "workspace-1",
        home_node: "aidash://test",
        agent_id: "researcher",
        agent_version: "1.0.0",
        task_id: "task-1",
        phase: "COMPLETED",
        control: "ACTIVE",
        step: 3,
        context: {},
        error: null,
      },
    ],
    workspaces: [
      {
        id: "workspace-1",
        title: "Research",
        goal: "Collect evidence",
        state: {},
        revision: 1,
        created_at: "2026-09-22T10:00:00Z",
      },
    ],
    artifacts: [],
    events: [],
    conversations: [],
    human_requests: [],
    installations: [],
    peers: [],
  };
}
async function setup(page: Page, locale = "en-US") {
  let data = fixture();
  const errors: string[] = [];
  const requests: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await installBearerDashboard(page, "fixture-token");
  await page.addInitScript((language) => {
    localStorage.setItem("aidash-locale", language);
  }, locale);
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    requests.push(path);
    if (path === "/api/events/stream") return route.abort();
    const json =
      path === "/api/session"
        ? { access: data.access, node_id: data.node.id }
        : path === "/api/state"
          ? data
          : path === "/api/mesh"
            ? { nodes: [], errors: [] }
            : path === "/api/discover"
              ? { agents: [], errors: [] }
              : path.startsWith("/api/runs/")
                ? { run: data.runs[0], invocations: [], memory: [] }
                : path === "/api/workspaces/workspace-1"
                  ? {
                      workspace: data.workspaces[0],
                      tasks: data.tasks,
                      artifacts: [],
                      messages: [],
                      events: [],
                    }
                  : [];
    await route.fulfill({ json });
  });
  await page.goto("/agents");
  await page
    .locator(".entity-card")
    .filter({
      has: page.getByRole("heading", { name: "Researcher", exact: true }),
    })
    .click();
  return {
    errors,
    requests,
    revoke: () => {
      data = { ...data, registry: [] };
    },
  };
}

test("shows typed version-pinned relationships and opens existing details", async ({
  page,
}) => {
  const { errors, requests } = await setup(page);
  const graph = page.getByRole("region", { name: "Agent relationship graph" });
  await expect(graph).toBeVisible();
  await expect(
    graph.locator('[data-graph-node][data-kind="model"]'),
  ).toHaveCount(1);
  await expect(graph.locator('[data-layer="configuration"]')).toHaveCount(6);
  await expect(graph.locator('[data-layer="runtime"]')).toHaveCount(2);
  await graph
    .getByRole("button", { name: "Relationship list", exact: true })
    .click();
  const list = graph.getByRole("table", { name: "Relationship list" });
  await expect(
    list.getByRole("rowheader", { name: "Pinned model · v1.0.0", exact: true }),
  ).toBeVisible();
  await expect(
    list.getByRole("rowheader", {
      name: /New model|Wrong tool version|Other agent/,
    }),
  ).toHaveCount(0);
  const missing = list
    .getByRole("row")
    .filter({ has: page.getByRole("rowheader", { name: /Tool · v1.0.0/ }) });
  await expect(
    missing.getByRole("button", { name: "Open details: Tool · v1.0.0" }),
  ).toBeDisabled();
  const model = list.getByRole("row").filter({
    has: page.getByRole("rowheader", {
      name: "Pinned model · v1.0.0",
      exact: true,
    }),
  });
  await model.getByRole("button", { name: "Expand neighbors" }).click();
  await expect(
    list.getByRole("rowheader", { name: "Other agent · v1.0.0", exact: true }),
  ).toBeVisible();
  await model
    .getByRole("button", { name: "Open details: Pinned model · v1.0.0" })
    .click();
  await expect(
    page
      .getByRole("dialog")
      .getByRole("heading", { name: "Pinned model", exact: true }),
  ).toBeVisible();
  expect(requests.some((path) => /knowledge|documents/.test(path))).toBe(false);
  expect(errors).toEqual([]);
});

test("filters runtime separately and follows a task from the list", async ({
  page,
}) => {
  const { errors } = await setup(page);
  const graph = page.getByRole("region", { name: "Agent relationship graph" });
  await graph
    .getByRole("checkbox", { name: "Runtime activity", exact: true })
    .uncheck();
  await expect(graph.locator('[data-layer="runtime"]')).toHaveCount(0);
  await expect(
    graph.locator('[data-kind="run"], [data-kind="task"]'),
  ).toHaveCount(0);
  await graph
    .getByRole("checkbox", { name: "Runtime activity", exact: true })
    .check();
  await graph.getByRole("checkbox", { name: "Model", exact: true }).uncheck();
  await expect(graph.locator('[data-kind="model"]')).toHaveCount(0);
  await expect(graph.locator('[data-kind="agent"]')).toHaveCount(1);
  await graph
    .getByRole("button", { name: "Relationship list", exact: true })
    .click();
  await graph
    .getByRole("button", {
      name: "Open details: Collect evidence",
      exact: true,
    })
    .click();
  await expect(
    page
      .getByRole("dialog")
      .getByRole("heading", { name: "Collect evidence", exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("supports keyboard viewport controls and Japanese list fallback", async ({
  page,
}) => {
  const { errors } = await setup(page, "ja-JP");
  const graph = page.getByRole("region", { name: "エージェント関係グラフ" });
  const camera = graph.locator("[data-camera]");
  const before = await camera.getAttribute("transform");
  await graph.getByRole("button", { name: "拡大", exact: true }).focus();
  await page.keyboard.press("Enter");
  await expect(camera).not.toHaveAttribute("transform", before!);
  await graph.getByRole("button", { name: "関係の一覧", exact: true }).click();
  await expect(graph.getByRole("table", { name: "関係の一覧" })).toBeVisible();
  await expect(
    graph.getByRole("button", { name: "詳細を開く: Collect evidence" }),
  ).toBeEnabled();
  expect(errors).toEqual([]);
});

test("removes graph and stale entity metadata after an authorized snapshot revokes the root", async ({
  page,
}) => {
  const { revoke, errors } = await setup(page);
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("region", { name: "Agent relationship graph" }),
  ).toBeVisible();
  revoke();
  await expect(dialog.getByRole("status")).toHaveText(
    "This agent version is not available in the current authorized view.",
    { timeout: 15000 },
  );
  await expect(
    dialog.getByRole("region", { name: "Agent relationship graph" }),
  ).toHaveCount(0);
  await expect(
    dialog.getByText("Researcher description", { exact: true }),
  ).toHaveCount(0);
  expect(errors).toEqual([]);
});

for (const locale of ["en-US", "ja-JP"]) {
  test(`SVG agent graph fits labels in the current viewport using the keyboard in ${locale}`, async ({
    page,
  }) => {
    if (locale === "ja-JP")
      await page.setViewportSize({ width: 390, height: 844 });
    const { errors, requests } = await setup(page, locale);
    const graph = page.locator(".agent-graph");
    const canvas = graph.locator(".agent-graph-canvas > svg");
    const fit = graph.getByRole("button", {
      name: locale === "ja-JP" ? "全体表示" : "Fit entire graph",
      exact: true,
    });
    await canvas.scrollIntoViewIfNeeded();
    await expect(fit).toBeEnabled();
    await page.evaluate(() => document.fonts.ready);
    const zoomOut = graph.getByRole("button", {
      name: locale === "ja-JP" ? "縮小" : "Zoom out",
      exact: true,
    });
    for (let i = 0; i < 4; i++) await zoomOut.click();
    const positions = await graph
      .locator("[data-graph-node]")
      .evaluateAll((nodes) =>
        nodes.map((node) => ({
          id: node.getAttribute("data-graph-node"),
          position: node.getAttribute("transform"),
        })),
      );
    const selected = await graph.locator(".agent-graph-selection").innerText();
    const right = graph.getByRole("button", {
      name: locale === "ja-JP" ? "右へ移動" : "Pan right",
      exact: true,
    });
    for (let i = 0; i < 15; i++) await right.click();
    // Dashboard polling continues independently of camera input.
    const backgroundPaths = new Set([
      "/api/state",
      "/api/session",
      "/api/mesh",
      "/api/discover",
      "/api/events/stream",
    ]);
    const resourceRequests = () =>
      requests.filter((path) => !backgroundPaths.has(path));
    const requestCount = resourceRequests().length;
    await fit.focus();
    await fit.press("Space");
    await expectSvgFitted(canvas);
    const after = await graph.locator("[data-camera]").evaluate((element) => {
      const box = element.getBoundingClientRect();
      return {
        left: box.left,
        top: box.top,
        right: box.right,
        bottom: box.bottom,
      };
    });
    await fit.press("Enter");
    await expectSvgFitted(canvas);
    const repeated = await graph
      .locator("[data-camera]")
      .evaluate((element) => {
        const box = element.getBoundingClientRect();
        return {
          left: box.left,
          top: box.top,
          right: box.right,
          bottom: box.bottom,
        };
      });
    for (const key of ["left", "top", "right", "bottom"] as const)
      expect(Math.abs(repeated[key] - after[key])).toBeLessThanOrEqual(1);
    expect(
      await graph.locator("[data-graph-node]").evaluateAll((nodes) =>
        nodes.map((node) => ({
          id: node.getAttribute("data-graph-node"),
          position: node.getAttribute("transform"),
        })),
      ),
    ).toEqual(positions);
    expect(await graph.locator(".agent-graph-selection").innerText()).toEqual(
      selected,
    );
    expect(resourceRequests()).toHaveLength(requestCount);
    await page.screenshot({
      path: `test-results/whole-graph-fit-svg-${locale}.png`,
    });
    expect(errors).toEqual([]);
  });
}
