import { AgentDocuments } from "./agent-documents";
import type { ReferenceDocument } from "./generated/models";
import { Fragment, useRef, useState } from "react";
import { EntityConfiguration } from "./entity-configuration";
import { OpenRouterModelPicker } from "./openrouter-model-picker";
import { useForm } from "@tanstack/react-form";
import { Field, useI18n } from "./ui";
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
                  {e.id} · {e.version} ({t(e.kind)})
                </option>
              ))}
            </select>
          </Field>
        )}
      </form.Field>
      <form.Subscribe selector={(s) => s.isSubmitting}>
        {(pending) => (
          <button
            disabled={pending || targets.length === 0}
            className="primary"
            type="submit"
          >
            {t("newGoal")}
          </button>
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
      <button className="primary">{t("create")}</button>
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
      <button className="primary">{t("create")}</button>
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
  const { t, local } = useI18n();
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
  const toolEntries = data.registry.filter((e) => e.kind === "tool");
  const skillEntries = data.registry.filter((e) => e.kind === "skill");
  const [documents, setDocuments] = useState<ReferenceDocument[]>([]);
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
                  tools: d.getAll("tools").map((v) => ref(String(v))),
                  skills: d.getAll("skills").map((v) => ref(String(v))),
                  cluster: s("cluster") ? ref(s("cluster")) : null,
                  max_steps: 64,
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
                    modalities: ["text"],
                    cost: JSON.parse(s("cost")),
                  }
                : kind === "embedding"
                  ? {
                      provider: "openai",
                      endpoint: s("endpoint"),
                      credential_env: d.has("embedding_credentials")
                        ? "AIDASH_SECRET_EMBEDDING"
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
                    : JSON.parse(s("config"));
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
            !d.getAll("skills").length
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
            "cluster",
            "node",
            "compactor",
            "embedding",
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
                    {e.id} · {e.version}
                  </option>
                ))}
              </select>
            </Field>
            {models.length === 0 && <p className="notice">{t("noModel")}</p>}
            <fieldset>
              <legend>{t("skill")}</legend>
              <p className="muted">{t("agentSkillsHelp")}</p>
              {skillEntries.length === 0 && (
                <p className="notice">{t("agentNoSkills")}</p>
              )}
              {skillEntries.map((e) => (
                <label className="check" key={`${e.id}@${e.version}`}>
                  <input
                    type="checkbox"
                    name="skills"
                    value={`${e.id}@${e.version}`}
                  />
                  {local(e.name) || e.id} · {e.version}
                </label>
              ))}
            </fieldset>
            <Field label={t("additionalInstructions")}>
              <textarea
                name="instructions"
                rows={3}
                placeholder={t("additionalInstructionsHelp")}
              />
            </Field>
            <AgentDocuments
              documents={documents}
              change={setDocuments}
              busyChange={setReadingDocuments}
            />
            <Field label={t("cluster")}>
              <select name="cluster" defaultValue="">
                <option value="">{t("noAssignment")}</option>
                {data.registry
                  .filter((e) => e.kind === "cluster")
                  .map((e) => (
                    <option
                      key={`${e.id}@${e.version}`}
                      value={`${e.id}@${e.version}`}
                    >
                      {e.id} · {e.version}
                    </option>
                  ))}
              </select>
            </Field>
            <fieldset>
              <legend>{t("tools")}</legend>
              {toolEntries.map((e) => (
                <label className="check" key={`${e.id}@${e.version}`}>
                  <input
                    type="checkbox"
                    name="tools"
                    value={`${e.id}@${e.version}`}
                  />
                  {e.id} · {e.version}
                </label>
              ))}
            </fieldset>
          </>
        ) : kind === "model" ? (
          <>
            <p className="muted">{t("modelHelp")}</p>
            <OpenRouterModelPicker onNameChange={setModelName} />
          </>
        ) : kind === "embedding" ? (
          <>
            <p className="muted">{t("embeddingRegistryHelp")}</p>
            <Field label={t("endpoint")}>
              <input
                name="endpoint"
                type="url"
                required
                placeholder="https://provider.example/v1"
              />
            </Field>
            <Field label={t("modelId")}>
              <input name="model_id" required />
            </Field>
            <Field label={t("embeddingModelVersion")}>
              <input name="model_version" required />
            </Field>
            <Field label={t("embeddingDimensions")}>
              <input
                name="dimensions"
                type="number"
                required
                min={1}
                max={8192}
              />
            </Field>
            <label className="check">
              <input type="checkbox" name="embedding_credentials" />
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
        ) : (
          <EntityConfiguration kind={kind} data={data} />
        )}
      </Fragment>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      <button className="primary" disabled={readingDocuments}>
        {t("register")}
      </button>
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
            protocol_version: "0.1",
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
      <button className="primary">{t("addPeer")}</button>
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
  const { t, local } = useI18n();
  const agents = discovery.agents.filter(
    (a) => data.access.kind === "operator" || a.node_id === data.node.id,
  );
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
            ]) === d.get("agent"),
        );
        if (!a) return;
        void submit(() =>
          taskDelegate(task.id, {
            node_id: a.node_id,
            agent: { id: a.entity.id, version: a.entity.version },
          }),
        );
      }}
    >
      <Field label={t("agent")}>
        <select name="agent" required defaultValue="">
          <option value="">{t("choose")}</option>
          {agents.map((a) => (
            <option
              value={JSON.stringify([a.node_id, a.entity.id, a.entity.version])}
              key={`${a.node_id}/${a.entity.id}@${a.entity.version}`}
            >
              {local(a.entity.name)} · {a.node_id} · {a.entity.version}
            </option>
          ))}
        </select>
      </Field>
      <button className="primary">{t("delegate")}</button>
    </form>
  );
}
export function PublishForm({ data, submit }: { data: State; submit: Submit }) {
  const { t } = useI18n();
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
              <option key={`${e.id}@${e.version}`}>
                {e.id}@{e.version}
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
              {entry.id}@{entry.version}
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
      <button className="primary">{t("publish")}</button>
    </form>
  );
}
export { ref as entityRef };
