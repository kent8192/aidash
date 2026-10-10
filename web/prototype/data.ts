// PROTOTYPE ONLY. Static scenario shared by every redesign variant.
// Shapes mirror /api/state (StateResponse) and Workbench/Trust records, trimmed to what the screens show.

export type TaskStatus =
  | "OPEN"
  | "CLAIMED"
  | "RUNNING"
  | "COMPLETED"
  | "FAILED"
  | "BLOCKED"
  | "CANCELLED";
export type RunPhase =
  | "READY"
  | "THINKING"
  | "TOOL_CALL"
  | "WAITING"
  | "COMPLETED"
  | "FAILED";

export const statusLabel: Record<string, string> = {
  OPEN: "未着手",
  CLAIMED: "引受済み",
  RUNNING: "実行中",
  COMPLETED: "完了",
  FAILED: "失敗",
  BLOCKED: "保留",
  CANCELLED: "取消",
  READY: "準備完了",
  THINKING: "思考中",
  TOOL_CALL: "ツール実行中",
  WAITING: "承認待ち",
  PAUSED: "一時停止",
  APPROVAL_REQUIRED: "承認が必要",
  INFORMATION_REQUEST: "情報の依頼",
};

export const node = {
  id: "aidash://home",
  name: "home",
  endpoint: "https://home.aidash.local",
};
export const peers = [
  { id: "aidash://lab", name: "lab", status: "connected", latencyMs: 38 },
  { id: "aidash://edge-02", name: "edge-02", status: "degraded", latencyMs: 412 },
];

export const viewer = {
  subject: "mika.tanaka@aidash.dev",
  displayName: "田中 美香",
  tenant: "aidash-core",
  authority: "aidash-core / mika.tanaka",
};

export const agents = [
  { id: "researcher", name: "Researcher", node: "aidash://home", model: "claude-sonnet-4.5", phase: "COMPLETED" as RunPhase, role: "仕様と差分の調査" },
  { id: "coder", name: "Coder", node: "aidash://home", model: "gpt-5.1-codex", phase: "COMPLETED" as RunPhase, role: "接続処理の修正" },
  { id: "verifier", name: "Verifier", node: "aidash://lab", model: "gemini-2.5-pro", phase: "THINKING" as RunPhase, role: "障害注入と復旧テスト" },
  { id: "publisher", name: "Publisher", node: "aidash://home", model: "claude-haiku-4.5", phase: "WAITING" as RunPhase, role: "パッケージの公開" },
];

export const workspaces = [
  {
    id: "workspace-one",
    title: "v0.1-release",
    goal: "異なるノードのエージェントで、登録・協調・実行・復旧を一周させる。",
    status: "RUNNING",
    pending: 1,
    tasksDone: 4,
    tasksTotal: 6,
    updatedAt: "2026-10-10T09:41:00+09:00",
    agents: ["researcher", "coder", "verifier", "publisher"],
  },
  {
    id: "workspace-two",
    title: "docs-improvement",
    goal: "運用ドキュメントの欠落を洗い出し、手順を検証可能な形に直す。",
    status: "RUNNING",
    pending: 0,
    tasksDone: 2,
    tasksTotal: 5,
    updatedAt: "2026-10-10T08:12:00+09:00",
    agents: ["researcher"],
  },
  {
    id: "workspace-three",
    title: "performance-test",
    goal: "1,200 同時実行での p95 レイテンシを計測し、劣化箇所を特定する。",
    status: "FAILED",
    pending: 0,
    tasksDone: 3,
    tasksTotal: 4,
    updatedAt: "2026-10-09T22:47:00+09:00",
    agents: ["verifier", "coder"],
  },
  {
    id: "workspace-four",
    title: "website-launch",
    goal: "製品サイトを公開し、問い合わせフォームの到達性を確認する。",
    status: "COMPLETED",
    pending: 0,
    tasksDone: 7,
    tasksTotal: 7,
    updatedAt: "2026-10-08T17:30:00+09:00",
    agents: ["publisher"],
  },
];

export const tasks = [
  { id: "task-0", title: "仕様・変更差分の確認", owner: "researcher", status: "COMPLETED" as TaskStatus, dependencies: [] as string[], startedAt: "09:02", finishedAt: "09:11", step: 6 },
  { id: "task-1", title: "リリースチェックリスト", owner: "researcher", status: "COMPLETED" as TaskStatus, dependencies: ["task-0"], startedAt: "09:11", finishedAt: "09:16", step: 4 },
  { id: "task-2", title: "ドキュメントの確認", owner: "coder", status: "COMPLETED" as TaskStatus, dependencies: ["task-0"], startedAt: "09:12", finishedAt: "09:20", step: 5 },
  { id: "task-3", title: "Federation 接続処理の修正", owner: "coder", status: "COMPLETED" as TaskStatus, dependencies: ["task-0"], startedAt: "09:14", finishedAt: "09:33", step: 14 },
  { id: "task-4", title: "ノード障害・復旧テスト", owner: "verifier", status: "RUNNING" as TaskStatus, dependencies: ["task-3"], startedAt: "09:34", finishedAt: null, step: 9 },
  { id: "task-5", title: "パッケージの公開", owner: "publisher", status: "BLOCKED" as TaskStatus, dependencies: ["task-4"], startedAt: "09:38", finishedAt: null, step: 2 },
];

export const humanRequest = {
  id: "request-one",
  kind: "APPROVAL_REQUIRED",
  agent: "publisher",
  taskId: "task-5",
  prompt: "パッケージの公開を承認しますか？",
  detail:
    "aidash-runtime 0.1.0 を crates.io と社内レジストリへ公開します。復旧テスト (task-4) の完了後に実行されます。",
  createdAt: "09:38",
};

export const artifacts = [
  {
    id: "artifact-one",
    name: "release-checklist.md",
    kind: "document",
    by: "researcher",
    createdAt: "09:16",
    size: "2.4 KB",
    preview: ["接続処理を確認", "復旧テストを実行", "人間の承認後に公開"],
  },
  {
    id: "artifact-two",
    name: "federation-fix.diff",
    kind: "patch",
    by: "coder",
    createdAt: "09:33",
    size: "18.1 KB",
    preview: ["crates/aidash-runtime/src/peer.rs  +42 −17", "server/src/apps/federation/handshake.rs  +9 −3"],
  },
];

export type Message = {
  id: string;
  sender: string; // "human" or agent id or "system"
  at: string;
  content: string;
  attachment?: string;
  thread?: number;
};

export const messages: Message[] = [
  { id: "m1", sender: "human", at: "09:01", content: "v0.1 のリリース準備をお願いします。lab ノードの Verifier も使って、障害時の復旧まで確認してください。" },
  { id: "m2", sender: "researcher", at: "09:11", content: "前回リリースからの差分を確認しました。Federation のハンドシェイクで再接続時にトークンが再発行されない問題があります。", thread: 2 },
  { id: "m3", sender: "researcher", at: "09:16", content: "リリースチェックリストを作成しました。", attachment: "release-checklist.md" },
  { id: "m4", sender: "coder", at: "09:33", content: "再接続時のトークン再発行を修正し、ハンドシェイクのテストを追加しました。変更は 2 ファイルです。", attachment: "federation-fix.diff" },
  { id: "m5", sender: "system", at: "09:34", content: "Verifier (aidash://lab) が「ノード障害・復旧テスト」を引き受けました。" },
  { id: "m6", sender: "verifier", at: "09:37", content: "edge-02 を切断してリース回復を確認中です。3 回中 2 回、14 秒以内に回復しました。" },
  { id: "m7", sender: "publisher", at: "09:38", content: "公開前に承認が必要です。復旧テストの完了を待って公開します。" },
];

export type EventKind = "task" | "run" | "tool" | "request" | "artifact" | "peer" | "error";
export const events: { at: string; kind: EventKind; actor: string; text: string; severity?: "info" | "warn" | "error" }[] = [
  { at: "09:01:12", kind: "task", actor: "human", text: "依頼を作成: v0.1-release" },
  { at: "09:02:03", kind: "run", actor: "researcher", text: "run-0 開始 (THINKING)" },
  { at: "09:05:40", kind: "tool", actor: "researcher", text: "git.diff v0.0.9..HEAD (212 files)" },
  { at: "09:11:02", kind: "task", actor: "researcher", text: "task-0 完了" },
  { at: "09:14:20", kind: "run", actor: "coder", text: "run-1 開始" },
  { at: "09:16:44", kind: "artifact", actor: "researcher", text: "release-checklist.md を作成" },
  { at: "09:21:08", kind: "tool", actor: "coder", text: "cargo test -p aidash-runtime (148 passed)" },
  { at: "09:27:51", kind: "error", actor: "coder", text: "handshake_reconnect が失敗 (1/3)", severity: "warn" },
  { at: "09:33:10", kind: "artifact", actor: "coder", text: "federation-fix.diff を作成" },
  { at: "09:34:02", kind: "peer", actor: "verifier", text: "aidash://lab が task-4 を引受" },
  { at: "09:36:15", kind: "tool", actor: "verifier", text: "chaos.disconnect edge-02 (30s)" },
  { at: "09:36:47", kind: "peer", actor: "system", text: "edge-02 の応答遅延 412ms", severity: "warn" },
  { at: "09:37:29", kind: "run", actor: "verifier", text: "リース回復を確認 (14.2s)" },
  { at: "09:38:05", kind: "request", actor: "publisher", text: "承認を要求: パッケージの公開", severity: "warn" },
  { at: "09:40:58", kind: "tool", actor: "verifier", text: "chaos.disconnect edge-02 (3回目)" },
];

export const runs = [
  { id: "run-0", agent: "researcher", task: "task-1", phase: "COMPLETED" as RunPhase, step: 10, tokensIn: 48210, tokensOut: 6120, toolCalls: 7, duration: "14m 02s" },
  { id: "run-1", agent: "coder", task: "task-3", phase: "COMPLETED" as RunPhase, step: 14, tokensIn: 121904, tokensOut: 18342, toolCalls: 23, duration: "19m 11s" },
  { id: "run-2", agent: "verifier", task: "task-4", phase: "THINKING" as RunPhase, step: 9, tokensIn: 30455, tokensOut: 2210, toolCalls: 6, duration: "7m 48s" },
  { id: "run-3", agent: "publisher", task: "task-5", phase: "WAITING" as RunPhase, step: 2, tokensIn: 3120, tokensOut: 410, toolCalls: 0, duration: "3m 40s" },
];

// Mesh graph for the observation screen.
export type MeshKind = "node" | "agent" | "task" | "model" | "tool" | "artifact";
export const mesh = {
  nodes: [
    { id: "n-home", kind: "node" as MeshKind, label: "home" },
    { id: "n-lab", kind: "node" as MeshKind, label: "lab" },
    { id: "n-edge", kind: "node" as MeshKind, label: "edge-02" },
    { id: "a-researcher", kind: "agent" as MeshKind, label: "Researcher", parent: "n-home" },
    { id: "a-coder", kind: "agent" as MeshKind, label: "Coder", parent: "n-home" },
    { id: "a-publisher", kind: "agent" as MeshKind, label: "Publisher", parent: "n-home" },
    { id: "a-verifier", kind: "agent" as MeshKind, label: "Verifier", parent: "n-lab" },
    { id: "t-github", kind: "tool" as MeshKind, label: "github" },
    { id: "t-cargo", kind: "tool" as MeshKind, label: "cargo" },
    { id: "t-chaos", kind: "tool" as MeshKind, label: "chaos" },
    { id: "m-sonnet", kind: "model" as MeshKind, label: "claude-sonnet-4.5" },
    { id: "m-codex", kind: "model" as MeshKind, label: "gpt-5.1-codex" },
  ],
  edges: [
    { from: "a-researcher", to: "a-coder", relation: "handoff" },
    { from: "a-coder", to: "a-verifier", relation: "handoff" },
    { from: "a-verifier", to: "a-publisher", relation: "depends" },
    { from: "a-researcher", to: "t-github", relation: "uses" },
    { from: "a-coder", to: "t-cargo", relation: "uses" },
    { from: "a-verifier", to: "t-chaos", relation: "uses" },
    { from: "a-researcher", to: "m-sonnet", relation: "model" },
    { from: "a-coder", to: "m-codex", relation: "model" },
    { from: "n-home", to: "n-lab", relation: "peer" },
    { from: "n-lab", to: "n-edge", relation: "peer" },
  ],
};

// Governance (Trust) screen.
export const trustMetrics = [
  { label: "有効なポリシー", value: "23", delta: "+2 (7日)" },
  { label: "承認待ち", value: "1", delta: "公開 1件" },
  { label: "拒否された操作", value: "7", delta: "直近24時間" },
  { label: "未解決インシデント", value: "2", delta: "high 1 / medium 1" },
];

export const policies = [
  { id: "pol-publish", name: "パッケージ公開には人間の承認", effect: "require-approval", scope: "agent:publisher", revision: 4, updatedAt: "10/07" },
  { id: "pol-net", name: "外部ネットワークは許可リストのみ", effect: "deny", scope: "tenant:aidash-core", revision: 11, updatedAt: "10/05" },
  { id: "pol-chaos", name: "障害注入は lab / edge ノードのみ", effect: "allow", scope: "tool:chaos", revision: 2, updatedAt: "10/09" },
  { id: "pol-secrets", name: "資格情報の読み取りを禁止", effect: "deny", scope: "role:agent", revision: 6, updatedAt: "09/30" },
  { id: "pol-budget", name: "1 実行あたり 200k トークン上限", effect: "limit", scope: "tenant:aidash-core", revision: 3, updatedAt: "10/02" },
];

export const audit = [
  { at: "10/10 09:38:05", actor: "publisher", action: "package.publish", target: "aidash-runtime 0.1.0", decision: "pending", policy: "pol-publish" },
  { at: "10/10 09:36:15", actor: "verifier", action: "tool.chaos.disconnect", target: "edge-02", decision: "allow", policy: "pol-chaos" },
  { at: "10/10 09:29:42", actor: "coder", action: "net.fetch", target: "registry.npmjs.org", decision: "deny", policy: "pol-net" },
  { at: "10/10 09:21:08", actor: "coder", action: "tool.cargo.test", target: "aidash-runtime", decision: "allow", policy: "default" },
  { at: "10/10 09:18:30", actor: "coder", action: "secret.read", target: "CRATES_IO_TOKEN", decision: "deny", policy: "pol-secrets" },
  { at: "10/10 09:05:40", actor: "researcher", action: "tool.github.diff", target: "aidash/aidash", decision: "allow", policy: "default" },
  { at: "10/10 09:01:12", actor: "田中 美香", action: "workspace.create", target: "v0.1-release", decision: "allow", policy: "default" },
  { at: "10/09 22:47:19", actor: "verifier", action: "run.budget", target: "run-77", decision: "deny", policy: "pol-budget" },
  { at: "10/09 18:02:55", actor: "鈴木 健太", action: "policy.update", target: "pol-chaos r2", decision: "allow", policy: "admin" },
  { at: "10/09 14:11:03", actor: "publisher", action: "net.fetch", target: "crates.io", decision: "allow", policy: "pol-net" },
];

export const incidents = [
  { id: "INC-12", title: "edge-02 のリース回復が 30 秒を超過", severity: "high", status: "open", openedAt: "10/10 09:41" },
  { id: "INC-11", title: "performance-test で p95 が 2.8s に劣化", severity: "medium", status: "open", openedAt: "10/09 22:47" },
  { id: "INC-09", title: "古いトークンでの再接続を受理", severity: "high", status: "resolved", openedAt: "10/08 11:20" },
];

export const agentName = (id: string) =>
  id === "human" ? viewer.displayName : id === "system" ? "Aidash" : (agents.find((a) => a.id === id)?.name ?? id);
