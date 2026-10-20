import { test } from "node:test";
import assert from "node:assert/strict";
import {
  isToolCard,
  toolCallView,
  toolGrantScope,
  toolLabel,
  toolStateText,
} from "../src/capabilities/tool-approval-model.ts";
import { approvalNotice } from "../src/research.ts";

const card = {
  kind: "tool_call",
  state: "pending",
  tool: "crm",
  version: "1.0.0",
  operation: "invoke",
  expires_at: "2099-01-01T00:00:00Z",
  targets: ["tool:crm@1.0.0#invoke"],
  agent_id: "researcher",
  agent_version: "1.0.0",
  requester: "alice",
};
const when = (value) => value ?? "";

test("only cards that name a tool are tool cards", () => {
  assert.equal(isToolCard(card), true);
  assert.equal(isToolCard({ ...card, kind: "grant" }), true);
  assert.equal(isToolCard({ tool: null }), false);
  assert.equal(isToolCard({ tool: "" }), false);
  assert.equal(isToolCard({}), false);
  assert.equal(toolLabel(card), "crm@1.0.0");
  assert.equal(toolLabel({ tool: "crm" }), "crm");
});

test("the call shows its name and its arguments exactly as recorded", () => {
  const view = toolCallView({
    id: "call-0",
    name: "crm",
    arguments: { account: "acme", fields: ["owner", "stage"] },
  });
  assert.equal(view.name, "crm");
  assert.deepEqual(JSON.parse(view.arguments), {
    account: "acme",
    fields: ["owner", "stage"],
  });
  assert.match(view.arguments, /\n {2}"account": "acme"/);
  assert.equal(
    toolCallView({ name: "crm", arguments: "raw text" }).arguments,
    "raw text",
  );
  assert.equal(toolCallView({ name: "crm", arguments: {} }).arguments, "{}");
  assert.deepEqual(toolCallView({ name: "crm" }), {
    name: "crm",
    arguments: null,
  });
  assert.deepEqual(toolCallView(null), { name: null, arguments: null });
  assert.deepEqual(toolCallView("crm"), { name: null, arguments: null });
});

test("a Run grant on a tool states that it covers other arguments and survives new input", () => {
  assert.equal(
    toolGrantScope(card, false),
    "Every later invoke call of crm@1.0.0 in this Run, whatever its arguments, until expiry. New user input does not end it",
  );
  assert.match(
    toolGrantScope(card, true),
    /crm@1\.0\.0 の invoke 呼び出しすべて/,
  );
  assert.match(toolGrantScope(card, true), /新しい入力では終了しません/);
});

test("a pending Tool call approval says the exact call runs once allowed", () => {
  assert.equal(
    toolStateText(card, false, when),
    "Awaiting a decision · expires 2099-01-01T00:00:00Z · once allowed, this exact call runs",
  );
  assert.match(
    toolStateText(card, true, when),
    /許可すると、この呼び出しがそのまま実行されます/,
  );
});

test("a refused call is reported as refused while the Run continues", () => {
  const decided = { decided_by: "bob", decided_at: "2026-10-19T00:00:00Z" };
  assert.equal(
    toolStateText({ ...card, ...decided, state: "denied" }, false, when),
    "Denied by bob at 2026-10-19T00:00:00Z. The call is refused and the Run continues.",
  );
  assert.equal(
    toolStateText({ ...card, state: "expired" }, false, when),
    "Expired. The call is refused and the Run continues.",
  );
  assert.equal(
    toolStateText({ ...card, state: "revoked" }, false, when),
    "Revoked. The call is refused and the Run continues.",
  );
  assert.equal(
    toolStateText({ ...card, state: "blocked" }, false, when),
    "Nobody in this Tenant may approve calls of crm@1.0.0. The call is refused and the Run continues.",
  );
  assert.match(
    toolStateText({ ...card, state: "denied" }, true, when),
    /呼び出しは拒否され、Run は続行します。$/,
  );
});

test("an allowed call runs as shown, and a closed one does not run", () => {
  assert.equal(
    toolStateText(
      {
        ...card,
        state: "approved",
        decided_by: "bob",
        decided_at: "2026-10-19T00:00:00Z",
      },
      false,
      when,
    ),
    "Allowed by bob at 2026-10-19T00:00:00Z. This exact call runs.",
  );
  assert.equal(
    toolStateText({ ...card, state: "consumed" }, false, when),
    "Allowed. This exact call runs.",
  );
  assert.equal(
    toolStateText(
      { ...card, state: "closed", closed_reason: "run_ended" },
      false,
      when,
    ),
    "Closed because the Run ended. The call does not run.",
  );
  assert.equal(
    toolStateText({ ...card, state: "closed" }, false, when),
    "Closed. The call does not run.",
  );
});

test("other cards keep their own state line", () => {
  assert.equal(toolStateText({ ...card, kind: "outbound" }, false, when), null);
  assert.equal(
    toolStateText({ ...card, kind: "grant", state: "active" }, false, when),
    null,
  );
  assert.equal(
    toolStateText({ ...card, state: "something_else" }, false, when),
    null,
  );
});

test("a notification names the tool and the Agent, never the arguments", () => {
  assert.equal(
    approvalNotice({ ...card, call: { arguments: { secret: "x" } } }, false),
    "Tool call · crm@1.0.0 · researcher@1.0.0 · requested by alice",
  );
  assert.equal(
    approvalNotice(card, true),
    "ツール呼び出し · crm@1.0.0 · researcher@1.0.0 · 依頼者 alice",
  );
});
