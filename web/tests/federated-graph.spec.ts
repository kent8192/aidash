import { expect, test } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import { meshScene } from "./mesh-scene.mjs";

const firstPeer = "aidash://graph-b";
const secondPeer = "aidash://graph-c";
const workspace = "11111111-1111-4111-8111-111111111111";
const task = "22222222-2222-4222-8222-222222222222";
const key = (node: string, kind: string, id: string) =>
  JSON.stringify(["resource", node, kind, id]);
const entity = (node: string, kind: string, id: string, version: string) =>
  JSON.stringify(["entity", node, kind, id, version]);
const timestamp = new Date().toISOString();
const goal =
  "Full authorized Goal body.\nSecond paragraph with a unique search phrase.";
const projected = {
  node_id: firstPeer,
  generation: `sha256:${"a".repeat(64)}`,
  checked_at: timestamp,
  nodes: [
    {
      id: key(firstPeer, "workspace", workspace),
      node_id: firstPeer,
      kind: "workspace",
      name: { en: "B Workspace" },
      resource_id: workspace,
      version: null,
      workspace_id: workspace,
      status: null,
      goal_body: null,
      at: timestamp,
    },
    {
      id: key(firstPeer, "goal", workspace),
      node_id: firstPeer,
      kind: "goal",
      name: { en: "Full authorized Goal body." },
      resource_id: workspace,
      version: null,
      workspace_id: workspace,
      status: null,
      goal_body: goal,
      at: timestamp,
    },
    {
      id: key(firstPeer, "task", task),
      node_id: firstPeer,
      kind: "task",
      name: { en: "B Task" },
      resource_id: task,
      version: null,
      workspace_id: workspace,
      status: "RUNNING",
      goal_body: null,
      at: timestamp,
    },
    {
      id: entity(firstPeer, "agent", "worker", "1.0.0"),
      node_id: firstPeer,
      kind: "agent",
      name: { en: "B Agent" },
      resource_id: "worker",
      version: "1.0.0",
      workspace_id: null,
      status: null,
      goal_body: null,
      at: null,
    },
  ],
  edges: [
    {
      source: key(firstPeer, "workspace", workspace),
      target: key(firstPeer, "goal", workspace),
      relation: "goal",
      layer: "configuration",
    },
    {
      source: key(firstPeer, "goal", workspace),
      target: key(firstPeer, "task", task),
      relation: "contains",
      layer: "activity",
    },
  ],
  activity: [
    {
      kind: "task.started",
      at: timestamp,
      reference: key(firstPeer, "task", task),
    },
  ],
  next_cursor: null,
};

test("operator tenant input distinguishes invalid IDs from oversized resources", async ({
  page,
}) => {
  const scene = meshScene();
  await installBearerDashboard(page, "synthetic-federated-tenant");
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  let requests = 0;
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/federation/graph/peers")
      return route.fulfill({ json: [{ node_id: firstPeer }] });
    if (path === "/api/federation/graph") {
      requests++;
      return route.fulfill({
        status: 400,
        json: {
          error:
            requests === 1
              ? "identifiers require 1..256 bytes without whitespace or wildcards"
              : "remote graph resource exceeds page limit",
        },
      });
    }
    return route.fulfill({
      json:
        path === "/api/session"
          ? { access: scene.data.access, node_id: scene.data.node.id }
          : path === "/api/state"
            ? scene.data
            : path === "/api/discover"
              ? scene.discovery
              : path === "/api/mesh"
                ? { nodes: [], errors: [] }
                : [],
    });
  });
  await page.goto("/graph");
  const tenant = page.getByLabel("Peer tenant");
  await tenant.fill("acme tenant");
  await expect(tenant).toHaveAttribute("aria-invalid", "true");
  await expect(page.getByRole("alert")).toContainText("without spaces");
  await expect(
    page.getByRole("button", { name: "Expand node" }),
  ).toBeDisabled();
  expect(requests).toBe(0);
  await tenant.fill("acme");
  await page.getByRole("button", { name: "Expand node" }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "tenant ID" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Refresh node" }).click();
  await expect(
    page.getByText("An authorized resource exceeds the graph page limit."),
  ).toBeVisible();
});

test("subject expands only direct authorized peers and clears remote details on denial", async ({
  page,
}) => {
  const scene = meshScene();
  let denied = false;
  const requests: unknown[] = [];
  scene.data.access = { kind: "subject", tenant: "acme", subject: "ryota" };
  scene.data.tasks.push({
    ...scene.data.tasks[0],
    id: task,
    title: "A Task",
    description: "LOCAL-ONLY-DESCRIPTION",
  });
  await installBearerDashboard(page, "synthetic-federated-graph", {
    tenant: "acme",
    name: "ryota",
  });
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/federation/graph/peers")
      return route.fulfill({
        json: [{ node_id: firstPeer }, { node_id: secondPeer }],
      });
    if (path === "/api/federation/graph") {
      const body = route.request().postDataJSON();
      requests.push(body);
      if (denied && body.node_id === firstPeer)
        return route.fulfill({ status: 403, json: { error: "forbidden" } });
      return route.fulfill({
        json:
          body.node_id === firstPeer
            ? projected
            : {
                ...projected,
                node_id: secondPeer,
                nodes: [],
                edges: [],
                activity: [],
              },
      });
    }
    return route.fulfill({
      json:
        path === "/api/session"
          ? { access: scene.data.access, node_id: scene.data.node.id }
          : path === "/api/state"
            ? scene.data
            : path === "/api/discover"
              ? scene.discovery
              : path === "/api/mesh"
                ? { nodes: [], errors: [] }
                : [],
    });
  });
  await page.goto("/graph");
  await expect(page.getByRole("button", { name: "Expand node" })).toHaveCount(
    2,
  );
  await page.getByLabel("Graph perspective").selectOption("topology");
  await expect(
    page.locator(".mesh-node-label").filter({ hasText: firstPeer }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Expand node" }).first().click();
  await expect(
    page.locator(".mesh-node-label").filter({ hasText: "B Agent" }),
  ).toBeVisible();
  await expect
    .poll(() =>
      requests.some(
        (body) =>
          (body as { scope_workspace: string | null }).scope_workspace ===
          "product-lab",
      ),
    )
    .toBe(true);
  await page.getByLabel("Graph workspace").selectOption("");
  await expect
    .poll(() =>
      requests.some(
        (body) =>
          (body as { scope_workspace: string | null }).scope_workspace === null,
      ),
    )
    .toBe(true);
  for (const perspective of [
    "collaboration",
    "knowledge",
    "execution",
    "mesh",
  ]) {
    await page.getByLabel("Graph perspective").selectOption(perspective);
    await expect(
      page.locator(".mesh-node-label").filter({ hasText: "B Agent" }),
    ).toBeVisible();
  }
  await page.getByRole("button", { name: "Relationship list" }).click();
  const list = page.getByRole("table", { name: "Relationship list" });
  await expect(list).toContainText("B Workspace");
  await list
    .getByRole("button", { name: "Full authorized Goal body." })
    .click();
  const inspector = page.getByRole("complementary", { name: "Node details" });
  await expect(inspector).toContainText(goal);
  await page.getByRole("searchbox").fill("unique search phrase");
  await expect(list).toContainText("Full authorized Goal body.");
  await page.getByRole("searchbox").fill("");
  await list.getByRole("button", { name: "B Task" }).click();
  await expect(inspector).not.toContainText("LOCAL-ONLY-DESCRIPTION");
  await expect(inspector).toContainText("task.started");
  await page.getByRole("button", { name: "Expand node" }).click();
  await expect(
    page.getByText("No authorized graph resources in this view."),
  ).toBeVisible();
  denied = true;
  await page.getByRole("button", { name: "Refresh node" }).first().click();
  await expect(page.getByText("Remote graph access denied.")).toBeVisible();
  await expect(list).not.toContainText("B Task");
  await expect(inspector).not.toContainText("B Task");
  expect(
    requests.every(
      (body) => (body as { target_tenant: null }).target_tenant === null,
    ),
  ).toBe(true);
  expect(
    requests.every(
      (body) => (body as { node_id: string }).node_id !== "aidash://unknown",
    ),
  ).toBe(true);
  await page.reload();
  await expect(page.getByRole("button", { name: "Expand node" })).toHaveCount(
    2,
  );
  await expect(page.getByText("B Workspace")).toHaveCount(0);
});

test("compact Japanese view replaces page windows and clears outages and forged third-node data", async ({
  page,
}) => {
  const scene = meshScene();
  scene.data.access = { kind: "subject", tenant: "acme", subject: "ryota" };
  await page.setViewportSize({ width: 390, height: 844 });
  await installBearerDashboard(page, "synthetic-federated-graph-ja", {
    tenant: "acme",
    name: "ryota",
  });
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "ja-JP"),
  );
  let response:
    | "normal"
    | "outage"
    | "unsupported"
    | "oversized"
    | "forged"
    | "empty" = "normal";
  const agent = (name: string, node = firstPeer) => ({
    id: entity(node, "agent", name, "1.0.0"),
    node_id: node,
    kind: "agent",
    name: { ja: name },
    resource_id: name,
    version: "1.0.0",
    workspace_id: null,
    status: null,
    goal_body: null,
    at: null,
  });
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/federation/graph/peers")
      return route.fulfill({ json: [{ node_id: firstPeer }] });
    if (path === "/api/federation/graph") {
      if (response === "outage")
        return route.fulfill({ status: 503, json: { error: "unavailable" } });
      if (response === "unsupported")
        return route.fulfill({ status: 404, json: { error: "missing" } });
      if (response === "oversized")
        return route.fulfill({ status: 400, json: { error: "size limit" } });
      const next = Boolean(route.request().postDataJSON().cursor);
      return route.fulfill({
        json: {
          node_id: firstPeer,
          generation: `sha256:${"b".repeat(64)}`,
          checked_at: timestamp,
          nodes:
            response === "empty" && !next
              ? []
              : [
                  response === "forged"
                    ? agent("C非公開", secondPeer)
                    : agent(next ? "B次ページ" : "B最初のページ"),
                ],
          edges: [],
          activity: [],
          next_cursor: next ? null : "next",
        },
      });
    }
    return route.fulfill({
      json:
        path === "/api/session"
          ? { access: scene.data.access, node_id: scene.data.node.id }
          : path === "/api/state"
            ? scene.data
            : path === "/api/discover"
              ? scene.discovery
              : path === "/api/mesh"
                ? { nodes: [], errors: [] }
                : [],
    });
  });
  await page.goto("/graph");
  await page.getByRole("button", { name: "ノードを展開" }).click();
  await expect(
    page.locator(".mesh-node-label").filter({ hasText: "B最初のページ" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "認可済みの次ページを表示" }).click();
  await expect(
    page.locator(".mesh-node-label").filter({ hasText: "B次ページ" }),
  ).toBeVisible();
  await expect(page.getByText("B最初のページ")).toHaveCount(0);
  response = "outage";
  await page.getByRole("button", { name: "ノードを更新" }).click();
  await expect(page.getByText("接続先グラフを利用できません。")).toBeVisible();
  await expect(page.getByText("B次ページ")).toHaveCount(0);
  response = "unsupported";
  await page.getByRole("button", { name: "ノードを更新" }).click();
  await expect(
    page.getByText("このノードはグラフ投影に対応していません。"),
  ).toBeVisible();
  response = "oversized";
  await page.getByRole("button", { name: "ノードを更新" }).click();
  await expect(
    page.getByText("認可済みリソースがグラフのページ上限を超えています。"),
  ).toBeVisible();
  response = "forged";
  await page.getByRole("button", { name: "ノードを更新" }).click();
  await expect(page.getByText("C非公開")).toHaveCount(0);
  response = "normal";
  await page.getByRole("button", { name: "ノードを折りたたむ" }).click();
  await page.getByRole("button", { name: "ノードを展開" }).click();
  await expect(
    page.locator(".mesh-node-label").filter({ hasText: "B最初のページ" }),
  ).toBeVisible();
  response = "empty";
  await page.getByRole("button", { name: "ノードを更新" }).click();
  await expect(
    page.getByText(
      "このページにリソースはありません。次のページを表示できます。",
    ),
  ).toBeVisible();
  await page.getByRole("button", { name: "認可済みの次ページを表示" }).click();
  await expect(
    page.locator(".mesh-node-label").filter({ hasText: "B次ページ" }),
  ).toBeVisible();
});
