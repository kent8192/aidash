import assert from "node:assert/strict";
import test from "node:test";
import { bundleForTest } from "./bundle-for-test.mjs";

const { renderManagement, renderOutageWait } = await bundleForTest(
  import.meta.url,
  {
    entryPoints: [
      new URL("./run-management-render.tsx", import.meta.url).pathname,
    ],
  },
);

test("a decline-only holder sees no Run controls", () => {
  for (const control of ["ACTIVE", "PAUSED"]) {
    const html = renderManagement(control, false);
    assert.doesNotMatch(html, />\s*(Pause|Cancel execution|Resume)/);
  }
  assert.match(renderManagement("PAUSED", false), /Decline this call/);
  assert.match(renderManagement("ACTIVE", true), />Pause<\/button>/);
  assert.match(renderManagement("ACTIVE", true), />Cancel execution<\/button>/);
});

test("Outage Wait facts disappear when a waiting Run is paused", () => {
  const active = renderOutageWait("ACTIVE");
  assert.match(active, /Outage Wait: Decider/);
  assert.match(active, /Next try/);
  assert.equal(renderOutageWait("PAUSED"), "");
});
