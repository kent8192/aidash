import {
  AgentBindings,
  coordinatorDefaults,
  hasInstructionalBinding,
  type Binding,
  type BindingConfiguration,
} from "./agent-bindings";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import { Table, TableBody, TableCell, TableRow } from "./components/ui/table";
import { RemoteGenerationAssignForm } from "./remote-generation";
import { RecordView } from "./record-view";
import { disambiguateLabels } from "./display-labels";
import { useState, type FormEvent } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, ChevronRight } from "lucide-react";
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
  Alert,
  Check,
  Disclosure,
  Facts,
  Hint,
  Loading,
  Metric,
  MetricRow,
  RowList,
  Section,
} from "./components/patterns";
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
  GenerationRemoteApprovals,
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
  schema_version?: number;
  bindings?: Binding[];
  remove_default?: string[];
  cluster?: EntityRef | null;
  instructions?: string;
  max_steps?: number;
};

export function GenerationPage({ data }: { data: State }) {
  const { t, locale } = useI18n();
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
    <div className="generation-page grid min-w-0 gap-6">
      <div className="grid min-w-0 gap-3">
        {subjectTenant === null ? (
          <form
            className="generation-tenant flex flex-wrap items-end gap-2"
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
            <div className="w-full min-w-0 sm:w-72">
              <Field label={t("tenant")}>
                <Input
                  name="tenant"
                  required
                  maxLength={256}
                  autoComplete="off"
                  spellCheck={false}
                  className="font-mono"
                />
              </Field>
            </div>
            <Button variant="outline">{t("open")}</Button>
          </form>
        ) : (
          <p className="text-xs text-muted-foreground">
            {t("tenant")}{" "}
            <span className="font-mono text-foreground">{tenant}</span>
          </p>
        )}
        {!tenant && <Hint>{t("generationTenantHelp")}</Hint>}
        {error && !editing && !selected && <Alert>{error}</Alert>}
        {queryError && <Alert>{queryError.message}</Alert>}
      </div>
      {tenant && (
        <>
          <Panel
            title={t("generationPolicies")}
            action={
              <Button
                size="sm"
                disabled={
                  busy || (subjectTenant === null && !catalog.isSuccess)
                }
                onClick={() => {
                  setError("");
                  setEditing("new");
                }}
              >
                <Plus />
                {t("newGenerationPolicy")}
              </Button>
            }
          >
            {policies.isPending ? (
              <Loading />
            ) : (
              !policies.isError &&
              (policies.data.length ? (
                <RowList>
                  {policies.data.map((policy) => (
                    <article
                      className="generation-policy grid min-w-0 gap-x-6 gap-y-2 py-3 lg:grid-cols-[minmax(0,1.3fr)_minmax(0,2fr)_auto] lg:items-center"
                      key={policy.id}
                    >
                      <div className="grid min-w-0 gap-0.5">
                        <div className="flex min-w-0 items-center gap-2">
                          <h3 className="truncate text-[13px] font-medium text-foreground">
                            {policyPresentation.name(policy)}
                          </h3>
                          <Badge
                            value={policy.spec.enabled ? "ENABLED" : "DISABLED"}
                          />
                        </div>
                        <p className="truncate font-mono text-[11px] text-faint">
                          {policy.tenant} · {t("revision")} {policy.revision}
                        </p>
                      </div>
                      <MetricRow compact columns={4} className="border-b-0">
                        <Metric
                          label={t("generationCount")}
                          value={`${policy.generated_count} / ${policy.spec.limits.max_agents}`}
                        />
                        <Metric
                          label={t("generationAllocated")}
                          value={`${policy.allocated_tokens.toLocaleString(locale)} / ${policy.spec.limits.token_budget.toLocaleString(locale)}`}
                        />
                        <Metric
                          label={t("generationCompactionBudget")}
                          value={`${policy.allocated_compaction_calls} / ${policy.spec.compaction?.call_budget ?? "-"}`}
                        />
                        <Metric
                          label={t("generationEmbeddingBudget")}
                          value={`${policy.allocated_embedding_calls} / ${policy.spec.embedding?.call_budget ?? "-"}`}
                        />
                      </MetricRow>
                      <div className="flex flex-wrap items-center gap-1.5 lg:justify-end">
                        <Button
                          variant="outline"
                          size="sm"
                          disabled={busy}
                          onClick={() => {
                            setError("");
                            setEditing(policy);
                          }}
                        >
                          {t("editPolicy")}
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
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
                        </Button>
                      </div>
                    </article>
                  ))}
                </RowList>
              ) : (
                <p className="text-xs text-muted-foreground">
                  {t("generationNoPolicies")}
                </p>
              ))
            )}
          </Panel>
          <Panel
            title={t("generationRequests")}
            action={
              <span className="rounded-sm bg-raised px-1.5 font-mono text-[11px] tabular text-muted-foreground">
                {requests.isError ? "-" : (requests.data?.length ?? 0)}
              </span>
            }
          >
            {requests.isPending ? (
              <Loading />
            ) : (
              !requests.isError &&
              (requests.data.length ? (
                <RowList>
                  {requests.data.map((request) => {
                    const policy = policies.data?.find(
                      (item) => item.id === request.policy_id,
                    );
                    return (
                      <button
                        type="button"
                        className="generation-request group mb-0 grid w-full min-w-0 cursor-pointer grid-cols-[minmax(0,1fr)_auto_auto] items-center gap-3 px-2 py-2 text-left transition-colors duration-150 hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
                        key={request.id}
                        onClick={() => {
                          setError("");
                          setSelected(request.id);
                        }}
                      >
                        <span className="grid min-w-0 gap-0.5">
                          <span className="truncate font-medium text-foreground">
                            {data.tasks.find(
                              (task) => task.id === request.task_id,
                            )?.title || t("task")}
                          </span>
                          <span className="truncate text-xs text-muted-foreground">
                            {request.reason}
                          </span>
                          <span className="truncate text-[11px] text-faint">
                            {policy
                              ? policyPresentation.label(policy)
                              : `${t("unavailableEntity")} · ${t("revision")} ${request.policy_revision}`}
                            {" · "}
                            <time
                              className="font-mono"
                              dateTime={request.expires_at}
                            >
                              {new Date(request.expires_at).toLocaleString(
                                locale,
                              )}
                            </time>
                          </span>
                        </span>
                        <Badge value={request.status} />
                        <ChevronRight
                          aria-hidden
                          className="size-3.5 text-faint transition-colors group-hover:text-foreground"
                        />
                      </button>
                    );
                  })}
                </RowList>
              ) : (
                <p className="text-xs text-muted-foreground">
                  {t("generationNoRequests")}
                </p>
              ))
            )}
          </Panel>
        </>
      )}
      {editing && (
        <Modal
          title={t(editing === "new" ? "newGenerationPolicy" : "editPolicy")}
          close={() => setEditing(null)}
        >
          {error && <Alert>{error}</Alert>}
          <fieldset className="min-w-0" disabled={busy}>
            <PolicyEditor
              entries={entries}
              node={data.node.id}
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
          {error && <Alert>{error}</Alert>}
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
            <Hint role="status">{t("generationUnavailable")}</Hint>
          )}
        </Modal>
      )}
    </div>
  );
}

function PolicyEditor({
  node,
  entries,
  policy,
  save,
}: {
  node: string;
  entries: Entry[];
  policy?: GenerationPolicy;
  save: (id: string, spec: GenerationSpec) => Promise<void>;
}) {
  const { t, locale } = useI18n();
  const entityLabel = useEntityLabel(entries);
  const [policyId] = useState(() => policy?.id ?? crypto.randomUUID());
  const initial = policy?.spec;
  const config = initial?.template.config as AgentFields | undefined;
  const [bindings, setBindings] = useState<BindingConfiguration>({
    bindings: config?.bindings ?? [],
    remove_default: config?.remove_default ?? [],
  });
  const [cluster, setCluster] = useState(
    config?.cluster ? key(config.cluster) : "",
  );
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
    if (
      !text("instructions").trim() &&
      !hasInstructionalBinding(bindings.bindings, entries, node)
    ) {
      setError(t("agentNeedsSkill"));
      return;
    }
    try {
      const attributes: unknown = JSON.parse(text("attributes"));
      const remote: unknown = JSON.parse(text("remote_approvals"));
      if (!remote || typeof remote !== "object" || Array.isArray(remote))
        throw new Error(t("jsonHint"));
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
          schema_version: 1,
          ...bindings,
          cluster: text("cluster") ? entityRef(text("cluster")) : null,
          max_steps: Number(text("max_steps")),
        },
      };
      void save(id, {
        remote: remote as GenerationRemoteApprovals,
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
    <form onSubmit={submit} className="grid min-w-0 gap-4">
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={t("version")}>
          <Input
            name="version"
            required
            className="font-mono"
            defaultValue={initial?.template.version ?? "1.0.0"}
          />
        </Field>
      </div>
      <div className="flex flex-wrap gap-x-5 gap-y-2">
        <Check name="enabled" defaultChecked={initial?.enabled ?? true}>
          {t("enabled")}
        </Check>
        <Check
          name="approval"
          defaultChecked={initial?.approval_required ?? true}
        >
          {t("generationRequireApproval")}
        </Check>
      </div>
      <Section level={3} title={t("generationTemplate")}>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label={`${t("name")} · English`}>
            <Input
              name="name_en"
              required
              defaultValue={
                initial?.template.name["en-US"] ?? initial?.template.name.en
              }
            />
          </Field>
          <Field label={`${t("name")} · 日本語`}>
            <Input
              name="name_ja"
              defaultValue={
                initial?.template.name["ja-JP"] ?? initial?.template.name.ja
              }
            />
          </Field>
        </div>
        <Field label={`${t("description")} · English`}>
          <Textarea
            name="description_en"
            required
            rows={2}
            defaultValue={
              initial?.template.description["en-US"] ??
              initial?.template.description.en
            }
          />
        </Field>
        <Field label={`${t("description")} · 日本語`}>
          <Textarea
            name="description_ja"
            rows={2}
            defaultValue={
              initial?.template.description["ja-JP"] ??
              initial?.template.description.ja
            }
          />
        </Field>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label={t("capabilities")}>
            <Input
              name="capabilities"
              placeholder={t("commaSeparated")}
              defaultValue={initial?.template.capabilities.join(", ")}
            />
          </Field>
          <Field label={t("languages")}>
            <Input
              name="languages"
              required
              defaultValue={initial?.template.languages.join(", ") ?? "en, ja"}
            />
          </Field>
        </div>
        <div className="grid gap-1.5">
          <Field label={t("model")}>
            <NativeSelect
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
                <option value={model}>
                  {t("generationReferenceUnavailable")}
                </option>
              )}
            </NativeSelect>
          </Field>
          <p className="text-xs leading-relaxed text-muted-foreground">
            {t("generationModelHelp")}
            {window > 0 && (
              <>
                {" "}
                {t("generationMinReservation")}:{" "}
                <span className="font-mono tabular text-foreground">
                  {minTokens.toLocaleString(locale)}
                </span>
              </>
            )}
          </p>
        </div>
        <AgentBindings
          value={bindings}
          change={setBindings}
          cluster={Boolean(cluster)}
          entries={entries}
          node={node}
        />
        <Field label={t("additionalInstructions")}>
          <Textarea
            name="instructions"
            rows={4}
            defaultValue={config?.instructions}
          />
        </Field>
        <Field label={t("cluster")}>
          <NativeSelect
            name="cluster"
            value={cluster}
            onChange={(e) => {
              const selected = e.target.value;
              setCluster(selected);
              if (selected)
                setBindings((previous) => ({
                  ...previous,
                  remove_default: previous.remove_default.filter(
                    (name) => !coordinatorDefaults.includes(name),
                  ),
                }));
            }}
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
                  {t("generationReferenceUnavailable")} ·{" "}
                  {config.cluster.version}
                </option>
              )}
          </NativeSelect>
        </Field>
      </Section>
      <Section level={3}
        title={t("permissions")}
        description={t("generationPermissionsHelp")}
      >
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label={t("generationRoles")}>
            <Input
              name="roles"
              defaultValue={initial?.permissions.roles?.join(", ")}
              placeholder={t("commaSeparated")}
            />
          </Field>
          <Field label={t("generationGroups")}>
            <Input
              name="groups"
              defaultValue={initial?.permissions.groups?.join(", ")}
              placeholder={t("commaSeparated")}
            />
          </Field>
        </div>
        <Field label={t("generationAttributes")}>
          <Textarea
            name="attributes"
            rows={3}
            spellCheck={false}
            className="font-mono text-xs"
            defaultValue={JSON.stringify(
              initial?.permissions.attributes ?? {},
              null,
              2,
            )}
          />
        </Field>
      </Section>
      <Section level={3}
        title={t("generationCompaction")}
        description={t("generationCompactionHelp")}
      >
        <Field label={t("generationCompactor")}>
          <NativeSelect
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
          </NativeSelect>
        </Field>
        {compactor && (
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label={t("generationCompactionPerAgent")}>
              <Input
                type="number"
                name="calls_per_agent"
                required
                min={1}
                max={1000000}
                className="font-mono tabular"
                defaultValue={initial?.compaction?.calls_per_agent ?? 10}
              />
            </Field>
            <Field label={t("generationCompactionBudget")}>
              <Input
                type="number"
                name="call_budget"
                required
                min={1}
                max={1000000}
                className="font-mono tabular"
                defaultValue={initial?.compaction?.call_budget ?? 100}
              />
            </Field>
          </div>
        )}
      </Section>
      <Section level={3}
        title={t("generationEmbedding")}
        description={t("generationEmbeddingHelp")}
      >
        <Field label={t("generationEmbedder")}>
          <NativeSelect
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
          </NativeSelect>
        </Field>
        {embedding && (
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label={t("generationEmbeddingPerAgent")}>
              <Input
                type="number"
                name="embedding_calls_per_agent"
                required
                min={1}
                max={1000000}
                className="font-mono tabular"
                defaultValue={initial?.embedding?.calls_per_agent ?? 10}
              />
            </Field>
            <Field label={t("generationEmbeddingBudget")}>
              <Input
                type="number"
                name="embedding_call_budget"
                required
                min={1}
                max={1000000}
                className="font-mono tabular"
                defaultValue={initial?.embedding?.call_budget ?? 100}
              />
            </Field>
          </div>
        )}
      </Section>
      <Section level={3}
        title={t("generationLimits")}
        description={t("generationBudgetHelp")}
      >
        <div className="grid gap-3 sm:grid-cols-2">
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
              <Input
                type="number"
                required
                name={name}
                min={min}
                max={max}
                step={1}
                className="font-mono tabular"
                defaultValue={limits[name]}
              />
            </Field>
          ))}
          <Field label={t("generationMaxSteps")}>
            <Input
              type="number"
              name="max_steps"
              required
              min={1}
              max={1000}
              className="font-mono tabular"
              defaultValue={config?.max_steps ?? 64}
            />
          </Field>
        </div>
      </Section>
      <section className="grid min-w-0 gap-3 border-t border-border pt-4">
        <Disclosure
          summary={
            locale === "ja-JP"
              ? "別 Node のプロバイダー承認"
              : "Provider approvals at other nodes"
          }
        >
          <Hint>
            {locale === "ja-JP"
              ? "所有 Node、定義のバージョンとダイジェストを固定します。embedding と compaction はそれぞれ呼び出し予算が必要です。空のオブジェクトは遠隔プロバイダーを許可しません。"
              : "Pin each provider's owning node, version and digests. Embedding and compaction each require call allowances. An empty object grants no remote provider approval."}
          </Hint>
          <Field
            label={
              locale === "ja-JP"
                ? "遠隔プロバイダー承認（JSON）"
                : "Remote provider approvals (JSON)"
            }
          >
            <Textarea
              name="remote_approvals"
              defaultValue={JSON.stringify(initial?.remote ?? {}, null, 2)}
              rows={10}
              spellCheck={false}
              className="font-mono text-xs"
              required
            />
          </Field>
        </Disclosure>
      </section>
      {error && <Alert>{error}</Alert>}
      <div className="flex justify-end border-t border-border pt-4">
        <Button disabled={!models.length && !model}>{t("save")}</Button>
      </div>
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
  const pinnedRows = pinned.data
    ? ([
        [t("generationRoles"), pinned.data.permissions.roles?.join(", ") || "-"],
        [
          t("generationGroups"),
          pinned.data.permissions.groups?.join(", ") || "-",
        ],
        [t("generationMaxConcurrent"), pinned.data.limits.max_concurrent],
        [t("generationMaxDepth"), pinned.data.limits.max_depth],
        [
          t("generationCompactor"),
          pinned.data.compaction
            ? entryLabel(pinned.data.compaction.provider)
            : t("generationCompactionDisabled"),
        ],
        ...(pinned.data.compaction
          ? [
              [
                t("generationCompactionPerAgent"),
                pinned.data.compaction.calls_per_agent,
              ] as const,
            ]
          : []),
        [
          t("generationEmbedder"),
          pinned.data.embedding
            ? entryLabel(pinned.data.embedding.provider)
            : t("generationEmbeddingDisabled"),
        ],
        ...(pinned.data.embedding
          ? [
              [
                t("generationEmbeddingPerAgent"),
                pinned.data.embedding.calls_per_agent,
              ] as const,
            ]
          : []),
      ] as const)
    : [];
  return (
    <>
      <header className="grid min-w-0 gap-1.5">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <h3 className="min-w-0 text-[15px] font-semibold text-foreground [overflow-wrap:anywhere]">
            {data.tasks.find((task) => task.id === request.task_id)?.title ??
              t("task")}
          </h3>
          <Badge value={request.status} />
        </div>
        <p className="text-[13px] leading-relaxed text-muted-foreground [overflow-wrap:anywhere]">
          {request.reason}
        </p>
      </header>
      <Facts
        items={[
          [
            t("generationTemplate"),
            <>
              {local(request.definition.name)} ·{" "}
              <span className="font-mono">{request.agent_version}</span>
            </>,
          ],
          [t("model"), config.model && entryLabel(config.model)],
          [
            t("tools"),
            config.bindings
              ?.filter((b) => b.kind === "tool" || b.kind === "bundle")
              .map((b) => entryLabel(b.target))
              .join(", ") || "-",
          ],
          [
            t("generationSkills"),
            config.bindings
              ?.filter((b) => b.kind === "skill")
              .map((b) => entryLabel(b.target))
              .join(", ") || "-",
          ],
          [
            t("capabilities"),
            request.definition.capabilities.join(", ") || "-",
          ],
          [
            t("generationPolicy"),
            <>
              {local(pinned.data?.template.name ?? {}) ||
                t("unavailableEntity")}{" "}
              · {t("revision")}{" "}
              <span className="font-mono">{request.policy_revision}</span>
            </>,
          ],
          [
            t("generationOrigin"),
            <span className="font-mono">
              {request.subject_chain.join(" → ")}
            </span>,
          ],
          [
            t("generationDepth"),
            <span className="font-mono tabular">{request.depth}</span>,
          ],
          [
            t("generationExpires"),
            <time className="font-mono" dateTime={request.expires_at}>
              {new Date(request.expires_at).toLocaleString(locale)}
            </time>,
          ],
        ]}
      />
      {usage.isError ? (
        <Alert>{usage.error.message}</Alert>
      ) : (
        usage.data && (
          <MetricRow compact columns={4} className="border-y">
            <Metric
              label={t("generationUsedTokens")}
              value={`${usage.data.used_tokens.toLocaleString(locale)} / ${usage.data.token_limit.toLocaleString(locale)}`}
            />
            <Metric
              label={t("generationInferenceAttempts")}
              value={usage.data.inference_attempts}
            />
            <Metric
              label={t("generationCompactionCalls")}
              value={`${usage.data.compaction_calls} / ${usage.data.compaction_call_limit}`}
            />
            <Metric
              label={t("generationEmbeddingCalls")}
              value={`${usage.data.embedding_calls} / ${usage.data.embedding_call_limit}`}
            />
          </MetricRow>
        )
      )}
      {pinned.isError ? (
        <Alert>{pinned.error.message}</Alert>
      ) : (
        pinned.data && (
          <section className="generation-permissions grid min-w-0 gap-2">
            <h4 className="text-[13px] font-semibold text-foreground">
              {t("generationPinnedPermissions")}
            </h4>
            <div className="overflow-hidden rounded-md border border-border">
              <Table>
                <TableBody>
                  {pinnedRows.map(([label, value]) => (
                    <TableRow key={label} className="hover:bg-transparent">
                      <TableCell className="h-8 w-[42%] text-xs text-faint">
                        {label}
                      </TableCell>
                      <TableCell className="h-8 text-xs [overflow-wrap:anywhere]">
                        {value}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
            <JsonView value={pinned.data.permissions.attributes ?? {}} />
            {pinned.data.remote && (
              <Disclosure
                summary={
                  locale === "ja-JP"
                    ? "別 Node のプロバイダー承認"
                    : "Provider approvals at other nodes"
                }
              >
                <RecordView value={pinned.data.remote} />
              </Disclosure>
            )}
          </section>
        )
      )}
      <Disclosure summary={t("generationDefinition")}>
        <RecordView value={request.definition} />
      </Disclosure>
      {request.status !== "DELETED" && (
        <form
          className="grid min-w-0 gap-3 border-t border-border pt-4"
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
          <fieldset disabled={busy} className="grid min-w-0 gap-3">
            <Field label={t("generationDecisionReason")}>
              <Textarea name="reason" required maxLength={4096} rows={2} />
            </Field>
            <div className="flex flex-wrap items-center gap-2">
              {request.status === "PENDING_APPROVAL" && (
                <>
                  <Button value="approve" disabled={!pinned.isSuccess}>
                    {t("generationApprove")}
                  </Button>
                  <Button variant="outline" value="deny">
                    {t("generationDeny")}
                  </Button>
                </>
              )}
              {active(request.status) ? (
                <Button
                  variant="destructive"
                  className="sm:ml-auto"
                  value="stop"
                >
                  {t("generationStop")}
                </Button>
              ) : (
                <Button variant="outline" className="sm:ml-auto" value="delete">
                  {t("generationDelete")}
                </Button>
              )}
            </div>
          </fieldset>
        </form>
      )}
      <section className="grid min-w-0 gap-3 border-t border-border pt-4">
        <h4 className="text-[13px] font-semibold text-foreground">
          {t("generationHistory")}
        </h4>
        {history.isError ? (
          <Alert>{history.error.message}</Alert>
        ) : (
          <ol className="generation-history grid min-w-0">
            {history.data?.map((entry) => (
              <li
                key={entry.sequence}
                className="relative grid min-w-0 gap-1 border-l border-border pb-4 pl-4 last:pb-0"
              >
                <span
                  aria-hidden
                  className="absolute -left-[4.5px] top-1.5 size-2 rounded-full bg-border-strong ring-2 ring-popover"
                />
                <div className="flex min-w-0 flex-wrap items-center gap-2">
                  <Badge value={entry.status} />
                  <span className="font-mono text-xs text-foreground">
                    {entry.actor}
                  </span>
                  <time
                    className="m-0 inline font-mono text-[11px] text-faint"
                    dateTime={entry.created_at}
                  >
                    {new Date(entry.created_at).toLocaleString(locale)}
                  </time>
                </div>
                <p className="m-0 text-xs leading-relaxed text-muted-foreground [overflow-wrap:anywhere]">
                  {entry.reason}
                </p>
              </li>
            ))}
          </ol>
        )}
      </section>
    </>
  );
}

export function GenerationAssignForm({
  data,
  tenant,
  task,
  submit,
}: {
  data: State;
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
    <>
      <form
        className="grid min-w-0 gap-3"
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
        <div className="grid gap-1">
          <h3 className="text-[13px] font-semibold text-foreground [overflow-wrap:anywhere]">
            {task.title}
          </h3>
          <p className="text-xs leading-relaxed text-muted-foreground">
            {t("generationAssignHelp")}
          </p>
        </div>
        {policies.isError && (
          <Alert>{policies.error.message}</Alert>
        )}
        <Field label={t("generationPolicy")}>
          <NativeSelect name="policy" required defaultValue="">
            <option value="">{t("choose")}</option>
            {choices.map((policy) => (
              <option key={policy.id} value={policy.id}>
                {policyPresentation.label(policy)}
              </option>
            ))}
          </NativeSelect>
        </Field>
        {!policies.isPending && !choices.length && (
          <Hint>{t("generationNoPolicies")}</Hint>
        )}
        <Field label={t("generationRequestReason")}>
          <Textarea name="reason" required maxLength={4096} rows={3} />
        </Field>
        <div className="flex justify-end">
          <Button disabled={!choices.length}>{t("generationAssign")}</Button>
        </div>
      </form>
      <RemoteGenerationAssignForm
        task={task.id}
        workspace={task.workspace_id}
        entries={data.registry}
        submit={submit}
      />
    </>
  );
}
