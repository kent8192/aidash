# Permit policy-authorized Agent-initiated file sharing

Date: 2026-09-25

Status: Accepted

Issue: [#43 — Move core agent capabilities into the harness](https://github.com/kent8192/aidash/issues/43)

## Context

Agent working areas are scoped to a thread and an Agent. Collaboration requires selected files to reach another Agent without exposing the entire working area. Requiring a human to approve every transfer would interrupt otherwise authorized Agent-to-Agent work. Conversely, permission to run or delegate a task does not automatically grant permission to disclose its files.

## Decision

An Agent may invoke an explicit sharing operation that identifies the selected files and the recipient Agent. When effective policy permits that disclosure and the recipient's access, the Harness may complete the operation without a separate human approval for each transfer.

The Harness checks the sender's authority to share the selected files and the recipient's authority to receive them. It records the sender, recipient, selected files, authorization decision and outcome in the execution history. Thread or Workspace membership alone grants neither sharing authority nor access to another Agent's working area.

An operation that needs additional approval uses the existing requester-first approval routing and invocation-only or time-limited Run-scoped grant rules. An operation prohibited by the applicable policy remains denied; approval cannot override that ceiling.

Sharing selected files does not confer general access to the source working directory, transfer credentials or approval grants, or bypass restrictions on private reference documents. No implicit transfer follows merely from Agent discovery or task delegation.

## Consequences

Authorized collaboration can proceed without a human acting as a file-transfer intermediary. Disclosure remains an explicit, auditable action rather than a side effect of joining a thread.

This decision establishes who can initiate a permitted transfer. It does not select snapshot versus live-reference semantics, expand the supported federation boundary, or define revocation of already delivered copies. Those contracts must be specified separately.

## Related decisions

- [Core capability baseline](2026-09-25-codex-capability-baseline.md)
- [Domain vocabulary](../../CONTEXT.md)
