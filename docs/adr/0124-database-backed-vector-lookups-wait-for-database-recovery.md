# Database-backed vector lookups wait for database recovery

Decided on 2026-11-09 in #787. A temporary database failure during a database-backed vector lookup follows the same uncounted wait and recovery path as other database failures (ADR 0113). It does not degrade an optional Workspace slot under ADR 0059 and does not pause the Run with retries exhausted. An external vector store outage remains a semantic outage and follows the existing bounded retry and degradation rules.

## Considered Options

- **Treat the database as a vector backend outage.** Rejected: the database also holds the Run and its authority, so the Run cannot safely proceed as if only optional context were unavailable.
- **Count repeated statement timeouts.** Rejected: #728 Q17 treats a statement timeout as a temporary database failure without an attempt limit.

## Consequences

- A lookup that hits PostgreSQL error `57014` every time waits indefinitely. This follows the unbounded database recovery rule and may require operator intervention.
- A recovered lookup retries without spending a Run attempt.
