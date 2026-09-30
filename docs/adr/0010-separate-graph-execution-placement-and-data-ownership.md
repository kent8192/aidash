---
status: accepted
---

# Separate Graph execution placement from Workspace data ownership

For [Issue #95](https://github.com/kent8192/aidash/issues/95), organize Agents and
Runs into peer-level Node execution regions, place authoritative Workspace data
in a separate Home-attributed group, and show referenced configuration separately.
Generic containment under the executing Node would incorrectly assign remote
work outputs to that executor; nesting Nodes under a Workspace would also imply
an ownership relationship that Aidash does not have.
This separation trades a single containment hierarchy for explicit, independent
placement and ownership meanings, shared by the five Workspace Graph View modes.

The [design interview](../design/2026-09-30-issue-95-node-regions.md) records the
accepted scope: communication-arrow work and the API expansion proposed for it
are excluded, and existing data ownership and authorization semantics remain in
force.
