import { useContext, useState, useRef, useEffect } from "react";
import type {
  TransactionManifest,
  TransactionMutation,
  TransactionParticipant,
} from "./generated/models";
import { DisplayState, ReferenceName } from "./record-view";
import { Field, useEntityLabel, useI18n } from "./ui";

export function TransactionComposer({
  value,
  change,
}: {
  value: TransactionManifest;
  change: (value: TransactionManifest) => void;
}) {
  const data = useContext(DisplayState);
  const { t } = useI18n();
  const entityLabel = useEntityLabel(data?.registry ?? []);
  const update = (index: number, participant: TransactionParticipant) =>
    change({
      ...value,
      participants: value.participants.map((old, i) =>
        i === index ? participant : old,
      ),
    });
  const nodes = [
    ...new Set([
      value.coordinator,
      ...(data?.peers
        .filter((peer) => peer.enabled)
        .map((peer) => peer.node_id) ?? []),
    ]),
  ];
  return (
    <>
      <Field label={t("transactionDeadline")}>
        <input
          type="datetime-local"
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
        <fieldset key={index}>
          <legend>
            {t("transactionParticipant")} {index + 1}
          </legend>
          <Field label={t("node")}>
            <select
              required
              value={participant.node_id}
              onChange={(event) =>
                update(index, { ...participant, node_id: event.target.value })
              }
            >
              <option value="">{t("choose")}</option>
              {nodes
                .filter(
                  (node) =>
                    node === participant.node_id ||
                    !value.participants.some((other) => other.node_id === node),
                )
                .map((node) => (
                  <option key={node} value={node}>
                    <ReferenceName id={node} />
                  </option>
                ))}
            </select>
          </Field>
          {participant.mutations.map((mutation, mutationIndex) => {
            const setMutation = (next: TransactionMutation) =>
              update(index, {
                ...participant,
                mutations: participant.mutations.map((old, i) =>
                  i === mutationIndex ? next : old,
                ),
              });
            return (
              <fieldset key={mutationIndex}>
                <legend>
                  {t("transactionOperation")} {mutationIndex + 1}
                </legend>
                <Field label={t("transactionOperation")}>
                  <select
                    value={mutation.kind}
                    onChange={(event) => {
                      const kind = event.target.value;
                      if (kind === "workspace_state")
                        setMutation({
                          kind,
                          workspace_id: "",
                          expected_revision: 0,
                          state: {},
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
                      else if (kind === "registry_register")
                        setMutation({
                          kind,
                          entry: {
                            id: crypto.randomUUID(),
                            version: "1.0.0",
                            kind: "skill",
                            name: { en: "" },
                            description: {},
                            config: {},
                            schema: {},
                            skills: [],
                            capabilities: [],
                            tags: [],
                            languages: [],
                          },
                        });
                    }}
                  >
                    {[
                      "workspace_state",
                      "complete_task",
                      "finish_run",
                      "registry_register",
                    ].map((kind) => (
                      <option key={kind} value={kind}>
                        {t(`transactionOperation_${kind}`)}
                      </option>
                    ))}
                  </select>
                </Field>
                {mutation.kind === "workspace_state" && (
                  <>
                    <Field label={t("workspace")}>
                      <select
                        required
                        value={mutation.workspace_id}
                        onChange={(event) =>
                          setMutation({
                            ...mutation,
                            workspace_id: event.target.value,
                            expected_revision:
                              data?.workspaces.find(
                                (workspace) =>
                                  workspace.id === event.target.value,
                              )?.revision ?? 0,
                          })
                        }
                      >
                        <option value="">{t("choose")}</option>
                        {data?.workspaces.map((workspace) => (
                          <option key={workspace.id} value={workspace.id}>
                            {workspace.title}
                          </option>
                        ))}
                      </select>
                    </Field>
                    <JsonInput
                      objectOnly
                      label={t("transactionState")}
                      value={mutation.state}
                      change={(state) => setMutation({ ...mutation, state })}
                    />
                  </>
                )}
                {mutation.kind === "complete_task" && (
                  <>
                    <Field label={t("task")}>
                      <select
                        required
                        value={mutation.task_id}
                        onChange={(event) =>
                          setMutation({
                            ...mutation,
                            task_id: event.target.value,
                            expected_revision:
                              data?.tasks.find(
                                (task) => task.id === event.target.value,
                              )?.revision ?? 0,
                          })
                        }
                      >
                        <option value="">{t("choose")}</option>
                        {data?.tasks.map((task) => (
                          <option key={task.id} value={task.id}>
                            {task.title}
                          </option>
                        ))}
                      </select>
                    </Field>
                    <Field label={t("name")}>
                      <input
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
                      <input
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
                  <Field label={t("execution")}>
                    <select
                      required
                      value={mutation.run_id}
                      onChange={(event) => {
                        const run = data?.runs.find(
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
                      {data?.runs.map((run) => (
                        <option key={run.id} value={run.id}>
                          <ReferenceName id={run.id} />
                        </option>
                      ))}
                    </select>
                  </Field>
                )}
                {mutation.kind === "registry_register" && (
                  <>
                    <Field label={t("packageEntity")}>
                      <select
                        required
                        value={
                          data?.registry.some(
                            (entry) =>
                              entry.id === mutation.entry.id &&
                              entry.version === mutation.entry.version,
                          )
                            ? `${mutation.entry.id}@${mutation.entry.version}`
                            : ""
                        }
                        onChange={(event) => {
                          const entry = data?.registry.find(
                            (entry) =>
                              `${entry.id}@${entry.version}` ===
                              event.target.value,
                          );
                          if (entry) setMutation({ ...mutation, entry });
                        }}
                      >
                        <option value="">{t("choose")}</option>
                        {data?.registry.map((entry) => (
                          <option
                            key={`${entry.id}@${entry.version}`}
                            value={`${entry.id}@${entry.version}`}
                          >
                            {entityLabel(entry)}
                          </option>
                        ))}
                      </select>
                    </Field>
                    <p className="muted">{t("transactionRegistryHelp")}</p>
                  </>
                )}
                {mutation.kind !== "registry_register" && (
                  <Field label={t("revision")}>
                    <input
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
                )}
                <button
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
                </button>
              </fieldset>
            );
          })}
          <button
            type="button"
            onClick={() =>
              update(index, {
                ...participant,
                mutations: [
                  ...participant.mutations,
                  {
                    kind: "workspace_state",
                    workspace_id: "",
                    expected_revision: 0,
                    state: {},
                  },
                ],
              })
            }
          >
            {t("transactionAddOperation")}
          </button>
          <button
            type="button"
            disabled={value.participants.length === 1}
            onClick={() =>
              change({
                ...value,
                participants: value.participants.filter((_, i) => i !== index),
              })
            }
          >
            {t("delete")}
          </button>
        </fieldset>
      ))}
      <button
        type="button"
        disabled={value.participants.length >= Math.min(nodes.length, 16)}
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
      </button>
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
      <textarea
        ref={input}
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
