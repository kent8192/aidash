import { test } from "node:test";
import assert from "node:assert/strict";
import {
  approvalRequest,
  approvalState,
  dependencies,
  earlierAgentGroups,
  requestedGroups,
  unapprovedClosure,
  unapprovedDependency,
} from "../src/catalog-approval-model.ts";

const version = (id, kind, config = {}, state = {}) => ({
  entry: { id, version: state.version ?? "1.0.0", kind, config },
  needs_approval: state.needs_approval ?? true,
  approval: state.approval ?? null,
  history: [],
  subject: state.subject ?? null,
});
const ref = (id, version = "1.0.0") => ({ id, version });

test("each version shows whether its Tenant approved it", () => {
  assert.equal(
    approvalState(version("tool", "tool", {}, { needs_approval: false })),
    "builtin",
  );
  assert.equal(approvalState(version("tool", "tool")), "unapproved");
  assert.equal(
    approvalState(
      version("tool", "tool", {}, { approval: { enabled: true, revision: 1 } }),
    ),
    "approved",
  );
  assert.equal(
    approvalState(
      version(
        "tool",
        "tool",
        {},
        { approval: { enabled: false, revision: 2 } },
      ),
    ),
    "withdrawn",
  );
});

test("dependencies follow the definitions each kind names", () => {
  assert.deepEqual(
    dependencies({
      kind: "agent",
      config: {
        model: ref("model"),
        cluster: ref("team"),
        context_policy: {
          version: "context-recovery/1",
          summary: { model: ref("summarizer") },
        },
        bindings: [{ kind: "tool", target: ref("search") }, null],
      },
    }),
    [ref("model"), ref("team"), ref("summarizer"), ref("search")],
  );
  assert.deepEqual(
    dependencies({
      kind: "bundle",
      config: { members: [ref("aidash.shell"), ref("aidash.python")] },
    }),
    [ref("aidash.shell"), ref("aidash.python")],
  );
  assert.deepEqual(
    dependencies({ kind: "cluster", config: { coordinator: ref("lead") } }),
    [ref("lead")],
  );
  assert.deepEqual(
    dependencies({ kind: "tool", config: { model: ref("ignored") } }),
    [],
  );
});

test("the closure lists every unapproved dependency once, transitively", () => {
  const versions = [
    version("agent", "agent", {
      model: ref("model"),
      bindings: [
        { target: ref("bundle") },
        { target: ref("approved") },
        { target: ref("native") },
      ],
    }),
    version("model", "model"),
    version("bundle", "bundle", {
      members: [ref("shell"), ref("model")],
    }),
    version("shell", "tool", {}, { approval: { enabled: false, revision: 3 } }),
    version(
      "approved",
      "skill",
      { unused: ref("never-read") },
      {
        approval: { enabled: true, revision: 1 },
      },
    ),
    version("native", "tool", {}, { needs_approval: false }),
  ];
  const closure = unapprovedClosure(versions[0], versions);
  assert.deepEqual(
    closure.dependencies.map((item) => item.entry.id),
    ["model", "bundle", "shell"],
  );
  assert.deepEqual(closure.blocked, []);
});

test("the closure keeps the dependency the server named and flags what it cannot approve", () => {
  const versions = [
    version("agent", "agent", { model: ref("installed") }),
    version("installed", "model", {}, {}),
    version("extra", "tool"),
  ];
  versions[1].entry.installation = { installation: "copy" };
  const closure = unapprovedClosure(versions[0], versions, ref("extra"));
  assert.deepEqual(
    closure.dependencies.map((item) => item.entry.id),
    ["extra"],
  );
  assert.deepEqual(closure.blocked, [
    { reference: ref("installed"), installed: true },
  ]);
  assert.deepEqual(
    unapprovedClosure(versions[2], versions, ref("missing", "2.0.0")).blocked,
    [{ reference: ref("missing", "2.0.0"), installed: false }],
  );
});

test("only a dependency refusal names a dependency", () => {
  assert.deepEqual(
    unapprovedDependency("CATALOG_DEPENDENCY_UNAPPROVED: aidash.shell@1.0.0"),
    ref("aidash.shell"),
  );
  assert.equal(unapprovedDependency("AGENT_SUBJECT_EXISTS"), null);
  assert.equal(unapprovedDependency("forbidden"), null);
});

test("a new agent version starts from the latest approved earlier version's groups", () => {
  const agent = (number, groups, enabled = true) =>
    version(
      "writer",
      "agent",
      {},
      {
        version: number,
        approval: { enabled, revision: 1 },
        subject: groups && { id: `writer@${number}`, groups, enabled: true },
      },
    );
  const versions = [
    agent("1.2.0", ["members"]),
    agent("1.10.0", ["reviewers"]),
    agent("1.11.0", ["withdrawn"], false),
    agent("2.0.0", ["later"]),
    version(
      "other",
      "agent",
      {},
      {
        version: "1.0.0",
        approval: { enabled: true, revision: 1 },
        subject: { id: "other", groups: ["other"], enabled: true },
      },
    ),
  ];
  const target = version("writer", "agent", {}, { version: "1.12.0" });
  assert.deepEqual(earlierAgentGroups(target, versions), ["reviewers"]);
  assert.equal(
    earlierAgentGroups(
      version("writer", "agent", {}, { version: "1.0.0" }),
      versions,
    ),
    undefined,
  );
});

test("an approval names groups only when they differ from the default", () => {
  const assignable = ["members", "reviewers"];
  assert.equal(requestedGroups(["members"], assignable), undefined);
  assert.deepEqual(requestedGroups(["reviewers", "members"], assignable), [
    "members",
    "reviewers",
  ]);
  assert.deepEqual(requestedGroups([], assignable), []);
  assert.equal(requestedGroups([], ["reviewers"]), undefined);
});

test("an approval fences on the version's current approval revision", () => {
  assert.deepEqual(approvalRequest(version("tool", "tool"), true), {
    entry: ref("tool"),
    enabled: true,
    expected_revision: 0,
  });
  assert.deepEqual(
    approvalRequest(
      version(
        "agent",
        "agent",
        {},
        { approval: { enabled: true, revision: 4 } },
      ),
      false,
      ["members"],
    ),
    {
      entry: ref("agent"),
      enabled: false,
      expected_revision: 4,
      groups: ["members"],
    },
  );
});

test("warning acknowledgment belongs to the enabled exact version only", () => {
  const shell = {
    ...version("provider.openrouter.shell", "tool"),
    provider_warning: "Containers are not deleted by Aidash",
  };
  assert.equal(
    approvalRequest(shell, true).provider_warning_acknowledged,
    undefined,
  );
  assert.equal(
    approvalRequest(shell, true, undefined, true).provider_warning_acknowledged,
    true,
  );
  assert.equal(
    approvalRequest(shell, false, undefined, true)
      .provider_warning_acknowledged,
    undefined,
  );
  assert.equal(
    approvalRequest(version("ordinary", "tool"), true, undefined, true)
      .provider_warning_acknowledged,
    undefined,
  );
});
