---
status: accepted
---

# Whole-graph fit preserves the current view's scope

"Fit entire graph" is an overview of the bounded current graph, including its
off-screen elements; it must not widen filters, bypass presentation limits,
fetch remote pages or alter resource selection to create a larger graph.
The overview includes all rendered labels and group boundaries inside an
unobscured viewport, even when that requires a scale below ordinary zoom limits
and makes individual labels too small to read.
We choose consistent view-local camera semantics across Graph View and agent
views over implicitly expanding "entire" to all available data or excluding
elements to maintain detail readability; users retain manual zoom for inspection.

Accepted in Round 1 of the [Issue #96 design](../design/2026-09-30-issue-96-entire-graph-fit.md).
