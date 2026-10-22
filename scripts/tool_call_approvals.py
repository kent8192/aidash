"""Tool call approvals as the evaluation and acceptance scripts find and answer them (ADR 0075).

A Run on this Node that calls a Tenant's HTTP or MCP tool without direct permission raises a
capability approval of kind `tool_call` and waits. Its card names the registry `tool` and `version`,
the `operation` and the exact `call` (`{name, arguments}`). The Run's subject is the designated
approver unless the Tenant names another person, so a script holding that subject's token lists
the card and decides it. A Run executing for another Node still asks its Home Node through a
human request, which these helpers never see.
"""
import uuid

PENDING = "/api/capabilities/approvals?state=pending&role=approver"
# What the model is told when a call is refused before it runs: `Refusal::message` in
# crates/aidash-application/src/ports/capabilities/approvals.rs, and the unscoped and Home
# Node cases in crates/aidash-harness/src/agent.rs. test_fixed_task_evaluation.py checks them.
REFUSALS = (
    "tool call was refused by the Tenant's policy",
    "tool call was refused by an Approval Rule",
    "tool call was refused: the person asked not to be asked",
    "tool call was blocked by Auto: data_exfiltration",
    "tool call was blocked by Auto: destructive",
    "tool call was blocked by Auto: beyond_request",
    "tool call was blocked by Auto: injected",
    "tool call was blocked by Auto: boundary",
    "tool call was blocked by Auto: unavailable",
    "tool call approval was refused: no person is eligible to approve it",
    "tool call approval was denied",
    "tool call approval expired",
    "tool call approval was revoked",
    "tool call approval was closed",
    "tool call approval is unavailable without scoped authority",
    "tool call approval was denied or expired",
)


def pending(get, run_id=None):
    """The pending Tool call approval cards addressed to the caller, of one Run when `run_id` is given.

    `get(path)` returns the decoded JSON of a GET. The list is paged by `next_cursor`.
    """
    cards, path = [], PENDING
    while True:
        page = get(path)
        cards += [card for card in page["items"]
                  if card["kind"] == "tool_call" and (run_id is None or card["run_id"] == run_id)]
        if page.get("next_cursor") is None:
            return cards
        path = f"{PENDING}&cursor={page['next_cursor']}"


def call_name(card):
    """The name of the tool call a card asks about (the Agent's alias for the tool), else None."""
    call = card.get("call")
    return call.get("name") if isinstance(call, dict) else None


def decide(post, card, approved):
    """Allow the card's exact call once, or deny it. `post(path, body)` sends a POST."""
    post(f"/api/capabilities/approvals/{card['id']}/decide", {
        "idempotency_key": str(uuid.uuid4()), "expected_revision": card["revision"],
        "choice": "allow_once" if approved else "deny"})
