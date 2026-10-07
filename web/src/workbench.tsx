import { AgentBindings, type Binding } from "./agent-bindings";
import { Button } from "./components/ui/button";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useBlocker } from "@tanstack/react-router";
import {
  Award,
  Download,
  History,
  Bot,
  Blocks,
  Boxes,
  FlaskConical,
  Settings2,
  Send,
  Wrench,
  Zap,
  BookOpen,
  Clock3,
  CheckCircle2,
  CircleAlert,
  FileText,
  ShieldCheck,
} from "lucide-react";
import { ApiError, apiFetch, authenticatedFetch } from "./transport";
import { useI18n } from "./ui";
import type { State } from "./types";
import { TrustOverview } from "./trust-overview";
import {
  TrustAudit,
  TrustCertifications,
  TrustEmpty,
  TrustSummary,
} from "./trust-details";

type Ref = { id: string; version: string };
export type AgentEntry = {
  id: string;
  version: string;
  kind: string;
  name: Record<string, string>;
  description: Record<string, string>;
  capabilities: string[];
  tags: string[];
  languages: string[];
  skills: string[];
  schema: Record<string, unknown>;
  config: {
    model: Ref;
    instructions: string;
    schema_version: number;
    bindings: Binding[];
    remove_default: string[];
    cluster: Ref | null;
    max_steps: number;
  };
};
type Document = { name: string; media_type: string; text: string };
type Draft = {
  id: string;
  tenant: string;
  owner: string;
  revision: number;
  entry: AgentEntry;
  documents: Document[];
  release_notes: string;
  archived: boolean;
  updated_at: string;
};
async function loadDraftPages(): Promise<Draft[]> {
  const drafts: Draft[] = [];
  let path = "/api/workbench/drafts";
  for (;;) {
    const page = await apiFetch<Draft[]>(path);
    drafts.push(...page);
    if (page.length < 100) return drafts;
    const last = page[page.length - 1];
    const cursor = new URLSearchParams({
      before_updated_at: last.updated_at,
      before_id: last.id,
    });
    path = `/api/workbench/drafts?${cursor}`;
  }
}

type DraftShare = {
  subject: string;
  can_edit: boolean;
  documents_current: boolean;
};
type Validation = {
  draft_id: string;
  revision: number;
  valid: boolean;
  message: string;
};
type Registration = {
  draft_id: string;
  revision: number;
  entry: AgentEntry;
  behavioral_tested: boolean;
};
type RegisteredVersion = {
  registered_knowledge_digest?: string | null;
  draft_knowledge_digest?: string | null;
  entry: AgentEntry;
  draft_revision: number | null;
  registered_by: string | null;
  registered_at: string | null;
  release_notes: string;
  source_id: string | null;
  source_version: string | null;
  behavioral_tested: boolean | null;
};
export type Inspection = {
  entry: AgentEntry;
  source_node: string;
  observed_at: string;
  workspaces: {
    workspace_id: string;
    title: string;
    current: boolean;
    latest_run_at: string;
  }[];
  usage_truncated: boolean;
  test_evidence?: {
    session_id: string;
    draft_revision: number;
    mode: string;
    profile_id: string | null;
    profile_revision: number | null;
    status: string;
    usage: Record<string, unknown>;
    created_at: string;
    expired_at: string | null;
  }[];
  test_evidence_truncated?: boolean;
  external_assessment_available: boolean;
};
type TestSession = {
  id: string;
  draft_id: string;
  revision: number;
  status: string;
  scenario: Record<string, unknown>;
  conversation: { role: string; content: unknown }[] | null;
  tool_calls: Record<string, unknown>[] | null;
  usage: Record<string, unknown>;
  error: string | null;
  created_at: string;
  expires_at: string;
  expired_at: string | null;
};
type TestProfile = { id: string; revision: number; enabled: boolean };
type TestLimits = {
  max_input_bytes: number;
  max_output_tokens: number;
  max_total_tokens: number;
  max_steps: number;
  max_duration_secs: number;
  max_concurrent: number;
  payload_days: number;
};
export type Incident = {
  id: string;
  revision: number;
  severity: string;
  status: string;
  archived: boolean;
  owner: string;
  notes: string;
  evidence: { title: string; content: string | null; sha256: string }[];
  created_at: string;
  evidence_expired_at: string | null;
};
export type PermissionContext = {
  tenant: string;
  subject: string;
  workspace_id: string | null;
  policy_revision: number;
  observed_at: string;
  requested_capabilities: string[];
  rows: {
    reference: Ref;
    kind: string;
    action: string;
    catalog_enabled: boolean;
    policy_allowed: boolean;
    registry_read_allowed: boolean | null;
    effective_for_component: boolean;
  }[];
  workspace_read: boolean | null;
  note: string;
};
export type AuditPage = {
  observed_at: string;
  items: {
    source: string;
    kind: string;
    at: string;
    actor: string | null;
    details: Record<string, unknown>;
  }[];
  next_offset: number | null;
  source_boundary: string;
};
type Mode = "creator" | "trust";
type CreatorTab = "overview" | "build" | "test" | "versions" | "register";
type TrustTab =
  | "overview"
  | "policies"
  | "audit"
  | "certifications"
  | "incidents";

const text = {
  "ja-JP": {
    new: "新しいエージェント",
    draft: "下書き",
    save: "下書きを保存",
    saving: "保存中…",
    saved: "保存済み",
    dirty: "未保存の変更",
    conflict:
      "別の画面で更新されました。入力は保持されています。内容を確認してください。",
    validate: "技術検証",
    register: "Registryに登録",
    name: "名前",
    description: "説明",
    category: "カテゴリ",
    tags: "タグ（カンマ区切り）",
    capabilities: "能力（カンマ区切り）",
    instructions: "追加の指示",
    model: "モデル",
    skills: "Skills",
    tools: "ツール",
    maxSteps: "最大ステップ",
    docs: "非公開参照テキスト",
    docName: "文書名",
    docText: "抽出済みテキスト",
    release: "リリースノート",
    dependencies: "依存関係",
    permission:
      "要求する能力と実効権限は別です。登録だけでは実行権限は付与されません。",
    noModels:
      "利用可能なモデルがありません。管理者にモデル登録と参照権限を依頼してください。",
    noDrafts: "閲覧できる下書きはありません。",
    setup:
      "テスト環境はまだ構成されていません。管理者が安全なテスト接続を設定する必要があります。",
    noTests: "この版の動作テストは未実施です。",
    emptyTrust: "閲覧できる登録済みエージェントがありません。",
    noAssessment: "外部の審査体制が整うまで、Trust評価・認証は行いません。",
    noIncident:
      "この版に関連付けられた報告記録はありません。これは安全性の評価を意味しません。",
    noAudit: "この画面で閲覧できる監査履歴はありません。",
    policyContext:
      "実効権限は対象のテナント・主体・ワークスペースを指定した時点で判定されます。",
    workspaces: "この版を使用したワークスペース",
    noUse:
      "閲覧できる実行記録はありません。Catalog登録だけでは使用とみなしません。",
    noModel: "モデル未選択",
    source: "接続中のNode",
    details: "詳細",
    showDetails: "詳細を開く",
    hideDetails: "詳細を閉じる",
    registered: "Registryに登録しました。Marketplace公開や実行許可は別です。",
    refresh: "再読み込み",
    loading: "読み込み中…",
    operatorTenant: "テナント",
    operatorOwner: "所有者",
    create: "下書きを作成",
    version: "バージョン",
    owner: "所有者",
    registeredVersion: "登録済みの版",
    noVersions: "登録済みの版はありません。",
    trust: "Trustで確認",
    creator: "Creatorで開く",
    status: "状態",
    categoryHint: "表示用の分類です。権限は増えません。",
    profile: "エージェント設定",
    build: "構築",
    overview: "概要",
    test: "テスト",
    versions: "バージョン",
    incidents: "インシデント",
    certifications: "認証",
    policies: "ポリシー",
    audit: "監査",
    safe: "外部審査による評価なし",
    select: "下書きを選択",
    testHistory: "テスト履歴",
    unavailable: "利用できません",
    icon: "アイコン（絵文字）",
  },
  "en-US": {
    new: "New agent",
    draft: "Draft",
    save: "Save draft",
    saving: "Saving…",
    saved: "Saved",
    dirty: "Unsaved changes",
    conflict:
      "Another client changed this draft. Your edits are preserved; review before retrying.",
    validate: "Technical validation",
    register: "Register in Registry",
    name: "Name",
    description: "Description",
    category: "Category",
    tags: "Tags (comma separated)",
    capabilities: "Capabilities (comma separated)",
    instructions: "Additional instructions",
    model: "Model",
    skills: "Skills",
    tools: "Tools",
    maxSteps: "Maximum steps",
    docs: "Private reference text",
    docName: "Document name",
    docText: "Extracted text",
    release: "Release notes",
    dependencies: "Dependencies",
    permission:
      "Requested capabilities and effective permissions differ. Registration does not grant execution.",
    noModels:
      "No available model. Ask an administrator to register a model and grant reference access.",
    noDrafts: "No drafts are visible to this identity.",
    setup:
      "The test environment is not configured. An administrator must provide a safe test connection.",
    noTests: "No behavioral test has run for this version.",
    emptyTrust: "No registered agent is visible.",
    noAssessment:
      "Trust assessments and certification are unavailable until external review arrangements exist.",
    noIncident:
      "No report is linked to this version. This is not a safety assessment.",
    noAudit: "No audit history is available in this view.",
    policyContext:
      "Effective permissions require a specific tenant, subject and workspace at observation time.",
    workspaces: "Workspaces using this version",
    noUse:
      "No authorized execution record is visible. Catalog approval alone is not usage.",
    noModel: "No model selected",
    source: "Connected node",
    details: "Details",
    showDetails: "Show details",
    hideDetails: "Hide details",
    registered:
      "Registered in the Registry. Marketplace publication and execution grants are separate.",
    refresh: "Reload",
    loading: "Loading…",
    operatorTenant: "Tenant",
    operatorOwner: "Owner",
    create: "Create draft",
    version: "Version",
    owner: "Owner",
    registeredVersion: "Registered version",
    noVersions: "No registered version is visible.",
    trust: "Inspect in Trust",
    creator: "Open in Creator",
    status: "Status",
    categoryHint: "Discovery label only; it grants no permission.",
    profile: "Agent profile",
    build: "Build",
    overview: "Overview",
    test: "Test",
    versions: "Versions",
    incidents: "Incidents",
    certifications: "Certifications",
    policies: "Policies",
    audit: "Audit",
    safe: "No external assessment",
    select: "Select a draft",
    testHistory: "Test history",
    unavailable: "Unavailable",
    icon: "Icon (emoji)",
  },
} as const;

function split(value: string): string[] {
  return [
    ...new Set(
      value
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean),
    ),
  ];
}
function versionFields(
  draft: AgentEntry,
  registered: AgentEntry,
  draftKnowledgeDigest: string | null,
  registeredKnowledgeDigest: string | null,
): [string, unknown, unknown][] {
  return [
    ["Profile", draft.name, registered.name],
    ["Description", draft.description, registered.description],
    ["Capabilities", draft.capabilities, registered.capabilities],
    ["Tags", draft.tags, registered.tags],
    ["Schema", draft.schema, registered.schema],
    ["Model", draft.config.model, registered.config.model],
    ["Bindings", draft.config.bindings, registered.config.bindings],
    [
      "Removed defaults",
      draft.config.remove_default,
      registered.config.remove_default,
    ],
    ["Cluster", draft.config.cluster, registered.config.cluster],
    ["Private references", draftKnowledgeDigest, registeredKnowledgeDigest],
    ["Instructions", draft.config.instructions, registered.config.instructions],
    ["Maximum steps", draft.config.max_steps, registered.config.max_steps],
  ];
}
function versionDifferences(
  draft: AgentEntry,
  registered: AgentEntry,
  draftKnowledgeDigest: string | null,
  registeredKnowledgeDigest: string | null,
): string[] {
  return versionFields(
    draft,
    registered,
    draftKnowledgeDigest,
    registeredKnowledgeDigest,
  )
    .filter(
      ([, current, previous]) =>
        JSON.stringify(current) !== JSON.stringify(previous),
    )
    .map(([name]) => name);
}
function label(
  entry: { name: Record<string, string> },
  locale: string,
): string {
  return (
    entry.name[locale] ||
    entry.name[locale.slice(0, 2)] ||
    entry.name.en ||
    Object.values(entry.name)[0] ||
    "—"
  );
}
function refKey(ref: Ref): string {
  return `${ref.id}@${ref.version}`;
}
function newEntry(model?: Ref): AgentEntry {
  return {
    id: "",
    version: "1.0.0",
    kind: "agent",
    name: { ja: "", en: "" },
    description: { ja: "", en: "" },
    capabilities: [],
    tags: [],
    languages: ["ja", "en"],
    skills: [],
    schema: {},
    config: {
      model: model ?? { id: "", version: "" },
      instructions: "",
      schema_version: 1,
      bindings: [],
      remove_default: [],
      cluster: null,
      max_steps: 64,
    },
  };
}
function profile(entry: AgentEntry): { category?: string; icon?: string } {
  return (entry.schema["x-aidash-profile"] ?? {}) as {
    category?: string;
    icon?: string;
  };
}

export function Workbench({
  mode,
  data,
  focus,
  select,
  switchMode,
  integratedTools,
}: {
  mode: Mode;
  data: State;
  focus: string;
  select: (focus: string) => void;
  switchMode: (mode: Mode, focus: string) => void;
  integratedTools?: ReactNode;
}) {
  const { locale } = useI18n();
  const t = text[locale];
  const [drafts, setDrafts] = useState<Draft[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [validation, setValidation] = useState<Validation | null>(null);
  const [creatorTab, setCreatorTab] = useState<CreatorTab>("overview");
  const [trustTab, setTrustTab] = useState<TrustTab>("overview");
  const [editing, setEditing] = useState<AgentEntry | null>(null);
  const [documents, setDocuments] = useState<Document[]>([]);
  const [releaseNotes, setReleaseNotes] = useState("");
  const [baseRevision, setBaseRevision] = useState<number | null>(null);
  const [dirty, setDirty] = useState(false);
  const hydratedDraft = useRef<string | null>(null);
  const skipRouteBlock = useRef(false);
  const [details, setDetails] = useState(false);
  const [tenant, setTenant] = useState("");
  const [owner, setOwner] = useState("");
  const [shareSubject, setShareSubject] = useState("");
  const [shareCanEdit, setShareCanEdit] = useState(false);
  const [shareIncludesDocuments, setShareIncludesDocuments] = useState(false);
  const [draftShares, setDraftShares] = useState<DraftShare[]>([]);
  const [transferOwner, setTransferOwner] = useState("");
  const [adoptRef, setAdoptRef] = useState("");
  const [inspection, setInspection] = useState<Inspection | null>(null);
  const [incidentsLoaded, setIncidentsLoaded] = useState(false);
  const [incidents, setIncidents] = useState<Incident[]>([]);
  const [incidentFilter, setIncidentFilter] = useState("all");
  const [incidentSearch, setIncidentSearch] = useState("");
  const [incidentOwner, setIncidentOwner] = useState("");
  const [incidentNotes, setIncidentNotes] = useState("");
  const [incidentSeverity, setIncidentSeverity] = useState("medium");
  const [evidenceTitle, setEvidenceTitle] = useState("");
  const [evidenceContent, setEvidenceContent] = useState("");
  const [policyTenant, setPolicyTenant] = useState(
    data.access.kind === "subject" ? data.access.tenant : "",
  );
  const [policySubject, setPolicySubject] = useState(
    data.access.kind === "subject" ? data.access.subject : "",
  );
  const [policyWorkspace, setPolicyWorkspace] = useState("");
  const [permissionResult, setPermissionContext] = useState<
    (PermissionContext & { agent_ref: string }) | null
  >(null);
  const permissionContext =
    permissionResult?.agent_ref === focus ? permissionResult : null;
  const [auditPage, setAuditPage] = useState<AuditPage | null>(null);
  const [auditOffset, setAuditOffset] = useState(0);
  const [auditError, setAuditError] = useState("");
  const [auditContextKey, setAuditContextKey] = useState("");
  const [testSessions, setTestSessions] = useState<TestSession[]>([]);
  const [registeredVersions, setRegisteredVersions] = useState<
    RegisteredVersion[]
  >([]);
  const [selectedVersion, setSelectedVersion] = useState("");
  const [testInput, setTestInput] = useState("");
  const [fixtures, setFixtures] = useState("{}");
  const [testMode, setTestMode] = useState<"simulated" | "real">("real");
  const [testProfileId, setTestProfileId] = useState("");
  const [testProfiles, setTestProfiles] = useState<TestProfile[]>([]);
  const [testLimits, setTestLimits] = useState<TestLimits | null>(null);
  const [continueFrom, setContinueFrom] = useState<string | null>(null);
  const [pendingTestId, setPendingTestId] = useState<string | null>(null);
  useEffect(() => {
    if (!pendingTestId) return;
    const session = testSessions.find((item) => item.id === pendingTestId);
    if (!session || session.status === "running") return;
    let active = true;
    queueMicrotask(() => {
      if (active) {
        setContinueFrom(
          session.status === "completed" && !session.expired_at
            ? session.id
            : null,
        );
        setPendingTestId(null);
      }
    });
    return () => {
      active = false;
    };
  }, [testSessions, pendingTestId]);
  const isOperator = data.access.kind === "operator";
  useBlocker({
    disabled: !dirty,
    enableBeforeUnload: false,
    shouldBlockFn: ({ current, next }) => {
      if (
        !dirty ||
        skipRouteBlock.current ||
        (current.pathname === next.pathname &&
          current.search.focus === next.search.focus)
      )
        return false;
      if (
        !window.confirm(
          locale === "ja-JP"
            ? "未保存の変更を破棄しますか？"
            : "Discard unsaved changes?",
        )
      )
        return true;
      setDirty(false);
      setBaseRevision(null);
      hydratedDraft.current = null;
      return false;
    },
  });
  const models = data.registry.filter((entry) => entry.kind === "model");
  const agents = data.registry.filter((entry) => entry.kind === "agent");

  const reload = async () => {
    setLoading(true);
    try {
      setDrafts(await loadDraftPages());
      setError("");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
    }
  };
  useEffect(() => {
    let active = true;
    void loadDraftPages()
      .then((value) => {
        if (active) {
          setDrafts(value);
          setError("");
        }
      })
      .catch((cause) => {
        if (active) {
          setError(String(cause));
        }
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    const timer = window.setInterval(() => {
      void loadDraftPages()
        .then((value) => {
          if (active) setDrafts(value);
        })
        .catch((cause) => {
          if (active) {
            setError(String(cause));
          }
        });
    }, 10000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, []);
  const current = useMemo(
    () =>
      drafts.find(
        (draft) => `${draft.entry.id}@${draft.entry.version}` === focus,
      ) ?? null,
    [drafts, focus],
  );
  const currentId = current?.id;
  const currentRevision = current?.revision;
  const currentTenant = current?.tenant;
  const selectedRegisteredVersion = useMemo(
    () =>
      registeredVersions.find(
        (version) => version.entry.version === selectedVersion,
      ) ??
      registeredVersions[0] ??
      null,
    [registeredVersions, selectedVersion],
  );
  useEffect(() => {
    let active = true;
    if (mode !== "creator" || !currentId) {
      queueMicrotask(() => {
        if (active) setRegisteredVersions([]);
      });
      return;
    }
    void apiFetch<RegisteredVersion[]>(
      `/api/workbench/drafts/${currentId}/versions`,
    )
      .then((value) => {
        if (active) setRegisteredVersions(value);
      })
      .catch((cause) => {
        if (active) {
          setRegisteredVersions([]);
          setError(String(cause));
        }
      });
    return () => {
      active = false;
    };
  }, [mode, currentId, currentRevision]);
  const canManageDraft =
    !!current &&
    (isOperator ||
      (data.access.kind === "subject" &&
        data.access.subject === current.owner));
  useEffect(() => {
    let active = true;
    if (!currentId || !canManageDraft || mode !== "creator") {
      queueMicrotask(() => {
        if (active) setDraftShares([]);
      });
      return;
    }
    void apiFetch<DraftShare[]>(`/api/workbench/drafts/${currentId}/shares`)
      .then((shares) => {
        if (active) setDraftShares(shares);
      })
      .catch(() => {
        if (active) setDraftShares([]);
      });
    return () => {
      active = false;
    };
  }, [mode, currentId, canManageDraft]);
  const selectedAgent = useMemo(
    () =>
      mode === "trust" &&
      inspection?.entry &&
      `${inspection.entry.id}@${inspection.entry.version}` === focus
        ? inspection.entry
        : null,
    [mode, inspection, focus],
  );
  useEffect(() => {
    let active = true;
    if (mode !== "trust" || !focus.includes("@")) {
      queueMicrotask(() => {
        if (active) setInspection(null);
      });
      return;
    }
    const [id, version] = focus.split("@");
    queueMicrotask(() => {
      if (active) setInspection(null);
    });
    const read = () => {
      void apiFetch<Inspection>(
        `/api/workbench/versions/${encodeURIComponent(id)}/${encodeURIComponent(version)}`,
      )
        .then((value) => {
          if (active) {
            setInspection(value);
            setError("");
          }
        })
        .catch((cause) => {
          if (active) {
            setInspection(null);
            setIncidents([]);
            setPermissionContext(null);
            setAuditPage(null);
            setError(String(cause));
          }
        });
    };
    read();
    const timer = window.setInterval(read, 10000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [mode, focus]);
  useEffect(() => {
    let active = true;
    queueMicrotask(() => {
      if (active) setPermissionContext(null);
    });
    return () => {
      active = false;
    };
  }, [focus]);
  useEffect(() => {
    let active = true;
    queueMicrotask(() => {
      if (active) {
        setAuditOffset(0);
        setAuditPage(null);
      }
    });
    return () => {
      active = false;
    };
  }, [focus]);
  const inspectedId = inspection?.entry.id;
  const inspectedVersion = inspection?.entry.version;
  useEffect(() => {
    let active = true;
    if (
      mode !== "trust" ||
      !inspectedId ||
      !inspectedVersion ||
      `${inspectedId}@${inspectedVersion}` !== focus
    ) {
      queueMicrotask(() => {
        if (active) {
          setIncidents([]);
          setIncidentsLoaded(false);
        }
      });
      return;
    }
    const read = () => {
      void apiFetch<Incident[]>(
        `/api/workbench/versions/${encodeURIComponent(inspectedId)}/${encodeURIComponent(inspectedVersion)}/incidents`,
      )
        .then((value) => {
          if (active) {
            setIncidents(value);
            setIncidentsLoaded(true);
          }
        })
        .catch((cause) => {
          if (active) {
            setIncidents([]);
            setIncidentsLoaded(false);
            setError(String(cause));
          }
        });
    };
    read();
    const timer = window.setInterval(read, 10000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [mode, focus, inspectedId, inspectedVersion]);
  useEffect(() => {
    let active = true;
    if (mode !== "creator" || !currentId || !currentTenant) {
      queueMicrotask(() => {
        if (active) {
          setTestSessions([]);
          setTestProfiles([]);
          setTestLimits(null);
          setContinueFrom(null);
          setPendingTestId(null);
        }
      });
      return;
    }
    const profilePath = `/api/workbench/test-profiles?tenant=${encodeURIComponent(currentTenant)}&draft_id=${encodeURIComponent(currentId)}`;
    void apiFetch<TestProfile[]>(profilePath)
      .then((value) => {
        if (active) setTestProfiles(value.filter((profile) => profile.enabled));
      })
      .catch(() => {
        if (active) setTestProfiles([]);
      });
    void apiFetch<TestLimits>(`/api/workbench/drafts/${currentId}/test-limits`)
      .then((value) => {
        if (active) setTestLimits(value);
      })
      .catch(() => {
        if (active) setTestLimits(null);
      });
    const read = () => {
      void apiFetch<TestSession[]>(`/api/workbench/drafts/${currentId}/tests`)
        .then((value) => {
          if (active) setTestSessions(value);
        })
        .catch((cause) => {
          if (active) setError(String(cause));
        });
    };
    read();
    const timer = window.setInterval(read, 2500);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [mode, currentId, currentTenant]);
  useEffect(() => {
    let active = true;
    queueMicrotask(() => {
      if (active) {
        setContinueFrom(null);
        setPendingTestId(null);
        setTestProfileId("");
        setTestMode("real");
      }
    });
    return () => {
      active = false;
    };
  }, [currentId]);
  useEffect(() => {
    const key = current ? `${current.id}:${current.revision}` : null;
    if (hydratedDraft.current === key) return;
    if (
      current &&
      dirty &&
      baseRevision !== null &&
      current.revision !== baseRevision
    )
      return;
    let active = true;
    queueMicrotask(() => {
      if (!active) return;
      hydratedDraft.current = key;
      if (!current) {
        setEditing(null);
        setBaseRevision(null);
        setDirty(false);
        return;
      }
      setEditing(structuredClone(current.entry));
      setDocuments(structuredClone(current.documents));
      setReleaseNotes(current.release_notes);
      setBaseRevision(current.revision);
      setDirty(false);
      setValidation(null);
    });
    return () => {
      active = false;
    };
  }, [current, dirty, baseRevision]);
  useEffect(() => {
    const prevent = (event: BeforeUnloadEvent) => {
      if (dirty) event.preventDefault();
    };
    window.addEventListener("beforeunload", prevent);
    return () => window.removeEventListener("beforeunload", prevent);
  }, [dirty]);

  const change = (update: (value: AgentEntry) => void) => {
    setEditing((previous) => {
      if (!previous) return previous;
      const next = structuredClone(previous);
      update(next);
      return next;
    });
    setDirty(true);
    setValidation(null);
    setMessage("");
  };
  const create = async () => {
    if (!models.length) {
      setError(t.noModels);
      return;
    }
    setBusy(true);
    setError("");
    try {
      const result = await apiFetch<Draft>("/api/workbench/drafts", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          ...(isOperator ? { tenant, owner } : {}),
          entry: newEntry(models[0]),
          documents: [],
          release_notes: "",
        }),
      });
      setDrafts((previous) => [result, ...previous]);
      select(`${result.entry.id}@${result.entry.version}`);
      setCreatorTab("build");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };
  const save = async (): Promise<Draft | null> => {
    if (!current || !editing || baseRevision === null) return null;
    setBusy(true);
    setError("");
    try {
      const result = await apiFetch<Draft>(
        `/api/workbench/drafts/${current.id}`,
        {
          method: "PUT",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            expected_revision: baseRevision,
            entry: editing,
            documents,
            release_notes: releaseNotes,
          }),
        },
      );
      setDrafts((previous) =>
        previous.map((draft) => (draft.id === result.id ? result : draft)),
      );
      if (`${result.entry.id}@${result.entry.version}` !== focus)
        select(`${result.entry.id}@${result.entry.version}`);
      setBaseRevision(result.revision);
      setDirty(false);
      setMessage(t.saved);
      return result;
    } catch (cause) {
      setError(
        cause instanceof ApiError && cause.status === 409
          ? t.conflict
          : String(cause),
      );
      return null;
    } finally {
      setBusy(false);
    }
  };
  const duplicateDraft = async () => {
    const source = dirty ? await save() : current;
    if (!source) return;
    setBusy(true);
    setError("");
    try {
      const result = await apiFetch<Draft>(
        `/api/workbench/drafts/${source.id}/duplicate`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ expected_revision: source.revision }),
        },
      );
      setDrafts((previous) => [result, ...previous]);
      select(`${result.entry.id}@${result.entry.version}`);
      setCreatorTab("build");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };
  const toggleArchive = async () => {
    if (!current || dirty) return;
    setBusy(true);
    setError("");
    try {
      const result = await apiFetch<Draft>(
        `/api/workbench/drafts/${current.id}/archive`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            expected_revision: current.revision,
            archived: !current.archived,
          }),
        },
      );
      setDrafts((previous) =>
        previous.map((draft) => (draft.id === result.id ? result : draft)),
      );
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };
  const updateShare = async (
    subject: string,
    enabled: boolean,
    canEdit: boolean,
  ) => {
    if (!current || dirty || !subject.trim()) return;
    setBusy(true);
    setError("");
    try {
      await apiFetch<Draft>(`/api/workbench/drafts/${current.id}/shares`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          subject,
          enabled,
          can_edit: canEdit,
          include_documents: shareIncludesDocuments,
        }),
      });
      setDraftShares(
        await apiFetch<DraftShare[]>(
          `/api/workbench/drafts/${current.id}/shares`,
        ),
      );
      setShareSubject("");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };
  const changeOwner = async () => {
    if (!current || dirty || !transferOwner.trim()) return;
    setBusy(true);
    setError("");
    try {
      const result = await apiFetch<Draft>(
        `/api/workbench/drafts/${current.id}/transfer`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            expected_revision: current.revision,
            new_owner: transferOwner,
          }),
        },
      );
      setDrafts((previous) =>
        previous.map((draft) => (draft.id === result.id ? result : draft)),
      );
      setTransferOwner("");
      if (!isOperator) select("");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };
  const adoptLegacy = async () => {
    const [id, version] = adoptRef.split("@");
    if (!isOperator || !id || !version || !tenant || !owner) return;
    setBusy(true);
    setError("");
    try {
      const result = await apiFetch<Draft>(
        `/api/workbench/agents/${encodeURIComponent(id)}/${encodeURIComponent(version)}/adopt`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ tenant, owner }),
        },
      );
      setDrafts((previous) => [result, ...previous]);
      select(`${result.entry.id}@${result.entry.version}`);
      setCreatorTab("build");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };
  const act = async (kind: "validate" | "register") => {
    const saved = dirty ? await save() : current;
    if (!saved) return;
    setBusy(true);
    setError("");
    try {
      if (kind === "validate") {
        const result = await apiFetch<Validation>(
          `/api/workbench/drafts/${saved.id}/validate`,
          {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ expected_revision: saved.revision }),
          },
        );
        setValidation(result);
        setMessage(result.message);
      } else {
        const result = await apiFetch<Registration>(
          `/api/workbench/drafts/${saved.id}/register`,
          {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ expected_revision: saved.revision }),
          },
        );
        setMessage(t.registered);
        setValidation({
          draft_id: saved.id,
          revision: saved.revision,
          valid: true,
          message: t.registered,
        });
        skipRouteBlock.current = true;
        switchMode("trust", `${result.entry.id}@${result.entry.version}`);
        window.setTimeout(() => {
          skipRouteBlock.current = false;
        }, 1000);
      }
    } catch (cause) {
      setError(
        cause instanceof ApiError && cause.status === 409
          ? t.conflict
          : String(cause),
      );
    } finally {
      setBusy(false);
    }
  };
  const runTest = async () => {
    const saved = dirty ? await save() : current;
    if (!saved || !testInput.trim()) return;
    const submittedMessage = testInput;
    setBusy(true);
    setError("");
    try {
      const parsed = (
        testMode === "simulated" ? JSON.parse(fixtures) : {}
      ) as Record<string, unknown>;
      if (!parsed || Array.isArray(parsed) || typeof parsed !== "object")
        throw new Error("Fixtures must be a JSON object");
      const session = await apiFetch<TestSession>(
        `/api/workbench/drafts/${saved.id}/tests`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            expected_revision: saved.revision,
            message: submittedMessage,
            mode: testMode,
            profile_id: testMode === "real" ? testProfileId : null,
            continue_from: continueFrom,
            fixtures: parsed,
          }),
        },
      );
      setTestSessions((previous) => [session, ...previous]);
      setContinueFrom(
        session.status === "completed" && !session.expired_at
          ? session.id
          : null,
      );
      setPendingTestId(session.status === "running" ? session.id : null);
      setTestInput((pending) => (pending === submittedMessage ? "" : pending));
      if (creatorTab !== "overview") setCreatorTab("test");
    } catch (cause) {
      setError(
        cause instanceof ApiError && cause.status === 409
          ? t.conflict
          : String(cause),
      );
    } finally {
      setBusy(false);
    }
  };
  const stopTest = async (session: TestSession) => {
    try {
      const stopped = await apiFetch<TestSession>(
        `/api/workbench/tests/${session.id}/stop`,
        { method: "POST" },
      );
      setTestSessions((previous) =>
        previous.map((item) => (item.id === stopped.id ? stopped : item)),
      );
      if (continueFrom === stopped.id) setContinueFrom(null);
      if (pendingTestId === stopped.id) setPendingTestId(null);
    } catch (cause) {
      setError(String(cause));
    }
  };
  const createIncident = async () => {
    if (!selectedAgent) return;
    setBusy(true);
    setError("");
    try {
      const selectedOwner =
        incidentOwner ||
        (!isOperator && data.access.kind === "subject"
          ? data.access.subject
          : "");
      const result = await apiFetch<Incident>(
        `/api/workbench/versions/${encodeURIComponent(selectedAgent.id)}/${encodeURIComponent(selectedAgent.version)}/incidents`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            ...(isOperator ? { tenant } : {}),
            owner: selectedOwner,
            severity: incidentSeverity,
            notes: incidentNotes,
            evidence:
              evidenceTitle && evidenceContent
                ? [{ title: evidenceTitle, content: evidenceContent }]
                : [],
          }),
        },
      );
      setIncidents((previous) => [result, ...previous]);
      setIncidentNotes("");
      setEvidenceTitle("");
      setEvidenceContent("");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };
  const changeIncident = async (
    incident: Incident,
    status: string,
    archived: boolean,
  ) => {
    setBusy(true);
    setError("");
    try {
      const result = await apiFetch<Incident>(
        `/api/workbench/incidents/${incident.id}`,
        {
          method: "PUT",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            expected_revision: incident.revision,
            severity: incident.severity,
            status,
            archived,
            owner: incident.owner,
            notes: incident.notes,
            add_evidence: [],
          }),
        },
      );
      setIncidents((previous) =>
        previous.map((item) => (item.id === result.id ? result : item)),
      );
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };
  const exportReport = async (format: "json" | "html") => {
    if (!selectedAgent) return;
    try {
      const params = new URLSearchParams({ format });
      if (permissionContext) {
        params.set("tenant", permissionContext.tenant);
        params.set("subject", permissionContext.subject);
        if (permissionContext.workspace_id)
          params.set("workspace_id", permissionContext.workspace_id);
      }
      const response = await authenticatedFetch(
        `/api/workbench/versions/${encodeURIComponent(selectedAgent.id)}/${encodeURIComponent(selectedAgent.version)}/report?${params.toString()}`,
      );
      const url = URL.createObjectURL(await response.blob());
      const link = document.createElement("a");
      link.href = url;
      link.download = `aidash-${selectedAgent.id}-${selectedAgent.version}-report.${format}`;
      link.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 60_000);
    } catch (cause) {
      setError(String(cause));
    }
  };
  const evaluatePermissions = async () => {
    if (!selectedAgent) return;
    setBusy(true);
    setError("");
    try {
      const result = await apiFetch<PermissionContext>(
        `/api/workbench/versions/${encodeURIComponent(selectedAgent.id)}/${encodeURIComponent(selectedAgent.version)}/permissions`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            tenant: policyTenant,
            subject: policySubject,
            workspace_id: policyWorkspace || null,
          }),
        },
      );
      setPermissionContext({ ...result, agent_ref: refKey(selectedAgent) });
    } catch (cause) {
      setPermissionContext(null);
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };
  const selectedAgentId = selectedAgent?.id;
  const selectedAgentVersion = selectedAgent?.version;
  const currentAuditKey = JSON.stringify([
    selectedAgentId,
    selectedAgentVersion,
    auditOffset,
    isOperator ? policyTenant : null,
  ]);
  useEffect(() => {
    if (
      mode !== "trust" ||
      trustTab !== "audit" ||
      !selectedAgentId ||
      !selectedAgentVersion
    )
      return;
    let active = true;
    const path = `/api/workbench/versions/${encodeURIComponent(selectedAgentId)}/${encodeURIComponent(selectedAgentVersion)}/audit?offset=${auditOffset}${isOperator && policyTenant ? `&tenant=${encodeURIComponent(policyTenant)}` : ""}`;
    const read = () => {
      void apiFetch<AuditPage>(path)
        .then((value) => {
          if (active) {
            setAuditPage(value);
            setAuditContextKey(currentAuditKey);
            setAuditError("");
          }
        })
        .catch((cause) => {
          if (active) {
            setAuditPage(null);
            setAuditContextKey(currentAuditKey);
            setAuditError(String(cause));
          }
        });
    };
    read();
    const timer = window.setInterval(read, 10000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [
    mode,
    trustTab,
    auditOffset,
    policyTenant,
    isOperator,
    selectedAgentId,
    selectedAgentVersion,
    currentAuditKey,
  ]);
  const testConfiguration = (
    <div className="wb-test-configuration">
      <p>
        {testMode === "real"
          ? locale === "ja-JP"
            ? "実モデルと管理者設定の隔離テスト接続。プロファイル外の呼び出しは模擬応答が必要です。"
            : "Real model and administrator-configured isolated test connection. Calls outside the profile require fixtures."
          : locale === "ja-JP"
            ? "実モデル＋明示した模擬ツール応答。未設定のツール応答は実行せず停止します。"
            : "Real model with explicit simulated tool responses. Missing fixtures block calls without live fallback."}
      </p>
      {testLimits && (
        <p className="wb-test-limits">
          {locale === "ja-JP" ? "上限" : "Limits"}: {testLimits.max_steps}{" "}
          {locale === "ja-JP" ? "ステップ" : "steps"} ·{" "}
          {testLimits.max_duration_secs}s · {testLimits.max_output_tokens}{" "}
          {locale === "ja-JP" ? "出力token" : "output tokens"} ·{" "}
          {testLimits.max_total_tokens}{" "}
          {locale === "ja-JP" ? "累計token" : "total tokens"} ·{" "}
          {testLimits.max_concurrent}{" "}
          {locale === "ja-JP" ? "同時実行" : "concurrent"} ·{" "}
          {testLimits.payload_days}{" "}
          {locale === "ja-JP" ? "日保存" : "days retained"}
        </p>
      )}
      <div className="wb-test-options">
        <label>
          {locale === "ja-JP" ? "ツールモード" : "Tool mode"}
          <select
            value={testMode}
            disabled={!!pendingTestId}
            onChange={(event) => {
              setTestMode(event.target.value as "simulated" | "real");
              setContinueFrom(null);
              setPendingTestId(null);
            }}
          >
            <option value="simulated">
              {locale === "ja-JP" ? "模擬" : "Simulated"}
            </option>
            <option value="real">
              {locale === "ja-JP"
                ? "隔離された実接続"
                : "Isolated real connection"}
            </option>
          </select>
        </label>
        {testMode === "real" && (
          <label>
            {locale === "ja-JP"
              ? "テスト接続プロファイル"
              : "Test connection profile"}
            <select
              value={testProfileId}
              disabled={!!pendingTestId}
              onChange={(event) => {
                setTestProfileId(event.target.value);
                setContinueFrom(null);
                setPendingTestId(null);
              }}
            >
              <option value="">
                {locale === "ja-JP" ? "選択してください" : "Select a profile"}
              </option>
              {testProfiles.map((profile) => (
                <option key={profile.id} value={profile.id}>
                  {profile.id} · r{profile.revision}
                </option>
              ))}
            </select>
          </label>
        )}
      </div>
      {testMode === "real" && !testProfiles.length && (
        <p className="wb-alert">{t.setup}</p>
      )}
      {testMode === "simulated" && (
        <details>
          <summary>
            {locale === "ja-JP"
              ? "明示的な模擬ツール応答"
              : "Explicit simulated tool responses"}
          </summary>
          <label>
            {locale === "ja-JP"
              ? "模擬ツール応答（名前 → status / response のJSON）"
              : "Simulated tool fixtures (name → status / response JSON)"}
            <textarea
              rows={3}
              spellCheck={false}
              value={fixtures}
              onChange={(event) => setFixtures(event.target.value)}
            />
          </label>
        </details>
      )}
    </div>
  );
  const testPanel = (
    <section className="wb-card wb-test">
      <h2>
        <FlaskConical size={18} />
        {locale === "ja-JP" ? "テストサンドボックス" : "Test sandbox"}
      </h2>
      {creatorTab === "overview" && (
        <details className="wb-test-config">
          <summary>
            {locale === "ja-JP"
              ? "テスト環境と上限"
              : "Test environment & limits"}
          </summary>
          {testConfiguration}
        </details>
      )}
      <div className="wb-test-options">
        <small>
          {continueFrom
            ? locale === "ja-JP"
              ? `会話を継続: ${continueFrom}`
              : `Continuing conversation: ${continueFrom}`
            : locale === "ja-JP"
              ? "新しい会話"
              : "New conversation"}
        </small>
        <Button
          variant="outline"
          type="button"
          disabled={!!pendingTestId}
          onClick={() => {
            setContinueFrom(null);
            setPendingTestId(null);
            setTestInput("");
          }}
        >
          {locale === "ja-JP" ? "会話をリセット" : "Reset conversation"}
        </Button>
      </div>
      <div className="wb-test-space">
        <div className="wb-test-log">
          {testSessions.length ? (
            testSessions.map((session) => (
              <article
                className="wb-test-session"
                id={`test-${session.id}`}
                key={session.id}
              >
                <header>
                  <strong>
                    r{session.revision} · {session.status}
                  </strong>
                  <small>
                    {session.scenario.mode === "real"
                      ? locale === "ja-JP"
                        ? "実接続"
                        : "Real connection"
                      : locale === "ja-JP"
                        ? "模擬ツール"
                        : "Simulated tools"}
                  </small>
                  <time>
                    {new Date(session.created_at).toLocaleString(locale)}
                  </time>
                  {session.status === "running" && (
                    <Button
                      variant="outline"
                      type="button"
                      onClick={() => void stopTest(session)}
                    >
                      {locale === "ja-JP" ? "停止" : "Stop"}
                    </Button>
                  )}
                  {session.status === "completed" && !session.expired_at && (
                    <Button
                      variant="outline"
                      type="button"
                      disabled={!!pendingTestId}
                      onClick={() => {
                        setContinueFrom(session.id);
                        setTestMode(
                          session.scenario.mode === "real"
                            ? "real"
                            : "simulated",
                        );
                        setTestProfileId(
                          typeof session.scenario.profile_id === "string"
                            ? session.scenario.profile_id
                            : "",
                        );
                      }}
                    >
                      {locale === "ja-JP" ? "ここから継続" : "Continue here"}
                    </Button>
                  )}
                </header>
                {session.expired_at ? (
                  <p>
                    {locale === "ja-JP"
                      ? "テスト本文は保存期間終了により削除されました。"
                      : "Test payload expired and was removed."}
                  </p>
                ) : (
                  <>
                    <div className="wb-chat">
                      {session.conversation?.map((part, index) => (
                        <p key={index} className={part.role}>
                          {typeof part.content === "string"
                            ? part.content
                            : JSON.stringify(part.content, null, 2)}
                        </p>
                      ))}
                    </div>
                    {session.tool_calls?.length ? (
                      <pre>{JSON.stringify(session.tool_calls, null, 2)}</pre>
                    ) : null}
                  </>
                )}
                {session.error && <p className="wb-alert">{session.error}</p>}
                <small>{JSON.stringify(session.usage)}</small>
              </article>
            ))
          ) : (
            <div className="wb-test-empty">
              <FlaskConical size={36} />
              <h3>
                {locale === "ja-JP"
                  ? "テスト会話を開始"
                  : "Start a test conversation"}
              </h3>
              <p>{t.noTests}</p>
            </div>
          )}
        </div>
      </div>
      <div className="wb-test-compose">
        <textarea
          aria-label={locale === "ja-JP" ? "テストメッセージ" : "Test message"}
          rows={2}
          value={testInput}
          placeholder={
            locale === "ja-JP" ? "テストメッセージ…" : "Test message…"
          }
          onChange={(event) => setTestInput(event.target.value)}
        />
        <Button
          variant="outline"
          className="wb-primary"
          type="button"
          disabled={
            busy ||
            !testInput.trim() ||
            (testMode === "real" &&
              !testProfiles.some((profile) => profile.id === testProfileId)) ||
            testSessions.some(
              (session) =>
                session.id === pendingTestId && session.status === "running",
            )
          }
          onClick={() => void runTest()}
        >
          {dirty ? `${t.save} + ${t.test}` : t.test}
        </Button>
      </div>
    </section>
  );
  const choose = (value: string) => {
    if (value !== focus) select(value);
  };

  const renderEditor = () =>
    !editing ? null : (
      <div className="wb-editor wb-editor-grid">
        <section className="wb-card wb-profile wb-span">
          <h2>
            <Bot size={18} /> {t.profile}
          </h2>
          <div className="wb-fields">
            <label>
              {t.name}
              <input
                value={editing.name[locale.slice(0, 2)] ?? ""}
                onChange={(event) =>
                  change((value) => {
                    value.name[locale.slice(0, 2)] = event.target.value;
                    if (!value.name.en) value.name.en = event.target.value;
                  })
                }
              />
            </label>
            <label>
              {t.category}
              <input
                value={profile(editing).category ?? ""}
                onChange={(event) =>
                  change((value) => {
                    value.schema["x-aidash-profile"] = {
                      ...profile(value),
                      category: event.target.value,
                    };
                  })
                }
              />
              <small className="wb-profile-hint">{t.categoryHint}</small>
            </label>
            <label className="wb-span">
              {t.description}
              <textarea
                rows={3}
                value={editing.description[locale.slice(0, 2)] ?? ""}
                onChange={(event) =>
                  change((value) => {
                    value.description[locale.slice(0, 2)] = event.target.value;
                    if (!value.description.en)
                      value.description.en = event.target.value;
                  })
                }
              />
            </label>
            <label>
              {t.tags}
              <input
                value={editing.tags.join(", ")}
                onChange={(event) =>
                  change((value) => {
                    value.tags = split(event.target.value);
                  })
                }
              />
            </label>
            <label>
              {t.icon}
              <input
                maxLength={8}
                value={profile(editing).icon ?? ""}
                onChange={(event) =>
                  change((value) => {
                    value.schema["x-aidash-profile"] = {
                      ...profile(value),
                      icon: event.target.value,
                    };
                  })
                }
              />
            </label>
          </div>
        </section>
        <section className="wb-card wb-instructions wb-span">
          <h2>
            <FileText size={18} />
            {t.instructions}
          </h2>
          <label>
            {t.instructions}
            <textarea
              rows={4}
              value={editing.config.instructions}
              onChange={(event) =>
                change((value) => {
                  value.config.instructions = event.target.value;
                })
              }
            />
          </label>
        </section>
        <section className="wb-card wb-capabilities">
          <h2>
            <Zap size={18} />
            {t.capabilities.split("（")[0].split(" (")[0]}
          </h2>
          <label className="wb-span">
            {t.capabilities}
            <input
              value={editing.capabilities.join(", ")}
              onChange={(event) =>
                change((value) => {
                  value.capabilities = split(event.target.value);
                })
              }
            />
          </label>
        </section>
        <section className="wb-card wb-model">
          <h2>
            <Blocks size={18} />
            {t.model}
          </h2>
          <div className="wb-model-field">
            <label>
              {t.model}
              <select
                value={refKey(editing.config.model)}
                onChange={(event) =>
                  change((value) => {
                    const model = models.find(
                      (item) =>
                        `${item.id}@${item.version}` === event.target.value,
                    );
                    if (model)
                      value.config.model = {
                        id: model.id,
                        version: model.version,
                      };
                  })
                }
              >
                {!models.some(
                  (model) => refKey(model) === refKey(editing.config.model),
                ) && (
                  <option value={refKey(editing.config.model)} disabled>
                    {editing.config.model.id
                      ? `${refKey(editing.config.model)} · ${locale === "ja-JP" ? "利用不可" : "Unavailable"}`
                      : t.noModel}
                  </option>
                )}
                {models.map((model) => (
                  <option
                    key={`${model.id}@${model.version}`}
                    value={`${model.id}@${model.version}`}
                  >
                    {label(model, locale)} · {model.version}
                  </option>
                ))}
              </select>
            </label>
          </div>
        </section>
        <section className="wb-card wb-integrations wb-span">
          <AgentBindings
            value={editing.config}
            entries={data.registry}
            node={data.node.id}
            change={(configuration) =>
              change((value) => {
                value.config.bindings = configuration.bindings;
                value.config.remove_default = configuration.remove_default;
              })
            }
          />
          <label>
            {t.maxSteps}
            <input
              type="number"
              min={1}
              max={1000}
              value={editing.config.max_steps}
              onChange={(event) =>
                change((value) => {
                  value.config.max_steps = Number(event.target.value);
                })
              }
            />
          </label>{" "}
          <p>
            {locale === "ja-JP"
              ? "ここで許可しても、既存の権限ポリシーは広がりません。"
              : "These settings never expand existing permissions."}
          </p>
        </section>
        <section className="wb-card wb-documents wb-span">
          <h2>
            <FileText size={18} /> {t.docs}
          </h2>
          {documents.map((doc, index) => (
            <div className="wb-fields" key={index}>
              <label>
                {t.docName}
                <input
                  value={doc.name}
                  onChange={(event) => {
                    setDocuments((previous) =>
                      previous.map((value, i) =>
                        i === index
                          ? { ...value, name: event.target.value }
                          : value,
                      ),
                    );
                    setDirty(true);
                  }}
                />
              </label>
              <Button
                variant="outline"
                type="button"
                onClick={() => {
                  setDocuments((previous) =>
                    previous.filter((_, i) => i !== index),
                  );
                  setDirty(true);
                }}
              >
                ×
              </Button>
              <label className="wb-span">
                {t.docText}
                <textarea
                  rows={4}
                  value={doc.text}
                  onChange={(event) => {
                    setDocuments((previous) =>
                      previous.map((value, i) =>
                        i === index
                          ? { ...value, text: event.target.value }
                          : value,
                      ),
                    );
                    setDirty(true);
                  }}
                />
              </label>
            </div>
          ))}
          <Button
            variant="outline"
            type="button"
            onClick={() => {
              setDocuments((previous) => [
                ...previous,
                { name: "", media_type: "text/plain", text: "" },
              ]);
              setDirty(true);
            }}
          >
            ＋ {t.docs}
          </Button>
        </section>
      </div>
    );

  const filteredIncidents = incidents.filter(
    (incident) =>
      (incidentFilter === "all" ||
        (incidentFilter === "archived"
          ? incident.archived
          : !incident.archived && incident.status === incidentFilter)) &&
      `${incident.notes} ${incident.owner} ${incident.id}`
        .toLocaleLowerCase()
        .includes(incidentSearch.toLocaleLowerCase()),
  );
  const decisionBadge = (value: boolean | null) => (
    <span
      className={`trust-badge ${value === null ? "neutral" : value ? "success" : "danger"}`}
    >
      {value === null
        ? locale === "ja-JP"
          ? "対象外"
          : "N/A"
        : value
          ? locale === "ja-JP"
            ? "許可"
            : "Allowed"
          : locale === "ja-JP"
            ? "制限"
            : "Restricted"}
    </span>
  );
  const incidentPanel = (
    <div className="trust-incident-layout">
      <section className="wb-card wb-incidents">
        <h2>
          <CircleAlert size={18} />
          {t.incidents}
        </h2>
        <div className="trust-filter-bar">
          <label>
            {locale === "ja-JP" ? "状態" : "Status"}
            <select
              value={incidentFilter}
              onChange={(event) => setIncidentFilter(event.target.value)}
            >
              <option value="all">
                {locale === "ja-JP" ? "すべて" : "All"}
              </option>
              <option value="open">
                {locale === "ja-JP" ? "未解決" : "Open"}
              </option>
              <option value="resolved">
                {locale === "ja-JP" ? "解決済み" : "Resolved"}
              </option>
              <option value="archived">
                {locale === "ja-JP" ? "アーカイブ" : "Archived"}
              </option>
            </select>
          </label>
          <label>
            {locale === "ja-JP" ? "記録を検索" : "Search reports"}
            <input
              type="search"
              value={incidentSearch}
              onChange={(event) => setIncidentSearch(event.target.value)}
            />
          </label>
        </div>
        <p className="trust-caption">
          {incidentsLoaded
            ? `${filteredIncidents.length} / ${incidents.length}`
            : locale === "ja-JP"
              ? "記録を取得中"
              : "Loading reports"}
        </p>
        {filteredIncidents.length ? (
          filteredIncidents.map((incident) => (
            <article key={incident.id}>
              <header>
                <strong
                  className={`trust-badge ${incident.status === "resolved" ? "success" : incident.severity === "high" || incident.severity === "critical" ? "danger" : "warning"}`}
                >
                  {incident.severity} · {incident.status}
                  {incident.archived
                    ? ` · ${locale === "ja-JP" ? "アーカイブ済み" : "Archived"}`
                    : ""}
                </strong>
                <time>
                  {new Date(incident.created_at).toLocaleString(locale)}
                </time>
              </header>
              <p>{incident.notes}</p>
              <small>
                {t.owner}: {incident.owner} · r{incident.revision}
              </small>
              <ul>
                {incident.evidence.map((evidence, index) => (
                  <li key={index}>
                    {evidence.title} · SHA-256 {evidence.sha256.slice(0, 12)}
                    {incident.evidence_expired_at
                      ? ` · ${locale === "ja-JP" ? "期限切れ" : "expired"}`
                      : ""}
                  </li>
                ))}
              </ul>
              <Button
                variant="outline"
                type="button"
                disabled={busy}
                onClick={() =>
                  void changeIncident(
                    incident,
                    incident.status === "resolved" ? "open" : "resolved",
                    incident.archived,
                  )
                }
              >
                {incident.status === "resolved"
                  ? locale === "ja-JP"
                    ? "再オープン"
                    : "Reopen"
                  : locale === "ja-JP"
                    ? "解決済みにする"
                    : "Resolve"}
              </Button>
              <Button
                variant="outline"
                type="button"
                disabled={busy}
                onClick={() =>
                  void changeIncident(
                    incident,
                    incident.status,
                    !incident.archived,
                  )
                }
              >
                {incident.archived
                  ? locale === "ja-JP"
                    ? "アーカイブ解除"
                    : "Unarchive"
                  : locale === "ja-JP"
                    ? "アーカイブ"
                    : "Archive"}
              </Button>
            </article>
          ))
        ) : (
          <div className="trust-empty-state">
            <FileText size={36} />
            <p>
              {!incidentsLoaded
                ? locale === "ja-JP"
                  ? "記録はまだ取得できていません。"
                  : "Reports are not available yet."
                : incidents.length
                  ? locale === "ja-JP"
                    ? "検索条件に一致する記録はありません。"
                    : "No reports match these filters."
                  : locale === "ja-JP"
                    ? "この権限で閲覧できる報告記録はありません。安全性の評価を意味しません。"
                    : "No report is visible with this access. This is not a safety assessment."}
            </p>
          </div>
        )}
      </section>
      <section className="wb-card wb-incidents trust-report-form">
        <h2>
          <FileText size={18} />
          {locale === "ja-JP" ? "報告を追加" : "Add a report"}
        </h2>
        {isOperator && (
          <label>
            {t.operatorTenant}
            <input
              value={tenant}
              onChange={(event) => setTenant(event.target.value)}
            />
          </label>
        )}
        <label>
          {t.owner}
          <input
            value={incidentOwner}
            onChange={(event) => setIncidentOwner(event.target.value)}
            placeholder={
              !isOperator && data.access.kind === "subject"
                ? data.access.subject
                : ""
            }
          />
        </label>
        <label>
          {locale === "ja-JP" ? "報告した重大度" : "Reported severity"}
          <select
            value={incidentSeverity}
            onChange={(event) => setIncidentSeverity(event.target.value)}
          >
            <option value="low">Low</option>
            <option value="medium">Medium</option>
            <option value="high">High</option>
            <option value="critical">Critical</option>
          </select>
        </label>
        <label>
          {locale === "ja-JP" ? "内容" : "Notes"}
          <textarea
            rows={4}
            value={incidentNotes}
            onChange={(event) => setIncidentNotes(event.target.value)}
          />
        </label>
        <details className="trust-evidence-inputs">
          <summary>
            {locale === "ja-JP" ? "証拠（任意）" : "Evidence (optional)"}
          </summary>
          <label>
            {locale === "ja-JP"
              ? "証拠の名前（任意）"
              : "Evidence title (optional)"}
            <input
              value={evidenceTitle}
              onChange={(event) => setEvidenceTitle(event.target.value)}
            />
          </label>
          <label>
            {locale === "ja-JP"
              ? "証拠の固定コピー（任意）"
              : "Fixed evidence copy (optional)"}
            <textarea
              rows={3}
              value={evidenceContent}
              onChange={(event) => setEvidenceContent(event.target.value)}
            />
          </label>
          {Boolean(evidenceTitle.trim()) !==
            Boolean(evidenceContent.trim()) && (
            <p role="status">
              {locale === "ja-JP"
                ? "証拠を添付する場合は、名前と本文の両方を入力してください。"
                : "Enter both an evidence title and content to attach evidence."}
            </p>
          )}
        </details>
        <Button
          variant="outline"
          type="button"
          className="wb-primary"
          disabled={
            busy ||
            !incidentNotes.trim() ||
            Boolean(evidenceTitle.trim()) !== Boolean(evidenceContent.trim())
          }
          onClick={() => void createIncident()}
        >
          {locale === "ja-JP" ? "報告を記録" : "Record incident"}
        </Button>
      </section>
    </div>
  );
  const policyPanel = (
    <div className="trust-policy-stack">
      <div className="trust-policy-metrics">
        {[
          [
            locale === "ja-JP" ? "許可" : "Allowed",
            permissionContext
              ? permissionContext.rows.filter(
                  (row) => row.effective_for_component,
                ).length
              : "—",
          ],
          [
            locale === "ja-JP" ? "制限" : "Restricted",
            permissionContext
              ? permissionContext.rows.filter(
                  (row) => !row.effective_for_component,
                ).length
              : "—",
          ],
          [
            locale === "ja-JP" ? "ポリシー改訂" : "Policy revision",
            permissionContext?.policy_revision ?? "—",
          ],
        ].map(([title, value]) => (
          <section className="wb-card" key={title}>
            <ShieldCheck size={22} />
            <div>
              <span>{title}</span>
              <strong>{value}</strong>
            </div>
          </section>
        ))}
      </div>
      <section className="wb-card wb-policy trust-context-form">
        <h2>
          <ShieldCheck size={18} />
          {locale === "ja-JP" ? "権限の確認条件" : "Permission context"}
        </h2>
        <p>{t.policyContext}</p>
        <div className="wb-fields trust-context-fields">
          <label>
            {t.operatorTenant}
            <input
              value={policyTenant}
              disabled={!isOperator}
              onChange={(event) => setPolicyTenant(event.target.value)}
            />
          </label>
          <label>
            {locale === "ja-JP" ? "主体" : "Subject"}
            <input
              value={policySubject}
              disabled={!isOperator}
              onChange={(event) => setPolicySubject(event.target.value)}
            />
          </label>
          <label>
            {locale === "ja-JP"
              ? "ワークスペースID（任意）"
              : "Workspace ID (optional)"}
            <input
              value={policyWorkspace}
              onChange={(event) => setPolicyWorkspace(event.target.value)}
            />
          </label>
        </div>
        <Button
          variant="outline"
          type="button"
          disabled={busy || !policyTenant || !policySubject}
          onClick={() => void evaluatePermissions()}
        >
          {locale === "ja-JP" ? "この条件で権限を確認" : "Check this context"}
        </Button>
      </section>
      <section className="wb-card wb-policy">
        <h2>
          <FileText size={20} />
          {locale === "ja-JP" ? "権限マトリクス" : "Permission matrix"}
        </h2>
        <p>
          {locale === "ja-JP"
            ? "表示される権限は、確認した条件にのみ適用されます。"
            : "Permissions apply only to the checked context."}
        </p>
        {permissionContext && (
          <>
            <p>
              {permissionContext.tenant} / {permissionContext.subject} /{" "}
              {permissionContext.workspace_id ||
                (locale === "ja-JP"
                  ? "ワークスペース未指定"
                  : "No workspace selected")}
            </p>
            <p>
              {locale === "ja-JP" ? "ポリシー改訂" : "Policy revision"}{" "}
              {permissionContext.policy_revision} ·{" "}
              {new Date(permissionContext.observed_at).toLocaleString(locale)}
            </p>
            <p>
              {locale === "ja-JP" ? "宣言した能力" : "Declared capabilities"}:{" "}
              {permissionContext.requested_capabilities.join(", ") || "—"}
            </p>
          </>
        )}
        <div className="wb-policy-table">
          <table>
            <thead>
              <tr>
                <th>{t.dependencies}</th>
                <th>{locale === "ja-JP" ? "操作" : "Action"}</th>
                <th>Catalog</th>
                <th>Policy</th>
                <th>{locale === "ja-JP" ? "Registry参照" : "Registry read"}</th>
                <th>
                  {locale === "ja-JP"
                    ? "この部品で有効"
                    : "Effective component"}
                </th>
              </tr>
            </thead>
            <tbody>
              {permissionContext ? (
                permissionContext.rows.map((row) => (
                  <tr key={`${row.kind}:${refKey(row.reference)}`}>
                    <td>
                      {row.kind} · {refKey(row.reference)}
                    </td>
                    <td>{row.action}</td>
                    <td>{decisionBadge(row.catalog_enabled)}</td>
                    <td>{decisionBadge(row.policy_allowed)}</td>
                    <td>{decisionBadge(row.registry_read_allowed)}</td>
                    <td>{decisionBadge(row.effective_for_component)}</td>
                  </tr>
                ))
              ) : (
                <tr>
                  <td colSpan={6}>
                    {" "}
                    <TrustEmpty
                      icon={<ShieldCheck size={36} />}
                      title={
                        locale === "ja-JP"
                          ? "条件を指定して権限を確認"
                          : "Choose a context to inspect permissions"
                      }
                    >
                      {locale === "ja-JP"
                        ? "テナントと主体を指定して、権限の確認を実行してください。"
                        : "Select a tenant and subject above to view permission details."}
                    </TrustEmpty>
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
        {permissionContext && (
          <>
            <p>
              {permissionContext.workspace_id
                ? `${t.workspaces}: ${permissionContext.workspace_read ? "✓" : "—"}`
                : locale === "ja-JP"
                  ? "ワークスペース未指定"
                  : "No workspace selected"}
            </p>
            <p>{permissionContext.note}</p>
          </>
        )}
      </section>
    </div>
  );
  const auditPanel = (
    <TrustAudit
      page={auditContextKey === currentAuditKey ? auditPage : null}
      error={auditContextKey === currentAuditKey ? auditError : ""}
      locale={locale}
      offset={auditOffset}
      onOffset={(offset) => {
        setAuditPage(null);
        setAuditOffset(offset);
      }}
      tenantControl={
        isOperator ? (
          <label>
            {t.operatorTenant}
            <input
              value={policyTenant}
              onChange={(event) => {
                setAuditPage(null);
                setAuditOffset(0);
                setPolicyTenant(event.target.value);
              }}
            />
          </label>
        ) : null
      }
    />
  );

  const validationCurrent =
    !!current &&
    !dirty &&
    validation?.draft_id === current.id &&
    validation.revision === current.revision;
  const referenceName = (ref: Ref) => {
    const entry = data.registry.find(
      (item) => item.id === ref.id && item.version === ref.version,
    );
    return entry ? `${label(entry, locale)} · ${ref.version}` : refKey(ref);
  };
  const formatComparison = (value: unknown) =>
    typeof value === "string"
      ? value || "—"
      : value == null
        ? "—"
        : JSON.stringify(value, null, 2);
  const heroEntry = mode === "creator" ? editing : selectedAgent;
  const heroIcon = heroEntry ? profile(heroEntry).icon : undefined;
  return (
    <main
      className={`wb-page ${mode === "creator" ? "wb-creator" : "wb-trust-page"}`}
    >
      <div className="wb-header">
        <div className="wb-identity">
          <span className="wb-hero-icon" aria-hidden="true">
            {heroIcon ? (
              heroIcon
            ) : mode === "creator" ? (
              <Bot size={32} />
            ) : (
              <ShieldCheck size={32} />
            )}
          </span>
          <div>
            <small>Aidash / {mode === "creator" ? "Creator" : "Trust"}</small>
            <h1>
              {mode === "creator"
                ? editing
                  ? label(editing, locale) || t.new
                  : "Creator Workbench"
                : selectedAgent
                  ? label(selectedAgent, locale)
                  : "Trust Workbench"}
            </h1>
            <p>
              {mode === "creator" && current
                ? `${t.draft} · ${current.tenant} / ${current.owner} · r${current.revision}`
                : selectedAgent
                  ? `${selectedAgent.id} · ${selectedAgent.version} · ${t.source}: ${data.node.id}`
                  : t.safe}
            </p>
            {heroEntry && (
              <p className="wb-description">
                {heroEntry.description[locale.slice(0, 2)] ||
                  heroEntry.description.en}
              </p>
            )}
          </div>
        </div>
        <div className="wb-actions">
          {mode === "creator" && current && (
            <>
              <span className={`wb-status ${dirty ? "dirty" : ""}`}>
                {dirty ? t.dirty : t.saved}
              </span>
              <Button
                variant="outline"
                type="button"
                disabled={busy}
                onClick={() => void save()}
              >
                {busy ? t.saving : t.save}
              </Button>
              <Button
                variant="outline"
                type="button"
                disabled={busy}
                onClick={() => void act("validate")}
              >
                {dirty ? `${t.save} + ${t.validate}` : t.validate}
              </Button>
              <Button
                variant="outline"
                className="wb-primary"
                type="button"
                disabled={busy}
                onClick={() => setCreatorTab("register")}
              >
                {locale === "ja-JP" ? "登録内容を確認" : "Review registration"}
              </Button>
            </>
          )}
          {mode === "trust" && selectedAgent && (
            <>
              <Button
                variant="outline"
                type="button"
                onClick={() => void exportReport("json")}
              >
                <Download size={15} />{" "}
                {locale === "ja-JP" ? "レポート JSON" : "Export JSON"}
              </Button>
              <Button
                variant="outline"
                type="button"
                onClick={() => void exportReport("html")}
              >
                <Download size={15} /> HTML / PDF
              </Button>
              <Button
                variant="outline"
                type="button"
                onClick={() => switchMode("creator", focus)}
              >
                {t.creator}
              </Button>
            </>
          )}
        </div>
      </div>
      {integratedTools}
      {error && (
        <p className="wb-alert" role="alert">
          <CircleAlert size={16} />
          {error}
        </p>
      )}
      {message && (
        <p className="wb-notice" role="status">
          {message}
        </p>
      )}
      {mode === "creator" ? (
        <>
          <details
            className="wb-draft-management"
            open={current ? undefined : true}
          >
            <summary>
              {locale === "ja-JP"
                ? "下書きの選択・管理"
                : "Select & manage drafts"}
            </summary>
            <div className="wb-picker">
              <label>
                {t.select}
                <select
                  value={current ? focus : ""}
                  onChange={(event) => choose(event.target.value)}
                >
                  <option value="">{t.new}</option>
                  {drafts.map((draft) => (
                    <option
                      key={draft.id}
                      value={`${draft.entry.id}@${draft.entry.version}`}
                    >
                      {label(draft.entry, locale)} · {draft.entry.version} · r
                      {draft.revision}
                    </option>
                  ))}
                </select>
              </label>
              {isOperator && (
                <>
                  <label>
                    {t.operatorTenant}
                    <input
                      value={tenant}
                      onChange={(event) => setTenant(event.target.value)}
                    />
                  </label>
                  <label>
                    {t.operatorOwner}
                    <input
                      value={owner}
                      onChange={(event) => setOwner(event.target.value)}
                    />
                  </label>
                </>
              )}
              <Button
                variant="outline"
                type="button"
                disabled={busy || !models.length}
                onClick={() => void create()}
              >
                {t.create}
              </Button>
              <Button
                variant="outline"
                type="button"
                onClick={() => void reload()}
              >
                {t.refresh}
              </Button>
            </div>
            {isOperator && agents.length > 0 && (
              <div className="wb-picker">
                <label>
                  {locale === "ja-JP"
                    ? "既存エージェントを割り当て"
                    : "Assign existing agent"}
                  <select
                    value={adoptRef}
                    onChange={(event) => setAdoptRef(event.target.value)}
                  >
                    <option value="">{t.select}</option>
                    {agents.map((agent) => (
                      <option
                        key={`${agent.id}@${agent.version}`}
                        value={`${agent.id}@${agent.version}`}
                      >
                        {label(agent, locale)} · {agent.version}
                      </option>
                    ))}
                  </select>
                </label>
                <Button
                  variant="outline"
                  type="button"
                  disabled={busy || !adoptRef || !tenant || !owner}
                  onClick={() => void adoptLegacy()}
                >
                  {locale === "ja-JP"
                    ? "Creatorに割り当て"
                    : "Assign to Creator"}
                </Button>
              </div>
            )}
          </details>
          {!models.length && (
            <p className="wb-notice" role="status">
              {t.noModels}
            </p>
          )}
          {loading ? (
            <p role="status">{t.loading}</p>
          ) : !current ? (
            <p className="wb-empty">{t.noDrafts}</p>
          ) : (
            <>
              <nav className="wb-tabs" aria-label="Creator">
                <Button
                  variant="outline"
                  aria-current={creatorTab === "overview" ? "page" : undefined}
                  className={creatorTab === "overview" ? "active" : ""}
                  onClick={() => setCreatorTab("overview")}
                >
                  <FileText size={15} /> {t.overview}
                </Button>
                <Button
                  variant="outline"
                  aria-current={creatorTab === "build" ? "page" : undefined}
                  className={creatorTab === "build" ? "active" : ""}
                  onClick={() => setCreatorTab("build")}
                >
                  <Settings2 size={15} /> {t.build}
                </Button>
                <Button
                  variant="outline"
                  aria-current={creatorTab === "test" ? "page" : undefined}
                  className={creatorTab === "test" ? "active" : ""}
                  onClick={() => setCreatorTab("test")}
                >
                  <FlaskConical size={15} /> {t.test}
                </Button>
                <Button
                  variant="outline"
                  aria-current={creatorTab === "versions" ? "page" : undefined}
                  className={creatorTab === "versions" ? "active" : ""}
                  onClick={() => setCreatorTab("versions")}
                >
                  <History size={15} /> {t.versions}
                </Button>
                <Button
                  variant="outline"
                  aria-current={creatorTab === "register" ? "page" : undefined}
                  className={creatorTab === "register" ? "active" : ""}
                  onClick={() => setCreatorTab("register")}
                >
                  <Boxes size={15} /> {t.register}
                </Button>
              </nav>
              <div
                className={`wb-layout ${creatorTab === "overview" ? "overview" : "single"} wb-tab-${creatorTab}`}
              >
                <div>
                  {(creatorTab === "overview" || creatorTab === "build") &&
                    renderEditor()}
                  {creatorTab === "test" && (
                    <div className="wb-test-layout">
                      <section className="wb-card">
                        <h2>
                          <Settings2 size={18} />
                          {locale === "ja-JP"
                            ? "テスト環境"
                            : "Test environment"}
                        </h2>
                        {testConfiguration}
                      </section>
                      {testPanel}
                      <aside className="wb-stack">
                        <section className="wb-card">
                          <h2>
                            <Wrench size={18} />
                            {locale === "ja-JP"
                              ? "ツール実行"
                              : "Tool activity"}
                          </h2>
                          {testSessions[0]?.tool_calls?.length ? (
                            <pre className="wb-json">
                              {JSON.stringify(
                                testSessions[0].tool_calls,
                                null,
                                2,
                              )}
                            </pre>
                          ) : (
                            <p>
                              {locale === "ja-JP"
                                ? "表示できる実行記録はありません。"
                                : "No tool activity available."}
                            </p>
                          )}
                        </section>
                        <section className="wb-card">
                          <h2>
                            <Zap size={18} />
                            {locale === "ja-JP" ? "使用量" : "Usage"}
                          </h2>
                          {testSessions[0] ? (
                            <pre className="wb-json">
                              {JSON.stringify(testSessions[0].usage, null, 2)}
                            </pre>
                          ) : (
                            <p>
                              {locale === "ja-JP" ? "未実行" : "No run yet"}
                            </p>
                          )}
                        </section>
                        <section className="wb-card">
                          <h2>
                            <History size={18} />
                            {locale === "ja-JP" ? "テスト履歴" : "Test history"}
                          </h2>
                          {!testSessions.length && <p>{t.noTests}</p>}
                          {testSessions.map((session) => (
                            <a
                              className="wb-history-item"
                              key={session.id}
                              href={`#test-${session.id}`}
                            >
                              <strong>
                                r{session.revision} · {session.status}
                              </strong>
                              <time>
                                {new Date(session.created_at).toLocaleString(
                                  locale,
                                )}
                              </time>
                            </a>
                          ))}
                        </section>
                      </aside>
                    </div>
                  )}
                  {creatorTab === "versions" && (
                    <div className="wb-stack">
                      <div className="wb-versions-layout">
                        <section className="wb-card wb-version-list">
                          <h2>
                            <History size={18} />
                            {t.versions}
                          </h2>
                          {registeredVersions.map((version) => (
                            <Button
                              variant="outline"
                              key={version.entry.version}
                              type="button"
                              aria-pressed={
                                selectedRegisteredVersion?.entry.version ===
                                version.entry.version
                              }
                              onClick={() =>
                                setSelectedVersion(version.entry.version)
                              }
                            >
                              {version.entry.version} ·{" "}
                              {version.draft_revision === null
                                ? locale === "ja-JP"
                                  ? "既存版"
                                  : "Legacy"
                                : `r${version.draft_revision}`}
                            </Button>
                          ))}
                          {!registeredVersions.length && <p>{t.noVersions}</p>}
                        </section>
                        <section className="wb-card wb-version-compare">
                          <h2>
                            <History size={18} />
                            {locale === "ja-JP"
                              ? "版履歴と差分"
                              : "Version history & comparison"}
                          </h2>
                          <label>
                            {t.registeredVersion}
                            <select
                              disabled={!registeredVersions.length}
                              value={
                                selectedRegisteredVersion?.entry.version ?? ""
                              }
                              onChange={(event) =>
                                setSelectedVersion(event.target.value)
                              }
                            >
                              {!registeredVersions.length && (
                                <option value="">
                                  {locale === "ja-JP"
                                    ? "版を選択"
                                    : "Select a version"}
                                </option>
                              )}
                              {registeredVersions.map((version) => (
                                <option
                                  key={version.entry.version}
                                  value={version.entry.version}
                                >
                                  {version.entry.version}
                                </option>
                              ))}
                            </select>
                          </label>
                          <p>
                            {locale === "ja-JP"
                              ? "保存済み下書きとの比較"
                              : "Compared with saved draft"}{" "}
                            · r{current.revision}
                            {dirty ? ` · ${t.dirty}` : ""}
                          </p>
                          {selectedRegisteredVersion ? (
                            <div className="wb-comparison-scroll">
                              <table className="wb-comparison">
                                <thead>
                                  <tr>
                                    <th>
                                      {locale === "ja-JP" ? "項目" : "Field"}
                                    </th>
                                    <th>
                                      {selectedRegisteredVersion.entry.version}
                                    </th>
                                    <th>{t.draft}</th>
                                  </tr>
                                </thead>
                                <tbody>
                                  {versionFields(
                                    current.entry,
                                    selectedRegisteredVersion.entry,
                                    selectedRegisteredVersion.draft_knowledge_digest ??
                                      null,
                                    selectedRegisteredVersion.registered_knowledge_digest ??
                                      null,
                                  ).map(
                                    ([name, draftValue, registeredValue]) => (
                                      <tr
                                        key={name}
                                        className={
                                          JSON.stringify(draftValue) !==
                                          JSON.stringify(registeredValue)
                                            ? "changed"
                                            : ""
                                        }
                                      >
                                        <th scope="row">{name}</th>
                                        <td>
                                          <pre>
                                            {formatComparison(registeredValue)}
                                          </pre>
                                        </td>
                                        <td>
                                          <pre>
                                            {formatComparison(draftValue)}
                                          </pre>
                                        </td>
                                      </tr>
                                    ),
                                  )}
                                </tbody>
                              </table>
                            </div>
                          ) : (
                            <div className="wb-test-empty">
                              <History size={36} />
                              <h3>
                                {locale === "ja-JP"
                                  ? "最初の版を登録すると履歴が表示されます"
                                  : "Register your first version to view history"}
                              </h3>
                              <p>{t.noVersions}</p>
                            </div>
                          )}
                        </section>
                        <section className="wb-card">
                          <h2>
                            <FileText size={18} />
                            {locale === "ja-JP"
                              ? "版の詳細"
                              : "Version details"}
                          </h2>
                          {selectedRegisteredVersion && (
                            <div className="wb-version-detail">
                              <p>
                                {selectedRegisteredVersion.entry.id}@
                                {selectedRegisteredVersion.entry.version}
                              </p>
                              <p>
                                {selectedRegisteredVersion.registered_at
                                  ? new Date(
                                      selectedRegisteredVersion.registered_at,
                                    ).toLocaleString(locale)
                                  : locale === "ja-JP"
                                    ? "登録時刻は記録されていません"
                                    : "Registration time unavailable"}{" "}
                                ·{" "}
                                {selectedRegisteredVersion.registered_by ?? "—"}
                              </p>
                              <p>
                                {selectedRegisteredVersion.release_notes ||
                                  (locale === "ja-JP"
                                    ? "リリースノートなし"
                                    : "No release notes")}
                              </p>
                              <p>
                                {selectedRegisteredVersion.behavioral_tested ===
                                true
                                  ? locale === "ja-JP"
                                    ? "登録時に完了済みの動作テストあり"
                                    : "Completed behavioral test at registration"
                                  : selectedRegisteredVersion.behavioral_tested ===
                                      false
                                    ? locale === "ja-JP"
                                      ? "登録時に完了済みの動作テストなし"
                                      : "No completed behavioral test at registration"
                                    : locale === "ja-JP"
                                      ? "既存版のテスト情報なし"
                                      : "Legacy test provenance unavailable"}
                              </p>
                              {selectedRegisteredVersion.source_id && (
                                <p>
                                  {locale === "ja-JP" ? "元の版" : "Source"}:{" "}
                                  {selectedRegisteredVersion.source_id}@
                                  {selectedRegisteredVersion.source_version}
                                </p>
                              )}
                              <p>
                                {locale === "ja-JP"
                                  ? "保存済み下書きとの差分"
                                  : "Differences from saved draft"}
                                :{" "}
                                {versionDifferences(
                                  current.entry,
                                  selectedRegisteredVersion.entry,
                                  selectedRegisteredVersion.draft_knowledge_digest ??
                                    null,
                                  selectedRegisteredVersion.registered_knowledge_digest ??
                                    null,
                                ).join(", ") ||
                                  (locale === "ja-JP" ? "なし" : "None")}
                              </p>
                              <Button
                                variant="outline"
                                type="button"
                                onClick={() =>
                                  switchMode(
                                    "trust",
                                    `${selectedRegisteredVersion.entry.id}@${selectedRegisteredVersion.entry.version}`,
                                  )
                                }
                              >
                                {t.trust}
                              </Button>
                            </div>
                          )}
                          {!selectedRegisteredVersion && (
                            <p>
                              {locale === "ja-JP"
                                ? "版が選択されていません。"
                                : "No version selected."}
                            </p>
                          )}
                          <Button
                            variant="outline"
                            type="button"
                            disabled={busy}
                            onClick={() => void duplicateDraft()}
                          >
                            {locale === "ja-JP"
                              ? "新しいIDに複製"
                              : "Duplicate with new ID"}
                          </Button>
                          <p>
                            {locale === "ja-JP"
                              ? "同じIDの新しい版は「Registryに登録」で版番号を変更して作成します。"
                              : "For a new version under this ID, edit the version in Register in Registry."}
                          </p>
                        </section>
                      </div>
                      {canManageDraft && (
                        <section className="wb-card">
                          <h2>
                            {locale === "ja-JP"
                              ? "所有と共有"
                              : "Ownership and sharing"}
                          </h2>
                          <p>
                            {t.owner}: {current.owner}
                          </p>
                          <ul>
                            {draftShares.map((share) => (
                              <li key={share.subject}>
                                {share.subject} ·{" "}
                                {share.can_edit
                                  ? locale === "ja-JP"
                                    ? "編集可"
                                    : "Can edit"
                                  : locale === "ja-JP"
                                    ? "閲覧のみ"
                                    : "Read only"}{" "}
                                {!share.documents_current && (
                                  <strong>
                                    {locale === "ja-JP"
                                      ? "資料変更により再確認が必要"
                                      : "Documents changed; re-share required"}
                                  </strong>
                                )}{" "}
                                <Button
                                  variant="outline"
                                  type="button"
                                  disabled={busy || dirty}
                                  onClick={() =>
                                    void updateShare(
                                      share.subject,
                                      false,
                                      share.can_edit,
                                    )
                                  }
                                >
                                  {locale === "ja-JP" ? "解除" : "Remove"}
                                </Button>
                              </li>
                            ))}
                          </ul>
                          <label>
                            {locale === "ja-JP"
                              ? "共有先の主体"
                              : "Share with subject"}
                            <input
                              value={shareSubject}
                              onChange={(event) =>
                                setShareSubject(event.target.value)
                              }
                            />
                          </label>
                          <label className="wb-check">
                            <input
                              type="checkbox"
                              checked={shareCanEdit}
                              onChange={(event) =>
                                setShareCanEdit(event.target.checked)
                              }
                            />
                            {locale === "ja-JP"
                              ? "編集を許可"
                              : "Allow editing"}
                          </label>
                          {documents.length > 0 && (
                            <label className="wb-check">
                              <input
                                type="checkbox"
                                checked={shareIncludesDocuments}
                                onChange={(event) =>
                                  setShareIncludesDocuments(
                                    event.target.checked,
                                  )
                                }
                              />
                              {locale === "ja-JP"
                                ? "非公開資料も共有することを確認"
                                : "Acknowledge sharing private documents"}
                            </label>
                          )}
                          <Button
                            variant="outline"
                            type="button"
                            disabled={busy || dirty || !shareSubject.trim()}
                            onClick={() =>
                              void updateShare(shareSubject, true, shareCanEdit)
                            }
                          >
                            {locale === "ja-JP" ? "共有" : "Share"}
                          </Button>
                          <label>
                            {locale === "ja-JP" ? "新しい所有者" : "New owner"}
                            <input
                              value={transferOwner}
                              onChange={(event) =>
                                setTransferOwner(event.target.value)
                              }
                            />
                          </label>
                          <Button
                            variant="outline"
                            type="button"
                            disabled={busy || dirty || !transferOwner.trim()}
                            onClick={() => void changeOwner()}
                          >
                            {locale === "ja-JP"
                              ? "所有権を移す"
                              : "Transfer ownership"}
                          </Button>
                          <Button
                            variant="outline"
                            type="button"
                            disabled={busy || dirty}
                            onClick={() => void toggleArchive()}
                          >
                            {current.archived
                              ? locale === "ja-JP"
                                ? "復元"
                                : "Restore"
                              : locale === "ja-JP"
                                ? "下書きをアーカイブ"
                                : "Archive draft"}
                          </Button>
                        </section>
                      )}
                    </div>
                  )}
                  {creatorTab === "register" && (
                    <div className="wb-register-layout">
                      <section className="wb-card">
                        <h2>
                          <Send size={18} />
                          {t.register}
                        </h2>
                        <label>
                          {t.version}
                          <input
                            value={editing?.version ?? ""}
                            onChange={(event) =>
                              change((value) => {
                                value.version = event.target.value;
                              })
                            }
                          />
                        </label>
                        <label>
                          {t.release}
                          <textarea
                            rows={4}
                            value={releaseNotes}
                            onChange={(event) => {
                              setReleaseNotes(event.target.value);
                              setDirty(true);
                            }}
                          />
                        </label>
                        <p>{t.permission}</p>
                        <Button
                          variant="outline"
                          className="wb-primary"
                          type="button"
                          disabled={busy}
                          onClick={() => void act("register")}
                        >
                          {dirty ? `${t.save} + ${t.register}` : t.register}
                        </Button>
                      </section>
                      <section className="wb-card">
                        <h2>
                          <FileText size={18} />
                          {locale === "ja-JP"
                            ? "登録内容の確認"
                            : "Review draft"}
                        </h2>
                        <dl className="wb-summary-list">
                          <div>
                            <dt>{t.profile}</dt>
                            <dd>{editing ? label(editing, locale) : "—"}</dd>
                          </div>
                          <div>
                            <dt>{t.model}</dt>
                            <dd>
                              {editing?.config.model.id
                                ? referenceName(editing.config.model)
                                : t.noModel}
                            </dd>
                          </div>
                          <div>
                            <dt>{t.tools}</dt>
                            <dd>
                              {editing?.config.bindings
                                .filter((b) => b.kind === "tool")
                                .map((b) => b.target)
                                .map(referenceName)
                                .join(", ") || "—"}
                            </dd>
                          </div>
                          <div>
                            <dt>{t.skills}</dt>
                            <dd>
                              {editing?.config.bindings
                                .filter((b) => b.kind === "skill")
                                .map((b) => b.target)
                                .map(referenceName)
                                .join(", ") || "—"}
                            </dd>
                          </div>
                          <div>
                            <dt>{t.docs}</dt>
                            <dd>{documents.length}</dd>
                          </div>
                          <div>
                            <dt>{t.version}</dt>
                            <dd>{editing?.version || "—"}</dd>
                          </div>
                        </dl>
                        <p>
                          {locale === "ja-JP"
                            ? "登録するとRegistryに変更不可の版が作成されます。"
                            : "Registration creates an immutable version in the Registry."}
                        </p>
                        {(!validationCurrent || !validation?.valid) && (
                          <p>
                            {locale === "ja-JP"
                              ? "登録時にサーバーが権限と内容を検証します。"
                              : "The server checks permissions and validates the draft when registering."}
                          </p>
                        )}
                      </section>
                    </div>
                  )}
                </div>
                {creatorTab === "overview" && testPanel}
                {(creatorTab === "overview" ||
                  creatorTab === "build" ||
                  creatorTab === "register") && (
                  <aside className={`wb-sidebar ${details ? "open" : ""}`}>
                    <Button
                      variant="outline"
                      type="button"
                      className="wb-details-toggle"
                      onClick={() => setDetails((value) => !value)}
                    >
                      {details ? t.hideDetails : t.showDetails}
                    </Button>
                    <div className="wb-side-content">
                      <section className="wb-card">
                        <h2>
                          <ShieldCheck size={18} />
                          {t.validate}
                        </h2>
                        <div
                          className={`wb-validation-status ${validationCurrent ? (validation?.valid ? "valid" : "invalid") : ""}`}
                        >
                          {validationCurrent && validation?.valid ? (
                            <CheckCircle2 size={24} />
                          ) : (
                            <CircleAlert size={24} />
                          )}
                          <p>
                            {validationCurrent
                              ? validation?.message
                              : locale === "ja-JP"
                                ? "この下書きの技術検証は未実施です。"
                                : "This draft has not been validated."}
                          </p>
                        </div>
                        <Button
                          variant="outline"
                          type="button"
                          disabled={busy}
                          onClick={() => void act("validate")}
                        >
                          {t.validate}
                        </Button>
                      </section>
                      <section className="wb-card">
                        <h2>
                          <Blocks size={18} />
                          {t.dependencies}
                        </h2>
                        <ul className="wb-dependencies">
                          {editing?.config.model.id && (
                            <li>
                              <Blocks size={18} />
                              <span>
                                {referenceName(editing.config.model)}
                                <small>{t.model}</small>
                              </span>
                            </li>
                          )}
                          {editing?.config.bindings
                            .filter((b) => b.kind === "tool")
                            .map((b) => b.target)
                            .map((ref) => (
                              <li key={`tool-${refKey(ref)}`}>
                                <Wrench size={18} />
                                <span>
                                  {referenceName(ref)}
                                  <small>{t.tools}</small>
                                </span>
                              </li>
                            ))}
                          {editing?.config.bindings
                            .filter((b) => b.kind === "skill")
                            .map((b) => b.target)
                            .map((ref) => (
                              <li key={`skill-${refKey(ref)}`}>
                                <BookOpen size={18} />
                                <span>
                                  {referenceName(ref)}
                                  <small>{t.skills}</small>
                                </span>
                              </li>
                            ))}
                        </ul>
                        {!editing?.config.model.id && <p>{t.noModel}</p>}
                      </section>
                      <section className="wb-card">
                        <h2>
                          <ShieldCheck size={18} />
                          {locale === "ja-JP"
                            ? "要求する能力"
                            : "Requested capabilities"}
                        </h2>
                        <div className="wb-tags">
                          {editing?.capabilities.map((capability) => (
                            <span key={capability}>{capability}</span>
                          ))}
                        </div>
                        {!editing?.capabilities.length && (
                          <p>
                            {locale === "ja-JP" ? "未設定" : "None selected"}
                          </p>
                        )}
                        <p>{t.permission}</p>
                      </section>
                      {creatorTab === "register" && (
                        <section className="wb-card">
                          <h2>
                            <FlaskConical size={18} />
                            {locale === "ja-JP"
                              ? "動作テスト"
                              : "Behavioral test"}
                          </h2>
                          <p>
                            {!dirty &&
                            testSessions.some(
                              (session) =>
                                session.revision === current.revision &&
                                session.status === "completed",
                            )
                              ? locale === "ja-JP"
                                ? "現在の版のテスト実行記録があります。"
                                : "A completed test run exists for this revision."
                              : t.noTests}
                          </p>
                          <Button
                            variant="outline"
                            type="button"
                            onClick={() => setCreatorTab("test")}
                          >
                            {t.test}
                          </Button>
                          <p>{t.noAssessment}</p>
                        </section>
                      )}
                      <section className="wb-card">
                        <h2>
                          <Clock3 size={18} />
                          {t.version}
                        </h2>
                        <dl className="wb-summary-list">
                          <div>
                            <dt>{t.version}</dt>
                            <dd>
                              {editing?.version} · r{baseRevision}
                            </dd>
                          </div>
                          <div>
                            <dt>{t.owner}</dt>
                            <dd>{current.owner}</dd>
                          </div>
                          <div>
                            <dt>
                              {locale === "ja-JP" ? "最終更新" : "Last updated"}
                            </dt>
                            <dd>
                              {new Date(current.updated_at).toLocaleString(
                                locale,
                              )}
                            </dd>
                          </div>
                        </dl>
                        {creatorTab === "register" ? (
                          <>
                            <h3>{t.release}</h3>
                            <p>{releaseNotes || "—"}</p>
                          </>
                        ) : (
                          <label>
                            {t.release}
                            <textarea
                              rows={3}
                              value={releaseNotes}
                              onChange={(event) => {
                                setReleaseNotes(event.target.value);
                                setDirty(true);
                              }}
                            />
                          </label>
                        )}
                      </section>
                    </div>
                  </aside>
                )}
              </div>
            </>
          )}
        </>
      ) : (
        <>
          <div className="wb-picker">
            <label>
              {t.registeredVersion}
              <select
                value={selectedAgent ? focus : ""}
                disabled={busy}
                onChange={(event) => select(event.target.value)}
              >
                <option value="">{t.select}</option>
                {selectedAgent &&
                  !agents.some(
                    (agent) =>
                      agent.id === selectedAgent.id &&
                      agent.version === selectedAgent.version,
                  ) && (
                    <option value={focus}>
                      {label(selectedAgent, locale)} · {selectedAgent.version}
                    </option>
                  )}
                {agents.map((agent) => (
                  <option
                    key={`${agent.id}@${agent.version}`}
                    value={`${agent.id}@${agent.version}`}
                  >
                    {label(agent, locale)} · {agent.version}
                  </option>
                ))}
              </select>
            </label>
          </div>
          {!selectedAgent ? (
            <p className="wb-empty">{t.emptyTrust}</p>
          ) : (
            <>
              <nav className="wb-tabs" aria-label="Trust">
                <Button
                  variant="outline"
                  aria-current={trustTab === "overview" ? "page" : undefined}
                  className={trustTab === "overview" ? "active" : ""}
                  onClick={() => setTrustTab("overview")}
                >
                  <FileText size={15} /> {t.overview}
                </Button>
                <Button
                  variant="outline"
                  aria-current={trustTab === "policies" ? "page" : undefined}
                  className={trustTab === "policies" ? "active" : ""}
                  onClick={() => setTrustTab("policies")}
                >
                  <ShieldCheck size={15} /> {t.policies}
                </Button>
                <Button
                  variant="outline"
                  aria-current={trustTab === "audit" ? "page" : undefined}
                  className={trustTab === "audit" ? "active" : ""}
                  onClick={() => setTrustTab("audit")}
                >
                  <History size={15} /> {t.audit}
                </Button>
                <Button
                  variant="outline"
                  aria-current={
                    trustTab === "certifications" ? "page" : undefined
                  }
                  className={trustTab === "certifications" ? "active" : ""}
                  onClick={() => setTrustTab("certifications")}
                >
                  <Award size={15} /> {t.certifications}
                </Button>
                <Button
                  variant="outline"
                  aria-current={trustTab === "incidents" ? "page" : undefined}
                  className={trustTab === "incidents" ? "active" : ""}
                  onClick={() => setTrustTab("incidents")}
                >
                  <CircleAlert size={15} /> {t.incidents}
                </Button>
              </nav>
              {trustTab === "overview" ? (
                <TrustOverview
                  agent={selectedAgent}
                  inspection={inspection}
                  incidents={incidents}
                  incidentsLoaded={incidentsLoaded}
                  permissions={permissionContext}
                  locale={locale}
                  navigate={setTrustTab}
                />
              ) : (
                <div className="wb-trust-layout trust-detail-layout">
                  <div className="trust-detail-main">
                    {trustTab === "policies" && policyPanel}
                    {trustTab === "audit" && auditPanel}
                    {trustTab === "certifications" && (
                      <TrustCertifications
                        locale={locale}
                        navigate={setTrustTab}
                      />
                    )}
                    {trustTab === "incidents" && incidentPanel}
                  </div>
                  <TrustSummary
                    inspection={inspection}
                    version={selectedAgent.version}
                    locale={locale}
                    audit={trustTab === "audit"}
                  />
                </div>
              )}
            </>
          )}
        </>
      )}
    </main>
  );
}
