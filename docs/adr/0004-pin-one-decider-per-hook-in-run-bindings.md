---
status: accepted
---

# Pin one explicit Decider per hook in Run Bindings

Each executable hook has at most one explicitly bound Decider whose exact definition, provider contract, model configuration, builder, and restrictions belong to the immutable Run Binding snapshot. Duplicate hooks and unsupported implementations are rejected instead of resolved by precedence or a latest-definition lookup. This sacrifices composition within a hook to make authority review, request grouping, and recovery unambiguous.
