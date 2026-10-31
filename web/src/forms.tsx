import { contextSourceConfiguration } from "./context-source-model";
import { selfHostedPlanConnections } from "@slots/plan-connections";
import { coordinatorDefaults, hasInstructionalBinding } from "./agent-bindings";
import { registryDeployment } from "./node-selection";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import {
  Alert,
  Check,
  Group,
  Hint,
  Notice,
  formClass,
  multiSelectClass,
  pairClass,
} from "./components/patterns";
import { MemoryRegistryFields, memoryConfiguration } from "./memory-registry";
import { ApiError, apiFetch } from "./transport";
import {
  HomeNativeMemoryFields,
  nativeMemoryRequest,
} from "./remote-native-memory";
import {
  CapabilityConfiguration,
  emptyCore,
} from "./capabilities/configuration";
import { ReferenceName } from "./record-view";
import { AgentDocuments } from "./agent-documents";
import { AgentPluginImport } from "./agent-plugin-import";
import { agentPlugins } from "./agent-plugins";
import type { ReferenceDocument } from "./generated/models";
import { useRef, useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { EntityConfiguration } from "./entity-configuration";
import { useNodes } from "./nodes";
import { RegistryModelPicker } from "./registry-model-picker";
import { PlanModelPicker } from "./plan-model-picker";
import { OpenRouterModelPicker } from "./openrouter-model-picker";
import { useForm } from "@tanstack/react-form";
import { Field, useAgentLabel, useEntityLabel, useI18n } from "./ui";
import type { State, EntityRef, Discovery, Task } from "./types";
import {
  conversationCreate,
  workspaceCreate,
  taskCreate,
  registryCreate,
  personalAgentCreate,
  taskDelegate,
} from "./generated/aidash";
export type Submit = (request: () => Promise<unknown>) => Promise<boolean>;
/**
 * Where a new Workspace goes: the Node the person works in, unless the Tenant
 * has others to choose from. `field` is that choice (absent while there is
 * nothing to choose) and `follow` moves the dashboard to the Node chosen.
 */
function useNodeChoice(): {
  node: string | undefined;
  field: ReactNode;
  follow: () => void;
} {
  const { t } = useI18n();
  const { nodes, selected, select } = useNodes();
  const [chosen, setChosen] = useState<string>();
  const node = chosen ?? selected?.name;
  return {
    node,
    follow: () => {
      if (node) select(node);
    },
    field: nodes.length > 1 && (
      <Field label={t("node")}>
        <NativeSelect
          name="node"
          value={node ?? ""}
          onChange={(event) => setChosen(event.target.value)}
        >
          {nodes.map((value) => (
            <option key={value.name} value={value.name}>
              {value.label}
            </option>
          ))}
        </NativeSelect>
      </Field>
    ),
  };
}
const split = (s: string) =>
  s
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
const ref = (s: string): EntityRef => {
  const index = s.lastIndexOf("@");
  return { id: s.slice(0, index), version: s.slice(index + 1) };
};
// Cloudflare's Clef on its three zero-retention OpenRouter hosts, asked only
// when Jev is unavailable (ADR 0066). Its threshold 0.2 (as binary64
// bits) comes from docs/decisions/decider-route-thresholds.md, and only Jev
// has a long-context mode.
const CLEF_ROUTE = {
  provider_contract: "openrouter.decisions/1",
  model: "cloudflare/clef",
  hosts: ["cloudflare", "deepinfra", "primeintellect"],
  threshold: "3fc999999999999a",
  state_budget: {
    default_request_bytes: 32000,
    default_state_bytes: 24000,
    long_context_request_bytes: null,
    long_context_state_bytes: null,
    input_stages: [null, 200, 60],
  },
};
const deciderContracts: Record<
  string,
  { builder: string; option_source: string; rule: string }
> = {
  compaction: {
    builder: "aidash.compaction/2",
    option_source: "history_event/1",
    rule: "compaction.keep/1",
  },
  approval_observation: {
    builder: "aidash.approval-observation/1",
    option_source: "pending_action/1",
    rule: "approval.observe/1",
  },
  context_selection: {
    builder: "aidash.context-selection/1",
    option_source: "source_candidate/1",
    rule: "context-source.keep/1",
  },
};
function probabilityBits(value: number): string {
  if (!Number.isFinite(value) || value <= 0 || value > 0.5)
    throw new Error("Enter a Route threshold above 0 and at most 0.5");
  const view = new DataView(new ArrayBuffer(8));
  view.setFloat64(0, value);
  return view.getBigUint64(0).toString(16).padStart(16, "0");
}
export function GoalForm({ data, submit }: { data: State; submit: Submit }) {
  const { t } = useI18n();
  const where = useNodeChoice();
  const entityLabel = useEntityLabel(data.registry);
  const targets = data.registry.filter((e) =>
    ["agent", "cluster"].includes(e.kind),
  );
  const form = useForm({
    defaultValues: { title: "", goal: "", target: "" },
    onSubmit: async ({ value }) => {
      const target = targets.find(
        (e) => `${e.id}@${e.version}` === value.target,
      );
      if (!target) throw new Error(t("choose"));
      if (
        await submit(() =>
          conversationCreate({
            ...value,
            target: ref(value.target),
            target_kind: target.kind,
            node: where.node,
          }),
        )
      )
        where.follow();
    },
  });
  return (
    <form
      className={formClass}
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field name="title">
        {(f) => (
          <Field label={t("title")}>
            <Input
              required
              value={f.state.value}
              onChange={(e) => f.handleChange(e.target.value)}
              placeholder={t("goalTitle")}
            />
          </Field>
        )}
      </form.Field>
      <form.Field name="goal">
        {(f) => (
          <Field label={t("goal")}>
            <Textarea
              required
              value={f.state.value}
              onChange={(e) => f.handleChange(e.target.value)}
              placeholder={t("goalPlaceholder")}
              rows={5}
            />
          </Field>
        )}
      </form.Field>
      <form.Field name="target">
        {(f) => (
          <Field label={t("target")}>
            <NativeSelect
              required
              value={f.state.value}
              onChange={(e) => f.handleChange(e.target.value)}
            >
              <option value="">{t("choose")}</option>
              {targets.map((e) => (
                <option
                  key={`${e.id}@${e.version}`}
                  value={`${e.id}@${e.version}`}
                >
                  {entityLabel(e)} ({t(e.kind)})
                </option>
              ))}
            </NativeSelect>
          </Field>
        )}
      </form.Field>
      {where.field}
      <form.Subscribe selector={(s) => s.isSubmitting}>
        {(pending) => (
          <Button
            disabled={pending || targets.length === 0}
            className="justify-self-end"
            type="submit"
          >
            {t("newGoal")}
          </Button>
        )}
      </form.Subscribe>
    </form>
  );
}
export function WorkspaceForm({ submit }: { submit: Submit }) {
  const { t } = useI18n();
  const where = useNodeChoice();
  return (
    <form
      className={formClass}
      onSubmit={(e) => {
        e.preventDefault();
        const d = new FormData(e.currentTarget);
        void submit(() =>
          workspaceCreate({
            title: String(d.get("title")),
            goal: String(d.get("goal")),
            node: where.node,
          }),
        ).then((created) => {
          if (created) where.follow();
        });
      }}
    >
      <Field label={t("title")}>
        <Input name="title" required />
      </Field>
      <Field label={t("goal")}>
        <Textarea name="goal" rows={4} required />
      </Field>
      {where.field}
      <Button className="justify-self-end">{t("create")}</Button>
    </form>
  );
}
export function TaskForm({
  data,
  submit,
  workspace,
}: {
  data: State;
  submit: Submit;
  workspace?: string;
}) {
  const { t } = useI18n();
  const [selectedWorkspace, setSelectedWorkspace] = useState(workspace ?? "");
  const [parent, setParent] = useState("");
  const tasks = data.tasks.filter(
    (task) => task.workspace_id === selectedWorkspace,
  );
  const ancestors = new Set<string>();
  let ancestor: string | null | undefined = parent;
  while (ancestor && !ancestors.has(ancestor)) {
    ancestors.add(ancestor);
    ancestor = tasks.find((task) => task.id === ancestor)?.parent_id;
  }
  return (
    <form
      className={formClass}
      onSubmit={(e) => {
        e.preventDefault();
        const d = new FormData(e.currentTarget);
        let requirements;
        try {
          requirements = JSON.parse(String(d.get("requirements")));
        } catch {
          e.currentTarget
            .querySelector<HTMLTextAreaElement>('textarea[name="requirements"]')
            ?.setCustomValidity(t("jsonHint"));
          return;
        }
        void submit(() =>
          taskCreate(String(d.get("workspace")), {
            title: String(d.get("title")),
            description: String(d.get("description")),
            requirements,
            dependencies: d.getAll("dependencies").map(String),
            parent_id: parent || null,
          }),
        );
      }}
    >
      <Field label={t("workspace")}>
        <NativeSelect
          name="workspace"
          required
          value={selectedWorkspace}
          onChange={(event) => {
            setSelectedWorkspace(event.target.value);
            setParent("");
          }}
        >
          <option value="">{t("choose")}</option>
          {data.workspaces.map((w) => (
            <option key={w.id} value={w.id}>
              {w.title}
            </option>
          ))}
        </NativeSelect>
      </Field>
      <Field label={t("title")}>
        <Input name="title" required />
      </Field>
      <Field label={t("description")}>
        <Textarea name="description" required rows={3} />
      </Field>
      <Field label={t("parentTask")}>
        <NativeSelect
          name="parent_id"
          value={parent}
          onChange={(event) => setParent(event.target.value)}
        >
          <option value="">{t("noParent")}</option>
          {tasks.map((task) => (
            <option key={task.id} value={task.id}>
              {task.title}
            </option>
          ))}
        </NativeSelect>
      </Field>
      <Field label={t("dependencies")}>
        <select
          className={multiSelectClass}
          name="dependencies"
          multiple
          key={`${selectedWorkspace}:${parent}`}
        >
          {tasks
            .filter((task) => !ancestors.has(task.id))
            .map((task) => (
              <option key={task.id} value={task.id}>
                {task.title}
              </option>
            ))}
        </select>
      </Field>
      <Field label={t("requirements")}>
        <Textarea
          name="requirements"
          defaultValue={'{"capability":"web.search","language":"ja"}'}
          onChange={(e) => e.currentTarget.setCustomValidity("")}
        />
      </Field>
      <Button className="justify-self-end">{t("create")}</Button>
    </form>
  );
}
export function EntityForm({
  data,
  submit,
  initial = "agent",
}: {
  data: State;
  submit: Submit;
  initial?: string;
}) {
  const { t, locale } = useI18n();
  const ja = locale === "ja-JP";
  const [kind, setKind] = useState(initial);
  const registration = useRef<{ body: string; key: string } | null>(null);
  const [modelSource, setModelSource] = useState("openrouter");
  const [modelName, setModelName] = useState("");
  const [customModelName, setCustomModelName] = useState<string | null>(null);
  const [defaultName] = useState(() => {
    const adjectives = [
      "calm",
      "bright",
      "clever",
      "gentle",
      "swift",
      "curious",
      "nimble",
      "brave",
    ];
    const animals = [
      "otter",
      "fox",
      "owl",
      "panda",
      "dolphin",
      "falcon",
      "lynx",
      "badger",
    ];
    const values = crypto.getRandomValues(new Uint32Array(2));
    return `${adjectives[values[0] % adjectives.length]}-${animals[values[1] % animals.length]}`;
  });
  const [error, setError] = useState("");
  const models = data.registry.filter((e) => e.kind === "model");
  const [documents, setDocuments] = useState<ReferenceDocument[]>([]);
  const [core, setCore] = useState(emptyCore);
  const [pluginConfig, setPluginConfig] = useState<Record<string, unknown>>({});
  const [coordinator, setCoordinator] = useState(false);
  const [deciderHook, setDeciderHook] = useState("compaction");
  const [readingDocuments, setReadingDocuments] = useState(false);
  return (
    <form
      className={formClass}
      onSubmit={(e) => {
        e.preventDefault();
        setError("");
        const d = new FormData(e.currentTarget);
        const s = (k: string) => String(d.get(k) ?? "");
        let config: Record<string, unknown>;
        try {
          if (kind === "model" && !s("model_id"))
            throw new Error(t("modelChoose"));
          config =
            kind === "agent"
              ? {
                  model: ref(s("model")),
                  instructions: s("instructions"),
                  schema_version: 1,
                  max_steps: 64,
                  ...core,
                  ...pluginConfig,
                }
              : kind === "model"
                ? {
                    provider:
                      modelSource === "keyless" ? "openrouter" : modelSource,
                    ...(modelSource === "chatgpt"
                      ? {
                          plan_connection: "chatgpt",
                          streaming: true,
                          projection_versions: ["legacy", "ordered", "native"],
                        }
                      : modelSource === "openrouter"
                        ? {
                            provider_credential: "openrouter",
                            // Native lets new Agents default to structured@3 (#532).
                            projection_versions: [
                              "legacy",
                              "ordered",
                              "native",
                            ],
                          }
                        : {}),
                    model_id: s("model_id"),
                    endpoint: s("endpoint"),
                    reasoning_effort: s("reasoning_effort") || null,
                    context_window: Number(s("context_window")),
                    max_output_tokens: Number(s("max_output_tokens")),
                    modalities: JSON.parse(s("modalities")),
                    media_routes: JSON.parse(s("media_routes") || "[]"),
                    cost: JSON.parse(s("cost")),
                    // New versions count requests with the Token Encoding
                    // chosen for them, or size them from observed usage;
                    // versions that omit it keep the byte estimate (ADR 0017).
                    request_estimator: s("token_encoding")
                      ? { tokenized: s("token_encoding") }
                      : "calibrated",
                  }
                : kind === "embedding"
                  ? {
                      provider: s("embedding_provider"),
                      endpoint: s("endpoint"),
                      // OpenRouter uses the Tenant's Provider Credential; an
                      // OpenAI-compatible endpoint is called without a key.
                      ...(s("embedding_provider") === "openrouter"
                        ? { provider_credential: "openrouter" }
                        : {}),
                      model: s("model_id"),
                      model_version: s("model_version"),
                      dimensions: Number(s("dimensions")),
                    }
                  : kind === "decider"
                    ? {
                        hook: deciderHook,
                        answer_type: "noul",
                        description: s("description_en"),
                        provider_credential: "openrouter",
                        routes: [
                          {
                            provider_contract: "typesafe.jev/1",
                            model: s("model_id"),
                            hosts: [],
                            threshold:
                              deciderHook === "compaction"
                                ? "3fe0000000000000"
                                : probabilityBits(Number(s("route_threshold"))),
                          },
                          ...(d.has("clef_route") ? [CLEF_ROUTE] : []),
                        ],
                        ...deciderContracts[deciderHook],
                        mode:
                          deciderHook === "approval_observation"
                            ? "shadow"
                            : s("decider_mode"),
                        max_calls_per_run: Number(s("max_calls_per_run")),
                        state_retention: {
                          enabled:
                            deciderHook === "compaction" &&
                            d.has("retain_state"),
                          days: Number(s("retention_days")),
                        },
                      }
                    : (memoryConfiguration(kind, d) ?? JSON.parse(s("config")));
          const declaration =
            kind === "source" ||
            (kind === "memory" && s("context_adapter") === "native_memory")
              ? contextSourceConfiguration(d)
              : undefined;
          if (declaration) config.context_source = declaration;
          const entry = {
            id: "",
            version: s("version"),
            kind,
            name: { en: s("name_en") },
            description: {
              en: s("description_en"),
            },
            capabilities: split(s("capabilities")),
            languages: split(s("languages")),
            tags: split(s("tags")),
            skills: [],
            schema: kind === "tool" ? JSON.parse(s("schema")) : {},
            config,
          };
          if (
            kind === "agent" &&
            !s("instructions").trim() &&
            !hasInstructionalBinding(
              core.bindings,
              data.registry,
              registryDeployment(data),
              agentPlugins({ kind, config: pluginConfig }) ?? [],
            )
          )
            throw new Error(t("agentNeedsSkill"));
          if (readingDocuments) return;
          const personal = kind === "agent" && documents.length > 0;
          const body = JSON.stringify(personal ? { entry, documents } : entry);
          if (registration.current?.body !== body) {
            registration.current = { body, key: crypto.randomUUID() };
          }
          const key = registration.current.key;
          void submit(() =>
            personal
              ? personalAgentCreate(
                  { entry, documents },
                  { headers: { "Idempotency-Key": key } },
                )
              : registryCreate(entry, { headers: { "Idempotency-Key": key } }),
          );
        } catch (err) {
          setError(String(err));
        }
      }}
    >
      <div className={pairClass}>
        <Field label={t("entityKind")}>
          <NativeSelect
            value={kind}
            onChange={(e) => {
              setKind(e.target.value);
              setModelName("");
              setCustomModelName(null);
            }}
          >
            {[
              "agent",
              "model",
              "tool",
              "external_service",
              "skill",
              "memory",
              "source",
              "bundle",
              "cluster",
              "node",
              "decider",
              "embedding",
              "memory",
              "source",
              "reranker",
              "tokenizer",
            ].map((k) => (
              <option key={k}>{k}</option>
            ))}
          </NativeSelect>
        </Field>
        <Field label={t("version")}>
          <Input
            name="version"
            required
            defaultValue="1.0.0"
            className="font-mono text-xs"
          />
        </Field>
      </div>
      <Field label={t("name")}>
        {kind === "model" ? (
          <Input
            key="model-name"
            name="name_en"
            required
            value={customModelName ?? modelName}
            onChange={(event) => setCustomModelName(event.target.value || null)}
          />
        ) : (
          <Input
            key="entity-name"
            name="name_en"
            required
            defaultValue={defaultName}
          />
        )}
      </Field>
      <Field label={t("description")}>
        <Textarea name="description_en" required className="min-h-16" />
      </Field>
      <div className={pairClass}>
        <Field label={t("capabilities")}>
          <Input name="capabilities" placeholder="web.search, coding" />
        </Field>
        <Field label={t("languages")}>
          <Input
            name="languages"
            placeholder={t("commaSeparated")}
            defaultValue="ja, en"
          />
        </Field>
      </div>
      <Field label={t("tags")}>
        <Input name="tags" placeholder={t("commaSeparated")} />
      </Field>
      <div
        key={kind}
        className="grid min-w-0 gap-4 border-t border-border pt-4"
      >
        {kind === "agent" ? (
          <>
            <Hint>{t("agentHelp")}</Hint>
            <RegistryModelPicker entries={models} access={data.access} />
            {models.length === 0 && (
              <Notice tone="warning">{t("noModel")}</Notice>
            )}
            <Field label={t("additionalInstructions")}>
              <Textarea
                name="instructions"
                rows={3}
                placeholder={t("additionalInstructionsHelp")}
              />
            </Field>
            <CapabilityConfiguration
              value={core}
              change={setCore}
              coordinator={coordinator}
              node={registryDeployment(data)}
              entries={data.registry}
            />
            <AgentPluginImport config={pluginConfig} change={setPluginConfig} />
            <AgentDocuments
              documents={documents}
              change={setDocuments}
              busyChange={setReadingDocuments}
            />
            <Check
              checked={coordinator}
              onChange={(e) => {
                const selected = e.target.checked;
                setCoordinator(selected);
                if (selected)
                  setCore((previous) => ({
                    ...previous,
                    remove_default: previous.remove_default.filter(
                      (name) => !coordinatorDefaults.includes(name),
                    ),
                  }));
              }}
            >
              {t("useAsCoordinator")}
            </Check>
          </>
        ) : kind === "model" ? (
          <>
            <Hint>{t("modelHelp")}</Hint>
            <Field label={t("provider")}>
              <NativeSelect
                name="model_source"
                value={modelSource}
                onChange={(event) => {
                  setModelSource(event.target.value);
                  setModelName("");
                  setCustomModelName(null);
                }}
              >
                <option value="openrouter">
                  OpenRouter · {t("providerCredentials")}
                </option>
                {selfHostedPlanConnections &&
                  data.access.kind === "subject" && (
                    <option value="chatgpt">
                      ChatGPT · {t("planConnections")}
                    </option>
                  )}
                <option value="keyless">
                  {t("openaiCompatible")} · Keyless
                </option>
              </NativeSelect>
            </Field>
            <div key={modelSource} className="grid min-w-0 gap-4">
              {modelSource === "openrouter" ? (
                <OpenRouterModelPicker onNameChange={setModelName} />
              ) : selfHostedPlanConnections &&
                modelSource === "chatgpt" &&
                data.access.kind === "subject" ? (
                <PlanModelPicker
                  tenant={data.access.tenant}
                  subject={data.access.subject}
                  onNameChange={setModelName}
                />
              ) : (
                <>
                  <Notice tone="warning">{t("keylessEndpointHelp")}</Notice>
                  <Field label={t("endpoint")}>
                    <Input
                      name="endpoint"
                      type="url"
                      required
                      placeholder="http://127.0.0.1:11434/v1"
                    />
                  </Field>
                  <Field label={t("modelId")}>
                    <Input
                      name="model_id"
                      required
                      onChange={(event) => setModelName(event.target.value)}
                    />
                  </Field>
                  <div className={pairClass}>
                    <Field label={t("contextWindow")}>
                      <Input
                        name="context_window"
                        type="number"
                        min={1}
                        required
                        defaultValue={128000}
                      />
                    </Field>
                    <Field label={t("modelMaxOutputTokens")}>
                      <Input
                        name="max_output_tokens"
                        type="number"
                        min={1}
                        required
                        defaultValue={16384}
                      />
                    </Field>
                  </div>
                  <input type="hidden" name="modalities" value='["text"]' />
                  <input type="hidden" name="cost" value="{}" />
                </>
              )}
            </div>
          </>
        ) : kind === "embedding" ? (
          <>
            <Hint>{t("embeddingRegistryHelp")}</Hint>
            <Field label={t("provider")}>
              <NativeSelect name="embedding_provider" defaultValue="openrouter">
                <option value="openrouter">OpenRouter</option>
                <option value="openai">{t("openaiCompatible")}</option>
              </NativeSelect>
            </Field>
            <Field label={t("endpoint")}>
              <Input
                name="endpoint"
                type="url"
                required
                defaultValue="https://openrouter.ai/api/v1"
              />
            </Field>
            <Field label={t("modelId")}>
              <Input
                name="model_id"
                required
                defaultValue="google/gemini-embedding-2"
              />
            </Field>
            <div className={pairClass}>
              <Field label={t("embeddingModelVersion")}>
                <Input name="model_version" required defaultValue="1.0.0" />
              </Field>
              <Field label={t("embeddingDimensions")}>
                <Input
                  name="dimensions"
                  type="number"
                  required
                  min={1}
                  max={8192}
                  defaultValue={3072}
                />
              </Field>
            </div>
          </>
        ) : kind === "decider" ? (
          <>
            <Hint>
              {deciderHook === "compaction"
                ? t("deciderHelp")
                : ja
                  ? "Jev の正確なリリースを選び、この不変バージョンを Node の Tenant Catalog で承認してください。Tenant の OpenRouter 接続を使います。Shadow は観測や提案を記録し、保護された操作やリクエストを変更しません。"
                  : "Select an exact Jev release and approve this immutable version in the Node's Tenant Catalog. It uses the Tenant's OpenRouter connection. Shadow records observations or proposals without changing the protected action or request."}
            </Hint>
            <Field label={ja ? "判断の用途" : "Decision hook"}>
              <NativeSelect
                value={deciderHook}
                onChange={(event) => setDeciderHook(event.target.value)}
              >
                <option value="compaction">
                  {ja ? "履歴の圧縮" : "History compaction"}
                </option>
                <option value="approval_observation">
                  {ja
                    ? "独立した Shadow 承認の観測"
                    : "Independent Shadow approval observation"}
                </option>
                <option value="context_selection">
                  {ja
                    ? "コンテキスト候補の選択"
                    : "Context candidate selection"}
                </option>
              </NativeSelect>
            </Field>
            <Hint>
              {ja
                ? "契約と質問は用途ごとにバージョンで固定されます。しきい値を変更するには新しい不変バージョンと Catalog Approval が必要です。承認の観測は Auto の強制判断を変更しません。"
                : "Each hook pins its versioned contract and questions. A threshold change requires a new immutable version and Catalog Approval. Observation does not change Auto enforcement."}
            </Hint>
            {deciderHook !== "compaction" && (
              <Field
                label={ja ? "Jev Route のしきい値" : "Jev Route threshold"}
              >
                <Input
                  name="route_threshold"
                  type="number"
                  min={0.000001}
                  max={0.5}
                  step="any"
                  required
                  defaultValue={0.5}
                />
              </Field>
            )}
            <Field label={t("model")}>
              <Input
                name="model_id"
                required
                maxLength={128}
                pattern="typesafe/jev-(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)"
                defaultValue="typesafe/jev-1.14"
              />
            </Field>
            <div className="grid min-w-0 gap-4 sm:grid-cols-3">
              <Field label={t("deciderMode")}>
                <NativeSelect
                  key={deciderHook}
                  name="decider_mode"
                  defaultValue={
                    deciderHook === "approval_observation"
                      ? "shadow"
                      : "enforce"
                  }
                  disabled={deciderHook === "approval_observation"}
                >
                  <option value="enforce">{t("deciderModeEnforce")}</option>
                  <option value="shadow">{t("deciderModeShadow")}</option>
                </NativeSelect>
              </Field>
              <Field label={t("deciderCalls")}>
                <Input
                  name="max_calls_per_run"
                  type="number"
                  required
                  min={1}
                  max={1000}
                  defaultValue={20}
                />
              </Field>
              <Field label={t("deciderRetentionDays")}>
                <Input
                  name="retention_days"
                  type="number"
                  required
                  min={1}
                  max={30}
                  defaultValue={7}
                />
              </Field>
            </div>
            <label className="flex items-center gap-2 text-sm">
              <input type="checkbox" name="clef_route" />
              {t("deciderClefRoute")}
            </label>
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                name="retain_state"
                disabled={deciderHook !== "compaction"}
              />
              {t("deciderRetainState")}
            </label>
          </>
        ) : ["reranker", "tokenizer"].includes(kind) ? (
          <MemoryRegistryFields kind={kind} entries={data.registry} />
        ) : (
          <EntityConfiguration kind={kind} data={data} />
        )}
      </div>
      {error && <Alert>{error}</Alert>}
      <Button className="justify-self-end" disabled={readingDocuments}>
        {t("register")}
      </Button>
    </form>
  );
}
export function AssignForm({
  task,
  discovery,
  data,
  submit,
}: {
  task: Task;
  discovery: Discovery;
  data: State;
  submit: Submit;
}) {
  const { t, locale } = useI18n();
  const ja = locale === "ja-JP";
  const [selected, setSelected] = useState("");
  const [memory, setMemory] = useState(false);
  const [grantPending, setGrantPending] = useState(false);
  const [grantBusy, setGrantBusy] = useState(false);
  const request = useRef<{ binding: string; id: string } | null>(null);
  const agentLabel = useAgentLabel(data, discovery);
  const agents = discovery.agents;
  const chosen = agents.find(
    (a) =>
      JSON.stringify([a.node_id, a.entity.id, a.entity.version]) === selected,
  );
  const scopedRemote =
    data.access.kind === "subject" && chosen && chosen.node_id !== data.node.id;
  const inspection = useQuery({
    queryKey: ["remote-agent-inspection", task.id, selected],
    enabled: !!scopedRemote,
    staleTime: 0,
    retry: false,
    queryFn: () =>
      apiFetch<{ native_required: boolean; memory_available: boolean }>(
        `/api/tasks/${task.id}/remote-grants/inspect`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            node_id: chosen!.node_id,
            agent: { id: chosen!.entity.id, version: chosen!.entity.version },
          }),
        },
      ),
  });
  const nativeRequired =
    !!scopedRemote && inspection.data?.native_required === true;
  const memoryAvailable =
    !!scopedRemote && inspection.data?.memory_available === true;
  const useMemory = nativeRequired || (memoryAvailable && memory);
  const inspectionBlocked =
    !!scopedRemote && (inspection.isPending || inspection.isError);
  return (
    <form
      className={formClass}
      onSubmit={(e) => {
        e.preventDefault();
        const d = new FormData(e.currentTarget);
        const a = agents.find(
          (agent) =>
            JSON.stringify([
              agent.node_id,
              agent.entity.id,
              agent.entity.version,
            ]) === (d.get("agent") ?? selected),
        );
        if (!a || inspectionBlocked) return;
        setGrantBusy(true);
        void submit(async () => {
          if (data.access.kind !== "subject" || a.node_id === data.node.id) {
            return taskDelegate(task.id, {
              node_id: a.node_id,
              agent: { id: a.entity.id, version: a.entity.version },
            });
          }
          const decider = String(d.get("decider") ?? "").trim();
          const input = request.current
            ? JSON.parse(request.current.binding)
            : {
                node_id: a.node_id,
                agent: { id: a.entity.id, version: a.entity.version },
                ttl_seconds: 3600,
                semantic: useMemory
                  ? {
                      mode: "required_home",
                      embedding: ref(String(d.get("embedding"))),
                      ...(nativeMemoryRequest(d)
                        ? { native: nativeMemoryRequest(d) }
                        : {}),
                      ...(decider ? { decider: ref(decider) } : {}),
                    }
                  : { mode: "disabled" },
              };
          const binding = JSON.stringify(input);
          if (request.current?.binding !== binding)
            request.current = { binding, id: crypto.randomUUID() };
          const id = request.current.id;
          setGrantPending(true);
          try {
            await apiFetch(`/api/tasks/${task.id}/remote-grants`, {
              method: "POST",
              headers: { "content-type": "application/json" },
              body: JSON.stringify({ id, ...input }),
            });
            const result = await apiFetch(
              `/api/tasks/${task.id}/remote-grants/${id}/activate`,
              {
                method: "POST",
                headers: { "content-type": "application/json" },
                body: "{}",
              },
            );
            request.current = null;
            setGrantPending(false);
            return result;
          } catch (cause) {
            if (cause instanceof ApiError && cause.responseReceived) {
              request.current = null;
              setGrantPending(false);
            }
            throw cause;
          }
        }).finally(() => setGrantBusy(false));
      }}
    >
      <fieldset
        disabled={grantPending || grantBusy}
        className="grid min-w-0 gap-4"
      >
        <Field label={t("agent")}>
          <NativeSelect
            name="agent"
            required
            value={selected}
            onChange={(e) => {
              setSelected(e.target.value);
              setMemory(false);
            }}
          >
            <option value="">{t("choose")}</option>
            {agents.map((a) => (
              <option
                value={JSON.stringify([
                  a.node_id,
                  a.entity.id,
                  a.entity.version,
                ])}
                key={`${a.node_id}/${a.entity.id}@${a.entity.version}`}
              >
                {agentLabel(a.node_id, a.entity)} ·{" "}
                <ReferenceName id={a.node_id} />
              </option>
            ))}
          </NativeSelect>
        </Field>
        {scopedRemote && (
          <Group
            nested
            legend={
              ja ? "遠隔実行で参照する記憶" : "Memory for remote execution"
            }
          >
            <Check
              checked={useMemory}
              disabled={nativeRequired || !memoryAvailable || inspectionBlocked}
              onChange={(e) => setMemory(e.target.checked)}
            >
              {ja
                ? "各推論の前に Home の記憶を検索する"
                : "Require Home memory before each inference"}
            </Check>
            {inspection.isPending && (
              <Hint role="status">
                {ja
                  ? "実行先のメモリ要件を確認しています"
                  : "Checking memory requirements at the execution node"}
              </Hint>
            )}
            {inspection.isError && (
              <Alert>
                {ja
                  ? "実行先のメモリ要件を確認できませんでした"
                  : "Could not inspect memory requirements at the execution node"}
              </Alert>
            )}
            {useMemory && (
              <>
                <label className="grid gap-1.5 text-xs font-medium text-muted-foreground">
                  {ja ? "Home の embedding 定義" : "Home embedding definition"}
                  <NativeSelect name="embedding" required defaultValue="">
                    <option value="">{t("choose")}</option>
                    {data.registry
                      .filter((entry) => entry.kind === "embedding")
                      .map((entry) => (
                        <option
                          key={`${entry.id}@${entry.version}`}
                          value={`${entry.id}@${entry.version}`}
                        >
                          {entry.id}@{entry.version}
                        </option>
                      ))}
                  </NativeSelect>
                </label>
                <label className="grid gap-1.5 text-xs font-medium text-muted-foreground">
                  {ja
                    ? "任意: 実行 Node で承認済みの圧縮 Decider"
                    : "Optional: compaction Decider approved at the execution node"}
                  <Input
                    name="decider"
                    placeholder="decider-id@1.0.0"
                    pattern=".+@[0-9]+\.[0-9]+\.[0-9]+.*"
                    className="font-mono text-xs"
                  />
                </label>
                <Hint>
                  {ja
                    ? "検索結果を選択した Agent のモデルへ開示します。予算や権限が不足すると実行は一時停止します。"
                    : "Retrieval results are disclosed to the selected agent's model. Execution pauses when authority or budget is insufficient."}
                </Hint>
                <HomeNativeMemoryFields
                  key={selected}
                  workspace={task.workspace_id}
                  entries={data.registry}
                  required={nativeRequired}
                />
              </>
            )}
          </Group>
        )}
      </fieldset>
      {grantPending && (
        <Notice role="status">
          {ja
            ? "同じ実行許可を再確認します。結果が確定するまで設定を変更できません。"
            : "Recheck the same execution grant. Its settings stay fixed until the outcome is confirmed."}
        </Notice>
      )}
      <Button
        className="justify-self-end"
        disabled={grantBusy || inspectionBlocked}
      >
        {grantPending
          ? ja
            ? "同じ実行許可を再試行"
            : "Retry the same execution grant"
          : t("delegate")}
      </Button>
    </form>
  );
}
export { ref as entityRef };
