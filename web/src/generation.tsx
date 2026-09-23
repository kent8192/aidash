import { RecordView } from "./record-view";
import { disambiguateLabels } from "./display-labels";
import { useState, type FormEvent } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, ArrowUpRight } from "lucide-react";
import {
  Badge,
  Field,
  JsonView,
  Modal,
  Panel,
  useI18n,
  useEntryLabel,
  useEntityLabel,
} from "./ui";
import {
  authorizationCatalog,
  generationAssign,
  generationControl,
  generationHistory,
  generationPolicies,
  generationRequests,
  generationSetPolicy,
  generationUsage,
  generationSpec,
} from "./generated/aidash";
import type {
  EntityRef,
  Entry,
  GenerationAction,
  GenerationPolicy,
  GenerationRequest,
  GenerationSpec,
} from "./generated/models";
import type { State, Task } from "./types";
import { entityRef, type Submit } from "./forms";

const key = (reference: EntityRef) => `${reference.id}@${reference.version}`;
const localized = (
  previous: Record<string, string> | undefined,
  english: string,
  japanese: string,
) => ({
  ...previous,
  en: english,
  ja: japanese || english,
  ...(previous?.["en-US"] !== undefined ? { "en-US": english } : {}),
  ...(previous?.["ja-JP"] !== undefined
    ? { "ja-JP": japanese || english }
    : {}),
});
const split = (value: string) =>
  value
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
const active = (status: string) =>
  ["PENDING_APPROVAL", "QUEUED", "ACTIVE"].includes(status);
function usePolicyPresentation(policies: readonly GenerationPolicy[]) {
  const { t, local } = useI18n();
  const name = (policy: GenerationPolicy) =>
    local(policy.spec.template.name) || t("unnamedEntity");
  const base = (policy: GenerationPolicy) =>
    `${name(policy)} · ${t("revision")} ${policy.revision}`;
  const labels = disambiguateLabels(policies, (policy) => policy.id, base);
  const label = (policy: GenerationPolicy) =>
    labels.get(policy.id) ?? base(policy);
  return {
    label,
    name: (policy: GenerationPolicy) =>
      `${name(policy)}${label(policy).slice(base(policy).length)}`,
  };
}
type AgentFields = {
  model?: EntityRef;
  tools?: EntityRef[];
  skills?: EntityRef[];
  cluster?: EntityRef | null;
  instructions?: string;
  max_steps?: number;
};

export function GenerationPage({ data }: { data: State }) {
  const { t } = useI18n();
  const client = useQueryClient();
  const subjectTenant =
    data.access.kind === "subject" ? data.access.tenant : null;
  const [chosenTenant, setChosenTenant] = useState("");
  const tenant = subjectTenant ?? chosenTenant;
  const [editing, setEditing] = useState<GenerationPolicy | "new" | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const policies = useQuery({
    queryKey: ["generation", tenant, "policies"],
    queryFn: () => generationPolicies(encodeURIComponent(tenant)),
    enabled: !!tenant,
    refetchInterval: 5000,
  });
  const policyPresentation = usePolicyPresentation(
    policies.isError ? [] : (policies.data ?? []),
  );
  const requests = useQuery({
    queryKey: ["generation", tenant, "requests"],
    queryFn: () => generationRequests(encodeURIComponent(tenant)),
    enabled: !!tenant,
    refetchInterval: 3000,
  });
  const catalog = useQuery({
    queryKey: ["generation", tenant, "catalog"],
    queryFn: () => authorizationCatalog(encodeURIComponent(tenant)),
    enabled: !!tenant && subjectTenant === null,
    refetchInterval: 10000,
  });
  const entries =
    subjectTenant !== null
      ? data.registry
      : data.registry.filter(
          (entry) =>
            !catalog.isError &&
            catalog.data?.some(
              (b) =>
                b.enabled &&
                b.entry_id === entry.id &&
                b.entry_version === entry.version,
            ),
        );
  const selectedRequest = !requests.isError
    ? requests.data?.find((r) => r.id === selected)
    : undefined;
  const mutate = async (action: () => Promise<unknown>, close = true) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await action();
      await client.invalidateQueries();
      if (close) {
        setEditing(null);
        setSelected(null);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const queryError = policies.error ?? requests.error ?? catalog.error;
  return (
    <div className="generation-page">
      {subjectTenant === null ? (
        <form
          className="generation-tenant"
          onSubmit={(event) => {
            event.preventDefault();
            const tenant = String(
              new FormData(event.currentTarget).get("tenant"),
            ).trim();
            setChosenTenant(tenant);
            setSelected(null);
            setEditing(null);
            setError("");
          }}
        >
          <Field label={t("tenant")}>
            <input name="tenant" required maxLength={256} />
          </Field>
          <button>{t("open")}</button>
        </form>
      ) : (
        <p className="muted">
          {t("tenant")}: {tenant}
        </p>
      )}
      {!tenant && <p className="notice">{t("generationTenantHelp")}</p>}
      {error && !editing && !selected && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {queryError && (
        <p className="error" role="alert">
          {queryError.message}
        </p>
      )}
      {tenant && (
        <>
          <Panel
            title={t("generationPolicies")}
            action={
              <button
                disabled={
                  busy || (subjectTenant === null && !catalog.isSuccess)
                }
                onClick={() => {
                  setError("");
                  setEditing("new");
                }}
              >
                <Plus size={16} />
                {t("newGenerationPolicy")}
              </button>
            }
          >
            {policies.isPending ? (
              <p className="generation-padding">{t("loading")}</p>
            ) : (
              !policies.isError && (
                <div className="generation-padding">
                  {!policies.data?.length && (
                    <p className="muted">{t("generationNoPolicies")}</p>
                  )}
                  {policies.data?.map((policy) => (
                    <article className="generation-policy" key={policy.id}>
                      <div>
                        <h3>{policyPresentation.name(policy)}</h3>
                        <p className="muted">
                          {t("revision")} {policy.revision}
                        </p>
                      </div>
                      <Badge
                        value={policy.spec.enabled ? "ENABLED" : "DISABLED"}
                      />
                      <dl>
                        <dt>{t("generationCount")}</dt>
                        <dd>
                          {policy.generated_count} /{" "}
                          {policy.spec.limits.max_agents}
                        </dd>
                        <dt>{t("generationAllocated")}</dt>
                        <dd>
                          {policy.allocated_tokens.toLocaleString()} /{" "}
                          {policy.spec.limits.token_budget.toLocaleString()}
                        </dd>
                        <dt>{t("generationCompactionBudget")}</dt>
                        <dd>
                          {policy.allocated_compaction_calls} /{" "}
                          {policy.spec.compaction?.call_budget ?? "—"}
                        </dd>
                        <dt>{t("generationEmbeddingBudget")}</dt>
                        <dd>
                          {policy.allocated_embedding_calls} /{" "}
                          {policy.spec.embedding?.call_budget ?? "—"}
                        </dd>
                      </dl>
                      <div className="generation-actions">
                        <button
                          disabled={busy}
                          onClick={() => {
                            setError("");
                            setEditing(policy);
                          }}
                        >
                          {t("editPolicy")}
                        </button>
                        <button
                          disabled={busy}
                          onClick={() =>
                            void mutate(
                              () =>
                                generationSetPolicy(
                                  encodeURIComponent(tenant),
                                  encodeURIComponent(policy.id),
                                  {
                                    expected_revision: policy.revision,
                                    spec: {
                                      ...policy.spec,
                                      enabled: !policy.spec.enabled,
                                    },
                                  },
                                ),
                              false,
                            )
                          }
                        >
                          {t(
                            policy.spec.enabled
                              ? "disableGeneration"
                              : "enableGeneration",
                          )}
                        </button>
                      </div>
                    </article>
                  ))}
                </div>
              )
            )}
          </Panel>
          <Panel
            title={t("generationRequests")}
            action={
              <span className="count">
                {requests.isError ? "—" : (requests.data?.length ?? 0)}
              </span>
            }
          >
            {requests.isPending ? (
              <p className="generation-padding">{t("loading")}</p>
            ) : (
              !requests.isError && (
                <div className="generation-padding">
                  {!requests.data?.length && (
                    <p className="muted">{t("generationNoRequests")}</p>
                  )}
                  {requests.data?.map((request) => (
                    <button
                      className="generation-request"
                      key={request.id}
                      onClick={() => {
                        setError("");
                        setSelected(request.id);
                      }}
                    >
                      <div>
                        <strong>
                          {data.tasks.find(
                            (task) => task.id === request.task_id,
                          )?.title || t("task")}
                        </strong>
                        <p>{request.reason}</p>
                        <small>
                          {(() => {
                            const policy = policies.data?.find(
                              (item) => item.id === request.policy_id,
                            );
                            return policy
                              ? policyPresentation.label(policy)
                              : `${t("unavailableEntity")} · ${t("revision")} ${request.policy_revision}`;
                          })()}
                        </small>
                      </div>
                      <Badge value={request.status} />
                      <ArrowUpRight size={16} />
                    </button>
                  ))}
                </div>
              )
            )}
          </Panel>
        </>
      )}
      {editing && (
        <Modal
          title={t(editing === "new" ? "newGenerationPolicy" : "editPolicy")}
          close={() => setEditing(null)}
        >
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          <fieldset className="generation-fieldset" disabled={busy}>
            <PolicyEditor
              entries={entries}
              policy={editing === "new" ? undefined : editing}
              save={(id, spec) =>
                mutate(() =>
                  generationSetPolicy(
                    encodeURIComponent(tenant),
                    encodeURIComponent(id),
                    {
                      expected_revision:
                        editing === "new" ? 0 : editing.revision,
                      spec,
                    },
                  ),
                )
              }
            />
          </fieldset>
        </Modal>
      )}
      {selected && (
        <Modal title={t("generationDetails")} close={() => setSelected(null)}>
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          {selectedRequest ? (
            <RequestDetail
              key={selectedRequest.id}
              tenant={tenant}
              request={selectedRequest}
              data={data}
              busy={busy}
              control={(action, reason) =>
                mutate(() =>
                  generationControl(
                    encodeURIComponent(tenant),
                    selectedRequest.id,
                    {
                      action,
                      reason,
                    },
                  ),
                )
              }
            />
          ) : (
            <p role="status">{t("generationUnavailable")}</p>
          )}
        </Modal>
      )}
    </div>
  );
}

function RefChoices({
  label,
  name,
  entries,
  kind,
  selected = [],
}: {
  label: string;
  name: string;
  entries: Entry[];
  kind: string;
  selected?: EntityRef[];
}) {
  const { t } = useI18n();
  const entityLabel = useEntityLabel(entries);
  const available = entries.filter((entry) => entry.kind === kind);
  const choices = [
    ...available.map((entry) => ({
      value: key(entry),
      label: entityLabel(entry),
    })),
    ...selected
      .filter((r) => !available.some((entry) => key(entry) === key(r)))
      .map((r) => ({
        value: key(r),
        label: `${t("generationReferenceUnavailable")} · ${r.version}`,
      })),
  ];
  return (
    <Field label={label}>
      <select
        name={name}
        multiple
        defaultValue={selected.map(key)}
        size={Math.min(4, Math.max(2, choices.length))}
      >
        {choices.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
    </Field>
  );
}

function PolicyEditor({
  entries,
  policy,
  save,
}: {
  entries: Entry[];
  policy?: GenerationPolicy;
  save: (id: string, spec: GenerationSpec) => Promise<void>;
}) {
  const { t } = useI18n();
  const entityLabel = useEntityLabel(entries);
  const [policyId] = useState(() => policy?.id ?? crypto.randomUUID());
  const initial = policy?.spec;
  const config = initial?.template.config as AgentFields | undefined;
  const [model, setModel] = useState(config?.model ? key(config.model) : "");
  const [compactor, setCompactor] = useState(
    initial?.compaction ? key(initial.compaction.provider) : "",
  );
  const [embedding, setEmbedding] = useState(
    initial?.embedding ? key(initial.embedding.provider) : "",
  );
  const [error, setError] = useState("");
  const embeddings = entries.filter((entry) => entry.kind === "embedding");
  const compactors = entries.filter((entry) => entry.kind === "compactor");
  const models = entries.filter((entry) => entry.kind === "model");
  const modelEntry = models.find((entry) => key(entry) === model);
  const window = Number(modelEntry?.config.context_window ?? 0);
  const minTokens = window
    ? window + Math.min(4096, Math.max(256, Math.floor(window / 8)))
    : 1;
  const limits = initial?.limits ?? {
    max_agents: 20,
    max_concurrent: 4,
    max_depth: 3,
    token_budget: 1000000,
    tokens_per_agent: 200000,
    lifetime_seconds: 3600,
  };
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setError("");
    const form = new FormData(event.currentTarget);
    const text = (name: string) => String(form.get(name) ?? "");
    if (!text("instructions").trim() && !form.getAll("skills").length) {
      setError(t("agentNeedsSkill"));
      return;
    }
    try {
      const attributes: unknown = JSON.parse(text("attributes"));
      if (
        !attributes ||
        typeof attributes !== "object" ||
        Array.isArray(attributes)
      )
        throw new Error(t("jsonHint"));
      const id = policyId;
      const template: Entry = {
        id: initial?.template.id ?? `template-${id}`,
        version: text("version"),
        kind: "agent",
        name: localized(
          initial?.template.name,
          text("name_en"),
          text("name_ja"),
        ),
        description: localized(
          initial?.template.description,
          text("description_en"),
          text("description_ja"),
        ),
        capabilities: split(text("capabilities")),
        languages: split(text("languages")),
        tags: initial?.template.tags ?? [],
        skills: initial?.template.skills ?? [],
        schema: initial?.template.schema ?? {},
        config: {
          ...initial?.template.config,
          model: entityRef(model),
          instructions: text("instructions"),
          tools: form.getAll("tools").map((v) => entityRef(String(v))),
          skills: form.getAll("skills").map((v) => entityRef(String(v))),
          cluster: text("cluster") ? entityRef(text("cluster")) : null,
          max_steps: Number(text("max_steps")),
        },
      };
      void save(id, {
        enabled: form.has("enabled"),
        approval_required: form.has("approval"),
        compaction: compactor
          ? {
              provider: entityRef(compactor),
              calls_per_agent: Number(text("calls_per_agent")),
              call_budget: Number(text("call_budget")),
            }
          : undefined,
        embedding: embedding
          ? {
              provider: entityRef(embedding),
              calls_per_agent: Number(text("embedding_calls_per_agent")),
              call_budget: Number(text("embedding_call_budget")),
            }
          : undefined,
        template,
        permissions: {
          roles: split(text("roles")),
          groups: split(text("groups")),
          attributes: attributes as Record<string, unknown>,
        },
        limits: {
          max_agents: Number(text("max_agents")),
          max_concurrent: Number(text("max_concurrent")),
          max_depth: Number(text("max_depth")),
          token_budget: Number(text("token_budget")),
          tokens_per_agent: Number(text("tokens_per_agent")),
          lifetime_seconds: Number(text("lifetime_seconds")),
        },
      });
    } catch {
      setError(t("jsonHint"));
    }
  };
  return (
    <form onSubmit={submit} className="generation-editor">
      <div className="two-columns">
        <Field label={t("version")}>
          <input
            name="version"
            required
            defaultValue={initial?.template.version ?? "1.0.0"}
          />
        </Field>
      </div>
      <div className="generation-actions">
        <label className="generation-check">
          <input
            name="enabled"
            type="checkbox"
            defaultChecked={initial?.enabled ?? true}
          />
          {t("enabled")}
        </label>
        <label className="generation-check">
          <input
            name="approval"
            type="checkbox"
            defaultChecked={initial?.approval_required ?? true}
          />
          {t("generationRequireApproval")}
        </label>
      </div>
      <h3>{t("generationTemplate")}</h3>
      <div className="two-columns">
        <Field label={`${t("name")} · English`}>
          <input
            name="name_en"
            required
            defaultValue={
              initial?.template.name["en-US"] ?? initial?.template.name.en
            }
          />
        </Field>
        <Field label={`${t("name")} · 日本語`}>
          <input
            name="name_ja"
            defaultValue={
              initial?.template.name["ja-JP"] ?? initial?.template.name.ja
            }
          />
        </Field>
      </div>
      <Field label={`${t("description")} · English`}>
        <textarea
          name="description_en"
          required
          defaultValue={
            initial?.template.description["en-US"] ??
            initial?.template.description.en
          }
        />
      </Field>
      <Field label={`${t("description")} · 日本語`}>
        <textarea
          name="description_ja"
          defaultValue={
            initial?.template.description["ja-JP"] ??
            initial?.template.description.ja
          }
        />
      </Field>
      <div className="two-columns">
        <Field label={t("capabilities")}>
          <input
            name="capabilities"
            placeholder={t("commaSeparated")}
            defaultValue={initial?.template.capabilities.join(", ")}
          />
        </Field>
        <Field label={t("languages")}>
          <input
            name="languages"
            required
            defaultValue={initial?.template.languages.join(", ") ?? "en, ja"}
          />
        </Field>
      </div>
      <Field label={t("model")}>
        <select
          required
          value={model}
          onChange={(event) => setModel(event.target.value)}
        >
          <option value="">{t("choose")}</option>
          {models.map((entry) => (
            <option key={key(entry)} value={key(entry)}>
              {entityLabel(entry)}
            </option>
          ))}
          {model && !modelEntry && (
            <option value={model}>{t("generationReferenceUnavailable")}</option>
          )}
        </select>
      </Field>
      <p className="muted">
        {t("generationModelHelp")}
        {window > 0 &&
          ` ${t("generationMinReservation")}: ${minTokens.toLocaleString()}`}
      </p>
      <div className="two-columns">
        <RefChoices
          label={t("tools")}
          name="tools"
          entries={entries}
          kind="tool"
          selected={config?.tools}
        />
        <RefChoices
          label={t("generationSkills")}
          name="skills"
          entries={entries}
          kind="skill"
          selected={config?.skills}
        />
      </div>
      <p className="muted">{t("generationMultiSelect")}</p>
      <Field label={t("additionalInstructions")}>
        <textarea
          name="instructions"
          rows={4}
          defaultValue={config?.instructions}
        />
      </Field>

      <Field label={t("cluster")}>
        <select
          name="cluster"
          defaultValue={config?.cluster ? key(config.cluster) : ""}
        >
          <option value="">{t("generationNoCluster")}</option>
          {entries
            .filter((entry) => entry.kind === "cluster")
            .map((entry) => (
              <option key={key(entry)} value={key(entry)}>
                {entityLabel(entry)}
              </option>
            ))}
          {config?.cluster &&
            !entries.some((entry) => key(entry) === key(config.cluster!)) && (
              <option value={key(config.cluster)}>
                {t("generationReferenceUnavailable")} · {config.cluster.version}
              </option>
            )}
        </select>
      </Field>
      <h3>{t("permissions")}</h3>
      <p className="muted">{t("generationPermissionsHelp")}</p>
      <div className="two-columns">
        <Field label={t("generationRoles")}>
          <input
            name="roles"
            defaultValue={initial?.permissions.roles?.join(", ")}
            placeholder={t("commaSeparated")}
          />
        </Field>
        <Field label={t("generationGroups")}>
          <input
            name="groups"
            defaultValue={initial?.permissions.groups?.join(", ")}
            placeholder={t("commaSeparated")}
          />
        </Field>
      </div>
      <Field label={t("generationAttributes")}>
        <textarea
          name="attributes"
          rows={3}
          defaultValue={JSON.stringify(
            initial?.permissions.attributes ?? {},
            null,
            2,
          )}
        />
      </Field>
      <h3>{t("generationCompaction")}</h3>
      <p className="muted">{t("generationCompactionHelp")}</p>
      <Field label={t("generationCompactor")}>
        <select
          name="compactor"
          value={compactor}
          onChange={(e) => setCompactor(e.target.value)}
        >
          <option value="">{t("generationCompactionDisabled")}</option>
          {compactors.map((entry) => (
            <option key={key(entry)} value={key(entry)}>
              {entityLabel(entry)}
            </option>
          ))}
          {compactor &&
            !compactors.some((entry) => key(entry) === compactor) && (
              <option value={compactor}>
                {t("generationReferenceUnavailable")}
              </option>
            )}
        </select>
      </Field>
      {compactor && (
        <div className="two-columns">
          <Field label={t("generationCompactionPerAgent")}>
            <input
              type="number"
              name="calls_per_agent"
              required
              min={1}
              max={1000000}
              defaultValue={initial?.compaction?.calls_per_agent ?? 10}
            />
          </Field>
          <Field label={t("generationCompactionBudget")}>
            <input
              type="number"
              name="call_budget"
              required
              min={1}
              max={1000000}
              defaultValue={initial?.compaction?.call_budget ?? 100}
            />
          </Field>
        </div>
      )}
      <h3>{t("generationEmbedding")}</h3>
      <p className="muted">{t("generationEmbeddingHelp")}</p>
      <Field label={t("generationEmbedder")}>
        <select
          name="embedding"
          value={embedding}
          onChange={(e) => setEmbedding(e.target.value)}
        >
          <option value="">{t("generationEmbeddingDisabled")}</option>
          {embeddings.map((entry) => (
            <option key={key(entry)} value={key(entry)}>
              {entityLabel(entry)}
            </option>
          ))}
          {embedding &&
            !embeddings.some((entry) => key(entry) === embedding) && (
              <option value={embedding}>
                {t("generationReferenceUnavailable")}
              </option>
            )}
        </select>
      </Field>
      {embedding && (
        <div className="two-columns">
          <Field label={t("generationEmbeddingPerAgent")}>
            <input
              type="number"
              name="embedding_calls_per_agent"
              required
              min={1}
              max={1000000}
              defaultValue={initial?.embedding?.calls_per_agent ?? 10}
            />
          </Field>
          <Field label={t("generationEmbeddingBudget")}>
            <input
              type="number"
              name="embedding_call_budget"
              required
              min={1}
              max={1000000}
              defaultValue={initial?.embedding?.call_budget ?? 100}
            />
          </Field>
        </div>
      )}
      <h3>{t("generationLimits")}</h3>
      <p className="muted">{t("generationBudgetHelp")}</p>
      <div className="two-columns">
        {(
          [
            ["max_agents", "generationMaxAgents", 1, 512],
            ["max_concurrent", "generationMaxConcurrent", 1, 512],
            ["max_depth", "generationMaxDepth", 1, 31],
            ["token_budget", "generationTokenBudget", 1, 1000000000000],
            [
              "tokens_per_agent",
              "generationTokensPerAgent",
              minTokens,
              1000000000000,
            ],
            ["lifetime_seconds", "generationLifetime", 1, 2592000],
          ] as const
        ).map(([name, label, min, max]) => (
          <Field key={name} label={t(label)}>
            <input
              type="number"
              required
              name={name}
              min={min}
              max={max}
              step={1}
              defaultValue={limits[name]}
            />
          </Field>
        ))}
        <Field label={t("generationMaxSteps")}>
          <input
            type="number"
            name="max_steps"
            required
            min={1}
            max={1000}
            defaultValue={config?.max_steps ?? 64}
          />
        </Field>
      </div>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      <button className="primary" disabled={!models.length && !model}>
        {t("save")}
      </button>
    </form>
  );
}

function RequestDetail({
  tenant,
  request,
  data,
  busy,
  control,
}: {
  tenant: string;
  request: GenerationRequest;
  data: State;
  busy: boolean;
  control: (action: GenerationAction, reason: string) => Promise<void>;
}) {
  const { t, local, locale } = useI18n();
  const entryLabel = useEntryLabel(data.registry);
  const history = useQuery({
    queryKey: ["generation", tenant, "history", request.id],
    queryFn: () => generationHistory(encodeURIComponent(tenant), request.id),
    refetchInterval: 3000,
  });
  const usage = useQuery({
    queryKey: ["generation", tenant, "usage", request.id],
    queryFn: () => generationUsage(encodeURIComponent(tenant), request.id),
    refetchInterval: 3000,
  });
  const pinned = useQuery({
    queryKey: ["generation", tenant, "spec", request.id],
    queryFn: () => generationSpec(encodeURIComponent(tenant), request.id),
  });
  const config = request.definition.config as AgentFields;
  return (
    <>
      <h3>
        {data.tasks.find((task) => task.id === request.task_id)?.title ??
          t("task")}
      </h3>
      <Badge value={request.status} />
      <p className="generation-reason">{request.reason}</p>
      <dl>
        <dt>{t("generationTemplate")}</dt>
        <dd>
          {local(request.definition.name)} · {request.agent_version}
        </dd>
        <dt>{t("model")}</dt>
        <dd>{config.model && entryLabel(config.model)}</dd>
        <dt>{t("tools")}</dt>
        <dd>{config.tools?.map(entryLabel).join(", ") || "—"}</dd>
        <dt>{t("generationSkills")}</dt>
        <dd>{config.skills?.map(entryLabel).join(", ") || "—"}</dd>
        <dt>{t("capabilities")}</dt>
        <dd>{request.definition.capabilities.join(", ") || "—"}</dd>
        <dt>{t("generationPolicy")}</dt>
        <dd>
          {local(pinned.data?.template.name ?? {}) || t("unavailableEntity")} ·{" "}
          {t("revision")} {request.policy_revision}
        </dd>
        <dt>{t("generationOrigin")}</dt>
        <dd>{request.subject_chain.join(" → ")}</dd>
        <dt>{t("generationDepth")}</dt>
        <dd>{request.depth}</dd>
        <dt>{t("generationExpires")}</dt>
        <dd>{new Date(request.expires_at).toLocaleString(locale)}</dd>
      </dl>
      {usage.isError ? (
        <p role="alert" className="error">
          {usage.error.message}
        </p>
      ) : (
        usage.data && (
          <dl>
            <dt>{t("generationUsedTokens")}</dt>
            <dd>
              {usage.data.used_tokens.toLocaleString()} /{" "}
              {usage.data.token_limit.toLocaleString()}
            </dd>
            <dt>{t("generationInferenceAttempts")}</dt>
            <dd>{usage.data.inference_attempts}</dd>
            <dt>{t("generationCompactionCalls")}</dt>
            <dd>
              {usage.data.compaction_calls} / {usage.data.compaction_call_limit}
            </dd>
            <dt>{t("generationEmbeddingCalls")}</dt>
            <dd>
              {usage.data.embedding_calls} / {usage.data.embedding_call_limit}
            </dd>
          </dl>
        )
      )}
      {pinned.isError ? (
        <p role="alert" className="error">
          {pinned.error.message}
        </p>
      ) : (
        pinned.data && (
          <details className="generation-permissions" open>
            <summary>{t("generationPinnedPermissions")}</summary>
            <dl>
              <dt>{t("generationRoles")}</dt>
              <dd>{pinned.data.permissions.roles?.join(", ") || "—"}</dd>
              <dt>{t("generationGroups")}</dt>
              <dd>{pinned.data.permissions.groups?.join(", ") || "—"}</dd>
              <dt>{t("generationMaxConcurrent")}</dt>
              <dd>{pinned.data.limits.max_concurrent}</dd>
              <dt>{t("generationMaxDepth")}</dt>
              <dd>{pinned.data.limits.max_depth}</dd>
              <dt>{t("generationCompactor")}</dt>
              <dd>
                {pinned.data.compaction
                  ? entryLabel(pinned.data.compaction.provider)
                  : t("generationCompactionDisabled")}
              </dd>
              {pinned.data.compaction && (
                <>
                  <dt>{t("generationCompactionPerAgent")}</dt>
                  <dd>{pinned.data.compaction.calls_per_agent}</dd>
                </>
              )}
              <dt>{t("generationEmbedder")}</dt>
              <dd>
                {pinned.data.embedding
                  ? entryLabel(pinned.data.embedding.provider)
                  : t("generationEmbeddingDisabled")}
              </dd>
              {pinned.data.embedding && (
                <>
                  <dt>{t("generationEmbeddingPerAgent")}</dt>
                  <dd>{pinned.data.embedding.calls_per_agent}</dd>
                </>
              )}
            </dl>
            <JsonView value={pinned.data.permissions.attributes ?? {}} />
          </details>
        )
      )}
      <details>
        <summary>{t("generationDefinition")}</summary>
        <RecordView value={request.definition} />
      </details>
      {request.status !== "DELETED" && (
        <form
          className="generation-control"
          onSubmit={(event) => {
            event.preventDefault();
            const action = (event.nativeEvent as SubmitEvent)
              .submitter as HTMLButtonElement | null;
            if (action)
              void control(
                action.value as GenerationAction,
                String(new FormData(event.currentTarget).get("reason")),
              );
          }}
        >
          <fieldset disabled={busy} className="generation-fieldset">
            <Field label={t("generationDecisionReason")}>
              <textarea name="reason" required maxLength={4096} rows={2} />
            </Field>
            <div className="generation-actions">
              {request.status === "PENDING_APPROVAL" && (
                <>
                  <button
                    className="primary"
                    value="approve"
                    disabled={!pinned.isSuccess}
                  >
                    {t("generationApprove")}
                  </button>
                  <button value="deny">{t("generationDeny")}</button>
                </>
              )}
              {active(request.status) ? (
                <button className="danger" value="stop">
                  {t("generationStop")}
                </button>
              ) : (
                <button value="delete">{t("generationDelete")}</button>
              )}
            </div>
          </fieldset>
        </form>
      )}
      <h4>{t("generationHistory")}</h4>
      {history.isError ? (
        <p role="alert" className="error">
          {history.error.message}
        </p>
      ) : (
        <ol className="generation-history">
          {history.data?.map((entry) => (
            <li key={entry.sequence}>
              <Badge value={entry.status} />
              <strong>{entry.actor}</strong>
              <time>{new Date(entry.created_at).toLocaleString(locale)}</time>
              <p>{entry.reason}</p>
            </li>
          ))}
        </ol>
      )}
    </>
  );
}

export function GenerationAssignForm({
  tenant,
  task,
  submit,
}: {
  tenant: string;
  task: Task;
  submit: Submit;
}) {
  const { t } = useI18n();
  const policies = useQuery({
    queryKey: ["generation", tenant, "policies"],
    queryFn: () => generationPolicies(encodeURIComponent(tenant)),
  });
  const choices = policies.isError
    ? []
    : (policies.data?.filter((policy) => policy.spec.enabled) ?? []);
  const policyPresentation = usePolicyPresentation(
    policies.isError ? [] : (policies.data ?? []),
  );
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        const form = new FormData(event.currentTarget);
        void submit(() =>
          generationAssign(encodeURIComponent(tenant), task.id, {
            policy_id: String(form.get("policy")),
            reason: String(form.get("reason")),
          }),
        );
      }}
    >
      <h3>{task.title}</h3>
      <p>{t("generationAssignHelp")}</p>
      {policies.isError && (
        <p role="alert" className="error">
          {policies.error.message}
        </p>
      )}
      <Field label={t("generationPolicy")}>
        <select name="policy" required defaultValue="">
          <option value="">{t("choose")}</option>
          {choices.map((policy) => (
            <option key={policy.id} value={policy.id}>
              {policyPresentation.label(policy)}
            </option>
          ))}
        </select>
      </Field>
      {!policies.isPending && !choices.length && (
        <p className="notice">{t("generationNoPolicies")}</p>
      )}
      <Field label={t("generationRequestReason")}>
        <textarea name="reason" required maxLength={4096} rows={3} />
      </Field>
      <button className="primary" disabled={!choices.length}>
        {t("generationAssign")}
      </button>
    </form>
  );
}
