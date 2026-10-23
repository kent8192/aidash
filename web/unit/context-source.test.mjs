import { test } from "node:test";
import assert from "node:assert/strict";
import { contextSourceConfiguration } from "../src/context-source-model.ts";
const ref = (id) => ({
  registry_deployment: "aidash://home/default",
  id,
  version: "2.0.0",
});
const form = () => {
  const input = new FormData();
  for (const [name, value] of Object.entries({
    source_declaration: "on",
    source_provider: "workspace",
    source_operation: "retrieve",
    source_pin: "scored",
    source_mode: "shadow",
    source_decider: JSON.stringify(ref("select")),
    source_volatility: "step",
    source_query: "accepted-inputs-first@1",
    source_disclose_previews: "on",
    source_disclose_query: "on",
  }))
    input.set(name, value);
  return input;
};
test("existing source definitions remain unmodified until explicitly opted in", () => {
  assert.equal(contextSourceConfiguration(new FormData()), undefined);
});
test("source policy submits exact qualified dependencies and versioned query selection", () => {
  const input = form();
  input.append("source_read", JSON.stringify(ref("read")));
  input.append("source_write", JSON.stringify(ref("write")));
  input.set("source_detail", JSON.stringify(ref("detail")));
  const value = contextSourceConfiguration(input);
  assert.deepEqual(value.pin, {
    policy: "scored",
    mode: "shadow",
    decider: ref("select"),
  });
  assert.deepEqual(value.tools, {
    read: [ref("read")],
    write: [ref("write")],
    detail: ref("detail"),
  });
  assert.equal(value.query, "accepted-inputs-first@1");
  assert.equal(value.refresh, "revision-bound@1");
});
test("required Home may be observed but cannot be filtered through Enforce", () => {
  const input = form();
  input.set("source_required_home", "on");
  assert.equal(contextSourceConfiguration(input).required_home, true);
  input.set("source_mode", "enforce");
  assert.throws(() => contextSourceConfiguration(input), /cannot enforce/);
});
test("Scored requires declared preview and query disclosure and a complete exact reference", () => {
  const input = form();
  input.delete("source_disclose_query");
  assert.throws(
    () => contextSourceConfiguration(input),
    /explicit preview and query disclosure/,
  );
  input.set("source_disclose_query", "on");
  input.set(
    "source_decider",
    JSON.stringify({ id: "select", version: "2.0.0" }),
  );
  assert.throws(
    () => contextSourceConfiguration(input),
    /exact Registry version/,
  );
});
