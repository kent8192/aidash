import { SafetyStopPanel } from "./safety-stop";
import { ElicitationForm, elicitationMessage } from "./elicitation";
import { RunCompactionDetails } from "../compaction-details";
import { RunPlanUsage } from "../model-source";
import { X } from "lucide-react";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { Textarea } from "../components/ui/textarea";
import { RemoteMemoryProvenance, RemoteRunManagement } from "./remote-memory";
import { RemoteExecutions } from "./remote-executions";
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
  workspaceGet,
  runGet,
  runControl,
  runMessage,
  channelAttachmentUpload,
  humanAnswer,
  taskAbandon,
} from "../generated/aidash";
import type {
  Artifact,
  Discovery,
  Entry,
  HumanRequest,
  Run,
  State,
  Task,
  Workspace,
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
  GoalForm,
  TaskForm,
  WorkspaceForm,
  type Submit,
} from "../forms";
import { GenerationAssignForm } from "../generation";
import { RunResearchEvidence } from "../research-governance";
import { EntityDetails } from "../entity-details";
import { ArtifactList } from "./channel";
import {
  answeredAtDeadline,
  continuesAt,
  deadlineTime,
} from "../approval-settings-model";
import { collaborationCopy } from "./copy";
import { workspaceCopy } from "./workspace-copy";
import { InferenceProgress } from "./inference-progress";
import { resumableRun } from "./workspace-model";
import { researchFailure } from "../research";
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
  workspace?: Workspace;
  run?: Run;
  request?: HumanRequest;
  artifact?: Artifact;
  node?: string;
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
  const request = data.human_requests.find(
    (value) => value.id === d.request?.id,
  );
  const continues = request ? continuesAt(request) : null;
  const isDeploymentAdministrator =
    data.access.kind === "deployment_administrator";
  const unavailable =
    isDeploymentAdministrator && (d.kind === "entity" || d.kind === "cluster");
  const execution = ["workspace", "goal", "task", "assign", "human"].includes(
    d.kind,
  );
  const taskRuns = task
    ? data.runs.filter(
        (run) => run.task_id === task.id && isLocalRun(run, data),
      )
    : [];
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
            {d.kind === "workspace" && (
              <>
                <Hint>{copy.setupNotice}</Hint>
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
                key={`${d.node ?? data.node.id}:${d.run.id}:${d.run.workspace_id}`}
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
              ) : request && answeredAtDeadline(request) ? (
                <div className="grid min-w-0 justify-items-start gap-2">
                  <Badge value={request.kind} />
                  <p className="whitespace-pre-wrap text-[13px] leading-relaxed text-foreground [overflow-wrap:anywhere]">
                    {elicitationMessage(request)}
                  </p>
                  <Hint role="status">{copy.noAnswerBeforeDeadline}</Hint>
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
                      onClick={() => visitChannel(task.workspace_id)}
                    >
                      {copy.channelLink}
                    </Button>
                  </div>
                  {data.access.kind === "subject" && (
                    <RemoteExecutions task={task.id} />
                  )}
                  {["FAILED", "BLOCKED", "CANCELLED"].includes(task.status) && (
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
                  {taskRuns.length > 0 && (
                    <Panel title={t("execution")}>
                      <ul className="grid min-w-0 divide-y divide-border border-y border-border">
                        {taskRuns.map((run) => (
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
 * Steps that went without optional Workspace semantic context (ADR 0059). Only a
 * Run local to its executing node degrades, which need not be the viewer's node.
 */
function DegradedContext({ run }: { run: Run }) {
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
  const fileCopy = workspaceCopy[locale];
  const agentLabel = useAgentLabel(data, discovery);
  const query = useInfiniteQuery({
    queryKey: ["run", node, id],
    initialPageParam: 0,
    queryFn: ({ pageParam }) => runGet(id, { offset: pageParam }),
    getNextPageParam: (last, pages) =>
      last.invocations.length === 100 || last.egress_requests?.length === 100
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
  if (!run)
    return (
      <>
        <Hint role="status">
          {query.isPending ? copy.processing : copy.unavailable}
        </Hint>
        {data.access.kind === "subject" && <RemoteRunManagement id={id} />}
      </>
    );
  const mediaInputRoutes = query.data?.pages[0].media_input_routes ?? [];
  const mediaAccept = runMediaAccept(mediaInputRoutes);
  // Sending attachments still requires an exact Node, even when an
  // administrator may monitor the Run across its Deployment.
  const canAttach = run.home_node === data.node.id && !!mediaAccept;
  const terminal = ["COMPLETED", "FAILED", "CANCELLED"].includes(run.phase);
  const control = (action: "pause" | "resume" | "cancel") => {
    if (action === "cancel" && !window.confirm(t("confirmCancel"))) return;
    void submit(() => runControl(id, { action }));
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
            </>
          )}
          {home && (
            <Button
              variant="ghost"
              type="button"
              onClick={() => visitChannel(run.workspace_id)}
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
      <RunCompactionDetails
        bindings={run.context?.binding_snapshot?.bindings ?? []}
        reason={run.recovery?.context_reason}
      />
      <DegradedContext run={run} />
      {/* The server denies subjects the stream of a foreign-home run. */}
      {!managed && (
        <InferenceProgress run={id} node={node} active={!terminal} />
      )}
      {!managed && (
        <RunResearchEvidence
          run={id}
          deploymentAdministrator={
            data.access.kind === "deployment_administrator"
          }
        />
      )}
      {managed && <RemoteMemoryProvenance url={`/api/runs/${id}/semantic`} />}
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
                  const current = await runGet(id, { offset: 0 });
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
                  runMessage(id, {
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
            {researchFailure(call.result, locale === "ja-JP") && (
              <Hint role="status">
                {researchFailure(call.result, locale === "ja-JP")}
              </Hint>
            )}
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
      <Disclosure summary={t("context")}>
        <JsonView value={run.context} />
      </Disclosure>
      <Disclosure summary={t("memory")}>
        <JsonView value={query.data?.pages[0].memory ?? {}} />
      </Disclosure>
    </>
  );
}
