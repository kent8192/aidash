import { test } from "node:test";
import assert from "node:assert/strict";
import {
  settingsSectionsFor,
  tenantContentSettingsSections,
} from "../src/collaboration/model.ts";

const deploymentOnly = { deployment: true };

test("a Deployment Administrator sees no Tenant content settings", () => {
  const sections = settingsSectionsFor(true, deploymentOnly);
  for (const hidden of Object.keys(tenantContentSettingsSections))
    assert.ok(!sections.includes(hidden), `${hidden} stays hidden`);
  assert.ok(!sections.includes("approvals"));
  assert.ok(!sections.includes("planConnections"));
});

test("a Deployment Administrator keeps Deployment-level settings", () => {
  const sections = settingsSectionsFor(true, deploymentOnly);
  for (const kept of ["node", "deployment", "marketplace"])
    assert.ok(sections.includes(kept), `${kept} stays available`);
});

test("a Mapping keeps Tenant settings and loses Deployment settings", () => {
  const sections = settingsSectionsFor(false, deploymentOnly);
  for (const kept of ["agents", "registry", "providerCredentials", "nodes"])
    assert.ok(sections.includes(kept), `${kept} stays available`);
  assert.ok(!sections.includes("deployment"));
});
