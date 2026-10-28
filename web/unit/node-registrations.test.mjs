import { test } from "node:test";
import assert from "node:assert/strict";
import {
  changedRegistration,
  inOtherDeployment,
  newRegistration,
} from "../src/node-registrations.ts";
import { deploymentId } from "../src/node-selection.ts";

const own = "aidash://home";
const draft = {
  source_node: " aidash://home/sibling ",
  endpoint: " https://other.example.com ",
  dashboard_origin: " https://dashboard.away.example.com ",
  key_id: " K ",
  atomic_transactions: false,
};

test("a Node address names its Deployment, and a Deployment names itself", () => {
  assert.equal(deploymentId("aidash://home/brave-otter"), own);
  assert.equal(deploymentId(own), own);
});

test("only a Node of another Deployment has an endpoint and a key to pin", () => {
  assert.equal(inOtherDeployment("aidash://home/a", own), false);
  assert.equal(inOtherDeployment("aidash://away/a", own), true);
  assert.equal(inOtherDeployment("not an address", own), false);
});

test("registering a Node of this Deployment sends no endpoint or key ID", () => {
  assert.deepEqual(newRegistration(draft, own), {
    source_node: "aidash://home/sibling",
    atomic_transactions: false,
    enabled: true,
    expected_revision: 0,
  });
});

test("registering a Node of another Deployment pins its endpoint and key ID", () => {
  assert.deepEqual(
    newRegistration(
      { ...draft, source_node: "aidash://away/a", atomic_transactions: true },
      own,
    ),
    {
      source_node: "aidash://away/a",
      atomic_transactions: true,
      enabled: true,
      expected_revision: 0,
      endpoint: "https://other.example.com",
      dashboard_origin: "https://dashboard.away.example.com",
      key_id: "K",
    },
  );
});

const record = (overrides = {}) => ({
  node: "mine",
  source_node: "aidash://away/a",
  endpoint: "https://other.example.com",
  dashboard_origin: "https://dashboard.away.example.com",
  key_id: "K",
  atomic_transactions: false,
  enabled: true,
  revision: 3,
  actor: "oidc:x",
  created_at: "2026-10-21T00:00:00Z",
  updated_at: "2026-10-21T00:00:00Z",
  ...overrides,
});

test("disabling replaces the revision it saw and needs no answer from the other Deployment", () => {
  assert.deepEqual(changedRegistration(record(), { enabled: false }), {
    source_node: "aidash://away/a",
    atomic_transactions: false,
    enabled: false,
    expected_revision: 3,
  });
});

test("enabling again sends back what the registration pinned", () => {
  assert.deepEqual(
    changedRegistration(record({ enabled: false }), { enabled: true }),
    {
      source_node: "aidash://away/a",
      atomic_transactions: false,
      enabled: true,
      expected_revision: 3,
      endpoint: "https://other.example.com",
      dashboard_origin: "https://dashboard.away.example.com",
      key_id: "K",
    },
  );
});

test("a Node of this Deployment never sends an endpoint", () => {
  assert.deepEqual(
    changedRegistration(
      record({ source_node: "aidash://home/b", endpoint: null, key_id: null }),
      { atomic_transactions: true },
    ),
    {
      source_node: "aidash://home/b",
      atomic_transactions: true,
      enabled: true,
      expected_revision: 3,
    },
  );
});
