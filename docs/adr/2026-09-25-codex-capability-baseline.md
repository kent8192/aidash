# Use Codex as the behavioral baseline for core capabilities

Date: 2026-09-25

Status: Accepted — behavioral baseline; not an implementation-complete specification.

Issue: [#43 — Move core agent capabilities into the harness](https://github.com/kent8192/aidash/issues/43)

## Context

Aidash is moving File search, Code interpreter, Shell, Apply patch, and Skills into the Harness. Designing each interaction independently would introduce avoidable differences from the requested Codex experience. Aidash also has versioned Agents, scoped authorization, independently operated nodes, and durable execution recovery that a local coding workflow does not replace.

## Decision

Use publicly documented Codex behavior as the reference for these capabilities. Adopt equivalent observable behavior where applicable, rather than inventing a separate interaction model. The baseline does not require embedding Codex, changing Aidash's model provider, matching every Codex wire protocol, or automatically adopting future upstream changes.

The reference material was reviewed on the date above. Codex Local, Worktree, and cloud environments are different execution surfaces; an implementation must identify which behavior it adopts rather than treating all of them as one specification.

### Working directories

Use an explicitly selected working directory or isolated checkout as the filesystem context. Codex supports both a local checkout and conversation-associated Worktrees. Therefore, Codex alignment does not mean creating a fresh directory for every tool call, sharing one directory across every Run of an Agent, or making an Aidash Workspace a shared filesystem.

The concrete mapping from Aidash conversations and Runs to working-directory ownership and retention is an Aidash integration decision. Independent writers must not accidentally share mutable files merely because they participate in the same Workspace.

### Skills

Use directory-based Skills and progressive disclosure: discover permitted metadata first, then load the selected `SKILL.md` and supporting files when needed. Codex's repository and user `.agents/skills` locations are the format and discovery reference; Aidash must resolve equivalent locations inside authorized mounts, not scan the worker host's home directory indiscriminately.

Directory-based authoring and runtime content identity are separate concerns. Recording the content used by a Run does not require a Registry Skill record. Loading a Skill must not execute its scripts or expand permissions.

### Execution and approvals

Keep sandbox enforcement separate from approval policy. The ordinary Codex workspace-write workflow is the reference for permitted local edits and execution, with network access disabled unless granted. Aidash's applicable policy remains authoritative: an approval cannot bypass a tenant or node denial, and a capability is not exposed merely because the Harness implements it.

### File search, Shell, Apply patch, and Code interpreter

Provide Harness-owned interfaces with documented input/output contracts and bounded results. Search and patch operate only on authorized files. Shell and code execution stay within the authorized execution environment. Patch failures and conflicts remain explicit.

The Issue's File search requirement does not require a hosted vector store. The separately documented OpenAI API Code Interpreter is not, by itself, a Codex specification. Its hosted API, Python session lifetime, and billing model are not implied dependencies. A concrete interpreter runtime and state-lifetime contract must be named in the implementation specification rather than claimed to follow from the word "Codex".

## Aidash constraints retained

- Enabling a core capability does not implicitly enable it for every Agent or Run. Existing authorization boundaries remain in force.
- Tool invocations and outcomes remain part of the durable execution journal. An interrupted unsafe effect with an unknown outcome must not be blindly replayed.
- A remote Run does not gain implicit access to another node's filesystem, private references, or credentials. Transfers require an explicit authorized mechanism.
- Third-party Registry integrations remain supported. Removing the need for Registry-managed Skills does not authorize silently rewriting immutable Agent versions or dropping their existing Skills.

## Consequences

The baseline reduces bespoke design choices while preserving Aidash's security and recovery contracts. It also makes deviations reviewable: each difference should have an Aidash-specific reason rather than an accidental implementation limitation.

This decision does not claim the five capabilities are implemented. Runtime selection, numeric resource limits, working-directory lifecycle, migration mechanics, and executable acceptance tests belong in the implementation specification.

## References

- [Codex Skills](https://developers.openai.com/codex/skills)
- [Codex Worktrees](https://developers.openai.com/codex/app/worktrees)
- [Codex agent approvals and security](https://developers.openai.com/codex/agent-approvals-security)
- [OpenAI API Code Interpreter](https://developers.openai.com/api/docs/guides/tools-code-interpreter)
- [Aidash architecture](../architecture.md)
- [Aidash Skills-first agents](../features/skills-first-agents.md)
- [Aidash protocol and recovery](../protocol.md)
