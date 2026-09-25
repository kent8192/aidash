import type { Artifact } from "../src/generated/models";
import type { Page } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";

function fixture(reference = false) {
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
    dependencies: [] as string[],
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
  const data = {
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
    artifacts: [] as Artifact[],
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
  if (reference) {
    data.workspaces = [
      {
        ...workspaces[0],
        title: "v0.1-release",
        goal: "異なるノードのエージェントで、登録・協調・実行・復旧を一周させる。",
      },
      {
        ...workspaces[1],
        title: "docs-improvement",
        goal: "ドキュメントを改善する",
      },
      {
        ...workspaces[1],
        id: "workspace-three",
        title: "performance-test",
        goal: "性能を検証する",
      },
      {
        ...workspaces[1],
        id: "workspace-four",
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
        workspace_id: "workspace-one",
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
  workspace_id: string;
  sender: string;
  content: string;
  idempotency_key: string | null;
  created_at: string;
};

export async function setup(
  page: Page,
  options: {
    failFirstMessage?: boolean;
    subject?: boolean;
    olderMessages?: number;
    remoteParticipant?: boolean;
    unpairedRemoteRequest?: boolean;
    meshErrors?: boolean;
    runMemory?: boolean;
    latestMessageChangesOnPoll?: boolean;
    messageAttachment?: boolean;
    extraGraphAgent?: boolean;
    referenceLayout?: boolean;
    locale?: "ja-JP" | "en-US";
    approval?: boolean;
    failFirstUpload?: boolean;
    coreCapabilities?: boolean;
  } = {},
) {
  let data = fixture(options.referenceLayout);
  if (options.coreCapabilities) {
    data.registry[0].config = {
      ...data.registry[0].config,
      core_capabilities: {
        files: true,
        shell: true,
        python: true,
        patch: true,
        skills: true,
        sharing: true,
      },
    };
  }
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
      config: { model: { id: "model", version: "1.0.0" } },
    });
  }
  if (options.approval) data.human_requests[0].kind = "APPROVAL_REQUIRED";
  const messages: Record<string, Message[]> = {
    "workspace-one": [
      ...Array.from({ length: options.olderMessages ?? 0 }, (_, index) => ({
        id: `older-${index}`,
        workspace_id: "workspace-one",
        sender: "alice",
        content: `Previous message ${index}`,
        idempotency_key: null,
        created_at: new Date(Date.UTC(2026, 8, 21, 0, 0, index)).toISOString(),
      })),
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
  if (options.referenceLayout) {
    messages["workspace-one"] = [
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
      workspace_id: "workspace-one",
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
  const threads = new Map<
    string,
    { id: string; workspace_id: string; root_message_id: string }
  >();
  const replies = new Map<string, string>();
  const submissions: { path: string; body: Record<string, unknown> }[] = [];
  const errors: string[] = [];
  let denyHistory = false;
  let denyThreads = false;
  let failures = options.failFirstMessage ? 1 : 0;
  let historyRequests = 0;
  const envelope = (message: Message) => {
    const root = [...threads.values()].find(
      (thread) => thread.root_message_id === message.id,
    );
    return {
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
      thread_id: replies.get(message.id) ?? root?.id ?? null,
      is_thread_root: Boolean(root),
    };
  };
  page.on("pageerror", (error) => errors.push(error.message));
  await installBearerDashboard(page, "fixture");
  await page.addInitScript((locale) => {
    localStorage.setItem("aidash-locale", locale);
  }, options.locale ?? "en-US");
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    if (path === "/api/events/stream") return route.abort();
    const access = options.subject
      ? { kind: "subject", tenant: "acme", subject: "alice" }
      : data.access;
    if (path === "/api/session")
      return route.fulfill({ json: { access, node_id: data.node.id } });
    if (path === "/api/state")
      return route.fulfill({ json: { ...data, access } });
    if (path === "/api/mesh")
      return route.fulfill({
        json: {
          nodes:
            options.remoteParticipant || options.unpairedRemoteRequest
              ? [
                  {
                    node_id: "aidash://peer",
                    runs: options.remoteParticipant
                      ? [
                          {
                            ...data.runs[0],
                            id: "remote-run",
                            task_id: "remote-task",
                          },
                        ]
                      : [],
                    human_requests: options.unpairedRemoteRequest
                      ? [
                          {
                            id: "uncached-run-request",
                            workspace_id: "workspace-one",
                            run_id: "run-omitted-from-snapshot",
                            kind: "INFORMATION_REQUEST",
                            prompt: "The run is outside the capped snapshot.",
                            response: null,
                            answered_by: null,
                            created_at: "2026-09-22T10:12:00Z",
                          },
                        ]
                      : [],
                  },
                ]
              : [],
          errors: options.meshErrors
            ? [
                {
                  node_id: "aidash://peer-down",
                  error: "connection refused",
                },
              ]
            : [],
        },
      });
    if (path === "/api/discover")
      return route.fulfill({
        json: {
          agents: data.registry
            .filter((entity) => entity.kind === "agent")
            .map((entity) => ({ node_id: data.node.id, entity })),
          errors: [],
        },
      });
    if (/^\/api\/workspaces\/[^/]+\/message-history$/.test(path)) {
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
      const threadId = url.searchParams.get("thread_id");
      if (denyHistory || (denyThreads && threadId))
        return route.fulfill({ status: 403, json: { error: "unavailable" } });
      const thread = threadId ? threads.get(threadId) : undefined;
      let selected = (messages[id] ?? []).filter((message) =>
        threadId
          ? message.id === thread?.root_message_id ||
            replies.get(message.id) === threadId
          : !replies.has(message.id),
      );
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
        },
      });
    }
    if (
      (options.messageAttachment || options.referenceLayout) &&
      path === "/api/workspaces/workspace-one/attachments/attachment-one"
    ) {
      return route.fulfill({
        body: "Source evidence",
        contentType: "text/plain",
      });
    }
    if (
      /^\/api\/workspaces\/[^/]+\/attachments$/.test(path) &&
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
      if (/^\/api\/workspaces\/[^/]+\/threads$/.test(path)) {
        const id = path.split("/")[3];
        const previous = [...threads.values()].find(
          (thread) => thread.root_message_id === body.root_message_id,
        );
        const thread = previous ?? {
          id: `thread-${body.root_message_id}`,
          workspace_id: id,
          root_message_id: body.root_message_id,
        };
        threads.set(thread.id, thread);
        return route.fulfill({ json: thread });
      }
      if (/^\/api\/workspaces\/[^/]+\/thread-messages$/.test(path)) {
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
            workspace_id: id,
            sender: "alice",
            content: body.content,
            idempotency_key: body.idempotency_key,
            created_at: "2026-09-22T12:00:00Z",
          };
          messages[id].push(message);
          messageFiles.set(message.id, body.attachment_ids ?? []);
          if (body.thread_id) replies.set(message.id, body.thread_id);
        }
        return route.fulfill({ json: envelope(message) });
      }
      const controlled = data.runs.find(
        (run) => path === `/api/runs/${run.id}/control`,
      );
      if (controlled) {
        controlled.control = body.action === "pause" ? "PAUSED" : "ACTIVE";
        return route.fulfill({ json: controlled });
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
          artifacts: data.artifacts.filter(
            (artifact) => artifact.workspace_id === workspace.id,
          ),
          events: [],
        },
      });
    }
    const run = data.runs.find((value) => path === `/api/runs/${value.id}`);
    if (run)
      return route.fulfill({
        json: {
          run,
          invocations: [],
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
    revokeThreads: () => {
      denyThreads = true;
    },
    revokeAgents: () => {
      data = { ...data, registry: [] };
    },
  };
}
