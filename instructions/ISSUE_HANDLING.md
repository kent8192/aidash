# Batch Issue Handling

## Scope

These rules apply when processing multiple independent Issues as a batch.
A single feature or Issue follows the coherent-task and worktree-reuse rules
in [AGENTS.md](../AGENTS.md), including related changes across components.

## Work Units (MUST)

- Group batch Issues by component and cohesive fix pattern.
- Use one branch per independently reviewable component and fix pattern.
- Reuse each work unit's branch and worktree throughout implementation,
  verification, and follow-up fixes.

## Combining Issues (SHOULD)

- Combine related Issues in the same component when they share files, context,
  or interrelated fixes.
- Avoid combining unrelated fixes, different severity levels, or changes that
  would make the combined review too large (over 400 changed lines).
- The size guideline governs combining independent Issues; a single coherent
  task may span more than 400 changed lines.

## Shared Foundations (MUST)

- When independent batch fixes in multiple components require a shared API or
  utility change, put the minimal shared foundation on its own branch first.
- Integrate the foundation before applying it in each dependent work unit.
- Keep dependent branches focused on their component and fix pattern, and
  reference the shared foundation instead of duplicating its implementation.
