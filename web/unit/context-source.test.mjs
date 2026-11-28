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
    source_provider: "thread",
    source_operation: "retrieve",
    source_pin: "always",
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
    policy: "always",
  });
  assert.deepEqual(value.tools, {
    read: [ref("read")],
    write: [ref("write")],
    detail: ref("detail"),
  });
  assert.equal(value.query, "accepted-inputs-first@1");
  assert.equal(value.refresh, "revision-bound@1");
});
test("Required-Home keeps Always delivery and rejects OnDemand", () => {
  const input = form();
  input.set("source_required_home", "on");
  assert.equal(contextSourceConfiguration(input).required_home, true);
  input.set("source_pin", "on_demand");
  assert.throws(
    () => contextSourceConfiguration(input),
    /Required-Home sources use Always delivery/,
  );
});
test("Scored pins are rejected and Tool references must be exact", () => {
  const input = form();
  input.set("source_pin", "scored");
  assert.throws(
    () => contextSourceConfiguration(input),
    /Choose a source pin policy/,
  );
  input.set("source_pin", "always");
  input.append("source_read", JSON.stringify({ id: "read", version: "2.0.0" }));
  assert.throws(
    () => contextSourceConfiguration(input),
    /exact Registry version/,
  );
});
