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
