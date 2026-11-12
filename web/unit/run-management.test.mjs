import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { createRequire } from "node:module";

const result = await build({
  entryPoints: [
    new URL("./run-management-render.tsx", import.meta.url).pathname,
  ],
  bundle: true,
  platform: "node",
  format: "cjs",
  write: false,
});
const module = { exports: {} };
new Function("require", "module", "exports", result.outputFiles[0].text)(
  createRequire(import.meta.url),
  module,
  module.exports,
);
const { renderManagement } = module.exports;

test("a decline-only holder sees no Run controls", () => {
  for (const control of ["ACTIVE", "PAUSED"]) {
    const html = renderManagement(control, false);
    assert.doesNotMatch(html, />\s*(Pause|Cancel execution|Resume)/);
  }
  assert.match(renderManagement("PAUSED", false), /Decline this call/);
  assert.match(renderManagement("ACTIVE", true), />Pause<\/button>/);
  assert.match(renderManagement("ACTIVE", true), />Cancel execution<\/button>/);
});
