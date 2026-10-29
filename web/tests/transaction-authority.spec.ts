import { test, expect } from "@playwright/test";
import { setup } from "./collaboration-fixture";
import { defaultNode } from "./registry-fixture";
import type { TransactionManifest } from "../src/generated/models";

// UI contract evidence; the Rust suites exercise the same authority boundaries
// against independent PostgreSQL databases and authenticated peer HTTP.
test("subject transaction controls survive a barrier and discard revoked detail", async ({
  page,
}) => {
  await setup(page, { subject: true });
  let denied = false;
  let barrier = false;
  const deploymentAdministratorRequests: string[] = [];
  const transaction = {
    id: "50000000-0000-0000-0000-000000000040",
    digest: "a".repeat(64),
    decision: null,
    visible: false,
    complete: false,
    last_error: null,
    manifest: {
      id: "50000000-0000-0000-0000-000000000040",
      coordinator: "aidash://home",
      isolation: "serializable",
      deadline: "2099-01-01T00:00:00Z",
      participants: [{ node_id: "aidash://home", mutations: [] }],
    },
  };
  await page.route("**/api/transactions**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/participants") || path.endsWith("/trust"))
      deploymentAdministratorRequests.push(path);
    if (path === "/api/transactions")
      return route.fulfill({ json: denied ? [] : [transaction] });
    return denied
      ? route.fulfill({ status: 403, json: { error: "permission revoked" } })
      : route.fulfill({ json: { transaction, participants: [], history: [] } });
  });
  await page.route("**/api/state", (route) =>
    barrier
      ? route.fulfill({
          status: 503,
          headers: { "x-aidash-transaction-pending": "1" },
          json: { error: "atomic transaction visibility pending" },
        })
      : route.fallback(),
  );
  await page.goto("/transactions");
  await expect(
    page.getByRole("heading", { name: "Transactions", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Peer transaction trust", exact: true }),
  ).toHaveCount(0);
  await page
    .getByRole("button", { name: "Create transaction", exact: true })
    .click();
  await expect(
    page.getByRole("dialog").locator("option[value=registry_register]"),
  ).toHaveCount(0);
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Close", exact: true })
    .click();
  barrier = true;
  await page.reload();
  await expect(
    page.getByRole("heading", { name: "Transactions", exact: true }),
  ).toBeVisible();
  await page.locator("button.transaction-id").click();
  await expect(page.locator(".transaction-details")).toBeVisible();
  await expect(
    page.getByRole("button", {
      name: "Abort undecided transaction",
      exact: true,
    }),
  ).toBeVisible();
  denied = true;
  await expect(page.locator(".transaction-details")).toHaveCount(0);
  await expect(page.getByRole("dialog")).toContainText(
    "Your current permissions do not allow this operation.",
  );
  await expect(
    page.getByRole("button", {
      name: "Abort undecided transaction",
      exact: true,
    }),
  ).toHaveCount(0);
  expect(deploymentAdministratorRequests).toEqual([]);
});

test("authority controls distinguish pending revocation while ordinary data is unavailable", async ({
  page,
}) => {
  await setup(page, { administrator: true });
  let pending = true;
  await page.route("**/api/state", (route) =>
    route.fulfill({
      status: 503,
      json: { error: "atomic transaction visibility pending" },
    }),
  );
  await page.route("**/api/authorization/acme**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/transaction-revocations"))
      return route.fulfill({ json: pending ? ["pending-transaction"] : [] });
    if (path === "/api/authorization/acme")
      return route.fulfill({
        json: {
          revision: 1,
          bundle: {
            tenant: "acme",
            subjects: { alice: { kind: "user" } },
            groups: {},
            roles: {},
            policies: [],
          },
        },
      });
    return route.fulfill({ json: [] });
  });
  await page.goto("/");
  // Navigate within the SPA: Vite's development /auth proxy also matches the
  // legacy /authorization path, whereas the deployed router serves this page.
  await page.evaluate(() => {
    history.pushState(null, "", "/authorization");
    window.dispatchEvent(new PopStateEvent("popstate"));
  });
  await page
    .locator(".authorization-page")
    .getByLabel("Tenant", { exact: true })
    .fill("acme");
  await page
    .locator(".authorization-page")
    .getByRole("button", { name: "Open", exact: true })
    .click();
  const notice = page.getByText("New admissions are denied after revocation.", {
    exact: false,
  });
  await expect(notice).toBeVisible();
  pending = false;
  await expect(notice).toHaveCount(0);
});

test("subjects visiting authorization see their own API tokens without Deployment Administrator controls", async ({
  page,
}) => {
  await setup(page, { subject: true });
  const deploymentAdministratorRequests: string[] = [];
  await page.route("**/api/authorization/**", (route) => {
    const url = new URL(route.request().url());
    // A member reads the API tokens it issued for itself (#363).
    if (url.searchParams.get("mine") === "true")
      return route.fulfill({ json: [] });
    deploymentAdministratorRequests.push(url.toString());
    return route.fulfill({ status: 403, json: { error: "forbidden" } });
  });
  await page.goto("/");
  await page.evaluate(() => {
    history.pushState(null, "", "/authorization");
    window.dispatchEvent(new PopStateEvent("popstate"));
  });
  await expect(page.getByText("Your API tokens")).toBeVisible();
  await expect(
    page.locator('.collab-settings-select option[value="authorization"]'),
  ).toHaveCount(0);
  await expect(page.locator(".authorization-page")).toHaveCount(0);
  expect(deploymentAdministratorRequests).toEqual([]);
});

test("subject transaction creation waits for its selected Node", async ({
  page,
}) => {
  await setup(page, { subject: true });
  let releaseNodes!: () => void;
  const nodesReady = new Promise<void>((resolve) => {
    releaseNodes = resolve;
  });
  await page.route("**/api/tenants/acme/nodes", async (route) => {
    await nodesReady;
    await route.fallback();
  });
  await page.goto("/transactions");
  const create = page.getByRole("button", {
    name: "Create transaction",
    exact: true,
  });
  try {
    await expect(create).toBeDisabled();
  } finally {
    releaseNodes();
  }
  await expect(create).toBeEnabled();
  await create.click();
  const node = page.getByRole("dialog").getByLabel("Node", { exact: true });
  await expect(node).toBeDisabled();
  await expect(node).toHaveValue(`aidash://home/${defaultNode.name}`);
});

test("subjects compose transactions in their selected local Node", async ({
  page,
}) => {
  await setup(page);
  const workspaces = ["first", "second"].map((node, index) => ({
    id: `workspace-${index + 1}`,
    node,
    title: `${node} workspace`,
    goal: "Keep changes in one Node",
    state: {},
    revision: index + 1,
    created_at: "2026-10-22T00:00:00Z",
  }));
  const tasks = workspaces.map((workspace, index) => ({
    id: `task-${index + 1}`,
    workspace_id: workspace.id,
    title: `${workspace.node} task`,
    description: "Stay in the selected Node",
    status: "RUNNING",
    owner: null,
    created_by: "human",
    requirements: {},
    dependencies: [],
    parent_id: null,
    revision: index + 2,
    created_at: workspace.created_at,
  }));
  const runs = tasks.map((task, index) => ({
    id: `run-${index + 1}`,
    node: workspaces[index].node,
    workspace_id: task.workspace_id,
    task_id: task.id,
    home_node: `aidash://home/${workspaces[index].node}`,
    agent_id: "researcher",
    agent_version: "1.0.0",
    phase: "THINKING",
    control: "ACTIVE",
    state: {},
    recovery: { retry: null, lease_recovered: false },
    step: 1,
    revision: index + 3,
    error: null,
    updated_at: workspaces[index].created_at,
  }));
  await page.route("**/api/state", (route) =>
    route.fulfill({
      json: {
        access: { kind: "subject", tenant: "acme", subject: "alice" },
        deployment: {
          id: "aidash://home",
          endpoint: "http://localhost",
          protocol_version: "0.4",
          key: null,
        },
        workspaces,
        tasks,
        runs: [
          ...runs,
          { ...runs[1], id: "remote-run", home_node: "aidash://remote/lab" },
        ],
        registry: [],
        conversations: [],
        human_requests: [],
        events: [],
        artifacts: [],
      },
    }),
  );
  await page.route("**/api/tenants/acme/nodes", (route) =>
    route.fulfill({
      json: [
        {
          ...defaultNode,
          name: "first",
          address: "aidash://home/first",
          default: true,
        },
        {
          ...defaultNode,
          name: "second",
          address: "aidash://home/second",
          default: false,
        },
      ],
    }),
  );
  let submitted: TransactionManifest | undefined;
  await page.route("**/api/transactions**", (route) => {
    if (route.request().method() === "POST") {
      submitted = route.request().postDataJSON();
      return route.fulfill({ status: 403, json: { error: "forbidden" } });
    }
    return route.fulfill({ json: [] });
  });
  await page.addInitScript(() =>
    localStorage.setItem("aidash-node:acme", "second"),
  );
  await page.goto("/transactions");
  await page
    .getByRole("button", { name: "Create transaction", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  const node = dialog.getByLabel("Node", { exact: true });
  await expect(node).toBeDisabled();
  await expect(node).toHaveValue("aidash://home/second");
  const workspace = dialog.getByLabel("Workspace", { exact: true });
  await expect(workspace.locator('option[value="workspace-1"]')).toHaveCount(0);
  await workspace.selectOption("workspace-2");
  await expect(dialog.getByLabel("Revision", { exact: true })).toHaveValue("2");
  await expect(
    dialog.getByRole("button", { name: "Add participant", exact: true }),
  ).toBeEnabled();
  const operation = dialog.getByLabel("Operation", { exact: true });
  await operation.selectOption("complete_task");
  const task = dialog.getByLabel("Task", { exact: true });
  await expect(task.locator('option[value="task-1"]')).toHaveCount(0);
  await task.selectOption("task-2");
  await operation.selectOption("finish_run");
  const run = dialog.getByLabel("Execution", { exact: true });
  await expect(run.locator('option[value="run-1"]')).toHaveCount(0);
  await expect(run.locator('option[value="remote-run"]')).toHaveCount(0);
  await run.selectOption("run-2");
  await expect(dialog.getByLabel("Revision", { exact: true })).toHaveValue("4");

  await dialog
    .getByRole("button", { name: "Review changes", exact: true })
    .click();
  await dialog
    .getByRole("button", { name: "Submit transaction", exact: true })
    .click();
  await expect(dialog.getByRole("alert")).toBeVisible();
  expect(submitted?.coordinator).toBe("aidash://home/second");
  expect(submitted?.participants).toEqual([
    {
      node_id: "aidash://home/second",
      mutations: [
        {
          kind: "finish_run",
          run_id: "run-2",
          task_id: "task-2",
          expected_revision: 4,
        },
      ],
    },
  ]);
});

test("subjects submit from their selected Node with explicit remote participant IDs", async ({
  page,
}) => {
  await setup(page, { subject: true });
  let submitted: TransactionManifest | undefined;
  await page.route("**/api/transactions**", (route) => {
    if (route.request().method() === "POST") {
      submitted = route.request().postDataJSON();
      return route.fulfill({ status: 403, json: { error: "forbidden" } });
    }
    return route.fulfill({ json: [] });
  });
  await page.goto("/transactions");
  await page
    .getByRole("button", { name: "Create transaction", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByLabel("Workspace", { exact: true })
    .selectOption("workspace-one");
  await dialog
    .getByRole("button", { name: "Add participant", exact: true })
    .click();
  await dialog
    .getByLabel("Node", { exact: true })
    .nth(1)
    .fill("aidash://remote/lab");
  await dialog
    .getByRole("button", { name: "Review changes", exact: true })
    .click();
  await expect(dialog).toContainText("aidash://remote/lab");
  await dialog
    .getByRole("button", { name: "Submit transaction", exact: true })
    .click();
  await expect(dialog.getByRole("alert")).toBeVisible();
  expect(submitted?.coordinator).toBe(`aidash://home/${defaultNode.name}`);
  expect(
    submitted?.participants.map((participant) => participant.node_id),
  ).toEqual([`aidash://home/${defaultNode.name}`, "aidash://remote/lab"]);
});
