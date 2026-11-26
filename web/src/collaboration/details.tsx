import { ProviderToolActivity } from "../provider-tools";
import { SafetyStopPanel } from "./safety-stop";
import { ElicitationForm, elicitationMessage } from "./elicitation";
import { RunPlanUsage } from "../model-source";
import { X } from "lucide-react";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { Textarea } from "../components/ui/textarea";
import {
  RemoteMemoryProvenance,
  RemoteRunManagement,
  semanticReasons,
} from "./remote-memory";
import { ApiError, apiFetch } from "../transport";
import { DeclineCall } from "./decline-call";
import { offeredDecline } from "./decline-call-model";
import { RemoteExecutions } from "./remote-executions";
import { FlowStopButton } from "../org-chart";
import { ReferenceName } from "../record-view";
import {
  Alert,
  Disclosure,
  Facts,
  Hint,
  formClass,
} from "../components/patterns";
import { useRef, useState } from "react";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import {
  threadGet,
  executionGet,
  executionControl,
  executionMessage,
  channelAttachmentUpload,
  humanAnswer,
  humanRequests,
  taskAbandon,
  taskExecutions as taskRunPage,
} from "../generated/aidash";
import type {
  Artifact,
  Discovery,
  Entry,
  HumanRequest,
  Run,
  State,
  Task,
  Thread,
} from "../types";
import {
  Badge,
  Field,
  JsonView,
  Modal,
  Panel,
  useI18n,
  useAgentLabel,
} from "../ui";
import {
  AssignForm,
  EntityForm,
  RequestForm,
  TaskForm,
  ThreadForm,
  type Submit,
} from "../forms";
import { GenerationAssignForm } from "../generation";
import { EntityDetails } from "../entity-details";
import { ApprovalReviewPanel } from "../approval-review";
import { RunSubnodes } from "./subnodes";
import { ArtifactList } from "./channel";
import {
  answeredAtDeadline,
  closedWhenRunEnded,
  continuesAt,
  deadlineTime,
} from "../approval-settings-model";
import { collaborationCopy } from "./copy";
import { threadCopy } from "./thread-copy";
import { InferenceProgress } from "./inference-progress";
import { OutageWait } from "./outage-wait";
import { abandonable, resumableRun } from "./thread-model";
import { isLocalRun } from "../node-selection";
import {
  runMediaAccept,
  validRunMediaAttachments,
  validRunMediaRoute,
} from "./conversation-model";

export type Selection = {
  kind: string;
  task?: Task;
  entity?: Entry;
  thread?: Thread;
  run?: Run;
  request?: HumanRequest;
  artifact?: Artifact;
  node?: string;
  carryResults?: {
    node: string;
    source_version: string | null;
    changed: string[];
    unapproved_dependencies: { id: string; version: string }[];
    carried: boolean;
  }[];
};
export function OperationsDialog({
  selection,
  data,
  discovery,
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
  open: (selection: Selection) => void;
  close: () => void;
  submit: Submit;
  error: string;
  busy: boolean;
  visitChannel: (id: string) => void;
}) {
  const { t, locale } = useI18n();
  const agentLabel = useAgentLabel(data, discovery);
  const copy = collaborationCopy[locale];
  const d = selection;
  const [carryNode, setCarryNode] = useState<string>();
  const taskSnapshot = useQuery({
    queryKey: ["thread", d.task?.thread_id],
    queryFn: () => threadGet(d.task!.thread_id),
    enabled: Boolean(d.task),
    refetchInterval: 2000,
    retry: false,
  });
  const task =
    taskSnapshot.isError || !taskSnapshot.data
      ? undefined
      : taskSnapshot.data.tasks.find((value) => value.id === d.task?.id);
  const pendingRequest = data.human_requests.find(
    (value) => value.id === d.request?.id,
  );
  const requestDetail = useQuery({
    queryKey: ["human-request", d.request?.run_id, d.request?.id],
    enabled: Boolean(d.kind === "human" && d.request && !pendingRequest),
    queryFn: async ({ signal }) => {
      let cursor: string | null = null;
      do {
        const page = await humanRequests(
          { run: d.request!.run_id, state: "all", cursor },
          { signal },
        );
        const match = page.items.find((item) => item.id === d.request!.id);
        if (match) return match;
        cursor = page.next_cursor;
      } while (cursor);
      return null;
    },
    retry: false,
  });
  const request =
    pendingRequest ??
    (requestDetail.isError ? undefined : (requestDetail.data ?? undefined));
  const continues = request ? continuesAt(request) : null;
  const isDeploymentAdministrator =
    data.access.kind === "deployment_administrator";
  const unavailable = isDeploymentAdministrator && d.kind === "entity";
  const execution = ["thread", "goal", "task", "assign", "human"].includes(
    d.kind,
  );
  const taskExecutions = task
    ? data.runs.filter(
        (run) => run.task_id === task.id && isLocalRun(run, data),
      )
    : [];
  const started = useQuery({
    queryKey: ["task-has-runs", task?.id, task?.revision],
    enabled: task?.status === "OPEN",
    queryFn: async ({ signal }) => {
      let cursor: string | null = null;
      do {
        const page = await taskRunPage(task!.id, { cursor }, { signal });
        if (page.items.length) return true;
        cursor = page.next_cursor;
      } while (cursor);
      return false;
    },
    retry: false,
  });
  return (
    <Modal
      title={
        d.kind === "thread"
          ? copy.prepare
          : t(
              (
                {
                  goal: "newRequest",
                  task: "newTask",
                  taskDetail: "task",
                  entityDetail: "agent",
                  run: "execution",
                  human: "human",
                  assign: "delegate",
                  generate: "generationAssign",
                  entity: "register",
                  cluster: "register",
                  registrationCarry: "register",
                } as Record<string, string>
              )[d.kind] ?? d.kind,
            )
      }
      close={close}
    >
      {error && <Alert>{error}</Alert>}
      <fieldset disabled={busy} className="m-0 grid min-w-0 gap-4 border-0 p-0">
        {unavailable ? (
          <Hint>{copy.unavailable}</Hint>
        ) : execution && isDeploymentAdministrator ? (
          <Hint>{copy.deploymentAdministratorExecution}</Hint>
        ) : (
          <>
            {d.kind === "registrationCarry" &&
              d.entity &&
              data.access.kind === "subject" &&
              (carryNode ? (
                <ApprovalReviewPanel
                  tenant={data.access.tenant}
                  node={carryNode}
                  root={{ id: d.entity.id, version: d.entity.version }}
                  onClose={() => setCarryNode(undefined)}
                />
              ) : (
                <div className="grid gap-3">
                  {d.carryResults
                    ?.filter((result) => !result.carried)
                    .map((result) => (
                      <div key={result.node} className="grid gap-2">
                        <span>
                          {result.node}: {result.changed.join(", ")}
                          {result.unapproved_dependencies
                            .map(
                              (dependency) =>
                                ` ${dependency.id}@${dependency.version}`,
                            )
                            .join("")}
                        </span>
                        <Button
                          type="button"
                          onClick={() => setCarryNode(result.node)}
                        >
                          {t("approvalRequest")}
                        </Button>
                      </div>
                    ))}
                </div>
              ))}
            {d.kind === "thread" && (
              <>
                <Hint>{copy.setupNotice}</Hint>
                <ThreadForm submit={submit} />
              </>
            )}
            {d.kind === "goal" && <RequestForm data={data} submit={submit} />}
            {d.kind === "task" && (
              <TaskForm data={data} thread={d.thread?.id} submit={submit} />
            )}
            {d.kind === "entity" && (
              <EntityForm data={data} submit={submit} initial="agent" />
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
                  data={data}
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
                key={`${d.node ?? data.node.id}:${d.run.id}:${d.run.thread_id}`}
                id={d.run.id}
                node={d.node ?? data.node.id}
                data={data}
                discovery={discovery}
                submit={submit}
                visitChannel={visitChannel}
              />
            )}
            {d.kind === "human" &&
              (request &&
              request.response === null &&
              (!d.node || d.node === data.node.id) ? (
                request.kind === "MCP_ELICITATION" ? (
                  <ElicitationForm request={request} submit={submit} />
                ) : (
                  <form
                    className={formClass}
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
                      void submit(() => humanAnswer(request.id, response));
                    }}
                  >
                    <div className="grid min-w-0 justify-items-start gap-2">
                      <Badge value={request.kind} />
                      <p className="whitespace-pre-wrap text-[13px] leading-relaxed text-foreground [overflow-wrap:anywhere]">
                        {elicitationMessage(request)}
                      </p>
                      {continues && (
                        <Hint>
                          {copy.continuesAt.replace(
                            "{time}",
                            deadlineTime(continues, locale),
                          )}
                        </Hint>
                      )}
                    </div>
                    <Field label={t("answer")}>
                      <Textarea name="answer" rows={5} required />
                    </Field>
                    <div className="flex justify-end">
                      <Button type="submit">{copy.send}</Button>
                    </div>
                  </form>
                )
              ) : request &&
                (answeredAtDeadline(request) || closedWhenRunEnded(request)) ? (
                <div className="grid min-w-0 justify-items-start gap-2">
                  <Badge value={request.kind} />
                  <p className="whitespace-pre-wrap text-[13px] leading-relaxed text-foreground [overflow-wrap:anywhere]">
                    {elicitationMessage(request)}
                  </p>
                  <Hint role="status">
                    {closedWhenRunEnded(request)
                      ? copy.questionClosedRunEnded
                      : copy.noAnswerBeforeDeadline}
                  </Hint>
                </div>
              ) : (
                <Hint role="status">{copy.unavailable}</Hint>
              ))}
            {d.kind === "taskDetail" &&
              (task ? (
                <>
                  <header className="grid min-w-0 gap-2">
                    <div className="flex min-w-0 flex-wrap items-center gap-2">
                      <h2 className="min-w-0 text-[15px] font-semibold text-foreground [overflow-wrap:anywhere]">
                        {task.title}
                      </h2>
                      <Badge value={task.status} />
                    </div>
                    {task.description && (
                      <p className="whitespace-pre-wrap text-[13px] leading-relaxed text-muted-foreground [overflow-wrap:anywhere]">
                        {task.description}
                      </p>
                    )}
                  </header>
                  <Facts
                    items={[
                      [
                        t("owner"),
                        task.owner ? (
                          <ReferenceName id={task.owner} />
                        ) : (
                          t("noAssignment")
                        ),
                      ],
                      [t("revision"), task.revision, true],
                    ]}
                  />
                  <div className="flex min-w-0 flex-wrap gap-2">
                    {task.status === "OPEN" && (
                      <>
                        <Button
                          type="button"
                          onClick={() => open({ kind: "assign", task })}
                        >
                          {t("delegate")}
                        </Button>
                        {data.access.kind === "subject" && (
                          <Button
                            variant="outline"
                            type="button"
                            onClick={() => open({ kind: "generate", task })}
                          >
                            {t("generationAssign")}
                          </Button>
                        )}
                      </>
                    )}
                    <Button
                      variant="outline"
                      type="button"
                      onClick={() => visitChannel(task.thread_id)}
                    >
                      {copy.channelLink}
                    </Button>
                  </div>
                  {data.access.kind === "subject" && (
                    <RemoteExecutions task={task.id} />
                  )}
                  {data.access.kind === "subject" && !task.parent_id && (
                    // A Root Task: one action stops everything its flow made (#1012).
                    <FlowStopButton
                      tenant={data.access.tenant}
                      rootTask={task.id}
                    />
                  )}
                  {abandonable(
                    task,
                    task.status === "OPEN" && started.data !== false
                      ? [true]
                      : [],
                  ) && (
                    <form
                      className="grid min-w-0 gap-3 border-t border-border pt-4"
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
                        <Textarea required name="reason" />
                      </Field>
                      <Hint>{t("abandonHelp")}</Hint>
                      <div>
                        <Button variant="destructive" type="submit">
                          {t("abandonTask")}
                        </Button>
                      </div>
                    </form>
                  )}
                  {data.next_cursors.runs && (
                    <p className="text-xs text-faint">
                      {locale === "ja-JP"
                        ? "以前の実行があります"
                        : "Older runs exist"}
                    </p>
                  )}
                  {taskExecutions.length > 0 && (
                    <Panel title={t("execution")}>
                      <ul className="grid min-w-0 divide-y divide-border border-y border-border">
                        {taskExecutions.map((run) => (
                          <li key={run.id}>
                            <button
                              type="button"
                              className="flex h-10 w-full min-w-0 items-center gap-2.5 px-2 text-left text-[13px] text-foreground transition-colors duration-150 hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
                              onClick={() =>
                                open({ kind: "run", run, node: data.node.id })
                              }
                            >
                              <Badge value={run.phase} />
                              <span className="min-w-0 truncate">
                                {agentLabel(data.node.id, {
                                  id: run.agent_id,
                                  version: run.agent_version,
                                })}
                              </span>
                            </button>
                          </li>
                        ))}
                      </ul>
                    </Panel>
                  )}
                  <Disclosure summary={t("requirements")}>
                    <JsonView value={task.requirements} />
                  </Disclosure>
                  <ArtifactList
                    artifacts={(taskSnapshot.data?.artifacts ?? []).filter(
                      (artifact) => artifact.task_id === task.id,
                    )}
                  />
                </>
              ) : (
                <Hint role="status">
                  {taskSnapshot.isPending ? copy.processing : copy.unavailable}
                </Hint>
              ))}
          </>
        )}
      </fieldset>
    </Modal>
  );
}

/**
 * Steps that went without optional Thread semantic context (ADR 0059). Only a
 * Run local to its executing node degrades, which need not be the viewer's node.
 */
function DegradedContext({ run }: { run: Pick<Run, "recovery"> }) {
  const { t } = useI18n();
  const degraded = run.recovery?.degraded;
  if (!degraded?.count) return null;
  return (
    <section className="grid min-w-0 gap-1.5" role="status">
      <p className="text-xs font-medium text-foreground">
        {t("runDegradedContext")} ({degraded.count})
      </p>
      <Hint>{t("runDegradedContextHint")}</Hint>
      <Facts
        items={degraded.recent.map(({ step, cause }) => [
          `${t("runStep")} ${step}`,
          t(
            cause === "embedding_provider"
              ? "degradedCauseEmbedding"
              : cause === "index_incomplete"
                ? "degradedCauseIndexIncomplete"
                : cause === "data_protection"
                  ? "degradedCauseDataProtection"
                  : "degradedCauseVector",
          ),
        ])}
      />
    </section>
  );
}

function RunPanel({
  id,
  node,
  data,
  discovery,
  submit,
  visitChannel,
}: {
  id: string;
  node: string;
  data: State;
  discovery?: Discovery;
  submit: Submit;
  visitChannel: (id: string) => void;
}) {
  const { t, locale } = useI18n();
  const copy = collaborationCopy[locale];
  const fileCopy = threadCopy[locale];
  const agentLabel = useAgentLabel(data, discovery);
  const query = useInfiniteQuery({
    queryKey: ["run", node, id],
    initialPageParam: 0,
    queryFn: ({ pageParam }) => executionGet(id, { offset: pageParam }),
    getNextPageParam: (last, pages) =>
      last.invocations.length === 100 ||
      last.egress_requests?.length === 100 ||
      last.provider_tool_activity?.length === 100
        ? pages.length * 100
        : undefined,
    refetchInterval: 2000,
    retry: false,
  });
  const run = query.isError ? undefined : query.data?.pages[0].run;
  const [draft, setDraft] = useState("");
  const [files, setFiles] = useState<
    { key: string; file: File; uploaded?: string }[]
  >([]);
  const [fileError, setFileError] = useState("");
  const [uploading, setUploading] = useState(false);
  const [sendingMessage, setSendingMessage] = useState(false);
  const sending = useRef(false);
  const request = useRef<{
    content: string;
    attachments: string[];
    key: string;
  } | null>(null);
  if (!run) {
    const hiddenBy =
      query.error instanceof ApiError && query.error.runReads
        ? query.error.message
        : undefined;
    return (
      <>
        <Hint role="status">
          {query.isPending ? copy.processing : (hiddenBy ?? copy.unavailable)}
        </Hint>
        {data.access.kind === "subject" && (
          <RemoteRunManagement id={id} resumeElsewhere={!hiddenBy} />
        )}
      </>
    );
  }
  const mediaInputRoutes = query.data?.pages[0].media_input_routes ?? [];
  const mediaAccept = runMediaAccept(mediaInputRoutes);
  // Sending attachments still requires an exact Node, even when an
  // administrator may monitor the Run across its Deployment.
  const canAttach = run.home_node === data.node.id && !!mediaAccept;
  const terminal = ["COMPLETED", "FAILED", "CANCELLED"].includes(run.phase);
  const control = (action: "pause" | "resume" | "cancel") => {
    if (action === "cancel" && !window.confirm(t("confirmCancel"))) return;
    void submit(() => executionControl(id, { action }));
  };
  const invocations =
    query.data?.pages.flatMap((page) => page.invocations) ?? [];
  const managed =
    run.home_node !== data.node.id && data.access.kind === "subject";
  const controllable = !terminal && !managed;
  // A Deployment Administrator stops a Run only through a Safety Stop, with a
  // reason, and never resumes it (ADR 0101).
  const administrator = data.access.kind === "deployment_administrator";
  const home = isLocalRun(run, data);
  // The server offers the paused call only to someone who may decline it.
  const declinable = offeredDecline(query.data?.pages[0], run.control);
  return (
    <>
      <header className="grid min-w-0 gap-1.5">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <h2 className="min-w-0 text-[15px] font-semibold text-foreground [overflow-wrap:anywhere]">
            {agentLabel(node, { id: run.agent_id, version: run.agent_version })}
          </h2>
          <Badge value={run.phase} />
          <Badge value={run.control} />
        </div>
        <p className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
          <ReferenceName id={node} />
        </p>
      </header>
      {controllable &&
        run.control === "PAUSED" &&
        run.recovery?.semantic_reason && (
          // The same cause and next step a Run on another Node reports (#127).
          <Hint role="status">
            {
              semanticReasons[run.recovery.semantic_reason][
                locale === "ja-JP" ? 1 : 0
              ]
            }
          </Hint>
        )}
      {(controllable || home) && (
        <div className="flex min-w-0 flex-wrap gap-2">
          {controllable && !administrator && (
            <>
              <Button
                variant="outline"
                type="button"
                disabled={run.control === "PAUSED" && !resumableRun(run)}
                onClick={() =>
                  control(run.control === "PAUSED" ? "resume" : "pause")
                }
              >
                {t(run.control === "PAUSED" ? "resume" : "pause")}
              </Button>
              <Button
                variant="destructive"
                type="button"
                onClick={() => control("cancel")}
              >
                {t("cancel")}
              </Button>
              {declinable && (
                <DeclineCall
                  call={declinable}
                  submit={submit}
                  send={(content, idempotency_key) =>
                    executionMessage(id, {
                      content,
                      idempotency_key,
                      attachment_ids: [],
                    })
                  }
                  decline={(call, rest_of_run) =>
                    apiFetch(`/api/executions/${id}/declined-calls`, {
                      method: "POST",
                      headers: { "content-type": "application/json" },
                      body: JSON.stringify({
                        call,
                        ...(rest_of_run ? { rest_of_run: true } : {}),
                      }),
                    })
                  }
                />
              )}
            </>
          )}
          {home && (
            <Button
              variant="ghost"
              type="button"
              onClick={() => visitChannel(run.thread_id)}
            >
              {copy.channelLink}
            </Button>
          )}
        </div>
      )}
      {controllable && administrator && <SafetyStopPanel run={id} />}
      {run.control !== "ACTIVE" &&
        run.control_source === "deployment_administrator" && (
          <Hint role="status">
            {run.control === "CANCELLED"
              ? copy.cancelledByAdministrator
              : copy.pausedByAdministrator}
          </Hint>
        )}
      <RunPlanUsage run={run} data={data} />
      {run.state_error && <Alert>{run.state_error}</Alert>}
      {run.error && (
        <p className="text-xs leading-relaxed text-destructive [overflow-wrap:anywhere]">
          {run.error}
        </p>
      )}
      <OutageWait run={run} />
      <DegradedContext run={run} />
      {/* The server denies subjects the stream of a foreign-home run. */}
      {!managed && (
        <InferenceProgress run={id} node={node} active={!terminal} />
      )}
      {!managed && <RunSubnodes run={id} node={node} agentLabel={agentLabel} />}
      {managed && (
        <RemoteMemoryProvenance url={`/api/executions/${id}/semantic`} />
      )}
      {managed && <RemoteRunManagement id={id} />}
      {controllable && (
        <form
          className="grid min-w-0 gap-3 border-t border-border pt-4"
          onSubmit={(event) => {
            event.preventDefault();
            const content = draft.trim();
            if ((!content && files.length === 0) || sending.current) return;
            if (
              files.length > 0 &&
              (!canAttach ||
                !validRunMediaRoute(
                  files.map((entry) => entry.file),
                  mediaInputRoutes,
                ))
            ) {
              setFileError(fileCopy.invalidAttachment);
              return;
            }
            sending.current = true;
            setSendingMessage(true);
            setFileError("");
            void (async () => {
              try {
                const attachments: string[] = [];
                if (files.length > 0) {
                  const current = await executionGet(id, { offset: 0 });
                  if (
                    !validRunMediaRoute(
                      files.map((entry) => entry.file),
                      current.media_input_routes,
                    )
                  ) {
                    setFileError(fileCopy.invalidAttachment);
                    return;
                  }
                }
                for (const entry of files) {
                  let uploaded = entry.uploaded;
                  if (!uploaded) {
                    setUploading(true);
                    uploaded = (
                      await channelAttachmentUpload(run.thread_id, entry.file, {
                        filename: entry.file.name,
                        media_type:
                          entry.file.type || "application/octet-stream",
                        idempotency_key: entry.key,
                      })
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
                  executionMessage(id, {
                    content,
                    idempotency_key,
                    attachment_ids: attachments,
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
                setSendingMessage(false);
              }
            })();
          }}
        >
          <Field label={copy.message}>
            <Textarea
              value={draft}
              onChange={(event) => {
                if (!sending.current) setDraft(event.target.value);
              }}
              disabled={sendingMessage}
              required={files.length === 0}
              rows={3}
            />
          </Field>
          {canAttach && (
            <div className="grid min-w-0 gap-1.5">
              <Input
                type="file"
                accept={mediaAccept}
                multiple
                aria-label={fileCopy.attach}
                disabled={sendingMessage}
                className="h-auto py-1.5"
                onChange={(event) => {
                  if (sending.current) return;
                  const selected = [
                    ...files,
                    ...Array.from(event.target.files ?? []).map((file) => ({
                      key: crypto.randomUUID(),
                      file,
                    })),
                  ];
                  if (
                    validRunMediaAttachments(
                      selected.map((item) => item.file),
                    ) &&
                    validRunMediaRoute(
                      selected.map((item) => item.file),
                      mediaInputRoutes,
                    )
                  ) {
                    setFiles(selected);
                    request.current = null;
                    setFileError("");
                  } else setFileError(fileCopy.invalidAttachment);
                  event.target.value = "";
                }}
              />
              <p className="text-[11px] text-faint">
                {fileCopy.attachmentLimit}
              </p>
            </div>
          )}
          {files.length > 0 && (
            <ul className="flex min-w-0 flex-wrap gap-1.5">
              {files.map((entry) => (
                <li
                  key={entry.key}
                  className="flex h-7 min-w-0 max-w-full items-center gap-1 rounded-sm border border-border bg-raised pl-2 text-xs text-foreground"
                >
                  <span className="min-w-0 truncate font-mono">
                    {entry.file.name}
                  </span>
                  <Button
                    variant="ghost"
                    size="icon"
                    type="button"
                    className="size-6"
                    aria-label={`${fileCopy.removeAttachment}: ${entry.file.name}`}
                    disabled={sendingMessage}
                    onClick={() => {
                      if (sending.current) return;
                      setFiles((current) =>
                        current.filter((file) => file.key !== entry.key),
                      );
                      request.current = null;
                    }}
                  >
                    <X aria-hidden />
                  </Button>
                </li>
              ))}
            </ul>
          )}
          {fileError && <Alert>{fileError}</Alert>}
          <div className="flex justify-end">
            <Button type="submit" disabled={sendingMessage || uploading}>
              {uploading ? fileCopy.uploading : copy.send}
            </Button>
          </div>
        </form>
      )}
      {query.data?.pages.some((page) => page.egress_requests?.length) && (
        <Panel title="External services">
          {query.data.pages
            .flatMap((page) => page.egress_requests ?? [])
            .map((request) => (
              <Disclosure
                key={request.id}
                summary={`${request.service} · ${request.method} ${request.path} · ${request.status}`}
              >
                <JsonView value={request} />
              </Disclosure>
            ))}
        </Panel>
      )}
      <Panel title={t("toolCalls")}>
        {invocations.length === 0 && <Hint>{t("empty")}</Hint>}
        {invocations.map((call) => (
          <Disclosure
            key={call.idempotency_key}
            summary={
              <span className="flex min-w-0 items-center gap-2">
                <Badge value={call.status} />
                <span className="min-w-0 truncate font-mono text-foreground">
                  {call.tool}
                </span>
              </span>
            }
          >
            <h4 className="text-[11px] font-medium text-faint">{t("input")}</h4>
            <JsonView value={call.input} />
            <h4 className="text-[11px] font-medium text-faint">
              {t("result")}
            </h4>
            <JsonView value={call.result} />
          </Disclosure>
        ))}
        {query.hasNextPage && (
          <div>
            <Button
              variant="outline"
              type="button"
              disabled={query.isFetchingNextPage}
              onClick={() => void query.fetchNextPage()}
            >
              {t("loadMore")}
            </Button>
          </div>
        )}
      </Panel>
      <ProviderToolActivity
        context={run.context}
        journal={query.data?.pages.flatMap(
          (page) => page.provider_tool_activity ?? [],
        )}
      />
      <Disclosure summary={t("context")}>
        <JsonView value={run.context} />
      </Disclosure>
      <Disclosure summary={t("memory")}>
        <JsonView value={query.data?.pages[0].memory ?? {}} />
      </Disclosure>
    </>
  );
}
