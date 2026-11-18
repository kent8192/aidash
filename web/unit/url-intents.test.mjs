import { test } from "node:test";
import assert from "node:assert/strict";
import {
  bindsPageReading,
  snapshotReadsPages,
  urlIntents,
} from "../src/url-intents.ts";

const tool = (id, provider, operation) => ({
  id,
  version: "1.0.0",
  kind: "tool",
  config: { provider, operation },
});

const home = "aidash://home";
const member = (id) => ({ registry_deployment: home, id, version: "1.0.0" });
const registry = [
  tool("aidash.page.fetch", "research.page@1", "fetch"),
  tool("aidash.shell", "aidash.shell@1", "shell"),
  {
    id: "pages",
    version: "1.0.0",
    kind: "bundle",
    config: { members: [member("aidash.page.fetch"), member("aidash.shell")] },
  },
];

const agent = (bindings) => ({
  id: "reader",
  version: "1.0.0",
  kind: "agent",
  config: { schema_version: 1, bindings },
});

test("an Agent binds page reading directly or through a bundle", () => {
  const tool = (id) => ({ kind: "tool", target: { id, version: "1.0.0" } });
  assert.equal(
    bindsPageReading(agent([tool("aidash.page.fetch")]), registry),
    true,
  );
  assert.equal(
    bindsPageReading(
      agent([{ kind: "bundle", target: { id: "pages", version: "1.0.0" } }]),
      registry,
    ),
    true,
  );
  // A bundle Binding exposes only its selected members.
  assert.equal(
    bindsPageReading(
      agent([
        {
          kind: "bundle",
          target: { id: "pages", version: "1.0.0" },
          members: ["aidash.shell"],
        },
      ]),
      registry,
    ),
    false,
  );
  assert.equal(
    bindsPageReading(agent([tool("aidash.shell")]), registry),
    false,
  );
  assert.equal(bindsPageReading(agent([]), registry), false);
  assert.equal(bindsPageReading(undefined, registry), false);
});

test("a binding qualified with another Registry node never resolves a local entry", () => {
  const target = (node) => ({
    kind: "tool",
    target: {
      registry_deployment: node,
      id: "aidash.page.fetch",
      version: "1.0.0",
    },
  });
  assert.equal(
    bindsPageReading(agent([target("aidash://peer")]), registry, home),
    false,
  );
  assert.equal(bindsPageReading(agent([target(home)]), registry, home), true);
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

test("a URL keeps the closing brackets it opens itself", () => {
  assert.deepEqual(
    urlIntents(
      "See https://en.wikipedia.org/wiki/Foo_(bar), [https://example.com/x] and https://example.com/a[1].",
    ),
    [
      "https://en.wikipedia.org/wiki/Foo_(bar)",
      "https://example.com/x",
      "https://example.com/a[1]",
    ],
  );
});

test("a URL in angle brackets keeps its trailing punctuation", () => {
  assert.deepEqual(
    urlIntents(
      "Read <https://en.wikipedia.org/wiki/Yahoo!> and https://example.com/page.",
    ),
    ["https://en.wikipedia.org/wiki/Yahoo!", "https://example.com/page"],
  );
});

test("a URL longer than the server accepts is not offered", () => {
  const base = "https://example.com/";
  const fits = base + "a".repeat(4096 - base.length);
  assert.deepEqual(urlIntents(`${fits} ${fits}b`), [fits]);
  // The limit counts UTF-8 bytes, not characters.
  assert.deepEqual(urlIntents(base + "é".repeat(2040)), []);
});

test("addresses the host would refuse are not offered", () => {
  assert.deepEqual(
    urlIntents(
      "ftp://example.com/x https://user:secret@example.com/ https://example.com/#top example.com/page",
    ),
    [],
  );
});

test("a Run reads pages only through a fetch Binding it was admitted with", () => {
  const resolved = (provider, operation, excluded_reason = null) => ({
    definition: tool(`${provider}.${operation}`, provider, operation),
    excluded_reason,
  });
  const snapshot = (bindings, remote = false) => ({ remote, bindings });
  assert.equal(
    snapshotReadsPages(snapshot([resolved("research.page@1", "fetch")])),
    true,
  );
  // Excluded at admission: a Catalog Approval granted later does not count.
  assert.equal(
    snapshotReadsPages(
      snapshot([resolved("research.page@1", "fetch", "catalog_approval")]),
    ),
    false,
  );
  assert.equal(
    snapshotReadsPages(snapshot([resolved("research.fixture@1", "search")])),
    false,
  );
  assert.equal(
    snapshotReadsPages(snapshot([resolved("research.page@1", "fetch")], true)),
    false,
  );
  assert.equal(snapshotReadsPages(undefined), false);
  assert.equal(snapshotReadsPages(null), false);
});
