import { AgentBindings, type Binding } from "./agent-bindings";
import { Button } from "./components/ui/button";
import { Badge as ToneBadge } from "./components/ui/badge";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "./components/ui/tabs";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "./components/ui/table";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useBlocker } from "@tanstack/react-router";
import {
  Archive,
  ArchiveRestore,
  Blocks,
  BookOpen,
  CheckCircle2,
  CircleAlert,
  Copy,
  Download,
  FileText,
  FlaskConical,
  History,
  Plus,
  RefreshCw,
  Send,
  ShieldCheck,
  Wrench,
  X,
} from "lucide-react";
import { ApiError, apiFetch, authenticatedFetch } from "./transport";
import { Field, JsonView, useI18n } from "./ui";
import type { State } from "./types";
import { cn } from "./lib/utils";
import { TrustOverview } from "./trust-overview";
import { TrustAudit, TrustCertifications, TrustSummary } from "./trust-details";
import {
  Alert,
  bodyClass,
  Check,
  Disclosure,
  Dot,
  EmptyState,
  Facts,
  FormSkeleton,
  Hint,
  Inspector,
  InspectorSection,
  MainColumn,
  MetricRow,
  Metric,
  Notice,
  OverviewSkeleton,
  SaveState,
  ScreenHeader,
  Section,
  StatusWord,
} from "./components/patterns";

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
    policyContext:
      "実効権限は対象のテナント・主体・ワークスペースを指定した時点で判定されます。",
    workspaces: "この版を使用したワークスペース",
    noModel: "モデル未選択",
    source: "接続中のNode",
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
    policyContext:
      "Effective permissions require a specific tenant, subject and workspace at observation time.",
    workspaces: "Workspaces using this version",
    noModel: "No model selected",
    source: "Connected node",
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
  const [inspectionAttempt, setInspectionAttempt] = useState(0);
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
  }, [mode, focus, inspectionAttempt]);
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
  const ja = locale === "ja-JP";
  const latestSession = testSessions[0];
  const sendDisabled =
    busy ||
    !testInput.trim() ||
    (testMode === "real" &&
      !testProfiles.some((profile) => profile.id === testProfileId)) ||
    testSessions.some(
      (session) => session.id === pendingTestId && session.status === "running",
    );
  const choose = (value: string) => {
    if (value !== focus) select(value);
  };
  const testEnvironment = (
    <Section
      title={ja ? "テスト環境" : "Test environment"}
      description={
        testMode === "real"
          ? ja
            ? "実モデルと管理者設定の隔離テスト接続。プロファイル外の呼び出しは模擬応答が必要です。"
            : "Real model and administrator-configured isolated test connection. Calls outside the profile require fixtures."
          : ja
            ? "実モデル＋明示した模擬ツール応答。未設定のツール応答は実行せず停止します。"
            : "Real model with explicit simulated tool responses. Missing fixtures block calls without live fallback."
      }
    >
      {testLimits && (
        <MetricRow
          label={ja ? "上限" : "Limits"}
          columns={6}
          compact
          className="border-y"
        >
          {(
            [
              [ja ? "ステップ" : "Steps", testLimits.max_steps],
              [ja ? "秒" : "Seconds", testLimits.max_duration_secs],
              [
                ja ? "出力token" : "Output tokens",
                testLimits.max_output_tokens,
              ],
              [ja ? "累計token" : "Total tokens", testLimits.max_total_tokens],
              [ja ? "同時実行" : "Concurrent", testLimits.max_concurrent],
              [ja ? "保存日数" : "Days retained", testLimits.payload_days],
            ] as const
          ).map(([name, value]) => (
            <Metric
              key={name}
              label={name}
              value={value.toLocaleString(locale)}
            />
          ))}
        </MetricRow>
      )}
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={ja ? "ツールモード" : "Tool mode"}>
          <NativeSelect
            value={testMode}
            disabled={!!pendingTestId}
            onChange={(event) => {
              setTestMode(event.target.value as "simulated" | "real");
              setContinueFrom(null);
              setPendingTestId(null);
            }}
          >
            <option value="simulated">{ja ? "模擬" : "Simulated"}</option>
            <option value="real">
              {ja ? "隔離された実接続" : "Isolated real connection"}
            </option>
          </NativeSelect>
        </Field>
        {testMode === "real" && (
          <Field
            label={ja ? "テスト接続プロファイル" : "Test connection profile"}
          >
            <NativeSelect
              value={testProfileId}
              disabled={!!pendingTestId}
              onChange={(event) => {
                setTestProfileId(event.target.value);
                setContinueFrom(null);
                setPendingTestId(null);
              }}
            >
              <option value="">
                {ja ? "選択してください" : "Select a profile"}
              </option>
              {testProfiles.map((profile) => (
                <option key={profile.id} value={profile.id}>
                  {profile.id} · r{profile.revision}
                </option>
              ))}
            </NativeSelect>
          </Field>
        )}
      </div>
      {testMode === "real" && !testProfiles.length && (
        <Notice tone="warning">{t.setup}</Notice>
      )}
      {testMode === "simulated" && (
        <Disclosure
          summary={
            ja ? "明示的な模擬ツール応答" : "Explicit simulated tool responses"
          }
        >
          <Field
            label={
              ja
                ? "模擬ツール応答（名前 → status / response のJSON）"
                : "Simulated tool fixtures (name → status / response JSON)"
            }
          >
            <Textarea
              rows={3}
              spellCheck={false}
              className="font-mono text-xs"
              value={fixtures}
              onChange={(event) => setFixtures(event.target.value)}
            />
          </Field>
        </Disclosure>
      )}
    </Section>
  );
  const sandbox = (
    <Section
      title={ja ? "テストサンドボックス" : "Test sandbox"}
      description={
        continueFrom
          ? ja
            ? `会話を継続: ${continueFrom}`
            : `Continuing conversation: ${continueFrom}`
          : ja
            ? "新しい会話"
            : "New conversation"
      }
      action={
        <Button
          variant="outline"
          size="sm"
          type="button"
          disabled={!!pendingTestId}
          onClick={() => {
            setContinueFrom(null);
            setPendingTestId(null);
            setTestInput("");
          }}
        >
          {ja ? "会話をリセット" : "Reset conversation"}
        </Button>
      }
    >
      <div className="flex flex-col overflow-hidden rounded-lg border border-border bg-background">
        <div
          role="log"
          aria-label={ja ? "テスト会話" : "Test conversation"}
          className="max-h-[60vh] min-h-64 overflow-y-auto px-3 py-2"
        >
          {testSessions.length ? (
            [...testSessions].reverse().map((session) => (
              <article
                id={`test-${session.id}`}
                key={session.id}
                className="grid gap-1.5 border-b border-border py-2.5 last:border-b-0"
              >
                <header className="flex flex-wrap items-center gap-x-2 gap-y-1">
                  <time className="font-mono text-[11px] tabular text-faint">
                    {new Date(session.created_at).toLocaleString(locale)}
                  </time>
                  <span className="font-mono text-[11px] text-muted-foreground">
                    r{session.revision}
                  </span>
                  <StatusWord value={session.status} />
                  <span className="text-[11px] text-faint">
                    {session.scenario.mode === "real"
                      ? ja
                        ? "実接続"
                        : "Real connection"
                      : ja
                        ? "模擬ツール"
                        : "Simulated tools"}
                  </span>
                  <span className="ml-auto flex gap-1.5">
                    {session.status === "running" && (
                      <Button
                        variant="outline"
                        size="sm"
                        type="button"
                        onClick={() => void stopTest(session)}
                      >
                        {ja ? "停止" : "Stop"}
                      </Button>
                    )}
                    {session.status === "completed" && !session.expired_at && (
                      <Button
                        variant="ghost"
                        size="sm"
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
                        {ja ? "ここから継続" : "Continue here"}
                      </Button>
                    )}
                  </span>
                </header>
                {session.expired_at ? (
                  <p className="text-xs text-muted-foreground">
                    {ja
                      ? "テスト本文は保存期間終了により削除されました。"
                      : "Test payload expired and was removed."}
                  </p>
                ) : (
                  <>
                    {session.conversation?.map((part, index) => (
                      <div
                        key={index}
                        data-role={part.role}
                        className="grid grid-cols-[72px_minmax(0,1fr)] gap-2"
                      >
                        <span className="pt-px font-mono text-[11px] text-faint">
                          {part.role}
                        </span>
                        <pre
                          className={cn(
                            "min-w-0 whitespace-pre-wrap break-words text-foreground",
                            typeof part.content === "string"
                              ? "font-sans text-[13px]"
                              : "font-mono text-xs",
                          )}
                        >
                          {typeof part.content === "string"
                            ? part.content
                            : JSON.stringify(part.content, null, 2)}
                        </pre>
                      </div>
                    ))}
                    {session.tool_calls?.length ? (
                      <details>
                        <summary className="cursor-pointer font-mono text-[11px] text-muted-foreground hover:text-foreground">
                          {ja ? "ツール呼び出し" : "Tool calls"} ·{" "}
                          {session.tool_calls.length}
                        </summary>
                        <div className="mt-2">
                          <JsonView value={session.tool_calls} />
                        </div>
                      </details>
                    ) : null}
                  </>
                )}
                {session.error && (
                  <p className="text-xs text-destructive">{session.error}</p>
                )}
                {Object.keys(session.usage).length > 0 && (
                  <p className="font-mono text-[11px] tabular text-faint">
                    {Object.entries(session.usage)
                      .map(
                        ([key, value]) =>
                          `${key} ${typeof value === "object" ? JSON.stringify(value) : String(value)}`,
                      )
                      .join(" · ")}
                  </p>
                )}
              </article>
            ))
          ) : (
            <div className="py-3">
              <EmptyState
                icon={<FlaskConical />}
                title={ja ? "テスト会話を開始" : "Start a test conversation"}
              >
                {t.noTests}
              </EmptyState>
            </div>
          )}
        </div>
        <div className="wb-test-compose flex items-end gap-2 border-t border-border bg-surface p-2">
          <Textarea
            aria-label={ja ? "テストメッセージ" : "Test message"}
            rows={2}
            className="min-h-14 flex-1 resize-y"
            value={testInput}
            placeholder={ja ? "テストメッセージ…" : "Test message…"}
            onChange={(event) => setTestInput(event.target.value)}
          />
          <Button
            type="button"
            disabled={sendDisabled}
            onClick={() => void runTest()}
          >
            <Send aria-hidden />
            {dirty ? `${t.save} + ${t.test}` : t.test}
          </Button>
        </div>
      </div>
    </Section>
  );
  const testInspector = (
    <Inspector label={ja ? "テストの記録" : "Test records"}>
      <InspectorSection title={ja ? "ツール実行" : "Tool activity"}>
        {latestSession?.tool_calls?.length ? (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="w-8">#</TableHead>
                <TableHead>{ja ? "ツール" : "Tool"}</TableHead>
                <TableHead>{ja ? "結果" : "Outcome"}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {latestSession.tool_calls.map((call, index) => (
                <TableRow key={index}>
                  <TableCell className="font-mono text-xs text-faint">
                    {index + 1}
                  </TableCell>
                  <TableCell className="font-mono text-xs">
                    {typeof call.name === "string" ? call.name : "—"}
                  </TableCell>
                  <TableCell>
                    {typeof call.outcome === "string" ? (
                      <StatusWord value={call.outcome} />
                    ) : (
                      "—"
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        ) : (
          <Hint>
            {ja
              ? "表示できる実行記録はありません。"
              : "No tool activity available."}
          </Hint>
        )}
      </InspectorSection>
      <InspectorSection title={ja ? "使用量" : "Usage"}>
        {latestSession && Object.keys(latestSession.usage).length ? (
          <Facts
            items={Object.entries(latestSession.usage).map(
              ([key, value]): [ReactNode, ReactNode] => [
                <span className="font-mono">{key}</span>,
                <span className="font-mono tabular">
                  {typeof value === "number"
                    ? value.toLocaleString(locale)
                    : JSON.stringify(value)}
                </span>,
              ],
            )}
          />
        ) : (
          <Hint>{latestSession ? "—" : ja ? "未実行" : "No run yet"}</Hint>
        )}
      </InspectorSection>
      <InspectorSection title={t.testHistory}>
        {testSessions.length ? (
          <ul className="divide-y divide-border">
            {testSessions.map((session) => (
              <li key={session.id}>
                <a
                  href={`#test-${session.id}`}
                  className="flex items-center gap-2 py-1.5 text-[13px] text-muted-foreground transition-colors hover:text-foreground"
                >
                  <span className="font-mono text-xs">r{session.revision}</span>
                  <StatusWord value={session.status} />
                  <time className="ml-auto font-mono text-[11px] tabular text-faint">
                    {new Date(session.created_at).toLocaleString(locale)}
                  </time>
                </a>
              </li>
            ))}
          </ul>
        ) : (
          <Hint>{t.noTests}</Hint>
        )}
      </InspectorSection>
    </Inspector>
  );

  const editor = editing && (
    <>
      <Section title={t.profile}>
        <div className="grid items-start gap-4 sm:grid-cols-2">
          <Field label={t.name}>
            <Input
              value={editing.name[locale.slice(0, 2)] ?? ""}
              onChange={(event) =>
                change((value) => {
                  value.name[locale.slice(0, 2)] = event.target.value;
                  if (!value.name.en) value.name.en = event.target.value;
                })
              }
            />
          </Field>
          <div className="grid content-start gap-1.5">
            <Field label={t.category}>
              <Input
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
            </Field>
            <p className="text-[11px] text-faint">{t.categoryHint}</p>
          </div>
          <div className="sm:col-span-2">
            <Field label={t.description}>
              <Textarea
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
            </Field>
          </div>
          <Field label={t.tags}>
            <Input
              value={editing.tags.join(", ")}
              onChange={(event) =>
                change((value) => {
                  value.tags = split(event.target.value);
                })
              }
            />
          </Field>
          <Field label={t.icon}>
            <Input
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
          </Field>
        </div>
      </Section>
      <Section title={ja ? "指示" : "Instructions"}>
        <Field label={t.instructions}>
          <Textarea
            rows={6}
            value={editing.config.instructions}
            onChange={(event) =>
              change((value) => {
                value.config.instructions = event.target.value;
              })
            }
          />
        </Field>
      </Section>
      <Section title={ja ? "モデルと能力" : "Model and capabilities"}>
        <div className="grid items-start gap-4 sm:grid-cols-[minmax(0,1fr)_160px]">
          <Field label={t.model}>
            <NativeSelect
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
                    ? `${refKey(editing.config.model)} · ${ja ? "利用不可" : "Unavailable"}`
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
            </NativeSelect>
          </Field>
          <Field label={t.maxSteps}>
            <Input
              type="number"
              min={1}
              max={1000}
              className="font-mono tabular"
              value={editing.config.max_steps}
              onChange={(event) =>
                change((value) => {
                  value.config.max_steps = Number(event.target.value);
                })
              }
            />
          </Field>
          <div className="sm:col-span-2">
            <Field label={t.capabilities}>
              <Input
                value={editing.capabilities.join(", ")}
                onChange={(event) =>
                  change((value) => {
                    value.capabilities = split(event.target.value);
                  })
                }
              />
            </Field>
          </div>
        </div>
      </Section>
      <Section
        title={ja ? "連携" : "Integrations"}
        description={
          ja
            ? "ここで許可しても、既存の権限ポリシーは広がりません。"
            : "These settings never expand existing permissions."
        }
      >
        <AgentBindings
          value={editing.config}
          entries={data.registry}
          node={data.node.id}
          cluster={Boolean(editing.config.cluster)}
          change={(configuration) =>
            change((value) => {
              value.config.bindings = configuration.bindings;
              value.config.remove_default = configuration.remove_default;
            })
          }
        />
      </Section>
      <Section
        title={t.docs}
        action={
          <Button
            variant="outline"
            size="sm"
            type="button"
            onClick={() => {
              setDocuments((previous) => [
                ...previous,
                { name: "", media_type: "text/plain", text: "" },
              ]);
              setDirty(true);
            }}
          >
            <Plus aria-hidden />
            {ja ? "文書を追加" : "Add document"}
          </Button>
        }
      >
        {documents.length ? (
          documents.map((doc, index) => (
            <div
              key={index}
              className="grid gap-3 rounded-lg border border-border bg-surface p-3"
            >
              <div className="flex items-end gap-2">
                <div className="min-w-0 flex-1">
                  <Field label={t.docName}>
                    <Input
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
                  </Field>
                </div>
                <Button
                  variant="ghost"
                  size="icon"
                  type="button"
                  aria-label={
                    ja
                      ? `${doc.name || t.docName}を削除`
                      : `Remove ${doc.name || t.docName}`
                  }
                  onClick={() => {
                    setDocuments((previous) =>
                      previous.filter((_, i) => i !== index),
                    );
                    setDirty(true);
                  }}
                >
                  <X aria-hidden />
                </Button>
              </div>
              <Field label={t.docText}>
                <Textarea
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
              </Field>
            </div>
          ))
        ) : (
          <Hint>
            {ja
              ? "非公開参照テキストはありません。"
              : "No private reference text."}
          </Hint>
        )}
      </Section>
    </>
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
  const boundTargets = (kind: string) =>
    editing?.config.bindings
      .filter((binding) => binding.kind === kind)
      .map((binding) => binding.target) ?? [];
  const testedRevision =
    !!current &&
    !dirty &&
    testSessions.some(
      (session) =>
        session.revision === current.revision && session.status === "completed",
    );
  const creatorInspector = current && (
    <Inspector label={ja ? "下書きの状態" : "Draft status"}>
      <InspectorSection title={t.validate}>
        <div className="flex items-start gap-2 text-[13px] text-foreground">
          {validationCurrent && validation?.valid ? (
            <CheckCircle2
              aria-hidden
              className="mt-0.5 size-4 shrink-0 text-success"
            />
          ) : (
            <CircleAlert
              aria-hidden
              className={cn(
                "mt-0.5 size-4 shrink-0",
                validationCurrent ? "text-destructive" : "text-faint",
              )}
            />
          )}
          <p>
            {validationCurrent
              ? validation?.message
              : ja
                ? "この下書きの技術検証は未実施です。"
                : "This draft has not been validated."}
          </p>
        </div>
      </InspectorSection>
      {creatorTab !== "build" && (
        <InspectorSection
          title={ja ? "動作テスト" : "Behavioral test"}
          action={
            <Button
              variant="ghost"
              size="sm"
              type="button"
              onClick={() => setCreatorTab("test")}
            >
              {ja ? "テストを開く" : "Open test sandbox"}
            </Button>
          }
        >
          {latestSession && (
            <div className="flex items-center gap-2">
              <StatusWord value={latestSession.status} />
              <span className="font-mono text-[11px] tabular text-faint">
                r{latestSession.revision} ·{" "}
                {new Date(latestSession.created_at).toLocaleString(locale)}
              </span>
            </div>
          )}
          <Hint>
            {testedRevision
              ? ja
                ? "現在の版のテスト実行記録があります。"
                : "A completed test run exists for this revision."
              : t.noTests}
          </Hint>
          {creatorTab === "register" && (
            <p className="text-[11px] text-faint">{t.noAssessment}</p>
          )}
        </InspectorSection>
      )}
      <InspectorSection title={t.dependencies}>
        <ul className="grid gap-2">
          {editing?.config.model.id && (
            <li className="flex items-start gap-2">
              <Blocks aria-hidden className="mt-0.5 size-3.5 text-faint" />
              <span className="grid min-w-0">
                <span className="truncate text-[13px]">
                  {referenceName(editing.config.model)}
                </span>
                <small className="text-[11px] text-faint">{t.model}</small>
              </span>
            </li>
          )}
          {boundTargets("tool").map((ref) => (
            <li key={`tool-${refKey(ref)}`} className="flex items-start gap-2">
              <Wrench aria-hidden className="mt-0.5 size-3.5 text-faint" />
              <span className="grid min-w-0">
                <span className="truncate text-[13px]">
                  {referenceName(ref)}
                </span>
                <small className="text-[11px] text-faint">{t.tools}</small>
              </span>
            </li>
          ))}
          {boundTargets("skill").map((ref) => (
            <li key={`skill-${refKey(ref)}`} className="flex items-start gap-2">
              <BookOpen aria-hidden className="mt-0.5 size-3.5 text-faint" />
              <span className="grid min-w-0">
                <span className="truncate text-[13px]">
                  {referenceName(ref)}
                </span>
                <small className="text-[11px] text-faint">{t.skills}</small>
              </span>
            </li>
          ))}
        </ul>
        {!editing?.config.model.id && <Hint>{t.noModel}</Hint>}
      </InspectorSection>
      <InspectorSection title={ja ? "要求する能力" : "Requested capabilities"}>
        {editing?.capabilities.length ? (
          <ul className="flex flex-wrap gap-1.5">
            {editing.capabilities.map((capability) => (
              <li
                key={capability}
                className="rounded-sm bg-raised px-1.5 py-0.5 font-mono text-xs text-foreground"
              >
                {capability}
              </li>
            ))}
          </ul>
        ) : (
          <Hint>{ja ? "未設定" : "None selected"}</Hint>
        )}
        <p className="text-[11px] text-faint">{t.permission}</p>
      </InspectorSection>
      <InspectorSection title={ja ? "版の概要" : "Version summary"}>
        <Facts
          items={[
            [
              t.version,
              <span className="font-mono text-xs">
                {editing?.version} · r{baseRevision}
              </span>,
            ],
            [t.owner, current.owner],
            [
              ja ? "最終更新" : "Last updated",
              <span className="font-mono text-xs tabular">
                {new Date(current.updated_at).toLocaleString(locale)}
              </span>,
            ],
          ]}
        />
        {creatorTab === "register" ? (
          <div className="grid gap-1">
            <h3 className="text-xs text-faint">{t.release}</h3>
            <p className="whitespace-pre-wrap text-[13px] text-foreground">
              {releaseNotes || "—"}
            </p>
          </div>
        ) : (
          <Field label={t.release}>
            <Textarea
              rows={3}
              value={releaseNotes}
              onChange={(event) => {
                setReleaseNotes(event.target.value);
                setDirty(true);
              }}
            />
          </Field>
        )}
      </InspectorSection>
    </Inspector>
  );
  const versionsView = current && (
    <>
      <MainColumn wide>
        <Section
          title={t.versions}
          description={
            ja
              ? "Registryに登録済みの版です。選択すると保存済み下書きと比較します。"
              : "Versions registered in the Registry. Select one to compare with the saved draft."
          }
        >
          {registeredVersions.length ? (
            <ul className="divide-y divide-border rounded-lg border border-border bg-surface">
              {registeredVersions.map((version) => {
                const active =
                  selectedRegisteredVersion?.entry.version ===
                  version.entry.version;
                return (
                  <li key={version.entry.version}>
                    <button
                      type="button"
                      aria-pressed={active}
                      onClick={() => setSelectedVersion(version.entry.version)}
                      className={cn(
                        "flex h-10 w-full items-center gap-3 px-3 text-left text-[13px] transition-colors hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-ring",
                        active &&
                          "bg-brand-soft shadow-[inset_2px_0_0_var(--brand-mark)] hover:bg-brand-soft",
                      )}
                    >
                      <span className="font-mono text-foreground">
                        {version.entry.version}
                      </span>
                      <span className="font-mono text-xs text-faint">
                        {version.draft_revision === null
                          ? ja
                            ? "既存版"
                            : "Legacy"
                          : `r${version.draft_revision}`}
                      </span>
                      <span className="ml-auto font-mono text-[11px] tabular text-faint">
                        {version.registered_at
                          ? new Date(version.registered_at).toLocaleString(
                              locale,
                            )
                          : "—"}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          ) : (
            <EmptyState
              icon={<History />}
              title={
                ja
                  ? "最初の版を登録すると履歴が表示されます"
                  : "Register your first version to view history"
              }
            >
              {t.noVersions}
            </EmptyState>
          )}
        </Section>
        {selectedRegisteredVersion && (
          <Section
            title={ja ? "版履歴と差分" : "Version history & comparison"}
            description={`${ja ? "保存済み下書きとの比較" : "Compared with saved draft"} · r${current.revision}${dirty ? ` · ${t.dirty}` : ""}`}
          >
            <div className="rounded-lg border border-border bg-surface">
              <Table aria-label={ja ? "版の差分" : "Version comparison"}>
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-36">
                      {ja ? "項目" : "Field"}
                    </TableHead>
                    <TableHead className="font-mono">
                      {selectedRegisteredVersion.entry.version}
                    </TableHead>
                    <TableHead>{t.draft}</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {versionFields(
                    current.entry,
                    selectedRegisteredVersion.entry,
                    selectedRegisteredVersion.draft_knowledge_digest ?? null,
                    selectedRegisteredVersion.registered_knowledge_digest ??
                      null,
                  ).map(([name, draftValue, registeredValue]) => {
                    const changed =
                      JSON.stringify(draftValue) !==
                      JSON.stringify(registeredValue);
                    return (
                      <TableRow
                        key={name}
                        data-changed={changed || undefined}
                        className={changed ? "bg-warning-soft" : undefined}
                      >
                        <TableHead
                          scope="row"
                          className="h-auto py-2 align-top text-xs text-muted-foreground"
                        >
                          <span className="flex items-center gap-1.5">
                            {changed && <Dot tone="warning" />}
                            {name}
                          </span>
                        </TableHead>
                        {[registeredValue, draftValue].map((value, index) => (
                          <TableCell
                            key={index}
                            className="h-auto py-2 align-top"
                          >
                            <pre className="max-h-40 overflow-auto whitespace-pre-wrap break-words font-mono text-xs text-foreground">
                              {formatComparison(value)}
                            </pre>
                          </TableCell>
                        ))}
                      </TableRow>
                    );
                  })}
                </TableBody>
              </Table>
            </div>
          </Section>
        )}
      </MainColumn>
      <Inspector label={ja ? "版の詳細" : "Version details"}>
        <InspectorSection
          title={ja ? "版の詳細" : "Version details"}
          action={
            selectedRegisteredVersion && (
              <Button
                variant="ghost"
                size="sm"
                type="button"
                onClick={() =>
                  switchMode(
                    "trust",
                    `${selectedRegisteredVersion.entry.id}@${selectedRegisteredVersion.entry.version}`,
                  )
                }
              >
                <ShieldCheck aria-hidden />
                {t.trust}
              </Button>
            )
          }
        >
          {selectedRegisteredVersion ? (
            <Facts
              items={[
                [
                  ja ? "参照" : "Reference",
                  <span className="font-mono text-xs">
                    {selectedRegisteredVersion.entry.id}@
                    {selectedRegisteredVersion.entry.version}
                  </span>,
                ],
                [
                  ja ? "登録" : "Registered",
                  <span className="font-mono text-xs tabular">
                    {selectedRegisteredVersion.registered_at
                      ? new Date(
                          selectedRegisteredVersion.registered_at,
                        ).toLocaleString(locale)
                      : ja
                        ? "登録時刻は記録されていません"
                        : "Registration time unavailable"}{" "}
                    · {selectedRegisteredVersion.registered_by ?? "—"}
                  </span>,
                ],
                [
                  t.release,
                  selectedRegisteredVersion.release_notes ||
                    (ja ? "リリースノートなし" : "No release notes"),
                ],
                [
                  ja ? "動作テスト" : "Behavioral test",
                  selectedRegisteredVersion.behavioral_tested === true
                    ? ja
                      ? "登録時に完了済みの動作テストあり"
                      : "Completed behavioral test at registration"
                    : selectedRegisteredVersion.behavioral_tested === false
                      ? ja
                        ? "登録時に完了済みの動作テストなし"
                        : "No completed behavioral test at registration"
                      : ja
                        ? "既存版のテスト情報なし"
                        : "Legacy test provenance unavailable",
                ],
                ...(selectedRegisteredVersion.source_id
                  ? [
                      [
                        ja ? "元の版" : "Source",
                        <span className="font-mono text-xs">
                          {selectedRegisteredVersion.source_id}@
                          {selectedRegisteredVersion.source_version}
                        </span>,
                      ] satisfies [ReactNode, ReactNode],
                    ]
                  : []),
                [
                  ja
                    ? "保存済み下書きとの差分"
                    : "Differences from saved draft",
                  versionDifferences(
                    current.entry,
                    selectedRegisteredVersion.entry,
                    selectedRegisteredVersion.draft_knowledge_digest ?? null,
                    selectedRegisteredVersion.registered_knowledge_digest ??
                      null,
                  ).join(", ") || (ja ? "なし" : "None"),
                ],
              ]}
            />
          ) : (
            <Hint>
              {ja ? "版が選択されていません。" : "No version selected."}
            </Hint>
          )}
          <p className="text-[11px] text-faint">
            {ja
              ? "同じIDの新しい版は「Registryに登録」で版番号を変更して作成します。"
              : "For a new version under this ID, edit the version in Register in Registry."}
          </p>
        </InspectorSection>
        {canManageDraft && (
          <InspectorSection title={ja ? "所有と共有" : "Ownership and sharing"}>
            <Facts items={[[t.owner, current.owner]]} />
            {draftShares.length > 0 && (
              <ul className="divide-y divide-border">
                {draftShares.map((share) => (
                  <li
                    key={share.subject}
                    className="flex flex-wrap items-center gap-2 py-1.5"
                  >
                    <span className="font-mono text-xs text-foreground">
                      {share.subject}
                    </span>
                    <ToneBadge tone="neutral">
                      {share.can_edit
                        ? ja
                          ? "編集可"
                          : "Can edit"
                        : ja
                          ? "閲覧のみ"
                          : "Read only"}
                    </ToneBadge>
                    {!share.documents_current && (
                      <ToneBadge tone="warning">
                        {ja
                          ? "資料変更により再確認が必要"
                          : "Documents changed; re-share required"}
                      </ToneBadge>
                    )}
                    <Button
                      variant="ghost"
                      size="sm"
                      type="button"
                      className="ml-auto"
                      disabled={busy || dirty}
                      onClick={() =>
                        void updateShare(share.subject, false, share.can_edit)
                      }
                    >
                      {ja ? "解除" : "Remove"}
                    </Button>
                  </li>
                ))}
              </ul>
            )}
            <Field label={ja ? "共有先の主体" : "Share with subject"}>
              <Input
                value={shareSubject}
                onChange={(event) => setShareSubject(event.target.value)}
              />
            </Field>
            <Check
              checked={shareCanEdit}
              onChange={(event) => setShareCanEdit(event.target.checked)}
            >
              {ja ? "編集を許可" : "Allow editing"}
            </Check>
            {documents.length > 0 && (
              <Check
                checked={shareIncludesDocuments}
                onChange={(event) =>
                  setShareIncludesDocuments(event.target.checked)
                }
              >
                {ja
                  ? "非公開資料も共有することを確認"
                  : "Acknowledge sharing private documents"}
              </Check>
            )}
            <div>
              <Button
                variant="outline"
                size="sm"
                type="button"
                disabled={busy || dirty || !shareSubject.trim()}
                onClick={() =>
                  void updateShare(shareSubject, true, shareCanEdit)
                }
              >
                {ja ? "共有" : "Share"}
              </Button>
            </div>
            <div className="grid gap-2 border-t border-border pt-3">
              <Field label={ja ? "新しい所有者" : "New owner"}>
                <Input
                  value={transferOwner}
                  onChange={(event) => setTransferOwner(event.target.value)}
                />
              </Field>
              <div>
                <Button
                  variant="outline"
                  size="sm"
                  type="button"
                  disabled={busy || dirty || !transferOwner.trim()}
                  onClick={() => void changeOwner()}
                >
                  {ja ? "所有権を移す" : "Transfer ownership"}
                </Button>
              </div>
            </div>
          </InspectorSection>
        )}
      </Inspector>
    </>
  );
  const registerView = (
    <>
      <MainColumn>
        <Section title={t.register} description={t.permission}>
          <div className="max-w-48">
            <Field label={t.version}>
              <Input
                className="font-mono"
                value={editing?.version ?? ""}
                onChange={(event) =>
                  change((value) => {
                    value.version = event.target.value;
                  })
                }
              />
            </Field>
          </div>
          <Field label={t.release}>
            <Textarea
              rows={5}
              value={releaseNotes}
              onChange={(event) => {
                setReleaseNotes(event.target.value);
                setDirty(true);
              }}
            />
          </Field>
          <div>
            <Button
              type="button"
              disabled={busy}
              onClick={() => void act("register")}
            >
              <Send aria-hidden />
              {dirty ? `${t.save} + ${t.register}` : t.register}
            </Button>
          </div>
        </Section>
        <Section title={ja ? "登録内容の確認" : "Review draft"}>
          <Facts
            items={[
              [t.profile, editing ? label(editing, locale) : "—"],
              [
                t.model,
                editing?.config.model.id
                  ? referenceName(editing.config.model)
                  : t.noModel,
              ],
              [
                t.tools,
                boundTargets("tool").map(referenceName).join(", ") || "—",
              ],
              [
                t.skills,
                boundTargets("skill").map(referenceName).join(", ") || "—",
              ],
              [
                t.docs,
                <span className="font-mono tabular">{documents.length}</span>,
              ],
              [
                t.version,
                <span className="font-mono">{editing?.version || "—"}</span>,
              ],
            ]}
          />
          <Hint>
            {ja
              ? "登録するとRegistryに変更不可の版が作成されます。"
              : "Registration creates an immutable version in the Registry."}
          </Hint>
          {(!validationCurrent || !validation?.valid) && (
            <Hint>
              {ja
                ? "登録時にサーバーが権限と内容を検証します。"
                : "The server checks permissions and validates the draft when registering."}
            </Hint>
          )}
        </Section>
      </MainColumn>
      {creatorInspector}
    </>
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
  const decisionCell = (value: boolean | null) => (
    <TableCell
      className={cn(
        "text-xs font-medium shadow-[inset_0_0_0_2px_var(--surface)]",
        value === null
          ? "bg-neutral-soft text-muted-foreground"
          : value
            ? "bg-success-soft text-success"
            : "bg-destructive-soft text-destructive",
      )}
    >
      {value === null
        ? ja
          ? "対象外"
          : "N/A"
        : value
          ? ja
            ? "許可"
            : "Allowed"
          : ja
            ? "制限"
            : "Restricted"}
    </TableCell>
  );
  const incidentPanel = (
    <MainColumn wide>
      <Section
        title={t.incidents}
        description={
          ja
            ? "この版に関する報告記録です。安全性の評価ではありません。"
            : "Reports linked to this version. They are not a safety assessment."
        }
      >
        <div className="flex flex-wrap items-end gap-3">
          <div className="w-full sm:w-44">
            <Field label={ja ? "状態" : "Status"}>
              <NativeSelect
                value={incidentFilter}
                onChange={(event) => setIncidentFilter(event.target.value)}
              >
                <option value="all">{ja ? "すべて" : "All"}</option>
                <option value="open">{ja ? "未解決" : "Open"}</option>
                <option value="resolved">{ja ? "解決済み" : "Resolved"}</option>
                <option value="archived">
                  {ja ? "アーカイブ" : "Archived"}
                </option>
              </NativeSelect>
            </Field>
          </div>
          <div className="w-full sm:w-64">
            <Field label={ja ? "記録を検索" : "Search reports"}>
              <Input
                type="search"
                value={incidentSearch}
                onChange={(event) => setIncidentSearch(event.target.value)}
              />
            </Field>
          </div>
          <span className="ml-auto pb-2 font-mono text-xs tabular text-faint">
            {incidentsLoaded
              ? `${filteredIncidents.length} / ${incidents.length}`
              : ja
                ? "記録を取得中"
                : "Loading reports"}
          </span>
        </div>
        {filteredIncidents.length ? (
          <ul className="divide-y divide-border border-y border-border">
            {filteredIncidents.map((incident) => (
              <li
                key={incident.id}
                className="grid gap-2 py-3 sm:grid-cols-[80px_minmax(0,1fr)_auto] sm:gap-x-3"
              >
                <div className="flex items-start gap-1.5 sm:flex-col">
                  <ToneBadge
                    className="font-mono"
                    tone={
                      incident.status === "resolved"
                        ? "success"
                        : incident.severity === "high" ||
                            incident.severity === "critical"
                          ? "danger"
                          : "warning"
                    }
                  >
                    {incident.severity}
                  </ToneBadge>
                  <StatusWord value={incident.status} />
                </div>
                <div className="grid min-w-0 gap-1">
                  <p className="break-words text-[13px] text-foreground">
                    {incident.notes}
                  </p>
                  <p className="font-mono text-[11px] text-faint">
                    {incident.id} ·{" "}
                    {new Date(incident.created_at).toLocaleString(locale)} ·{" "}
                    {t.owner}: {incident.owner} · r{incident.revision}
                    {incident.archived
                      ? ` · ${ja ? "アーカイブ済み" : "Archived"}`
                      : ""}
                  </p>
                  {incident.evidence.length > 0 && (
                    <ul className="grid gap-0.5">
                      {incident.evidence.map((evidence, index) => (
                        <li
                          key={index}
                          className="font-mono text-[11px] text-muted-foreground"
                        >
                          {evidence.title} · SHA-256{" "}
                          {evidence.sha256.slice(0, 12)}
                          {incident.evidence_expired_at
                            ? ` · ${ja ? "期限切れ" : "expired"}`
                            : ""}
                        </li>
                      ))}
                    </ul>
                  )}
                </div>
                <div className="flex gap-1.5 sm:self-start">
                  <Button
                    variant="outline"
                    size="sm"
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
                      ? ja
                        ? "再オープン"
                        : "Reopen"
                      : ja
                        ? "解決済みにする"
                        : "Resolve"}
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
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
                      ? ja
                        ? "アーカイブ解除"
                        : "Unarchive"
                      : ja
                        ? "アーカイブ"
                        : "Archive"}
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        ) : (
          <EmptyState
            icon={<FileText />}
            title={
              !incidentsLoaded
                ? ja
                  ? "記録はまだ取得できていません。"
                  : "Reports are not available yet."
                : incidents.length
                  ? ja
                    ? "検索条件に一致する記録はありません。"
                    : "No reports match these filters."
                  : ja
                    ? "この権限で閲覧できる報告記録はありません。安全性の評価を意味しません。"
                    : "No report is visible with this access. This is not a safety assessment."
            }
          />
        )}
      </Section>
      <Section title={ja ? "報告を追加" : "Add a report"}>
        <div className="grid max-w-3xl gap-4 sm:grid-cols-2">
          {isOperator && (
            <Field label={t.operatorTenant}>
              <Input
                value={tenant}
                onChange={(event) => setTenant(event.target.value)}
              />
            </Field>
          )}
          <Field label={t.owner}>
            <Input
              value={incidentOwner}
              onChange={(event) => setIncidentOwner(event.target.value)}
              placeholder={
                !isOperator && data.access.kind === "subject"
                  ? data.access.subject
                  : ""
              }
            />
          </Field>
          <Field label={ja ? "報告した重大度" : "Reported severity"}>
            <NativeSelect
              value={incidentSeverity}
              onChange={(event) => setIncidentSeverity(event.target.value)}
            >
              <option value="low">Low</option>
              <option value="medium">Medium</option>
              <option value="high">High</option>
              <option value="critical">Critical</option>
            </NativeSelect>
          </Field>
          <div className="sm:col-span-2">
            <Field label={ja ? "内容" : "Notes"}>
              <Textarea
                rows={4}
                value={incidentNotes}
                onChange={(event) => setIncidentNotes(event.target.value)}
              />
            </Field>
          </div>
          <Disclosure
            className="sm:col-span-2"
            summary={ja ? "証拠（任意）" : "Evidence (optional)"}
          >
            <Field
              label={ja ? "証拠の名前（任意）" : "Evidence title (optional)"}
            >
              <Input
                value={evidenceTitle}
                onChange={(event) => setEvidenceTitle(event.target.value)}
              />
            </Field>
            <Field
              label={
                ja
                  ? "証拠の固定コピー（任意）"
                  : "Fixed evidence copy (optional)"
              }
            >
              <Textarea
                rows={3}
                value={evidenceContent}
                onChange={(event) => setEvidenceContent(event.target.value)}
              />
            </Field>
            {Boolean(evidenceTitle.trim()) !==
              Boolean(evidenceContent.trim()) && (
              <Hint role="status" className="text-warning">
                {ja
                  ? "証拠を添付する場合は、名前と本文の両方を入力してください。"
                  : "Enter both an evidence title and content to attach evidence."}
              </Hint>
            )}
          </Disclosure>
        </div>
        <div>
          <Button
            type="button"
            disabled={
              busy ||
              !incidentNotes.trim() ||
              Boolean(evidenceTitle.trim()) !== Boolean(evidenceContent.trim())
            }
            onClick={() => void createIncident()}
          >
            {ja ? "報告を記録" : "Record incident"}
          </Button>
        </div>
      </Section>
    </MainColumn>
  );
  const allowedCount = permissionContext?.rows.filter(
    (row) => row.effective_for_component,
  ).length;
  const policyPanel = (
    <MainColumn wide>
      <div className="wb-policy grid grid-cols-[minmax(0,1fr)] gap-6">
        <Section
          title={ja ? "権限の確認条件" : "Permission context"}
          description={t.policyContext}
        >
          <div className="grid max-w-3xl gap-3 sm:grid-cols-3">
            <Field label={t.operatorTenant}>
              <Input
                value={policyTenant}
                disabled={!isOperator}
                onChange={(event) => setPolicyTenant(event.target.value)}
              />
            </Field>
            <Field label={ja ? "主体" : "Subject"}>
              <Input
                value={policySubject}
                disabled={!isOperator}
                onChange={(event) => setPolicySubject(event.target.value)}
              />
            </Field>
            <Field
              label={
                ja ? "ワークスペースID（任意）" : "Workspace ID (optional)"
              }
            >
              <Input
                value={policyWorkspace}
                onChange={(event) => setPolicyWorkspace(event.target.value)}
              />
            </Field>
          </div>
          <div>
            <Button
              type="button"
              disabled={busy || !policyTenant || !policySubject}
              onClick={() => void evaluatePermissions()}
            >
              {ja ? "この条件で権限を確認" : "Check this context"}
            </Button>
          </div>
        </Section>
        <Section title={ja ? "判定の概要" : "Decision summary"}>
          <MetricRow columns={4} label={ja ? "判定の概要" : "Decision summary"}>
            <Metric
              label={ja ? "許可" : "Allowed"}
              tone="success"
              value={allowedCount ?? "—"}
              caption={ja ? "この部品で有効" : "Effective for component"}
            />
            <Metric
              label={ja ? "制限" : "Restricted"}
              tone="danger"
              value={
                permissionContext && allowedCount !== undefined
                  ? permissionContext.rows.length - allowedCount
                  : "—"
              }
              caption={ja ? "この部品で無効" : "Not effective"}
            />
            <Metric
              className="col-span-2"
              label={ja ? "ポリシー改訂" : "Policy revision"}
              value={permissionContext?.policy_revision ?? "—"}
              caption={ja ? "確認した条件" : "Checked context"}
            />
          </MetricRow>
          {permissionContext && (
            <>
              <p className="font-mono text-[11px] text-faint">
                {permissionContext.tenant} / {permissionContext.subject} /{" "}
                {permissionContext.workspace_id ||
                  (ja ? "ワークスペース未指定" : "No workspace selected")}{" "}
                ·{" "}
                {new Date(permissionContext.observed_at).toLocaleString(locale)}
              </p>
              <Hint>
                {ja ? "宣言した能力" : "Declared capabilities"}:{" "}
                {permissionContext.requested_capabilities.join(", ") || "—"}
              </Hint>
            </>
          )}
        </Section>
        <Section
          title={ja ? "権限マトリクス" : "Permission matrix"}
          description={
            ja
              ? "表示される権限は、確認した条件にのみ適用されます。"
              : "Permissions apply only to the checked context."
          }
        >
          {permissionContext ? (
            <>
              <div className="flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-faint">
                {(
                  [
                    ["bg-success-soft", ja ? "許可" : "Allowed"],
                    ["bg-destructive-soft", ja ? "制限" : "Restricted"],
                    ["bg-neutral-soft", ja ? "対象外" : "N/A"],
                  ] as const
                ).map(([swatch, name]) => (
                  <span key={name} className="inline-flex items-center gap-1.5">
                    <span
                      aria-hidden
                      className={cn("size-2.5 rounded-sm", swatch)}
                    />
                    {name}
                  </span>
                ))}
              </div>
              <div className="rounded-lg border border-border bg-surface">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>{t.dependencies}</TableHead>
                      <TableHead>{ja ? "操作" : "Action"}</TableHead>
                      <TableHead>Catalog</TableHead>
                      <TableHead>Policy</TableHead>
                      <TableHead>
                        {ja ? "Registry参照" : "Registry read"}
                      </TableHead>
                      <TableHead>
                        {ja ? "この部品で有効" : "Effective component"}
                      </TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {permissionContext.rows.map((row) => (
                      <TableRow
                        key={`${row.kind}:${refKey(row.reference)}`}
                        className="hover:bg-transparent"
                      >
                        <TableCell className="font-mono text-xs">
                          {row.kind} · {refKey(row.reference)}
                        </TableCell>
                        <TableCell className="font-mono text-xs text-muted-foreground">
                          {row.action}
                        </TableCell>
                        {decisionCell(row.catalog_enabled)}
                        {decisionCell(row.policy_allowed)}
                        {decisionCell(row.registry_read_allowed)}
                        {decisionCell(row.effective_for_component)}
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>
              <Hint>
                {permissionContext.workspace_id
                  ? `${t.workspaces}: ${permissionContext.workspace_read ? "✓" : "—"}`
                  : ja
                    ? "ワークスペース未指定"
                    : "No workspace selected"}
              </Hint>
              <Hint>{permissionContext.note}</Hint>
            </>
          ) : (
            <EmptyState
              icon={<ShieldCheck />}
              title={
                ja
                  ? "条件を指定して権限を確認"
                  : "Choose a context to inspect permissions"
              }
            >
              {ja
                ? "テナントと主体を指定して、権限の確認を実行してください。"
                : "Select a tenant and subject above to view permission details."}
            </EmptyState>
          )}
        </Section>
      </div>
    </MainColumn>
  );

  const archiveLabel = current?.archived
    ? ja
      ? "復元"
      : "Restore"
    : ja
      ? "下書きをアーカイブ"
      : "Archive draft";
  const pageClass =
    "flex h-full min-h-0 min-w-0 flex-col overflow-y-auto bg-background lg:overflow-hidden";
  const notices = (
    <>
      {integratedTools && (
        <div className="shrink-0 overflow-y-auto border-b border-border px-4 md:px-6 lg:max-h-[55%]">
          {integratedTools}
        </div>
      )}
      {error && (
        <Alert
          retryLabel={t.refresh}
          retry={
            mode === "creator"
              ? () => void reload()
              : () => setInspectionAttempt((attempt) => attempt + 1)
          }
          className="shrink-0 rounded-none border-x-0 border-t-0 px-4 md:px-6"
        >
          {error}
        </Alert>
      )}
      {message && (
        <Notice
          role="status"
          tone="info"
          className="shrink-0 rounded-none border-x-0 border-t-0 px-4 md:px-6"
        >
          {message}
        </Notice>
      )}
    </>
  );

  if (mode === "creator")
    return (
      <div className={pageClass}>
        <ScreenHeader
          title={
            editing ? label(editing, locale) || t.new : "Creator Workbench"
          }
          meta={
            current
              ? `${t.draft} · ${current.tenant} / ${current.owner} · r${current.revision}`
              : t.select
          }
          status={
            current && (
              <SaveState dirty={dirty} label={dirty ? t.dirty : t.saved} />
            )
          }
          actions={
            current && (
              <>
                <Button
                  variant={dirty ? "default" : "outline"}
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
                  type="button"
                  disabled={busy}
                  onClick={() => setCreatorTab("register")}
                >
                  {ja ? "登録内容を確認" : "Review registration"}
                </Button>
              </>
            )
          }
        />
        <div
          role="toolbar"
          aria-label={ja ? "下書きの選択・管理" : "Select & manage drafts"}
          className="flex shrink-0 flex-wrap items-end gap-x-2 gap-y-2 border-b border-border bg-surface px-4 py-2 md:px-6"
        >
          <div className="w-full sm:w-72">
            <Field label={t.select}>
              <NativeSelect
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
              </NativeSelect>
            </Field>
          </div>
          {isOperator && (
            <>
              <div className="w-[calc(50%-4px)] sm:w-32">
                <Field label={t.operatorTenant}>
                  <Input
                    value={tenant}
                    onChange={(event) => setTenant(event.target.value)}
                  />
                </Field>
              </div>
              <div className="w-[calc(50%-4px)] sm:w-32">
                <Field label={t.operatorOwner}>
                  <Input
                    value={owner}
                    onChange={(event) => setOwner(event.target.value)}
                  />
                </Field>
              </div>
            </>
          )}
          <Button
            variant="outline"
            type="button"
            disabled={busy || !models.length}
            onClick={() => void create()}
          >
            <Plus aria-hidden />
            {t.create}
          </Button>
          {current && (
            <Button
              variant="ghost"
              size="icon"
              type="button"
              disabled={busy}
              aria-label={ja ? "新しいIDに複製" : "Duplicate with new ID"}
              title={ja ? "新しいIDに複製" : "Duplicate with new ID"}
              onClick={() => void duplicateDraft()}
            >
              <Copy aria-hidden />
            </Button>
          )}
          {current && canManageDraft && (
            <Button
              variant="ghost"
              size="icon"
              type="button"
              disabled={busy || dirty}
              aria-label={archiveLabel}
              title={archiveLabel}
              onClick={() => void toggleArchive()}
            >
              {current.archived ? (
                <ArchiveRestore aria-hidden />
              ) : (
                <Archive aria-hidden />
              )}
            </Button>
          )}
          <Button
            variant="ghost"
            size="icon"
            type="button"
            aria-label={t.refresh}
            title={t.refresh}
            onClick={() => void reload()}
          >
            <RefreshCw aria-hidden />
          </Button>
          {isOperator && agents.length > 0 && (
            <div className="flex w-full flex-wrap items-end gap-2 sm:w-auto lg:ml-auto">
              <div className="min-w-0 flex-1 sm:w-56 sm:flex-none">
                <Field
                  label={
                    ja ? "既存エージェントを割り当て" : "Assign existing agent"
                  }
                >
                  <NativeSelect
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
                  </NativeSelect>
                </Field>
              </div>
              <Button
                variant="outline"
                type="button"
                disabled={busy || !adoptRef || !tenant || !owner}
                onClick={() => void adoptLegacy()}
              >
                {ja ? "Creatorに割り当て" : "Assign to Creator"}
              </Button>
            </div>
          )}
        </div>
        {notices}
        {!models.length && (
          <Notice
            role="status"
            tone="warning"
            className="shrink-0 rounded-none border-x-0 border-t-0 px-4 md:px-6"
          >
            {t.noModels}
          </Notice>
        )}
        {loading ? (
          <FormSkeleton label={t.loading} />
        ) : !current ? (
          <div className="px-4 py-6 md:px-6">
            <div className="max-w-[760px]">
              <EmptyState
                icon={<FileText />}
                title={
                  drafts.length
                    ? ja
                      ? "下書きを選択してください"
                      : "Select a draft to edit"
                    : t.noDrafts
                }
              >
                {drafts.length
                  ? ja
                    ? "上の一覧から下書きを選ぶか、新しい下書きを作成します。"
                    : "Choose a draft above, or create a new one."
                  : ja
                    ? "新しい下書きを作成すると、ここで編集・テスト・登録できます。"
                    : "Create a draft to edit, test and register it here."}
              </EmptyState>
            </div>
          </div>
        ) : (
          <Tabs
            value={creatorTab}
            onValueChange={(value) => setCreatorTab(value as CreatorTab)}
            className="flex min-w-0 flex-col lg:min-h-0 lg:flex-1"
          >
            <TabsList aria-label="Creator" className="md:px-6">
              <TabsTrigger value="overview">{t.overview}</TabsTrigger>
              <TabsTrigger value="build">{t.build}</TabsTrigger>
              <TabsTrigger value="test">{t.test}</TabsTrigger>
              <TabsTrigger value="versions">{t.versions}</TabsTrigger>
              <TabsTrigger value="register">{t.register}</TabsTrigger>
            </TabsList>
            <TabsContent
              value={creatorTab}
              className={cn("wb-layout", creatorTab, bodyClass)}
            >
              {(creatorTab === "overview" || creatorTab === "build") && (
                <>
                  <MainColumn>{editor}</MainColumn>
                  {creatorInspector}
                </>
              )}
              {creatorTab === "test" && (
                <>
                  <MainColumn>
                    {testEnvironment}
                    {sandbox}
                  </MainColumn>
                  {testInspector}
                </>
              )}
              {creatorTab === "versions" && versionsView}
              {creatorTab === "register" && registerView}
            </TabsContent>
          </Tabs>
        )}
      </div>
    );

  return (
    <div className={pageClass}>
      <ScreenHeader
        title={selectedAgent ? label(selectedAgent, locale) : "Trust Workbench"}
        meta={
          selectedAgent
            ? `${selectedAgent.id} · ${selectedAgent.version} · ${t.source}: ${data.node.id}`
            : t.safe
        }
        actions={
          selectedAgent && (
            <>
              <Button
                variant="outline"
                type="button"
                onClick={() => void exportReport("json")}
              >
                <Download aria-hidden />
                {ja ? "レポート JSON" : "Export JSON"}
              </Button>
              <Button
                variant="outline"
                type="button"
                onClick={() => void exportReport("html")}
              >
                <Download aria-hidden />
                HTML / PDF
              </Button>
              <Button
                variant="ghost"
                type="button"
                onClick={() => switchMode("creator", focus)}
              >
                {t.creator}
              </Button>
            </>
          )
        }
      />
      <div className="flex shrink-0 flex-wrap items-end gap-3 border-b border-border bg-surface px-4 py-2 md:px-6">
        <div className="w-full sm:w-80">
          <Field label={t.registeredVersion}>
            <NativeSelect
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
            </NativeSelect>
          </Field>
        </div>
      </div>
      {notices}
      {!selectedAgent ? (
        focus.includes("@") && !error ? (
          <OverviewSkeleton label={t.loading} />
        ) : (
          <div className="px-4 py-6 md:px-6">
            <div className="max-w-[760px]">
              <EmptyState
                icon={<ShieldCheck />}
                title={
                  agents.length
                    ? ja
                      ? "登録済みの版を選択してください"
                      : "Select a registered version"
                    : t.emptyTrust
                }
              >
                {t.noAssessment}
              </EmptyState>
            </div>
          </div>
        )
      ) : (
        <Tabs
          value={trustTab}
          onValueChange={(value) => setTrustTab(value as TrustTab)}
          className="flex min-w-0 flex-col lg:min-h-0 lg:flex-1"
        >
          <TabsList aria-label="Trust" className="md:px-6">
            <TabsTrigger value="overview">{t.overview}</TabsTrigger>
            <TabsTrigger value="policies">{t.policies}</TabsTrigger>
            <TabsTrigger value="audit">{t.audit}</TabsTrigger>
            <TabsTrigger value="certifications">{t.certifications}</TabsTrigger>
            <TabsTrigger value="incidents">{t.incidents}</TabsTrigger>
          </TabsList>
          {trustTab === "overview" ? (
            <TabsContent
              value="overview"
              className="min-w-0 lg:min-h-0 lg:flex-1 lg:overflow-y-auto"
            >
              <TrustOverview
                agent={selectedAgent}
                inspection={inspection}
                incidents={incidents}
                incidentsLoaded={incidentsLoaded}
                permissions={permissionContext}
                locale={locale}
                navigate={setTrustTab}
              />
            </TabsContent>
          ) : (
            <TabsContent value={trustTab} className={bodyClass}>
              {trustTab === "audit" ? (
                <TrustAudit
                  page={auditContextKey === currentAuditKey ? auditPage : null}
                  error={auditContextKey === currentAuditKey ? auditError : ""}
                  locale={locale}
                  offset={auditOffset}
                  inspection={inspection}
                  version={selectedAgent.version}
                  onOffset={(offset) => {
                    setAuditPage(null);
                    setAuditOffset(offset);
                  }}
                  tenantControl={
                    isOperator ? (
                      <div className="w-full sm:w-48">
                        <Field label={t.operatorTenant}>
                          <Input
                            value={policyTenant}
                            onChange={(event) => {
                              setAuditPage(null);
                              setAuditOffset(0);
                              setPolicyTenant(event.target.value);
                            }}
                          />
                        </Field>
                      </div>
                    ) : null
                  }
                />
              ) : (
                <>
                  {trustTab === "policies" && policyPanel}
                  {trustTab === "certifications" && (
                    <MainColumn>
                      <TrustCertifications
                        locale={locale}
                        navigate={setTrustTab}
                      />
                    </MainColumn>
                  )}
                  {trustTab === "incidents" && incidentPanel}
                  <Inspector label={ja ? "Trustサマリー" : "Trust summary"}>
                    <TrustSummary
                      inspection={inspection}
                      version={selectedAgent.version}
                      locale={locale}
                      audit={false}
                    />
                  </Inspector>
                </>
              )}
            </TabsContent>
          )}
        </Tabs>
      )}
    </div>
  );
}
