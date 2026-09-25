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

## Worktree

A separate Git checkout used to isolate file changes from another checkout. A Worktree is a working-directory option, not an operating-system security boundary.

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

## Registry integration

An externally supplied, versioned capability referenced through the Registry. Moving core capabilities into the Harness does not remove Registry integrations.
