import { test } from "node:test";
import assert from "node:assert/strict";
import {
  MAX_AGENT_PLUGINS,
  agentPlugins,
  canAttach,
  describeComponent,
  describeServer,
  hasPluginSkills,
  httpsLink,
  isAgentPluginRef,
  readImport,
  skippedServers,
  withPlugin,
  withoutPlugin,
} from "../src/agent-plugins.ts";

const stdio = {
  stdio: {
    command: { plugin_relative: "bin/serve" },
    args: ["--root", "${PLUGIN_DATA}"],
    env: { LEVEL: "debug", MODE: "fast" },
    cwd: { data: "cache" },
  },
};
const remote = {
  remote: {
    url: "https://mcp.example.com/sse",
    headers: [
      { name: "Authorization", value: "Bearer sk-live-secret" },
      { name: "X-Team", value: "blue" },
    ],
  },
};
const plugin = (name, patch = {}) => ({
  digest: `sha256:${"a".repeat(64)}`,
  name,
  source: `https://github.com/acme/${name}/tree/0123abc`,
  version: "1.2.0",
  license: "MIT",
  skills: ["review"],
  servers: { files: stdio, search: remote },
  findings: [
    {
      component: { server: "legacy" },
      outcome: "skipped",
      reason: "unsupported_transport",
      detail: "sse",
    },
    {
      component: "manifest",
      outcome: "ignored",
      reason: "unknown_field",
      detail: "homepage",
    },
  ],
  ...patch,
});
const agent = (config) => ({ kind: "agent", config });

test("an Agent's plugins are read from its definition in order", () => {
  const plugins = [plugin("alpha"), plugin("beta")];
  assert.deepEqual(agentPlugins(agent({ agent_plugins: plugins })), plugins);
});

test("other kinds and definitions without usable plugins show none", () => {
  assert.equal(
    agentPlugins({ kind: "tool", config: { agent_plugins: [plugin("a")] } }),
    null,
  );
  assert.equal(agentPlugins(agent({})), null);
  assert.equal(agentPlugins(agent(null)), null);
  assert.equal(agentPlugins(agent({ agent_plugins: [] })), null);
  assert.equal(agentPlugins(agent({ agent_plugins: "alpha" })), null);
  assert.equal(
    agentPlugins(agent({ agent_plugins: [plugin("a"), { name: "b" }] })),
    null,
  );
  assert.equal(
    agentPlugins(agent({ agent_plugins: [plugin("a", { skills: "review" })] })),
    null,
  );
  assert.equal(
    agentPlugins(
      agent({
        agent_plugins: [
          plugin("a", {
            servers: { x: { stdio: { command: { bare: "bash" } } } },
          }),
        ],
      }),
    ),
    null,
  );
  assert.equal(
    agentPlugins(
      agent({
        agent_plugins: [
          plugin("a", {
            findings: [
              {
                component: "manifest",
                outcome: "fatal",
                reason: "x",
                detail: "y",
              },
            ],
          }),
        ],
      }),
    ),
    null,
  );
});

test("a definition registered without the optional fields still shows its servers", () => {
  const sparse = plugin("sparse", {
    version: undefined,
    license: null,
    servers: {
      bare: { stdio: { command: { bare: "uvx" } } },
      web: { remote: { url: "https://a.example/mcp" } },
    },
  });
  assert.ok(isAgentPluginRef(sparse));
  assert.deepEqual(describeServer(sparse.servers.bare), {
    transport: "stdio",
    command: "uvx",
    fromPlugin: false,
    args: [],
    env: [],
    cwd: "${PLUGIN_ROOT}",
  });
  assert.deepEqual(describeServer(sparse.servers.web), {
    transport: "remote",
    url: "https://a.example/mcp",
    headers: [],
  });
});

test("a stdio server shows its command, arguments, environment and directory", () => {
  assert.deepEqual(describeServer(stdio), {
    transport: "stdio",
    command: "./bin/serve",
    fromPlugin: true,
    args: ["--root", "${PLUGIN_DATA}"],
    env: [
      { name: "LEVEL", value: "debug" },
      { name: "MODE", value: "fast" },
    ],
    cwd: "${PLUGIN_DATA}/cache",
  });
  const directories = (cwd) =>
    describeServer({ stdio: { command: { bare: "node" }, cwd } }).cwd;
  assert.equal(directories("plugin_root"), "${PLUGIN_ROOT}");
  assert.equal(directories({ plugin: "server" }), "${PLUGIN_ROOT}/server");
  assert.equal(directories({ data: "" }), "${PLUGIN_DATA}");
});

test("a remote server shows its URL and header names but never header values", () => {
  const description = describeServer(remote);
  assert.deepEqual(description, {
    transport: "remote",
    url: "https://mcp.example.com/sse",
    headers: ["Authorization", "X-Team"],
  });
  const text = JSON.stringify(description);
  assert.ok(!text.includes("sk-live-secret"));
  assert.ok(!text.includes("blue"));
});

test("a finding is described by the part of the plugin it concerns", () => {
  assert.deepEqual(describeComponent("manifest"), {
    kind: "manifest",
    name: null,
  });
  assert.deepEqual(describeComponent("mcp"), { kind: "mcp", name: null });
  assert.deepEqual(describeComponent({ skill: "review" }), {
    kind: "skill",
    name: "review",
  });
  assert.deepEqual(describeComponent({ server: "legacy" }), {
    kind: "server",
    name: "legacy",
  });
});

test("only an https source becomes a link", () => {
  assert.equal(
    httpsLink("https://github.com/acme/a"),
    "https://github.com/acme/a",
  );
  assert.equal(httpsLink("javascript:alert(1)"), null);
  assert.equal(httpsLink("http://github.com/acme/a"), null);
  assert.equal(httpsLink("not a url"), null);
});

test("an Agent counts plugin Skills as instructions, plugins without Skills do not", () => {
  assert.equal(
    hasPluginSkills([plugin("a", { skills: [] }), plugin("b")]),
    true,
  );
  assert.equal(hasPluginSkills([plugin("a", { skills: [] })]), false);
  assert.equal(hasPluginSkills([]), false);
});

test("attaching a plugin keeps every other key and adds it last", () => {
  const config = {
    model: { id: "m" },
    bindings: [],
    agent_plugins: [plugin("alpha")],
  };
  const next = withPlugin(config, plugin("beta"));
  assert.deepEqual(next, {
    ...config,
    agent_plugins: [plugin("alpha"), plugin("beta")],
  });
  assert.equal(config.agent_plugins.length, 1);
  assert.deepEqual(withPlugin({ max_steps: 64 }, plugin("alpha")), {
    max_steps: 64,
    agent_plugins: [plugin("alpha")],
  });
});

test("attaching a plugin of the same name replaces it in place", () => {
  const config = {
    agent_plugins: [plugin("alpha"), plugin("beta"), plugin("gamma")],
  };
  const newer = plugin("beta", {
    digest: `sha256:${"b".repeat(64)}`,
    version: "2.0.0",
  });
  assert.deepEqual(withPlugin(config, newer).agent_plugins, [
    plugin("alpha"),
    newer,
    plugin("gamma"),
  ]);
});

test("an Agent takes in at most eight plugins", () => {
  const names = Array.from(
    { length: MAX_AGENT_PLUGINS },
    (_, index) => `plugin-${index}`,
  );
  const full = names.reduce(
    (config, name) => withPlugin(config, plugin(name)),
    {},
  );
  assert.equal(full.agent_plugins.length, MAX_AGENT_PLUGINS);
  assert.equal(canAttach(full, "plugin-9"), false);
  assert.throws(() => withPlugin(full, plugin("plugin-9")), RangeError);
  assert.equal(canAttach(full, "plugin-3"), true);
  assert.equal(
    withPlugin(full, plugin("plugin-3", { version: "3.0.0" })).agent_plugins
      .length,
    MAX_AGENT_PLUGINS,
  );
});

test("removing a plugin keeps the others and drops the key with the last one", () => {
  const config = {
    model: { id: "m" },
    agent_plugins: [plugin("alpha"), plugin("beta")],
    agent_plugin_settings: {
      alpha: { servers: { search: { read_only: true } } },
      beta: { servers: { docs: { read_only: false } } },
    },
  };
  assert.deepEqual(withoutPlugin(config, "alpha"), {
    model: { id: "m" },
    agent_plugins: [plugin("beta")],
    agent_plugin_settings: {
      beta: { servers: { docs: { read_only: false } } },
    },
  });
  const emptied = withoutPlugin(withoutPlugin(config, "alpha"), "beta");
  assert.deepEqual(emptied, { model: { id: "m" } });
  assert.ok(!("agent_plugins" in emptied));
  assert.deepEqual(withoutPlugin({ model: 1 }, "alpha"), { model: 1 });
  assert.equal(config.agent_plugins.length, 2);
  assert.deepEqual(Object.keys(config.agent_plugin_settings), [
    "alpha",
    "beta",
  ]);
});

test("the import answer keeps the reference exactly as the server sent it", () => {
  const reference = { ...plugin("alpha"), extra: { kept: true } };
  const answer = {
    plugins: ["plugins/alpha"],
    selected: { reference, denied: ["links/out"] },
  };
  const read = readImport(answer);
  assert.equal(read.selected.reference, reference);
  assert.deepEqual(read.selected.denied, ["links/out"]);
  assert.deepEqual(readImport({ plugins: ["a", "b"], selected: null }), {
    plugins: ["a", "b"],
    selected: null,
  });
});

test("an import answer that is not the documented shape is refused", () => {
  assert.equal(readImport(null), null);
  assert.equal(readImport({ selected: null }), null);
  assert.equal(readImport({ plugins: [1], selected: null }), null);
  assert.equal(
    readImport({ plugins: ["a"], selected: { reference: plugin("a") } }),
    null,
  );
  assert.equal(
    readImport({
      plugins: ["a"],
      selected: { reference: { name: "a" }, denied: [] },
    }),
    null,
  );
});

test("a pin event names each unreachable plugin server once and ignores other skips", () => {
  const data = {
    skipped: [
      { plugin: "acme", server: "docs", reason: "server_unavailable" },
      { plugin: "acme", server: "docs", reason: "tool_invalid" },
      { plugin: "acme", server: "wiki", reason: "server_unavailable" },
      { plugin: "acme", server: "wiki", reason: "server_unavailable" },
      { plugin: 7, server: "x", reason: "server_unavailable" },
    ],
  };
  assert.deepEqual(skippedServers(data), ["acme/docs", "acme/wiki"]);
  assert.deepEqual(skippedServers({ skipped: [] }), []);
  assert.deepEqual(skippedServers({}), []);
  assert.deepEqual(skippedServers(null), []);
});
