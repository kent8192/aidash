import { test } from "node:test";
import assert from "node:assert/strict";
import { toolReplay } from "../src/tool-declaration.ts";

const tool = (transport) => ({ kind: "tool", config: { transport } });

test("an HTTP or MCP version reports its replay declaration", () => {
  for (const replay of ["read_only", "idempotent", "unsafe"]) {
    assert.equal(
      toolReplay(
        tool({ transport: "http", endpoint: "https://a.example", replay }),
      ),
      replay,
    );
    assert.equal(
      toolReplay(
        tool({
          transport: "mcp",
          endpoint: "https://a.example",
          tool_name: "x",
          replay,
        }),
      ),
      replay,
    );
  }
});

test("any other label asks, like the server", () => {
  assert.equal(
    toolReplay(tool({ transport: "http", replay: "READ_ONLY" })),
    "unsafe",
  );
  assert.equal(toolReplay(tool({ transport: "http" })), "unsafe");
});

test("definitions that are not Tenant HTTP or MCP tools declare nothing", () => {
  assert.equal(toolReplay(tool({ transport: "agent", node_id: "n" })), null);
  assert.equal(
    toolReplay(tool({ transport: "native", operation: "echo" })),
    null,
  );
  assert.equal(
    toolReplay({
      kind: "model",
      config: { transport: { transport: "http", replay: "read_only" } },
    }),
    null,
  );
  for (const config of [null, "tool", {}, { transport: "http" }])
    assert.equal(toolReplay({ kind: "tool", config }), null);
});
