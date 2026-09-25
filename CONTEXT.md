# Aidash domain vocabulary

## Workspace

The logical collaboration boundary containing tasks, messages, artifacts, and an ordered event log. Its home node owns authoritative state. A Workspace is not a shared host filesystem directory.

## Run

A durable execution of a task by an exact Agent version. Its execution state and journal survive worker restarts. A Run is not a single model request or tool invocation.

## Harness

The runtime that advances Runs, assembles model context, exposes permitted capabilities, invokes tools, and coordinates authorization, execution history, and recovery.

## Core capability

A Harness-managed facility available without a Registry tool record or a `plugin_N` alias. File search, Code interpreter, Shell, Apply patch, and Skills are core capabilities. Availability does not grant permission to use a capability.

## Working directory

The filesystem location selected for an execution context. It is distinct from an Aidash Workspace and from the complete set of paths a sandbox permits. Its lifetime need not match one tool invocation.

## Agent working area

The authorized working files associated with one thread and one Agent, available to that Agent across successive Runs in the thread. Other Agents receive explicitly shared files rather than implicit access to the working area. This association does not override access policy.

## Working-area cleanup

The user-authorized removal of an Agent working area after a lifecycle boundary. It is distinct from stopping an Interpreter session, deleting a thread, or deleting separately published artifacts. Recovery-copy retention is a separate decision.

## Worktree

A separate Git checkout used to isolate file changes from another checkout. A Worktree is a working-directory option, not an operating-system security boundary.

## Interpreter session

A live code execution environment whose variables and import state can persist across invocations while the environment remains alive. Its memory state is distinct from the Agent working area's files.

## Idle suspension

Stopping an unused Interpreter session to release execution resources without deleting its working files. Subsequent execution starts a new session rather than preserving the old process memory.

## Session reset

The explicit loss of an Interpreter session's in-memory state after the environment stops or is recreated. A reset does not by itself mean that working files have been deleted.

## Skill

Reusable instructions described by a `SKILL.md` manifest, together with optional scripts, references, and assets. A Skill describes how to perform work; it does not grant execution, filesystem, or network permission.

## Skill discovery

Finding Skills in authorized locations and presenting their identifying metadata so an Agent can select relevant instructions.

## Skill activation

Loading a selected Skill's instructions into the Agent's context. Activation is distinct from executing a bundled script.

## Execution policy

The effective authorization and resource constraints governing a Run's capabilities and actions.

## Sandbox

The enforced execution boundary for processes, filesystem access, network access, and resource use. It is distinct from an approval decision.

## Approval

An authorized decision on a particular action or explicitly scoped permission request. Approval does not implicitly grant access beyond the applicable policy ceiling.

## Run-scoped approval

An explicit, time-limited authorization reusable only by the identified Run and Agent for the approved actions and targets. It is distinct from approval of a single invocation and does not transfer to another Run or Agent.

## Designated approver

A person assigned to review approval requests that the original requester is not authorized to approve. Assignment does not grant authority beyond the applicable policy ceiling.

## Registry integration

An externally supplied, versioned capability referenced through the Registry. Moving core capabilities into the Harness does not remove Registry integrations.
