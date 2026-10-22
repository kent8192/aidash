import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import { Alert, Check, Group, Hint, pairClass } from "./components/patterns";
import { useQuery } from "@tanstack/react-query";
import { discover, registryMcpToolHint } from "./generated/aidash";
import { ReferenceName } from "./record-view";
import { SkillImport, type SkillPayload } from "./skill-import";
import { useRef, useState } from "react";
import { MemoryRegistryFields } from "./memory-registry";
import { replayAfterHint, replayAfterQueryChange, sameQuery } from "./mcp-hint";
import type { State } from "./types";
import { Field, useEntityLabel, useI18n } from "./ui";

type Argument = {
  key: string;
  name: string;
  type: string;
  description: string;
  enumValues: string;
  pattern: string;
  minimum: string;
  maximum: string;
  required: boolean;
  children: Argument[];
  items: string;
};

function argumentSchema(fields: Argument[]): Record<string, unknown> {
  return {
    type: "object",
    properties: Object.fromEntries(
      fields.map((field) => [
        field.name,
        {
          ...(field.type === "object"
            ? argumentSchema(field.children)
            : {
                type: field.type,
                ...(field.enumValues.trim()
                  ? {
                      enum: field.enumValues
                        .split(",")
                        .map((value) => value.trim())
                        .filter(Boolean)
                        .map((value) => {
                          if (field.type === "boolean") return value === "true";
                          if (field.type === "number") return Number(value);
                          if (field.type === "integer")
                            return Number.parseInt(value, 10);
                          return value;
                        }),
                    }
                  : {}),
                ...(field.type === "string" && field.pattern.trim()
                  ? { pattern: field.pattern.trim() }
                  : {}),
                ...(field.type === "number" || field.type === "integer"
                  ? {
                      ...(field.minimum.trim()
                        ? { minimum: Number(field.minimum) }
                        : {}),
                      ...(field.maximum.trim()
                        ? { maximum: Number(field.maximum) }
                        : {}),
                    }
                  : {}),
                ...(field.type === "array"
                  ? {
                      items:
                        field.items === "object"
                          ? argumentSchema(field.children)
                          : { type: field.items },
                    }
                  : {}),
              }),
          description: field.description,
        },
      ]),
    ),
    required: fields
      .filter((field) => field.required)
      .map((field) => field.name),
    additionalProperties: false,
  };
}

function Arguments({
  fields,
  change,
}: {
  fields: Argument[];
  change: (fields: Argument[]) => void;
}) {
  const { t } = useI18n();
  const update = (index: number, patch: Partial<Argument>) =>
    change(
      fields.map((field, i) => (i === index ? { ...field, ...patch } : field)),
    );
  return (
    <Group nested legend={t("toolArguments")}>
      {fields.map((field, index) => (
        <Group
          nested
          key={field.key}
          legend={`${t("toolArgument")} ${index + 1}`}
        >
          <Field label={t("toolArgumentName")}>
            <Input
              required
              value={field.name}
              ref={(input) => {
                input?.setCustomValidity(
                  fields.some(
                    (other, i) => i !== index && other.name === field.name,
                  )
                    ? t("toolDuplicateArgument")
                    : "",
                );
              }}
              onChange={(event) => update(index, { name: event.target.value })}
            />
          </Field>
          <Field label={t("toolArgumentType")}>
            <NativeSelect
              value={field.type}
              onChange={(event) => update(index, { type: event.target.value })}
            >
              {[
                "string",
                "number",
                "integer",
                "boolean",
                "object",
                "array",
              ].map((type) => (
                <option key={type} value={type}>
                  {t(`argument_${type}`)}
                </option>
              ))}
            </NativeSelect>
          </Field>
          <Field label={t("description")}>
            <Input
              value={field.description}
              onChange={(event) =>
                update(index, { description: event.target.value })
              }
            />
          </Field>
          {["string", "number", "integer", "boolean"].includes(field.type) && (
            <Field label={t("toolArgumentEnum")}>
              <Input
                value={field.enumValues}
                placeholder={t("toolArgumentEnumPlaceholder")}
                onChange={(event) =>
                  update(index, { enumValues: event.target.value })
                }
              />
            </Field>
          )}
          {field.type === "string" && (
            <Field label={t("toolArgumentPattern")}>
              <Input
                value={field.pattern}
                onChange={(event) =>
                  update(index, { pattern: event.target.value })
                }
              />
            </Field>
          )}
          {(field.type === "number" || field.type === "integer") && (
            <div className={pairClass}>
              <Field label={t("toolArgumentMinimum")}>
                <Input
                  type="number"
                  value={field.minimum}
                  onChange={(event) =>
                    update(index, { minimum: event.target.value })
                  }
                />
              </Field>
              <Field label={t("toolArgumentMaximum")}>
                <Input
                  type="number"
                  value={field.maximum}
                  onChange={(event) =>
                    update(index, { maximum: event.target.value })
                  }
                />
              </Field>
            </div>
          )}
          <Check
            checked={field.required}
            onChange={(event) =>
              update(index, { required: event.target.checked })
            }
          >
            {t("toolArgumentRequired")}
          </Check>
          {field.type === "array" && (
            <Field label={t("toolItemType")}>
              <NativeSelect
                value={field.items}
                onChange={(event) =>
                  update(index, { items: event.target.value })
                }
              >
                {["string", "number", "integer", "boolean", "object"].map(
                  (type) => (
                    <option key={type} value={type}>
                      {t(`argument_${type}`)}
                    </option>
                  ),
                )}
              </NativeSelect>
            </Field>
          )}
          {(field.type === "object" ||
            (field.type === "array" && field.items === "object")) && (
            <Arguments
              fields={field.children}
              change={(children) => update(index, { children })}
            />
          )}
          <Button
            variant="ghost"
            size="sm"
            type="button"
            className="justify-self-start"
            onClick={() => change(fields.filter((_, i) => i !== index))}
          >
            {t("toolRemoveArgument")}
          </Button>
        </Group>
      ))}
      <Button
        variant="outline"
        size="sm"
        type="button"
        className="justify-self-start"
        onClick={() =>
          change([
            ...fields,
            {
              key: crypto.randomUUID(),
              name: "",
              type: "string",
              description: "",
              enumValues: "",
              pattern: "",
              minimum: "",
              maximum: "",
              required: true,
              children: [],
              items: "string",
            },
          ])
        }
      >
        {t("toolAddArgument")}
      </Button>
    </Group>
  );
}

/** What an MCP server said about one tool, or why it could not be asked. */
type ServerHint = { transport: string; endpoint: string; toolName: string } & (
  | { readOnly: boolean | null }
  | { error: string }
);

export function EntityConfiguration({
  kind,
  data,
}: {
  kind: string;
  data: State;
}) {
  const { t } = useI18n();
  const entityLabel = useEntityLabel(data.registry);
  const [adapter, setAdapter] = useState("native_memory");
  const [sourceValue, setSourceValue] = useState("{}");
  const [transport, setTransport] = useState("http");
  const [alias, setAlias] = useState("integration_invoke");
  const [members, setMembers] = useState<string[]>([]);
  const [endpoint, setEndpoint] = useState("");
  const [replay, setReplay] = useState("unsafe");
  const [toolName, setToolName] = useState("");
  const [idempotency, setIdempotency] = useState("");
  const [hint, setHint] = useState<ServerHint | null>(null);
  const [checkingHint, setCheckingHint] = useState(false);
  const hintRequest = useRef(0);
  // The response arrives after more edits can happen, so the answer and the
  // automatic prefill it may cause are compared with these, not with the
  // values captured when the request started.
  const transportRef = useRef(transport);
  const endpointRef = useRef(endpoint);
  const toolNameRef = useRef(toolName);
  const replayRef = useRef(replay);
  const replayPrefilled = useRef(false);
  const [instructions, setInstructions] = useState("");
  const [skillFiles, setSkillFiles] = useState<SkillPayload["files"]>([]);
  const [skillSource, setSkillSource] = useState<string>();
  const [agent, setAgent] = useState("");
  const [node, setNode] = useState(data.node.id);
  const [fields, setFields] = useState<Argument[]>([]);
  const agents = data.registry.filter((entry) => entry.kind === "agent");
  const selectedAgent = agents.find(
    (entry) => `${entry.id}@${entry.version}` === agent,
  );
  const reference = selectedAgent
    ? { id: selectedAgent.id, version: selectedAgent.version }
    : { id: "", version: "1.0.0" };
  const [remoteId, setRemoteId] = useState("");
  const [remoteVersion, setRemoteVersion] = useState("1.0.0");
  const [manualRemote, setManualRemote] = useState(false);
  const discovery = useQuery({
    queryKey: ["discovery", "tool-picker"],
    queryFn: () => discover({}),
    enabled: kind === "tool" && transport === "agent" && node !== data.node.id,
    retry: false,
  });
  const remoteAgents = discovery.isError
    ? []
    : (discovery.data?.agents.filter((agent) => agent.node_id === node) ?? []);
  const peerError = discovery.data?.errors.find(
    (error) => error.node_id === node,
  );
  const remoteError = discovery.isError
    ? discovery.error.message
    : peerError?.error;
  const remoteLabel = useEntityLabel(remoteAgents.map((agent) => agent.entity));

  // The server's word only prefills the choice; it applies to the endpoint and
  // tool it was read for, and to nothing the person edits afterwards.
  const shownHint =
    hint && sameQuery(hint, transport, endpoint, toolName) ? hint : null;
  // A queried field changed: the pending answer is for another tool, and an
  // automatic read-only prefill no longer describes this one.
  const queryChanged = () => {
    hintRequest.current += 1;
    setCheckingHint(false);
    const next = replayAfterQueryChange(
      replayRef.current,
      replayPrefilled.current,
    );
    replayPrefilled.current = false;
    if (next !== replayRef.current) {
      replayRef.current = next;
      setReplay(next);
    }
  };
  const checkHint = async () => {
    const current = ++hintRequest.current;
    const asked = {
      transport: transportRef.current,
      endpoint: endpointRef.current,
      toolName: toolNameRef.current,
    };
    setCheckingHint(true);
    setHint(null);
    try {
      const { read_only_hint } = await registryMcpToolHint({
        endpoint: asked.endpoint,
        tool_name: asked.toolName,
      });
      if (current !== hintRequest.current) return;
      if (
        !sameQuery(
          asked,
          transportRef.current,
          endpointRef.current,
          toolNameRef.current,
        )
      )
        return;
      setHint({ ...asked, readOnly: read_only_hint ?? null });
      const next = replayAfterHint(
        replayRef.current,
        read_only_hint,
        replayPrefilled.current,
      );
      replayPrefilled.current = next.prefilled;
      if (next.replay !== replayRef.current) {
        replayRef.current = next.replay;
        setReplay(next.replay);
      }
    } catch (cause) {
      if (current !== hintRequest.current) return;
      if (
        !sameQuery(
          asked,
          transportRef.current,
          endpointRef.current,
          toolNameRef.current,
        )
      )
        return;
      setHint({
        ...asked,
        error: cause instanceof Error ? cause.message : String(cause),
      });
    } finally {
      if (current === hintRequest.current) setCheckingHint(false);
    }
  };

  let settings: Record<string, unknown> = {};
  try {
    const parsed: unknown = JSON.parse(sourceValue || "{}");
    // The input reports a non-object before submitting; keep the form usable.
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed))
      settings = parsed as Record<string, unknown>;
  } catch {
    /* The input reports invalid JSON before submitting. */
  }
  const sourceAdapter = (kind === "memory"
    ? ["native_memory", "conversation_memory", "semantic_memory"]
    : [
        "native_memory",
        "workspace_retrieval",
        "reference_attachments",
        "skill_attachments",
        "skill_roots",
      ]
  ).includes(adapter)
    ? adapter
    : kind === "memory"
      ? "conversation_memory"
      : "workspace_retrieval";
  const transportConfig =
    transport === "agent"
      ? {
          transport,
          node_id: node,
          agent:
            node === data.node.id
              ? reference
              : { id: remoteId, version: remoteVersion },
        }
      : {
          transport,
          endpoint,
          replay,
          ...(transport === "mcp"
            ? {
                tool_name: toolName,
                idempotency_argument:
                  replay === "idempotent" ? idempotency : null,
              }
            : {}),
        };
  const config =
    kind === "memory" || kind === "source"
      ? { schema_version: 1, source: { ...settings, adapter: sourceAdapter } }
      : kind === "bundle"
        ? {
            members: members.map((key) => {
              const entry = data.registry.find(
                (e) => `${e.id}@${e.version}` === key,
              )!;
              return {
                registry_node: data.node.id,
                id: entry.id,
                version: entry.version,
              };
            }),
          }
        : kind === "skill"
          ? {
              instructions,
              files: skillFiles,
              ...(skillSource ? { source: skillSource } : {}),
            }
          : kind === "cluster"
            ? { coordinator: reference }
            : kind === "tool"
              ? {
                  registry_node: data.node.id,
                  provider: `integration.${transport}@1`,
                  operation: "invoke",
                  default_alias: alias,
                  tier: "integration",
                  narrow: {},
                  transport: transportConfig,
                }
              : {};
  const agentSelect = (
    <Field label={t(kind === "cluster" ? "clusterCoordinator" : "toolAgent")}>
      <NativeSelect
        required
        value={agent}
        onChange={(event) => setAgent(event.target.value)}
      >
        <option value="">{t("choose")}</option>
        {agents.map((entry) => (
          <option
            key={`${entry.id}@${entry.version}`}
            value={`${entry.id}@${entry.version}`}
          >
            {entityLabel(entry)}
          </option>
        ))}
      </NativeSelect>
    </Field>
  );
  return (
    <>
      <input type="hidden" name="config" value={JSON.stringify(config)} />
      {(kind === "memory" || kind === "source") && (
        <>
          <Field label="Context source">
            <NativeSelect
              name="context_adapter"
              value={sourceAdapter}
              onChange={(e) => {
                setAdapter(e.target.value);
                setSourceValue("{}");
              }}
            >
              {(kind === "memory"
                ? ["native_memory", "conversation_memory", "semantic_memory"]
                : [
                    "native_memory",
                    "workspace_retrieval",
                    "reference_attachments",
                    "skill_attachments",
                    "skill_roots",
                  ]
              ).map((a) => (
                <option key={a} value={a}>
                  {a === "native_memory" ? "Native memory" : a}
                </option>
              ))}
            </NativeSelect>
          </Field>
          {sourceAdapter === "workspace_retrieval" && (
            <>
              <Field label={t("contextOnUnavailable")}>
                <NativeSelect
                  name="context_on_unavailable"
                  value={
                    settings.on_unavailable === "degrade" ? "degrade" : "pause"
                  }
                  onChange={(e) => {
                    const next = { ...settings };
                    // Pause is the default and is never written.
                    if (e.target.value === "degrade")
                      next.on_unavailable = "degrade";
                    else delete next.on_unavailable;
                    setSourceValue(JSON.stringify(next));
                  }}
                >
                  <option value="pause">
                    {t("contextOnUnavailablePause")}
                  </option>
                  <option value="degrade">
                    {t("contextOnUnavailableDegrade")}
                  </option>
                </NativeSelect>
              </Field>
              <Hint>{t("contextOnUnavailableHint")}</Hint>
            </>
          )}
          {sourceAdapter === "native_memory" ? (
            <MemoryRegistryFields
              key={kind}
              kind={kind}
              entries={data.registry}
            />
          ) : (
            <Field label="Source settings (JSON)">
              <Textarea
                key={sourceAdapter}
                value={sourceValue}
                onChange={(e) => {
                  setSourceValue(e.target.value);
                  try {
                    const value = JSON.parse(e.target.value);
                    e.target.setCustomValidity(
                      value &&
                        !Array.isArray(value) &&
                        typeof value === "object"
                        ? ""
                        : "Enter a JSON object",
                    );
                  } catch {
                    e.target.setCustomValidity("Enter valid JSON");
                  }
                }}
              />
            </Field>
          )}
        </>
      )}
      {kind === "skill" && (
        <>
          <SkillImport
            change={(payload) => {
              setInstructions(payload.instructions);
              setSkillFiles(payload.files);
              setSkillSource(payload.source);
            }}
          />
          <Field label={t("instructions")}>
            <Textarea
              required
              rows={6}
              value={instructions}
              onChange={(event) => setInstructions(event.target.value)}
            />
          </Field>
        </>
      )}
      {kind === "bundle" && (
        <Group legend="Bundle members">
          {data.registry
            .filter((e) => e.kind === "tool" || e.kind === "bundle")
            .map((entry) => {
              const key = `${entry.id}@${entry.version}`;
              return (
                <Check
                  key={key}
                  checked={members.includes(key)}
                  disabled={members.some(
                    (member) =>
                      member !== key && member.startsWith(`${entry.id}@`),
                  )}
                  onChange={(e) =>
                    setMembers(
                      e.target.checked
                        ? [...members, key]
                        : members.filter((m) => m !== key),
                    )
                  }
                >
                  {entityLabel(entry)} · {entry.version}
                </Check>
              );
            })}
        </Group>
      )}
      {kind === "cluster" && agentSelect}
      {kind === "node" && <Hint>{t("nodeNoConfiguration")}</Hint>}
      {kind === "tool" && (
        <>
          <Field label={t("toolTransport")}>
            <NativeSelect
              value={transport}
              onChange={(event) => {
                transportRef.current = event.target.value;
                queryChanged();
                setTransport(event.target.value);
              }}
            >
              {["http", "mcp", "agent"].map((value) => (
                <option key={value} value={value}>
                  {t(`toolTransport_${value}`)}
                </option>
              ))}
            </NativeSelect>
          </Field>
          <Field label="Stable alias">
            <Input
              className="font-mono text-xs"
              required
              value={alias}
              pattern="[A-Za-z0-9_-]{1,64}"
              onChange={(e) => setAlias(e.target.value)}
            />
          </Field>
          {(transport === "http" || transport === "mcp") && (
            <>
              <Field label={t("endpoint")}>
                <Input
                  type="url"
                  required
                  value={endpoint}
                  onChange={(event) => {
                    endpointRef.current = event.target.value;
                    queryChanged();
                    setEndpoint(event.target.value);
                  }}
                />
              </Field>
              {transport === "mcp" && (
                <Field label={t("toolRemoteName")}>
                  <Input
                    required
                    value={toolName}
                    onChange={(event) => {
                      toolNameRef.current = event.target.value;
                      queryChanged();
                      setToolName(event.target.value);
                    }}
                  />
                </Field>
              )}
              <Field label={t("toolReplay")}>
                <NativeSelect
                  value={replay}
                  onChange={(event) => {
                    // A choice made now outranks a hint still in flight.
                    hintRequest.current += 1;
                    setCheckingHint(false);
                    replayPrefilled.current = false;
                    replayRef.current = event.target.value;
                    setReplay(event.target.value);
                  }}
                >
                  {["read_only", "unsafe"].map((value) => (
                    <option key={value} value={value}>
                      {t(`toolReplay_${value}`)}
                    </option>
                  ))}
                </NativeSelect>
              </Field>
              <Hint>{t("toolReplayHelp")}</Hint>
              {transport === "mcp" && (
                <div className="grid min-w-0 gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    type="button"
                    className="justify-self-start"
                    disabled={!endpoint || !toolName || checkingHint}
                    onClick={() => void checkHint()}
                  >
                    {t("toolHintCheck")}
                  </Button>
                  {shownHint &&
                    ("error" in shownHint ? (
                      <Alert>{shownHint.error}</Alert>
                    ) : (
                      <Hint>
                        {t(
                          shownHint.readOnly === true
                            ? "toolHintReadOnly"
                            : shownHint.readOnly === false
                              ? "toolHintWrites"
                              : "toolHintNone",
                        )}
                      </Hint>
                    ))}
                </div>
              )}
              {transport === "mcp" && replay === "idempotent" && (
                <Field label={t("toolIdempotency")}>
                  <Input
                    required
                    value={idempotency}
                    onChange={(event) => setIdempotency(event.target.value)}
                  />
                </Field>
              )}
            </>
          )}
          {transport === "agent" && (
            <>
              <Field label={t("node")}>
                <NativeSelect
                  value={node}
                  onChange={(event) => {
                    setNode(event.target.value);
                    setRemoteId("");
                    setRemoteVersion("1.0.0");
                    setManualRemote(false);
                  }}
                >
                  <option value={data.node.id}>
                    <ReferenceName id={data.node.id} />
                  </option>
                  {data.peers
                    .filter(
                      (peer) => peer.enabled && peer.node_id !== data.node.id,
                    )
                    .map((peer) => (
                      <option key={peer.node_id} value={peer.node_id}>
                        <ReferenceName id={peer.node_id} />
                      </option>
                    ))}
                </NativeSelect>
              </Field>
              {node === data.node.id ? (
                agentSelect
              ) : (
                <>
                  {remoteError && (
                    <Alert retry={() => void discovery.refetch()}>
                      {remoteError}
                    </Alert>
                  )}
                  {(manualRemote ||
                    remoteError ||
                    (!discovery.isPending && remoteAgents.length === 0)) && (
                    <Button
                      variant="outline"
                      size="sm"
                      type="button"
                      className="justify-self-start"
                      onClick={() => {
                        setManualRemote(!manualRemote);
                        setRemoteId("");
                        setRemoteVersion("1.0.0");
                      }}
                    >
                      {t(
                        manualRemote
                          ? "toolChooseRemoteAgent"
                          : "toolManualRemoteAgent",
                      )}
                    </Button>
                  )}
                  {manualRemote ? (
                    <>
                      <Field label={t("toolRemoteAgentReference")}>
                        <Input
                          required
                          pattern="[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}"
                          value={remoteId}
                          onChange={(event) => setRemoteId(event.target.value)}
                        />
                      </Field>
                      <Field label={t("toolRemoteAgentVersion")}>
                        <Input
                          required
                          value={remoteVersion}
                          onChange={(event) =>
                            setRemoteVersion(event.target.value)
                          }
                        />
                      </Field>
                    </>
                  ) : (
                    <Field label={t("toolRemoteAgentId")}>
                      <NativeSelect
                        required
                        value={
                          remoteAgents.some(
                            ({ entity }) =>
                              entity.id === remoteId &&
                              entity.version === remoteVersion,
                          )
                            ? JSON.stringify([remoteId, remoteVersion])
                            : ""
                        }
                        onChange={(event) => {
                          const [id, version] = event.target.value
                            ? (JSON.parse(event.target.value) as [
                                string,
                                string,
                              ])
                            : ["", "1.0.0"];
                          setRemoteId(id);
                          setRemoteVersion(version);
                        }}
                      >
                        <option value="">{t("choose")}</option>
                        {remoteAgents.map(({ entity }) => (
                          <option
                            key={`${entity.id}@${entity.version}`}
                            value={JSON.stringify([entity.id, entity.version])}
                          >
                            {remoteLabel(entity)}
                          </option>
                        ))}
                      </NativeSelect>
                    </Field>
                  )}
                </>
              )}
            </>
          )}
          {transport === "agent" ? (
            <>
              <Hint>{t("toolTaskArguments")}</Hint>
              <input
                type="hidden"
                name="schema"
                value={JSON.stringify({
                  type: "object",
                  properties: {
                    title: { type: "string" },
                    description: { type: "string" },
                    requirements: {
                      type: "object",
                      additionalProperties: true,
                    },
                    dependencies: {
                      type: "array",
                      items: { type: "string", format: "uuid" },
                    },
                    parent_id: { type: ["string", "null"], format: "uuid" },
                  },
                  required: ["title", "description"],
                  additionalProperties: false,
                })}
              />
            </>
          ) : (
            <>
              <Arguments fields={fields} change={setFields} />
              <input
                type="hidden"
                name="schema"
                value={JSON.stringify(
                  fields.length ? argumentSchema(fields) : { type: "object" },
                )}
              />
            </>
          )}
        </>
      )}
    </>
  );
}
