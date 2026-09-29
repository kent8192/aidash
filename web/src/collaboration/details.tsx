import { RemoteExecutions } from "./remote-executions";
import { ReferenceName } from "../record-view";
import { RecordView } from "../record-view";
import { useRef, useState } from "react";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import {
  workspaceGet,
  runGet,
  runControl,
  runMessage,
  channelAttachmentUpload,
  humanAnswer,
  taskAbandon,
  remoteAction,
  packageInstall,
} from "../generated/aidash";
import type {
  Artifact,
  Discovery,
  Entry,
  HumanRequest,
  Mesh,
  Package,
  Run,
  State,
  Task,
  Workspace,
} from "../types";
import { Badge, Field, JsonView, Modal, useI18n, useAgentLabel } from "../ui";
import {
  AssignForm,
  EntityForm,
  GoalForm,
  PeerForm,
  PublishForm,
  TaskForm,
  WorkspaceForm,
  type Submit,
} from "../forms";
import { GenerationAssignForm } from "../generation";
import { EntityDetails } from "../entity-details";
import { ArtifactList } from "./channel";
import { collaborationCopy } from "./copy";
import { workspaceCopy } from "./workspace-copy";
import {
  RUN_MEDIA_ACCEPT,
  validRunMediaAttachments,
} from "./conversation-model";

export type Selection = {
  kind: string;
  task?: Task;
  entity?: Entry;
  workspace?: Workspace;
  run?: Run;
  request?: HumanRequest;
  artifact?: Artifact;
  package?: Package;
  node?: string;
};
export function OperationsDialog({
  selection,
  data,
  discovery,
  mesh,
  open,
  close,
  submit,
  error,
  busy,
  visitChannel,
}: {
  selection: Selection;
  data: State;
  discovery?: Discovery;
  mesh?: Mesh;
  open: (selection: Selection) => void;
  close: () => void;
  submit: Submit;
  error: string;
  busy: boolean;
  visitChannel: (id: string) => void;
}) {
  const { t, local, locale } = useI18n();
  const agentLabel = useAgentLabel(data, discovery);
  const copy = collaborationCopy[locale];
  const d = selection;
  const taskSnapshot = useQuery({
    queryKey: ["workspace", d.task?.workspace_id],
    queryFn: () => workspaceGet(d.task!.workspace_id),
    enabled: Boolean(d.task),
    refetchInterval: 2000,
    retry: false,
  });
  const task =
    taskSnapshot.isError || !taskSnapshot.data
      ? undefined
      : taskSnapshot.data.tasks.find((value) => value.id === d.task?.id);
  const request = (
    d.node && d.node !== data.node.id
      ? mesh?.nodes.find((node) => node.node_id === d.node)?.human_requests
      : data.human_requests
  )?.find((value) => value.id === d.request?.id);
  const isOperator = data.access.kind === "operator";
  const administrative = [
    "entity",
    "cluster",
    "peer",
    "publish",
    "package",
  ].includes(d.kind);
  return (
    <Modal
      title={
        d.kind === "workspace"
          ? copy.prepare
          : t(
              (
                {
                  goal: "newGoal",
                  task: "newTask",
                  taskDetail: "task",
                  entityDetail: "agent",
                  run: "execution",
                  human: "human",
                  assign: "delegate",
                  generate: "generationAssign",
                  entity: "register",
                  cluster: "register",
                  peer: "addPeer",
                  publish: "publish",
                  package: "package",
                } as Record<string, string>
              )[d.kind] ?? d.kind,
            )
      }
      close={close}
    >
      {error && (
        <div className="error" role="alert">
          {error}
        </div>
      )}
      <fieldset disabled={busy} className="collab-dialog-fields">
        {administrative && !isOperator ? (
          <p>{copy.unavailable}</p>
        ) : (
          <>
            {d.kind === "workspace" && (
              <>
                <p className="muted">{copy.setupNotice}</p>
                <WorkspaceForm submit={submit} />
              </>
            )}
            {d.kind === "goal" && <GoalForm data={data} submit={submit} />}
            {d.kind === "task" && (
              <TaskForm
                data={data}
                workspace={d.workspace?.id}
                submit={submit}
              />
            )}
            {(d.kind === "entity" || d.kind === "cluster") && (
              <EntityForm
                data={data}
                submit={submit}
                initial={d.kind === "cluster" ? "cluster" : "agent"}
              />
            )}
            {d.kind === "peer" && <PeerForm submit={submit} />}
            {d.kind === "publish" && (
              <PublishForm data={data} submit={submit} />
            )}
            {d.kind === "assign" && task && (
              <AssignForm
                data={data}
                task={task}
                discovery={discovery ?? { agents: [], errors: [] }}
                submit={submit}
              />
            )}
            {d.kind === "generate" &&
              task &&
              data.access.kind === "subject" && (
                <GenerationAssignForm
                  tenant={data.access.tenant}
                  task={task}
                  submit={submit}
                />
              )}
            {d.kind === "entityDetail" && d.entity && (
              <EntityDetails entity={d.entity} data={data} open={open} />
            )}
            {d.kind === "run" && d.run && (
              <RunPanel
                id={d.run.id}
                node={d.node ?? data.node.id}
                data={data}
                mesh={mesh}
                discovery={discovery}
                submit={submit}
                visitChannel={visitChannel}
              />
            )}
            {d.kind === "human" &&
              (request && request.response === null ? (
                <form
                  onSubmit={(event) => {
                    event.preventDefault();
                    const raw = String(
                      new FormData(event.currentTarget).get("answer"),
                    );
                    let response: unknown = raw;
                    try {
                      response = JSON.parse(raw);
                    } catch {
                      /* A plain text answer is valid. */
                    }
                    void submit(() =>
                      d.node && d.node !== data.node.id
                        ? remoteAction({
                            node_id: d.node,
                            control: {
                              run_id: request.run_id,
                              request_id: request.id,
                              action: "answer",
                              response,
                            },
                          })
                        : humanAnswer(request.id, response),
                    );
                  }}
                >
                  <Badge value={request.kind} />
                  <p className="human-prompt">{request.prompt}</p>
                  <Field label={t("answer")}>
                    <textarea name="answer" rows={5} required />
                  </Field>
                  <button className="primary">{copy.send}</button>
                </form>
              ) : (
                <p role="status">{copy.unavailable}</p>
              ))}
            {d.kind === "taskDetail" &&
              (task ? (
                <>
                  <h2>{task.title}</h2>
                  <p>{task.description}</p>
                  <Badge value={task.status} />
                  {data.access.kind === "subject" && (
                    <RemoteExecutions task={task.id} />
                  )}
                  <dl>
                    <dt>{t("owner")}</dt>
                    <dd>{task.owner ?? t("noAssignment")}</dd>
                    <dt>{t("revision")}</dt>
                    <dd>{task.revision}</dd>
                  </dl>
                  <button
                    type="button"
                    onClick={() => visitChannel(task.workspace_id)}
                  >
                    {copy.channelLink}
                  </button>
                  {task.status === "OPEN" && (
                    <div className="button-row">
                      <button
                        type="button"
                        onClick={() => open({ kind: "assign", task })}
                      >
                        {t("delegate")}
                      </button>
                      {data.access.kind === "subject" && (
                        <button
                          type="button"
                          onClick={() => open({ kind: "generate", task })}
                        >
                          {t("generationAssign")}
                        </button>
                      )}
                    </div>
                  )}
                  {["FAILED", "BLOCKED", "CANCELLED"].includes(task.status) && (
                    <form
                      onSubmit={(event) => {
                        event.preventDefault();
                        const reason = String(
                          new FormData(event.currentTarget).get("reason"),
                        );
                        void submit(() =>
                          taskAbandon(task.id, {
                            revision: task.revision,
                            reason,
                          }),
                        );
                      }}
                    >
                      <Field label={t("abandonReason")}>
                        <textarea required name="reason" />
                      </Field>
                      <button className="danger">{t("abandonTask")}</button>
                      <p className="muted">{t("abandonHelp")}</p>
                    </form>
                  )}
                  {data.runs
                    .filter(
                      (run) =>
                        run.task_id === task.id &&
                        run.home_node === data.node.id,
                    )
                    .map((run) => (
                      <button
                        type="button"
                        className="collab-task"
                        key={run.id}
                        onClick={() =>
                          open({ kind: "run", run, node: data.node.id })
                        }
                      >
                        <Badge value={run.phase} />
                        {agentLabel(data.node.id, {
                          id: run.agent_id,
                          version: run.agent_version,
                        })}
                      </button>
                    ))}
                  {(mesh?.nodes ?? []).flatMap((node) =>
                    node.runs
                      .filter(
                        (run) =>
                          run.task_id === task.id &&
                          run.home_node === data.node.id,
                      )
                      .map((run) => (
                        <button
                          type="button"
                          className="collab-task"
                          key={`${node.node_id}:${run.id}`}
                          onClick={() =>
                            open({ kind: "run", run, node: node.node_id })
                          }
                        >
                          <Badge value={run.phase} />
                          {agentLabel(node.node_id, {
                            id: run.agent_id,
                            version: run.agent_version,
                          })}{" "}
                          · <ReferenceName id={node.node_id} />
                        </button>
                      )),
                  )}
                  <details>
                    <summary>{t("requirements")}</summary>
                    <JsonView value={task.requirements} />
                  </details>
                  <ArtifactList
                    artifacts={(taskSnapshot.data?.artifacts ?? []).filter(
                      (artifact) => artifact.task_id === task.id,
                    )}
                  />
                </>
              ) : (
                <p role="status">
                  {taskSnapshot.isPending ? copy.processing : copy.unavailable}
                </p>
              ))}
            {d.kind === "package" && d.package && (
              <>
                <h2>{local(d.package.manifest.entity.name)}</h2>
                <p>{local(d.package.manifest.entity.description)}</p>
                <RecordView value={d.package.manifest} />
                <button
                  type="button"
                  className="primary"
                  onClick={() =>
                    void submit(() =>
                      packageInstall(d.package!.id, d.package!.version, {
                        digest: d.package!.digest,
                        config: {},
                      }),
                    )
                  }
                >
                  {t("install")}
                </button>
              </>
            )}
          </>
        )}
      </fieldset>
    </Modal>
  );
}

function RunPanel({
  id,
  node,
  data,
  mesh,
  discovery,
  submit,
  visitChannel,
}: {
  id: string;
  node: string;
  data: State;
  mesh?: Mesh;
  discovery?: Discovery;
  submit: Submit;
  visitChannel: (id: string) => void;
}) {
  const { t, locale } = useI18n();
  const copy = collaborationCopy[locale];
  const fileCopy = workspaceCopy[locale];
  const local = node === data.node.id;
  const agentLabel = useAgentLabel(data, discovery);
  const query = useInfiniteQuery({
    queryKey: ["run", node, id],
    initialPageParam: 0,
    queryFn: ({ pageParam }) => runGet(id, { offset: pageParam }),
    getNextPageParam: (last, pages) =>
      last.invocations.length === 100 ? pages.length * 100 : undefined,
    enabled: local,
    refetchInterval: 2000,
    retry: false,
  });
  const remoteNode = mesh?.nodes.find((value) => value.node_id === node);
  const run = local
    ? query.isError
      ? undefined
      : query.data?.pages[0].run
    : remoteNode?.runs.find((value) => value.id === id);
  const [draft, setDraft] = useState("");
  const [files, setFiles] = useState<
    { key: string; file: File; uploaded?: string }[]
  >([]);
  const [fileError, setFileError] = useState("");
  const [uploading, setUploading] = useState(false);
  const sending = useRef(false);
  const request = useRef<{
    content: string;
    attachments: string[];
    key: string;
  } | null>(null);
  if (!run)
    return (
      <p role="status">
        {local && query.isPending ? copy.processing : copy.unavailable}
      </p>
    );
  const terminal = ["COMPLETED", "FAILED", "CANCELLED"].includes(run.phase);
  const control = (action: string) => {
    if (action === "cancel" && !window.confirm(t("confirmCancel"))) return;
    void submit(() =>
      local
        ? runControl(id, { action })
        : remoteAction({ node_id: node, control: { run_id: id, action } }),
    );
  };
  const invocations = local
    ? (query.data?.pages.flatMap((page) => page.invocations) ?? [])
    : (remoteNode?.invocations.filter((value) => value.run_id === id) ?? []);
  return (
    <>
      <h2>
        {agentLabel(node, { id: run.agent_id, version: run.agent_version })}
      </h2>
      <p>
        <ReferenceName id={node} />
      </p>
      <Badge value={run.phase} />
      <Badge value={run.control} />
      {run.home_node === data.node.id && (
        <button type="button" onClick={() => visitChannel(run.workspace_id)}>
          {copy.channelLink}
        </button>
      )}
      {!terminal && (
        <>
          <div className="button-row">
            <button
              type="button"
              onClick={() =>
                control(run.control === "PAUSED" ? "resume" : "pause")
              }
            >
              {t(run.control === "PAUSED" ? "resume" : "pause")}
            </button>
            <button
              type="button"
              className="danger"
              onClick={() => control("cancel")}
            >
              {t("cancel")}
            </button>
          </div>
          <form
            onSubmit={(event) => {
              event.preventDefault();
              const content = draft.trim();
              if ((!content && files.length === 0) || sending.current) return;
              sending.current = true;
              setFileError("");
              void (async () => {
                try {
                  const attachments: string[] = [];
                  for (const entry of files) {
                    let uploaded = entry.uploaded;
                    if (!uploaded) {
                      setUploading(true);
                      uploaded = (
                        await channelAttachmentUpload(
                          run.workspace_id,
                          entry.file,
                          {
                            filename: entry.file.name,
                            media_type:
                              entry.file.type || "application/octet-stream",
                            idempotency_key: entry.key,
                          },
                        )
                      ).id;
                      setFiles((current) =>
                        current.map((file) =>
                          file.key === entry.key ? { ...file, uploaded } : file,
                        ),
                      );
                    }
                    attachments.push(uploaded);
                  }
                  if (
                    request.current?.content !== content ||
                    JSON.stringify(request.current.attachments) !==
                      JSON.stringify(attachments)
                  ) {
                    request.current = {
                      content,
                      attachments,
                      key: crypto.randomUUID(),
                    };
                  }
                  const idempotency_key = request.current.key;
                  const ok = await submit(() =>
                    local
                      ? runMessage(id, {
                          content,
                          idempotency_key,
                          attachment_ids: attachments,
                        })
                      : remoteAction({
                          node_id: node,
                          control: {
                            run_id: id,
                            action: "message",
                            content,
                            idempotency_key,
                          },
                        }),
                  );
                  if (ok) {
                    setDraft("");
                    setFiles([]);
                    request.current = null;
                  }
                } catch (reason) {
                  setFileError(
                    `${fileCopy.attachmentError} ${reason instanceof Error ? reason.message : String(reason)}`,
                  );
                } finally {
                  setUploading(false);
                  sending.current = false;
                }
              })();
            }}
          >
            <Field label={copy.message}>
              <textarea
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                required={files.length === 0}
                rows={3}
              />
            </Field>
            {local && (
              <>
                <input
                  type="file"
                  accept={RUN_MEDIA_ACCEPT}
                  multiple
                  aria-label={fileCopy.attach}
                  onChange={(event) => {
                    const selected = [
                      ...files,
                      ...Array.from(event.target.files ?? []).map((file) => ({
                        key: crypto.randomUUID(),
                        file,
                      })),
                    ];
                    if (validRunMediaAttachments(selected.map((item) => item.file))) {
                      setFiles(selected);
                      request.current = null;
                      setFileError("");
                    } else setFileError(fileCopy.invalidAttachment);
                    event.target.value = "";
                  }}
                />
                {files.length > 0 && (
                  <ul className="workspace-draft-files">
                    {files.map((entry) => (
                      <li key={entry.key}>
                        {entry.file.name}
                        <button
                          type="button"
                          aria-label={`${fileCopy.removeAttachment}: ${entry.file.name}`}
                          onClick={() => {
                            setFiles((current) =>
                              current.filter((file) => file.key !== entry.key),
                            );
                            request.current = null;
                          }}
                        >
                          ×
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
                <p>{fileCopy.attachmentLimit}</p>
              </>
            )}
            {fileError && (
              <p role="alert" className="error">
                {fileError}
              </p>
            )}
            <button className="primary" disabled={uploading}>
              {uploading ? fileCopy.uploading : copy.send}
            </button>
          </form>
        </>
      )}
      {run.error && <p className="error">{run.error}</p>}
      <h3>{t("toolCalls")}</h3>
      {invocations.map((call) => (
        <details className="call-detail" key={call.idempotency_key}>
          <summary>
            <Badge value={call.status} />
            {call.tool}
          </summary>
          <h4>{t("input")}</h4>
          <JsonView value={call.input} />
          <h4>{t("result")}</h4>
          <JsonView value={call.result} />
        </details>
      ))}
      {local && query.hasNextPage && (
        <button
          type="button"
          disabled={query.isFetchingNextPage}
          onClick={() => void query.fetchNextPage()}
        >
          {t("loadMore")}
        </button>
      )}
      <details>
        <summary>{t("context")}</summary>
        <JsonView value={run.context} />
      </details>
      {local && (
        <details>
          <summary>{t("memory")}</summary>
          <JsonView value={query.data?.pages[0].memory ?? {}} />
        </details>
      )}
    </>
  );
}
