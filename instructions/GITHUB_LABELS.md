# GitHub Labels

## Select Labels

Before creating or updating an Issue or PR, inspect the repository's current labels
and descriptions:

```bash
gh label list --repo kent8192/aidash --limit 200 --json name,description
```

Use exact names from this live inventory. Templates and the examples below may lag
behind it. Reuse existing labels; creating, renaming, or deleting repository labels
belongs in a task that explicitly changes the label taxonomy. If a narrower label
is unavailable, use the appropriate type label and describe the scope in the body.

For implementation Issues and PRs, choose one primary type based on the main
purpose of the work:

| Label           | Use for                                                                                           |
| --------------- | ------------------------------------------------------------------------------------------------- |
| `bug`           | Defects and their fixes, including regressions, broken CI, and performance defects.               |
| `enhancement`   | Features and improvements, including refactoring, maintenance, and optimization without a defect. |
| `documentation` | Work limited to documentation, instructions, or contribution templates.                           |

Use `question` for information requests. On implementation items, add it while
clarification is needed and remove it once the missing information is supplied.
Classify each PR from its actual changes rather than copying all labels from its
linked Issue. Supporting documentation or tests do not change a fix's primary type.

Add supplementary labels only when their meaning applies:

- `breaking-change`: add alongside the type label when existing APIs, stored data,
  configuration, or runtime behavior become incompatible, even if Rust signatures
  stay the same. Keep the PR's breaking-change assessment, migration guidance, and
  Conventional Commit `!` marker consistent with this label.
- `accessibility`: barriers affecting people with disabilities.
- `help wanted` or `good first issue`: an explicit request for help or a bounded
  task suitable for newcomers, respectively.
- `duplicate`, `invalid`, or `wontfix`: a recorded triage decision. Link the
  canonical Issue for duplicates and explain invalid or wontfix decisions. Closing
  an Issue or merging a PR alone does not justify these labels.

## Apply and Verify

1. Apply labels during creation with `gh issue create --label` or
   `gh pr create --label`; checking boxes or mentioning labels in a body does not
   apply them on GitHub.
2. When triaging an existing item, starting its implementation, or changing scope,
   inspect its current labels and reassess them. Use `gh issue edit --add-label`
   or `gh pr edit --add-label` for missing labels and `--remove-label` only for
   labels that no longer apply. Preserve other valid labels.
3. Before reporting completion, read back the labels using the commands below.
   Confirm the primary type and any supplementary labels match the final scope.
   If access or permissions prevent applying or verifying labels, report that
   specific blocker rather than claiming labeling is complete.

```bash
gh issue view <number> --repo kent8192/aidash --json number,url,labels
gh pr view <number> --repo kent8192/aidash --json number,url,labels
```
