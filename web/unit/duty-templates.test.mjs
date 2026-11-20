import { test } from "node:test";
import assert from "node:assert/strict";
import {
  agentChoices,
  defaultAgent,
  installBody,
  nextVersion,
  packageIdOf,
  publishBody,
  publishForm,
  publishProblems,
  templateOf,
  timeTriggerOf,
  toCreate,
} from "../src/duty-templates-model.ts";

function draft() {
  return {
    title: "GitHub CI duty",
    mission: "Fix failing CI runs.",
    procedure: "1. Read the failed job.",
    router: { language: "python", source: "print('{}')" },
    watcher: null,
    triggers: [
      {
        kind: "cron",
        expression: "0 9 * * 1-5",
        time_zone: "Asia/Tokyo",
        instruction: "",
      },
      { kind: "webhook", instruction: "" },
    ],
    agent: { id: "ci-fixer", version: "1.0.0" },
  };
}

test("a manifest of another kind carries no template", () => {
  assert.equal(
    templateOf({ entity: { kind: "skill", config: {} } }),
    undefined,
  );
  const template = templateOf({
    entity: { kind: "duty_template", config: draft() },
  });
  assert.equal(template?.mission, "Fix failing CI runs.");
  assert.deepEqual(timeTriggerOf(template), {
    kind: "cron",
    expression: "0 9 * * 1-5",
    time_zone: "Asia/Tokyo",
  });
  assert.deepEqual(
    toCreate(template).map((trigger) => trigger.kind),
    ["webhook"],
  );
});

test("the publish form starts from the Duty and needs an instruction for every trigger", () => {
  const form = publishForm(draft());
  assert.equal(form.packageId, "github-ci-duty");
  assert.equal(form.version, "1.0.0");
  assert.deepEqual(publishProblems(form, draft()).sort(), [
    "author",
    "instruction",
  ]);
  const filled = {
    ...form,
    author: " Acme ",
    instructions: {
      time: " Weekday mornings. ",
      webhook: "Add the URL as a workflow_run webhook.",
    },
  };
  assert.deepEqual(publishProblems(filled, draft()), []);
  assert.deepEqual(publishBody(filled, draft(), "key-1"), {
    package_id: "github-ci-duty",
    version: "1.0.0",
    author: "Acme",
    description: "",
    triggers: [
      { kind: "time", instruction: "Weekday mornings." },
      {
        kind: "webhook",
        instruction: "Add the URL as a workflow_run webhook.",
      },
    ],
    suggest_agent: true,
    idempotency_key: "key-1",
  });
  assert.deepEqual(
    publishProblems({ ...filled, version: "1.0", packageId: "a b" }, draft()),
    ["package_id", "version"],
  );
});

test("publishing again suggests the next minor version of the same package", () => {
  const form = publishForm(draft(), {
    key: "k",
    repository: "aidash://d/n",
    owner_tenant: "t",
    package_id: "ci",
    version: "1.4.2",
  });
  assert.equal(form.packageId, "ci");
  assert.equal(form.version, "1.5.0");
  assert.equal(nextVersion("not-semver"), "1.0.0");
  assert.equal(packageIdOf("職務"), "duty-template");
});

test("the suggested Agent is preselected only when the Node approved it", () => {
  const agents = [
    { id: "ci-fixer", version: "1.0.0" },
    { id: "reporter", version: "2.0.0" },
  ];
  const approvals = [
    { entry_id: "reporter", entry_version: "2.0.0", enabled: true },
    { entry_id: "ci-fixer", entry_version: "1.0.0", enabled: false },
  ];
  const choices = agentChoices(agents, approvals);
  assert.deepEqual(choices, [{ id: "reporter", version: "2.0.0" }]);
  assert.equal(defaultAgent(draft(), choices), undefined);
  assert.deepEqual(defaultAgent(draft(), agents), agents[0]);
  // Without readable approvals every readable Agent stays a choice.
  assert.deepEqual(agentChoices(agents), agents);
});

test("an install names the reviewed digest, the Thread and the chosen Agent", () => {
  assert.deepEqual(installBody("sha256:x", "t-1", undefined, "k"), {
    digest: "sha256:x",
    thread_id: "t-1",
    idempotency_key: "k",
  });
  assert.deepEqual(
    installBody("sha256:x", "t-1", { id: "a", version: "1.0.0" }, "k").agent,
    { id: "a", version: "1.0.0" },
  );
});
