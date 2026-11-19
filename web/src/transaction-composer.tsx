import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import { useContext, useState, useRef, useEffect } from "react";
import type {
  TransactionManifest,
  TransactionMutation,
  TransactionParticipant,
} from "./generated/models";
import { DisplayState, ReferenceName } from "./record-view";
import { disambiguateLabels } from "./display-labels";
import { parseNodeAddress, registryDeployment } from "./node-selection";
import { Field, useI18n } from "./ui";
import { Group, Hint } from "./components/patterns";
import { useTaskPages } from "./task-picker-pages";

function clearResource(mutation: TransactionMutation): TransactionMutation {
  switch (mutation.kind) {
    case "thread_title":
      return { ...mutation, thread_id: "", expected_revision: 0 };
    case "complete_task":
      return { ...mutation, task_id: "", expected_revision: 0 };
    case "finish_run":
      return { ...mutation, run_id: "", task_id: "", expected_revision: 0 };
  }
}

export function TransactionComposer({
  value,
  change,
  deploymentAdministrator = true,
}: {
  deploymentAdministrator?: boolean;
  value: TransactionManifest;
  change: (value: TransactionManifest) => void;
}) {
  const data = useContext(DisplayState);
  // A manifest names one exact Node per Deployment. An administrator chooses
  // a local Node; a subject uses the Node selected in the dashboard.
  const local = value.coordinator;
  const localNode = parseNodeAddress(local)?.node;
  const threads =
    data?.threads.filter((thread) => thread.node === localNode) ?? [];
  const threadIds = new Set(threads.map((thread) => thread.id));
  // The Task window of `/api/state` holds 50 (#664); older local Tasks are
  // read on request. `GET /api/tasks` spans every Node, so only Tasks of the
  // local Node's Threads are offered. The window wins for a Task both
  // hold: it is re-read on every event.
  const taskPages = useTaskPages(data, ["transaction", local]);
  const tasks = [
    ...new Map(
      [...taskPages.older, ...(data?.tasks ?? [])]
        .filter((task) => threadIds.has(task.thread_id))
        .map((task) => [task.id, task]),
    ).values(),
  ];
  const runs =
    data?.runs.filter(
      (run) => threadIds.has(run.thread_id) && run.home_node === local,
    ) ?? [];
  const { t, locale } = useI18n();
  const threadLabels = disambiguateLabels(
    threads,
    (thread) => thread.id,
    (thread) => thread.title,
  );
  const taskLabels = disambiguateLabels(
    tasks,
    (task) => task.id,
    (task) =>
      `${threadLabels.get(task.thread_id) ?? t("unavailableEntity")} / ${task.title}`,
  );
  const runLabels = disambiguateLabels(
    runs,
    (run) => run.id,
    (run) =>
      `${taskLabels.get(run.task_id) ?? t("unavailableEntity")} · ${t(run.phase)}`,
  );
  const update = (index: number, participant: TransactionParticipant) =>
    change({
      ...value,
      participants: value.participants.map((old, i) =>
        i === index ? participant : old,
      ),
    });
  const nodes = deploymentAdministrator
    ? [
        ...new Set(
          data?.threads.map(
            (thread) => `${registryDeployment(data)}/${thread.node}`,
          ),
        ),
      ]
    : [local];
  return (
    <>
      <Field label={t("transactionDeadline")}>
        <Input
          type="datetime-local"
          className="font-mono tabular"
          required
          value={new Date(
            Date.parse(value.deadline) -
              new Date(value.deadline).getTimezoneOffset() * 60000,
          )
            .toISOString()
            .slice(0, 19)}
          onChange={(event) => {
            if (event.target.value)
              change({
                ...value,
                deadline: new Date(event.target.value).toISOString(),
              });
          }}
        />
      </Field>
      {value.participants.map((participant, index) => (
        <Group
          key={index}
          legend={`${t("transactionParticipant")} ${index + 1}`}
        >
          <Field label={t("node")}>
            {!deploymentAdministrator && index > 0 ? (
              <Input
                required
                placeholder="aidash://remote-deployment/node"
                value={participant.node_id}
                onChange={(event) =>
                  update(index, {
                    ...participant,
                    node_id: event.target.value,
                    mutations: participant.mutations.map(clearResource),
                  })
                }
              />
            ) : (
              <NativeSelect
                required
                disabled={
                  !deploymentAdministrator &&
                  participant.node_id === value.coordinator
                }
                value={participant.node_id}
                onChange={(event) => {
                  const next = {
                    ...participant,
                    node_id: event.target.value,
                    mutations: participant.mutations.map(clearResource),
                  };
                  if (deploymentAdministrator)
                    change({
                      ...value,
                      coordinator: next.node_id,
                      participants: [next],
                    });
                  else update(index, next);
                }}
              >
                <option value="">{t("choose")}</option>
                {nodes
                  .filter(
                    (node) =>
                      node === participant.node_id ||
                      !value.participants.some(
                        (other) => other.node_id === node,
                      ),
                  )
                  .map((node) => (
                    <option key={node} value={node}>
                      {deploymentAdministrator ? (
                        parseNodeAddress(node)?.node
                      ) : (
                        <ReferenceName id={node} />
                      )}
                    </option>
                  ))}
              </NativeSelect>
            )}
          </Field>
          {participant.node_id !== local && (
            <Hint>{t("transactionRemoteReferenceHelp")}</Hint>
          )}
          {participant.mutations.map((mutation, mutationIndex) => {
            const setMutation = (next: TransactionMutation) =>
              update(index, {
                ...participant,
                mutations: participant.mutations.map((old, i) =>
                  i === mutationIndex ? next : old,
                ),
              });
            return (
              <Group
                key={mutationIndex}
                nested
                legend={`${t("transactionOperation")} ${mutationIndex + 1}`}
              >
                <Field label={t("transactionOperation")}>
                  <NativeSelect
                    value={mutation.kind}
                    onChange={(event) => {
                      const kind = event.target.value;
                      if (kind === "thread_title")
                        setMutation({
                          kind,
                          thread_id: "",
                          expected_revision: 0,
                          title: "",
                        });
                      else if (kind === "complete_task")
                        setMutation({
                          kind,
                          task_id: "",
                          expected_revision: 0,
                          artifact: { name: "", kind: "text", content: "" },
                        });
                      else if (kind === "finish_run")
                        setMutation({
                          kind,
                          run_id: "",
                          task_id: "",
                          expected_revision: 0,
                        });
                    }}
                  >
                    {["thread_title", "complete_task", "finish_run"].map(
                      (kind) => (
                        <option key={kind} value={kind}>
                          {t(`transactionOperation_${kind}`)}
                        </option>
                      ),
                    )}
                  </NativeSelect>
                </Field>
                {mutation.kind === "thread_title" && (
                  <>
                    <Field label={t("thread")}>
                      {participant.node_id === local ? (
                        <NativeSelect
                          required
                          value={mutation.thread_id}
                          onChange={(event) =>
                            setMutation({
                              ...mutation,
                              thread_id: event.target.value,
                              expected_revision:
                                threads.find(
                                  (thread) => thread.id === event.target.value,
                                )?.revision ?? 0,
                            })
                          }
                        >
                          <option value="">{t("choose")}</option>
                          {threads.map((thread) => (
                            <option key={thread.id} value={thread.id}>
                              {threadLabels.get(thread.id)}
                            </option>
                          ))}
                        </NativeSelect>
                      ) : (
                        <Input
                          required
                          value={mutation.thread_id}
                          onChange={(event) =>
                            setMutation({
                              ...mutation,
                              thread_id: event.target.value,
                              expected_revision: 0,
                            })
                          }
                        />
                      )}
                    </Field>
                    <Field label={t("title")}>
                      <Input
                        required
                        value={mutation.title}
                        onChange={(event) =>
                          setMutation({
                            ...mutation,
                            title: event.target.value,
                          })
                        }
                      />
                    </Field>
                  </>
                )}
                {mutation.kind === "complete_task" && (
                  <>
                    <Field label={t("task")}>
                      {participant.node_id === local ? (
                        <NativeSelect
                          required
                          value={mutation.task_id}
                          onChange={(event) =>
                            setMutation({
                              ...mutation,
                              task_id: event.target.value,
                              expected_revision:
                                tasks.find(
                                  (task) => task.id === event.target.value,
                                )?.revision ?? 0,
                            })
                          }
                        >
                          <option value="">{t("choose")}</option>
                          {tasks.map((task) => (
                            <option key={task.id} value={task.id}>
                              {taskLabels.get(task.id)}
                            </option>
                          ))}
                        </NativeSelect>
                      ) : (
                        <Input
                          required
                          value={mutation.task_id}
                          onChange={(event) =>
                            setMutation({
                              ...mutation,
                              task_id: event.target.value,
                              expected_revision: 0,
                            })
                          }
                        />
                      )}
                    </Field>
                    {participant.node_id === local && taskPages.more && (
                      <Button
                        type="button"
                        variant="outline"
                        disabled={taskPages.loading}
                        onClick={taskPages.load}
                      >
                        {locale === "ja-JP"
                          ? "タスクをさらに表示"
                          : "Show more tasks"}
                      </Button>
                    )}
                    {participant.node_id === local && taskPages.error && (
                      <p role="alert">
                        {locale === "ja-JP"
                          ? "タスクを読み込めませんでした。"
                          : "Unable to load tasks."}
                      </p>
                    )}
                    <Field label={t("name")}>
                      <Input
                        required
                        value={mutation.artifact.name}
                        onChange={(event) =>
                          setMutation({
                            ...mutation,
                            artifact: {
                              ...mutation.artifact,
                              name: event.target.value,
                            },
                          })
                        }
                      />
                    </Field>
                    <Field label={t("kind")}>
                      <Input
                        required
                        value={mutation.artifact.kind}
                        onChange={(event) =>
                          setMutation({
                            ...mutation,
                            artifact: {
                              ...mutation.artifact,
                              kind: event.target.value,
                            },
                          })
                        }
                      />
                    </Field>
                    <JsonInput
                      label={t("artifact")}
                      value={mutation.artifact.content}
                      change={(content) =>
                        setMutation({
                          ...mutation,
                          artifact: { ...mutation.artifact, content },
                        })
                      }
                    />
                  </>
                )}
                {mutation.kind === "finish_run" && (
                  <>
                    <Field label={t("execution")}>
                      {participant.node_id === local ? (
                        <NativeSelect
                          required
                          value={mutation.run_id}
                          onChange={(event) => {
                            const run = runs.find(
                              (run) => run.id === event.target.value,
                            );
                            if (run)
                              setMutation({
                                ...mutation,
                                run_id: run.id,
                                task_id: run.task_id,
                                expected_revision: run.revision,
                              });
                          }}
                        >
                          <option value="">{t("choose")}</option>
                          {runs.map((run) => (
                            <option key={run.id} value={run.id}>
                              {runLabels.get(run.id)}
                            </option>
                          ))}
                        </NativeSelect>
                      ) : (
                        <Input
                          required
                          value={mutation.run_id}
                          onChange={(event) =>
                            setMutation({
                              ...mutation,
                              run_id: event.target.value,
                              expected_revision: 0,
                            })
                          }
                        />
                      )}
                    </Field>
                    {participant.node_id !== local && (
                      <Field label={t("task")}>
                        <Input
                          required
                          value={mutation.task_id}
                          onChange={(event) =>
                            setMutation({
                              ...mutation,
                              task_id: event.target.value,
                              expected_revision: 0,
                            })
                          }
                        />
                      </Field>
                    )}
                  </>
                )}
                <Field label={t("revision")}>
                  <Input
                    type="number"
                    required
                    min={0}
                    value={mutation.expected_revision}
                    onChange={(event) =>
                      setMutation({
                        ...mutation,
                        expected_revision: Number(event.target.value),
                      })
                    }
                  />
                </Field>
                <Button
                  variant="ghost"
                  size="sm"
                  className="justify-self-start hover:text-destructive"
                  type="button"
                  onClick={() =>
                    update(index, {
                      ...participant,
                      mutations: participant.mutations.filter(
                        (_, i) => i !== mutationIndex,
                      ),
                    })
                  }
                >
                  {t("delete")}
                </Button>
              </Group>
            );
          })}
          <div className="flex flex-wrap gap-2">
            <Button
              variant="outline"
              size="sm"
              type="button"
              onClick={() =>
                update(index, {
                  ...participant,
                  mutations: [
                    ...participant.mutations,
                    {
                      kind: "thread_title",
                      thread_id: "",
                      expected_revision: 0,
                      title: "",
                    },
                  ],
                })
              }
            >
              {t("transactionAddOperation")}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="hover:text-destructive"
              type="button"
              disabled={
                value.participants.length === 1 ||
                participant.node_id === value.coordinator
              }
              onClick={() =>
                change({
                  ...value,
                  participants: value.participants.filter(
                    (_, i) => i !== index,
                  ),
                })
              }
            >
              {t("delete")}
            </Button>
          </div>
        </Group>
      ))}
      <Button
        variant="outline"
        size="sm"
        className="justify-self-start"
        type="button"
        disabled={deploymentAdministrator || value.participants.length >= 16}
        onClick={() =>
          change({
            ...value,
            participants: [
              ...value.participants,
              {
                node_id:
                  nodes.find(
                    (node) =>
                      !value.participants.some(
                        (participant) => participant.node_id === node,
                      ),
                  ) ?? "",
                mutations: [],
              },
            ],
          })
        }
      >
        {t("transactionAddParticipant")}
      </Button>
    </>
  );
}
function JsonInput({
  value,
  change,
  label,
  objectOnly = false,
}: {
  value: unknown;
  change: (value: unknown) => void;
  label: string;
  objectOnly?: boolean;
}) {
  const [draft, setDraft] = useState(JSON.stringify(value, null, 2));
  const { t } = useI18n();
  const committed = useRef(value);
  const input = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    if (value !== committed.current) {
      committed.current = value;
      input.current?.setCustomValidity("");
      setDraft(JSON.stringify(value, null, 2));
    }
  }, [value]);
  return (
    <Field label={label}>
      <Textarea
        ref={input}
        className="min-h-20 font-mono text-xs"
        required
        value={draft}
        onChange={(event) => {
          setDraft(event.target.value);
          try {
            const parsed: unknown = JSON.parse(event.target.value);
            if (
              objectOnly &&
              (!parsed || typeof parsed !== "object" || Array.isArray(parsed))
            )
              throw new Error("object required");
            event.target.setCustomValidity("");
            committed.current = parsed;
            change(parsed);
          } catch {
            event.target.setCustomValidity(t("jsonHint"));
          }
        }}
      />
    </Field>
  );
}
