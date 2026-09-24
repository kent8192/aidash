/** Synthetic, deterministic graph records for tests and local visual QA only. */
export function meshScene(now = Date.now()) {
  const time = (minutes) => new Date(now - minutes * 60_000).toISOString();
  const home = "aidash://product-lab";
  const peer = "aidash://acme-corp";
  const ref = (id) => ({ id, version: "1.0.0" });
  const principal = (id, node = home) => `${node}/agents/${id}@1.0.0`;
  const entry = (id, kind, name, config = {}, description = name) => ({
    ...ref(id),
    kind,
    name: { en: name },
    description: { en: description },
    config,
    capabilities: [],
    languages: ["en"],
    skills: [],
    tags: [],
    schema: {},
  });
  const agent = (id, name, tools, description) =>
    entry(
      id,
      "agent",
      name,
      {
        model: ref("reasoning-model"),
        cluster: ref("local-cluster"),
        tools: tools.map(ref),
        skills: [ref("planning")],
      },
      description,
    );
  const registry = [
    agent(
      "planner",
      "Planner Agent",
      ["search"],
      "Plans · Decomposes · Coordinates",
    ),
    agent(
      "researcher",
      "Research Agent",
      ["search", "notes"],
      "Searches · Synthesizes",
    ),
    agent("builder", "Builder Agent", ["github"], "Implements · Tests"),
    agent("observer", "Observer Agent", [], "Observes · Alerts"),
    agent("reviewer", "Reviewer Agent", ["github"], "Reviews · Validates"),
    entry("local-cluster", "cluster", "Local Agent Cluster", {
      coordinator: ref("planner"),
    }),
    entry("search", "tool", "Web Search"),
    entry("github", "tool", "GitHub"),
    entry("notes", "tool", "Notes"),
    entry("reasoning-model", "model", "Reasoning model"),
    entry("planning", "skill", "Task planning"),
  ];
  registry[0].capabilities = [
    "Task Planning",
    "Decomposition",
    "Agent Coordination",
    "Context Synthesis",
    "Risk Analysis",
  ];
  const workspaces = [
    {
      id: "product-lab",
      title: "Product Lab",
      goal: "Launch Aidash v0.1",
      state: {},
      revision: 1,
      created_at: time(120),
    },
  ];
  const tasks = [
    {
      id: "t-101",
      title: "Research & Plan",
      status: "COMPLETED",
      owner: principal("researcher"),
      created_by: "Ryota",
      dependencies: [],
      parent_id: null,
    },
    {
      id: "t-102",
      title: "Build Prototype",
      status: "RUNNING",
      owner: principal("builder"),
      created_by: principal("planner"),
      dependencies: ["t-101"],
      parent_id: "t-101",
    },
    {
      id: "t-103",
      title: "Run Evaluation",
      status: "BLOCKED",
      owner: principal("reviewer"),
      created_by: principal("planner"),
      dependencies: ["t-102"],
      parent_id: "t-101",
    },
  ].map((t, i) => ({
    ...t,
    workspace_id: "product-lab",
    description: `${t.title} for the first Aidash release.`,
    requirements: {},
    revision: 1,
    created_at: time(90 - i * 20),
  }));
  const runs = ["planner", "researcher", "builder", "observer", "reviewer"].map(
    (id, i) => ({
      id: `run-${id}`,
      task_id: tasks[[0, 0, 1, 1, 2][i]].id,
      workspace_id: "product-lab",
      home_node: home,
      agent_id: id,
      agent_version: "1.0.0",
      phase: i === 4 ? "WAITING" : i === 1 ? "COMPLETED" : "THINKING",
      control: "ACTIVE",
      step: i + 1,
      revision: 1,
      pending: {},
      context: {},
      error: null,
      lease_owner: null,
      lease_until: null,
      updated_at: time(i + 1),
    }),
  );
  const artifacts = ["PRD", "Prototype", "Evaluation Report"].map(
    (name, i) => ({
      id: `artifact-${i}`,
      name,
      kind: i === 1 ? "code" : "text",
      content: `Sample ${name} content for local visual verification.`,
      workspace_id: "product-lab",
      task_id: tasks[i].id,
      created_by: principal(["researcher", "builder", "reviewer"][i]),
      idempotency_key: `artifact-${i}`,
      created_at: time(20 - i * 5),
    }),
  );
  const events = Array.from({ length: 18 }, (_, i) => ({
    id: `event-${i}`,
    sequence: i,
    node_id: home,
    workspace_id: "product-lab",
    kind: [
      "task.created",
      "task.claimed",
      "run.started",
      "artifact.created",
      "task.completed",
      "run.waiting",
    ][i % 6],
    data: { task_id: tasks[i % 3].id, run_id: runs[i % 5].id },
    created_at: time(3 + i * 4),
  }));
  const conversations = [
    {
      id: "conversation-strategy",
      workspace_id: "product-lab",
      target: "planner@1.0.0",
      target_kind: "agent",
      created_by: "Ryota",
      created_at: time(90),
    },
  ];
  const peers = [
    {
      node_id: peer,
      endpoint: "https://example.invalid",
      credential_env: "TEST_ONLY_NOT_A_SECRET",
      protocol_version: "0.1",
      enabled: true,
    },
  ];
  const discovery = {
    agents: [
      entry("analyst", "agent", "Data Analyst"),
      entry("expert", "agent", "Domain Expert"),
    ].map((entity) => ({ node_id: peer, entity })),
    errors: [],
  };
  return {
    data: {
      access: { kind: "operator" },
      node: {
        id: home,
        endpoint: "http://localhost",
        protocol_version: "0.1",
        capabilities: [],
        clusters: [],
      },
      registry,
      workspaces,
      tasks,
      runs,
      artifacts,
      events,
      conversations,
      peers,
      installations: [],
      human_requests: [],
    },
    discovery,
  };
}
