import { coordinatorDefaults, hasInstructionalBinding } from "./agent-bindings";
import { Button } from "./components/ui/button";
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
import type { ReferenceDocument } from "./generated/models";
import { Fragment, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { EntityConfiguration } from "./entity-configuration";
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
  peerCreate,
  taskDelegate,
  packagePublish,
} from "./generated/aidash";
export type Submit = (request: () => Promise<unknown>) => Promise<boolean>;
const split = (s: string) =>
  s
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
const ref = (s: string): EntityRef => {
  const index = s.lastIndexOf("@");
  return { id: s.slice(0, index), version: s.slice(index + 1) };
};
export function GoalForm({ data, submit }: { data: State; submit: Submit }) {
  const { t } = useI18n();
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
      await submit(() =>
        conversationCreate({
          ...value,
          target: ref(value.target),
          target_kind: target.kind,
        }),
      );
    },
  });
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field name="title">
        {(f) => (
          <Field label={t("title")}>
            <input
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
            <textarea
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
            <select
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
            </select>
          </Field>
        )}
      </form.Field>
      <form.Subscribe selector={(s) => s.isSubmitting}>
        {(pending) => (
          <Button
            variant="outline"
            disabled={pending || targets.length === 0}
            className="primary"
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
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        const d = new FormData(e.currentTarget);
        void submit(() =>
          workspaceCreate({
            title: String(d.get("title")),
            goal: String(d.get("goal")),
          }),
        );
      }}
    >
      <Field label={t("title")}>
        <input name="title" required />
      </Field>
      <Field label={t("goal")}>
        <textarea name="goal" rows={4} required />
      </Field>
      <Button variant="outline" className="primary">
        {t("create")}
      </Button>
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
        <select
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
        </select>
      </Field>
      <Field label={t("title")}>
        <input name="title" required />
      </Field>
      <Field label={t("description")}>
        <textarea name="description" required rows={3} />
      </Field>
      <Field label={t("parentTask")}>
        <select
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
        </select>
      </Field>
      <Field label={t("dependencies")}>
        <select
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
        <textarea
          name="requirements"
          defaultValue={'{"capability":"web.search","language":"ja"}'}
          onChange={(e) => e.currentTarget.setCustomValidity("")}
        />
      </Field>
      <Button variant="outline" className="primary">
        {t("create")}
      </Button>
    </form>
  );
}
/** Omits the field for the default legacy-only set so stored bytes stay unchanged. */
function projectionVersions(d: FormData): { projection_versions?: string[] } {
  const versions = d.getAll("projection_versions").map(String);
  return versions.length === 1 && versions[0] === "legacy"
    ? {}
    : { projection_versions: versions };
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
  const entityLabel = useEntityLabel(data.registry);
  const [kind, setKind] = useState(initial);
  const registration = useRef<{ body: string; key: string } | null>(null);
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
  const [cluster, setCluster] = useState("");
  const [readingDocuments, setReadingDocuments] = useState(false);
  return (
    <form
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
                  cluster: s("cluster") ? ref(s("cluster")) : null,
                  max_steps: 64,
                  ...(s("projection_version") === "ordered"
                    ? { projection_version: "ordered" }
                    : {}),
                  ...(s("prompt_cache") === "explicit"
                    ? { prompt_cache: "explicit" }
                    : {}),
                  ...core,
                }
              : kind === "model"
                ? {
                    provider: "openrouter",
                    model_id: s("model_id"),
                    endpoint: s("endpoint"),
                    credential_env: "AIDASH_SECRET_OPENROUTER",
                    reasoning_effort: s("reasoning_effort") || null,
                    context_window: Number(s("context_window")),
                    max_output_tokens: Number(s("max_output_tokens")),
                    modalities: JSON.parse(s("modalities")),
                    media_routes: JSON.parse(s("media_routes") || "[]"),
                    ...projectionVersions(d),
                    ...(s("cache_mode") && s("cache_mode") !== "none"
                      ? { cache_mode: s("cache_mode") }
                      : {}),
                    cost: JSON.parse(s("cost")),
                  }
                : kind === "embedding"
                  ? {
                      provider: s("embedding_provider"),
                      endpoint: s("endpoint"),
                      credential_env: d.has("embedding_credentials")
                        ? s("embedding_provider") === "openrouter"
                          ? "AIDASH_SECRET_OPENROUTER"
                          : "AIDASH_SECRET_EMBEDDING"
                        : null,
                      model: s("model_id"),
                      model_version: s("model_version"),
                      dimensions: Number(s("dimensions")),
                    }
                  : kind === "compactor"
                    ? {
                        provider: "typesafe-system-one",
                        endpoint: s("endpoint"),
                        model: s("model_id"),
                        credential_env: "AIDASH_SECRET_JEV",
                        max_request_bytes: Number(s("max_request_bytes")),
                        max_questions: Number(s("max_questions")),
                        max_response_bytes: Number(s("max_response_bytes")),
                      }
                    : (memoryConfiguration(kind, d) ?? JSON.parse(s("config")));
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
            !hasInstructionalBinding(core.bindings, data.registry, data.node.id)
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
      <Field label={t("entityKind")}>
        <select
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
            "skill",
            "memory",
            "source",
            "bundle",
            "cluster",
            "node",
            "compactor",
            "embedding",
            "memory",
            "source",
            "reranker",
            "tokenizer",
          ].map((k) => (
            <option key={k}>{k}</option>
          ))}
        </select>
      </Field>
      <Field label={t("version")}>
        <input name="version" required defaultValue="1.0.0" />
      </Field>
      <Field label={t("name")}>
        {kind === "model" ? (
          <input
            key="model-name"
            name="name_en"
            required
            value={customModelName ?? modelName}
            onChange={(event) => setCustomModelName(event.target.value || null)}
          />
        ) : (
          <input
            key="entity-name"
            name="name_en"
            required
            defaultValue={defaultName}
          />
        )}
      </Field>
      <Field label={t("description")}>
        <textarea name="description_en" required />
      </Field>
      <div className="two-columns">
        <Field label={t("capabilities")}>
          <input name="capabilities" placeholder="web.search, coding" />
        </Field>
        <Field label={t("languages")}>
          <input
            name="languages"
            placeholder={t("commaSeparated")}
            defaultValue="ja, en"
          />
        </Field>
      </div>
      <Field label={t("tags")}>
        <input name="tags" placeholder={t("commaSeparated")} />
      </Field>
      <Fragment key={kind}>
        {kind === "agent" ? (
          <>
            <p className="muted">{t("agentHelp")}</p>
            <Field label={t("model")}>
              <select name="model" required defaultValue="">
                <option value="">{t("choose")}</option>
                {models.map((e) => (
                  <option
                    key={`${e.id}@${e.version}`}
                    value={`${e.id}@${e.version}`}
                  >
                    {entityLabel(e)}
                  </option>
                ))}
              </select>
            </Field>
            {models.length === 0 && <p className="notice">{t("noModel")}</p>}

            <Field label={t("additionalInstructions")}>
              <textarea
                name="instructions"
                rows={3}
                placeholder={t("additionalInstructionsHelp")}
              />
            </Field>
            <CapabilityConfiguration
              value={core}
              change={setCore}
              cluster={Boolean(cluster)}
              node={data.node.id}
              entries={data.registry}
            />
            <AgentDocuments
              documents={documents}
              change={setDocuments}
              busyChange={setReadingDocuments}
            />
            <Field label={t("cluster")}>
              <select
                name="cluster"
                value={cluster}
                onChange={(e) => {
                  const selected = e.target.value;
                  setCluster(selected);
                  if (selected)
                    setCore((previous) => ({
                      ...previous,
                      remove_default: previous.remove_default.filter(
                        (name) => !coordinatorDefaults.includes(name),
                      ),
                    }));
                }}
              >
                <option value="">{t("noAssignment")}</option>
                {data.registry
                  .filter((e) => e.kind === "cluster")
                  .map((e) => (
                    <option
                      key={`${e.id}@${e.version}`}
                      value={`${e.id}@${e.version}`}
                    >
                      {entityLabel(e)}
                    </option>
                  ))}
              </select>
            </Field>
            <Field
              label={
                locale === "ja-JP" ? "投影バージョン" : "Projection version"
              }
            >
              <select name="projection_version" defaultValue="legacy">
                <option value="legacy">legacy</option>
                <option value="ordered">ordered</option>
              </select>
            </Field>
            <Field
              label={
                locale === "ja-JP" ? "プロンプトキャッシュ" : "Prompt cache"
              }
            >
              <select name="prompt_cache" defaultValue="off">
                <option value="off">off</option>
                <option value="explicit">explicit</option>
              </select>
            </Field>
          </>
        ) : kind === "model" ? (
          <>
            <p className="muted">{t("modelHelp")}</p>
            <OpenRouterModelPicker onNameChange={setModelName} />
          </>
        ) : kind === "embedding" ? (
          <>
            <p className="muted">{t("embeddingRegistryHelp")}</p>
            <Field label={t("provider")}>
              <select name="embedding_provider" defaultValue="openrouter">
                <option value="openrouter">OpenRouter</option>
                <option value="openai">{t("openaiCompatible")}</option>
              </select>
            </Field>
            <Field label={t("endpoint")}>
              <input
                name="endpoint"
                type="url"
                required
                defaultValue="https://openrouter.ai/api/v1"
              />
            </Field>
            <Field label={t("modelId")}>
              <input
                name="model_id"
                required
                defaultValue="google/gemini-embedding-2"
              />
            </Field>
            <Field label={t("embeddingModelVersion")}>
              <input name="model_version" required defaultValue="1.0.0" />
            </Field>
            <Field label={t("embeddingDimensions")}>
              <input
                name="dimensions"
                type="number"
                required
                min={1}
                max={8192}
                defaultValue={3072}
              />
            </Field>
            <label className="check">
              <input
                type="checkbox"
                name="embedding_credentials"
                defaultChecked
              />
              {t("configuredCredentials")}
            </label>
          </>
        ) : kind === "compactor" ? (
          <>
            <p className="muted">{t("compactorHelp")}</p>
            <Field label={t("model")}>
              <input
                name="model_id"
                required
                maxLength={128}
                defaultValue="jev-latest"
              />
            </Field>
            <Field label={t("endpoint")}>
              <input
                name="endpoint"
                type="url"
                required
                defaultValue="https://api.typesafe.ai/v1/systemone"
              />
            </Field>

            <Field label={t("compactorRequestBytes")}>
              <input
                name="max_request_bytes"
                type="number"
                required
                min={1024}
                max={1048576}
                defaultValue={200000}
              />
            </Field>
            <Field label={t("compactorQuestions")}>
              <input
                name="max_questions"
                type="number"
                required
                min={1}
                max={1024}
                defaultValue={200}
              />
            </Field>
            <Field label={t("compactorResponseBytes")}>
              <input
                name="max_response_bytes"
                type="number"
                required
                min={128}
                max={1048576}
                defaultValue={16000}
              />
            </Field>
          </>
        ) : ["reranker", "tokenizer"].includes(kind) ? (
          <MemoryRegistryFields kind={kind} entries={data.registry} />
        ) : (
          <EntityConfiguration kind={kind} data={data} />
        )}
      </Fragment>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      <Button variant="outline" className="primary" disabled={readingDocuments}>
        {t("register")}
      </Button>
    </form>
  );
}
export function PeerForm({ submit }: { submit: Submit }) {
  const { t } = useI18n();
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        const d = new FormData(e.currentTarget);
        void submit(() =>
          peerCreate({
            node_id: String(d.get("node_id")),
            endpoint: String(d.get("endpoint")),
            credential_env: String(d.get("credential_env")),
            protocol_version: "0.2",
            enabled: true,
          }),
        );
      }}
    >
      <Field label={t("node")}>
        <input name="node_id" placeholder="aidash://node-b" required />
      </Field>
      <Field label={t("endpoint")}>
        <input
          name="endpoint"
          type="url"
          placeholder="http://127.0.0.1:8081"
          required
        />
      </Field>
      <Field label={t("peerCredential")}>
        <input
          name="credential_env"
          placeholder="AIDASH_SECRET_NODE_B"
          required
        />
      </Field>
      <Button variant="outline" className="primary">
        {t("addPeer")}
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
          const compactor = String(d.get("compactor") ?? "").trim();
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
                      ...(compactor ? { compactor: ref(compactor) } : {}),
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
      <fieldset disabled={grantPending || grantBusy}>
        <Field label={t("agent")}>
          <select
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
          </select>
        </Field>
        {scopedRemote && (
          <fieldset>
            <legend>
              {ja ? "遠隔実行で参照する記憶" : "Memory for remote execution"}
            </legend>
            <label>
              <input
                type="checkbox"
                checked={useMemory}
                disabled={
                  nativeRequired || !memoryAvailable || inspectionBlocked
                }
                onChange={(e) => setMemory(e.target.checked)}
              />
              {ja
                ? "各推論の前に Home の記憶を検索する"
                : "Require Home memory before each inference"}
            </label>
            {inspection.isPending && (
              <p role="status">
                {ja
                  ? "実行先のメモリ要件を確認しています"
                  : "Checking memory requirements at the execution node"}
              </p>
            )}
            {inspection.isError && (
              <p role="alert">
                {ja
                  ? "実行先のメモリ要件を確認できませんでした"
                  : "Could not inspect memory requirements at the execution node"}
              </p>
            )}
            {useMemory && (
              <>
                <label>
                  {ja ? "Home の embedding 定義" : "Home embedding definition"}
                  <select name="embedding" required defaultValue="">
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
                  </select>
                </label>
                <label>
                  {ja
                    ? "任意: 実行 Node の承認済み compactor"
                    : "Optional: approved compactor at the execution node"}
                  <input
                    name="compactor"
                    placeholder="compactor-id@1.0.0"
                    pattern=".+@[0-9]+\.[0-9]+\.[0-9]+.*"
                  />
                </label>
                <p>
                  {ja
                    ? "検索結果を選択した Agent のモデルへ開示します。予算や権限が不足すると実行は一時停止します。"
                    : "Retrieval results are disclosed to the selected agent's model. Execution pauses when authority or budget is insufficient."}
                </p>
                <HomeNativeMemoryFields
                  key={selected}
                  workspace={task.workspace_id}
                  entries={data.registry}
                  required={nativeRequired}
                />
              </>
            )}
          </fieldset>
        )}
      </fieldset>
      {grantPending && (
        <p role="status">
          {ja
            ? "同じ実行許可を再確認します。結果が確定するまで設定を変更できません。"
            : "Recheck the same execution grant. Its settings stay fixed until the outcome is confirmed."}
        </p>
      )}
      <Button
        variant="outline"
        className="primary"
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
export function PublishForm({ data, submit }: { data: State; submit: Submit }) {
  const { t } = useI18n();
  const entityLabel = useEntityLabel(data.registry);
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        const d = new FormData(e.currentTarget);
        const entity = data.registry.find(
          (e) => `${e.id}@${e.version}` === d.get("entity"),
        );
        if (!entity) return;
        void submit(() =>
          packagePublish({
            entity,
            author: String(d.get("author")),
            permissions: split(String(d.get("permissions"))),
            dependencies: d.getAll("dependencies").map((value) => {
              const [id, version] = JSON.parse(String(value)) as [
                string,
                string,
              ];
              return { id, version };
            }),
          }),
        );
      }}
    >
      <p>{t("publishHelp")}</p>
      <Field label={t("packageEntity")}>
        <select name="entity" required defaultValue="">
          <option value="">{t("choose")}</option>
          {data.registry
            .filter((e) => ["agent", "tool", "skill"].includes(e.kind))
            .map((e) => (
              <option
                key={`${e.id}@${e.version}`}
                value={`${e.id}@${e.version}`}
              >
                {entityLabel(e)}
              </option>
            ))}
        </select>
      </Field>
      <Field label={t("dependencies")}>
        <select name="dependencies" multiple>
          {data.registry.map((entry) => (
            <option
              key={`${entry.id}@${entry.version}`}
              value={JSON.stringify([entry.id, entry.version])}
            >
              {entityLabel(entry)}
            </option>
          ))}
        </select>
      </Field>
      <Field label={t("author")}>
        <input name="author" required />
      </Field>
      <Field label={t("permissions")}>
        <input name="permissions" placeholder={t("commaSeparated")} />
      </Field>
      <Button variant="outline" className="primary">
        {t("publish")}
      </Button>
    </form>
  );
}
export { ref as entityRef };
