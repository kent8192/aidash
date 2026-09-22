import { expect, test, type Page } from "@playwright/test";

function fixture() {
  const node = {
    id: "aidash://home",
    endpoint: "http://localhost",
    protocol_version: "0.1",
    capabilities: [],
    clusters: [],
  };
  const component = (id: string, kind: string, name: string, config = {}) => ({
    id,
    kind,
    version: "1.0.0",
    name: { en: name },
    description: { en: name },
    capabilities: [],
    tags: [],
    languages: ["en"],
    skills: [],
    schema: {},
    config,
  });
  const workspaces = [
    {
      id: "workspace-one",
      title: "Research",
      goal: "Find reliable evidence",
      state: {},
      revision: 1,
      created_at: "2026-09-22T10:00:00Z",
    },
    {
      id: "workspace-two",
      title: "Planning",
      goal: "Plan the next release",
      state: {},
      revision: 1,
      created_at: "2026-09-22T11:00:00Z",
    },
  ];
  const tasks = workspaces.map((workspace, index) => ({
    id: `task-${index}`,
    workspace_id: workspace.id,
    title: index ? "Plan release" : "Collect evidence",
    description: "Use recorded sources",
    status: "RUNNING",
    owner: "aidash://home/agents/researcher@1.0.0",
    created_by: "human",
    requirements: {},
    dependencies: [],
    parent_id: null,
    revision: 2,
    created_at: workspace.created_at,
  }));
  const runs = tasks.map((task, index) => ({
    id: `run-${index}`,
    task_id: task.id,
    workspace_id: task.workspace_id,
    home_node: node.id,
    agent_id: "researcher",
    agent_version: "1.0.0",
    phase: "THINKING",
    control: "ACTIVE",
    context: {},
    pending: {},
    step: 1,
    revision: 1,
    error: null,
    lease_owner: null,
    lease_until: null,
    updated_at: task.created_at,
  }));
  return {
    access: { kind: "operator" },
    node,
    workspaces,
    tasks,
    runs,
    registry: [
      component("researcher", "agent", "Researcher", {
        model: { id: "model", version: "1.0.0" },
      }),
      component("model", "model", "Evidence model"),
    ],
    conversations: [],
    events: [],
    peers: [],
    artifacts: [],
    installations: [],
    human_requests: [
      {
        id: "question-one",
        workspace_id: "workspace-one",
        run_id: "run-0",
        kind: "INFORMATION_REQUEST",
        prompt: "Which market should I examine?",
        response: null as unknown,
        answered_by: null,
        created_at: "2026-09-22T10:10:00Z",
      },
    ],
  };
}

async function setup(
  page: Page,
  options: { failFirstMessage?: boolean; subject?: boolean } = {},
) {
  let data = fixture();
  const messages: Record<
    string,
    {
      id: string;
      workspace_id: string;
      sender: string;
      content: string;
      idempotency_key: string | null;
      created_at: string;
    }[]
  > = {
    "workspace-one": [
      {
        id: "message-one",
        workspace_id: "workspace-one",
        sender: "aidash://home/agents/researcher@1.0.0",
        content: "Evidence is ready to review.",
        idempotency_key: null,
        created_at: "2026-09-22T10:11:00Z",
      },
    ],
    "workspace-two": [],
  };
  const submissions: { path: string; body: Record<string, unknown> }[] = [];
  const errors: string[] = [];
  let denyHistory = false;
  let failures = options.failFirstMessage ? 1 : 0;
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() => {
    sessionStorage.setItem("aidash-token", "fixture");
    localStorage.setItem("aidash-locale", "en-US");
  });
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    const access = options.subject
      ? { kind: "subject", tenant: "acme", subject: "alice" }
      : data.access;
    if (path === "/api/session")
      return route.fulfill({ json: { access, node_id: data.node.id } });
    if (path === "/api/state")
      return route.fulfill({ json: { ...data, access } });
    if (path === "/api/mesh")
      return route.fulfill({ json: { nodes: [], errors: [] } });
    if (path === "/api/discover")
      return route.fulfill({
        json: {
          agents: data.registry
            .filter((entity) => entity.kind === "agent")
            .map((entity) => ({ node_id: data.node.id, entity })),
          errors: [],
        },
      });
    if (request.method() === "POST") {
      const body = request.postDataJSON();
      submissions.push({ path, body });
      if (path === "/api/workspaces") {
        const workspace = {
          ...data.workspaces[0],
          id: "prepared",
          title: body.title,
          goal: body.goal,
        };
        data.workspaces.push(workspace);
        messages.prepared = [];
        return route.fulfill({ json: workspace });
      }
      if (/^\/api\/workspaces\/[^/]+\/messages$/.test(path)) {
        if (failures-- > 0)
          return route.fulfill({
            status: 503,
            json: { error: "temporarily unavailable" },
          });
        const id = path.split("/")[3];
        messages[id].push({
          id: String(body.idempotency_key),
          workspace_id: id,
          sender: "alice",
          content: body.content,
          idempotency_key: body.idempotency_key,
          created_at: "2026-09-22T12:00:00Z",
        });
        return route.fulfill({ json: { sent: true } });
      }
      if (path === "/api/runs/run-0/control") {
        data.runs[0].control = body.action === "pause" ? "PAUSED" : "ACTIVE";
        return route.fulfill({ json: data.runs[0] });
      }
      if (path === "/api/human-requests/question-one/answer") {
        data.human_requests[0].response = body;
        return route.fulfill({ json: data.human_requests[0] });
      }
    }
    const workspace = data.workspaces.find(
      (value) => path === `/api/workspaces/${value.id}`,
    );
    if (workspace) {
      if (denyHistory)
        return route.fulfill({ status: 403, json: { error: "forbidden" } });
      return route.fulfill({
        json: {
          workspace,
          messages: messages[workspace.id],
          tasks: data.tasks.filter(
            (task) => task.workspace_id === workspace.id,
          ),
          artifacts: [],
          events: [],
        },
      });
    }
    const run = data.runs.find((value) => path === `/api/runs/${value.id}`);
    if (run)
      return route.fulfill({ json: { run, invocations: [], memory: {} } });
    if (path === "/api/marketplace") return route.fulfill({ json: [] });
    return route.fulfill({
      status: 404,
      json: { error: `Unconfigured fixture: ${path}` },
    });
  });
  return {
    submissions,
    errors,
    revokeHistory: () => {
      denyHistory = true;
    },
    revokeAgents: () => {
      data = { ...data, registry: [] };
    },
  };
}

test("collaboration is the landing view, with two primary destinations and secondary settings", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "# Research", exact: true }),
  ).toBeVisible();
  const primary = page.locator(".collab-rail nav").first();
  await expect(primary.getByRole("link")).toHaveCount(2);
  await expect(
    primary.getByRole("link", { name: "Collaboration", exact: true }),
  ).toBeVisible();
  await expect(
    primary.getByRole("link", { name: "Graph View", exact: true }),
  ).toBeVisible();
  await expect(
    page.locator(".collab-secondary").getByRole("link", { name: "Settings" }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("failed message keeps its draft and idempotency key on retry", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { failFirstMessage: true });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("textbox", { name: "Message", exact: true })
    .fill("Please check the source.");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Your text has been kept",
  );
  await expect(
    page.getByRole("textbox", { name: "Message", exact: true }),
  ).toHaveValue("Please check the source.");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(
    page.getByRole("textbox", { name: "Message", exact: true }),
  ).toHaveValue("");
  await expect(
    page.getByText("Please check the source.", { exact: true }),
  ).toBeVisible();
  const messages = submissions.filter((item) =>
    item.path.endsWith("/messages"),
  );
  expect(messages).toHaveLength(2);
  expect(messages[0].body.idempotency_key).toBe(
    messages[1].body.idempotency_key,
  );
  expect(errors).toEqual([]);
});

test("preparing a channel does not invoke agent execution", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page);
  await page.goto("/");
  await page
    .getByRole("button", { name: "Prepare a channel", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByRole("textbox", { name: "Title", exact: true })
    .fill("Draft research");
  await dialog
    .getByRole("textbox", { name: "Goal", exact: true })
    .fill("Discuss sources before starting");
  await dialog.getByRole("button", { name: "Create", exact: true }).click();
  await expect(page).toHaveURL(/channel=prepared/);
  await expect(
    page.getByRole("heading", { name: "# Draft research", exact: true }),
  ).toBeVisible();
  expect(submissions.map((item) => item.path)).toEqual(["/api/workspaces"]);
  expect(errors).toEqual([]);
});

test("graph renders with Cytoscape and offers current related channel first", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/collaboration?channel=workspace-two");
  await page
    .getByRole("button", { name: "View in graph", exact: true })
    .click();
  await expect(page).toHaveURL(/\/graph\?channel=workspace-two/);
  await expect(page.locator(".collab-cytoscape canvas").first()).toBeVisible();
  const related = page.locator(".collab-selection .collab-channel-link");
  await expect(related).toHaveCount(2);
  await expect(related.first()).toHaveText("# Planning");
  await related.filter({ hasText: "Research" }).click();
  await expect(page).toHaveURL(/\/collaboration\?channel=workspace-one/);
  await expect(
    page.getByText("Evidence is ready to review.", { exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("channel intervention answers a human request without settings navigation", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page);
  await page.goto("/");
  await page
    .getByRole("button")
    .filter({ hasText: "Which market should I examine?" })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByRole("textbox", { name: "Answer", exact: true })
    .fill("Japan");
  await dialog.getByRole("button", { name: "Send", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(
    submissions.some(
      (item) => item.path === "/api/human-requests/question-one/answer",
    ),
  ).toBe(true);
  expect(errors).toEqual([]);
});

test("revoked history is hidden rather than retaining a stale conversation", async ({
  page,
}) => {
  const { revokeHistory } = await setup(page);
  await page.goto("/collaboration?channel=workspace-one");
  await expect(
    page.getByText("Evidence is ready to review.", { exact: true }),
  ).toBeVisible();
  revokeHistory();
  await expect(
    page.getByText("Evidence is ready to review.", { exact: true }),
  ).toHaveCount(0);
  await expect(page.getByRole("alert")).toContainText("unavailable");
});

test("an inaccessible explicit channel is not silently replaced with another channel", async ({
  page,
}) => {
  await setup(page);
  await page.goto("/collaboration?channel=unavailable");
  await expect(
    page.getByRole("heading", { name: "# Research", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", {
      name: "This resource is unavailable under your current access.",
      exact: true,
    }),
  ).toBeVisible();
});

test("legacy management links are routed into secondary settings", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/agents");
  await expect(page).toHaveURL(/\/settings\?.*view=agents/);
  await expect(
    page.getByRole("heading", { name: "Researcher", exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("mobile conversation and channel switching do not require horizontal scrolling", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/collaboration?channel=workspace-one");
  await expect(
    page.getByRole("textbox", { name: "Message", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Channels", exact: true }).click();
  await page.getByRole("link", { name: "# Planning", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "# Planning", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  expect(errors).toEqual([]);
});
