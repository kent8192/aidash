import { RecordView, useRecordLabels } from "./record-view";
import { useRef, useState, type FormEvent } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
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
} from "./generated/aidash";
import type {
  SemanticEntry,
  SemanticIndexSpec,
  SemanticSearchResult,
  SemanticSource,
} from "./generated/models";
import type { State } from "./types";
import { ApiError } from "./transport";
import { Badge, Empty, Field, Modal, Panel, useI18n } from "./ui";

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
  "agent memory slots are updated through memory_write": "semanticManagedSlot",
  "deleted semantic keys cannot be reused": "semanticDeletedKey",
  "semantic source revision changed": "semanticRevisionChanged",
  "semantic source revision changed or deleted": "semanticRevisionChanged",
  "semantic index revision changed": "semanticRevisionChanged",
};

export function SemanticPage({ data }: { data: State }) {
  const { t } = useI18n();
  const [chosen, setChosen] = useState("");
  const workspace =
    data.workspaces.find((w) => w.id === chosen)?.id ??
    data.workspaces[0]?.id ??
    "";
  return (
    <div className="semantic-page">
      <p className="muted">{t("semanticHelp")}</p>
      <Field label={t("workspace")}>
        <select value={workspace} onChange={(e) => setChosen(e.target.value)}>
          {data.workspaces.map((w) => (
            <option key={w.id} value={w.id}>
              {w.title}
            </option>
          ))}
        </select>
      </Field>
      {workspace ? (
        <SemanticWorkspace
          key={workspace}
          workspace={workspace}
          data={data}
          operator={data.access.kind === "operator"}
        />
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
  return (
    <>
      {(error || failures.length > 0) && (
        <div className="error" role="alert">
          {error
            ? describe(error)
            : failures
                .map((reason) =>
                  describe(
                    reason instanceof Error ? reason.message : String(reason),
                  ),
                )
                .join("; ")}
        </div>
      )}
      <Panel
        title={t("semanticIndex")}
        action={
          operator && (
            <button onClick={openConfiguring}>{t("semanticConfigure")}</button>
          )
        }
      >
        {current && spec ? (
          <div className="semantic-summary">
            <Badge value={spec.enabled ? "ENABLED" : "DISABLED"} />
            <span>
              {spec.embedding.model} · {spec.embedding.model_version}
            </span>
            <span>
              {t("revision")} {current.revision}
            </span>
            <span>
              {t(spec.auto_context ? "semanticAutoOn" : "semanticAutoOff")}
            </span>
          </div>
        ) : (
          <p>{t("semanticUnconfigured")}</p>
        )}
      </Panel>
      {current && spec && (
        <>
          <Panel title={t("semanticSearch")}>
            <form
              className="semantic-search"
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
                <input name="query" required />
              </Field>
              <Field label={t("semanticAgent")}>
                <AgentScope data={data} />
              </Field>
              <Field label={t("semanticFilters")}>
                <input name="metadata" defaultValue="{}" />
              </Field>
              <button className="primary" disabled={busy || !spec.enabled}>
                {t("semanticSearch")}
              </button>
            </form>
            {result && !entries.isError && (
              <div aria-live="polite">
                <p className="muted">
                  {result.model} · {result.model_version} ·{" "}
                  {result.estimated_tokens} {t("semanticTokens")}
                  {result.truncated ? ` · ${t("semanticTruncated")}` : ""}
                </p>
                {result.matches.length === 0 && <p>{t("semanticNoMatches")}</p>}
                {result.matches.map((match) => (
                  <article className="semantic-result" key={match.entry_id}>
                    <p>{match.text}</p>
                    <small>
                      {t("revision")} {match.revision} ·{" "}
                      {match.score.toFixed(3)}
                    </small>
                    <RecordView
                      labels={labels}
                      value={{
                        source: match.source,
                        metadata: match.metadata,
                        agent: scopeLabel(match.agent),
                      }}
                    />
                  </article>
                ))}
              </div>
            )}
          </Panel>
          <Panel
            title={t("semanticSources")}
            action={
              <button onClick={() => openEditing("new")}>
                {t("semanticAdd")}
              </button>
            }
          >
            {!entries.isError &&
              entries.data?.map((entry) => (
                <article className="semantic-entry" key={entry.id}>
                  <div>
                    <strong>{entryName(entry)}</strong>
                    <Badge value={entry.state} />
                    <small>
                      {t("revision")} {entry.revision} · {entry.attempts}{" "}
                      {t("semanticAttempts")}
                    </small>
                  </div>
                  {entry.last_error && (
                    <p className="error">{describe(entry.last_error)}</p>
                  )}
                  <p className="muted">{scopeLabel(entry.agent)}</p>
                  <div className="actions">
                    <button disabled={busy} onClick={() => openEditing(entry)}>
                      {t("edit")}
                    </button>
                    <button
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
                    </button>
                    <button disabled={busy} onClick={() => openDeleting(entry)}>
                      {t("delete")}
                    </button>
                  </div>
                </article>
              ))}
            {!entries.isError && entries.data?.length === 0 && <Empty />}
          </Panel>
          {operator && cleanup.data && !cleanup.isError && (
            <Panel title={t("semanticCleanup")}>
              <p>{t("semanticCleanupHelp")}</p>
              <div className="semantic-summary">
                <span>
                  {t("semanticCleanupPending")}:{" "}
                  {cleanup.data.points.pending +
                    cleanup.data.collections.pending}
                </span>
                <span>
                  {t("semanticCleanupFailed")}:{" "}
                  {cleanup.data.points.failed + cleanup.data.collections.failed}
                </span>
              </div>
            </Panel>
          )}
          <Panel title={t("semanticHistory")}>
            {!history.isError &&
              history.data?.slice(0, 30).map((event) => (
                <div className="semantic-history" key={event.sequence}>
                  <Badge
                    value={
                      event.state === "DELETED"
                        ? "semanticDeleted"
                        : event.state
                    }
                  />
                  <span>{describe(event.detail)}</span>
                  <small>{new Date(event.created_at).toLocaleString()}</small>
                </div>
              ))}
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
          {error && (
            <div className="error" role="alert">
              {describe(error)}
            </div>
          )}
        </Modal>
      )}
      {editing && (
        <Modal title={t("semanticAdd")} close={closeDialogs}>
          <EntryForm
            data={data}
            sources={{ artifacts, messages }}
            entry={editing === "new" ? null : editing}
            busy={busy}
            submit={(draft) => mutate(() => semanticPut(workspace, draft))}
          />
          {error && (
            <div className="error" role="alert">
              {describe(error)}
            </div>
          )}
        </Modal>
      )}
      {deleting && (
        <Modal title={t("semanticDelete")} close={closeDialogs}>
          <p>{t("semanticDeleteHelp")}</p>
          <p>
            <strong>{entryName(deleting)}</strong>
          </p>
          <button
            className="primary"
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
          </button>
          {error && (
            <div className="error" role="alert">
              {describe(error)}
            </div>
          )}
        </Modal>
      )}
    </>
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
      className="form-grid"
      onSubmit={(e) => {
        e.preventDefault();
        const values = new FormData(e.currentTarget);
        void submit({
          embedding: {
            provider: "openai",
            endpoint: String(values.get("embedding")),
            credential_env: String(values.get("embeddingSecret")) || null,
            model: String(values.get("model")),
            model_version: String(values.get("version")),
            dimensions: Number(values.get("dimensions")),
          },
          vector: {
            provider: "qdrant",
            endpoint: String(values.get("vector")),
            credential_env: String(values.get("vectorSecret")) || null,
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
      <p className="muted">{t("semanticConfigHelp")}</p>
      <Field label={t("semanticEmbeddingEndpoint")}>
        <input
          name="embedding"
          type="url"
          required
          defaultValue={previous?.embedding.endpoint ?? ""}
          placeholder="https://api.openai.com/v1"
        />
      </Field>
      <Field label={t("semanticEmbeddingSecret")}>
        <input
          name="embeddingSecret"
          defaultValue={previous?.embedding.credential_env ?? ""}
          placeholder="AIDASH_SECRET_EMBEDDING"
        />
      </Field>
      <Field label={t("semanticModel")}>
        <input
          name="model"
          required
          defaultValue={previous?.embedding.model ?? ""}
        />
      </Field>
      <Field label={t("semanticModelVersion")}>
        <input
          name="version"
          required
          defaultValue={previous?.embedding.model_version ?? ""}
        />
      </Field>
      <Field label={t("semanticDimensions")}>
        <input
          name="dimensions"
          type="number"
          min={1}
          max={8192}
          required
          defaultValue={previous?.embedding.dimensions ?? 1536}
        />
      </Field>
      <Field label={t("semanticVectorEndpoint")}>
        <input
          name="vector"
          type="url"
          required
          defaultValue={previous?.vector.endpoint ?? "http://127.0.0.1:63370"}
        />
      </Field>
      <Field label={t("semanticVectorSecret")}>
        <input
          name="vectorSecret"
          defaultValue={
            previous?.vector.credential_env ?? "AIDASH_SECRET_TEST_QDRANT"
          }
        />
      </Field>
      <label>
        <input
          type="checkbox"
          name="enabled"
          defaultChecked={previous?.enabled ?? true}
        />{" "}
        {t("semanticEnabled")}
      </label>
      <label>
        <input
          type="checkbox"
          name="auto"
          defaultChecked={previous?.auto_context ?? true}
        />{" "}
        {t("semanticAutoOn")}
      </label>
      <button className="primary" disabled={busy}>
        {t("save")}
      </button>
    </form>
  );
}
function EntryForm({
  data,
  sources,
  entry,
  busy,
  submit,
}: {
  data: State;
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
  return (
    <form
      className="form-grid"
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
          <input
            name="key"
            required
            readOnly={!!entry}
            defaultValue={entry?.key ?? ""}
          />
        </Field>
      )}
      <Field label={t("semanticSourceKind")}>
        <select
          value={kind}
          disabled={!!entry}
          onChange={(e) => setKind(e.target.value as SemanticSource["kind"])}
        >
          <option value="memory">{t("semanticMemoryText")}</option>
          <option value="artifact">{t("artifact")}</option>
          <option value="message">{t("message")}</option>
        </select>
      </Field>
      {kind === "memory" ? (
        <Field label={t("semanticText")}>
          <textarea
            name="text"
            required
            rows={5}
            defaultValue={source?.kind === "memory" ? source.text : ""}
          />
        </Field>
      ) : (
        <Field label={t("semanticSource")}>
          <select
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
              : sources.messages.map((message) => ({
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
              !(
                kind === "artifact" ? sources.artifacts : sources.messages
              ).some((option) => option.id === source.id) && (
                <option value={source.id}>{t("unavailableEntity")}</option>
              )}
          </select>
        </Field>
      )}
      <Field label={t("semanticAgent")}>
        <AgentScope data={data} initial={entry?.agent ?? ""} />
      </Field>
      <Field label={t("semanticFilters")}>
        <textarea
          name="metadata"
          rows={3}
          defaultValue={JSON.stringify(entry?.metadata ?? {}, null, 2)}
        />
      </Field>
      {metadataError && (
        <p role="alert" className="error">
          {metadataError}
        </p>
      )}
      <button className="primary" disabled={busy}>
        {t("save")}
      </button>
    </form>
  );
}

function AgentScope({
  data,
  initial = "",
  id,
}: {
  data: State;
  initial?: string;
  id?: string;
}) {
  const { t, entityLabel } = useI18n();
  const agents = data.registry.filter((entry) => entry.kind === "agent");
  const reference = (entry: (typeof agents)[number]) =>
    `${data.node.id}/agents/${entry.id}@${entry.version}`;
  return (
    <select id={id} name="agent" defaultValue={initial}>
      <option value="">{t("semanticWorkspaceScope")}</option>
      {agents.map((entry) => (
        <option key={reference(entry)} value={reference(entry)}>
          {entityLabel(entry)}
        </option>
      ))}
      {initial && !agents.some((entry) => reference(entry) === initial) && (
        <option value={initial}>{t("unavailableEntity")}</option>
      )}
    </select>
  );
}
