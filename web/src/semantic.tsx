import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import { Badge as StatusBadge } from "./components/ui/badge";
import { MemoryWorkspace } from "./memory";
import { useRecordLabels } from "./record-view";
import { disambiguateLabels } from "./display-labels";
import { useRef, useState, type FormEvent } from "react";
import {
  useInfiniteQuery,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import {
  semanticConfigure,
  semanticIndex,
  semanticEntries,
  semanticPut,
  workspaceGet,
  semanticDelete,
  semanticReindex,
  semanticSearch,
  semanticHistory,
  semanticCleanup,
  channelMessageHistory,
  discover,
} from "./generated/aidash";
import type {
  SemanticEntry,
  SemanticIndexSpec,
  SemanticSearchResult,
  SemanticSource,
} from "./generated/models";
import type { State } from "./types";
import { ApiError } from "./transport";
import {
  Badge,
  Empty,
  Field,
  Modal,
  Panel,
  useAgentLabel,
  useI18n,
} from "./ui";

import { Alert, Check, Hint } from "./components/patterns";

const semanticMessages: Record<string, string> = {
  "semantic backend unavailable or invalid; inspect index status and retry":
    "semanticBackendFailure",
  "semantic index is incomplete; inspect entries and retry after indexing":
    "semanticIncomplete",
  "semantic index is disabled": "semanticDisabled",
  "embedding or vector backend unavailable, invalid, or source too large":
    "semanticIndexFailure",
  "source authority revoked or source removed": "semanticSourceRevoked",
  "indexing credential or policy revoked": "semanticCredentialRevoked",
  "new immutable index generation": "semanticNewGeneration",
  "source accepted": "semanticAccepted",
  "source tombstoned": "semanticTombstoned",
  "reindex requested": "semanticReindexRequested",
  "indexing authority or source unavailable": "semanticSourceRevoked",
  "linked source changed or authority restored": "semanticSourceChanged",
  "vector write acknowledged": "semanticWriteAcknowledged",
  "indexing failed; durable retry scheduled": "semanticRetryScheduled",
  "indexing authority unavailable": "semanticCredentialRevoked",
  "deleted semantic keys cannot be reused": "semanticDeletedKey",
  "semantic source revision changed": "semanticRevisionChanged",
  "semantic source revision changed or deleted": "semanticRevisionChanged",
  "semantic index revision changed": "semanticRevisionChanged",
};

export function SemanticPage({
  data,
  workspaceId = "",
}: {
  data: State;
  workspaceId?: string;
}) {
  const { t, locale } = useI18n();
  const [chosen, setChosen] = useState<string | null>(null);
  const requested = chosen ?? workspaceId;
  const workspace = requested
    ? (data.workspaces.find((w) => w.id === requested)?.id ?? "")
    : (data.workspaces[0]?.id ?? "");
  return (
    <div className="semantic-page grid min-w-0 gap-6">
      <div className="flex flex-wrap items-end justify-between gap-x-6 gap-y-3">
        <Hint>{t("semanticHelp")}</Hint>
        <div className="w-full max-w-xs">
          <Field label={t("workspace")}>
            <NativeSelect
              value={workspace}
              onChange={(e) => setChosen(e.target.value)}
            >
              {!workspace && (
                <option value="">
                  {locale === "ja-JP"
                    ? "依頼を選択してください"
                    : "Choose a request"}
                </option>
              )}
              {data.workspaces.map((w) => (
                <option key={w.id} value={w.id}>
                  {w.title}
                </option>
              ))}
            </NativeSelect>
          </Field>
        </div>
      </div>
      {workspace ? (
        <>
          <MemoryWorkspace
            key={`memory:${workspace}`}
            workspace={workspace}
            data={data}
          />
          <SemanticWorkspace
            key={workspace}
            workspace={workspace}
            data={data}
            operator={data.access.kind === "operator"}
          />
        </>
      ) : (
        <Empty />
      )}
    </div>
  );
}
function SemanticWorkspace({
  data,
  workspace,
  operator,
}: {
  data: State;
  workspace: string;
  operator: boolean;
}) {
  const { t } = useI18n();
  const labels = useRecordLabels();
  const discovery = useQuery({
    queryKey: ["discovery"],
    queryFn: () => discover({}),
    retry: false,
  });
  const agentLabel = useAgentLabel(
    data,
    discovery.isError ? undefined : discovery.data,
  );
  for (const agent of discovery.isError ? [] : (discovery.data?.agents ?? []))
    labels.set(
      `${agent.node_id}/agents/${agent.entity.id}@${agent.entity.version}`,
      agentLabel(agent.node_id, agent.entity),
    );
  const snapshot = useQuery({
    queryKey: ["workspace", workspace],
    queryFn: () => workspaceGet(workspace),
    retry: false,
    refetchInterval: 5000,
  });
  const available = snapshot.isError ? undefined : snapshot.data;
  const artifacts = available?.artifacts ?? [];
  const messages = available?.messages ?? [];
  for (const artifact of artifacts)
    labels.set(artifact.id, artifact.name || t("artifact"));
  for (const message of messages)
    labels.set(message.id, message.content.slice(0, 100) || t("message"));
  const scopeLabel = (agent: string | null | undefined) =>
    agent
      ? (labels.get(agent) ?? t("unavailableEntity"))
      : t("semanticWorkspaceScope");
  const entryName = (entry: SemanticEntry) =>
    entry.key.startsWith("agent-memory:")
      ? `${scopeLabel(entry.agent)} · ${t("semanticMemoryText")}`
      : entry.key;
  const describe = (message: string) => t(semanticMessages[message] ?? message);
  const client = useQueryClient();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [configuring, setConfiguring] = useState(false);
  const [editing, setEditing] = useState<SemanticEntry | "new" | null>(null);
  const [deleting, setDeleting] = useState<SemanticEntry | null>(null);
  const [result, setResult] = useState<SemanticSearchResult | null>(null);
  const dialogGeneration = useRef(0);
  const closeDialogs = () => {
    dialogGeneration.current += 1;
    setConfiguring(false);
    setEditing(null);
    setDeleting(null);
  };
  const openConfiguring = () => {
    dialogGeneration.current += 1;
    setConfiguring(true);
  };
  const openEditing = (entry: SemanticEntry | "new") => {
    dialogGeneration.current += 1;
    setEditing(entry);
  };
  const openDeleting = (entry: SemanticEntry) => {
    dialogGeneration.current += 1;
    setDeleting(entry);
  };
  const index = useQuery({
    queryKey: ["semantic", workspace, "index"],
    queryFn: async () => {
      try {
        return await semanticIndex(workspace);
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return null;
        throw e;
      }
    },
    refetchInterval: 3000,
  });
  const entries = useQuery({
    queryKey: ["semantic", workspace, "entries"],
    queryFn: () => semanticEntries(workspace),
    enabled: !!index.data && !index.isError,
    refetchInterval: 2000,
  });
  const history = useQuery({
    queryKey: ["semantic", workspace, "history"],
    queryFn: () => semanticHistory(workspace),
    enabled: !!index.data && !index.isError,
    refetchInterval: 3000,
  });
  const cleanup = useQuery({
    queryKey: ["semantic", workspace, "cleanup"],
    queryFn: () => semanticCleanup(workspace),
    enabled: operator && !!index.data && !index.isError,
    refetchInterval: 3000,
  });
  const current = !index.isError ? index.data : null;
  const spec = current?.spec as SemanticIndexSpec | undefined;
  const mutate = async (action: () => Promise<unknown>) => {
    if (busy) return;
    const submittedDialogGeneration = dialogGeneration.current;
    setBusy(true);
    setError("");
    setResult(null);
    try {
      await action();
      await client.invalidateQueries({ queryKey: ["semantic", workspace] });
      if (dialogGeneration.current === submittedDialogGeneration) {
        closeDialogs();
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const failures = [
    index.error,
    entries.error,
    history.error,
    cleanup.error,
  ].filter(Boolean);
  const knownScopes =
    (entries.isError ? [] : entries.data)
      ?.map((entry) => entry.agent)
      .filter((agent): agent is string => !!agent) ?? [];
  const dialogAlert = error && (
    <Alert>{describe(error)}</Alert>
  );
  return (
    <>
      {(error || failures.length > 0) && (
        <Alert>
          {error
            ? describe(error)
            : failures
                .map((reason) =>
                  describe(
                    reason instanceof Error ? reason.message : String(reason),
                  ),
                )
                .join("; ")}
        </Alert>
      )}
      <Panel
        title={t("semanticIndex")}
        action={
          operator && (
            <Button variant="outline" size="sm" onClick={openConfiguring}>
              {t("semanticConfigure")}
            </Button>
          )
        }
      >
        {current && spec ? (
          <div className="semantic-summary flex flex-wrap items-center gap-x-4 gap-y-1.5 text-xs text-muted-foreground">
            <Badge value={spec.enabled ? "ENABLED" : "DISABLED"} />
            <span className="font-mono text-foreground">
              {spec.embedding.model} · {spec.embedding.model_version}
            </span>
            <span className="font-mono tabular">
              {t("revision")} {current.revision}
            </span>
            <span>
              {t(spec.auto_context ? "semanticAutoOn" : "semanticAutoOff")}
            </span>
          </div>
        ) : (
          <Hint>{t("semanticUnconfigured")}</Hint>
        )}
      </Panel>
      {current && spec && (
        <>
          <Panel title={t("semanticSearch")}>
            <form
              className="grid items-end gap-3 md:grid-cols-[minmax(0,2fr)_minmax(0,1fr)_minmax(0,1fr)_auto]"
              onSubmit={(e: FormEvent<HTMLFormElement>) => {
                e.preventDefault();
                const values = new FormData(e.currentTarget);
                void mutate(async () => {
                  const found = await semanticSearch(workspace, {
                    query: String(values.get("query")),
                    agent: String(values.get("agent") || "") || null,
                    metadata: JSON.parse(
                      String(values.get("metadata") || "{}"),
                    ),
                    limit: spec.max_results,
                    max_tokens: spec.max_result_tokens,
                  });
                  setResult(found);
                });
              }}
            >
              <Field label={t("semanticQuery")}>
                <Input name="query" required />
              </Field>
              <Field label={t("semanticAgent")}>
                <AgentScope data={data} knownScopes={knownScopes} />
              </Field>
              <Field label={t("semanticFilters")}>
                <Input
                  name="metadata"
                  className="font-mono"
                  defaultValue="{}"
                />
              </Field>
              <Button disabled={busy || !spec.enabled}>
                {t("semanticSearch")}
              </Button>
            </form>
            {result && !entries.isError && (
              <div aria-live="polite" className="grid gap-2">
                <p className="font-mono text-[11px] text-faint tabular">
                  {result.model} · {result.model_version} ·{" "}
                  {result.estimated_tokens} {t("semanticTokens")}
                  {result.truncated ? ` · ${t("semanticTruncated")}` : ""}
                </p>
                {result.matches.length === 0 && (
                  <Hint>{t("semanticNoMatches")}</Hint>
                )}
                {result.matches.length > 0 && (
                  <ol className="divide-y divide-border border-y border-border">
                    {result.matches.map((match) => (
                      <li key={match.entry_id}>
                        <article className="semantic-result grid gap-2 py-3 sm:grid-cols-[minmax(0,1fr)_auto]">
                          <p className="min-w-0 whitespace-pre-wrap break-words text-[13px] text-foreground">
                            {match.text}
                          </p>
                          <div className="flex items-baseline gap-3 font-mono text-xs tabular sm:justify-end">
                            <span className="text-brand">
                              {match.score.toFixed(3)}
                            </span>
                            <span className="text-faint">
                              {t("revision")} {match.revision}
                            </span>
                          </div>
                          <MatchProvenance
                            source={match.source}
                            metadata={match.metadata}
                            sourceLabel={(id) => labels.get(id) ?? id}
                            scope={scopeLabel(match.agent)}
                          />
                        </article>
                      </li>
                    ))}
                  </ol>
                )}
              </div>
            )}
          </Panel>
          <Panel
            title={t("semanticSources")}
            action={
              <Button
                variant="outline"
                size="sm"
                onClick={() => openEditing("new")}
              >
                {t("semanticAdd")}
              </Button>
            }
          >
            {!entries.isError && !!entries.data?.length && (
              <ul className="divide-y divide-border border-y border-border">
                {entries.data.map((entry) => (
                  <li key={entry.id}>
                    <article className="semantic-entry flex flex-wrap items-center gap-x-4 gap-y-2 py-2.5">
                      <div className="grid min-w-0 flex-1 gap-0.5">
                        <div className="flex min-w-0 items-center gap-2">
                          <strong className="truncate font-mono text-xs font-medium text-foreground">
                            {entryName(entry)}
                          </strong>
                          <Badge value={entry.state} />
                        </div>
                        <p className="break-words text-xs text-muted-foreground">
                          {scopeLabel(entry.agent)}
                          <span className="font-mono text-faint tabular">
                            {" "}
                            · {t("revision")} {entry.revision} ·{" "}
                            {entry.attempts} {t("semanticAttempts")}
                          </span>
                        </p>
                        {entry.last_error && (
                          <p className="text-xs text-destructive">
                            {describe(entry.last_error)}
                          </p>
                        )}
                      </div>
                      <div className="flex flex-wrap gap-1">
                        <Button
                          variant="ghost"
                          size="sm"
                          disabled={busy}
                          onClick={() => openEditing(entry)}
                        >
                          {t("edit")}
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          disabled={busy}
                          onClick={() =>
                            void mutate(() =>
                              semanticReindex(workspace, entry.id, {
                                expected_revision: entry.revision,
                              }),
                            )
                          }
                        >
                          {t("semanticReindex")}
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          className="hover:text-destructive"
                          disabled={busy}
                          onClick={() => openDeleting(entry)}
                        >
                          {t("delete")}
                        </Button>
                      </div>
                    </article>
                  </li>
                ))}
              </ul>
            )}
            {!entries.isError && entries.data?.length === 0 && <Empty />}
          </Panel>
          {operator && cleanup.data && !cleanup.isError && (
            <Panel title={t("semanticCleanup")}>
              <Hint>{t("semanticCleanupHelp")}</Hint>
              <div className="semantic-summary flex flex-wrap gap-x-6 gap-y-1 text-xs text-muted-foreground">
                <span>
                  {t("semanticCleanupPending")}:{" "}
                  <span className="font-mono text-foreground tabular">
                    {cleanup.data.points.pending +
                      cleanup.data.collections.pending}
                  </span>
                </span>
                <span>
                  {t("semanticCleanupFailed")}:{" "}
                  <span className="font-mono text-foreground tabular">
                    {cleanup.data.points.failed +
                      cleanup.data.collections.failed}
                  </span>
                </span>
              </div>
            </Panel>
          )}
          <Panel title={t("semanticHistory")}>
            {!history.isError && !!history.data?.length && (
              <ol className="divide-y divide-border border-y border-border">
                {history.data.slice(0, 30).map((event) => (
                  <li
                    className="flex flex-wrap items-center gap-x-3 gap-y-1 py-2"
                    key={event.sequence}
                  >
                    <Badge
                      value={
                        event.state === "DELETED"
                          ? "semanticDeleted"
                          : event.state
                      }
                    />
                    <span className="min-w-0 flex-1 text-xs text-foreground">
                      {describe(event.detail)}
                    </span>
                    <time
                      dateTime={event.created_at}
                      className="font-mono text-[11px] text-faint tabular"
                    >
                      {new Date(event.created_at).toLocaleString()}
                    </time>
                  </li>
                ))}
              </ol>
            )}
          </Panel>
        </>
      )}
      {configuring && (
        <Modal title={t("semanticConfigure")} close={closeDialogs}>
          <IndexForm
            previous={spec}
            busy={busy}
            submit={(draft) =>
              mutate(() =>
                semanticConfigure(workspace, {
                  expected_revision: current?.revision ?? 0,
                  spec: draft,
                }),
              )
            }
          />
          {dialogAlert}
        </Modal>
      )}
      {editing && (
        <Modal title={t("semanticAdd")} close={closeDialogs}>
          <EntryForm
            data={data}
            workspace={workspace}
            knownScopes={knownScopes}
            sources={{ artifacts, messages }}
            entry={editing === "new" ? null : editing}
            busy={busy}
            submit={(draft) => mutate(() => semanticPut(workspace, draft))}
          />
          {dialogAlert}
        </Modal>
      )}
      {deleting && (
        <Modal title={t("semanticDelete")} close={closeDialogs}>
          <Hint>{t("semanticDeleteHelp")}</Hint>
          <p className="rounded-md border border-border bg-surface px-3 py-2 font-mono text-xs text-foreground">
            {entryName(deleting)}
          </p>
          <div className="flex justify-end gap-2">
            <Button variant="ghost" disabled={busy} onClick={closeDialogs}>
              {t("cancel")}
            </Button>
            <Button
              variant="destructive"
              disabled={busy}
              onClick={() =>
                void mutate(() =>
                  semanticDelete(workspace, deleting.id, {
                    expected_revision: deleting.revision,
                  }),
                )
              }
            >
              {t("delete")}
            </Button>
          </div>
          {dialogAlert}
        </Modal>
      )}
    </>
  );
}
/** Readable provenance for one match: source kind and reference, scope, metadata filters. */
function MatchProvenance({
  source,
  metadata,
  sourceLabel,
  scope,
}: {
  source: unknown;
  metadata: unknown;
  sourceLabel: (id: string) => string;
  scope: string;
}) {
  const value = (source ?? {}) as { kind?: unknown; id?: unknown };
  const fields =
    metadata && typeof metadata === "object" && !Array.isArray(metadata)
      ? Object.entries(metadata)
      : [];
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground sm:col-span-2">
      {typeof value.kind === "string" && (
        <StatusBadge tone="neutral" className="font-mono">
          {value.kind}
        </StatusBadge>
      )}
      {typeof value.id === "string" && (
        <span className="min-w-0 break-all font-mono text-foreground">
          {sourceLabel(value.id)}
        </span>
      )}
      <span>{scope}</span>
      {fields.map(([key, field]) => (
        <span key={key} className="break-all font-mono text-faint">
          {key}={typeof field === "string" ? field : JSON.stringify(field)}
        </span>
      ))}
    </div>
  );
}
function IndexForm({
  previous,
  busy,
  submit,
}: {
  previous?: SemanticIndexSpec;
  busy: boolean;
  submit: (spec: SemanticIndexSpec) => Promise<void>;
}) {
  const { t } = useI18n();
  return (
    <form
      className="grid gap-3 sm:grid-cols-2"
      onSubmit={(e) => {
        e.preventDefault();
        const values = new FormData(e.currentTarget);
        void submit({
          embedding: {
            provider: String(values.get("embeddingProvider")),
            endpoint: String(values.get("embedding")),
            credential_env: String(values.get("embeddingSecret")) || null,
            model: String(values.get("model")),
            model_version: String(values.get("version")),
            dimensions: Number(values.get("dimensions")),
          },
          vector: {
            provider: "postgres",
            endpoint: "local",
            credential_env: null,
          },
          enabled: values.has("enabled"),
          auto_context: values.has("auto"),
          max_sources: previous?.max_sources ?? 256,
          max_results: previous?.max_results ?? 10,
          max_result_tokens: previous?.max_result_tokens ?? 4096,
          max_input_bytes: previous?.max_input_bytes ?? 8192,
        });
      }}
    >
      <Hint className="sm:col-span-2">{t("semanticConfigHelp")}</Hint>
      <Field label={t("provider")}>
        <NativeSelect
          name="embeddingProvider"
          defaultValue={previous?.embedding.provider ?? "openrouter"}
        >
          <option value="openrouter">OpenRouter</option>
          <option value="openai">{t("openaiCompatible")}</option>
        </NativeSelect>
      </Field>
      <Field label={t("semanticEmbeddingEndpoint")}>
        <Input
          name="embedding"
          type="url"
          required
          defaultValue={
            previous?.embedding.endpoint ?? "https://openrouter.ai/api/v1"
          }
        />
      </Field>
      <Field label={t("semanticEmbeddingSecret")}>
        <Input
          name="embeddingSecret"
          defaultValue={
            previous
              ? (previous.embedding.credential_env ?? "")
              : "AIDASH_SECRET_OPENROUTER"
          }
          placeholder="AIDASH_SECRET_OPENROUTER"
        />
      </Field>
      <Field label={t("semanticModel")}>
        <Input
          name="model"
          required
          defaultValue={
            previous?.embedding.model ?? "google/gemini-embedding-2"
          }
        />
      </Field>
      <Field label={t("semanticModelVersion")}>
        <Input
          name="version"
          required
          defaultValue={previous?.embedding.model_version ?? "1.0.0"}
        />
      </Field>
      <Field label={t("semanticDimensions")}>
        <Input
          name="dimensions"
          type="number"
          min={1}
          max={8192}
          required
          defaultValue={previous?.embedding.dimensions ?? 3072}
        />
      </Field>
      <div className="grid gap-2 sm:col-span-2">
        <Check name="enabled" defaultChecked={previous?.enabled ?? true}>
          {t("semanticEnabled")}
        </Check>
        <Check name="auto" defaultChecked={previous?.auto_context ?? true}>
          {t("semanticAutoOn")}
        </Check>
      </div>
      <div className="flex justify-end sm:col-span-2">
        <Button disabled={busy}>{t("save")}</Button>
      </div>
    </form>
  );
}
function EntryForm({
  data,
  workspace,
  knownScopes,
  sources,
  entry,
  busy,
  submit,
}: {
  data: State;
  workspace: string;
  knownScopes: string[];
  sources: {
    artifacts: State["artifacts"];
    messages: { id: string; content: string; created_at: string }[];
  };
  entry: SemanticEntry | null;
  busy: boolean;
  submit: (draft: Parameters<typeof semanticPut>[1]) => Promise<void>;
}) {
  const { t } = useI18n();
  const source = entry?.source as SemanticSource | undefined;
  const [kind, setKind] = useState(source?.kind ?? "memory");
  const [metadataError, setMetadataError] = useState("");
  const olderMessages = useInfiniteQuery({
    queryKey: ["workspace", workspace, "semantic-message-sources"],
    queryFn: ({ pageParam }) =>
      channelMessageHistory(workspace, {
        before: pageParam ?? undefined,
        limit: 100,
      }),
    initialPageParam: null as string | null,
    getNextPageParam: (page) => page.next_before ?? undefined,
    enabled: kind === "message",
    retry: false,
  });
  const messages = [
    ...new Map(
      [
        ...sources.messages,
        ...(olderMessages.isError
          ? []
          : (olderMessages.data?.pages.flatMap((page) =>
              page.messages.map((item) => item.message),
            ) ?? [])),
      ].map((message) => [message.id, message] as const),
    ).values(),
  ];
  return (
    <form
      className="grid gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        const values = new FormData(e.currentTarget);
        let metadata: Record<string, unknown>;
        try {
          metadata = JSON.parse(String(values.get("metadata")));
          setMetadataError("");
        } catch {
          setMetadataError(t("semanticInvalidMetadata"));
          return;
        }
        void submit({
          key: String(values.get("key")),
          expected_revision: entry?.revision ?? 0,
          source:
            kind === "memory"
              ? { kind, text: String(values.get("text")) }
              : { kind, id: String(values.get("sourceId")) },
          agent: String(values.get("agent") || "") || null,
          metadata,
        });
      }}
    >
      {entry?.key.startsWith("agent-memory:") ? (
        <input type="hidden" name="key" value={entry.key} />
      ) : (
        <Field label={t("semanticKey")}>
          <Input
            name="key"
            required
            readOnly={!!entry}
            defaultValue={entry?.key ?? ""}
          />
        </Field>
      )}
      <Field label={t("semanticSourceKind")}>
        <NativeSelect
          value={kind}
          disabled={!!entry}
          onChange={(e) => setKind(e.target.value as SemanticSource["kind"])}
        >
          <option value="memory">{t("semanticMemoryText")}</option>
          <option value="artifact">{t("artifact")}</option>
          <option value="message">{t("message")}</option>
        </NativeSelect>
      </Field>
      {kind === "memory" ? (
        <Field label={t("semanticText")}>
          <Textarea
            name="text"
            required
            rows={5}
            defaultValue={source?.kind === "memory" ? source.text : ""}
          />
        </Field>
      ) : (
        <Field label={t("semanticSource")}>
          <NativeSelect
            name="sourceId"
            required
            defaultValue={source && source.kind !== "memory" ? source.id : ""}
          >
            <option value="">{t("choose")}</option>
            {(kind === "artifact"
              ? sources.artifacts.map((artifact) => ({
                  id: artifact.id,
                  label: artifact.name,
                }))
              : messages.map((message) => ({
                  id: message.id,
                  label: `${message.content.slice(0, 100)} · ${new Date(message.created_at).toLocaleString()}`,
                }))
            )
              .filter(
                (option) =>
                  !entry ||
                  (source &&
                    source.kind !== "memory" &&
                    option.id === source.id),
              )
              .map((option) => (
                <option key={option.id} value={option.id}>
                  {option.label}
                </option>
              ))}
            {source &&
              source.kind !== "memory" &&
              !(kind === "artifact" ? sources.artifacts : messages).some(
                (option) => option.id === source.id,
              ) && <option value={source.id}>{t("unavailableEntity")}</option>}
          </NativeSelect>
        </Field>
      )}
      {kind === "message" && olderMessages.hasNextPage && (
        <Button
          variant="ghost"
          size="sm"
          className="justify-self-start"
          type="button"
          disabled={olderMessages.isFetchingNextPage}
          onClick={() => void olderMessages.fetchNextPage()}
        >
          {t("semanticOlderMessages")}
        </Button>
      )}
      {kind === "message" && olderMessages.isError && (
        <Alert retry={() => void olderMessages.refetch()}>
          {olderMessages.error.message}
        </Alert>
      )}
      <Field label={t("semanticAgent")}>
        <AgentScope
          data={data}
          knownScopes={knownScopes}
          initial={entry?.agent ?? ""}
        />
      </Field>
      <Field label={t("semanticFilters")}>
        <Textarea
          name="metadata"
          rows={3}
          className="font-mono text-xs"
          defaultValue={JSON.stringify(entry?.metadata ?? {}, null, 2)}
        />
      </Field>
      {metadataError && (
        <Alert>{metadataError}</Alert>
      )}
      <div className="flex justify-end">
        <Button disabled={busy}>{t("save")}</Button>
      </div>
    </form>
  );
}

function AgentScope({
  data,
  knownScopes = [],
  initial = "",
  id,
}: {
  data: State;
  knownScopes?: string[];
  initial?: string;
  id?: string;
}) {
  const { t } = useI18n();
  const discovery = useQuery({
    queryKey: ["discovery"],
    queryFn: () => discover({}),
    retry: false,
  });
  const agentLabel = useAgentLabel(
    data,
    discovery.isError ? undefined : discovery.data,
  );
  const options = new Map<string, string>();
  for (const entry of data.registry.filter((item) => item.kind === "agent"))
    options.set(
      `${data.node.id}/agents/${entry.id}@${entry.version}`,
      agentLabel(data.node.id, entry),
    );
  for (const agent of discovery.isError ? [] : (discovery.data?.agents ?? []))
    options.set(
      `${agent.node_id}/agents/${agent.entity.id}@${agent.entity.version}`,
      agentLabel(agent.node_id, agent.entity),
    );
  for (const scope of [...knownScopes, initial])
    if (scope && !options.has(scope))
      options.set(scope, t("unavailableEntity"));
  const labels = disambiguateLabels(
    [...options],
    ([scope]) => scope,
    ([, name]) => name,
  );
  return (
    <NativeSelect id={id} name="agent" defaultValue={initial}>
      <option value="">{t("semanticWorkspaceScope")}</option>
      {[...options].map(([scope]) => (
        <option key={scope} value={scope}>
          {labels.get(scope)}
        </option>
      ))}
    </NativeSelect>
  );
}
