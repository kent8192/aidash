import type { Artifact } from "../src/generated/models";
import type { Unconfirmed } from "../src/unconfirmed";
import type { Page } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import { defaultNode, registryVersion } from "./registry-fixture";

export function fixture(reference = false) {
  const deployment = {
    id: "aidash://home",
    endpoint: "http://localhost",
    protocol_version: "0.2",
    key: null,
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
    config:
      kind === "agent"
        ? { schema_version: 1, bindings: [], remove_default: [], ...config }
        : config,
  });
  const threads = [
    {
      id: "thread-one",
      scratch_space_id: "11111111-1111-4111-8111-111111111111",
      node: defaultNode.name,
      title: "Research",
      goal: "Find reliable evidence",
      state: {},
      revision: 1,
      created_at: "2026-09-22T10:00:00Z",
    },
    {
      id: "thread-two",
      scratch_space_id: "22222222-2222-4222-8222-222222222222",
      node: defaultNode.name,
      title: "Planning",
      goal: "Plan the next release",
      state: {},
      revision: 1,
      created_at: "2026-09-22T11:00:00Z",
    },
  ];
  const tasks = threads.map((thread, index) => ({
    id: `task-${index}`,
    thread_id: thread.id,
    title: index ? "Plan release" : "Collect evidence",
    description: "Use recorded sources",
    status: "RUNNING",
    owner: "aidash://home/agents/researcher@1.0.0",
    created_by: "human",
    requirements: {},
    dependencies: [] as string[],
    parent_id: null,
    revision: 2,
    created_at: thread.created_at,
  }));
  const runs = tasks.map((task, index) => ({
    id: `run-${index}`,
    node: defaultNode.name,
    task_id: task.id,
    thread_id: task.thread_id,
    home_node: `${deployment.id}/${defaultNode.name}`,
    agent_id: "researcher",
    agent_version: "1.0.0",
    phase: "THINKING",
    control: "ACTIVE",
    state_version: 1,
    context: {} as unknown,
    state: {
      phase: "THINKING",
      data: {
        force_thread_read_compaction: false,
        deferred_thread_read: null,
        deferred_skill_read: null,
        deferred_workspace_observation: null,
        selected_media: [],
        media_intake_through_seq: null,
        deferred_run_message_reads: [],
      },
    } as unknown,
    recovery: {
      retry: null,
      semantic_reason: null as string | null,
    },
    state_error: null as string | null,
    observed_input_seq: 0,
    ledger_worker_ready: true,
    step: 1,
    revision: 1,
    error: null,
    lease_owner: null,
    lease_until: null,
    updated_at: task.created_at,
  }));
  const data = {
    access: { kind: "deployment_administrator" },
    deployment,
    threads,
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
    artifacts: [] as Artifact[],
    unconfirmed: [] as Unconfirmed[],
    installations: [],
    next_cursors: {
      threads: null,
      tasks: null,
      runs: null,
      artifacts: null,
      human_requests: null,
      conversations: null,
    },
    human_requests: [
      {
        id: "question-one",
        thread_id: "thread-one",
        run_id: "run-0",
        kind: "INFORMATION_REQUEST",
        prompt: "Which market should I examine?",
        response: null as unknown,
        answered_by: null as string | null,
        answer_by: null as string | null,
        created_at: "2026-09-22T10:10:00Z",
      },
    ],
  };
  if (reference) {
    data.threads = [
      {
        ...threads[0],
        title: "v0.1-release",
        goal: "異なるノードのエージェントで、登録・協調・実行・復旧を一周させる。",
      },
      {
        ...threads[1],
        title: "docs-improvement",
        goal: "ドキュメントを改善する",
      },
      {
        ...threads[1],
        id: "thread-three",
        title: "performance-test",
        goal: "性能を検証する",
      },
      {
        ...threads[1],
        id: "thread-four",
        title: "website-launch",
        goal: "ウェブサイトを公開する",
      },
    ];
    const agents = ["researcher", "coder", "verifier", "publisher"];
    const names = ["Researcher", "Coder", "Verifier", "Publisher"];
    data.registry = [
      ...agents.map((id, index) =>
        component(id, "agent", names[index], {
          model: { id: "model", version: "1.0.0" },
        }),
      ),
      component("model", "model", "Evidence model"),
    ];
    data.tasks = [
      "仕様・変更差分の確認",
      "リリースチェックリスト",
      "ドキュメントの確認",
      "Federation 接続処理の修正",
      "ノード障害・復旧テスト",
      "パッケージの公開",
    ].map((title, index) => ({
      ...tasks[0],
      id: `task-${index}`,
      title,
      description: "v0.1 のリリースに向けた検証と準備",
      status: index < 4 ? "COMPLETED" : index === 4 ? "RUNNING" : "BLOCKED",
      dependencies: index === 4 ? ["task-3"] : index === 5 ? ["task-4"] : [],
      owner: `aidash://home/agents/${agents[Math.min(3, Math.max(0, index - 2))]}@1.0.0`,
    }));
    data.runs = agents.map((id, index) => ({
      ...runs[0],
      id: `run-${index}`,
      task_id: `task-${index + 2}`,
      agent_id: id,
      phase: index < 2 ? "COMPLETED" : index === 2 ? "THINKING" : "WAITING",
      updated_at: `2026-09-24T01:3${index}:00Z`,
    }));
    data.human_requests = [
      {
        ...data.human_requests[0],
        run_id: "run-3",
        kind: "APPROVAL_REQUIRED",
        prompt: "パッケージの公開を承認しますか？",
      },
    ];
    data.artifacts = [
      {
        id: "artifact-one",
        thread_id: "thread-one",
        task_id: "task-1",
        name: "release-checklist.md",
        kind: "document",
        content: {
          checklist: [
            "接続処理を確認",
            "復旧テストを実行",
            "人間の承認後に公開",
          ],
        },
        created_by: "researcher",
        idempotency_key: "artifact-key",
        created_at: "2026-09-24T01:34:00Z",
      },
    ];
  }
  return data;
}

type Message = {
  id: string;
  thread_id: string;
  sender: string;
  content: string;
  idempotency_key: string | null;
  created_at: string;
  run_id?: string | null;
};

export async function setup(
  page: Page,
  options: {
    failFirstMessage?: boolean;
    /** A person's Mapping context; the default unless `administrator` is set (ADR 0102). */
    subject?: boolean;
    /** The Deployment Administrator context, which shows no Tenant content. */
    administrator?: boolean;
    dashboardContext?: string | null;
    olderMessages?: number;
    runMemory?: boolean;
    planModel?: boolean;
    latestMessageChangesOnPoll?: boolean;
    messageAttachment?: boolean;
    artifacts?: Artifact[];
    extraGraphAgent?: boolean;
    referenceLayout?: boolean;
    locale?: "ja-JP" | "en-US";
    approval?: boolean;
    failFirstUpload?: boolean;
    coreCapabilities?: boolean;
    coreVersion?: boolean;
    taskModelAgent?: boolean;
    runMediaRoutes?: string[][];
    invalidRun?: boolean;
    semanticPause?: string;
    /** The Run is paused on a call the server offers to decline (#763). */
    declinableCall?: boolean;
    /** Declining fails, as when the Run is no longer paused on that call. */
    failDecline?: boolean;
    failureDeliveryRun?: boolean;
    openTask?: boolean;
    foreignRun?: boolean | string;
    remoteAssignment?: boolean;
    nativeMemory?: boolean;
    nativeMemoryDisabled?: boolean;
    remoteNativeCollision?: boolean;
    tenantActions?: string[];
    configuratorRole?: "administrator" | "member" | "node administrator";
    /** When the Run waiting for the question continues without an answer. */
    answerBy?: string;
    setting?: Record<string, unknown>;
    elicitation?: { message: string; schema: Record<string, unknown> };
    /** Items a silent Node left out; every list response carries them (#770). */
    unconfirmed?: Unconfirmed[];
    /** A peer on an older build: no response carries `unconfirmed`. */
    omitUnconfirmed?: boolean;
    /** A request without Tasks, Runs or questions, to show an empty list. */
    withoutWork?: boolean;
  } = {},
) {
  // Tenant content belongs to a Tenant's Mapping (ADR 0102): a spec opens it as a
  // subject unless it names the administrator context or selects a context itself.
  const asSubject =
    options.subject ??
    (!options.administrator && options.dashboardContext === undefined);
  let data = fixture(options.referenceLayout);
  if (options.artifacts) data.artifacts = options.artifacts;
  if (options.configuratorRole) {
    data.registry.push({
      ...data.registry[0],
      id: "aidash.tenant-configurator",
      version: "0.1.0",
      name: { en: "Tenant Configurator" },
      config: { schema_version: 1, bindings: [], remove_default: [] },
    });
  }
  data.unconfirmed = options.unconfirmed ?? [];
  if (options.withoutWork) {
    data.tasks = [];
    data.runs = [];
    data.human_requests = [];
  }
  if (options.planModel)
    data.registry[1].config = {
      ...data.registry[1].config,
      plan_connection: "chatgpt",
      provider: "chatgpt",
    };
  // The reference request verifies on a peer so the request canvas shows a federated zone.
  const labRuns =
    options.referenceLayout && !options.subject
      ? data.runs.filter((run) => run.agent_id === "verifier")
      : [];
  data.runs = data.runs.filter((run) => !labRuns.includes(run));
  const referenceEvents = options.referenceLayout
    ? (
        [
          ["task.created", "01:30:00", data.tasks[0]],
          ["peer.registered", "01:30:40", { node_id: "aidash://lab" }],
          ["task.completed", "01:31:10", { task: data.tasks[0] }],
          ["task.claimed", "01:31:40", data.tasks[1]],
          ["execution.created", "01:32:20", data.runs[0]],
          ["tool.invoke", "01:33:10", {}],
          ["task.completed", "01:34:00", { task: data.tasks[2] }],
          ["artifact.created", "01:34:30", {}],
          ["task.claimed", "01:35:00", data.tasks[3]],
          ["task.completed", "01:36:30", { task: data.tasks[3] }],
          ["task.claimed", "01:37:00", data.tasks[4]],
          ["tool.invoke", "01:37:50", {}],
          ["human_request.created", "01:38:20", {}],
          ["tool.invoke", "01:39:30", {}],
          ["execution.invalid_state", "01:40:10", {}],
        ] as const
      ).map(([kind, time, payload], index) => ({
        id: `reference-event-${index}`,
        kind,
        created_at: `2026-09-24T${time}Z`,
        data: payload,
        node: defaultNode.name,
        node_id: data.deployment.id,
        sequence: index + 1,
        thread_id: "thread-one",
      }))
    : [];
  if (options.semanticPause) {
    data.runs[0].control = "PAUSED";
    data.runs[0].recovery.semantic_reason = options.semanticPause;
  }
  let declinable: Record<string, string> | null = options.declinableCall
    ? { call: "run-0:3:0", tool: "task_delegate", kind: "policy" }
    : null;
  if (options.declinableCall) data.runs[0].control = "PAUSED";
  if (options.invalidRun || options.failureDeliveryRun) {
    data.runs[0].control = "PAUSED";
    data.runs[0].context = null;
    data.runs[0].state = null;
    data.runs[0].state_error = "invalid execution context";
    if (options.failureDeliveryRun) {
      data.runs[0].phase = "WAITING";
      data.runs[0].state = {
        phase: "WAITING",
        data: {
          reason: "failure_delivery",
          target: "FAILED",
          wake_at: "2026-09-22T10:00:00Z",
          last_delivery_error: null,
        },
      };
    }
  }
  if (options.openTask) data.tasks[0].status = "OPEN";
  if (options.foreignRun)
    data.runs[0].home_node =
      typeof options.foreignRun === "string"
        ? options.foreignRun
        : "aidash://remote-home";
  if (options.remoteAssignment)
    data.registry.push({
      ...data.registry[1],
      id: "home-embedding",
      kind: "embedding",
    });
  if (options.nativeMemory) {
    data.registry[0].config = {
      ...data.registry[0].config,
      bindings: [
        ...(data.registry[0].config.bindings ?? []),
        {
          kind: "memory",
          target: {
            registry_deployment: data.deployment.id,
            id: "native-memory",
            version: "1.0.0",
          },
          narrow: {},
        },
      ],
      ...(options.nativeMemoryDisabled
        ? { remove_default: ["memory_recall", "memory_reflect"] }
        : {}),
    };
    data.registry.push({
      ...data.registry[1],
      id: "native-memory",
      kind: "memory",
      config: { engine: "hindsight_rust" },
    });
  }
  if (options.remoteNativeCollision)
    data.registry.push({
      ...data.registry[1],
      id: "remote-native",
      kind: "memory",
      config: { schema_version: 1, source: { adapter: "conversation_memory" } },
    });
  let runMediaRoutes = options.runMediaRoutes ?? [["image/png", "audio/wav"]];
  if (options.coreCapabilities) {
    data.registry[0].config = {
      ...data.registry[0].config,
      schema_version: 1,
      bindings: [
        {
          kind: "bundle",
          target: {
            registry_deployment: data.deployment.id,
            id: "approved-host-tools",
            version: "1.0.0",
          },
          narrow: {},
        },
      ],
      remove_default: [],
    };
  }
  if (options.taskModelAgent) {
    data.registry[0].config = { ...data.registry[0].config, model: null };
    data.registry[1].config = {
      ...data.registry[1].config,
      provider_credential: "openrouter",
      model_id: "anthropic/claude-sonnet-5.5",
      reasoning_effort: "high",
    };
  }
  if (options.coreVersion)
    data.registry.push({ ...data.registry[0], version: "1.1.0" });
  if (options.extraGraphAgent) {
    data.registry.push({
      id: 'review/"[special]:/agent',
      kind: "agent",
      version: "1.0.0",
      name: { en: "Special reviewer" },
      description: { en: "Special reviewer" },
      capabilities: [],
      tags: [],
      languages: ["en"],
      skills: [],
      schema: {},
      config: {
        schema_version: 1,
        bindings: [],
        remove_default: [],
        model: { id: "model", version: "1.0.0" },
      },
    });
  }
  if (options.approval) data.human_requests[0].kind = "APPROVAL_REQUIRED";
  if (options.answerBy) data.human_requests[0].answer_by = options.answerBy;
  if (options.setting) {
    data.human_requests[0].kind = "SETTING_REQUEST";
    data.human_requests[0].prompt = JSON.stringify(options.setting);
  }
  if (options.elicitation) {
    data.human_requests[0].kind = "MCP_ELICITATION";
    data.human_requests[0].prompt = JSON.stringify(options.elicitation);
  }
  const deadlineEvents: unknown[] = [];
  const messages: Record<string, Message[]> = {
    "thread-one": [
      ...Array.from({ length: options.olderMessages ?? 0 }, (_, index) => ({
        id: `older-${index}`,
        thread_id: "thread-one",
        sender: "alice",
        content: `Previous message ${index}`,
        idempotency_key: null,
        created_at: new Date(Date.UTC(2026, 8, 21, 0, 0, index)).toISOString(),
      })),
      {
        id: "message-one",
        thread_id: "thread-one",
        sender: "aidash://home/agents/researcher@1.0.0",
        content: "Evidence is ready to review.",
        idempotency_key: null,
        created_at: "2026-09-22T10:11:00Z",
      },
    ],
    "thread-two": [],
  };
  if (options.referenceLayout) {
    messages["thread-one"] = [
      [
        "南 健人",
        "v0.1 のリリース準備を進めてください。差分の調査・実装・テストを分担し、\nパッケージを公開する前に、私の承認をお願いします。",
      ],
      [
        "aidash://home/agents/researcher@1.0.0",
        "仕様と変更差分の調査が完了しました。検証する項目を共有します。\n@coder 接続処理の修正をお願いします。復旧テストは並行して進められます。",
      ],
      [
        "aidash://home/agents/coder@1.0.0",
        "接続処理の修正を完了しました。@verifier が復旧テストを実行中です。",
      ],
      [
        "aidash://home/agents/publisher@1.0.0",
        "配布パッケージを用意しました。公開操作の承認をお願いします。",
      ],
    ].map(([sender, content], index) => ({
      id: index === 1 ? "message-one" : `reference-message-${index}`,
      thread_id: "thread-one",
      sender,
      content,
      idempotency_key: null,
      created_at: `2026-09-24T01:${32 + index * 2}:00Z`,
    }));
  }
  const uploads = new Map<
    string,
    { id: string; filename: string; media_type: string; size_bytes: number }
  >();
  const messageFiles = new Map<string, string[]>();
  let uploadFailures = options.failFirstUpload ? 1 : 0;
  const submissions: { path: string; body: Record<string, unknown> }[] = [];
  const errors: string[] = [];
  let denyHistory = false;
  let failures = options.failFirstMessage ? 1 : 0;
  let historyRequests = 0;
  // A Thread's messages are flat (#1008): an envelope names no reply thread.
  const envelope = (message: Message) => ({
    message,
    attachments:
      (options.messageAttachment || options.referenceLayout) &&
      message.id === "message-one"
        ? [
            {
              id: "attachment-one",
              filename: options.referenceLayout
                ? "release-checklist.md"
                : "evidence.txt",
              media_type: "text/plain",
              size_bytes: 15,
            },
          ]
        : (messageFiles.get(message.id) ?? []).flatMap((id) => {
            const file = [...uploads.values()].find((file) => file.id === id);
            return file ? [file] : [];
          }),
  });
  page.on("pageerror", (error) => errors.push(error.message));
  // A subject works through a Mapping: a Deployment Administrator context
  // shows no Tenant content (ADR 0102).
  await installBearerDashboard(
    page,
    "fixture",
    asSubject ? { tenant: "acme", name: "alice" } : undefined,
    options.dashboardContext,
  );
  await page.addInitScript((locale) => {
    localStorage.setItem("aidash-locale", locale);
  }, options.locale ?? "en-US");
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    if (
      path === "/api/events/stream" ||
      /^\/api\/executions\/[^/]+\/inference\/stream$/.test(path)
    )
      return route.abort();
    const access = asSubject
      ? { kind: "subject", tenant: "acme", subject: "alice" }
      : data.access;
    if (path === "/api/session")
      return route.fulfill({ json: { access, node_id: data.deployment.id } });
    // Every page lists Runs a suspension paused (#876). Unmocked, its 404 is
    // retried, and every action's query refresh waits for that retry.
    if (path === "/api/suspended-executions")
      return route.fulfill({ json: { runs: [] } });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          ...data,
          runs: data.runs.map(({ context, ...run }) => ({
            ...run,
            input_tokens: null,
            output_tokens: null,
            tool_calls: context == null ? null : 0,
          })),
          human_requests: data.human_requests.filter(
            (request) => request.response === null,
          ),
          access,
          ...(options.omitUnconfirmed ? { unconfirmed: undefined } : {}),
        },
      });
    if (path === "/api/human-requests" && request.method() === "GET")
      return route.fulfill({
        json: {
          items: data.human_requests,
          next_cursor: null,
          ...(options.omitUnconfirmed ? {} : { unconfirmed: [] }),
        },
      });
    if (path === "/api/tenants/acme/nodes")
      return route.fulfill({
        json: [
          {
            ...defaultNode,
            address: `${data.deployment.id}/${defaultNode.name}`,
          },
        ],
      });
    if (path === "/api/tenants/acme/administration")
      return route.fulfill({
        json: {
          tenant: "acme",
          label: null,
          actions: options.tenantActions ?? ["registry.create"],
          assignable_groups: [],
          policy_revision: options.configuratorRole === "member" ? null : 1,
          can_start_configurator: options.configuratorRole === "administrator",
        },
      });
    if (path === "/api/registry/versions")
      return route.fulfill({
        json: {
          versions: data.registry.map((entry) => registryVersion(entry)),
          next_offset: null,
        },
      });
    if (path.endsWith("/memory/participants"))
      return route.fulfill({
        json: {
          items: [
            {
              id: "home-logical-agent",
              agent: { id: "researcher", version: "1.0.0" },
              revision: 7,
            },
          ],
          next: null,
        },
      });
    if (path.endsWith("/remote-grants/inspect"))
      return route.fulfill({
        json: {
          native_required:
            !!options.nativeMemory && !options.nativeMemoryDisabled,
          memory_available: true,
        },
      });
    if (path === "/api/discover")
      return route.fulfill({
        json: {
          agents: data.registry
            .filter((entity) => entity.kind === "agent")
            .map((entity) => ({
              node_id: options.remoteAssignment
                ? "aidash://remote"
                : data.deployment.id,
              entity:
                options.remoteAssignment && options.nativeMemory
                  ? {
                      ...entity,
                      config: {
                        ...entity.config,
                        bindings: (entity.config.bindings ?? []).map(
                          (binding) =>
                            binding.kind === "memory"
                              ? {
                                  ...binding,
                                  target: {
                                    ...binding.target,
                                    registry_deployment: "aidash://remote",
                                    id: "remote-native",
                                  },
                                }
                              : binding,
                        ),
                      },
                    }
                  : entity,
            }))
            .concat(
              labRuns.length
                ? data.registry
                    .filter((entity) => entity.id === "verifier")
                    .map((entity) => ({ node_id: "aidash://lab", entity }))
                : [],
            ),
          errors: [],
        },
      });
    if (/^\/api\/threads\/[^/]+\/message-history$/.test(path)) {
      const id = path.split("/")[3];
      historyRequests += 1;
      if (options.latestMessageChangesOnPoll && historyRequests === 2) {
        const current = messages[id].at(-1);
        if (current) {
          messages[id][messages[id].length - 1] = {
            ...current,
            id: "message-updated",
            content: "New latest message.",
          };
        }
      }
      if (denyHistory)
        return route.fulfill({ status: 403, json: { error: "unavailable" } });
      let selected = messages[id] ?? [];
      const before = url.searchParams.get("before");
      if (before)
        selected = selected.slice(
          0,
          selected.findIndex((m) => m.id === before),
        );
      const limit = Number(url.searchParams.get("limit") ?? 40);
      const hasOlder = selected.length > limit;
      selected = selected.slice(-limit);
      return route.fulfill({
        json: {
          messages: selected.map(envelope),
          next_before: hasOlder ? selected[0].id : null,
          ...(options.omitUnconfirmed ? {} : { unconfirmed: data.unconfirmed }),
        },
      });
    }
    if (
      (options.messageAttachment || options.referenceLayout) &&
      path === "/api/threads/thread-one/attachments/attachment-one"
    ) {
      return route.fulfill({
        body: "Source evidence",
        contentType: "text/plain",
      });
    }
    if (
      /^\/api\/threads\/[^/]+\/attachments$/.test(path) &&
      request.method() === "POST"
    ) {
      submissions.push({
        path,
        body: {
          ...Object.fromEntries(url.searchParams),
          size: request.postDataBuffer()?.byteLength,
        },
      });
      if (uploadFailures-- > 0)
        return route.fulfill({
          status: 503,
          json: { error: "upload unavailable" },
        });
      const key = url.searchParams.get("idempotency_key")!;
      const attachment = uploads.get(key) ?? {
        id: `upload-${key}`,
        filename: url.searchParams.get("filename")!,
        media_type: url.searchParams.get("media_type")!,
        size_bytes: request.postDataBuffer()?.byteLength ?? 0,
      };
      uploads.set(key, attachment);
      return route.fulfill({ json: attachment });
    }
    if (request.method() === "POST") {
      const body = request.postDataJSON();
      submissions.push({ path, body });
      if (path === "/api/threads") {
        const thread = {
          ...data.threads[0],
          id: "prepared",
          title: body.title,
        };
        data.threads.push(thread);
        messages.prepared = [];
        // A Thread opened without a target has no Root Task (#1008).
        return route.fulfill({
          json: { ...thread, conversation: null, task: null, delegation: null },
        });
      }
      if (/^\/api\/threads\/[^/]+\/channel-messages$/.test(path)) {
        if (failures-- > 0)
          return route.fulfill({
            status: 503,
            json: { error: "temporarily unavailable" },
          });
        const id = path.split("/")[3];
        let message = messages[id].find(
          (m) => m.idempotency_key === body.idempotency_key,
        );
        if (!message) {
          message = {
            id: String(body.idempotency_key),
            thread_id: id,
            sender: "alice",
            content: body.content,
            idempotency_key: body.idempotency_key,
            created_at: "2026-09-22T12:00:00Z",
          };
          messages[id].push(message);
          messageFiles.set(message.id, body.attachment_ids ?? []);
        }
        return route.fulfill({ json: envelope(message) });
      }
      const controlled = data.runs.find(
        (run) => path === `/api/executions/${run.id}/control`,
      );
      if (controlled) {
        controlled.control = body.action === "pause" ? "PAUSED" : "ACTIVE";
        return route.fulfill({ json: controlled });
      }
      if (path === "/api/executions/run-0/message")
        return route.fulfill({ json: { sent: true } });
      if (path === "/api/executions/run-0/declined-calls") {
        if (options.failDecline)
          return route.fulfill({
            status: 409,
            json: { error: "the run is not paused on that call" },
          });
        declinable = null;
        data.runs[0].control = "ACTIVE";
        return route.fulfill({ json: data.runs[0] });
      }
      if (path === "/api/setting-requests/question-one/answer") {
        data.human_requests[0].response = {
          outcome: body.action === "decline" ? "declined" : "saved",
        };
        return route.fulfill({ json: data.human_requests[0] });
      }
      if (path === "/api/human-requests/question-one/answer") {
        data.human_requests[0].response = body;
        return route.fulfill({ json: data.human_requests[0] });
      }
    }
    const runsThread = path.match(/^\/api\/threads\/([^/]+)\/executions$/)?.[1];
    if (runsThread)
      return route.fulfill({
        json: {
          items: data.runs
            .filter(
              (run) =>
                run.thread_id === runsThread &&
                !["COMPLETED", "FAILED", "CANCELLED"].includes(run.phase),
            )
            .map(({ context, ...run }) => ({
              ...run,
              input_tokens: null,
              output_tokens: null,
              tool_calls: context == null ? null : 0,
            })),
          next_cursor: null,
        },
      });
    const thread = data.threads.find(
      (value) => path === `/api/threads/${value.id}`,
    );
    if (thread) {
      if (denyHistory)
        return route.fulfill({ status: 403, json: { error: "forbidden" } });
      return route.fulfill({
        json: {
          thread,
          messages: messages[thread.id],
          tasks: data.tasks.filter((task) => task.thread_id === thread.id),
          artifacts: data.artifacts.filter(
            (artifact) => artifact.thread_id === thread.id,
          ),
          events:
            thread.id === "thread-one"
              ? [...referenceEvents, ...deadlineEvents]
              : [],
          ...(options.omitUnconfirmed ? {} : { unconfirmed: data.unconfirmed }),
        },
      });
    }
    if (/^\/api\/executions\/[^/]+\/live-preview$/.test(path))
      return route.fulfill({ status: 404, json: { error: "not_found" } });
    const run = data.runs.find(
      (value) => path === `/api/executions/${value.id}`,
    );
    if (run)
      return route.fulfill({
        json: {
          run,
          ...(declinable && run.id === "run-0"
            ? { declinable_call: declinable }
            : {}),
          invocations: [],
          media_input_routes: runMediaRoutes,
          memory: options.runMemory
            ? { evidence: "Retained execution memory." }
            : {},
        },
      });
    if (/^\/api\/tasks\/[^/]+\/remote-executions$/.test(path))
      return route.fulfill({ json: [] });
    if (path === "/api/working-areas" || path === "/api/references")
      return route.fulfill({ json: { items: [], next_cursor: null } });
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
    /** The Thread `id` is deleted: `/api/state` no longer lists it or its
     *  records (#1008). */
    deleteThread: (id: string) => {
      data.threads = data.threads.filter((thread) => thread.id !== id);
      data.tasks = data.tasks.filter((task) => task.thread_id !== id);
      data.runs = data.runs.filter((run) => run.thread_id !== id);
      data.human_requests = data.human_requests.filter(
        (request) => request.thread_id !== id,
      );
    },
    revokeAgents: () => {
      data = { ...data, registry: [] };
    },
    setRunMediaRoutes: (routes: string[][]) => {
      runMediaRoutes = routes;
    },
    finishRuns: () => {
      for (const run of data.runs) run.phase = "COMPLETED";
    },
    /** The person's Answer Deadline passes: the system closes the question. */
    passAnswerDeadline: () => {
      const request = data.human_requests[0];
      request.response = { answered: false, reason: "answer_deadline" };
      request.answered_by = "system";
      request.answer_by = null;
      deadlineEvents.push({
        id: "deadline-event",
        kind: "human.answered",
        created_at: "2026-09-22T10:20:00Z",
        data: { ...request },
        node: data.threads.find((thread) => thread.id === request.thread_id)!
          .node,
        sequence: 100,
        thread_id: request.thread_id,
      });
    },
  };
}
