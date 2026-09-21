import { useState } from "react";
import type { State } from "./types";
import { Field, useI18n } from "./ui";

type Argument = {
  key: string;
  name: string;
  type: string;
  description: string;
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
  const [transport, setTransport] = useState("native");
  const [operation, setOperation] = useState("echo");
  const [endpoint, setEndpoint] = useState("");
  const [authenticated, setAuthenticated] = useState(false);
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
                  credential_env: authenticated ? "AIDASH_SECRET_TOOL" : null,
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
            {entry.name.en || entry.id} · {entry.version}
          </option>
        ))}
      </select>
    </Field>
  );
  return (
    <>
      <input type="hidden" name="config" value={JSON.stringify(config)} />
      {kind === "skill" && (
        <Field label={t("instructions")}>
          <textarea
            required
            rows={6}
            value={instructions}
            onChange={(event) => setInstructions(event.target.value)}
          />
        </Field>
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
                  onChange={(event) => setNode(event.target.value)}
                >
                  <option value={data.node.id}>{data.node.id}</option>
                  {data.peers
                    .filter(
                      (peer) => peer.enabled && peer.node_id !== data.node.id,
                    )
                    .map((peer) => (
                      <option key={peer.node_id} value={peer.node_id}>
                        {peer.node_id}
                      </option>
                    ))}
                </select>
              </Field>
              {node === data.node.id ? (
                agentSelect
              ) : (
                <>
                  <Field label={t("toolRemoteAgentId")}>
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
                      onChange={(event) => setRemoteVersion(event.target.value)}
                    />
                  </Field>
                </>
              )}
            </>
          )}
          {transport === "native" && operation === "http_get" ? (
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
