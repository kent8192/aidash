import { useQuery } from "@tanstack/react-query";
import { discover } from "./generated/aidash";
import { ReferenceName } from "./record-view";
import { SkillImport } from "./skill-import";
import { useState } from "react";
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
    <fieldset className="argument-fields">
      <legend>{t("toolArguments")}</legend>
      {fields.map((field, index) => (
        <fieldset key={field.key}>
          <legend>
            {t("toolArgument")} {index + 1}
          </legend>
          <Field label={t("toolArgumentName")}>
            <input
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
            <select
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
            </select>
          </Field>
          <Field label={t("description")}>
            <input
              value={field.description}
              onChange={(event) =>
                update(index, { description: event.target.value })
              }
            />
          </Field>
          {["string", "number", "integer", "boolean"].includes(field.type) && (
            <Field label={t("toolArgumentEnum")}>
              <input
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
              <input
                value={field.pattern}
                onChange={(event) =>
                  update(index, { pattern: event.target.value })
                }
              />
            </Field>
          )}
          {(field.type === "number" || field.type === "integer") && (
            <div className="two-columns">
              <Field label={t("toolArgumentMinimum")}>
                <input
                  type="number"
                  value={field.minimum}
                  onChange={(event) =>
                    update(index, { minimum: event.target.value })
                  }
                />
              </Field>
              <Field label={t("toolArgumentMaximum")}>
                <input
                  type="number"
                  value={field.maximum}
                  onChange={(event) =>
                    update(index, { maximum: event.target.value })
                  }
                />
              </Field>
            </div>
          )}
          <label className="check">
            <input
              type="checkbox"
              checked={field.required}
              onChange={(event) =>
                update(index, { required: event.target.checked })
              }
            />
            {t("toolArgumentRequired")}
          </label>
          {field.type === "array" && (
            <Field label={t("toolItemType")}>
              <select
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
              </select>
            </Field>
          )}
          {(field.type === "object" ||
            (field.type === "array" && field.items === "object")) && (
            <Arguments
              fields={field.children}
              change={(children) => update(index, { children })}
            />
          )}
          <button
            type="button"
            onClick={() => change(fields.filter((_, i) => i !== index))}
          >
            {t("toolRemoveArgument")}
          </button>
        </fieldset>
      ))}
      <button
        type="button"
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
      </button>
    </fieldset>
  );
}

export function EntityConfiguration({
  kind,
  data,
}: {
  kind: string;
  data: State;
}) {
  const { t } = useI18n();
  const entityLabel = useEntityLabel(data.registry);
  const [transport, setTransport] = useState("native");
  const [operation, setOperation] = useState("echo");
  const [endpoint, setEndpoint] = useState("");
  const [authenticated, setAuthenticated] = useState(false);
  const [credentialEnv, setCredentialEnv] = useState("AIDASH_SECRET_TOOL");
  const [replay, setReplay] = useState("unsafe");
  const [toolName, setToolName] = useState("");
  const [idempotency, setIdempotency] = useState("");
  const [hosts, setHosts] = useState("");
  const [instructions, setInstructions] = useState("");
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

  const config =
    kind === "skill"
      ? { instructions }
      : kind === "cluster"
        ? { coordinator: reference }
        : kind === "tool"
          ? transport === "native"
            ? {
                transport,
                operation,
                allowed_hosts:
                  operation === "http_get"
                    ? hosts
                        .split(",")
                        .map((host) => host.trim())
                        .filter(Boolean)
                    : [],
              }
            : transport === "agent"
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
                  credential_env: authenticated ? credentialEnv : null,
                  replay,
                  ...(transport === "mcp"
                    ? {
                        tool_name: toolName,
                        idempotency_argument:
                          replay === "idempotent" ? idempotency : null,
                      }
                    : {}),
                }
          : {};
  const agentSelect = (
    <Field label={t(kind === "cluster" ? "clusterCoordinator" : "toolAgent")}>
      <select
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
      </select>
    </Field>
  );
  return (
    <>
      <input type="hidden" name="config" value={JSON.stringify(config)} />
      {kind === "skill" && (
        <>
          <SkillImport change={setInstructions} />
          <Field label={t("instructions")}>
            <textarea
              required
              rows={6}
              value={instructions}
              onChange={(event) => setInstructions(event.target.value)}
            />
          </Field>
        </>
      )}
      {kind === "cluster" && agentSelect}
      {kind === "node" && <p className="muted">{t("nodeNoConfiguration")}</p>}
      {kind === "tool" && (
        <>
          <Field label={t("toolTransport")}>
            <select
              value={transport}
              onChange={(event) => setTransport(event.target.value)}
            >
              {["native", "http", "mcp", "agent"].map((value) => (
                <option key={value} value={value}>
                  {t(`toolTransport_${value}`)}
                </option>
              ))}
            </select>
          </Field>
          {transport === "native" && (
            <>
              <Field label={t("toolOperation")}>
                <select
                  value={operation}
                  onChange={(event) => setOperation(event.target.value)}
                >
                  <option value="echo">{t("toolEcho")}</option>
                  <option value="http_get">{t("toolHttpGet")}</option>
                </select>
              </Field>
              {operation === "http_get" && (
                <Field label={t("toolAllowedHosts")}>
                  <input
                    required
                    value={hosts}
                    placeholder="example.com, docs.example.com"
                    onChange={(event) => setHosts(event.target.value)}
                  />
                </Field>
              )}
            </>
          )}
          {(transport === "http" || transport === "mcp") && (
            <>
              <Field label={t("endpoint")}>
                <input
                  type="url"
                  required
                  value={endpoint}
                  onChange={(event) => setEndpoint(event.target.value)}
                />
              </Field>
              <label className="check">
                <input
                  type="checkbox"
                  checked={authenticated}
                  onChange={(event) => setAuthenticated(event.target.checked)}
                />
                {t("configuredCredentials")}
              </label>
              {authenticated && (
                <Field label={t("credentials")}>
                  <input
                    required
                    value={credentialEnv}
                    onChange={(event) => setCredentialEnv(event.target.value)}
                  />
                </Field>
              )}
              {transport === "mcp" && (
                <Field label={t("toolRemoteName")}>
                  <input
                    required
                    value={toolName}
                    onChange={(event) => setToolName(event.target.value)}
                  />
                </Field>
              )}
              <Field label={t("toolReplay")}>
                <select
                  value={replay}
                  onChange={(event) => setReplay(event.target.value)}
                >
                  {["unsafe", "read_only", "idempotent"].map((value) => (
                    <option key={value} value={value}>
                      {t(`toolReplay_${value}`)}
                    </option>
                  ))}
                </select>
              </Field>
              {transport === "mcp" && replay === "idempotent" && (
                <Field label={t("toolIdempotency")}>
                  <input
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
                <select
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
                </select>
              </Field>
              {node === data.node.id ? (
                agentSelect
              ) : (
                <>
                  {remoteError && <p role="alert">{remoteError}</p>}
                  {remoteError && (
                    <button
                      type="button"
                      onClick={() => void discovery.refetch()}
                    >
                      {t("retry")}
                    </button>
                  )}
                  {(manualRemote ||
                    remoteError ||
                    (!discovery.isPending && remoteAgents.length === 0)) && (
                    <button
                      type="button"
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
                    </button>
                  )}
                  {manualRemote ? (
                    <>
                      <Field label={t("toolRemoteAgentReference")}>
                        <input
                          required
                          pattern="[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}"
                          value={remoteId}
                          onChange={(event) => setRemoteId(event.target.value)}
                        />
                      </Field>
                      <Field label={t("toolRemoteAgentVersion")}>
                        <input
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
                      <select
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
                      </select>
                    </Field>
                  )}
                </>
              )}
            </>
          )}
          {transport === "agent" ? (
            <>
              <p className="muted">{t("toolTaskArguments")}</p>
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
          ) : transport === "native" && operation === "http_get" ? (
            <>
              <p className="muted">{t("toolUrlArgument")}</p>
              <input
                type="hidden"
                name="schema"
                value={JSON.stringify({
                  type: "object",
                  properties: { url: { type: "string", format: "uri" } },
                  required: ["url"],
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
