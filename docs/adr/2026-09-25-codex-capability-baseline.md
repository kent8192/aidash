# Use Codex as the behavioral baseline for core capabilities

Date: 2026-09-25

Status: Accepted — behavioral baseline and Aidash integration decisions; not an implementation-complete specification.

Issue: [#43 — Move core agent capabilities into the harness](https://github.com/kent8192/aidash/issues/43)

## Context

Aidash is moving File search, Code interpreter, Shell, Apply patch, and Skills into the Harness. Designing each interaction independently would introduce avoidable differences from the requested Codex experience. Aidash also has versioned Agents, scoped authorization, independently operated nodes, and durable execution recovery that a local coding workflow does not replace.

## Decision

Use publicly documented Codex behavior as the reference for these capabilities. Adopt equivalent observable behavior where applicable, rather than inventing a separate interaction model. The baseline does not require embedding Codex, changing Aidash's model provider, matching every Codex wire protocol, or automatically adopting future upstream changes.

The reference material was reviewed on the date above. Codex Local, Worktree, and cloud environments are different execution surfaces; an implementation must identify which behavior it adopts rather than treating all of them as one specification. The working-area ownership, interpreter-state guarantees, and approval routing below are Aidash integration decisions, not claims about Codex's implementation.

### Working directories and ownership

Use an explicitly selected working directory or isolated checkout as the filesystem context. Codex supports both a local checkout and conversation-associated Worktrees. Therefore, Codex alignment does not mean creating a fresh directory for every tool call, sharing one directory across every Run of an Agent, or making an Aidash Workspace a shared filesystem.

Associate each Agent working area with one thread and one Agent. Successive Runs of that Agent in the same thread can use the existing working files. A different Agent receives only explicitly shared files; joining the same thread or Workspace does not grant access to another Agent's working area. Each accessing Run remains subject to its effective authorization.

This scope preserves continuity without making concurrent Agents implicit co-writers. Simultaneous Runs of the same Agent and Agent-version transitions require explicit lifecycle rules in the implementation specification. No concurrency mechanism is selected by this decision.

### Working-area cleanup

At lifecycle boundaries such as goal completion and the thread-deletion flow, offer buttons for the user to decide whether to remove the associated working area. Reaching a boundary or leaving the prompt unanswered does not itself authorize file deletion. Keep the decision explicit rather than deleting working files solely because a goal finished or an idle timeout elapsed.

The cleanup control must identify the working area it affects and remain separate from stopping an Interpreter session. The thread-deletion flow must resolve the cleanup choice before its controls become inaccessible. The treatment of published artifacts, recovery snapshots, and retained files after thread deletion requires a separate lifecycle contract; this decision does not silently select their deletion or indefinite retention.

This is an intentional difference from the documented Codex-managed Worktree lifecycle: Codex supports automatic cleanup on chat archival or retention-limit pressure and saves a snapshot before removal. Aidash adopts an explicit cleanup choice; Codex-style snapshot recovery has not yet been selected. See the Worktree cleanup section of the Codex reference below.

### Skills

Use directory-based Skills and progressive disclosure: discover permitted metadata first, then load the selected `SKILL.md` and supporting files when needed. Codex's repository and user `.agents/skills` locations are the format and discovery reference; Aidash must resolve equivalent locations inside authorized mounts, not scan the worker host's home directory indiscriminately.

Directory-based authoring and runtime content identity are separate concerns. Recording the content used by a Run does not require a Registry Skill record. Loading a Skill must not execute its scripts or expand permissions.

### Execution and approvals

Keep sandbox enforcement separate from approval policy. The ordinary Codex workspace-write workflow is the reference for permitted local edits and execution, with network access disabled unless granted. Aidash's applicable policy remains authoritative: an approval cannot bypass a tenant or node denial, and a capability is not exposed merely because the Harness implements it.

Route an additional-permission request to the original human requester when that person has authority to approve its scope. Otherwise, route it to a designated approver. Agent-to-Agent delegation does not change who the original requester is or confer approval authority. If no eligible approver exists, the requested action remains blocked.

Default the approval choice to the pending invocation only. An eligible approver may explicitly choose a time-limited grant reusable within the same Run for the approved Agent, actions, and targets. Such a grant expires when that Run ends or its recorded expiry is reached, whichever occurs first. It does not automatically carry into a delegated Agent or a subsequent Run. Revocation and stricter effective policy prevent subsequent invocations, even while the recorded grant has not expired. The concrete expiry duration remains an implementation parameter.

### File search, Shell, and Apply patch

Provide Harness-owned interfaces with documented input/output contracts and bounded results. Search and patch operate only on authorized files. Shell execution stays within the authorized execution environment. Patch failures and conflicts remain explicit.

The Issue's File search requirement does not require a hosted vector store.

### Code interpreter

Use a live Interpreter session that preserves variables and import state across invocations while its execution environment remains alive. If that environment stops or is recreated, explicitly report a session reset. Restoring in-memory state after an environment failure is not guaranteed. Working-file retention is a separate contract; losing variables does not imply losing saved files.

Automatically stop an Interpreter session after a configurable period of inactivity. Code that is still executing is not idle; execution-time limits are a separate policy. On the next invocation, create a new session and explicitly report the loss of variable and import state. Idle suspension leaves working files available under their cleanup policy.

Do not reconstruct the session by blindly replaying past code. Such code may have performed writes or external actions whose outcome is unknown. Recovery must preserve Aidash's existing tool-effect contract.

Provide a Harness-owned input/output contract, bounded outputs, and filesystem, network, and resource enforcement under the applicable policy. The separately documented OpenAI API Code Interpreter is not, by itself, a Codex specification. Its hosted API, Python session lifetime, and billing model are not implied dependencies. The concrete language/runtime and numeric idle timeout must be named in the implementation specification rather than inferred from the word "Codex".

## Aidash constraints retained

- Enabling a core capability does not implicitly enable it for every Agent or Run. Existing authorization boundaries remain in force.
- Tool invocations and outcomes remain part of the durable execution journal. An interrupted unsafe effect with an unknown outcome must not be blindly replayed.
- A remote Run does not gain implicit access to another node's filesystem, private references, or credentials. Transfers require an explicit authorized mechanism.
- Third-party Registry integrations remain supported. Removing the need for Registry-managed Skills does not authorize silently rewriting immutable Agent versions or dropping their existing Skills.

## Consequences

Thread-and-Agent-scoped working files support follow-up tasks without sharing unfinished mutable state with every participant. Explicit transfer is required when another Agent needs those files. Shared ownership across different requesters must still satisfy each Run's current authorization.

Live Interpreter sessions support iterative analysis without promising durable process memory. Tasks that must recover need saved intermediate results, and Agents must handle an explicit session reset rather than assuming old variables still exist.

Requester-first approval routing supports individual use while allowing organizations to assign authorized reviewers. It requires the system to retain the original requester through delegation and check approval authority, rather than treating thread membership or an approver label as permission.

Explicit cleanup choices avoid silently discarding working files at task boundaries. Automatic idle suspension releases process resources independently of that choice. Run-scoped approval reduces repeated prompts without turning a one-time decision into a permanent or cross-Agent permission.

The baseline reduces bespoke design choices while preserving Aidash's security and recovery contracts. Each deviation from Codex should have an Aidash-specific reason rather than an accidental implementation limitation.

This decision does not claim the five capabilities are implemented. Runtime selection, numeric resource limits and idle timeout, concurrency and version-transition rules, cleanup recovery and retained-file access after thread deletion, the treatment of published artifacts, concrete approval expiry, migration mechanics, and executable acceptance tests belong in the implementation specification.

## References

- [Codex Skills](https://developers.openai.com/codex/skills)
- [Codex Worktrees and cleanup](https://developers.openai.com/codex/app/worktrees#worktree-cleanup)
- [Codex agent approvals and security](https://developers.openai.com/codex/agent-approvals-security)
- [OpenAI API Code Interpreter](https://developers.openai.com/api/docs/guides/tools-code-interpreter)
- [Aidash architecture](../architecture.md)
- [Aidash Skills-first agents](../features/skills-first-agents.md)
- [Aidash protocol and recovery](../protocol.md)
