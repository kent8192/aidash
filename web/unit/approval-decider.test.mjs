import { test } from "node:test";
import assert from "node:assert/strict";
import {
  BUILT_IN_CHOICE,
  approvalDeciderChoices,
  deciderUpdate,
  focusedVersion,
  registryVersionOf,
  versionKey,
} from "../src/approval-decider-model.ts";

const version = (id, patch = {}) => ({
  entry: {
    id,
    version: "1.0.0",
    kind: "decider",
    config: { hook: "approval" },
    ...patch,
  },
});

const versions = [
  version("aidash.approval-decider"),
  version("tenant.judge"),
  version("tenant.judge", { version: "2.0.0" }),
  version("tenant.compaction", { config: { hook: "compaction" } }),
  version("tenant.model", { kind: "model", config: { hook: "approval" } }),
  version("tenant.broken", { config: null }),
];

test("a Tenant names its own Deciders of hook approval, not the built-in one", () => {
  assert.deepEqual(
    approvalDeciderChoices(versions).map(({ entry }) => versionKey(entry)),
    ["tenant.judge@1.0.0", "tenant.judge@2.0.0"],
  );
});

test("a select value names a Decider by id and version", () => {
  assert.deepEqual(deciderUpdate({ revision: 4 }, "tenant.judge@2.0.0"), {
    expected_revision: 4,
    decider: { id: "tenant.judge", version: "2.0.0" },
  });
});

test("the built-in choice resets the Tenant's Decider", () => {
  assert.deepEqual(deciderUpdate({ revision: 0 }, BUILT_IN_CHOICE), {
    expected_revision: 0,
    decider: null,
  });
});

test("a value that names no version resets instead of sending a broken reference", () => {
  for (const choice of ["@1.0.0", "judge"])
    assert.equal(deciderUpdate({ revision: 1 }, choice).decider, null);
});

test("the Registry version of a reference is found by id and version", () => {
  assert.equal(
    registryVersionOf(versions, { id: "tenant.judge", version: "2.0.0" }),
    versions[2],
  );
  assert.equal(
    registryVersionOf(versions, { id: "tenant.judge", version: "3.0.0" }),
    null,
  );
});

test("a settings link opens the version it names, and none for another focus", () => {
  assert.equal(focusedVersion(versions, "tenant.judge@1.0.0"), versions[1]);
  assert.equal(focusedVersion(versions, "semantic"), null);
  assert.equal(focusedVersion(versions, ""), null);
});
