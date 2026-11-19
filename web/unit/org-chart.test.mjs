import assert from "node:assert/strict";
import test from "node:test";
import {
  flowStopPath,
  hiddenCount,
  nodeFlows,
  orgChartPath,
  spendText,
  stopSummary,
} from "../src/org-chart-model.ts";

const node = (overrides = {}) => ({
  name: "research",
  label: "Research",
  default: false,
  created_by: {
    kind: "agent",
    root_subject: "alice",
    task_id: "t1",
    run_id: "r1",
  },
  flow: "root-1",
  duties: [],
  hidden_duties: 0,
  subnodes: [],
  hidden_subnodes: 0,
  ...overrides,
});

test("routes encode the Tenant and the Root Task", () => {
  assert.equal(orgChartPath("acme corp"), "/api/tenants/acme%20corp/org-chart");
  assert.equal(
    flowStopPath("acme", "a/b"),
    "/api/tenants/acme/flows/a%2Fb/stop",
  );
});

test("spend shows credits and calls without a reported cost", () => {
  assert.equal(
    spendText({ nanocredits: 1_250_000_000, unpriced_calls: 0 }),
    "1.25",
  );
  assert.equal(spendText({ nanocredits: 0, unpriced_calls: 3 }), "0.00 (+3)");
});

test("a Node lists each flow that made something on it once", () => {
  const flows = nodeFlows(
    node({
      duties: [
        { id: "d1", flow: "root-2" },
        { id: "d2", flow: "root-1" },
        { id: "d3" },
      ],
      subnodes: [{ id: "s1", flow: "root-2" }],
    }),
  );
  assert.deepEqual(flows, ["root-1", "root-2"]);
  assert.deepEqual(nodeFlows(node({ flow: undefined })), []);
});

test("a stop counts what it stopped, what was already stopped and failures", () => {
  const summary = stopSummary({
    root_task: "root-1",
    tasks: 3,
    runs: [
      { id: "r1", outcome: "stopped" },
      { id: "r2", outcome: "already_stopped" },
    ],
    subnodes: [{ id: "s1", outcome: "failed", error: "busy" }],
    duties: [{ id: "d1", outcome: "stopped" }],
    nodes: [{ name: "research", label: "Research" }],
  });
  assert.deepEqual(summary, { stopped: 2, already: 1, failed: 1, nodes: 1 });
});

test("hidden Duties and Subnodes are counted together", () => {
  assert.equal(hiddenCount(node()), 0);
  assert.equal(hiddenCount(node({ hidden_duties: 2, hidden_subnodes: 1 })), 3);
});
