import { test } from "node:test";
import assert from "node:assert/strict";
import {
  dollars,
  researchBindings,
  runGrantScope,
  urlIntents,
} from "../src/research.ts";

const tool = (id, provider, operation) => ({
  id,
  version: "1.0.0",
  kind: "tool",
  config: { provider, operation },
});

const registry = [
  tool("aidash.research.fixture.find", "research.fixture@1", "find"),
  tool("aidash.research.fixture.search", "research.fixture@1", "search"),
  tool("aidash.research.page.fetch", "research.page@1", "fetch"),
  tool("aidash.shell", "aidash.shell@1", "shell"),
  {
    id: "research-fixture",
    version: "1.0.0",
    kind: "bundle",
    config: {
      members: [
        {
          registry_node: "aidash://home",
          id: "aidash.research.fixture.find",
          version: "1.0.0",
        },
        {
          registry_node: "aidash://home",
          id: "aidash.research.fixture.search",
          version: "1.0.0",
        },
      ],
    },
  },
];

const agent = (bindings) => ({
  id: "researcher",
  version: "1.0.0",
  kind: "agent",
  config: { schema_version: 1, bindings },
});

test("research operations come from tool and bundle bindings, once each, in contract order", () => {
  const bound = researchBindings(
    agent([
      { kind: "bundle", target: { id: "research-fixture", version: "1.0.0" } },
      {
        kind: "tool",
        target: { id: "aidash.research.page.fetch", version: "1.0.0" },
      },
      {
        kind: "tool",
        target: { id: "aidash.research.fixture.search", version: "1.0.0" },
      },
      { kind: "tool", target: { id: "aidash.shell", version: "1.0.0" } },
    ]),
    registry,
  );
  assert.deepEqual(bound, [
    { provider: "research.fixture@1", operation: "search" },
    { provider: "research.page@1", operation: "fetch" },
    { provider: "research.fixture@1", operation: "find" },
  ]);
});

test("an Agent without research bindings, or an unknown version, binds none", () => {
  assert.deepEqual(researchBindings(agent([]), registry), []);
  assert.deepEqual(
    researchBindings(
      agent([
        {
          kind: "tool",
          target: { id: "aidash.research.page.fetch", version: "2.0.0" },
        },
      ]),
      registry,
    ),
    [],
  );
  assert.deepEqual(researchBindings(undefined, registry), []);
});

test("URL intents are the distinct readable http(s) addresses in the text", () => {
  assert.deepEqual(
    urlIntents(
      "Compare https://example.com/a, http://example.org/b?x=1. Again https://example.com/a and (https://example.net/c).",
    ),
    [
      "https://example.com/a",
      "http://example.org/b?x=1",
      "https://example.net/c",
    ],
  );
});

test("addresses the host would refuse are not offered", () => {
  assert.deepEqual(
    urlIntents(
      "ftp://example.com/x https://user:secret@example.com/ https://example.com/#top example.com/page",
    ),
    [],
  );
});

test("prices read as dollars and Run grant scopes say what they cover", () => {
  assert.equal(dollars(5000), "$0.0050");
  assert.match(
    runGrantScope(
      {
        operation: "search",
        provider: "research.brave@1",
        targets: ["api.search.brave.com"],
      },
      false,
    ),
    /Every later research\.brave@1 search in this Run, including queries you have not seen/,
  );
  assert.match(
    runGrantScope(
      {
        operation: "fetch",
        provider: "research.page@1",
        targets: ["example.com"],
      },
      false,
    ),
    /Later page readings from example\.com in this Run/,
  );
});
