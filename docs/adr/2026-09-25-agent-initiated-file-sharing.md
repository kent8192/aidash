# Share immutable file snapshots across authorized nodes

Date: 2026-09-25

Status: Accepted

Issue: [#43 — Move core agent capabilities into the harness](https://github.com/kent8192/aidash/issues/43)

## Context

Agent working areas are scoped to a thread and an Agent. Collaboration requires selected files to reach another Agent without exposing the entire working area. Requiring a human to approve every transfer would interrupt otherwise authorized Agent-to-Agent work. Conversely, permission to run or delegate a task does not automatically grant permission to disclose its files.

## Decision

An Agent may invoke an explicit sharing operation that identifies the selected files and the recipient Agent. When effective policy permits that disclosure and the recipient's access, the Harness may complete the operation without a separate human approval for each transfer.

The Harness checks the sender's authority to share the selected files and the recipient's authority to receive them. It records the sender, recipient, selected files, authorization decision and outcome in the execution history. Thread or Workspace membership alone grants neither sharing authority nor access to another Agent's working area.

An operation that needs additional approval uses the existing requester-first approval routing and invocation-only or time-limited Run-scoped grant rules. An operation prohibited by the applicable policy remains denied; approval cannot override that ceiling.

### Immutable shared snapshots

Sharing fixes the selected files' contents at the time of the operation. Subsequent source edits do not change the shared snapshot. Sharing updated content requires another explicit operation. The recipient may work on its own copy without changing the sender's files or the identified shared snapshot.

Keep the shared content identifiable so execution history can distinguish which version a recipient used. A shared file snapshot is a collaboration input, not a cleanup recovery snapshot.

### Cross-node transfers

The scope includes same-node sharing and transfers to explicitly authorized recipient Agents on other nodes. Both nodes' applicable policies must permit the transfer; the sending side must authorize disclosure of these files to that destination. Peer connectivity or task-delegation permission alone is not file-transfer permission.

The transfer requires verified node and Agent identities, bounded file sizes, integrity verification, recoverable retries and durable receipt tracking. Do not report delivery as complete until the recipient has durably accepted the identified content. Failed or interrupted transfers must not expose partial files as completed inputs or silently substitute newer source content on retry.

Cross-node support is an implementation requirement, not a claim that the existing federation endpoints already satisfy this contract. If the required identity or authorization checks are unavailable, reject the transfer rather than falling back to a broader legacy permission. No remote database or general filesystem access is implied.

### Disclosure and cleanup boundaries

Sharing selected files does not confer general access to the source working directory, transfer credentials or approval grants, or bypass restrictions on private reference documents. No implicit transfer follows merely from Agent discovery or task delegation.

Deleting the sender's working area or cleanup recovery snapshots does not by itself erase copies delivered to a recipient. The receiving copy remains a separately held resource; withdrawal and deletion of already delivered copies require their own contract. Do not promise to recall content already read or copied outside the Harness.

## Consequences

Authorized collaboration can proceed without a human acting as a file-transfer intermediary. Disclosure remains an explicit, auditable action rather than a side effect of joining a thread.

Recipients receive stable inputs even when senders continue editing. Updated input requires re-sharing, and copies consume storage. Cross-node collaboration adds transfer recovery and recipient-side authorization to the implementation scope rather than relying on task delegation to carry files implicitly.

## Related decisions

- [Core capability baseline](2026-09-25-codex-capability-baseline.md)
- [Domain vocabulary](../../CONTEXT.md)
