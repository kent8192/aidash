import { test } from "node:test";
import assert from "node:assert/strict";
import {
  readinessKindLabel,
  readinessStatusLabel,
  readinessTone,
} from "../src/capabilities/secret-readiness-model.ts";

const statuses = [
  "ready",
  "secret_missing",
  "secret_hosts_mismatch",
  "secrets_unavailable_on_deployment",
  "broker_not_configured",
];

test("every readiness status has an English and Japanese label", () => {
  for (const status of statuses) {
    for (const locale of ["en-US", "ja-JP"]) {
      const label = readinessStatusLabel(status, locale);
      assert.equal(typeof label, "string");
      assert.ok(label.length > 0, `${locale} ${status}`);
    }
  }
  assert.equal(readinessStatusLabel("ready", "en-US"), "Ready");
  assert.equal(readinessStatusLabel("ready", "ja-JP"), "準備完了");
});

test("readiness kind labels distinguish tools, services and package slots", () => {
  assert.equal(readinessKindLabel("tool", "en-US"), "Tool");
  assert.equal(
    readinessKindLabel("external_service", "en-US"),
    "External service",
  );
  assert.equal(readinessKindLabel("slot", "ja-JP"), "Secret スロット");
});

test("ready is the only success tone; missing secrets warn and deployment faults fail", () => {
  assert.equal(readinessTone("ready"), "success");
  assert.equal(readinessTone("secret_missing"), "warning");
  assert.equal(readinessTone("secret_hosts_mismatch"), "warning");
  assert.equal(readinessTone("secrets_unavailable_on_deployment"), "danger");
  assert.equal(readinessTone("broker_not_configured"), "danger");
});
