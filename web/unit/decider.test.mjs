import { test } from "node:test";
import assert from "node:assert/strict";
import { deciderRecipients } from "../src/decider.ts";

test("a Decider's Routes name every host that receives Run state, in order", () => {
  assert.deepEqual(
    deciderRecipients({
      kind: "decider",
      config: {
        routes: [
          {
            provider_contract: "typesafe.jev/1",
            model: "typesafe/jev-1.14",
            hosts: [],
          },
          {
            provider_contract: "openrouter.decisions/1",
            model: "cloudflare/clef",
            hosts: ["cloudflare", "deepinfra", "primeintellect"],
          },
        ],
      },
    }),
    [
      { model: "typesafe/jev-1.14", hosts: ["typesafe"] },
      {
        model: "cloudflare/clef",
        hosts: ["cloudflare", "deepinfra", "primeintellect"],
      },
    ],
  );
});

test("other kinds and malformed Decider configurations show no recipients", () => {
  assert.equal(
    deciderRecipients({ kind: "model", config: { routes: [] } }),
    null,
  );
  assert.equal(deciderRecipients({ kind: "decider", config: {} }), null);
  assert.equal(
    deciderRecipients({ kind: "decider", config: { routes: [] } }),
    null,
  );
  assert.equal(
    deciderRecipients({ kind: "decider", config: { routes: [{ hosts: [] }] } }),
    null,
  );
});
