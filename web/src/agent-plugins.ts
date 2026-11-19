/**
 * The Agent Plugins an Agent definition takes in (ADR 0072), as the server
 * stores them in `config.agent_plugins`. These types follow the serde of the
 * Rust `AgentPluginRef`. Fields the server fills in when absent are optional
 * here, because a definition registered through the API may omit them.
 */
export const MAX_AGENT_PLUGINS = 8;

const BARE_COMMANDS = ["node", "npm", "npx", "python3", "uv", "uvx"] as const;
export type BareCommand = (typeof BARE_COMMANDS)[number];
export type StdioCommand = { bare: BareCommand } | { plugin_relative: string };
export type StdioCwd = "plugin_root" | { plugin: string } | { data: string };
export type McpServer =
  | {
      stdio: {
        command: StdioCommand;
        args?: string[];
        env?: Record<string, string>;
        cwd?: StdioCwd;
      };
    }
  | { remote: { url: string; headers?: { name: string; value: string }[] } };

export type FindingComponent =
  | "manifest"
  | "mcp"
  | { skill: string }
  | { server: string };
export type FindingOutcome = "ignored" | "skipped";
export type Finding = {
  component: FindingComponent;
  outcome: FindingOutcome;
  reason: string;
  detail: string;
};

export type AgentPluginRef = {
  digest: string;
  name: string;
  source: string;
  version?: string | null;
  license?: string | null;
  skills: string[];
  servers: Record<string, McpServer>;
  findings: Finding[];
};

/** `reference` goes into the definition. `denied` is shown at import and never stored. */
export type ImportedAgentPlugin = {
  reference: AgentPluginRef;
  denied: string[];
};
export type AgentPluginImportResult = {
  plugins: string[];
  selected: ImportedAgentPlugin | null;
};

type Config = Record<string, unknown>;

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const isString = (value: unknown): value is string => typeof value === "string";
const isStrings = (value: unknown): value is string[] =>
  Array.isArray(value) && value.every(isString);
const isOptionalString = (value: unknown) => value == null || isString(value);

/** Whether `value` holds exactly one key out of `keys`, with a string under it. */
function taggedString(value: unknown, keys: readonly string[]): boolean {
  if (!isRecord(value)) return false;
  const present = Object.keys(value);
  return (
    present.length === 1 &&
    keys.includes(present[0]) &&
    isString(value[present[0]])
  );
}

function isCommand(value: unknown): boolean {
  if (!isRecord(value)) return false;
  return "bare" in value
    ? taggedString(value, ["bare"]) &&
        (BARE_COMMANDS as readonly unknown[]).includes(value.bare)
    : taggedString(value, ["plugin_relative"]);
}

function isCwd(value: unknown): boolean {
  return value === "plugin_root" || taggedString(value, ["plugin", "data"]);
}

function isServer(value: unknown): value is McpServer {
  if (!isRecord(value)) return false;
  if (isRecord(value.stdio)) {
    const { command, args, env, cwd } = value.stdio;
    return (
      isCommand(command) &&
      (args === undefined || isStrings(args)) &&
      (env === undefined ||
        (isRecord(env) && Object.values(env).every(isString))) &&
      (cwd === undefined || isCwd(cwd))
    );
  }
  if (isRecord(value.remote)) {
    const { url, headers } = value.remote;
    return (
      isString(url) &&
      (headers === undefined ||
        (Array.isArray(headers) &&
          headers.every(
            (header) =>
              isRecord(header) &&
              isString(header.name) &&
              isString(header.value),
          )))
    );
  }
  return false;
}

function isFinding(value: unknown): value is Finding {
  if (!isRecord(value)) return false;
  const { component, outcome, reason, detail } = value;
  return (
    (component === "manifest" ||
      component === "mcp" ||
      taggedString(component, ["skill", "server"])) &&
    (outcome === "ignored" || outcome === "skipped") &&
    isString(reason) &&
    isString(detail)
  );
}

/**
 * Whether `value` has the shape of an `AgentPluginRef`. It checks what the
 * summary reads and ignores extra keys, so a reference passes through to the
 * definition exactly as the server wrote it.
 */
export function isAgentPluginRef(value: unknown): value is AgentPluginRef {
  if (!isRecord(value)) return false;
  return (
    isString(value.digest) &&
    isString(value.name) &&
    isString(value.source) &&
    isOptionalString(value.version) &&
    isOptionalString(value.license) &&
    isStrings(value.skills) &&
    isRecord(value.servers) &&
    Object.values(value.servers).every(isServer) &&
    Array.isArray(value.findings) &&
    value.findings.every(isFinding)
  );
}

/**
 * The answer of `POST /api/agent-plugins/import`, or `null` when it is not
 * one. The reference is kept exactly as sent, because it goes into the
 * definition unchanged.
 */
export function readImport(value: unknown): AgentPluginImportResult | null {
  if (!isRecord(value) || !isStrings(value.plugins)) return null;
  const { plugins, selected } = value;
  if (selected == null) return { plugins, selected: null };
  if (
    !isRecord(selected) ||
    !isStrings(selected.denied) ||
    !isAgentPluginRef(selected.reference)
  )
    return null;
  return {
    plugins,
    selected: { reference: selected.reference, denied: selected.denied },
  };
}

/**
 * The plugins of an Agent definition, or `null` for any other kind, a
 * definition without plugins, or malformed plugin data.
 *
 * Approving an Agent version admits the servers its plugins declare.
 */
export function agentPlugins(entry: {
  kind: string;
  config: unknown;
}): AgentPluginRef[] | null {
  if (entry.kind !== "agent" || !isRecord(entry.config)) return null;
  const plugins = entry.config.agent_plugins;
  if (!Array.isArray(plugins) || plugins.length === 0) return null;
  return plugins.every(isAgentPluginRef) ? plugins : null;
}

export function hasPluginSkills(plugins: readonly AgentPluginRef[]): boolean {
  return plugins.some((plugin) => plugin.skills.length > 0);
}

export type ServerDescription =
  | {
      transport: "stdio";
      command: string;
      /** The command is a file of the plugin copy rather than a runtime of the image. */
      fromPlugin: boolean;
      args: string[];
      env: { name: string; value: string }[];
      cwd: string;
    }
  | {
      transport: "remote";
      url: string;
      /** Names only. A header value may be a credential and is never shown. */
      headers: string[];
    };

export function describeServer(server: McpServer): ServerDescription {
  if ("remote" in server) {
    const { url, headers = [] } = server.remote;
    return {
      transport: "remote",
      url,
      headers: headers.map(({ name }) => name),
    };
  }
  const { command, args = [], env = {}, cwd = "plugin_root" } = server.stdio;
  const fromPlugin = "plugin_relative" in command;
  return {
    transport: "stdio",
    command: fromPlugin ? `./${command.plugin_relative}` : command.bare,
    fromPlugin,
    args,
    env: Object.entries(env).map(([name, value]) => ({ name, value })),
    cwd:
      cwd === "plugin_root"
        ? "${PLUGIN_ROOT}"
        : "plugin" in cwd
          ? joined("${PLUGIN_ROOT}", cwd.plugin)
          : joined("${PLUGIN_DATA}", cwd.data),
  };
}

function joined(base: string, path: string): string {
  return path ? `${base}/${path}` : base;
}

export function describeComponent(component: FindingComponent): {
  kind: "manifest" | "mcp" | "skill" | "server";
  name: string | null;
} {
  if (component === "manifest" || component === "mcp")
    return { kind: component, name: null };
  return "skill" in component
    ? { kind: "skill", name: component.skill }
    : { kind: "server", name: component.server };
}

/** `url` when a link to it is safe to render: a stored source is only checked for shape. */
export function httpsLink(url: string): string | null {
  try {
    return new URL(url).protocol === "https:" ? url : null;
  } catch {
    return null;
  }
}

function attached(config: Config): unknown[] {
  return Array.isArray(config.agent_plugins) ? config.agent_plugins : [];
}

const nameOf = (plugin: unknown) =>
  isRecord(plugin) && isString(plugin.name) ? plugin.name : null;

export function canAttach(config: Config, name: string): boolean {
  const current = attached(config);
  return (
    current.length < MAX_AGENT_PLUGINS ||
    current.some((plugin) => nameOf(plugin) === name)
  );
}

/**
 * `config` with `reference` attached. A plugin of the same name is replaced in
 * place, because the order of plugins is the order of their Skills.
 */
export function withPlugin(config: Config, reference: AgentPluginRef): Config {
  if (!canAttach(config, reference.name))
    throw new RangeError(
      `an Agent takes in at most ${MAX_AGENT_PLUGINS} plugins`,
    );
  const current = attached(config);
  const at = current.findIndex((plugin) => nameOf(plugin) === reference.name);
  return {
    ...config,
    agent_plugins:
      at < 0
        ? [...current, reference]
        : current.map((plugin, index) => (index === at ? reference : plugin)),
  };
}

export function withoutPlugin(config: Config, name: string): Config {
  const kept = attached(config).filter((plugin) => nameOf(plugin) !== name);
  const next = { ...config };
  if (kept.length > 0) next.agent_plugins = kept;
  else delete next.agent_plugins;
  if (isRecord(config.agent_plugin_settings)) {
    const settings = { ...config.agent_plugin_settings };
    delete settings[name];
    if (Object.keys(settings).length > 0) next.agent_plugin_settings = settings;
    else delete next.agent_plugin_settings;
  }
  return next;
}

/**
 * The plugin servers a `run.plugin_tools_pinned` event reports as unreachable, as
 * `plugin/server`, once each. The event carries names and reason codes only.
 */
export function skippedServers(data: unknown): string[] {
  const skipped = (data as { skipped?: unknown } | null)?.skipped;
  if (!Array.isArray(skipped)) return [];
  const servers = new Set<string>();
  for (const entry of skipped) {
    const { plugin, server, reason } = (entry ?? {}) as Record<string, unknown>;
    if (
      reason === "server_unavailable" &&
      typeof plugin === "string" &&
      typeof server === "string"
    )
      servers.add(`${plugin}/${server}`);
  }
  return [...servers];
}

/** The header a server secret travels in unless the definition names another. */
export const SECRET_HEADER = "Authorization";

/**
 * The pasted secret one remote server authenticates with when it offers no
 * OAuth (ADR 0100, ADR 0073 amended 2026-11-16), as `ServerSecret` stores it.
 */
export type ServerSecret = {
  name: string;
  header?: string;
  /**
   * `raw` sends the value as it is in `Authorization` too. Without it,
   * `Authorization` carries `Bearer <value>` and any other header the raw
   * value; `bearer` is valid only in `Authorization`.
   */
  form?: "bearer" | "raw";
  owner?: "any" | "personal";
};
export type ReadOnlyTool = { name: string; schema_digest: string };
/** What an Agent definition decides about one server, as `ServerSettings` stores it. */
export type ServerSettings = {
  tools?: string[] | null;
  exposure?: "eager" | "deferred" | null;
  read_only?: ReadOnlyTool[];
  secret?: ServerSecret;
};
/** `agent_plugin_settings`: plugin name, then server name. */
export type PluginSettings = Record<string, Record<string, ServerSettings>>;

/** The settings of every attached plugin, or none when they are not an object. */
export function pluginSettings(config: Config): PluginSettings {
  return isRecord(config.agent_plugin_settings)
    ? (config.agent_plugin_settings as PluginSettings)
    : {};
}

export function serverSettings(
  config: Config,
  plugin: string,
  server: string,
): ServerSettings {
  const servers = pluginSettings(config)[plugin];
  return isRecord(servers) && isRecord(servers[server]) ? servers[server] : {};
}

/**
 * `config` with one server's settings replaced. Empty choices are left out,
 * and so are a plugin and the whole key once nothing is chosen, so a
 * definition without choices keeps its bytes.
 */
export function withServerSettings(
  config: Config,
  plugin: string,
  server: string,
  settings: ServerSettings,
): Config {
  const kept: ServerSettings = {};
  if (settings.tools != null) kept.tools = settings.tools;
  if (settings.exposure != null) kept.exposure = settings.exposure;
  if (settings.read_only && settings.read_only.length > 0)
    kept.read_only = settings.read_only;
  if (settings.secret && settings.secret.name) kept.secret = settings.secret;
  const all = { ...pluginSettings(config) };
  const servers = { ...(all[plugin] ?? {}) };
  if (Object.keys(kept).length > 0) servers[server] = kept;
  else delete servers[server];
  if (Object.keys(servers).length > 0) all[plugin] = servers;
  else delete all[plugin];
  const next = { ...config };
  if (Object.keys(all).length > 0) next.agent_plugin_settings = all;
  else delete next.agent_plugin_settings;
  return next;
}

export type SecretDescription = {
  name: string;
  header: string;
  /** `Authorization` carries `Bearer <value>` unless the form is `raw`; any other header the raw value. */
  form: "bearer" | "raw";
  /** `personal`: only the Run's person's own secret. `either`: theirs wins, the shared one otherwise. */
  owner: "personal" | "either";
};

/** What Catalog Approval and the Agent form show of a server secret: never its value. */
export function describeSecret(secret: ServerSecret): SecretDescription {
  const header = secret.header?.trim() || SECRET_HEADER;
  return {
    name: secret.name,
    header,
    form: secretBearer(header, secret.form) ? "bearer" : "raw",
    owner: secret.owner === "personal" ? "personal" : "either",
  };
}

/** Whether a secret in `header` goes as `Bearer <value>`: only in `Authorization`, unless its form is `raw`. */
function secretBearer(header: string, form: ServerSecret["form"]): boolean {
  return header.toLowerCase() === SECRET_HEADER.toLowerCase() && form !== "raw";
}

/**
 * A server secret after one edit in the Agent form, as `ServerSecret` stores
 * it: the default header and owner are left out, and so is a form that only
 * repeats what the header implies. Only `Authorization` keeps `raw`; any
 * other header always carries the raw value.
 */
export function editSecret(
  secret: ServerSecret | undefined,
  patch: Partial<ServerSecret>,
): ServerSecret | undefined {
  const next: ServerSecret = { name: "", ...secret, ...patch };
  if (next.header === "" || next.header === SECRET_HEADER) delete next.header;
  if (
    next.form !== "raw" ||
    (next.header ?? SECRET_HEADER).toLowerCase() !== SECRET_HEADER.toLowerCase()
  )
    delete next.form;
  if (next.owner !== "personal") delete next.owner;
  return next.name ? next : undefined;
}

export type AttachTool = { name: string; schema_digest: string };
/** The answer of `POST /api/agent-plugins/tools`. */
export type AttachListing =
  | { outcome: "listed"; tools: AttachTool[] }
  | { outcome: "unavailable"; reason: string };

/** The answer of `POST /api/agent-plugins/tools`, or `null` when it is not one. */
export function readListing(value: unknown): AttachListing | null {
  if (!isRecord(value)) return null;
  if (value.outcome === "unavailable" && isString(value.reason))
    return { outcome: "unavailable", reason: value.reason };
  if (
    value.outcome === "listed" &&
    Array.isArray(value.tools) &&
    value.tools.every(
      (tool) =>
        isRecord(tool) && isString(tool.name) && isString(tool.schema_digest),
    )
  )
    return {
      outcome: "listed",
      tools: (value.tools as AttachTool[]).map(({ name, schema_digest }) => ({
        name,
        schema_digest,
      })),
    };
  return null;
}

/**
 * Read-only declarations after one tool of a listing is checked or
 * unchecked. A checked tool records the digest the listing returned, so Run
 * start can confirm the schema it was declared against.
 */
export function toggleReadOnly(
  declared: readonly ReadOnlyTool[],
  tool: AttachTool,
  checked: boolean,
): ReadOnlyTool[] {
  const others = declared.filter((item) => item.name !== tool.name);
  return checked
    ? [...others, { name: tool.name, schema_digest: tool.schema_digest }].sort(
        (a, b) => a.name.localeCompare(b.name),
      )
    : others;
}

/**
 * Declared Read-only tools the latest listing does not show: every
 * declaration while nothing is listed, otherwise those whose name the server
 * no longer lists. The form shows them so they can still be removed.
 */
export function unlistedReadOnly(
  declared: readonly ReadOnlyTool[],
  listing: AttachListing | null,
): ReadOnlyTool[] {
  if (listing?.outcome !== "listed") return [...declared];
  const listed = new Set(listing.tools.map((tool) => tool.name));
  return declared.filter((tool) => !listed.has(tool.name));
}

/** Whether `tool` is declared Read-only against the schema the listing returned. */
export function isDeclaredReadOnly(
  declared: readonly ReadOnlyTool[],
  tool: AttachTool,
): boolean {
  return declared.some(
    (item) =>
      item.name === tool.name && item.schema_digest === tool.schema_digest,
  );
}
