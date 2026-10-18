import { test } from "node:test";
import assert from "node:assert/strict";
import {
  approvalNotice,
  dollars,
  exceedsCeiling,
  hasResearch,
  periodSeconds,
  periodUnits,
  periodWords,
  recordedActor,
  requestPrice,
  researchFailure,
  researchBindings,
  runGrantScope,
  shortenedPeriods,
  snapshotReadsPages,
  splitPeriod,
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
  const ref = (id) => ({ id, version: "1.0.0" });
  assert.deepEqual(bound, [
    {
      provider: "research.fixture@1",
      operation: "search",
      // Bound through the bundle and directly: one operation, one definition.
      definitions: [ref("aidash.research.fixture.search")],
    },
    {
      provider: "research.page@1",
      operation: "fetch",
      definitions: [ref("aidash.research.page.fetch")],
    },
    {
      provider: "research.fixture@1",
      operation: "find",
      definitions: [ref("aidash.research.fixture.find")],
    },
  ]);
});

test("a bundle Binding exposes only its selected members, nested bundles whole", () => {
  const nested = [
    ...registry,
    {
      id: "research-outer",
      version: "1.0.0",
      kind: "bundle",
      config: {
        members: [
          {
            registry_node: "aidash://home",
            id: "research-fixture",
            version: "1.0.0",
          },
          {
            registry_node: "aidash://home",
            id: "aidash.research.page.fetch",
            version: "1.0.0",
          },
        ],
      },
    },
  ];
  const operations = (bindings) =>
    researchBindings(agent(bindings), nested).map(
      (binding) => binding.operation,
    );
  assert.deepEqual(
    operations([
      {
        kind: "bundle",
        target: { id: "research-fixture", version: "1.0.0" },
        members: ["aidash.research.fixture.search"],
      },
    ]),
    ["search"],
  );
  assert.deepEqual(
    operations([
      {
        kind: "bundle",
        target: { id: "research-outer", version: "1.0.0" },
        members: ["research-fixture"],
      },
    ]),
    ["search", "find"],
  );
  assert.deepEqual(
    operations([
      { kind: "bundle", target: { id: "research-outer", version: "1.0.0" } },
    ]),
    ["search", "fetch", "find"],
  );
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

test("a binding qualified with another Registry node never resolves a local entry", () => {
  const remote = agent([
    {
      kind: "tool",
      target: {
        registry_node: "aidash://peer",
        id: "aidash.research.page.fetch",
        version: "1.0.0",
      },
    },
  ]);
  assert.deepEqual(researchBindings(remote, registry, "aidash://home"), []);
  const local = agent([
    {
      kind: "tool",
      target: {
        registry_node: "aidash://home",
        id: "aidash.research.page.fetch",
        version: "1.0.0",
      },
    },
  ]);
  assert.deepEqual(
    researchBindings(local, registry, "aidash://home").map(
      (binding) => binding.operation,
    ),
    ["fetch"],
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

test("prices read as dollars and Run grant scopes say what they cover", () => {
  assert.equal(dollars(5000), "$0.0050");
  // A nonzero price never reads as zero.
  assert.equal(dollars(49), "$0.000049");
  assert.equal(dollars(12_345), "$0.012345");
  assert.equal(dollars(0), "$0.0000");
  assert.equal(requestPrice(5000, false), "$0.0050 (estimate, not an invoice)");
  assert.equal(requestPrice(0, false), "Free");
  assert.equal(requestPrice(0, true), "無料");
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
    /Later page readings from example\.com in this Run, whichever Research provider reads them/,
  );
});

test("a notification names what is asked, never what would be sent", () => {
  const card = {
    kind: "disclosure",
    operation: "search",
    targets: ["api.search.brave.com"],
    agent_id: "researcher",
    agent_version: "1.0.0",
    requester: "alice",
    disclosure: '{"q":"confidential merger plans"}',
    query: { query: "confidential merger plans" },
  };
  const line = approvalNotice(card, false);
  assert.equal(
    line,
    "Web search · api.search.brave.com · researcher@1.0.0 · requested by alice",
  );
  assert.doesNotMatch(line, /merger/);
  assert.match(
    approvalNotice({ ...card, operation: "fetch" }, true),
    /^ページ読み取り · /,
  );
  assert.match(
    approvalNotice(
      { kind: "outbound", targets: ["https://example.com"], requester: "bob" },
      false,
    ),
    /^Outbound request · https:\/\/example\.com · requested by bob$/,
  );
});

test("research failures read in plain words without amounts", () => {
  const failure = (code, operation = "search") => ({
    version: 1,
    operation,
    status: "error",
    error: { code, explanation: "x", retryable: false },
  });
  assert.match(
    researchFailure(failure("budget_exhausted"), false),
    /monthly limit .* reached\. It is available again next month\.$/,
  );
  assert.doesNotMatch(
    researchFailure(failure("budget_exhausted"), false),
    /\$|\d/,
  );
  assert.match(
    researchFailure(failure("account_unavailable", "fetch"), true),
    /現在使えません/,
  );
  assert.match(
    researchFailure(failure("disclosure_denied", "fetch"), false),
    /nothing was sent/,
  );
  assert.equal(researchFailure(failure("rate_limited"), false), null);
  assert.equal(
    researchFailure({ error: { code: "budget_exhausted" } }, false),
    null,
    "only research tool results are explained",
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

test("a retention period reads in the longest unit that divides it", () => {
  assert.deepEqual(splitPeriod(604_800), { value: 7, unit: "days" });
  assert.deepEqual(splitPeriod(86_400 + 3_600), { value: 25, unit: "hours" });
  assert.deepEqual(splitPeriod(90), { value: 90, unit: "seconds" });
  assert.deepEqual(splitPeriod(0), { value: 0, unit: "hours" });
  assert.equal(periodSeconds(25, "hours"), 90_000);
  assert.equal(periodWords(86_400, false), "1 day");
  assert.equal(periodWords(86_400 * 7, true), "7 日");
  assert.deepEqual(periodUnits(3_600), ["days", "hours", "minutes"]);
  assert.deepEqual(periodUnits(90), ["days", "hours", "minutes", "seconds"]);
});

const periods = (after, max) => ({
  document_snapshots: { after_run_seconds: after, max_age_seconds: max },
});

test("only a shortening, or a Tenant period above a lowered ceiling, is called out", () => {
  assert.deepEqual(
    shortenedPeriods(periods(86_400, 604_800), periods(3_600, 604_800)),
    ["after_run_seconds"],
  );
  assert.deepEqual(
    shortenedPeriods(periods(3_600, 86_400), periods(86_400, 604_800)),
    [],
  );
  assert.equal(exceedsCeiling(null, periods(1, 1)), false);
  assert.equal(
    exceedsCeiling(periods(7_200, 60), periods(3_600, 604_800)),
    true,
  );
  assert.equal(exceedsCeiling(periods(3_600, 60), periods(3_600, 60)), false);
});

test("a Deployment Administrator's identity is shown only to Deployment Administrators", () => {
  assert.equal(recordedActor("subject:acme/alice", false, false), "alice");
  assert.equal(
    recordedActor("deployment-administrator:4b2c", false, false),
    "Deployment Administrator",
  );
  assert.equal(
    recordedActor("deployment-administrator:4b2c", true, false),
    "Deployment Administrator (4b2c)",
  );
  assert.equal(
    recordedActor("deployment-administrator", false, true),
    "デプロイメント管理者",
  );
});

test("a Run offers revocation only once it has done research", () => {
  const research = (search, inFlight = 0) => ({
    usage: { search_attempts: search, fetch_attempts: 0, sources: 0 },
    uncertain_attempts: 0,
    in_flight_attempts: inFlight,
  });
  assert.equal(hasResearch(research(0)), false);
  assert.equal(hasResearch(research(1)), true);
  assert.equal(hasResearch(research(0, 1)), true);
});
